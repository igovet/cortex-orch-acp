//! Actual machine socket with disposable HOME/XDG and offline fixture producers.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use workspace_service::{connect, read_frame, write_frame};

#[path = "support/config_catalog_producer.rs"]
mod catalog_producer;
struct Fixture {
    root: PathBuf,
    child: Option<Child>,
}
impl Fixture {
    fn new() -> Self {
        Self::named("cas")
    }
    fn named(tag: &str) -> Self {
        let root = PathBuf::from(format!("/tmp/cw-{tag}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for name in [
            "home", "config", "state", "data", "runtime", "bin", "projects",
        ] {
            fs::create_dir(root.join(name)).unwrap();
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self { root, child: None }
    }
    fn config(&self) -> PathBuf {
        if workspace_protocol::channel::current().is_dev() {
            workspace_protocol::channel::current().config_dir(&self.root.join("config"))
        } else {
            self.root.join("config")
        }
    }
    fn credentials_state(&self) -> PathBuf {
        if workspace_protocol::channel::current().is_dev() {
            workspace_protocol::channel::current()
                .state_dir(&self.root.join("state"))
                .join("state")
        } else {
            self.root.join("state")
        }
    }
    fn socket(&self) -> PathBuf {
        workspace_protocol::channel::current()
            .runtime_dir(&self.root.join("runtime"))
            .join("service.sock")
    }
    fn path(&self) -> PathBuf {
        self.config().join("cortex/connections.json")
    }
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let reply = self.wire(method, params).await;
        assert!(reply.get("error").is_none(), "{reply}");
        reply["result"].clone()
    }
    async fn wire(&self, method: &str, params: Value) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut s = connect(&self.socket()).await.unwrap();
            write_frame(
                &mut s,
                &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
            )
            .await
            .unwrap();
            let reply = read_frame(&mut s).await.unwrap();
            assert_eq!(reply["id"], 1);
            reply
        })
        .await
        .unwrap()
    }
    async fn action(&self, params: Value) -> Value {
        self.rpc(
            "machine.call",
            json!({"op":"config.action","params":params}),
        )
        .await
    }
    async fn start(&mut self) {
        self.spawn().await;
    }
    async fn spawn(&mut self) {
        let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_owned();
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_workspace-service"))
                .arg(self.root.join("projects"))
                .arg(assets)
                .env_clear()
                .env("HOME", self.root.join("home"))
                .env("PATH", self.root.join("bin"))
                .env("XDG_CONFIG_HOME", self.root.join("config"))
                .env("XDG_STATE_HOME", self.root.join("state"))
                .env("XDG_DATA_HOME", self.root.join("data"))
                .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.socket().exists() {
                assert!(self.child.as_mut().unwrap().try_wait().unwrap().is_none());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(self.rpc("service.status", json!({})).await.is_object());
    }
    async fn stop(&mut self) {
        self.rpc("service.stop", json!({})).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        self.child.take();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn catalog_refresh_oracles(f: &Fixture) {
    let c: Value =
        serde_json::from_str(include_str!("fixtures/config-catalog-refresh-python.json")).unwrap();
    assert_eq!(c["processNetworkKeyringAttempts"], 0);
    let roots = [
        ("home", f.root.join("home")),
        ("config/cortex", f.config().join("cortex")),
        ("state/cortex", f.credentials_state().join("cortex")),
    ];
    let expand = |value: &Value| -> Value {
        serde_json::from_str(
            &value
                .to_string()
                .replace("@ROOT@", f.root.to_str().unwrap()),
        )
        .unwrap()
    };
    fn tree(path: &std::path::Path, key: &str, out: &mut Value) {
        let meta = fs::symlink_metadata(path).unwrap();
        assert!(!meta.file_type().is_symlink(), "unexpected link {key}");
        let mode = meta.permissions().mode() & 0o777;
        if meta.is_dir() {
            out[key] = json!({"directory":true,"mode":mode});
            for entry in fs::read_dir(path).unwrap() {
                let p = entry.unwrap().path();
                tree(
                    &p,
                    &format!("{key}/{}", p.file_name().unwrap().to_str().unwrap()),
                    out,
                );
            }
        } else {
            assert!(meta.is_file());
            out[key] = json!({"content":fs::read_to_string(path).unwrap(),"mode":mode});
        }
    }
    for case in c["cases"].as_array().unwrap() {
        for (_, top) in &roots {
            // Undo only this fixture's private read-only-directory setup.
            let native = top.join("native");
            if native.exists() {
                fs::set_permissions(native, fs::Permissions::from_mode(0o700)).unwrap();
            }
            if top.exists() {
                fs::remove_dir_all(top).unwrap();
            }
        }
        let initial = expand(&case["initial"]);
        for (relative, item) in initial.as_object().unwrap() {
            let (prefix, top) = roots
                .iter()
                .find(|(prefix, _)| {
                    relative == prefix || relative.starts_with(&format!("{prefix}/"))
                })
                .unwrap();
            let path = top.join(
                relative
                    .strip_prefix(prefix)
                    .unwrap()
                    .trim_start_matches('/'),
            );
            if item["directory"] == true {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, item["content"].as_str().unwrap()).unwrap();
            }
            if item["directory"] != true {
                fs::set_permissions(
                    path,
                    fs::Permissions::from_mode(item["mode"].as_u64().unwrap() as u32),
                )
                .unwrap();
            }
        }
        for (relative, item) in initial.as_object().unwrap().iter().rev() {
            if item["directory"] == true {
                let (prefix, top) = roots
                    .iter()
                    .find(|(prefix, _)| {
                        relative == prefix || relative.starts_with(&format!("{prefix}/"))
                    })
                    .unwrap();
                fs::set_permissions(
                    top.join(
                        relative
                            .strip_prefix(prefix)
                            .unwrap()
                            .trim_start_matches('/'),
                    ),
                    fs::Permissions::from_mode(item["mode"].as_u64().unwrap() as u32),
                )
                .unwrap();
            }
        }
        let actual = f.action(case["params"].clone()).await;
        let mut expected = expand(&case["reply"]);
        // Policy-disabled rows remain listed but are no longer selectable.
        if let Some(routes) = expected
            .get_mut("result")
            .and_then(|v| v.get_mut("routes"))
            .and_then(Value::as_array_mut)
        {
            for route in routes {
                route["selectable"] =
                    json!(route["selectable"] == true && route["disabled"] != true);
            }
        }
        assert_eq!(actual, expected, "catalog refresh {}", case["name"]);
        let mut files = json!({});
        for (prefix, top) in &roots {
            if top.exists() {
                tree(top, prefix, &mut files);
            }
        }
        assert_eq!(
            files,
            expand(&case["after"]),
            "catalog full tree/mode effects {}",
            case["name"]
        );
    }
    // A keyring-backed sibling retains the WHOLE refresh before any effects.
    // These two requests qualify selection/no-replay only, not a real launch.
    let metadata: Value =
        serde_json::from_str(include_str!("fixtures/config-action-python.json")).unwrap();
    let mut registry: Value =
        serde_json::from_str(metadata["cases"][0]["initial"].as_str().unwrap()).unwrap();
    registry["connections"]["cn_0123456789ab"]["launch_command"] = Value::Null;
    registry["connections"]["cn_abcdef012345"]["auth_mode"] = json!("api_key");
    registry["connections"]["cn_abcdef012345"]["key_kind"] = json!("anthropic_api_key");
    registry["connections"]["cn_abcdef012345"]["credential_ref"] =
        json!({"kind":"os_keyring","service":"fixture","account":"synthetic"});
    let bytes = serde_json::to_vec(&registry).unwrap();
    fs::write(f.path(), &bytes).unwrap();
    fs::set_permissions(f.path(), fs::Permissions::from_mode(0o600)).unwrap();
    // Native: the keyring-backed sibling is an explicit failed row (the keyring is
    // never opened), the registry is untouched and no provider is called.
    let reply = f.action(json!({"action":"refresh"})).await;
    assert_eq!(reply["ok"], true, "{reply}");
    let rows = reply["result"]["refresh"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let keyring = rows
        .iter()
        .find(|r| r["connection_id"] == "cn_abcdef012345")
        .unwrap();
    assert_eq!(keyring["success"], false);
    assert_eq!(keyring["reason"], "engine_unreachable:keyring_unobservable");
    assert_eq!(fs::read(f.path()).unwrap(), bytes);
}

async fn apply_action_oracles(f: &Fixture) {
    let c: Value = serde_json::from_str(include_str!("fixtures/config-apply-python.json")).unwrap();
    let folder = f.config().join("cortex");
    for case in c["cases"].as_array().unwrap() {
        fs::remove_dir_all(&folder).unwrap();
        fs::create_dir(&folder).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
        for (n, b) in case["initial"].as_object().unwrap() {
            let p = folder.join(n);
            fs::write(&p, b.as_str().unwrap()).unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let actual = f.action(case["params"].clone()).await;
        let mut observed = json!({});
        for e in fs::read_dir(&folder).unwrap() {
            let p = e.unwrap().path();
            observed[p.file_name().unwrap().to_str().unwrap()] = json!({"content":fs::read_to_string(&p).unwrap(),
                "mode":fs::metadata(&p).unwrap().permissions().mode() & 0o777});
        }
        let now = observed["routing-profiles.json"]["content"]
            .as_str()
            .and_then(|b| serde_json::from_str::<Value>(b).ok())
            .and_then(|v| v["applied"]["at"].as_str().map(str::to_owned))
            .unwrap_or_else(|| c["now"].as_str().unwrap().into());
        assert!(
            regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6}\+00:00$")
                .unwrap()
                .is_match(&now)
        );
        let expand = |v: &Value| -> Value {
            serde_json::from_str(
                &v.to_string()
                    .replace("$CONFIG", folder.to_str().unwrap())
                    .replace(c["now"].as_str().unwrap(), &now),
            )
            .unwrap()
        };
        let files = expand(&case["files"]);
        assert_eq!(observed, files, "apply {}", case["name"]);
        assert_eq!(
            fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            case["directory_mode"].as_u64().unwrap() as u32
        );
        let mut expected = expand(&case["reply"]);
        if let Some(b) = files["routing-profiles.json"]["content"].as_str() {
            expected["result"]["files"]["routing_profiles"]["revision"] =
                json!(format!("sha256:{:x}", Sha256::digest(b.as_bytes())));
        }
        assert_eq!(actual, expected, "apply {}", case["name"]);
    }
    // Four-section activation now stays native even after preset application.
    let read = f.action(json!({"action":"profile","verb":"read"})).await;
    assert_eq!(read["ok"], true);
    let activated = f
        .action(
            json!({"action":"profile","verb":"activate","name":"default",
        "expected_revision":read["result"]["revision"],
        "expected_revisions":read["result"]["current_revisions"],"_fixture_real":true}),
        )
        .await;
    assert_eq!(activated["ok"], true, "{activated}");
}
fn credential_snapshot(f: &Fixture) -> Value {
    fn walk(path: &std::path::Path, key: &str, output: &mut Value) {
        let meta = fs::symlink_metadata(path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        output[key] = if meta.file_type().is_symlink() {
            json!({"link":fs::read_link(path).unwrap().to_str().unwrap(),"mode":mode})
        } else if meta.is_dir() {
            json!({"directory":true,"mode":mode})
        } else {
            json!({"content":fs::read_to_string(path).unwrap(),"mode":mode})
        };
        if meta.is_dir() {
            for e in fs::read_dir(path).unwrap() {
                let p = e.unwrap().path();
                walk(
                    &p,
                    &format!("{key}/{}", p.file_name().unwrap().to_str().unwrap()),
                    output,
                );
            }
        }
    }
    let mut result = json!({});
    for (top, prefix) in [
        (f.config().join("cortex"), "config/cortex"),
        (f.credentials_state().join("cortex"), "state/cortex"),
    ] {
        if top.exists() {
            walk(&top, prefix, &mut result);
        }
    }
    for e in fs::read_dir(f.root.join("home")).unwrap() {
        let p = e.unwrap().path();
        walk(
            &p,
            &format!("home/{}", p.file_name().unwrap().to_str().unwrap()),
            &mut result,
        );
    }
    result
}
async fn connection_action_oracles(f: &Fixture) {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/config-connections-python.json")).unwrap();
    assert_eq!(corpus["processNetworkKeyringAttempts"], 0);
    let folder = f.path().parent().unwrap().to_owned();
    let token = |body: Option<&str>| {
        body.map_or("absent".into(), |s| {
            format!("sha256:{:x}", Sha256::digest(s.as_bytes()))
        })
    };
    let root_expand = |value: &Value| -> Value {
        serde_json::from_str(
            &value
                .to_string()
                .replace("$CONFIG", folder.to_str().unwrap())
                .replace("$HOME", f.root.join("home").to_str().unwrap())
                .replace("$STATE", f.credentials_state().to_str().unwrap()),
        )
        .unwrap()
    };
    let secrets: Value =
        serde_json::from_str(include_str!("fixtures/config-secret-remove-python.json")).unwrap();
    assert_eq!(secrets["processNetworkKeyringAttempts"], 0);
    for case in corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(secrets["cases"].as_array().unwrap())
    {
        for path in [
            folder.clone(),
            f.root.join("home"),
            f.credentials_state().join("cortex"),
        ] {
            if path.exists() {
                fs::remove_dir_all(&path).unwrap();
            }
        }
        fs::create_dir(f.root.join("home")).unwrap();
        fs::set_permissions(f.root.join("home"), fs::Permissions::from_mode(0o700)).unwrap();
        let initial = root_expand(&case["initial"]);
        let location = |key: &str| {
            if let Some(rest) = key.strip_prefix("config/") {
                f.config().join(rest)
            } else if let Some(rest) = key.strip_prefix("state/") {
                f.credentials_state().join(rest)
            } else {
                f.root.join(key)
            }
        };
        for (key, entry) in initial.as_object().unwrap() {
            let path = location(key);
            if entry["directory"] == true {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                if let Some(link) = entry["link"].as_str() {
                    std::os::unix::fs::symlink(link, &path).unwrap();
                    continue;
                }
                fs::write(&path, entry["content"].as_str().unwrap()).unwrap();
            }
            fs::set_permissions(
                path,
                fs::Permissions::from_mode(entry["mode"].as_u64().unwrap() as u32),
            )
            .unwrap();
        }
        let revision = token(initial["config/cortex/connections.json"]["content"].as_str());
        let request: Value = serde_json::from_str(
            &root_expand(&case["params"])
                .to_string()
                .replace("$REVISION", &revision),
        )
        .unwrap();
        let actual = f.action(request).await;
        let mut files = root_expand(&case["files"]);
        let mut expected = root_expand(&case["reply"]);
        if files.to_string().contains(corpus["now"].as_str().unwrap()) {
            let now = actual["result"]["connections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == case["params"]["connection_id"])
                .unwrap()["updated_at"]
                .as_str()
                .unwrap();
            assert!(
                regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6}\+00:00$")
                    .unwrap()
                    .is_match(now),
                "case {} timestamp {now}",
                case["name"]
            );
            files = serde_json::from_str(
                &files
                    .to_string()
                    .replace(corpus["now"].as_str().unwrap(), now),
            )
            .unwrap();
            expected = serde_json::from_str(
                &expected
                    .to_string()
                    .replace(corpus["now"].as_str().unwrap(), now),
            )
            .unwrap();
        }
        let after_revision = token(files["config/cortex/connections.json"]["content"].as_str());
        expected = serde_json::from_str(
            &expected
                .to_string()
                .replace("$AFTER_REVISION", &after_revision)
                .replace("$REVISION", &revision),
        )
        .unwrap();
        assert_eq!(actual, expected, "connection reply {}", case["name"]);
        assert_eq!(
            credential_snapshot(f),
            files,
            "connection files {}",
            case["name"]
        );
    }
    // Keyring validation and removal over unreadable defaults are explicit native
    // refusals (no keyring backend; no guess at the default route); secret deletion is native.
    for (kind, operation, malformed_defaults) in [
        ("os_keyring", "validate", false),
        ("secret_file", "remove", false),
        ("os_keyring", "remove", true),
    ] {
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["name"]
                    == if kind == "os_keyring" {
                        "remove-keyring"
                    } else {
                        "validate-secret-nonempty"
                    }
            })
            .unwrap();
        let initial = root_expand(&case["initial"]);
        let mut document: Value = serde_json::from_str(
            initial["config/cortex/connections.json"]["content"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let id = "cn_0123456789ab";
        document["connections"][id]["state"] = json!("invalid");
        if kind == "secret_file" {
            let secret = folder.join("secrets").join(id);
            fs::create_dir_all(secret.parent().unwrap()).unwrap();
            fs::set_permissions(secret.parent().unwrap(), fs::Permissions::from_mode(0o700))
                .unwrap();
            fs::write(&secret, b"synthetic metadata fixture").unwrap();
            fs::set_permissions(secret, fs::Permissions::from_mode(0o600)).unwrap();
        }
        if malformed_defaults {
            let path = folder.join("defaults.json");
            fs::write(&path, b"{broken").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let bytes = serde_json::to_vec(&document).unwrap();
        fs::write(f.path(), &bytes).unwrap();
        fs::set_permissions(f.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let result = f
            .action(json!({"action":"connection_update", "operation":operation,
            "connection_id":id, "expected_revision":format!("sha256:{:x}",Sha256::digest(&bytes))}))
            .await;
        // The retained provider is never called.
        if kind == "os_keyring" {
            assert_eq!(result["ok"], false, "{operation}/{kind}: {result}");
            assert_eq!(
                result["code"],
                if malformed_defaults {
                    "invalid_settings"
                } else {
                    "config_action_unavailable"
                },
                "{result}"
            );
            if malformed_defaults {
                assert!(result["message"]
                    .as_str()
                    .unwrap()
                    .contains("defaults.json"));
                assert_eq!(fs::read(folder.join("defaults.json")).unwrap(), b"{broken");
            }
            assert_eq!(fs::read(f.path()).unwrap(), bytes, "refusal must not write");
            continue;
        }
        assert_eq!(result["ok"], true, "{operation}/{kind}: {result}");
        let after: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
        if operation == "remove" {
            assert!(after["connections"].get(id).is_none());
            assert!(after["auto_connect_disabled"]
                .as_array()
                .unwrap()
                .contains(&json!("codex-acp")));
            assert!(!folder.join("secrets").join(id).exists());
        } else {
            assert_eq!(after["connections"][id]["state"], "unavailable");
        }
        if malformed_defaults {
            assert_eq!(fs::read(folder.join("defaults.json")).unwrap(), b"{broken");
        }
    }
}

#[tokio::test]
async fn provider_less_keyring_refresh_refuses_without_import_failure_effects() {
    let mut f = Fixture::named("cas-keyring");
    f.start().await;
    catalog_producer::check_keyring_less(&f).await;
    f.stop().await;
}

#[tokio::test]
async fn action_socket_selects_complete_metadata_and_retains_every_other_action() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/config-action-python.json")).unwrap();
    let mut f = Fixture::new();
    f.start().await;
    // Every previously refused action reaches its original producer once. External
    // producers use exactly the named offline fakes in the retained Python capture.
    for item in corpus["explicitlyUnavailable"].as_array().unwrap() {
        // Reset now uses real captures below, not the old named producer fake.
        if item["params"]["action"] == "apply_default_preset"
            || item["params"]["action"] == "reset_default_preset"
            || item["params"]["action"] == "profile"
            || item["params"]["action"] == "connection_update"
        {
            continue;
        }
        // These bare requests faked the whole Python handler; its real pre-effect
        // refusals are now native and compared with real captures below.
        if item["params"]["action"] == "install_engine"
            || item["params"]["action"] == "onboard_opencode"
        {
            continue;
        }
        // Bare refresh is native: with no registry it launches nothing and reports
        // an empty outcome list, creating no owner file.
        let reply = f.action(item["params"].clone()).await;
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["result"]["refresh"], json!([]));
        assert!(!f.path().exists());
    }
    let boundary: Value = serde_json::from_str(include_str!(
        "fixtures/settings-native-boundary-python.json"
    ))
    .unwrap();
    for case in boundary["refusals"].as_array().unwrap() {
        if case["initial"].as_object().unwrap().is_empty() {
            assert_eq!(f.action(case["params"].clone()).await, case["reply"]);
            assert!(!f.config().join("cortex").exists());
        }
    }
    // Pinned installation is native (engine-install-python.json). Here PATH has
    // no node, so it is the retained pre-effect refusal, with no helper call.
    for engine in ["codex-acp", "claude-agent-acp"] {
        assert_eq!(
            f.action(json!({"action":"install_engine","engine":engine}))
                .await,
            json!({"ok":false,"code":"engine_install_failed",
                "message":"Pinned installation is unavailable, busy or failed verification."})
        );
        assert!(!f.root.join("home/.local/share/cortex").exists());
        assert!(!f.path().exists());
    }
    // A candidate on PATH whose sandboxed `--version` prints no version is not a
    // usable OpenCode: native refusal, no helper call, no registry write. (A
    // working fake is onboarded in the unit oracle, never the owner's real binary.)
    fs::write(f.root.join("bin/opencode"), "#!/bin/sh\nexit 97\n").unwrap();
    fs::set_permissions(
        f.root.join("bin/opencode"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    {
        let reply = f
            .action(
                json!({"action":"onboard_opencode","explicit_opt_in":true,"expected_revision":"absent"}),
            )
            .await;
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["code"], "opencode_not_found");
        assert!(!f.path().exists());
    }
    fs::create_dir_all(f.path().parent().unwrap()).unwrap();
    fs::set_permissions(
        f.path().parent().unwrap(),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    for case in corpus["cases"].as_array().unwrap() {
        // Fixed-clock no-op is compared in the unit oracle; production uses UTC now.
        if case["name"] == "no-op" {
            continue;
        }
        fs::write(f.path(), case["initial"].as_str().unwrap()).unwrap();
        fs::set_permissions(f.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let mut p = case["params"].clone();
        // Untrusted roots/refresh flags cannot select another storage target.
        p["configRoot"] = json!("/not-an-authorized-target");
        p["refresh"] = json!(true);
        let actual = f.action(p).await;
        let mut expected = case["reply"].clone();
        let mut expected_bytes = case["after"].as_str().unwrap().as_bytes().to_vec();
        if actual["ok"] == true {
            let id = case["params"]["connection_id"].as_str().unwrap();
            let now = actual["result"]["connections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == id)
                .unwrap()["updated_at"]
                .as_str()
                .unwrap();
            assert!(
                regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6}\+00:00$")
                    .unwrap()
                    .is_match(now)
            );
            let mut body: Value = serde_json::from_slice(&expected_bytes).unwrap();
            body["connections"][id]["updated_at"] = json!(now);
            expected_bytes = serde_json::to_vec(&body).unwrap();
            for record in expected["result"]["connections"].as_array_mut().unwrap() {
                if record["id"] == id {
                    record["updated_at"] = json!(now);
                }
            }
            expected["result"]["revision"] =
                json!(format!("sha256:{:x}", Sha256::digest(&expected_bytes)));
        }
        assert_eq!(actual, expected, "{}", case["name"]);
        assert_eq!(
            fs::read(f.path()).unwrap(),
            expected_bytes,
            "{}",
            case["name"]
        );
        assert_eq!(
            fs::metadata(f.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let edge: Value =
        serde_json::from_str(include_str!("fixtures/config-metadata-python.json")).unwrap();
    for case in edge["cases"].as_array().unwrap() {
        let folder = f.path().parent().unwrap().to_owned();
        for name in ["connections.json", "connections.lock", ".gitignore"] {
            let path = folder.join(name);
            if path.exists() {
                fs::remove_file(path).unwrap();
            }
        }
        if let Some(initial) = case["initial"].as_str() {
            fs::write(f.path(), initial).unwrap();
            fs::set_permissions(f.path(), fs::Permissions::from_mode(0o600)).unwrap();
        }
        if let Some(ignore) = case["ignore"].as_str() {
            fs::write(folder.join(".gitignore"), ignore).unwrap();
            fs::set_permissions(folder.join(".gitignore"), fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let actual = f.action(case["params"].clone()).await;
        let mut expected: Value = serde_json::from_str(
            &case["reply"]
                .to_string()
                .replace("$CONFIG", &folder.display().to_string()),
        )
        .unwrap();
        let mut files = case["files"].clone();
        if actual["ok"] == true {
            let id = case["params"]["connection_id"].as_str().unwrap();
            let now = actual["result"]["connections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == id)
                .unwrap()["updated_at"]
                .as_str()
                .unwrap();
            files["connections.json"]["content"] = json!(files["connections.json"]["content"]
                .as_str()
                .unwrap()
                .replace(edge["now"].as_str().unwrap(), now));
            for record in expected["result"]["connections"].as_array_mut().unwrap() {
                if record["id"] == id {
                    record["updated_at"] = json!(now);
                }
            }
            expected["result"]["revision"] = json!(format!(
                "sha256:{:x}",
                Sha256::digest(
                    files["connections.json"]["content"]
                        .as_str()
                        .unwrap()
                        .as_bytes()
                )
            ));
        }
        assert_eq!(actual, expected, "socket {}", case["name"]);
        let mut observed = json!({});
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            observed[path.file_name().unwrap().to_str().unwrap()] = json!({
                "content":fs::read_to_string(&path).unwrap(),
                "mode":fs::metadata(&path).unwrap().permissions().mode() & 0o777});
        }
        assert_eq!(observed, files, "socket {}", case["name"]);
    }
    fs::write(f.path().parent().unwrap().join(".gitignore"), b"*\n").unwrap();
    // Unknown action refusal keeps the retained public error contract.
    assert_eq!(
        f.action(json!({"action":"not-real"})).await,
        json!({"ok":false,"code":"unsupported_action","message":"This machine does not support that yet."})
    );
    let reply = f
        .action(json!({"action":"onboard_opencode","explicit_opt_in":true,
            "expected_revision":"absent","_fixture_unknown":true}))
        .await;
    // The registry exists by now, so the stale "absent" revision is refused first.
    assert_eq!(reply["code"], "revision_conflict");
    // Numeric history now works natively, including the private root ignore file.
    let mut registry: Value =
        serde_json::from_str(corpus["cases"][0]["initial"].as_str().unwrap()).unwrap();
    registry["override_history"]["cn_0123456789ab"][0]["fingerprint"] = json!(1.5);
    let bytes = serde_json::to_vec(&registry).unwrap();
    fs::write(f.path(), &bytes).unwrap();
    let reply = f
        .action(json!({"action":"connection_update","operation":"rename",
        "connection_id":"cn_0123456789ab","name":"Float history retained",
        "expected_revision":format!("sha256:{:x}", Sha256::digest(&bytes))}))
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let after: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
    assert_eq!(
        after["override_history"]["cn_0123456789ab"][0]["fingerprint"],
        1.5
    );
    assert!(f.config().join("cortex/.gitignore").is_file());
    // Real preset/profile reads and writes still work (not producer fakes).
    let applied = f
        .action(json!({"action":"apply_default_preset","_fixture_real":true}))
        .await;
    assert_eq!(applied["ok"], true, "{applied}");
    assert!(f.config().join("cortex/routing.json").is_file());
    let profiles = f
        .action(json!({"action":"profile","verb":"read","_fixture_real":true}))
        .await;
    assert_eq!(profiles["ok"], true, "{profiles}");
    let refused = f
        .action(json!({"action":"reset_default_preset","confirm":false,"_fixture_real":true}))
        .await;
    assert_eq!(refused["code"], "confirmation_required");
    let resets: Value =
        serde_json::from_str(include_str!("fixtures/config-reset-python.json")).unwrap();
    for case in resets["cases"].as_array().unwrap() {
        let folder = f.path().parent().unwrap().to_owned();
        for entry in fs::read_dir(&folder).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        for (name, content) in case["initial"].as_object().unwrap() {
            let path = folder.join(name);
            fs::write(&path, content.as_str().unwrap()).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let actual = f.action(case["params"].clone()).await;
        let mut stamp = resets["stamp"].as_str().unwrap().to_owned();
        if let Some(path) = actual["result"]["backups"]
            .as_object()
            .and_then(|m| m.values().next())
            .and_then(Value::as_str)
        {
            stamp = path.rsplit_once(".cortex-backup-").unwrap().1.into();
            assert!(stamp.len() >= 18 && stamp.bytes().all(|b| b.is_ascii_digit()));
        }
        let expand = |value: &Value| -> Value {
            serde_json::from_str(
                &value
                    .to_string()
                    .replace("$CONFIG", folder.to_str().unwrap())
                    .replace(resets["stamp"].as_str().unwrap(), &stamp),
            )
            .unwrap()
        };
        assert_eq!(actual, expand(&case["reply"]), "reset {}", case["name"]);
        let mut files = json!({});
        for e in fs::read_dir(&folder).unwrap() {
            let path = e.unwrap().path();
            files[path.file_name().unwrap().to_str().unwrap()] = json!({
                "content":fs::read_to_string(&path).unwrap(),
                "mode":fs::metadata(&path).unwrap().permissions().mode() & 0o777});
        }
        assert_eq!(files, expand(&case["files"]), "reset {}", case["name"]);
    }
    // All six native profile verbs compare real complete Python effects/replies,
    // not the previous named fake producers. Only unqualified shapes retain Python.
    for source in [
        include_str!("fixtures/config-profiles-python.json"),
        include_str!("fixtures/config-profiles-edges-python.json"),
        include_str!("fixtures/config-activate-python.json"),
        include_str!("fixtures/config-activate-full-python.json"),
        include_str!("fixtures/config-activate-materialize-python.json"),
        include_str!("fixtures/config-profiles-wide-python.json"),
    ] {
        let c: Value = serde_json::from_str(source).unwrap();
        assert_eq!(c["processNetworkAttempts"], 0);
        for case in c["cases"].as_array().unwrap() {
            let folder = f.path().parent().unwrap().to_owned();
            for entry in fs::read_dir(&folder).unwrap() {
                let p = entry.unwrap().path();
                if p.is_dir() {
                    fs::remove_dir_all(p).unwrap();
                } else {
                    fs::remove_file(p).unwrap();
                }
            }
            for (n, b) in case["initial"].as_object().unwrap() {
                let p = folder.join(n);
                fs::write(&p, b.as_str().unwrap()).unwrap();
                fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
            }
            let actual = f.action(case["params"].clone()).await;
            let mut observed = json!({});
            let mut directories = json!({});
            fn scan(
                root: &std::path::Path,
                at: &std::path::Path,
                files: &mut Value,
                dirs: &mut Value,
            ) {
                for entry in fs::read_dir(at).unwrap() {
                    let p = entry.unwrap().path();
                    let n = p.strip_prefix(root).unwrap().to_str().unwrap().to_owned();
                    let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
                    if p.is_dir() {
                        dirs[&n] = json!(mode);
                        scan(root, &p, files, dirs);
                    } else {
                        files[&n] = json!({"content":fs::read_to_string(&p).unwrap(),"mode":mode});
                    }
                }
            }
            scan(&folder, &folder, &mut observed, &mut directories);
            let now_pattern =
                regex::Regex::new(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6}\+00:00").unwrap();
            let bytes = observed["routing-profiles.json"]["content"]
                .as_str()
                .unwrap_or("");
            let now = now_pattern
                .find_iter(bytes)
                .map(|m| m.as_str())
                .find(|s| *s != c["now"].as_str().unwrap())
                .unwrap_or(c["now"].as_str().unwrap());
            let stamp = observed
                .as_object()
                .unwrap()
                .keys()
                .find_map(|n| {
                    n.strip_prefix(".migration-backups/routing-profiles.format-1.")
                        .map(|s| s.strip_suffix(".json").unwrap())
                })
                .unwrap_or("20270115T080000123456Z");
            assert!(regex::Regex::new(r"^\d{8}T\d{12}Z$")
                .unwrap()
                .is_match(stamp));
            let captured_stamp = case["files"]
                .as_object()
                .unwrap()
                .keys()
                .find_map(|n| {
                    n.strip_prefix(".migration-backups/routing-profiles.format-1.")
                        .and_then(|s| s.strip_suffix(".json"))
                })
                .unwrap_or("20270115T080000123456Z");
            // Connection updated_at changes its byte-derived revision. Rebind only
            // the hash of the exact capture bytes after the validated UTC substitution.
            let connection_hash = case["files"]["connections.json"]["content"]
                .as_str()
                .map(|b| {
                    let rebound = b.replace(c["now"].as_str().unwrap(), now);
                    (
                        format!("sha256:{:x}", Sha256::digest(b.as_bytes())),
                        format!("sha256:{:x}", Sha256::digest(rebound.as_bytes())),
                    )
                });
            let expand = |v: &Value| -> Value {
                let mut text = v
                    .to_string()
                    .replace("$CONFIG", folder.to_str().unwrap())
                    .replace(c["now"].as_str().unwrap(), now)
                    .replace(captured_stamp, stamp);
                if let Some((old, new)) = &connection_hash {
                    text = text.replace(old, new);
                }
                serde_json::from_str(&text).unwrap()
            };
            let files = expand(&case["files"]);
            assert_eq!(observed, files, "profile files {}", case["name"]);
            assert_eq!(
                directories, case["dirs"],
                "profile directories {}",
                case["name"]
            );
            let mut expected = expand(&case["reply"]);
            if expected["ok"] == true && expected["result"]["changed"] == true {
                let body = files["routing-profiles.json"]["content"].as_str().unwrap();
                expected["result"]["revision"] =
                    json!(format!("sha256:{:x}", Sha256::digest(body.as_bytes())));
                let document: Value = serde_json::from_str(body).unwrap_or_else(|_| {
                    // Wide capture numbers occur only in ignored top-level metadata.
                    // Its entries contain both timestamp members; no wire number is narrowed.
                    assert!(source == include_str!("fixtures/config-profiles-wide-python.json"));
                    let mut profiles = json!({});
                    for entry in expected["result"]["profiles"].as_array().unwrap() {
                        assert!(entry["created_at"].is_string() && entry["updated_at"].is_string());
                        profiles[entry["name"].as_str().unwrap()] = json!({
                            "description":entry["description"],"created_at":entry["created_at"],
                            "updated_at":entry["updated_at"],"sections":entry["sections"],"settings":entry["settings"]});
                    }
                    json!({"profiles":profiles})
                });
                for entry in expected["result"]["profiles"].as_array_mut().unwrap() {
                    let name = entry["name"].as_str().unwrap();
                    entry["revision"] = json!(format!(
                        "sha256:{:x}",
                        Sha256::digest(serde_json::to_vec(&document["profiles"][name]).unwrap())
                    ));
                }
            }
            assert_eq!(actual, expected, "profile reply {}", case["name"]);
            // Every case is native now: the retained provider is never called.
        }
    }
    // Wider ignored profile-document metadata now stays native.
    for number in [
        "123456789012345678901234567890",
        "NaN",
        "-Infinity",
        "1e999",
    ] {
        for verb in ["read", "create", "save", "update", "delete", "deactivate"] {
            let folder = f.path().parent().unwrap().to_owned();
            for entry in fs::read_dir(&folder).unwrap() {
                let p = entry.unwrap().path();
                if p.is_dir() {
                    fs::remove_dir_all(p).unwrap();
                } else {
                    fs::remove_file(p).unwrap();
                }
            }
            let profile =
                serde_json::from_str::<Value>(include_str!("fixtures/config-profiles-python.json"))
                    .unwrap();
            let original = profile["cases"][3]["initial"]["routing-profiles.json"]
                .as_str()
                .unwrap();
            let index = original.rfind('}').unwrap();
            let bytes = format!(
                "{},\n  \"metadata\": {number}\n}}\n",
                original[..index].trim_end()
            );
            let path = folder.join("routing-profiles.json");
            fs::write(&path, bytes.as_bytes()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let mut params = json!({"action":"profile","verb":verb,"_fixture_real":true});
            if verb != "read" {
                params["expected_revision"] =
                    json!(format!("sha256:{:x}", Sha256::digest(bytes.as_bytes())));
            }
            if matches!(verb, "create" | "save") {
                params["expected_revisions"] = json!({"routing":"absent","user_defaults":"absent","mcp_servers":"absent","connections":"absent"});
            }
            if matches!(verb, "create" | "save" | "update" | "delete") {
                params["name"] = json!(if verb == "create" { "new" } else { "a" });
            }
            let reply = f.action(params).await;
            assert_eq!(
                reply["ok"], true,
                "numeric compatibility {verb}/{number}: {reply}"
            );
            let after = fs::read_to_string(path).unwrap();
            assert!(after.contains(if number == "1e999" && verb != "read" {
                "Infinity"
            } else {
                number
            }));
        }
    }
    assert_eq!(fs::read_dir(f.root.join("home")).unwrap().count(), 0);
    apply_action_oracles(&f).await;
    connection_action_oracles(&f).await;
    assert!(!f.root.join("projects/.cortex").exists());
    catalog_refresh_oracles(&f).await;
    catalog_producer::check(&f).await;
    f.stop().await;
}
