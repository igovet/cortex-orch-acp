use super::resolution_tests::{git, hex, rows, setup};
use super::*;
use std::os::unix::fs::PermissionsExt;

fn oracle() -> Value {
    serde_json::from_str(include_str!("../fixtures/task-merge-python.json")).unwrap()
}
fn observation(f: &Fixture, fixture: &Value, case: &Value) -> Value {
    let path = f.replace(&case["taskPath"]);
    let path = Path::new(path.as_str().unwrap());
    let run = |argv: &[&str]| {
        let out = git(f, fixture, path, argv, None);
        json!({"code":out.status.code(),"stdout":hex(&out.stdout)})
    };
    json!({"index":run(&["ls-files","--stage","-z"]),
        "status":run(&["status","--porcelain","-z","--untracked-files=all"]),
        "file":hex(&fs::read(path.join("file.txt")).unwrap()),
        "mergeHead":run(&["rev-parse","--verify","MERGE_HEAD"]),
        "commit":run(&["show","-s","--format=%T%n%P%n%B","HEAD"])})
}
#[tokio::test]
async fn merge_finish_and_abort_match_real_git() {
    let fixture = oracle();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 38);
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
        let before = store.as_ref().map(authority);
        let params = f.replace(&case["params"]);
        let start = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let reply = f.call(params.clone()).await;
        let end = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        if case["variant"].as_str().unwrap().starts_with("hook-") {
            // Legacy stderr is not a public native diagnostic; attempted Git effects stay unknown.
            assert_eq!(case["expected"]["code"], "git_failed");
            assert_eq!(reply["code"], "integration_unknown");
            assert!(!reply.to_string().contains("PRIVATE_HOOK_DIAGNOSTIC"));
        } else if case["action"] == "abort_merge"
            && matches!(
                case["variant"].as_str(),
                Some("no-merge-head" | "already-finished")
            )
        {
            assert_eq!(case["expected"]["code"], "git_failed");
            assert_eq!(reply["code"], "integration_unknown");
        } else if case["variant"] == "plain" {
            assert_eq!(case["expected"]["code"], "git_failed");
            assert_eq!(reply["code"], "git_failed");
            assert!(!reply["message"].as_str().unwrap().contains("fatal:"));
        } else {
            assert_eq!(reply, case["expected"], "{} reply", case["name"]);
        }
        let mut saved: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
        if reply["ok"] == true && case["action"] == "finish_merge" {
            let at = saved["tasks"]["t1"]["merge"]["finishedAt"]
                .as_u64()
                .unwrap();
            assert!((start..=end).contains(&at));
            saved["tasks"]["t1"]["merge"]["finishedAt"] = fixture["clock"].clone();
        }
        if !saved["ghAvailable"].is_null() {
            assert!(saved["ghAvailable"].is_boolean());
            saved["ghAvailable"] = json!("$GH_AVAILABLE");
        }
        assert_eq!(saved, f.replace(&case["after"]), "{} overlay", case["name"]);
        assert_eq!(
            observation(&f, &fixture, case),
            case["gitAfter"],
            "{} Git",
            case["name"]
        );
        assert_eq!(store.as_ref().map(authority), before);
        if let Some(store) = &store {
            assert_eq!(
                rows(store),
                f.replace(&case["observed"]),
                "{} SQLite",
                case["name"]
            );
        } else {
            assert!(!f.root.join(".cortex/cortex.sqlite3").exists());
            assert!(!f.root.join(".cortex-dev/cortex.sqlite3").exists());
        }
        let after = observation(&f, &fixture, case);
        let task_path = f.replace(&case["taskPath"]);
        let head = git(
            &f,
            &fixture,
            Path::new(task_path.as_str().unwrap()),
            &["rev-parse", "HEAD"],
            None,
        )
        .stdout;
        let retry = f.call(params).await;
        if reply["code"] == "integration_unknown" {
            assert_eq!(retry["code"], "outcome_unknown");
        } else {
            assert_eq!(retry, reply, "{} keyed replay", case["name"]);
        }
        assert_eq!(observation(&f, &fixture, case), after);
        assert_eq!(
            git(
                &f,
                &fixture,
                Path::new(task_path.as_str().unwrap()),
                &["rev-parse", "HEAD"],
                None
            )
            .stdout,
            head
        );
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn merge_preflight_refuses_store_exclusion_and_invalid_identity() {
    let fixture = oracle();
    for action in ["finish_merge", "abort_merge"] {
        for fault in ["store_lock", "invalid_request", "legacy"] {
            let case = fixture["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["action"] == action && c["variant"] == "resolved")
                .unwrap();
            let f = Fixture::new().await;
            let store = setup(&f, &fixture, case).unwrap();
            let saved = fs::read(f.path()).unwrap();
            let before = observation(&f, &fixture, case);
            let mut params = f.replace(&case["params"]);
            if fault == "invalid_request" {
                params["requestId"] = json!("");
            }
            if fault == "legacy" {
                store
                    .write(|db| {
                        db.execute(
                            "INSERT INTO workspace_task_options VALUES ('t1',?)",
                            [json!({"checkout":{"kind":"worktree"}}).to_string()],
                        )?;
                        Ok(())
                    })
                    .unwrap();
            }
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
            if fault == "legacy" {
                // The unmaterialized legacy checkout has no worktree to observe: the
                // native owner refuses it explicitly before any effect, never delegating.
                let reply = f.call(params).await;
                assert_eq!(reply["ok"], false);
                assert_eq!(reply["code"], "unknown_checkout");
            } else {
                let reply = f.call(params).await;
                assert_eq!(
                    reply["code"],
                    if fault == "store_lock" {
                        "busy"
                    } else {
                        "invalid_request"
                    }
                );
            }
            drop(guard);
            assert_eq!(fs::read(f.path()).unwrap(), saved);
            assert_eq!(observation(&f, &fixture, case), before);
            f.service.stop().await.unwrap();
        }
    }
}
#[tokio::test]
async fn merge_git_and_save_failures_stay_unknown_before_projection_fallback() {
    assert_ne!(unsafe { libc::geteuid() }, 0);
    let fixture = oracle();
    for action in ["finish_merge", "abort_merge"] {
        for fault in ["index_lock", "overlay_save"] {
            let case = fixture["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["action"] == action && c["variant"] == "resolved")
                .unwrap();
            let f = Fixture::new().await;
            let store = setup(&f, &fixture, case).unwrap();
            let saved = fs::read(f.path()).unwrap();
            let before = authority(&store);
            let parent = f.path().parent().unwrap().to_owned();
            let before_git = observation(&f, &fixture, case);
            let index_lock = f.root.join(".git/index.lock");
            if fault == "overlay_save" {
                fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).unwrap();
            } else {
                fs::write(&index_lock, b"owned test lock").unwrap();
            }
            let params = f.replace(&case["params"]);
            let reply = f.call(params.clone()).await;
            if fault == "overlay_save" {
                fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
            } else {
                fs::remove_file(index_lock).unwrap();
            }
            assert_eq!(reply["code"], "integration_unknown");
            assert_eq!(fs::read(f.path()).unwrap(), saved);
            assert_eq!(authority(&store), before);
            if fault == "overlay_save" {
                assert_eq!(observation(&f, &fixture, case), case["gitAfter"]);
            } else {
                assert_eq!(observation(&f, &fixture, case), before_git);
            }
            let git_after = observation(&f, &fixture, case);
            store
                .write(|db| {
                    db.execute(
                        "INSERT INTO workspace_task_options VALUES ('t1',?)",
                        [json!({"checkout":{"kind":"worktree"}}).to_string()],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert_eq!(f.call(params.clone()).await["code"], "outcome_unknown");
            let mut changed = params;
            changed["extra"] = json!(true);
            assert_eq!(f.call(changed).await["code"], "payload_conflict");
            assert_eq!(observation(&f, &fixture, case), git_after);
            f.service.stop().await.unwrap();
        }
    }
}
