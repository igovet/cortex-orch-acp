use super::*;
use std::{
    os::unix::fs::symlink,
    sync::atomic::{AtomicUsize, Ordering},
};
const AT: &str = "2026-11-14T00:00:00+00:00";
const ID: &str = "cn_000000000001";
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::path::PathBuf::from(format!(
            "/tmp/cw-ak-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn folder(&self) -> std::path::PathBuf {
        self.0.join("config/cortex")
    }
    fn call(
        &self,
        p: &Value,
        adapter: &Value,
        persist: impl FnMut(&Path, &[u8]) -> Result<()>,
    ) -> Result<Value> {
        validate(p)?;
        add_with(
            &self.folder(),
            p,
            adapter,
            || AT.into(),
            || Ok(ID.into()),
            persist,
            || Ok(()),
        )
    }
    fn params(&self) -> Value {
        json!({"engine":"codex-acp","name":" Fixture 🙂 ","key":"synthetic-fixture-only","revision":"absent"})
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn engine_connection_add_key_fixed_clock_complete_oracle_bytes() {
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
            .find(|c| c["name"] == format!("{engine}-add"))
            .unwrap();
        let files = &case["observations"][0]["files"];
        let mut expected: Value = serde_json::from_str(
            &files["config/cortex/connections.json"]["content"]
                .as_str()
                .unwrap()
                .replace("$ROOT", f.0.to_str().unwrap()),
        )
        .unwrap();
        let r = &mut expected["connections"][ID];
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
        let mut p = case["requests"][0].clone();
        p["key"] = json!("synthetic-fixture-only");
        let reply = f.call(&p, &adapter, write::replace).unwrap();
        assert_eq!(reply, case["observations"][0]["reply"]["result"]);
        assert_eq!(
            fs::read(f.folder().join("connections.json")).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        let secret = f.folder().join("secrets").join(ID);
        assert_eq!(fs::read(&secret).unwrap(), b"synthetic-fixture-only");
        for path in [f.folder(), secret.parent().unwrap().to_owned()] {
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o700);
        }
        assert_eq!(fs::metadata(secret).unwrap().mode() & 0o777, 0o600);
        assert!(!f.folder().join("secrets/.gitignore").exists());
        let mut p = p;
        p["revision"] = json!(write::token(Some(
            &fs::read(f.folder().join("connections.json")).unwrap()
        )));
        assert_eq!(
            f.call(&p, &adapter, write::replace).unwrap_err().code,
            "connection_failed"
        );
    }
}
#[test]
fn engine_connection_add_key_known_failures_match_complete_oracle_effects() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/settings-secrets-python.json"
    ))
    .unwrap();
    let original: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/engine-connection-python.json"
    ))
    .unwrap();
    for case in corpus["cases"].as_array().unwrap().iter().filter(|c| {
        c["variant"].get("fail_write").is_some() || c["variant"]["fail_validate"] == true
    }) {
        let f = Fixture::new();
        let engine = case["variant"]["engine"].as_str().unwrap();
        let installation = original["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == format!("{engine}-add"))
            .unwrap();
        let document: Value = serde_json::from_str(
            installation["observations"][0]["files"]["config/cortex/connections.json"]["content"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let adapter: Value = serde_json::from_str(
            &serde_json::to_string(&document["connections"][ID]["launch_command"])
                .unwrap()
                .replace("$ROOT", f.0.to_str().unwrap()),
        )
        .unwrap();
        let mut writes = vec![];
        let result = add_with(
            &f.folder(),
            &case["params"],
            &adapter,
            || AT.into(),
            || Ok(ID.into()),
            |path, bytes| {
                writes.push(
                    if path.parent().unwrap().file_name().unwrap() == "secrets" {
                        "secret"
                    } else {
                        "registry"
                    },
                );
                if Some(writes.len() as u64) == case["variant"]["fail_write"].as_u64() {
                    return Err(failed());
                }
                write::replace(path, bytes)
            },
            || {
                if case["variant"]["fail_validate"] == true {
                    Err(failed())
                } else {
                    Ok(())
                }
            },
        );
        let error = result.unwrap_err();
        assert_eq!(
            json!({"ok":false,"code":error.code,"message":error.message}),
            case["reply"]
        );
        assert_eq!(json!(writes), case["writes"]);
        fn walk(root: &Path, path: &Path, out: &mut serde_json::Map<String, Value>) {
            for entry in fs::read_dir(path).unwrap() {
                let p = entry.unwrap().path();
                let name = p.strip_prefix(root).unwrap().to_str().unwrap().to_owned();
                let m = fs::symlink_metadata(&p).unwrap();
                let value = if m.is_dir() {
                    json!({"directory":true,"mode":m.mode() & 0o777})
                } else {
                    json!({"content":fs::read_to_string(&p).unwrap().replace(root.to_str().unwrap(),"$ROOT"),"mode":m.mode() & 0o777})
                };
                out.insert(name, value);
                if m.is_dir() {
                    walk(root, &p, out);
                }
            }
        }
        let mut actual = serde_json::Map::new();
        walk(&f.0, &f.0.join("config"), &mut actual);
        let expected: serde_json::Map<String, Value> = case["after"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with("config/"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Config root itself is included by both complete snapshots.
        assert_eq!(
            Value::Object(actual),
            Value::Object(expected),
            "{}",
            case["name"]
        );
    }
}
#[test]
fn engine_connection_add_key_known_failures_remove_and_opt_out() {
    for fail_at in [1, 2, 3] {
        let f = Fixture::new();
        let mut calls = 0;
        let result = f.call(
            &f.params(),
            &json!(["/fixture/node", "/fixture/entry"]),
            |path, bytes| {
                calls += 1;
                if calls == fail_at {
                    return Err(failed());
                }
                write::replace(path, bytes)
            },
        );
        assert_eq!(result.unwrap_err().code, "connection_failed");
        let path = f.folder().join("connections.json");
        if fail_at == 1 {
            assert!(!path.exists());
        } else {
            let state: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert_eq!(state["connections"], json!({}));
            assert_eq!(state["auto_connect_disabled"], json!(["codex-acp"]));
            assert!(!f.folder().join("secrets").join(ID).exists());
        }
    }
}
#[test]
fn engine_connection_add_key_uncertainty_is_not_rolled_back_or_replayed() {
    for fail_at in [1, 2, 3] {
        let f = Fixture::new();
        let mut calls = 0;
        let result = f.call(
            &f.params(),
            &json!(["/fixture/node", "/fixture/entry"]),
            |path, bytes| {
                calls += 1;
                write::replace(path, bytes)?;
                if calls == fail_at {
                    Err(unknown())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result.unwrap_err().code, "machine_outcome_unknown");
        assert_eq!(calls, fail_at);
        let state: Value =
            serde_json::from_slice(&fs::read(f.folder().join("connections.json")).unwrap())
                .unwrap();
        assert!(state["connections"].get(ID).is_some());
        assert!(state.get("auto_connect_disabled").is_none());
    }
    let f = Fixture::new();
    let mut calls = 0;
    assert_eq!(
        f.call(
            &f.params(),
            &json!(["/fixture/node", "/fixture/entry"]),
            |path, bytes| {
                calls += 1;
                if calls >= 2 {
                    Err(failed())
                } else {
                    write::replace(path, bytes)
                }
            }
        )
        .unwrap_err()
        .code,
        "machine_outcome_unknown"
    );
}
#[test]
fn engine_connection_add_key_duplicate_preserves_record_and_secret() {
    let f = Fixture::new();
    let adapter = json!(["/fixture/node", "/fixture/entry"]);
    f.call(&f.params(), &adapter, write::replace).unwrap();
    let path = f.folder().join("connections.json");
    let before = fs::read(&path).unwrap();
    let mut params = f.params();
    params["revision"] = json!(write::token(Some(&before)));
    let result = add_with(
        &f.folder(),
        &params,
        &adapter,
        || AT.into(),
        || Ok("cn_000000000002".into()),
        |_, _| panic!("duplicate must not write"),
        || Ok(()),
    );
    assert_eq!(result.unwrap_err().code, "connection_failed");
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(
        fs::read(f.folder().join("secrets").join(ID)).unwrap(),
        b"synthetic-fixture-only"
    );
    assert!(!f.folder().join("secrets/cn_000000000002").exists());
}
#[test]
fn engine_connection_add_key_revision_and_unsafe_secret_fences() {
    let f = Fixture::new();
    let mut p = f.params();
    p["revision"] = json!("stale");
    assert_eq!(
        f.call(&p, &json!([]), |_, _| panic!("must not write"))
            .unwrap_err()
            .code,
        "revision_conflict"
    );
    assert!(!f.folder().join("secrets").exists());
    for shape in ["symlink", "hardlink", "public", "directory", "parent-link"] {
        let f = Fixture::new();
        registration::prepare(&f.folder()).unwrap();
        let parent = f.folder().join("secrets");
        if shape == "parent-link" {
            let target = f.0.join("outside");
            fs::create_dir(&target).unwrap();
            symlink(target, &parent).unwrap();
        } else {
            prepare_secrets(&f.folder()).unwrap();
            let path = parent.join(ID);
            match shape {
                "symlink" => symlink("/nonexistent", &path).unwrap(),
                "directory" => fs::create_dir(&path).unwrap(),
                _ => {
                    fs::write(&path, "synthetic prior value").unwrap();
                    fs::set_permissions(
                        &path,
                        fs::Permissions::from_mode(if shape == "public" { 0o644 } else { 0o600 }),
                    )
                    .unwrap();
                    if shape == "hardlink" {
                        fs::hard_link(&path, f.0.join("other")).unwrap();
                    }
                }
            }
        }
        assert_eq!(
            f.call(&f.params(), &json!(["/fixture"]), write::replace)
                .unwrap_err()
                .code,
            "connection_failed"
        );
        if shape == "hardlink" {
            assert_eq!(
                fs::read(f.0.join("other")).unwrap(),
                b"synthetic prior value"
            );
        }
    }
}
