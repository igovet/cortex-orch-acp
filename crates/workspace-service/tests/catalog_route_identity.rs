//! Real claude-agent-acp catalog identities and per-connection degradation.
//! Disposable owner settings only: no account files, engines or paid calls.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
};
use workspace_service::{
    config_read_model::ConfigurationReader,
    native_routes::{RouteEnvironment, RouteResolver},
};

const CODEX: &str = "cn_000000000001";
const CLAUDE: &str = "cn_000000000002";
/// `ExecutionRoute(engine="claude-agent-acp", connection_id=CLAUDE,
/// model="claude-sonnet-5", supported_parameters=CLAUDE_PAIRS).id` computed by
/// the Python oracle (`portable.identity.route_fingerprint`: sorted, never
/// deduplicated). The deduplicated list would hash to xr_f1ce1e8fdab08d6429439f91.
const CLAUDE_ROUTE: &str = "xr_b067806096f03b639ac4fcf2";
/// The Python oracle's id for the Codex route below.
const CODEX_ROUTE: &str = "xr_fd4ad9c738fd1736c931b007";

/// claude-agent-acp 0.97.0 advertises both `fast` and `ultracode` as
/// `model_config` on/off selects, so the observed pairs repeat.
fn claude_pairs() -> Value {
    json!([
        ["mode", "acceptEdits"],
        ["mode", "auto"],
        ["mode", "bypassPermissions"],
        ["mode", "default"],
        ["mode", "plan"],
        ["model_config", "off"],
        ["model_config", "off"],
        ["model_config", "on"],
        ["model_config", "on"],
        ["thought_level", "high"],
        ["thought_level", "low"],
        ["thought_level", "medium"]
    ])
}
fn codex_pairs() -> Value {
    json!([
        ["mode", "agent"],
        ["thought_level", "high"],
        ["thought_level", "low"],
        ["thought_level", "medium"]
    ])
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "catalog-route-identity-{}-{}",
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
            "state/cortex/catalog/cn_000000000001",
            "state/cortex/catalog/cn_000000000002",
            "project",
            "project/.cortex-dev",
            "credentials",
        ] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(base.join(path))
                .unwrap();
        }
        let f = Self(base);
        f.write(
            "config/cortex/connections.json",
            &json!({"schema_version":1,"connections":{
                CODEX:f.record(CODEX,"codex-acp","Codex"),CLAUDE:f.record(CLAUDE,"claude-agent-acp","Claude")}}),
        );
        f.write(
            "config/cortex/routing.json",
            &json!({"model_policy_version":2,
                "enabled":{"codex-acp":["gpt-6.1-sol"],"claude-agent-acp":["claude-sonnet-5"]},
                "models":{"codex-acp":{"auto":["gpt-6.1-sol"],"tiers":{"gpt-6.1-sol":["standard"]}},
                    "claude-agent-acp":{"auto":["claude-sonnet-5"],"tiers":{"claude-sonnet-5":["standard"]}}}}),
        );
        f.write(
            "config/cortex/defaults.json",
            &json!({"defaults":{"coordinator":{"engine":"claude-agent-acp","model":"claude-sonnet-5",
                "parameters":{"thought_level":"medium"}}}}),
        );
        f.snapshot(
            CODEX,
            "codex-acp",
            "gpt-6.1-sol",
            codex_pairs(),
            CODEX_ROUTE,
        );
        f.snapshot(
            CLAUDE,
            "claude-agent-acp",
            "claude-sonnet-5",
            claude_pairs(),
            CLAUDE_ROUTE,
        );
        f
    }
    fn record(&self, id: &str, engine: &str, name: &str) -> Value {
        let mut r = json!({"id":id,"name":name,"engine":engine,"provider":"fixture","endpoint":null,
            "launch_command":["/bin/true"],"auth_mode":"native_account","key_kind":null,"qualified":true,
            "state":"configured","credential_ref":{"kind":"engine_native_dir","path":self.0.join("credentials"),"owned":false},
            "max_concurrent_sessions":4,"created_at":"2026-11-15T00:00:00Z","updated_at":"2026-11-15T00:00:00Z"});
        r["approved_fingerprint"] = json!(digest(
            &serde_json::to_vec(&json!({"engine":r["engine"],"provider":r["provider"],
                "endpoint":r["endpoint"],"launch_command":r["launch_command"]}))
            .unwrap()
        ));
        r
    }
    fn snapshot(&self, connection: &str, engine: &str, model: &str, pairs: Value, id: &str) {
        let snapshot = "cs_00000000000a";
        let label = format!("{engine} · fixture · {model}");
        let provenance = json!({"source":"discovery","adapter_version":"unknown",
            "observed_at":"2026-11-15T00:00:00Z","evidence_ref":null});
        self.write(
            &format!("state/cortex/catalog/{connection}/{snapshot}.json"),
            &json!({"snapshot_id":snapshot,"created_at":"2026-11-15T00:00:00Z",
                "connection_registry_fingerprint":"fixture","setup_action":null,"unavailable_reason":null,
                "entries":[{"route":{"id":id,"engine":engine,"connection_id":connection,"model":model,
                    "supported_parameters":pairs},"connection_id":connection,"label":label,
                    "disambiguated_label":label,"catalog_snapshot_id":snapshot,"model_name":model,
                    "model_description":null,"context_window_tokens":null,
                    "evidence":{"catalog_obtained":provenance,"configured":provenance}}]}),
        );
    }
    fn path(&self, connection: &str) -> PathBuf {
        self.0.join(format!(
            "state/cortex/catalog/{connection}/cs_00000000000a.json"
        ))
    }
    fn write(&self, path: &str, value: &Value) {
        let path = self.0.join(path);
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn environment(&self) -> RouteEnvironment {
        RouteEnvironment {
            home: self.0.join("home"),
            config: self.0.join("config"),
            state: self.0.join("state"),
            routing_override: None,
        }
    }
    fn reader(&self) -> ConfigurationReader {
        ConfigurationReader::new(self.environment()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn route_ids(catalog: &Value) -> Vec<String> {
    catalog["routes"]["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["route_id"].as_str().unwrap().to_owned())
        .collect()
}
fn models(view: &Value, engine: &str) -> Vec<String> {
    view["config"]["catalogue"][engine]["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["routeId"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn real_claude_snapshot_with_repeated_model_config_pairs_is_read_and_qualifies() {
    let f = Fixture::new();
    let claude_bytes = fs::read(f.path(CLAUDE)).unwrap();
    let reader = f.reader();
    let catalog = reader.catalog_selector(None).unwrap();
    assert_eq!(catalog["routes"]["catalog_problems"], json!([]));
    // Records are listed by (engine, name, id), as in Python.
    assert_eq!(route_ids(&catalog), [CLAUDE_ROUTE, CODEX_ROUTE]);
    let view = reader
        .settings_with_catalog_readers(&json!({"surfaces":{},"workspace":{}}), 1800000000.0, None)
        .unwrap()
        .view;
    assert_eq!(models(&view, "claude-agent-acp"), [CLAUDE_ROUTE]);
    assert_eq!(models(&view, "codex-acp"), [CODEX_ROUTE]);
    assert_eq!(
        view["config"]["catalogue"]["claude-agent-acp"]["failed"],
        false
    );
    // The owner coordinator binds to the real Claude route, for display and execution.
    let defaults = reader.user_defaults_management_observation().unwrap();
    assert_eq!(
        defaults["result"]["coordinator_resolution"],
        json!({"state":"bound","route_id":CLAUDE_ROUTE,"connection_id":CLAUDE})
    );
    let selected = RouteResolver::load(
        &f.0.join("project"),
        &f.0.join("project/.cortex-dev"),
        f.environment(),
    )
    .unwrap()
    .coordinator()
    .unwrap();
    assert_eq!(selected.route_id(), CLAUDE_ROUTE);
    assert_eq!(selected.parameters["thought_level"], "medium");
    // Reads never rewrite or drop the owner's stored observation.
    assert_eq!(fs::read(f.path(CLAUDE)).unwrap(), claude_bytes);
}

#[test]
fn an_unreadable_connection_catalog_degrades_only_that_connection() {
    let f = Fixture::new();
    let mut bad: Value = serde_json::from_slice(&fs::read(f.path(CLAUDE)).unwrap()).unwrap();
    bad["entries"][0]["route"]["id"] = json!("xr_999999999999999999999999");
    let raw = serde_json::to_vec(&bad).unwrap();
    fs::write(f.path(CLAUDE), &raw).unwrap();
    let reader = f.reader();
    let catalog = reader.catalog_selector(None).unwrap();
    let problems = catalog["routes"]["catalog_problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0]["connection_id"], CLAUDE);
    assert_eq!(problems[0]["code"], "invalid_catalog_snapshot");
    assert!(problems[0]["message"]
        .as_str()
        .is_some_and(|m| !m.is_empty()));
    assert_eq!(route_ids(&catalog), [CODEX_ROUTE]);
    // The connection itself stays listed; only its model list is unavailable.
    let claude = catalog["routes"]["engines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["engine"] == "claude-agent-acp")
        .unwrap();
    assert_eq!(claude["connections"][0]["id"], CLAUDE);
    let view = reader
        .settings_with_catalog_readers(&json!({"surfaces":{},"workspace":{}}), 1800000000.0, None)
        .unwrap()
        .view;
    assert_eq!(models(&view, "codex-acp"), [CODEX_ROUTE]);
    assert!(models(&view, "claude-agent-acp").is_empty());
    assert_eq!(
        view["config"]["catalogue"]["claude-agent-acp"]["failed"],
        true
    );
    assert_eq!(view["config"]["catalogue"]["codex-acp"]["failed"], false);
    let defaults = reader.user_defaults_management_observation().unwrap();
    assert_eq!(
        defaults["result"]["coordinator_resolution"]["state"],
        "unavailable"
    );
    assert_eq!(
        defaults["result"]["coordinator_resolution"]["error"]["code"],
        "invalid_catalog_snapshot"
    );
    assert_eq!(fs::read(f.path(CLAUDE)).unwrap(), raw);
}
