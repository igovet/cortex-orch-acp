use super::*;
use crate::native_routes::RouteEnvironment;
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

struct Fixture {
    root: PathBuf,
    config: crate::Config,
    environment: RouteEnvironment,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/tmp/cw-msw-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let environment = RouteEnvironment {
            home: root.join("home"),
            config: root.join("config"),
            state: root.join("state"),
            routing_override: None,
        };
        crate::local::private_tree(&environment.config.join("cortex")).unwrap();
        let mut launch = crate::LaunchSpec::bare();
        launch.dev_config = Some(environment.config.clone());
        let config = crate::Config {
            runtime_dir: root.join("run"),
            state_dir: root.join("workspace"),
            allowed_root: root.clone(),
            launch,
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 16,
        };
        Self {
            root,
            config,
            environment,
        }
    }
    fn folder(&self) -> PathBuf {
        self.environment.config.join("cortex")
    }
    fn write(&self, name: &str, body: &str) {
        crate::local::private_tree(&self.folder()).unwrap();
        let path = self.folder().join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn bind(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => json!(s
                .replace("$CONFIG", self.environment.config.to_str().unwrap())
                .replace("$ROOT", self.root.to_str().unwrap())),
            Value::Array(a) => json!(a.iter().map(|v| self.bind(v)).collect::<Vec<_>>()),
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, v)| (k.clone(), self.bind(v))).collect())
            }
            v => v.clone(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn mcp_params() -> Value {
    json!({"surface":"mcp_servers","value":[{"name":"tools","command":"/never/launch",
    "enabled":true,"allowedTools":["read"]}],"tokens":{"mcp_servers":"absent"}})
}

#[test]
fn machine_save_complete_replies_and_files_match_the_frozen_oracle() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/machine-save-python.json"
    ))
    .unwrap();
    assert_eq!(fixture["processNetworkKeyringAttempts"], 0);
    let mut observations = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let mut f = Fixture::new();
        for (name, body) in case["initial"].as_object().unwrap() {
            f.write(name, body.as_str().unwrap());
        }
        for item in case["stateFiles"].as_array().into_iter().flatten() {
            let path = f
                .environment
                .state
                .join("cortex")
                .join(item["path"].as_str().unwrap());
            crate::local::private_tree(path.parent().unwrap()).unwrap();
            fs::write(&path, item["raw"].as_str().unwrap()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::File::open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(
                    std::time::UNIX_EPOCH + Duration::from_secs(item["mtime"].as_u64().unwrap()),
                ))
                .unwrap();
        }
        if case["override"] == true {
            f.environment.routing_override = Some(f.folder().join("override.json"));
        }
        for o in case["observations"].as_array().unwrap() {
            let params = &o["params"];
            let result = if case["surface"] == "routing" {
                user_routing::save(&f.config, &f.environment, params)
            } else {
                user_mcp::save(&f.config, params)
            };
            let actual = match result {
                Ok(v) => json!({"ok":true,"result":v}),
                Err(e) => json!({"ok":false,"code":e.code,"message":e.message}),
            };
            assert_eq!(actual, f.bind(&o["reply"]), "{}", case["name"]);
            let mut files = json!({});
            for entry in fs::read_dir(f.folder()).unwrap() {
                let p = entry.unwrap().path();
                let name = p.file_name().unwrap().to_str().unwrap();
                if name == "connections.lock" {
                    continue;
                }
                files[name] = json!({"body":fs::read_to_string(&p).unwrap(),"mode":fs::metadata(&p).unwrap().permissions().mode()&0o777});
            }
            assert_eq!(files, f.bind(&o["files"]), "{} files", case["name"]);
            observations += 1;
        }
        assert!(!f.config.state_dir.exists());
        assert!(!f.root.join(".cortex").exists());
    }
    assert!(observations >= 168);
}
#[test]
fn machine_mcp_preflight_and_unsafe_files_never_write() {
    let f = Fixture::new();
    crate::local::private_tree(&f.folder()).unwrap();
    let path = f.folder().join("mcp-servers.json");
    let outside = f.root.join("untouched");
    fs::write(&outside, "private").unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert_eq!(
        user_mcp::save(&f.config, &mcp_params()).err().unwrap().code,
        "unsafe_storage"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), "private");
    fs::remove_file(&path).unwrap();
    fs::hard_link(&outside, &path).unwrap();
    assert_eq!(
        user_mcp::save(&f.config, &mcp_params()).err().unwrap().code,
        "unsafe_storage"
    );
    fs::remove_file(&path).unwrap();
    let mut params = mcp_params();
    params["value"][0]["args"] = json!(["x".repeat(FRAME_BUDGET)]);
    assert_eq!(
        user_mcp::save(&f.config, &params).err().unwrap().code,
        "machine_reply_too_large"
    );
    assert!(!path.exists());
}
#[test]
fn machine_mcp_concurrent_writers_admit_one_revision() {
    let f = Fixture::new();
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|s| {
        let handles = (0..2)
            .map(|_| {
                s.spawn(|| {
                    barrier.wait();
                    user_mcp::save(&f.config, &mcp_params())
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results.into_iter().find_map(|r| r.err()).unwrap().code,
        "revision_conflict"
    );
    assert!(f.folder().join("mcp-servers.json").is_file());
}
#[test]
fn machine_mcp_uncertain_sync_never_replays() {
    let f = Fixture::new();
    let path = f.folder().join("mcp-servers.json");
    let mut calls = 0;
    let result = user_mcp::save_with(&f.config, &mcp_params(), |p, b| {
        calls += 1;
        replace_sync(p, b, |_| Err(refuse("injected_sync", "injected")))
    });
    assert_eq!(result.err().unwrap().code, "machine_outcome_unknown");
    assert_eq!(calls, 1);
    let bytes = fs::read(&path).unwrap();
    assert!(serde_json::from_slice::<Value>(&bytes).unwrap()["servers"]["tools"].is_object());
    assert_eq!(fs::read_dir(f.folder()).unwrap().count(), 3);
    assert_eq!(
        user_mcp::save(&f.config, &mcp_params()).err().unwrap().code,
        "revision_conflict"
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
}
#[test]
fn machine_routing_projection_failure_precedes_commit() {
    let f = Fixture::new();
    let mut environment = f.environment.clone();
    environment.state = PathBuf::from("relative");
    let p = json!({"surface":"routing","value":{"version":2},"tokens":{"routing":"absent"}});
    assert!(user_routing::save(&f.config, &environment, &p).is_err());
    assert!(!f.folder().join("routing.json").exists());
    environment = f.environment.clone();
    environment.config = f.root.join("different");
    assert_eq!(
        user_routing::save(&f.config, &environment, &p)
            .err()
            .unwrap()
            .code,
        "native_config_unavailable"
    );
    assert!(!f.folder().join("routing.json").exists());
}
#[test]
fn machine_mcp_containment_is_kept_inherited_or_explicit() {
    let f = Fixture::new();
    f.write(
        "mcp-servers.json",
        r#"{"version":1,"containment":"none","servers":{
            "a":{"command":"/never/launch","allowed_tools":["read"]},
            "b":{"command":"/never/launch","allowed_tools":["read"],"containment":"contained"}}}"#,
    );
    let server = |name: &str, containment: Option<&str>| {
        let mut s =
            json!({"name":name,"command":"/never/launch","enabled":true,"allowedTools":["read"]});
        if let Some(c) = containment {
            s["containment"] = json!(c);
        }
        s
    };
    // `a` asks for the default (stays inherited), `b` omits the field (kept),
    // `c` is new and explicitly contained against the uncontained default.
    let params = json!({"surface":"mcp_servers","value":[
        server("a", Some("none")), server("b", None), server("c", Some("contained"))]});
    let reply = user_mcp::save(&f.config, &params).unwrap();
    assert_eq!(reply["changed"], true);
    assert_eq!(reply["containment"], "none");
    let path = f.folder().join("mcp-servers.json");
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["containment"], "none");
    assert!(saved["servers"]["a"].get("containment").is_none());
    assert_eq!(saved["servers"]["b"]["containment"], "contained");
    assert_eq!(saved["servers"]["c"]["containment"], "contained");
    let rows = reply["servers"].as_array().unwrap();
    assert!(rows
        .iter()
        .all(|r| r["valid"] == true && r["unsupported_keys"] == json!([])));
    // Unknown values are refused and nothing is written.
    let before = fs::read(&path).unwrap();
    for bad in ["off", "None", ""] {
        let params = json!({"surface":"mcp_servers","value":[
            server("a", Some(bad)), server("b", None), server("c", None)]});
        assert!(user_mcp::save(&f.config, &params).is_err(), "{bad}");
    }
    assert_eq!(fs::read(&path).unwrap(), before);
    // An invalid registry default refuses the whole surface.
    f.write(
        "mcp-servers.json",
        r#"{"version":1,"containment":"off","servers":{}}"#,
    );
    assert_eq!(
        user_mcp::save(&f.config, &mcp_params()).err().unwrap().code,
        "invalid_mcp_registry"
    );
}

fn project_params(servers: Value, overrides: Value, token: &str) -> Value {
    json!({"surface":"mcp_servers","value":{"servers":servers,"overrides":overrides},
        "tokens":{"mcp_servers":token}})
}
#[test]
fn project_mcp_registry_is_private_per_project_and_leaves_the_machine_file_alone() {
    let f = Fixture::new();
    f.write(
        "mcp-servers.json",
        r#"{"version":1,"servers":{"kling":{"command":"/never/launch","allowed_tools":["read"]}}}"#,
    );
    let machine = fs::read(f.folder().join("mcp-servers.json")).unwrap();
    let (a, b) = (f.root.join("a"), f.root.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    // Nothing yet: absent token, no servers, no file created by reading.
    let (view, token) = reader::project_mcp(&a, &f.config);
    assert_eq!((token.as_str(), &view["servers"], &view["overrides"]), ("absent", &json!([]), &json!({})));
    assert!(!user_mcp::project_path(&a, &f.config).unwrap().exists());
    // Another name and a secret; the value is never read back.
    let server = json!({"name":"mine","command":"/never/launch","enabled":true,"allowedTools":["read"],
        "env":[{"name":"API_KEY","value":"s3cret","action":"replace"}]});
    let reply = user_mcp::save_project(
        &a,
        &f.config,
        &project_params(json!([server]), json!({"kling":false,"extra":true}), "absent"),
    )
    .unwrap();
    assert_eq!(reply["changed"], true);
    assert_eq!(reply["overrides"], json!({"extra":true,"kling":false}));
    let path = user_mcp::project_path(&a, &f.config).unwrap();
    assert!(path.starts_with(f.folder().join("projects")) && !path.starts_with(&a));
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["overrides"], json!({"extra":true,"kling":false}));
    assert_eq!(saved["servers"]["mine"]["env"]["API_KEY"], "s3cret");
    // The machine registry and another project are untouched.
    assert_eq!(fs::read(f.folder().join("mcp-servers.json")).unwrap(), machine);
    assert!(!user_mcp::project_path(&b, &f.config).unwrap().exists());
    // The editor sees presence only, and keeps a stored value with `keep`.
    let (view, token) = reader::project_mcp(&a, &f.config);
    assert_eq!(view["overrides"], json!({"extra":true,"kling":false}));
    assert_eq!(view["servers"][0]["env"], json!([{"name":"API_KEY","stored":true,"action":"keep","value":""}]));
    assert!(!view.to_string().contains("s3cret"));
    let mut kept = server.clone();
    kept["env"] = json!([{"name":"API_KEY","stored":true,"action":"keep","value":""}]);
    kept["args"] = json!(["--x"]);
    let reply = user_mcp::save_project(&a, &f.config, &project_params(json!([kept]), json!({"kling":false}), &token)).unwrap();
    assert_eq!(reply["changed"], true);
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["servers"]["mine"]["env"]["API_KEY"], "s3cret");
    assert_eq!(saved["servers"]["mine"]["args"], json!(["--x"]));
    // A stale revision is refused and nothing changes.
    let before = fs::read(&path).unwrap();
    let stale = user_mcp::save_project(&a, &f.config, &project_params(json!([]), json!({}), &token)).err().unwrap();
    assert_eq!(stale.code, "revision_conflict");
    assert_eq!(fs::read(&path).unwrap(), before);
    // The same validation as the machine editor: tools are required, 'cortex' is reserved,
    // an override cannot also be a project entry, and bad names are refused.
    let (_, token) = reader::project_mcp(&a, &f.config);
    for (servers, overrides) in [
        (json!([{"name":"x","command":"/never/launch","enabled":true}]), json!({})),
        (json!([{"name":"cortex","command":"/never/launch","enabled":true,"allowedTools":["r"]}]), json!({})),
        (json!([server.clone()]), json!({"mine":false})),
        (json!([server.clone()]), json!({"cortex":false})),
        (json!([server.clone()]), json!({"x":7})),
        (json!([server.clone()]), json!(["x"])),
    ] {
        assert!(
            user_mcp::save_project(&a, &f.config, &project_params(servers, overrides, &token)).is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    // Removing everything leaves a valid, empty project registry.
    let reply = user_mcp::save_project(&a, &f.config, &project_params(json!([]), json!({}), &token)).unwrap();
    assert_eq!(reply["changed"], true);
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["servers"], json!({}));
    assert!(saved.get("overrides").is_none());
    // A hand-damaged project file is shown as a problem, never as an empty registry,
    // and the save refuses to overwrite it.
    fs::write(&path, b"{not json").unwrap();
    let (view, _) = reader::project_mcp(&a, &f.config);
    assert!(view["problem"].is_string());
    assert!(user_mcp::save_project(&a, &f.config, &project_params(json!([]), json!({}), "absent")).is_err());
}
