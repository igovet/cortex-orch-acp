#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use workspace_service::{
    config_read_dispatch::{ConfigReadAction, ConfigReadHost, ConfigReadResult},
    config_read_model::SetupEnvironment,
    native_routes::RouteEnvironment,
};

const NOW: &str = "2026-11-13T21:30:00+00:00";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/tmp/cw-rd-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let f = Self(root);
        for name in ["home", "config", "state", "workspace", "data", "bin"] {
            f.directory(name);
        }
        f
    }
    fn directory(&self, name: &str) {
        let p = self.0.join(name);
        for d in p
            .ancestors()
            .take_while(|d| d.starts_with(&self.0))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            fs::create_dir_all(d).unwrap();
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        self.directory(Path::new(name).parent().unwrap().to_str().unwrap());
        let p = self.0.join(name);
        fs::write(&p, bytes).unwrap();
        fs::set_permissions(
            p,
            fs::Permissions::from_mode(if name.starts_with("bin/") {
                0o700
            } else {
                0o600
            }),
        )
        .unwrap();
    }
    fn expand(&self, v: &Value) -> Value {
        serde_json::from_str(&v.to_string().replace("@ROOT@", self.0.to_str().unwrap())).unwrap()
    }
    fn prepare(&self, case: &Value) {
        for d in case["directories"].as_array().unwrap() {
            self.directory(d.as_str().unwrap());
        }
        for (name, raw) in case["files"].as_object().unwrap() {
            self.write(name, self.expand(raw).as_str().unwrap().as_bytes());
        }
    }
    fn bytes(&self) -> Vec<u8> {
        fs::read(self.0.join("config/cortex/connections.json")).unwrap()
    }
    fn token(&self) -> String {
        format!("sha256:{:x}", Sha256::digest(self.bytes()))
    }
    fn read(&self, action: ConfigReadAction<'_>) -> cortex_core::CoreResult<ConfigReadResult> {
        let env = RouteEnvironment {
            home: self.0.join("home"),
            config: self.0.join("config"),
            state: self.0.join("state"),
            routing_override: None,
        };
        let setup = SetupEnvironment {
            data: self.0.join("data"),
            path: vec![self.0.join("bin")],
            node_override: None,
            bundle: None,
        };
        ConfigReadHost {
            environment: &env,
            setup: &setup,
            workspace_state: &self.0.join("workspace"),
            project: None,
        }
        .read(action, &json!({"surfaces":{}}), 1800000000.0)
    }
    fn refresh(&self, ids: &[String], revision: &str) -> cortex_core::CoreResult<ConfigReadResult> {
        self.read(ConfigReadAction::RefreshConnections {
            ids,
            expected_revision: revision,
            observed_at: NOW,
            wait: Duration::ZERO,
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/config-read-dispatch-python.json")).unwrap()
}
fn ids(c: &Value) -> Vec<String> {
    c["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect()
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    for e in fs::read_dir(root).unwrap() {
        let p = e.unwrap().path();
        let m = fs::symlink_metadata(&p).unwrap();
        let bytes = if m.is_file() {
            fs::read(&p).unwrap()
        } else {
            vec![]
        };
        out.insert(p.clone(), (m.permissions().mode(), bytes));
        if m.is_dir() {
            out.extend(snapshot(&p));
        }
    }
    out
}
fn expected_view(f: &Fixture, v: &Value) -> Value {
    let mut want = f.expand(v);
    // Different temporary roots and registry JSON serializers have different byte
    // tokens. Assert the native token against actual bytes independently, then
    // compare every other field, including profile-section connection revisions.
    let python_token = want["tokens"]["connections"].as_str().unwrap().to_owned();
    assert_eq!(python_token, "sha256:@CONNECTION_BYTES@");
    want = serde_json::from_str(&want.to_string().replace(&python_token, &f.token())).unwrap();
    assert_eq!(
        want["config"]["setup"]["opencode"]["status"],
        "not_downloaded"
    );
    want["config"]["setup"]["opencode"]["status"] = json!("discovery_unavailable");
    want
}

#[test]
fn display_and_explicit_filesystem_refresh_match_the_frozen_oracle() {
    let c = corpus();
    assert_eq!(c["cases"].as_array().unwrap().len(), 14);
    for case in c["cases"].as_array().unwrap() {
        let f = Fixture::new();
        f.prepare(case);
        let before = snapshot(&f.0);
        let token = f.token();
        let display = f.read(ConfigReadAction::Display).unwrap();
        assert_eq!(display.settings.view["tokens"]["connections"], token);
        assert_eq!(
            display.settings.view,
            expected_view(&f, &case["before"]),
            "{}",
            case["name"]
        );
        assert!(display.refresh.is_none());
        assert_eq!(before, snapshot(&f.0), "display wrote state");
        let result = f.refresh(&ids(case), &token);
        if !case["keyring"].is_null() {
            assert_eq!(result.err().unwrap().code(), "keyring_refresh_unsupported");
            assert_eq!(f.token(), token, "keyring refusal changed registry");
            assert!(display.settings.diagnostics.contains(&json!({
                "connection_id":"cn_000000000001","source":"keyring","code":"keyring_not_observed"})));
        } else {
            let result = result.unwrap();
            let receipt = result.refresh.unwrap();
            assert_eq!(receipt["connections"], case["states"]);
            assert_eq!(receipt["revision"], f.token());
            assert_eq!(result.settings.view["tokens"]["connections"], f.token());
            assert_eq!(
                result.settings.view,
                expected_view(&f, &case["after"]),
                "{}",
                case["name"]
            );
            let registry: Value = serde_json::from_slice(&f.bytes()).unwrap();
            assert_eq!(
                registry,
                f.expand(&case["registry_after"]),
                "{}",
                case["name"]
            );
            if case["name"].as_str().unwrap().ends_with("-noop") {
                assert_eq!(receipt["changed"], false);
                assert_eq!(f.token(), token);
            }
        }
        let mut after = snapshot(&f.0);
        after.remove(&f.0.join("config/cortex/connections.lock"));
        after.insert(
            f.0.join("config/cortex/connections.json"),
            before[&f.0.join("config/cortex/connections.json")].clone(),
        );
        assert_eq!(before, after, "refresh touched other files");
        let sanitized =
            json!({"view":display.settings.view,"diagnostics":display.settings.diagnostics})
                .to_string();
        for forbidden in [
            "fixture-service",
            "fixture-account",
            "fixture-not-a-real-secret",
            "credential_ref",
        ] {
            assert!(!sanitized.contains(forbidden));
        }
    }
}

#[test]
fn capture_provenance_and_fake_keyring_outcomes_are_explicit() {
    let c = corpus();
    for k in [
        "process_attempts",
        "real_engine_calls",
        "real_keyring_calls",
    ] {
        assert_eq!(c["evidence"][k], 0);
    }
    assert_eq!(c["evidence"]["disposable_home"], true);
    assert_eq!(c["evidence"]["fake_executables"], true);
    for case in c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| !c["keyring"].is_null())
    {
        let behavior = case["keyring"].as_str().unwrap();
        assert_eq!(
            case["fake_keyring_lookups"],
            if behavior == "missing-module" { 0 } else { 1 }
        );
        assert_eq!(
            case["states"][0]["state"],
            match behavior {
                "present" | "empty" => "configured",
                "absent" => "authorization_required",
                _ => "unavailable",
            }
        );
    }
}

#[test]
fn revision_selection_and_keyring_batches_fail_without_partial_refresh() {
    let c = corpus();
    let fs_case = &c["cases"][0];
    let f = Fixture::new();
    f.prepare(fs_case);
    let token = f.token();
    assert_eq!(
        f.refresh(&ids(fs_case), "sha256:stale")
            .err()
            .unwrap()
            .code(),
        "revision_conflict"
    );
    for (selection, code) in [
        (vec![], "invalid_refresh_request"),
        (vec!["cn_000000000001".into(); 2], "invalid_refresh_request"),
        (vec!["cn_ffffffffffff".into()], "connection_unavailable"),
    ] {
        assert_eq!(f.refresh(&selection, &token).err().unwrap().code(), code);
        assert_eq!(f.token(), token);
    }
    let keyring = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| !c["keyring"].is_null())
        .unwrap();
    let mut raw: Value = serde_json::from_slice(&f.bytes()).unwrap();
    let key: Value = serde_json::from_str(
        f.expand(&keyring["files"]["config/cortex/connections.json"])
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut rec = key["connections"]["cn_000000000001"].clone();
    rec["id"] = json!("cn_000000000002");
    raw["connections"]["cn_000000000002"] = rec;
    f.write(
        "config/cortex/connections.json",
        &serde_json::to_vec(&raw).unwrap(),
    );
    let token = f.token();
    for ids in [
        vec!["cn_000000000001".into(), "cn_000000000002".into()],
        vec!["cn_000000000002".into(), "cn_000000000001".into()],
    ] {
        assert_eq!(
            f.refresh(&ids, &token).err().unwrap().code(),
            "keyring_refresh_unsupported"
        );
        assert_eq!(f.token(), token, "mixed batch partially committed");
    }
    let result = f.refresh(&["cn_000000000001".into()], &token).unwrap();
    assert!(result
        .settings
        .diagnostics
        .iter()
        .any(|d| d["connection_id"] == "cn_000000000002" && d["code"] == "keyring_not_observed"));
}

#[test]
fn invalid_display_prevents_refresh_and_missing_display_never_initializes() {
    let f = Fixture::new();
    let before = snapshot(&f.0);
    let missing = f.read(ConfigReadAction::Display).unwrap();
    assert_eq!(missing.settings.view["tokens"]["connections"], "absent");
    assert_eq!(before, snapshot(&f.0));
    let c = corpus();
    f.prepare(&c["cases"][0]);
    let token = f.token();
    f.write("workspace/defaults.json", b"{broken");
    let before = snapshot(&f.0);
    assert!(f.refresh(&ids(&c["cases"][0]), &token).is_err());
    assert_eq!(
        before,
        snapshot(&f.0),
        "failed preflight created lock or changed registry"
    );
}
