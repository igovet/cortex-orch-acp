use super::*;
use std::os::unix::fs::PermissionsExt;

fn oracle() -> Value {
    serde_json::from_str(include_str!("../fixtures/task-resolution-python.json")).unwrap()
}
pub(super) fn git(
    f: &Fixture,
    fixture: &Value,
    cwd: &Path,
    argv: &[&str],
    input: Option<&str>,
) -> std::process::Output {
    use std::io::Write;
    let mut cmd = std::process::Command::new("git");
    cmd.args(argv).current_dir(cwd);
    for (key, value) in fixture["gitEnv"].as_object().unwrap() {
        cmd.env(key, value.as_str().unwrap());
    }
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("HOME", &f.base)
        .env("XDG_CONFIG_HOME", f.base.join("config"));
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if let Some(text) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    child.wait_with_output().unwrap()
}
fn sql(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
        _ => panic!("invalid SQL fixture"),
    }
}
pub(super) fn setup(f: &Fixture, fixture: &Value, case: &Value) -> Option<cortex_core::Store> {
    for step in case["steps"].as_array().unwrap() {
        let step = f.replace(step);
        if let Some(file) = step["file"].as_str() {
            fs::write(file, step["text"].as_str().unwrap()).unwrap();
            if step["executable"] == true {
                fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
            }
        } else if let Some(link) = step["symlink"].as_str() {
            fs::remove_file(link).unwrap();
            std::os::unix::fs::symlink(step["target"].as_str().unwrap(), link).unwrap();
        } else if let Some(from) = step["renameFrom"].as_str() {
            fs::rename(from, step["renameTo"].as_str().unwrap()).unwrap();
        } else {
            let args: Vec<_> = step["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap())
                .collect();
            let out = git(
                f,
                fixture,
                Path::new(step["cwd"].as_str().unwrap()),
                &args,
                step["input"].as_str(),
            );
            assert_eq!(
                out.status.code(),
                step["code"].as_i64().map(|n| n as i32),
                "{} setup {:?}",
                case["name"],
                args
            );
        }
    }
    let store = if case["store"] == true {
        let store = f.store();
        store
            .write(|db| {
                for row in case["rows"].as_array().unwrap() {
                    let row = f.replace(row);
                    db.execute(
                        row[0].as_str().unwrap(),
                        rusqlite::params_from_iter(row[1].as_array().unwrap().iter().map(sql)),
                    )?;
                }
                Ok(())
            })
            .unwrap();
        Some(store)
    } else {
        None
    };
    fs::create_dir_all(f.path().parent().unwrap()).unwrap();
    fs::set_permissions(
        f.path().parent().unwrap(),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::write(f.path(), f.replace(&case["saved"]).to_string()).unwrap();
    store
}
fn observation(f: &Fixture, fixture: &Value, case: &Value) -> Value {
    let path = f.replace(&case["taskPath"]);
    let path = Path::new(path.as_str().unwrap());
    let run = |args: &[&str]| {
        let out = git(f, fixture, path, args, None);
        json!({"code":out.status.code(),"stdout":hex(&out.stdout)})
    };
    let file = path.join(case["file"].as_str().unwrap());
    json!({"index":run(&["ls-files","--stage","-z"]),
        "status":run(&["status","--porcelain","-z","--untracked-files=all"]),
        "head":run(&["rev-parse","HEAD"]),
        "file":hex(&fs::read(&file).unwrap()),
        "mode":fs::symlink_metadata(&file).unwrap().permissions().mode() & 0o777,
        "symlink":if file.is_symlink(){Some(fs::read_link(&file).unwrap().to_str().unwrap().to_owned())}else{None}})
}
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub(super) fn rows(store: &cortex_core::Store) -> Value {
    store.read(|db|{
        let mut q=db.prepare("SELECT task_id,description,version,execution_id FROM workspace_tasks ORDER BY task_id")?;
        let tasks=q.query_map([],|r|Ok(json!([r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<String>>(3)?])))?.collect::<Result<Vec<_>,_>>()?;
        let mut q=db.prepare("SELECT task_id,options_json FROM workspace_task_options ORDER BY task_id")?;
        let options=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
            .map(|r|{let(t,s)=r?;Ok(json!([t,serde_json::from_str::<Value>(&s)?]))}).collect::<cortex_core::CoreResult<Vec<_>>>()?;
        let mut q=db.prepare("SELECT project_id,version FROM workspace_projects ORDER BY project_id")?;
        let projects=q.query_map([],|r|Ok(json!([r.get::<_,String>(0)?,r.get::<_,i64>(1)?])))?.collect::<Result<Vec<_>,_>>()?;
        Ok(json!({"tasks":tasks,"options":options,"projects":projects}))
    }).unwrap()
}
#[tokio::test]
async fn file_resolution_matches_real_git() {
    let fixture = oracle();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 36);
    assert_eq!(
        cases
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        cases.len()
    );
    for case in cases {
        let f = Fixture::new().await;
        let store = setup(&f, &fixture, case);
        let authority_before = store.as_ref().map(authority);
        let reply = f.call(f.replace(&case["params"])).await;
        if case["name"] == "plain" {
            // A read-only Git refusal names Git's own first error line, redacted and bounded.
            assert_eq!(reply["ok"], false);
            assert_eq!(reply["code"], "git_failed");
            let message = reply["message"].as_str().unwrap();
            assert!(message.starts_with("Git refused this operation. Inspect the checkout in Git; no automatic retry was made."), "{message}");
            assert!(message.contains("(git ls-files: fatal: not a git repository"), "{message}");
            assert_eq!(case["expected"]["code"], "git_failed");
        } else {
            assert_eq!(reply, case["expected"], "{} reply", case["name"]);
        }
        let mut saved: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
        if !saved["ghAvailable"].is_null() {
            assert!(saved["ghAvailable"].is_boolean());
            saved["ghAvailable"] = json!("$GH_AVAILABLE");
        }
        assert_eq!(saved, f.replace(&case["after"]), "{} overlay", case["name"]);
        assert_eq!(
            observation(&f, &fixture, case),
            case["gitAfter"],
            "{} Git/file",
            case["name"]
        );
        assert_eq!(store.as_ref().map(authority), authority_before);
        if let Some(store) = &store {
            assert_eq!(
                rows(store),
                f.replace(&case["observed"]),
                "{} SQLite",
                case["name"]
            );
        } else {
            assert!(
                !f.root.join(".cortex/cortex.sqlite3").exists()
                    && !f.root.join(".cortex-dev/cortex.sqlite3").exists()
            );
        }
        if reply["ok"] == true {
            let after = observation(&f, &fixture, case);
            assert_eq!(
                f.call(f.replace(&case["params"])).await,
                reply,
                "successful keyed replay"
            );
            assert_eq!(observation(&f, &fixture, case), after);
        }
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn resolution_unknown_after_staging_or_overlay_failure_cannot_replay() {
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "requires real non-root permission failures"
    );
    let fixture = oracle();
    let case = &fixture["cases"][0];
    for fault in ["index_lock", "overlay_save"] {
        let f = Fixture::new().await;
        let store = setup(&f, &fixture, case).unwrap();
        let authoritative = authority(&store);
        let saved = fs::read(f.path()).unwrap();
        let parent = f.path().parent().unwrap().to_owned();
        let index_lock = f.root.join(".git/index.lock");
        if fault == "index_lock" {
            fs::write(&index_lock, b"owned test lock").unwrap();
        } else {
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).unwrap();
        }
        let reply = f.call(f.replace(&case["params"])).await;
        assert_eq!(reply["code"], "integration_unknown", "{fault}");
        if fault == "index_lock" {
            fs::remove_file(index_lock).unwrap();
        } else {
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(fs::read(f.path()).unwrap(), saved);
        assert_eq!(fs::read(f.root.join("file.txt")).unwrap(), b"ours\n");
        let after = observation(&f, &fixture, case);
        let retry = f.call(f.replace(&case["params"])).await;
        assert_eq!(retry["code"], "outcome_unknown");
        assert_eq!(observation(&f, &fixture, case), after);
        assert_eq!(authority(&store), authoritative);
        // Even a later unsupported projection cannot route this unknown key into Python.
        store
            .write(|db| {
                db.execute(
                    "INSERT INTO workspace_task_options VALUES ('t1',?)",
                    [json!({"checkout":{"kind":"worktree"}}).to_string()],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            f.call(f.replace(&case["params"])).await["code"],
            "outcome_unknown"
        );
        let mut changed = f.replace(&case["params"]);
        changed["choice"] = json!("theirs");
        assert_eq!(f.call(changed).await["code"], "payload_conflict");
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn resolution_legacy_fallback_store_exclusion_and_private_paths_are_pre_effect() {
    let fixture = oracle();
    let case = &fixture["cases"][0];
    for fault in [
        "legacy",
        "store_lock",
        "private_path",
        "parent_link",
        "escape_link",
        "invalid_request",
    ] {
        let f = Fixture::new().await;
        let store = setup(&f, &fixture, case).unwrap();
        let mut saved = f.replace(&case["saved"]);
        let mut params = f.replace(&case["params"]);
        if fault == "legacy" {
            saved["tasks"]["t1"]["merge"]["files"][0]["blocks"][0]["results"]["ours"]["lines"] =
                json!(12);
        }
        if fault == "private_path" {
            params["path"] = json!(".cortex/private");
            saved["tasks"]["t1"]["merge"]["files"][0]["path"] = params["path"].clone();
        }
        if fault == "parent_link" || fault == "escape_link" {
            let target = if fault == "escape_link" {
                let target = f.base.join("outside");
                fs::create_dir(&target).unwrap();
                fs::write(target.join("file.txt"), "never overwrite outside").unwrap();
                target
            } else {
                f.root.clone()
            };
            std::os::unix::fs::symlink(&target, f.root.join("link")).unwrap();
            params["path"] = json!("link/file.txt");
            saved["tasks"]["t1"]["merge"]["files"][0]["path"] = params["path"].clone();
            // Seed real alias index stages so the no-follow path check, not a
            // missing-stage shortcut, refuses the file.
            let stages = git(&f, &fixture, &f.root, &["ls-files", "-u"], None);
            assert!(stages.status.success());
            let stages = String::from_utf8(stages.stdout)
                .unwrap()
                .replace("\tfile.txt", "\tlink/file.txt");
            assert!(git(
                &f,
                &fixture,
                &f.root,
                &["update-index", "--index-info"],
                Some(&stages)
            )
            .status
            .success());
        }
        if fault == "invalid_request" {
            params["requestId"] = json!("");
        }
        let before = observation(&f, &fixture, case);
        fs::write(f.path(), saved.to_string()).unwrap();
        let guard = if fault == "store_lock" {
            Some(
                cortex_core::store::locks::AccessLock::acquire(
                    &store.key().directory().join(".access.lock"),
                    false,
                    true,
                    Duration::ZERO,
                )
                .unwrap(),
            )
        } else {
            None
        };
        // A saved conflict of a shape the app never writes is an explicit native
        // refusal, never a delegation.
        let reply = f.call(params).await;
        assert_eq!(reply["ok"], false, "{fault}");
        assert_eq!(
            reply["code"],
            if fault == "store_lock" {
                "busy"
            } else if fault == "private_path" || fault == "escape_link" {
                "invalid_path"
            } else if fault == "invalid_request" || fault == "legacy" {
                "invalid_request"
            } else {
                "unsafe_conflict"
            }
        );
        drop(guard);
        assert_eq!(fs::read(f.path()).unwrap(), saved.to_string().as_bytes());
        assert_eq!(observation(&f, &fixture, case), before);
        if fault == "escape_link" {
            assert_eq!(
                fs::read(f.base.join("outside/file.txt")).unwrap(),
                b"never overwrite outside"
            );
        }
        assert!(
            !f.state.join("operations").exists()
                || fault == "parent_link"
                || fault == "escape_link"
        );
        f.service.stop().await.unwrap();
    }
}
