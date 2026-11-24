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
/// The historical fixtures omitted merge identities. Pin the two known fixture
/// parents so they still exercise their original Git behavior on today's surface.
fn bind_fixture_merge(f: &Fixture, fixture: &Value, case: &Value) -> Option<Value> {
    if !matches!(case["variant"].as_str(), Some(
        "resolved" | "unresolved" | "markers" | "whitespace" | "no-merge-head" |
        "worktree" | "no-store" | "extra-staged" |
        "hook-failure" | "hook-changed-head"
    )) {
        return None;
    }
    let path = f.replace(&case["taskPath"]);
    let path = Path::new(path.as_str().unwrap());
    let read = |args: &[&str]| {
        let out = git(f, fixture, path, args, None);
        assert!(out.status.success(), "{args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    let completed = matches!(case["variant"].as_str(), Some("no-merge-head" | "already-finished"));
    let source = read(&["rev-parse", if completed { "HEAD^1" } else { "HEAD" }]);
    let target = read(&["rev-parse", "other"]);
    assert_eq!(target, read(&["rev-parse", if completed { "HEAD^2" } else { "MERGE_HEAD" }]));
    let binding = json!({"into":read(&["branch","--show-current"]),
        "sourceRevision":source,"targetRevision":target,"gitState":"stopped","conflictId":"fixture-merge"});
    let mut saved: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
    for (key, value) in binding.as_object().unwrap() {
        saved["tasks"]["t1"]["merge"][key] = value.clone();
    }
    fs::write(f.path(), saved.to_string()).unwrap();
    Some(binding)
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
        let binding = bind_fixture_merge(&f, &fixture, case);
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
        } else if case["variant"] == "already-finished" {
            assert_eq!(reply["code"], "no_conflict");
        } else if case["action"] == "abort_merge" && case["variant"] == "no-merge-head" {
            assert_eq!(case["expected"]["code"], "git_failed");
            assert_eq!(reply["code"], "unsafe_conflict");
        } else if case["variant"] == "plain" {
            assert_eq!(case["expected"]["code"], "git_failed");
            assert_eq!(reply["code"], "unsafe_conflict");
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
        let mut expected = if case["variant"] == "already-finished" {
            f.replace(&case["saved"])
        } else {
            f.replace(&case["after"])
        };
        if let Some(binding) = binding {
            if expected["tasks"]["t1"]["merge"].is_object() {
                for (key, value) in binding.as_object().unwrap() {
                    expected["tasks"]["t1"]["merge"][key] = value.clone();
                }
            }
            if reply["ok"] == true {
                expected["tasks"]["t1"]["automaticReady"] = json!(false);
                if case["action"] == "finish_merge" {
                    let path = f.replace(&case["taskPath"]);
                    let head = git(&f, &fixture, Path::new(path.as_str().unwrap()),
                        &["rev-parse","HEAD"], None);
                    assert!(head.status.success());
                    let head = String::from_utf8(head.stdout).unwrap();
                    expected["tasks"]["t1"]["changesBaseRevision"] = binding["targetRevision"].clone();
                    expected["tasks"]["t1"]["merge"]["gitState"] = json!("completed");
                    expected["tasks"]["t1"]["merge"]["freshChecks"] =
                        json!({"state":"not_requested","revision":head.trim(),"after":null});
                }
            }
        }
        assert_eq!(saved, expected, "{} overlay", case["name"]);
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
async fn legacy_merge_record_can_be_cleared_after_owner_finishes_or_aborts_in_git() {
    let fixture = oracle();
    for (name, external) in [
        ("abort_merge-resolved", ["merge", "--abort"]),
        ("finish_merge-worktree", ["commit", "--no-edit"]),
        ("abort_merge-unresolved", ["merge", "--quit"]),
    ] {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap();
        let f = Fixture::new().await;
        let store = setup(&f, &fixture, case).unwrap();
        let authority_before = authority(&store);
        let saved_before = fs::read(f.path()).unwrap();
        let saved: Value = serde_json::from_slice(&saved_before).unwrap();
        assert_eq!(saved, f.replace(&case["saved"]));
        for key in ["conflictId", "sourceRevision", "targetRevision"] {
            assert!(saved["tasks"]["t1"]["merge"].get(key).is_none());
        }

        let path = f.replace(&case["taskPath"]);
        let path = Path::new(path.as_str().unwrap());
        let legacy_request = |request| {
            json!({"action":"abort_merge","taskId":"t1","requestId":request,"discardLegacy":true})
        };
        if path == f.root {
            // The historical main-checkout fixtures use "main" in Git but save
            // "task" in the overlay. Refuse that mismatch, then correct only Git.
            let reply = f.call(legacy_request("wrong-branch")).await;
            assert_eq!(reply["code"], "unsafe_conflict", "{name}: {reply}");
            let out = git(&f, &fixture, path, &["branch", "-m", "task"], None);
            assert!(out.status.success(), "{name}: could not rename fixture branch");
            assert_eq!(fs::read(f.path()).unwrap(), saved_before);
        }
        let git_before = observation(&f, &fixture, case);
        for action in ["finish_merge", "abort_merge"] {
            let reply = f.call(json!({"action":action,"taskId":"t1","requestId":action})).await;
            assert_eq!(reply["code"], "unsafe_conflict", "{reply}");
        }
        let reply = f.call(legacy_request("still-open")).await;
        assert_eq!(reply["code"], "legacy_merge_still_open", "{reply}");
        let mut invalid = legacy_request("ambiguous-clear");
        invalid["discardLegacy"] = json!("true");
        let reply = f.call(invalid).await;
        assert_eq!(reply["code"], "invalid_request", "{reply}");
        assert_eq!(observation(&f, &fixture, case), git_before);
        assert_eq!(fs::read(f.path()).unwrap(), saved_before);

        let out = git(&f, &fixture, path, &external, None);
        assert!(out.status.success(), "{name}: {external:?}");
        let git_after_external = observation(&f, &fixture, case);
        assert_ne!(git_after_external, git_before);

        let request = legacy_request("after-external");
        let reply = f.call(request.clone()).await;
        if external[1] == "--quit" {
            // Removing the marker alone is insufficient: the index is still unmerged.
            assert_eq!(reply["code"], "legacy_merge_still_open", "{reply}");
            assert_eq!(fs::read(f.path()).unwrap(), saved_before);
        } else {
            assert_eq!(reply["ok"], true, "{reply}");
            let cleared: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
            let record = &cleared["tasks"]["t1"];
            assert_eq!(record["merge"], Value::Null);
            assert_eq!(record["conflict"], "none");
            assert_eq!(record["acceptance"], Value::Null);
            assert_eq!(record["automaticReady"], false);
            assert_eq!(record["resolvedBy"], Value::Null);
            assert_eq!(record["changesBaseRevision"], saved["tasks"]["t1"]["changesBaseRevision"]);
        }
        assert_eq!(f.call(request).await, reply);
        assert_eq!(observation(&f, &fixture, case), git_after_external);
        assert_eq!(authority(&store), authority_before);
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
            bind_fixture_merge(&f, &fixture, case).unwrap();
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
