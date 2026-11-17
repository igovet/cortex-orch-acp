use super::*;
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    root: PathBuf,
    expected: Value,
    engine: String,
}
impl Fixture {
    fn new(engine: &str, bundled: bool) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = PathBuf::from(format!(
            "/tmp/cw-el-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let corpus: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/engine-connection-python.json"
        ))
        .unwrap();
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == format!("{engine}-login-ready"))
            .unwrap();
        let owned = format!("home/.local/share/cortex/engines/{engine}/1.0.0");
        let convert = |s: &str| {
            let s = if bundled {
                s.replace(&owned, "bundle")
                    .replace("bin/node", "bundle/node/bin/node")
            } else {
                s.to_owned()
            };
            let (arch, triple) = match std::env::consts::ARCH {
                "x86_64" => ("x64", "x86_64-unknown-linux-musl"),
                "aarch64" => ("arm64", "aarch64-unknown-linux-musl"),
                _ => panic!("unsupported test architecture"),
            };
            s.replace("$ROOT", root.to_str().unwrap())
                .replace("$ARCH", arch)
                .replace("$TRIPLE", triple)
        };
        for (path, file) in case["files"].as_object().unwrap() {
            let path = root.join(convert(path));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, file["content"].as_str().unwrap()).unwrap();
            fs::set_permissions(
                path,
                fs::Permissions::from_mode(file["mode"].as_u64().unwrap() as u32),
            )
            .unwrap();
        }
        let expected = serde_json::from_str(&convert(
            &serde_json::to_string(&case["observations"][0]["reply"]["result"]).unwrap(),
        ))
        .unwrap();
        Self {
            root,
            expected,
            engine: engine.to_owned(),
        }
    }
    fn lookup(&self, bundle: bool) -> Result<Value> {
        login(
            &self.engine,
            &self.root.join("home"),
            &self.root.join("home/.local/share"),
            bundle.then(|| self.root.join("bundle")).as_deref(),
            Some(self.root.join("bin").into_os_string()),
        )
    }
    fn package(&self, bundle: bool) -> PathBuf {
        let base = if bundle {
            self.root.join("bundle")
        } else {
            self.root.join(format!(
                "home/.local/share/cortex/engines/{}/1.0.0",
                self.engine
            ))
        };
        base.join("node_modules/@agentclientprotocol")
            .join(&self.engine)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn engine_connection_bundle_lookup_and_owned_fallback() {
    for engine in ["codex-acp", "claude-agent-acp"] {
        let f = Fixture::new(engine, true);
        assert_eq!(f.lookup(true).unwrap(), f.expected);
        // No PATH node exists. A bundle with an unqualified adapter is never used.
        let manifest = f.package(true).join("package.json");
        let mut doc = json_file(&manifest).unwrap();
        doc["version"] = json!("999.0.0");
        fs::write(manifest, serde_json::to_vec(&doc).unwrap()).unwrap();
        assert_eq!(f.lookup(true).unwrap_err().code, "engine_unavailable");

        let f = Fixture::new(engine, false);
        // An incomplete bundle must not mask a working owned installation.
        fs::create_dir_all(f.root.join("bundle/node/bin")).unwrap();
        fs::copy(f.root.join("bin/node"), f.root.join("bundle/node/bin/node")).unwrap();
        assert_eq!(f.lookup(true).unwrap(), f.expected);
    }
}
#[test]
fn engine_connection_qualified_selection_bin_and_canonical_node() {
    for engine in ["codex-acp", "claude-agent-acp"] {
        let f = Fixture::new(engine, false);
        let package = f.package(false);
        // A newer unqualified declared version loses, regardless of the folder label.
        let other = package.to_str().unwrap().replace("/1.0.0/", "/999.0.0/");
        fs::create_dir_all(Path::new(&other).join("dist")).unwrap();
        fs::write(
            Path::new(&other).join("package.json"),
            br#"{"version":"999.0.0","bin":"dist/index.js"}"#,
        )
        .unwrap();
        fs::write(Path::new(&other).join("dist/index.js"), "// never executed").unwrap();
        let manifest = package.join("package.json");
        let mut doc = json_file(&manifest).unwrap();
        doc["bin"] = json!("dist/index.js");
        fs::write(&manifest, serde_json::to_vec(&doc).unwrap()).unwrap();
        assert_eq!(f.lookup(false).unwrap(), f.expected);
        let nested = package.join("dist/package.json");
        fs::write(&nested, br#"{"bin":{"wrong":"missing.js"}}"#).unwrap();
        assert_eq!(f.lookup(false).unwrap_err().code, "engine_unavailable");
        fs::write(
            &nested,
            br#"{"version":"1.0.0","bin":{"wrong":"missing.js"}}"#,
        )
        .unwrap();
        assert_eq!(f.lookup(false).unwrap_err().code, "engine_unavailable");
        fs::remove_file(nested).unwrap();
        let original = f.root.join("bin/node");
        let real = f.root.join("bin/node-real");
        fs::rename(&original, &real).unwrap();
        symlink(&real, original).unwrap();
        let mut expected = f.expected.clone();
        expected["adapter"][0] = json!(real);
        assert_eq!(f.lookup(false).unwrap(), expected);
        // Escaping bin targets are not candidates, even if the file exists.
        doc["bin"] = json!(&real);
        fs::write(&manifest, serde_json::to_vec(&doc).unwrap()).unwrap();
        assert_eq!(f.lookup(false).unwrap_err().code, "engine_unavailable");
    }
}
#[test]
fn engine_connection_trust_and_bounded_inputs_never_become_commands() {
    let f = Fixture::new("codex-acp", false);
    let untrusted = f.root.join("home/.config/app.codeg/bin");
    fs::create_dir_all(&untrusted).unwrap();
    fs::copy(f.root.join("bin/node"), untrusted.join("node")).unwrap();
    let error = login(
        "codex-acp",
        &f.root.join("home"),
        &f.root.join("home/.local/share"),
        None,
        Some(untrusted.into_os_string()),
    )
    .unwrap_err();
    assert_eq!(error.code, "engine_unavailable");
    let binary = PathBuf::from(f.expected["program"].as_str().unwrap());
    fs::OpenOptions::new()
        .write(true)
        .open(&binary)
        .unwrap()
        .set_len(512 * 1024 * 1024 + 1)
        .unwrap();
    assert_eq!(f.lookup(false).unwrap_err().code, "engine_unavailable");
    // Every shape is native; unknown actions and truthy non-object params are
    // refused before any lookup (python oracle: machine-refusal-python.json).
    let config = crate::Config {
        runtime_dir: f.root.join("run"),
        state_dir: f.root.join("workspace"),
        allowed_root: f.root.clone(),
        launch: crate::LaunchSpec::bare(),
        request_timeout: std::time::Duration::from_secs(5),
        shutdown_timeout: std::time::Duration::from_millis(100),
        event_capacity: 16,
    };
    for params in [
        json!({"action":"unknown"}),
        Value::Null,
        json!([]),
        json!({}),
    ] {
        assert!(supported(&params));
        let error = action(&config, &params).unwrap_err();
        assert_eq!(
            (error.code, error.message.as_str()),
            ("unsupported_action", "Unsupported connection action.")
        );
    }
    for params in [json!("recheck"), json!([1]), json!(3), json!(true)] {
        let error = action(&config, &params).unwrap_err();
        assert_eq!(error.code, "machine_tool_failed");
        assert_eq!(
            error.message,
            super::super::native::UNCAUGHT_ATTRIBUTE_ERROR
        );
    }
    assert!(!config.state_dir.exists());
    assert!(supported(&json!({"action":"add_key"})));
    assert!(supported(&json!({"action":"recheck"})));
    assert!(supported(
        &json!({"action":"login_command","program":"/untrusted","args":["run"]})
    ));
}

#[tokio::test]
async fn engine_connection_status_is_silent_bounded_and_cancelled() {
    use std::time::Duration;
    let f = Fixture::new("codex-acp", false);
    let program = f.root.join("status");
    fs::write(
        &program,
        "#!/bin/sh\nprintf 'must never be returned'\nprintf 'must never be returned' >&2\nexit 2\n",
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let cmd = json!({"program":program});
    let environment = |c: &mut tokio::process::Command| {
        c.env_clear();
        Ok(())
    };
    let error = status(
        &cmd,
        "codex-acp",
        &f.root,
        Duration::from_secs(1),
        environment,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "native_login_required");
    assert!(!error.message.contains("must never"));
    fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
    status(
        &cmd,
        "claude-agent-acp",
        &f.root,
        Duration::from_secs(1),
        environment,
    )
    .await
    .unwrap();
    fs::write(
        &program,
        format!(
            "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nexec /bin/sleep 30\n",
            f.root.join("pid").display()
        ),
    )
    .unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        status(
            &cmd,
            "codex-acp",
            &f.root,
            Duration::from_millis(50),
            environment
        )
        .await
        .unwrap_err()
        .code,
        "engine_unavailable"
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    let pid: i32 = fs::read_to_string(f.root.join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    fs::remove_file(f.root.join("pid")).unwrap();

    let home = f.root.clone();
    let task = tokio::spawn(async move {
        status(
            &cmd,
            "codex-acp",
            &home,
            Duration::from_secs(10),
            environment,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !f.root.join("pid").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let pid: i32 = fs::read_to_string(f.root.join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), async {
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!f.root.join("config").exists());
}
