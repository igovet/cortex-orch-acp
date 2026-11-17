//! Disposable owner settings only: no account files, host registry, engines or paid calls.
#[path = "support/hermetic.rs"]
mod hermetic;
use cortex_core::CoreError;
use serde_json::{json, Value};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};
use workspace_service::native_routes::{RouteEnvironment, RouteResolver};

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
struct Fixture {
    base: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "native-routes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        for path in [
            "",
            "home",
            "config",
            "config/cortex",
            "state",
            "state/cortex",
            "state/cortex/catalog",
            "state/cortex/catalog/cn_fixture",
            "project",
            "project/.cortex-dev",
            "credentials",
        ] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(base.join(path))
                .unwrap();
        }
        let f = Self { base };
        f.write(
            "config/cortex/connections.json",
            &json!({"schema_version":1,"connections":{
            "cn_fixture":f.record()}}),
        );
        f.write("config/cortex/routing.json", &f.rules());
        f.write(
            "config/cortex/defaults.json",
            &json!({"defaults":{"coordinator":{
            "engine":"codex-acp","model":"fixture-model","parameters":{}}}}),
        );
        f.write(
            "state/cortex/catalog/cn_fixture/cs_fixture.json",
            &f.snapshot("cs_fixture"),
        );
        f
    }
    fn record(&self) -> Value {
        let mut r = json!({"id":"cn_fixture","engine":"codex-acp","provider":"fixture","endpoint":null,
            "launch_command":["/bin/true"],"auth_mode":"native_account","qualified":true,"state":"configured",
            "credential_ref":{"kind":"engine_native_dir","path":self.base.join("credentials"),"owned":false},
            "max_concurrent_sessions":4});
        Self::approve(&mut r);
        r
    }
    fn approve(r: &mut Value) {
        r["approved_fingerprint"]=json!(digest(&serde_json::to_vec(&json!({
            "engine":r["engine"],"provider":r["provider"],"endpoint":r["endpoint"],"launch_command":r["launch_command"]
        })).unwrap()));
    }
    fn rules(&self) -> Value {
        json!({"model_policy_version":2,"enabled":{"codex-acp":["fixture-model"]},
        "models":{"codex-acp":{"auto":["fixture-model"],"tiers":{"fixture-model":["light","standard"]}}},
        "pin":{"general":"codex-acp/fixture-model"}})
    }
    fn snapshot(&self, id: &str) -> Value {
        let mut route = json!({"engine":"codex-acp","connection_id":"cn_fixture","model":"fixture-model",
            "supported_parameters":[["thought_level","high"]]});
        let hash = digest(&serde_json::to_vec(&route).unwrap());
        route["id"] = json!(format!("xr_{}", &hash[..24]));
        json!({"snapshot_id":id,"unavailable_reason":null,"entries":[{
            "route":route,"connection_id":"cn_fixture","catalog_snapshot_id":id,
            "evidence":{"catalog_obtained":{"source":"discovery","adapter_version":"fixture",
                "observed_at":"2026-11-12T00:00:00Z","evidence_ref":"no-paid-fixture"}}
        }]})
    }
    fn write(&self, path: &str, value: &Value) {
        let path = self.base.join(path);
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn environment(&self) -> RouteEnvironment {
        RouteEnvironment {
            home: self.base.join("home"),
            config: self.base.join("config"),
            state: self.base.join("state"),
            routing_override: None,
        }
    }
    fn load(&self) -> Result<RouteResolver, CoreError> {
        RouteResolver::load(
            &self.base.join("project"),
            &self.base.join("project/.cortex-dev"),
            self.environment(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).unwrap();
    }
}
fn code<T>(value: Result<T, CoreError>) -> String {
    match value {
        Ok(_) => panic!("expected refusal"),
        Err(e) => e.code().into(),
    }
}
#[test]
fn exact_portable_and_saved_coordinator_binding_and_native_argv() {
    let f = Fixture::new();
    let selected = f.load().unwrap().coordinator().unwrap();
    assert_eq!(selected.connection(), "cn_fixture");
    assert_eq!(selected.model(), "fixture-model");
    let launch = selected.launch(&f.base.join("project")).unwrap();
    assert_eq!(
        launch.executable,
        Path::new("/bin/true").canonicalize().unwrap()
    );
    assert!(launch.args.is_empty());
    assert_eq!(launch.native_home, f.base.join("home"));
    f.write(
        "config/cortex/defaults.json",
        &json!({"defaults":{
        "route_connection_id":"cn_fixture","route_model":"fixture-model",
        "route_id":"obsolete","parameters":{"thought_level":"high"}}}),
    );
    let selected = f.load().unwrap().coordinator().unwrap();
    assert_eq!(selected.parameters["thought_level"], "high");
    assert_ne!(selected.route_id(), "obsolete");
}
#[test]
fn per_task_model_and_effort_require_existing_exact_eligible_catalog_values() {
    use workspace_service::{native_routes::ConfiguredRoutes, native_runtime::NativeRoutes};
    let f = Fixture::new();
    let mut rules = f.rules();
    rules["pin"] = json!({"*":"codex-acp/fixture-model:high"});
    rules["enabled"]["codex-acp"] = json!(["fixture-model", "fixture-other"]);
    rules["models"]["codex-acp"]["tiers"]["fixture-other"] = json!(["standard"]);
    f.write("config/cortex/routing.json", &rules);
    let mut snapshot = f.snapshot("cs_fixture");
    let mut other = snapshot["entries"][0].clone();
    other["route"]["model"] = json!("fixture-other");
    other["route"].as_object_mut().unwrap().remove("id");
    other["route"]["id"] = json!(format!(
        "xr_{}",
        &digest(&serde_json::to_vec(&other["route"]).unwrap())[..24]
    ));
    snapshot["entries"].as_array_mut().unwrap().push(other);
    f.write("state/cortex/catalog/cn_fixture/cs_fixture.json", &snapshot);
    let provider = ConfiguredRoutes {
        environment: f.environment(),
        relay: PathBuf::from("/bin/true"),
    };
    // The real task composer submits routeId, not the display model name.
    let settings = json!({"model":snapshot["entries"][1]["route"]["id"],"effort":"high","mode":"guarded","language":"ru"});
    let selected = provider
        .task_route(&f.base.join("project"), &settings)
        .unwrap();
    assert_eq!(selected.model, "fixture-other");
    assert_eq!(selected.parameters["thought_level"], "high");
    let mut invalid = settings.clone();
    invalid["effort"] = json!("invented");
    assert_eq!(
        code(provider.task_route(&f.base.join("project"), &invalid)),
        "unsupported_dependent_setting"
    );
    rules["deny"] = json!(["codex-acp/fixture-other"]);
    f.write("config/cortex/routing.json", &rules);
    assert_eq!(
        code(provider.task_route(&f.base.join("project"), &settings)),
        "native_task_route_unavailable"
    );
}
/// Every session the route provider starts (coordinator here; workers,
/// review, verification and planning use the same path) pins the native Codex
/// launcher for an install-shaped codex-acp launch, so App Server gets
/// non-persistent project trust and never writes the owner's config.toml.
/// The launcher is written, or an unlocatable bundled Codex refused, only
/// when the engine starts: resolving the route itself has no effect.
#[test]
fn codex_sessions_pin_the_native_launcher_and_other_launches_stay_unchanged() {
    use workspace_service::{native_routes::ConfiguredRoutes, native_runtime::NativeRoutes};
    let f = Fixture::new();
    let provider = ConfiguredRoutes {
        environment: f.environment(),
        relay: PathBuf::from("/bin/true"),
    };
    let project = f.base.join("project");
    // Not install-shaped (a fake-engine double): launched unchanged, as in Python.
    let route = provider.route(&project, "orchestrator").unwrap();
    assert!(route.launch.codex_launcher.is_none());
    assert!(!route
        .launch
        .launch_environment()
        .unwrap()
        .contains_key("CODEX_PATH"));
    let base = f.base.canonicalize().unwrap();
    let modules = base.join("engines/node_modules");
    let package = modules.join("@agentclientprotocol/codex-acp");
    let binary = modules.join("@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex");
    for (path, body, mode) in [
        (
            package.join("package.json"),
            r#"{"version":"1.12.0","bin":{"codex-acp":"dist/index.js"}}"#,
            0o600,
        ),
        (package.join("dist/index.js"), "// fixture", 0o600),
        (
            modules.join("@openai/codex/package.json"),
            r#"{"version":"0.164.0"}"#,
            0o600,
        ),
        (binary.clone(), "#!/bin/sh\nexit 1\n", 0o700),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let mut record = f.record();
    record["launch_command"] = json!(["/bin/true", package.join("dist/index.js")]);
    Fixture::approve(&mut record);
    f.write(
        "config/cortex/connections.json",
        &json!({"schema_version":1,"connections":{"cn_fixture":record}}),
    );
    let route = provider.route(&project, "orchestrator").unwrap();
    let home = f
        .base
        .join("state/cortex/connections/cn_fixture/codex-home");
    assert!(!home.exists(), "route resolution wrote launcher state");
    let environment = route.launch.launch_environment().unwrap();
    let launcher = PathBuf::from(&environment["CODEX_PATH"]);
    assert_eq!(environment["NO_BROWSER"], "1");
    assert!(launcher.starts_with(
        f.base
            .join("state/cortex/connections/cn_fixture/codex-home")
    ));
    let body = workspace_service::native_codex_runtime::artifact(
        &std::env::current_exe().unwrap(),
        &binary,
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&launcher).unwrap(), body);
    assert_eq!(
        std::fs::metadata(&launcher).unwrap().permissions().mode() & 0o777,
        0o700
    );
    // Repeated launches reuse the same content-addressed artifact.
    let again = provider.route(&project, "orchestrator").unwrap();
    assert_eq!(
        again.launch.launch_environment().unwrap()["CODEX_PATH"],
        environment["CODEX_PATH"]
    );
    // An install-shaped launch whose bundled Codex is missing never starts
    // unguarded, while route selection itself is unaffected.
    std::fs::remove_file(&binary).unwrap();
    let route = provider.route(&project, "orchestrator").unwrap();
    assert!(route.launch.launch_environment().is_err());
}
#[test]
fn providers_are_worker_only_and_deny_wins_over_a_pin() {
    let f = Fixture::new();
    let mut rules = f.rules();
    rules["providers"] = json!(["claude-agent-acp"]);
    f.write("config/cortex/routing.json", &rules);
    assert!(f.load().unwrap().coordinator().is_ok());
    assert_eq!(
        code(f.load().unwrap().worker("worker-general")),
        "pinned_route_unavailable"
    );
    rules["providers"] = json!(["codex-acp"]);
    rules["deny"] = json!(["codex-acp/*"]);
    f.write("config/cortex/routing.json", &rules);
    assert_eq!(
        code(f.load().unwrap().coordinator()),
        "portable_coordinator_unavailable"
    );
    assert_eq!(
        code(f.load().unwrap().worker("worker-general")),
        "pinned_route_unavailable"
    );
}
#[test]
fn first_existing_policy_is_not_merged_and_malformed_does_not_fall_through() {
    let f = Fixture::new();
    let mut rules = f.rules();
    rules["enabled"] = json!({});
    f.write("project/.cortex-dev/routing.json", &rules);
    assert_eq!(
        code(f.load().unwrap().coordinator()),
        "portable_coordinator_unavailable"
    );
    std::fs::write(f.base.join("project/.cortex-dev/routing.json"), b"{").unwrap();
    assert_eq!(code(f.load()), "native_route_settings_invalid");
}
#[test]
fn explicit_override_precedes_user_policy_and_project_precedes_override() {
    let f = Fixture::new();
    let mut r = f.rules();
    r["deny"] = json!(["fixture-model"]);
    f.write("override.json", &r);
    let mut env = f.environment();
    env.routing_override = Some(f.base.join("override.json"));
    let load = || {
        RouteResolver::load(
            &f.base.join("project"),
            &f.base.join("project/.cortex-dev"),
            env.clone(),
        )
    };
    assert_eq!(
        code(load().unwrap().coordinator()),
        "portable_coordinator_unavailable"
    );
    f.write("project/.cortex-dev/routing.json", &f.rules());
    assert!(load().unwrap().coordinator().is_ok());
}
#[test]
fn missing_policy_version_and_invalid_automatic_tiers_fail_closed() {
    let f = Fixture::new();
    f.write("config/cortex/routing.json", &json!({}));
    assert_eq!(code(f.load()), "model_policy_migration_required");
    let mut r = f.rules();
    r["models"]["codex-acp"]["tiers"] = json!({});
    f.write("config/cortex/routing.json", &r);
    assert_eq!(code(f.load()), "policy_missing");
}
#[test]
fn exact_role_then_alias_then_wildcard_and_observed_parameter_bundle() {
    let f = Fixture::new();
    let mut r = f.rules();
    r["pin"] = json!({"worker-general":"codex-acp/fixture-model:high",
        "general":"codex-acp/absent","*":"codex-acp/absent"});
    f.write("config/cortex/routing.json", &r);
    let s = f.load().unwrap().worker("worker-general").unwrap();
    assert_eq!(s.parameters["thought_level"], "high");
    r["pin"] = json!({"general":"codex-acp/fixture-model:unsupported"});
    f.write("config/cortex/routing.json", &r);
    assert_eq!(
        code(f.load().unwrap().worker("worker-general")),
        "pinned_effort_unavailable"
    );
    f.write("config/cortex/defaults.json",&json!({"defaults":{"coordinator":{
        "engine":"codex-acp","model":"fixture-model","parameters":{"thought_level":"high","mode":"absent"}}}}));
    assert_eq!(
        code(f.load().unwrap().coordinator()),
        "unsupported_dependent_setting"
    );
}
#[test]
fn automatic_ranking_and_configured_effort_policy_are_supported() {
    let f = Fixture::new();
    let mut r = f.rules();
    r["pin"] = json!({});
    f.write("config/cortex/routing.json", &r);
    assert_eq!(
        f.load().unwrap().worker("worker-general").unwrap().model(),
        "fixture-model"
    );
    r["pin"] = json!({"*":"codex-acp/fixture-model"});
    r["models"]["codex-acp"]["policy"] = json!({"fixture-model":{"max_effort":"high"}});
    f.write("config/cortex/routing.json", &r);
    assert_eq!(
        f.load()
            .unwrap()
            .worker("worker-general")
            .unwrap()
            .parameters["thought_level"],
        "high"
    );
}
#[test]
fn changed_command_is_not_owner_approved_and_relative_paths_have_no_project_fallback() {
    let f = Fixture::new();
    let mut r = f.record();
    r["launch_command"] = json!(["/bin/false"]);
    f.write(
        "config/cortex/connections.json",
        &json!({"schema_version":1,"connections":{"cn_fixture":r}}),
    );
    assert_eq!(code(f.load()), "untrusted_project_override");
    r["launch_command"] = json!(["./true"]);
    Fixture::approve(&mut r);
    f.write(
        "config/cortex/connections.json",
        &json!({"schema_version":1,"connections":{"cn_fixture":r}}),
    );
    assert_eq!(
        code(
            f.load()
                .unwrap()
                .coordinator()
                .unwrap()
                .launch(&f.base.join("project"))
        ),
        "native_route_executable_unavailable"
    );
}
#[test]
fn no_seed_auth_required_wrong_binding_or_forged_route_can_be_selected() {
    let f = Fixture::new();
    for source in ["seed", "manual"] {
        let mut s = f.snapshot("cs_fixture");
        s["entries"][0]["evidence"]["catalog_obtained"]["source"] = json!(source);
        f.write("state/cortex/catalog/cn_fixture/cs_fixture.json", &s);
        assert_eq!(
            code(f.load().unwrap().coordinator()),
            "portable_coordinator_unavailable"
        );
    }
    let mut s = f.snapshot("cs_fixture");
    s["entries"][0]["evidence"]["authorization_required"] = json!({"source":"observation"});
    f.write("state/cortex/catalog/cn_fixture/cs_fixture.json", &s);
    assert_eq!(
        code(f.load().unwrap().coordinator()),
        "portable_coordinator_unavailable"
    );
    for key in ["connection_id", "catalog_snapshot_id"] {
        let mut s = f.snapshot("cs_fixture");
        s["entries"][0][key] = json!("wrong");
        f.write("state/cortex/catalog/cn_fixture/cs_fixture.json", &s);
        assert_eq!(code(f.load()), "invalid_catalog_snapshot");
    }
    let mut s = f.snapshot("cs_fixture");
    s["entries"][0]["route"]["id"] = json!("xr_forged");
    f.write("state/cortex/catalog/cn_fixture/cs_fixture.json", &s);
    assert_eq!(code(f.load()), "invalid_catalog_snapshot");
}
#[test]
fn newest_placeholder_does_not_hide_a_genuine_snapshot_but_withdrawal_does() {
    let f = Fixture::new();
    // Explicit mtimes, no sleeps or filesystem timestamp-resolution assumption.
    let set_newest = |p: &str| {
        let file = std::fs::File::open(f.base.join(p)).unwrap();
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10)),
        )
        .unwrap();
    };
    f.write(
        "state/cortex/catalog/cn_fixture/cs_new.json",
        &json!({
        "snapshot_id":"cs_new","entries":[],"unavailable_reason":"unrelated_connection"}),
    );
    set_newest("state/cortex/catalog/cn_fixture/cs_new.json");
    assert!(f.load().unwrap().coordinator().is_ok());
    f.write(
        "state/cortex/catalog/cn_fixture/cs_new.json",
        &json!({
        "snapshot_id":"cs_new","entries":[],"unavailable_reason":"no_models_observed"}),
    );
    set_newest("state/cortex/catalog/cn_fixture/cs_new.json");
    assert_eq!(
        code(f.load().unwrap().coordinator()),
        "portable_coordinator_unavailable"
    );
}
#[test]
fn private_registry_refuses_public_modes_symlinks_and_hardlinks() {
    let f = Fixture::new();
    let path = f.base.join("config/cortex/connections.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(code(f.load()), "native_route_settings_unreadable");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&path, f.base.join("hardlink")).unwrap();
    assert_eq!(code(f.load()), "native_route_settings_unreadable");
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(f.base.join("hardlink"), &path).unwrap();
    assert_eq!(code(f.load()), "native_route_settings_unreadable");
}
#[test]
fn missing_credential_directory_and_non_native_auth_have_no_launch_fallback() {
    let f = Fixture::new();
    let mut r = f.record();
    r["credential_ref"]["path"] = json!(f.base.join("missing"));
    f.write(
        "config/cortex/connections.json",
        &json!({"schema_version":1,"connections":{"cn_fixture":r}}),
    );
    assert_eq!(
        code(
            f.load()
                .unwrap()
                .coordinator()
                .unwrap()
                .launch(&f.base.join("project"))
        ),
        "authorization_required"
    );
    r["auth_mode"] = json!("api_key");
    f.write(
        "config/cortex/connections.json",
        &json!({"schema_version":1,"connections":{"cn_fixture":r}}),
    );
    assert_eq!(
        code(f.load().unwrap().coordinator()),
        "portable_coordinator_unavailable"
    );
}
#[test]
fn missing_effort_unknown_role_and_unsafe_parent_have_explicit_refusals() {
    let f = Fixture::new();
    assert_eq!(
        f.load()
            .unwrap()
            .worker("worker-general")
            .unwrap()
            .parameters["thought_level"],
        "high"
    );
    assert_eq!(
        code(f.load().unwrap().worker("worker-unknown")),
        "unknown_session_role"
    );
    std::fs::set_permissions(
        f.base.join("config/cortex"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(code(f.load()), "native_route_settings_unreadable");
}
#[test]
fn missing_first_run_roots_report_missing_policy_without_creating_files() {
    let f = Fixture::new();
    let mut environment = f.environment();
    environment.config = f.base.join("not-created-config");
    environment.state = f.base.join("not-created-state");
    assert_eq!(
        code(RouteResolver::load(
            &f.base.join("project"),
            &f.base.join("project/.cortex-dev"),
            environment.clone()
        )),
        "model_policy_migration_required"
    );
    assert!(!environment.config.exists());
    assert!(!environment.state.exists());
}
#[test]
fn configured_provider_qualifies_all_engine_argv_without_running_them() {
    use workspace_service::native_runtime::NativeRoutes;
    for engine in ["codex-acp", "claude-agent-acp", "opencode"] {
        let f = Fixture::new();
        let marker = f.base.join("should-not-exist");
        let executable = f.base.join("fake-engine");
        std::fs::write(
            &executable,
            format!("#!/bin/sh\ntouch '{}'\nexit 99\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let credentials = f.base.join("credentials/opencode");
        std::fs::create_dir(&credentials).unwrap();
        let mut record = f.record();
        record["engine"] = json!(engine);
        record["launch_command"] = json!([executable]);
        record["credential_ref"]["path"] = json!(if engine == "opencode" {
            credentials
        } else {
            f.base.join("credentials")
        });
        Fixture::approve(&mut record);
        f.write(
            "config/cortex/connections.json",
            &json!({"schema_version":1,"connections":{"cn_fixture":record}}),
        );
        let mut snapshot = f.snapshot("cs_fixture");
        snapshot["entries"][0]["route"]["engine"] = json!(engine);
        snapshot["entries"][0]["route"]
            .as_object_mut()
            .unwrap()
            .remove("id");
        let hash = digest(&serde_json::to_vec(&snapshot["entries"][0]["route"]).unwrap());
        snapshot["entries"][0]["route"]["id"] = json!(format!("xr_{}", &hash[..24]));
        f.write("state/cortex/catalog/cn_fixture/cs_fixture.json", &snapshot);
        f.write("config/cortex/defaults.json",&json!({"defaults":{"coordinator":{"engine":engine,"model":"fixture-model"},"parameters":{}}}));
        f.write(
            "config/cortex/routing.json",
            &json!({"model_policy_version":2,"enabled":{engine:["fixture-model"]},
            "models":{engine:{"auto":["fixture-model"],"tiers":{"fixture-model":"standard"}}},
            "pin":{"*":format!("{engine}/fixture-model:high")}}),
        );
        let provider = workspace_service::native_routes::ConfiguredRoutes {
            environment: f.environment(),
            relay: PathBuf::from("/bin/true"),
        };
        for role in ["orchestrator", "worker-general", "worker-code-reviewer"] {
            let route = provider.route(&f.base.join("project"), role).unwrap();
            assert_eq!(route.launch.engine.name(), engine);
            assert_eq!(route.launch.executable, executable);
            assert!(!marker.exists(), "offline qualification executed an engine");
        }
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            code(provider.route(&f.base.join("project"), "orchestrator")),
            "native_route_executable_unavailable"
        );
        assert!(!marker.exists());
    }
}
#[test]
fn loader_does_not_create_or_rewrite_configuration_or_catalogs() {
    let f = Fixture::new();
    let paths = [
        "config/cortex/connections.json",
        "config/cortex/routing.json",
        "config/cortex/defaults.json",
        "state/cortex/catalog/cn_fixture/cs_fixture.json",
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|p| std::fs::read(f.base.join(p)).unwrap())
        .collect();
    assert!(f.load().unwrap().coordinator().is_ok());
    for (p, b) in paths.iter().zip(before) {
        assert_eq!(std::fs::read(f.base.join(p)).unwrap(), b);
    }
    assert_eq!(
        std::fs::read_dir(f.base.join("project/.cortex-dev"))
            .unwrap()
            .count(),
        0
    );
}

// Ports of the route-menu, model-policy, usage-tiebreak and recorded-consent cases.
impl Fixture {
    fn parity(&self) -> Value {
        let mut registry = json!({"schema_version":1,"connections":{}});
        let mut rules = json!({"model_policy_version":2,"enabled":{},"models":{},"pin":{}});
        for (engine, id, models) in [
            (
                "codex-acp",
                "cn_fixture",
                vec![
                    ("sol", json!(["standard", "deep"])),
                    ("luna", json!("light")),
                    ("astra", json!("flagship")),
                ],
            ),
            (
                "claude-agent-acp",
                "cn_claude",
                vec![
                    ("sonnet", json!(["standard", "deep"])),
                    ("opus", json!("flagship")),
                ],
            ),
        ] {
            let directory = self.base.join(format!("state/cortex/catalog/{id}"));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(&directory)
                .unwrap();
            let mut record = self.record();
            record["id"] = json!(id);
            record["engine"] = json!(engine);
            Self::approve(&mut record);
            registry["connections"][id] = record;
            let mut snapshot =
                json!({"snapshot_id":"cs_fixture","entries":[],"unavailable_reason":null});
            for (model, tier) in models {
                if rules["enabled"].get(engine).is_none() {
                    rules["enabled"][engine] = json!([]);
                    rules["models"][engine] = json!({"auto":[],"tiers":{},"policy":{}});
                }
                rules["enabled"][engine]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(model));
                rules["models"][engine]["auto"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(model));
                rules["models"][engine]["tiers"][model] = tier;
                rules["models"][engine]["policy"][model] = json!({"request_names":[model]});
                let mut entry = self.snapshot("cs_fixture")["entries"][0].clone();
                entry["connection_id"] = json!(id);
                entry["route"] = json!({"engine":engine,"connection_id":id,"model":model,
                    "supported_parameters":[["thought_level","high"],["thought_level","low"],["thought_level","max"],["thought_level","medium"],["thought_level","ultra"],["thought_level","xhigh"]]});
                // The canonical identity sorts and deduplicates parameter pairs.
                entry["route"]["supported_parameters"]
                    .as_array_mut()
                    .unwrap()
                    .sort_by_key(Value::to_string);
                entry["route"]["id"] = json!(format!(
                    "xr_{}",
                    &digest(&serde_json::to_vec(&entry["route"]).unwrap())[..24]
                ));
                snapshot["entries"].as_array_mut().unwrap().push(entry);
            }
            self.write(
                &format!("state/cortex/catalog/{id}/cs_fixture.json"),
                &snapshot,
            );
        }
        self.write("config/cortex/connections.json", &registry);
        self.write("config/cortex/routing.json", &rules);
        rules
    }
    fn used(&self, id: &str, used: f64) {
        if id == "cn_fixture" {
            let day = self.base.join("credentials/sessions/2026/11/12");
            std::fs::create_dir_all(&day).unwrap();
            std::fs::write(day.join("rollout.jsonl"),json!({"payload":{"rate_limits":{"primary":{"used_percent":used,"resets_at":4102444800.0}}}}).to_string()).unwrap();
            return;
        }
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(self.base.join("state/cortex/quota"))
            .unwrap();
        self.write(
            &format!("state/cortex/quota/{id}.json"),
            &json!({"seven_day":{"used":used,"resets_at":4102444800.0,"source":"event"}}),
        );
    }
    fn task(&self, goal: &str) -> cortex_core::Store {
        use cortex_core::{lifecycle::TaskMachine, OpenMode, ProjectKey, Store, StoreChannel};
        let store = Store::open(
            ProjectKey::new(self.base.join("project"), StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        TaskMachine::new(&store)
            .create("t_fixture", goal, "owner")
            .unwrap();
        store
            .write(|db| {
                cortex_core::routing::snapshot_tx(db, "t_fixture", &self.base.join("project"))?;
                Ok(())
            })
            .unwrap();
        store
    }
    fn evidence(&self, store: &cortex_core::Store) -> cortex_core::routing::Evidence {
        store
            .read(|db| cortex_core::routing::evidence(db, "t_fixture", &self.base.join("project")))
            .unwrap()
    }
}
#[test]
fn parity_menu_has_oracle_fields_live_tiers_roles_efforts_and_conditional_preferences() {
    let f = Fixture::new();
    let mut r = f.parity();
    r["models"]["codex-acp"]["policy"]["sol"] =
        json!({"preferred_for":["review"],"preferred_when":"Backend only."});
    r["models"]["codex-acp"]["policy"]["luna"] = json!({"effort_floor":"high","effort_ladder":["high","xhigh","max"],"also_standard_for":["implementation"]});
    f.write("config/cortex/routing.json", &r);
    f.used("cn_fixture", 95.0);
    let menu = f.load().unwrap().menu(&Default::default()).unwrap();
    let keys: BTreeSet<_> = menu
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "tiers",
            "quota_window_policy",
            "roles",
            "role_work_kinds",
            "affinity",
            "affinity_not_eligible",
            "affinity_source",
            "preferred",
            "preferred_when",
            "flagship",
            "stretch"
        ])
    );
    assert_eq!(menu["roles"]["worker-backend-dev"]["high"], "deep");
    assert_eq!(menu["role_work_kinds"]["worker-qa-engineer"], "testing");
    assert_eq!(
        menu["preferred_when"]["codex-acp/sol"],
        json!({"when":"Backend only.","work_kinds":["review"]})
    );
    assert_eq!(
        menu["flagship"]["request_names"]["claude-agent-acp/opus"],
        json!(["opus"])
    );
    let standard = menu["tiers"]["standard"].as_array().unwrap();
    let sol = standard.iter().find(|e| e["model"] == "sol").unwrap();
    assert_eq!(sol["quota"], json!({"band":"hot","used_percent":95.0}));
    assert_eq!(sol["efforts"], json!(["low", "medium", "high", "xhigh"]));
    assert_eq!(
        menu["stretch"][0]["efforts"],
        json!({"light":"high","standard":"xhigh","standard_escalated":"max"})
    );
    assert_eq!(
        menu["affinity"]["implementation_frontend_design"],
        "claude-agent-acp"
    );
}
use std::collections::BTreeSet;
#[test]
fn parity_usage_last_tie_break_and_hot_threshold_does_not_override_cool_hint() {
    let f = Fixture::new();
    let mut r = f.parity();
    for engine in ["codex-acp", "claude-agent-acp"] {
        let model = if engine == "codex-acp" {
            "sol"
        } else {
            "sonnet"
        };
        r["models"][engine]["policy"][model]["preferred_for"] = json!(["review"]);
    }
    f.write("config/cortex/routing.json", &r);
    f.used("cn_fixture", 7.0);
    f.used("cn_claude", 52.0);
    let node = json!({"model":"codex-acp/sol","model_reason":"A recommendation, not consent."});
    let selected = f
        .load()
        .unwrap()
        .choose(
            "worker-security-auditor",
            &node,
            &Default::default(),
            false,
            None,
        )
        .unwrap();
    assert_eq!(selected.model(), "sol"); // eligible hint outranks usage/affinity among preferred routes
    for used in [94.999, 95.0, 100.0] {
        f.used("cn_fixture", used);
        let s = f
            .load()
            .unwrap()
            .choose(
                "worker-security-auditor",
                &node,
                &Default::default(),
                false,
                None,
            )
            .unwrap();
        assert_eq!(s.model(), if used < 95.0 { "sol" } else { "sonnet" });
    }
    f.used("cn_fixture", 7.0);
    let s = f
        .load()
        .unwrap()
        .choose(
            "worker-security-auditor",
            &json!({}),
            &Default::default(),
            false,
            None,
        )
        .unwrap();
    assert_eq!(s.model(), "sonnet"); // review affinity, not the 45-point usage gap
}
#[test]
fn a_reviewer_is_routed_off_the_reviewed_model_when_another_is_available() {
    let f = Fixture::new();
    f.write("config/cortex/routing.json", &f.parity());
    let route = |avoid: Value| {
        f.load()
            .unwrap()
            .choose(
                "worker-code-reviewer",
                &json!({"work_kind":"review","avoid_routes":avoid}),
                &Default::default(),
                false,
                None,
            )
            .unwrap()
            .model()
            .to_owned()
    };
    assert_eq!(route(json!([{"engine":"claude-agent-acp","model":"sonnet"}])), "sol");
    assert_eq!(route(json!([{"engine":"codex-acp","model":"sol"}])), "sonnet");
    // Every model was used by the reviewed work: no other model exists, and routing
    // still proceeds (the runtime records the independence warning).
    let all = route(json!([
        {"engine":"codex-acp","model":"sol"},{"engine":"codex-acp","model":"luna"},{"engine":"codex-acp","model":"astra"},
        {"engine":"claude-agent-acp","model":"sonnet"},{"engine":"claude-agent-acp","model":"opus"}]));
    assert!(["sol", "luna", "astra", "sonnet", "opus"].contains(&all.as_str()));
}
#[test]
fn parity_flagship_hint_reason_never_grants_but_recorded_request_overrides_pin() {
    let f = Fixture::new();
    let mut r = f.parity();
    r["pin"] = json!({"backend-dev":"codex-acp/sol:medium"});
    f.write("config/cortex/routing.json", &r);
    let n = json!({"model":"codex-acp/astra","model_reason":"The user requires Astra."});
    assert_eq!(
        f.load()
            .unwrap()
            .choose("worker-backend-dev", &n, &Default::default(), false, None)
            .unwrap()
            .model(),
        "sol"
    );
    let store = f.task("Use Astra for the implementation.");
    let e = f.evidence(&store);
    for retry in [false, true] {
        let s = f
            .load()
            .unwrap()
            .choose("worker-backend-dev", &n, &e, retry, Some("codex-acp"))
            .unwrap();
        assert_eq!(s.model(), "astra");
        assert_eq!(s.basis, "auto:user_request");
        assert_eq!(s.parameters["thought_level"], "high");
        assert_eq!(
            s.decision["user_model_request"]["source_ref"],
            "t_fixture:goal"
        );
    }
}
#[test]
fn parity_user_requested_route_hard_refusals_never_substitute() {
    for reason in [
        "deny",
        "providers",
        "exhausted",
        "unavailable",
        "catalog_missing",
    ] {
        let f = Fixture::new();
        let mut r = f.parity();
        let store = f.task("Use Astra for the implementation.");
        if reason == "deny" {
            r["deny"] = json!(["codex-acp/astra"]);
        }
        if reason == "providers" {
            r["providers"] = json!(["claude-agent-acp"]);
        }
        if reason == "exhausted" {
            f.used("cn_fixture", 100.0);
        }
        if reason == "unavailable" {
            store.write(|db|{cortex_core::lifecycle::event_tx(db,"t_fixture","ROUTE_UNAVAILABLE",
                json!({"connection_id":"cn_fixture","model":"astra","expires_at":"2100-01-01T00:00:00+00:00"}))?;Ok(())}).unwrap();
        }
        if reason == "catalog_missing" {
            let path = "state/cortex/catalog/cn_fixture/cs_fixture.json";
            let mut s: Value =
                serde_json::from_slice(&std::fs::read(f.base.join(path)).unwrap()).unwrap();
            s["entries"]
                .as_array_mut()
                .unwrap()
                .retain(|e| e["route"]["model"] != "astra");
            f.write(path, &s);
        }
        f.write("config/cortex/routing.json", &r);
        assert_eq!(
            code(f.load().unwrap().choose(
                "worker-backend-dev",
                &json!({"model":"codex-acp/astra"}),
                &f.evidence(&store),
                false,
                None
            )),
            "user_requested_route_unavailable",
            "{reason}"
        );
    }
}
#[test]
fn parity_consent_requires_recorded_request_not_mention_question_quote_or_alternatives() {
    for text in [
        "Astra wrote the previous implementation.",
        "Compare Astra with Sol.",
        "What do you think about Astra?",
        "Does implementation have to be performed by Astra?",
        "Do not use Astra.",
        "> Use Astra for implementation.",
        "The last run on Astra failed.",
    ] {
        let f = Fixture::new();
        f.parity();
        let store = f.task(text);
        let n = json!({"model":"codex-acp/astra","model_reason":"Use Astra for implementation."});
        let s = f
            .load()
            .unwrap()
            .choose("worker-backend-dev", &n, &f.evidence(&store), false, None)
            .unwrap();
        assert_ne!(s.model(), "astra", "{text}");
    }
}
#[test]
fn parity_project_instruction_scope_changed_file_and_newer_user_override() {
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let f = Fixture::new();
        f.parity();
        let rule = "All audits must be performed by Opus.\n";
        std::fs::write(f.base.join("project").join(name), rule).unwrap();
        let store = f.task("Review the application.");
        let n = json!({"model":"codex-acp/sol","id":"audit","role":"security-auditor"});
        let resolver = f.load().unwrap();
        let e = f.evidence(&store);
        let s = resolver
            .choose("worker-security-auditor", &n, &e, false, None)
            .unwrap();
        assert_eq!(s.model(), "opus");
        assert_eq!(s.decision["decided_by"], "project_instruction");
        assert_eq!(s.decision["project_instruction"]["path"], name);
        assert_ne!(
            resolver
                .choose("worker-backend-dev", &json!({}), &e, false, None)
                .unwrap()
                .model(),
            "opus"
        );
        assert_eq!(
            resolver
                .previews(&json!({"tasks":[n.clone()]}), &e)
                .unwrap()[0]["preview_status"],
            "pending_dispatch:project_instruction"
        );
        std::fs::write(
            f.base.join("project").join(name),
            format!("{rule}\nChanged."),
        )
        .unwrap();
        let e = f.evidence(&store);
        let s = resolver
            .choose("worker-security-auditor", &n, &e, false, None)
            .unwrap();
        assert_ne!(s.model(), "opus");
        assert_eq!(
            s.decision["project_instruction"]["status"],
            "changed_during_task"
        );
        store
            .write(|db| {
                cortex_core::lifecycle::event_tx(
                    db,
                    "t_fixture",
                    "USER_STEERING",
                    json!({"text":"Use Sonnet for the review."}),
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            resolver
                .choose(
                    "worker-security-auditor",
                    &n,
                    &f.evidence(&store),
                    false,
                    None
                )
                .unwrap()
                .model(),
            "sonnet"
        );
    }
}
#[test]
fn parity_answer_selection_only_uses_actual_answer_never_question_text() {
    let f = Fixture::new();
    f.parity();
    let store = f.task("Review.");
    let question = "Which model should perform the audit?";
    let options = json!(["Opus", "Sonnet"]);
    store.write(|db|{
        db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at) VALUES ('pq_fixture','t_fixture','q',?,?,'open',?)",
            (question,options.to_string(),cortex_core::now()))?;Ok(())
    }).unwrap();
    let n = json!({"model":"claude-agent-acp/opus"});
    let resolver = f.load().unwrap();
    assert_ne!(
        resolver
            .choose(
                "worker-security-auditor",
                &n,
                &f.evidence(&store),
                false,
                None
            )
            .unwrap()
            .model(),
        "opus"
    );
    store.write(|db|{db.execute("UPDATE pending_questions SET status='answered',answer_text='Opus',answer_source='elicitation',answered_at=?",[cortex_core::now()])?;Ok(())}).unwrap();
    assert_eq!(
        resolver
            .choose(
                "worker-security-auditor",
                &n,
                &f.evidence(&store),
                false,
                None
            )
            .unwrap()
            .model(),
        "opus"
    );
}
#[test]
fn parity_effort_policy_observed_bounds_explicit_request_and_unsupported_requirement() {
    let f = Fixture::new();
    let mut r = f.parity();
    r["models"]["codex-acp"]["policy"]["astra"]["max_effort"] = json!("medium");
    f.write("config/cortex/routing.json", &r);
    let store = f.task("Use codex-acp/astra at medium effort for implementation.");
    let n = json!({"model":"codex-acp/astra"});
    let s = f
        .load()
        .unwrap()
        .choose("worker-backend-dev", &n, &f.evidence(&store), false, None)
        .unwrap();
    assert_eq!(s.parameters["thought_level"], "medium");
    store
        .write(|db| {
            cortex_core::lifecycle::event_tx(
                db,
                "t_fixture",
                "USER_STEERING",
                json!({"text":"Use codex-acp/astra:high for implementation."}),
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        code(
            f.load()
                .unwrap()
                .choose("worker-backend-dev", &n, &f.evidence(&store), false, None)
        ),
        "user_requested_effort_unavailable"
    );
}
#[test]
fn parity_conditional_preference_and_work_kind_only_apply_to_matching_nodes() {
    let f = Fixture::new();
    let mut r = f.parity();
    r["models"]["codex-acp"]["policy"]["sol"]["preferred_when"] = json!("Backend only.");
    r["models"]["codex-acp"]["policy"]["sol"]["preferred_for"] = json!(["review"]);
    f.write("config/cortex/routing.json", &r);
    let resolver = f.load().unwrap();
    let e = Default::default();
    assert_eq!(
        resolver
            .choose("worker-security-auditor", &json!({}), &e, false, None)
            .unwrap()
            .model(),
        "sonnet"
    );
    assert_eq!(
        resolver
            .choose(
                "worker-security-auditor",
                &json!({"model":"codex-acp/sol","model_reason":"Backend only."}),
                &e,
                false,
                None
            )
            .unwrap()
            .model(),
        "sol"
    );
    let retried = resolver
        .choose(
            "worker-security-auditor",
            &json!({"model":"codex-acp/sol"}),
            &e,
            true,
            Some("codex-acp"),
        )
        .unwrap();
    assert_ne!(retried.model(), "sol");
    // UI-design classification binds to the owner's affinity, independent of a coding hint.
    assert_eq!(
        resolver
            .choose(
                "worker-frontend-dev",
                &json!({"model":"codex-acp/sol","work_kind":"implementation_frontend_design"}),
                &e,
                false,
                None
            )
            .unwrap()
            .model(),
        "sonnet"
    );
}
#[test]
fn parity_stretch_ladder_and_high_complexity_exclusion() {
    let f = Fixture::new();
    let mut r = f.parity();
    r["models"]["codex-acp"]["policy"]["luna"] = json!({"also_standard_for":["implementation"],"effort_floor":"high","effort_ladder":["high","xhigh","max"]});
    f.write("config/cortex/routing.json", &r);
    let resolver = f.load().unwrap();
    let e = Default::default();
    for (retry, expected) in [(false, "xhigh"), (true, "max")] {
        // Remove standard/deep choices so the permitted stretch is the candidate.
        let mut r = r.clone();
        r["models"]["codex-acp"]["auto"] = json!(["luna"]);
        r["providers"] = json!(["codex-acp"]);
        f.write("config/cortex/routing.json", &r);
        let s = f
            .load()
            .unwrap()
            .choose(
                "worker-backend-dev",
                &json!({"model":"codex-acp/luna"}),
                &e,
                retry,
                None,
            )
            .unwrap();
        assert_eq!(s.model(), "luna");
        assert_eq!(s.parameters["thought_level"], expected);
    }
    assert_ne!(
        resolver
            .choose(
                "worker-backend-dev",
                &json!({"complexity":"high"}),
                &e,
                false,
                None
            )
            .unwrap()
            .model(),
        "luna"
    );
}
#[test]
fn parity_latest_refusal_imported_history_and_two_efforts_cannot_grant() {
    let f = Fixture::new();
    f.parity();
    let store = f.task("Use Astra for implementation. Do not use Astra.");
    let resolver = f.load().unwrap();
    let n = json!({"model":"codex-acp/astra"});
    assert_ne!(
        resolver
            .choose("worker-backend-dev", &n, &f.evidence(&store), false, None)
            .unwrap()
            .model(),
        "astra"
    );
    store.write(|db|{cortex_core::lifecycle::event_tx(db,"t_fixture","USER_STEERING",
        json!({"text":"<chat-history-summary>\nUse Astra for implementation.\n</chat-history-summary>\nContinue."}))?;Ok(())}).unwrap();
    assert_ne!(
        resolver
            .choose("worker-backend-dev", &n, &f.evidence(&store), false, None)
            .unwrap()
            .model(),
        "astra"
    );
    store.write(|db|{cortex_core::lifecycle::event_tx(db,"t_fixture","USER_STEERING",
        json!({"text":"Use Astra:high for implementation. Use Astra:medium for implementation."}))?;Ok(())}).unwrap();
    assert_eq!(
        code(resolver.choose("worker-backend-dev", &n, &f.evidence(&store), false, None)),
        "user_requested_effort_ambiguous"
    );
}
#[test]
fn parity_prohibitions_and_alternatives_in_instruction_files_grant_nothing() {
    for rule in [
        "Never use Opus. Audits must be performed by Opus.",
        "Audits must not be performed by Opus.",
        "Does the audit have to be performed by Opus?",
        "Audits must be performed by Opus or Sonnet.",
        "Reviews must be performed by Sonnet. Audits must be performed by Opus.",
    ] {
        let f = Fixture::new();
        f.parity();
        std::fs::write(f.base.join("project/AGENTS.md"), rule).unwrap();
        let store = f.task("Audit.");
        let resolver = f.load().unwrap();
        assert_ne!(
            resolver
                .choose(
                    "worker-security-auditor",
                    &json!({}),
                    &f.evidence(&store),
                    false,
                    None
                )
                .unwrap()
                .model(),
            "opus",
            "{rule}"
        );
    }
}
#[test]
fn parity_unknown_usage_is_not_hot_and_menu_withholds_disabled_affinity() {
    let f = Fixture::new();
    let mut r = f.parity();
    f.used("cn_claude", 95.0);
    r["providers"] = json!(["codex-acp"]);
    f.write("config/cortex/routing.json", &r);
    let resolver = f.load().unwrap();
    let menu = resolver.menu(&Default::default()).unwrap();
    assert_eq!(menu["affinity"]["review"], Value::Null);
    assert_eq!(menu["affinity_not_eligible"]["review"], "claude-agent-acp");
    assert!(menu["tiers"]["standard"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["engine"] == "codex-acp" && e["quota"]["band"] == "unknown"));
}

#[test]
fn legacy_preview_fields_and_dispatch_agree_table() {
    // test_usage_tiebreak.py:199-230 and project instruction preview assertions.
    for (hint, codex, claude, status, chosen) in [
        (
            "claude-agent-acp/opus",
            7.0,
            52.0,
            "ignored:owner_preferred",
            "sonnet",
        ),
        (
            "claude-agent-acp/sonnet",
            7.0,
            52.0,
            "pending_dispatch",
            "sonnet",
        ),
        (
            "claude-agent-acp/sonnet",
            7.0,
            97.0,
            "ignored:near_quota_limit",
            "sol",
        ),
        (
            "claude-agent-acp/sonnet",
            95.0,
            99.0,
            "pending_dispatch",
            "sonnet",
        ),
    ] {
        let f = Fixture::new();
        let mut rules = f.parity();
        rules["models"]["claude-agent-acp"]["tiers"]["opus"] = json!(["flagship", "deep"]);
        for (e, m) in [("codex-acp", "sol"), ("claude-agent-acp", "sonnet")] {
            rules["models"][e]["policy"][m]["preferred_for"] = json!(["review"]);
        }
        f.write("config/cortex/routing.json", &rules);
        f.used("cn_fixture", codex);
        f.used("cn_claude", claude);
        let resolver = f.load().unwrap();
        let n = json!({"id":"audit","role":"security-auditor","model":hint,"model_reason":"Advisory only"});
        let preview = resolver
            .previews(&json!({"tasks":[n.clone()]}), &Default::default())
            .unwrap()[0]
            .clone();
        assert_eq!(preview["preview_status"], status);
        assert_eq!(preview["work_kind"], "review");
        assert_eq!(preview["tier_now"], "deep");
        if status == "ignored:owner_preferred" {
            assert_eq!(
                preview["owner_preferred"],
                json!(["claude-agent-acp/sonnet", "codex-acp/sol"])
            );
            assert!(preview["preview_note"]
                .as_str()
                .unwrap()
                .contains("not a mismatch"));
        } else {
            assert!(preview.get("owner_preferred").is_none());
        }
        let dispatch = resolver
            .choose(
                "worker-security-auditor",
                &n,
                &Default::default(),
                false,
                None,
            )
            .unwrap();
        assert_eq!(dispatch.model(), chosen);
        assert_eq!(
            dispatch.decision["model_hint_status"],
            if status == "pending_dispatch" {
                "honoured"
            } else {
                status
            }
        );
    }
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let f = Fixture::new();
        f.parity();
        std::fs::write(
            f.base.join("project").join(name),
            "# Rules\n\nAll audits must be performed by Opus.\n",
        )
        .unwrap();
        let store = f.task("Audit.");
        let resolver = f.load().unwrap();
        let n = json!({"id":"audit","role":"security-auditor","model":"codex-acp/sol"});
        let backend = json!({"id":"backend","role":"backend-dev","model":"auto"});
        let plan = json!({"tasks":[n.clone(),backend]});
        let previews = resolver.previews(&plan, &f.evidence(&store)).unwrap();
        assert_eq!(
            previews[0]["preview_status"],
            "pending_dispatch:project_instruction"
        );
        assert_eq!(
            previews[0]["project_instruction"],
            json!({"route":"claude-agent-acp/opus","path":name,"line":3,"matched":"Opus"})
        );
        assert!(previews[0]["preview_note"]
            .as_str()
            .unwrap()
            .contains("instead of this hint"));
        assert_eq!(previews[1]["preview_status"], "auto");
        assert!(previews[1].get("project_instruction").is_none());
        std::fs::write(
            f.base.join("project").join(name),
            "# Rules\n\nAll audits must be performed by Opus.\nChanged.\n",
        )
        .unwrap();
        let mut auto = plan.clone();
        auto["tasks"][0]["model"] = json!("auto");
        let e = f.evidence(&store);
        let p = resolver.previews(&auto, &e).unwrap();
        assert_eq!(p[0]["preview_status"], "auto");
        assert_eq!(
            p[0]["project_instruction"],
            json!({"status":"changed_during_task","path":name,"line":3,"route":"claude-agent-acp/opus"})
        );
        assert!(p[0]["project_instruction_note"]
            .as_str()
            .unwrap()
            .contains("changed after this task started"));
        let dispatch = resolver
            .choose(
                "worker-security-auditor",
                &auto["tasks"][0],
                &e,
                false,
                None,
            )
            .unwrap();
        assert_eq!(
            dispatch.decision["project_instruction"],
            p[0]["project_instruction"]
        );
        assert_ne!(dispatch.model(), "opus");
    }
}

#[test]
fn recorded_normalized_option_and_latest_owner_input_bind_exact_task() {
    let f = Fixture::new();
    f.parity();
    let store = f.task("Audit.");
    store.write(|db|{
        db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at,answer_text,answer_source,answered_at) VALUES ('pq_fixture','t_fixture','q','Which model for the audit?','[\"Opus\",\"Sonnet\"]','answered',?,'  opus.  ','text',?)",(cortex_core::now(),cortex_core::now()))?;
        Ok(())
    }).unwrap();
    let e = f.evidence(&store);
    assert!(e.inputs.last().unwrap().selected_option);
    let resolver = f.load().unwrap();
    assert_eq!(
        resolver
            .choose(
                "worker-security-auditor",
                &json!({"model":"auto"}),
                &e,
                false,
                None
            )
            .unwrap()
            .model(),
        "opus"
    );
    store
        .write(|db| {
            cortex_core::lifecycle::event_tx(
                db,
                "t_fixture",
                "USER_STEERING",
                json!({"text":"Use Sonnet for the audit."}),
            )?;
            Ok(())
        })
        .unwrap();
    let selected = resolver
        .choose(
            "worker-security-auditor",
            &json!({"model":"auto"}),
            &f.evidence(&store),
            false,
            None,
        )
        .unwrap();
    assert_eq!(selected.model(), "sonnet");
    assert!(selected.decision["user_model_request"]["source_ref"]
        .as_str()
        .unwrap()
        .starts_with("event:"));
}

#[test]
fn flagship_is_never_an_ordinary_automatic_fallback_table() {
    for escalate in [false, true] {
        let f = Fixture::new();
        let mut rules = f.parity();
        rules["models"]["codex-acp"]["auto"] = json!(["astra"]);
        rules["models"]["claude-agent-acp"]["auto"] = json!(["opus"]);
        f.write("config/cortex/routing.json", &rules);
        let selected = f.load().unwrap().choose(
            "worker-security-auditor",
            &json!({"model":"auto"}),
            &Default::default(),
            escalate,
            None,
        );
        if escalate {
            assert!(["astra", "opus"].contains(&selected.unwrap().model()));
        } else {
            assert_eq!(code(selected), "no_available_route");
        }
    }
}
