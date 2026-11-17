use super::*;
use std::{
    os::unix::fs::symlink,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::path::PathBuf::from(format!(
            "/tmp/cw-er-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn call(
        &self,
        engine: &str,
        adapter: &Value,
        persist: impl FnOnce(&Path, &[u8]) -> Result<()>,
    ) -> Result<Value> {
        register_with(
            &self.0.join("config/cortex"),
            &self.0.join("home"),
            engine,
            adapter,
            "2026-11-14T00:00:00+00:00",
            || Ok("cn_000000000001".into()),
            persist,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn engine_connection_registration_matches_captured_bytes_and_reuses_unchanged() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/engine-connection-python.json"
    ))
    .unwrap();
    for engine in ["codex-acp", "claude-agent-acp"] {
        let f = Fixture::new();
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == format!("{engine}-recheck-ready"))
            .unwrap();
        let files = &case["observations"][0]["files"];
        let content = files["config/cortex/connections.json"]["content"]
            .as_str()
            .unwrap();
        let content = content.replace("$ROOT", f.0.to_str().unwrap());
        let mut expected: Value = serde_json::from_str(&content).unwrap();
        let r = &mut expected["connections"]["cn_000000000001"];
        // The captured fingerprint is bound to normalized root/architecture argv.
        let adapter = r["launch_command"].clone();
        let args: Vec<_> = adapter
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        r["approved_fingerprint"] = json!(crate::config_read_model::route_fingerprint(
            engine,
            if engine == "codex-acp" {
                "openai"
            } else {
                "anthropic"
            },
            None,
            Some(&args)
        ));
        let native = f.0.join(if engine == "codex-acp" {
            "home/.codex"
        } else {
            "home/.claude"
        });
        fs::create_dir_all(&native).unwrap();
        fs::set_permissions(&native, fs::Permissions::from_mode(0o700)).unwrap();
        let marker = native.join(if engine == "codex-acp" {
            "auth.json"
        } else {
            ".credentials.json"
        });
        fs::write(&marker, "offline marker, never read\n").unwrap();
        let reply = f.call(engine, &adapter, write::replace).unwrap();
        assert_eq!(reply, case["observations"][0]["reply"]["result"]);
        let path = f.0.join("config/cortex/connections.json");
        let actual = fs::read(&path).unwrap();
        // Assert complete JSON bytes, not merely parsed semantic equality.
        assert_eq!(actual, serde_json::to_vec(&expected).unwrap());
        for leaf in ["connections.json", "connections.lock", ".gitignore"] {
            assert_eq!(
                fs::metadata(path.with_file_name(leaf)).unwrap().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        // Existing references are reused even after marker disappearance, without validation.
        fs::remove_file(marker).unwrap();
        let reused = register_with(
            &f.0.join("config/cortex"),
            &f.0.join("home"),
            engine,
            &adapter,
            "later",
            || Ok("cn_000000000002".into()),
            |_, _| panic!("reuse must not write"),
        )
        .unwrap();
        assert_eq!(reused, reply);
        assert_eq!(fs::read(&path).unwrap(), actual);
        // Python reuses the first raw record, not the lexicographically first ID.
        let mut other = expected["connections"]["cn_000000000001"].clone();
        other["id"] = json!("cn_000000000002");
        let raw = format!("{{\"schema_version\":1,\"connections\":{{\"cn_000000000002\":{},\"cn_000000000001\":{}}},\"override_history\":{{}}}}",
            serde_json::to_string(&other).unwrap(),
            serde_json::to_string(&expected["connections"]["cn_000000000001"]).unwrap());
        fs::write(&path, &raw).unwrap();
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
        let reused = register_with(
            &f.0.join("config/cortex"),
            &f.0.join("home"),
            engine,
            &adapter,
            "later",
            || Ok("cn_000000000003".into()),
            |_, _| panic!("reuse must preserve original bytes"),
        )
        .unwrap();
        assert_eq!(reused["connectionId"], "cn_000000000002");
        assert_eq!(fs::read_to_string(path).unwrap(), raw);
        assert_eq!(
            fs::metadata(f.0.join("config/cortex")).unwrap().mode() & 0o777,
            0o700
        );
    }
}

#[test]
fn engine_connection_registration_refuses_unsafe_storage_and_retains_unknown_effect() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("config")).unwrap();
    fs::create_dir(f.0.join("target")).unwrap();
    symlink(f.0.join("target"), f.0.join("config/cortex")).unwrap();
    assert!(f
        .call("codex-acp", &json!(["/node", "/adapter"]), write::replace)
        .is_err());
    assert_eq!(fs::read_dir(f.0.join("target")).unwrap().count(), 0);
    let f = Fixture::new();
    let error = f
        .call("codex-acp", &json!(["/node", "/adapter"]), |path, bytes| {
            write::replace(path, bytes)?;
            Err(crate::machine::native::refuse(
                "machine_outcome_unknown",
                "Injected post-commit uncertainty",
            ))
        })
        .unwrap_err();
    assert_eq!(error.code, "machine_outcome_unknown");
    assert!(f.0.join("config/cortex/connections.json").is_file());
    assert!(!f.0.join("home/.codex").exists());
    assert_eq!(fs::read_dir(f.0.join("config/cortex")).unwrap().count(), 3);
}

#[test]
fn engine_connection_registration_stale_revision_and_id_exhaustion_do_not_write() {
    let f = Fixture::new();
    let folder = f.0.join("config/cortex");
    let adapter = json!(["/node", "/adapter"]);
    f.call("codex-acp", &adapter, write::replace).unwrap();
    let before = fs::read(folder.join("connections.json")).unwrap();
    let mut calls = 0;
    let error = register_with(
        &folder,
        &f.0.join("home"),
        "codex-acp",
        &adapter,
        "later",
        || {
            calls += 1;
            Ok("cn_000000000001".into())
        },
        |_, _| panic!("must not write"),
    )
    .unwrap_err();
    assert_eq!(calls, 32);
    assert_eq!(error.code, "engine_unavailable");
    assert_eq!(fs::read(folder.join("connections.json")).unwrap(), before);

    // Simulate a noncooperating replacement after the locked read but before commit.
    let error = register_with(
        &folder,
        &f.0.join("home"),
        "claude-agent-acp",
        &adapter,
        "later",
        || {
            fs::write(folder.join("connections.json"), b"changed").unwrap();
            Ok("cn_000000000002".into())
        },
        |_, _| panic!("must not overwrite changed bytes"),
    )
    .unwrap_err();
    assert_eq!(error.code, "revision_conflict");
    assert_eq!(
        fs::read(folder.join("connections.json")).unwrap(),
        b"changed"
    );
}
