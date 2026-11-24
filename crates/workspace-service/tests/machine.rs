//! Native machine parity at the service boundary. All Git remotes are local bare fixtures.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use workspace_service::{Config, LaunchSpec, Service};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    state: PathBuf,
    service: Arc<Service>,
    id: String,
}
impl Fixture {
    async fn new() -> Self {
        let f = Self::plain().await;
        f.git(&["init", "-b", "main"]);
        f.git(&["config", "user.name", "Owner"]);
        f.git(&["config", "user.email", "owner@example.invalid"]);
        f.git(&["config", "commit.gpgsign", "false"]);
        f.git(&[
            "config",
            "core.hooksPath",
            f.base.join("hooks").to_str().unwrap(),
        ]);
        f.write("a.txt", b"one\ntwo\n");
        f.git(&["add", "a.txt"]);
        f.git(&["commit", "-m", "first"]);
        f
    }
    async fn plain() -> Self {
        let base = std::env::temp_dir().join(format!(
            "cw-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        let repo = base.join("repo");
        fs::create_dir(&repo).unwrap();
        let state = base.join("state");
        // No engine is activated. Native calls must still work.
        let service = Service::bind(Config {
            runtime_dir: base.join("runtime"),
            state_dir: state.clone(),
            allowed_root: base.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 16,
        })
        .await
        .unwrap();
        let id = service.register(&repo, "Fixture").await.unwrap()["projectId"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            base,
            repo,
            state,
            service,
            id,
        }
    }
    fn store(&self) -> cortex_core::Store {
        let key = cortex_core::ProjectKey::new(
            &self.repo,
            if workspace_protocol::channel::current().is_dev() {
                cortex_core::StoreChannel::Dev
            } else {
                cortex_core::StoreChannel::Release
            },
        )
        .unwrap();
        let store = cortex_core::Store::open(key, cortex_core::OpenMode::Initialize).unwrap();
        store
            .write(|db| {
                db.execute(
                    "INSERT OR IGNORE INTO workspace_projects VALUES (?,?,?,?,1)",
                    (
                        &self.id,
                        self.repo.to_str().unwrap(),
                        "fixture",
                        cortex_core::now(),
                    ),
                )?;
                Ok(())
            })
            .unwrap();
        store
    }
    fn write(&self, name: &str, data: &[u8]) {
        let p = self.repo.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }
    fn git(&self, a: &[&str]) -> String {
        let output = Command::new("git")
            .args(a)
            .current_dir(&self.repo)
            .env("HOME", &self.base)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture Git failed: {a:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
    async fn call(&self, op: &str, p: Value) -> Value {
        self.service
            .machine(
                serde_json::from_value(json!({"op":op,"projectId":self.id,"params":p})).unwrap(),
            )
            .await
            .unwrap()
    }
    async fn ok(&self, op: &str, p: Value) -> Value {
        let reply = self.call(op, p).await;
        assert_eq!(reply["ok"], true, "{reply}");
        reply["result"].clone()
    }
    async fn lease(&self, file: &str) -> Value {
        self.ok("repo.file.lease.acquire", json!({"file":file})).await["leaseToken"].clone()
    }
    async fn action(&self, p: Value) -> Value {
        self.ok("git", p).await
    }
    async fn snapshot(&self) -> Value {
        self.ok("repo.snapshot", json!({})).await
    }
    fn remote(&self) -> PathBuf {
        let path = self.base.join("remote.git");
        self.git(&["init", "--bare", path.to_str().unwrap()]);
        self.git(&["remote", "add", "origin", path.to_str().unwrap()]);
        path
    }
    async fn stop(&self) {
        self.service.stop().await.unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
#[tokio::test]
async fn code_lsp_without_a_bundle_leaves_code_readable() {
    let fixture = Fixture::plain().await;
    fixture.write("lib.rs", b"fn on_disk() {}");
    let result = fixture.call("repo.lsp.open",
        json!({"file":"lib.rs","text":"fn unsaved() {}","version":1})).await;
    assert_eq!(result["ok"], false);
    assert_eq!(result["code"], "lsp_unavailable");
    assert_eq!(result["reason"], "server_not_installed");
    assert_eq!(fixture.ok("repo.file", json!({"file":"lib.rs"})).await["text"], "fn on_disk() {}");
    assert_eq!(fs::read(fixture.repo.join("lib.rs")).unwrap(), b"fn on_disk() {}");
    fixture.stop().await;
}

#[tokio::test]
async fn explicit_workflows_and_maintenance_reads_need_no_activation() {
    let f = Fixture::new().await;
    let _store = f.store();
    for action in ["harvest", "harvest-refresh", "skill-management"] {
        let v = f
            .ok(
                "workflow.describe",
                json!({"action":action,"scope":"  source-backed Rust docs  "}),
            )
            .await;
        assert!(v["description"]
            .as_str()
            .unwrap()
            .contains("source-backed Rust docs"));
        assert!(v["description"].as_str().unwrap().contains("read_skill"));
    }
    assert_eq!(
        f.call(
            "workflow.describe",
            json!({"action":"skill-management","scope":" "})
        )
        .await["code"],
        "description_required"
    );
    assert_eq!(
        f.call(
            "workflow.describe",
            json!({"action":"harvest","scope":false})
        )
        .await["code"],
        "invalid_workflow"
    );
    assert_eq!(
        f.call(
            "workflow.describe",
            json!({"action":"harvest","scope":"я".repeat(30001)})
        )
        .await["code"],
        "invalid_workflow"
    );
    let bounded = f
        .ok(
            "workflow.describe",
            json!({"action":"harvest","scope":"я".repeat(30000)}),
        )
        .await;
    assert!(bounded["description"]
        .as_str()
        .unwrap()
        .contains("я".repeat(30000).as_str()));
    assert_eq!(
        f.ok("maintenance", json!({"action":"voice.status"})).await["phase"],
        "not_downloaded"
    );
    assert_eq!(
        f.call("maintenance", json!({"action":"read","id":"../bad"}))
            .await["code"],
        "invalid_job"
    );
    assert_eq!(
        f.call("maintenance", json!({"action":"read","id":"0".repeat(32)}))
            .await["code"],
        "unknown_job"
    );
    let id = "a".repeat(32);
    let folder = f.state.join("maintenance").join(&id);
    fs::create_dir_all(&folder).unwrap();
    fs::set_permissions(folder.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        folder.join("status.json"),
        json!({"id":id,"kind":"install","phase":"starting","updated":0}).to_string(),
    )
    .unwrap();
    assert_eq!(
        f.ok("maintenance", json!({"action":"read","id":id})).await["phase"],
        "unknown"
    );
    assert_eq!(
        f.call("maintenance", json!({"action":"cancel","id":id}))
            .await["code"],
        "install_in_progress"
    );
    assert_eq!(
        f.call("maintenance", json!({"action":"acknowledge","id":id}))
            .await["code"],
        "confirmation_required"
    );
    let v = f
        .ok(
            "maintenance",
            json!({"action":"acknowledge","id":id,"confirmed":true}),
        )
        .await;
    assert_eq!(v["phase"], "unknown");
    assert_eq!(v["acknowledged"], true);
    f.stop().await;
}

#[tokio::test]
async fn cleanup_and_sprint_deletion_use_native_temp_store_with_start_fences() {
    let f = Fixture::new().await;
    let store = f.store();
    let sprint = f
        .ok(
            "product.sprint",
            json!({"action":"create","description":"Retire this sprint"}),
        )
        .await["sprintId"]
        .as_str()
        .unwrap()
        .to_owned();
    store.write(|db|{
        for task in ["member","other"] {
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,?,'fixture',?)",
                (task,&f.id,cortex_core::now()))?;
            db.execute("INSERT INTO workspace_task_options VALUES (?,'{}')",[task])?;
        }
        Ok(())
    }).unwrap();
    f.ok(
        "product.sprint",
        json!({"action":"patch","sprintId":sprint,"patch":{"taskIds":["member"]}}),
    )
    .await;
    // Each task's answer files are kept with it and released with it.
    let product_path = f.state.join("product").join(format!("{}.json", f.id));
    let answer_ref = |name: &str| json!({"attachments":[{"name":name,"mimeType":"image/png","size":1,"sha256":"a".repeat(64)}]});
    let mut saved: Value = serde_json::from_slice(&fs::read(&product_path).unwrap()).unwrap();
    saved["answerFiles"] = json!({"member":answer_ref("m.png"),"other":answer_ref("o.png")});
    fs::write(&product_path, saved.to_string()).unwrap();
    let answers = || -> Value {
        serde_json::from_slice::<Value>(&fs::read(&product_path).unwrap()).unwrap()["answerFiles"]
            .clone()
    };
    assert_eq!(
        f.call(
            "product.sprint",
            json!({"action":"delete_prepare","sprintId":sprint})
        )
        .await["code"],
        "confirmation_required"
    );
    let prepared = f
        .ok(
            "product.sprint",
            json!({"action":"delete_prepare","sprintId":sprint,"confirmed":true}),
        )
        .await;
    assert_eq!(prepared["done"], false);
    assert_eq!(prepared["executionIds"], json!([]));
    store
        .read(|db| {
            let raw: String = db.query_row(
                "SELECT options_json FROM workspace_task_options WHERE task_id='member'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(
                serde_json::from_str::<Value>(&raw)?["deletingSprint"],
                sprint
            );
            Ok(())
        })
        .unwrap();
    let start = json!({"protocol":{"major":1,"minor":8},"requestId":"start-deleted-sprint","projectId":f.id,"actorId":"fixture",
        "operation":"tasks.start","target":"member","expectedVersion":"1","ownerEpoch":null,"payload":{}});
    assert_eq!(
        cortex_core::workspace::start_native(&store, &f.id, "fixture", &start)
            .unwrap_err()
            .code(),
        "sprint_deleting"
    );
    let result = f
        .ok(
            "product.sprint",
            json!({"action":"delete_finish","sprintId":sprint,"confirmed":true}),
        )
        .await;
    assert_eq!(result["done"], true);
    assert_eq!(result["taskIds"], json!(["member"]));
    assert_eq!(answers(), json!({"other":answer_ref("o.png")}));
    assert_eq!(
        f.ok(
            "product.sprint",
            json!({"action":"delete","sprintId":sprint,"confirmed":true})
        )
        .await,
        result
    );
    store
        .read(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM workspace_tasks WHERE task_id='other'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM workspace_tasks WHERE task_id='member'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            Ok(())
        })
        .unwrap();
    let path = f.state.join("product").join(format!("{}.json", f.id));
    let mut product: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    product["tasks"]["other"] = json!({"status":"done","doneAt":1,"archive":{"at":1,"previousStatus":"done"},"work":"merged"});
    fs::write(&path, product.to_string()).unwrap();
    let cleanup = f.ok("cleanup.run", json!({})).await;
    assert_eq!(cleanup["removed"], 1);
    assert_eq!(cleanup["skipped"], 0);
    assert_eq!(answers(), json!({}));
    assert_eq!(f.ok("overview", json!({})).await["cleanup"], cleanup);
    store
        .read(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM workspace_tasks", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
    f.stop().await;
}

#[tokio::test]
async fn built_native_maintenance_worker_clones_and_refuses_replay() {
    let f = Fixture::new().await;
    let _store = f.store();
    let bare = f.base.join("source.git");
    f.git(&["init", "--bare", bare.to_str().unwrap()]);
    let id = "5".repeat(32);
    let job = f.state.join("maintenance").join(&id);
    fs::create_dir_all(&job).unwrap();
    for p in [&job, job.parent().unwrap()] {
        fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let destination = f.base.join("cloned");
    fs::write(
        job.join("request.json"),
        json!({"kind":"clone","url":bare,"destination":destination}).to_string(),
    )
    .unwrap();
    fs::write(
        job.join("status.json"),
        json!({"kind":"clone","phase":"starting","updated":0}).to_string(),
    )
    .unwrap();
    let run = || {
        let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_workspace-service"));
        c.args([
            "--native-maintenance-worker",
            f.state.to_str().unwrap(),
            &id,
        ])
        .env_clear()
        .env("HOME", &f.base)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
        c
    };
    let first = tokio::time::timeout(Duration::from_secs(10), run().status())
        .await
        .unwrap()
        .unwrap();
    assert!(first.success());
    assert!(destination.join(".git").is_dir());
    let status: Value =
        serde_json::from_slice(&fs::read(job.join("status.json")).unwrap()).unwrap();
    assert_eq!(status["phase"], "done");
    assert!(
        !run().status().await.unwrap().success(),
        "a retained job identity cannot replay"
    );
    f.stop().await;
}

#[tokio::test]
async fn native_retention_keeps_dirty_work_and_unconfirmed_executions() {
    let f = Fixture::new().await;
    let store = f.store();
    store.write(|db|{
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES ('dirty',?,'fixture',?)",
            (&f.id,cortex_core::now()))?;
        db.execute("INSERT INTO workspace_task_options VALUES ('dirty','{}')",[])?;
        Ok(())
    }).unwrap();
    let sprint = f
        .ok(
            "product.sprint",
            json!({"action":"create","description":"fixture"}),
        )
        .await["sprintId"]
        .clone();
    let file = f.state.join("product").join(format!("{}.json", f.id));
    let mut value: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    value["tasks"]["dirty"] =
        json!({"status":"done","work":"unmerged","doneAt":1,"archive":{"at":1}});
    fs::write(&file, value.to_string()).unwrap();
    let cleanup = f.ok("cleanup.run", json!({})).await;
    assert_eq!(cleanup["removed"], 0);
    assert_eq!(cleanup["skipped"], 1);
    assert_eq!(cleanup["asked"], 1);
    let value: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert!(value["tasks"]["dirty"]["archiveQuestion"].is_null());
    assert_eq!(
        value["tasks"]["dirty"]["archive"]["at"], 1,
        "stays archived with its work"
    );
    assert!(value["tasks"]["dirty"]["purgeBlocker"].is_string());
    // A runtime binding that cannot prove quiet is not permission to delete the draft.
    store.write(|db|{
        db.execute("INSERT INTO workspace_executions VALUES ('unknown','dirty','missing','session',1,1,?)",[cortex_core::now()])?;
        db.execute("UPDATE workspace_tasks SET execution_id='unknown' WHERE task_id='dirty'",[])?;
        Ok(())
    }).unwrap();
    f.ok(
        "product.sprint",
        json!({"action":"patch","sprintId":sprint,"patch":{"taskIds":["dirty"]}}),
    )
    .await;
    let prepared = f
        .ok(
            "product.sprint",
            json!({"action":"delete_prepare","sprintId":sprint,"confirmed":true}),
        )
        .await;
    assert_eq!(prepared["executionIds"], json!(["unknown"]));
    let finish = f
        .ok(
            "product.sprint",
            json!({"action":"delete_finish","sprintId":sprint,"confirmed":true}),
        )
        .await;
    assert_eq!(finish["done"], false);
    let reason = finish["reason"].as_str().unwrap();
    // Owner-readable: the task title, never its id or the raw refusal code.
    assert!(reason.contains("“fixture” no longer belongs"), "{reason}");
    assert!(
        !reason.contains("unauthorized") && !reason.contains("dirty:"),
        "{reason}"
    );
    store
        .read(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM workspace_tasks", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            Ok(())
        })
        .unwrap();
    f.stop().await;
}

#[tokio::test]
async fn status_diff_history_and_nested_untracked_follow_git_ignore_rules() {
    let f = Fixture::new().await;
    f.write("a.txt", b"one\nchanged\n");
    f.write("b.txt", b"new\n");
    let s = f.snapshot().await;
    assert_eq!(s["openBranch"], "main");
    assert_eq!(s["defaultBranch"], "main");
    assert_eq!(s["tree"], json!(["a.txt", "b.txt"]));
    assert_eq!(s["commits"][0]["message"], "first");
    let file = &s["checkouts"][0]["files"][0];
    assert_eq!(file["stage"], "unstaged");
    assert_eq!(file["add"], 1);
    assert_eq!(file["del"], 1);
    assert_eq!(
        file["hunks"][0]["lines"],
        json!([{"kind":"ctx","old":1,"new":1,"text":"one"},{"kind":"del","old":2,"new":null,"text":"two"},{"kind":"add","old":null,"new":2,"text":"changed"}])
    );
    let paths = [
        "new/deep/file.txt",
        "app/node_modules/.vite/results.json",
        ".venv/new.txt",
        "nested/__pycache__/new.txt",
    ];
    for name in paths {
        f.write(name, b"new\n");
    }
    f.write("ignored/cache", b"ignored");
    f.write(".cortex/private", b"private");
    f.write(".gitignore", b"ignored/\n");
    f.git(&["add", ".gitignore"]);
    let s = f.snapshot().await;
    let files = s["checkouts"][0]["files"].as_array().unwrap();
    for name in paths {
        assert!(files
            .iter()
            .any(|r| r["path"] == name && r["stage"] == "untracked"));
    }
    assert!(!files
        .iter()
        .any(|r| r["path"] == "ignored/cache" || r["path"] == ".cortex/private"));
    f.stop().await;
}
#[tokio::test]
async fn large_untracked_tree_collapses_and_one_auto_add_commits_every_visible_path() {
    let f = Fixture::new().await;
    f.write("app/package.json", b"{}");
    f.git(&["add", "app/package.json"]);
    f.git(&["commit", "-m", "app"]);
    for i in 0..2100 {
        f.write(&format!("app/node_modules/pkg{i}/index.js"), b"new\n");
    }
    for name in ["new.txt", "cache/.vite/TAG", "cache/.vitest-cache/metadata"] {
        f.write(name, b"new\n");
    }
    let s = f.snapshot().await;
    let files = s["checkouts"][0]["files"].as_array().unwrap();
    assert_eq!(files.len(), 4);
    assert!(files
        .iter()
        .any(|r| r["path"] == "app/node_modules" && r["hunks"] == json!([])));
    assert_eq!(s["treeTruncated"], false);
    assert!(!s["tree"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().starts_with("app/node_modules")));
    // A hook verifies exactly one add by looking at all selected paths at commit time.
    f.action(json!({"action":"commit","message":"everything"}))
        .await;
    assert_eq!(f.git(&["ls-files"]).lines().count(), 2105);
    f.stop().await;
}
#[tokio::test]
async fn staging_revert_commit_amend_and_private_index() {
    let f = Fixture::new().await;
    f.write("a.txt", b"selected");
    f.write("unselected", b"keep unstaged");
    f.action(json!({"action":"stage","path":"a.txt","staged":true}))
        .await;
    f.action(json!({"action":"stage","path":"a.txt","staged":false}))
        .await;
    assert!(f.git(&["diff", "--cached", "--name-only"]).is_empty());
    f.action(json!({"action":"stage","path":"a.txt","staged":true}))
        .await;
    f.action(json!({"action":"commit","message":"selected only"}))
        .await;
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "a.txt"
    );
    f.write("a.txt", b"broken");
    assert_eq!(
        f.call("git", json!({"action":"revert","path":"a.txt"}))
            .await["code"],
        "confirmation_required"
    );
    f.action(json!({"action":"revert","path":"a.txt","confirmed":true}))
        .await;
    assert_eq!(fs::read(f.repo.join("a.txt")).unwrap(), b"selected");
    f.action(json!({"action":"revert","path":"unselected","confirmed":true}))
        .await;
    assert!(!f.repo.join("unselected").exists());
    f.action(json!({"action":"commit","message":"new title","amend":true}))
        .await;
    assert_eq!(f.git(&["rev-list", "--count", "HEAD"]), "2");
    assert_eq!(f.git(&["log", "-1", "--format=%s"]), "new title");
    f.write(".cortex-workspace.json", b"{}");
    f.git(&["add", ".cortex-workspace.json"]);
    assert_eq!(
        f.call("git", json!({"action":"commit","message":"private"}))
            .await["code"],
        "private_index"
    );
    f.stop().await;
}
#[tokio::test]
async fn commit_literal_names_and_no_remote_needs_no_configuration() {
    let f = Fixture::new().await;
    f.write("[x].txt", b"brackets");
    f.write("x.txt", b"another");
    f.write(".cortex-dev/private", b"private");
    f.action(json!({"action":"commit","message":"local only"}))
        .await;
    assert_eq!(f.git(&["ls-files"]), "[x].txt\na.txt\nx.txt");
    assert_eq!(
        f.call(
            "git",
            json!({"action":"push","remote":"origin","branch":"main","confirmed":true})
        )
        .await["code"],
        "unknown_remote"
    );
    assert_eq!(
        f.call("git", json!({"action":"pull"})).await["code"],
        "upstream_required"
    );
    f.stop().await;
}
#[tokio::test]
async fn first_request_push_by_name_and_display_url_fetch_and_pull() {
    for by_name in [true, false] {
        let f = Fixture::new().await;
        let remote = f.remote();
        f.git(&["push", "-u", "origin", "main"]);
        f.write("a.txt", b"second");
        f.git(&["commit", "-am", "second"]);
        let s = f.snapshot().await;
        assert_eq!(s["checkouts"][0]["ahead"], 1);
        f.action(json!({"action":"push","remote":if by_name{json!("origin")}else{s["remote"].clone()},"branch":"main","confirmed":true})).await;
        assert_eq!(f.snapshot().await["checkouts"][0]["ahead"], 0);
        f.git(&["checkout", "-b", "incoming"]);
        f.write("a.txt", b"incoming");
        f.git(&["commit", "-am", "incoming"]);
        f.git(&["push", "origin", "incoming:main"]);
        f.git(&["checkout", "main"]);
        f.action(json!({"action":"fetch"})).await;
        assert_eq!(f.snapshot().await["checkouts"][0]["behind"], 1);
        f.action(json!({"action":"pull"})).await;
        assert_eq!(fs::read(f.repo.join("a.txt")).unwrap(), b"incoming");
        assert_eq!(f.snapshot().await["checkouts"][0]["behind"], 0);
        assert!(remote.is_dir());
        f.stop().await;
    }
}
#[tokio::test]
async fn stale_ambiguous_unconfigured_remotes_and_confirmation_are_refused() {
    let f = Fixture::new().await;
    let remote = f.remote();
    let url = remote.to_string_lossy();
    assert_eq!(
        f.call("git", json!({"action":"push","remote":url,"branch":"main"}))
            .await["code"],
        "confirmation_required"
    );
    f.git(&[
        "remote",
        "set-url",
        "origin",
        f.base.join("changed").to_str().unwrap(),
    ]);
    assert_eq!(
        f.call(
            "git",
            json!({"action":"push","remote":url,"branch":"main","confirmed":true})
        )
        .await["code"],
        "unknown_remote"
    );
    f.git(&["remote", "set-url", "origin", &url]);
    f.git(&["remote", "add", "duplicate", &url]);
    assert_eq!(
        f.call(
            "git",
            json!({"action":"push","remote":url,"branch":"main","confirmed":true})
        )
        .await["code"],
        "ambiguous_remote"
    );
    assert_eq!(
        f.call(
            "git",
            json!({"action":"push","remote":"not configured","branch":"main","confirmed":true})
        )
        .await["code"],
        "unknown_remote"
    );
    f.stop().await;
}
#[tokio::test]
async fn push_never_forces_non_fast_forward_or_accepts_an_option_as_branch() {
    let f = Fixture::new().await;
    f.remote();
    f.git(&["push", "-u", "origin", "main"]);
    f.git(&["checkout", "-b", "incoming"]);
    f.write("a.txt", b"incoming");
    f.git(&["commit", "-am", "incoming"]);
    f.git(&["push", "origin", "incoming:main"]);
    f.git(&["checkout", "main"]);
    f.write("a.txt", b"divergent");
    f.git(&["commit", "-am", "divergent"]);
    // A rejected non-fast-forward push says so; an option is never taken as a branch.
    for (branch, code) in [("main", "push_not_fast_forward"), ("--force", "git_failed")] {
        assert_eq!(
            f.call(
                "git",
                json!({"action":"push","remote":"origin","branch":branch,"confirmed":true})
            )
            .await["code"],
            code
        );
    }
    let out = Command::new("git")
        .arg("--git-dir")
        .arg(f.base.join("remote.git"))
        .args(["log", "-1", "--format=%s", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "incoming");
    f.stop().await;
}
#[tokio::test]
async fn hunks_stage_unstage_and_revert_independently() {
    let f = Fixture::new().await;
    let original = (0..40).map(|n| format!("{n}\n")).collect::<String>();
    f.write("long", &original.as_bytes());
    f.git(&["add", "long"]);
    f.git(&["commit", "-m", "long"]);
    let edited = original
        .replacen("1\n", "first\n", 1)
        .replacen("35\n", "second\n", 1);
    f.write("long", edited.as_bytes());
    f.action(json!({"action":"stage","path":"long","staged":true,"hunk":0}))
        .await;
    let s = f.snapshot().await;
    let files = s["checkouts"][0]["files"].as_array().unwrap();
    assert!(files
        .iter()
        .any(|r| r["stage"] == "staged" && r["add"] == 1));
    assert!(files
        .iter()
        .any(|r| r["stage"] == "unstaged" && r["add"] == 1));
    f.action(json!({"action":"stage","path":"long","staged":false,"hunk":0}))
        .await;
    f.action(json!({"action":"revert","path":"long","hunk":0,"confirmed":true}))
        .await;
    let text = fs::read_to_string(f.repo.join("long")).unwrap();
    assert!(!text.contains("first"));
    assert!(text.contains("second"));
    assert_eq!(
        f.call(
            "git",
            json!({"action":"stage","path":"long","staged":true,"hunk":99})
        )
        .await["code"],
        "stale_hunk"
    );
    f.stop().await;
}
#[tokio::test]
async fn project_skills_are_named_for_the_composer_without_their_bodies() {
    let f = Fixture::new().await;
    assert_eq!(
        f.ok("repo.skills", json!({})).await,
        json!({"skills":[],"omitted":0})
    );
    fs::create_dir_all(f.repo.join(".agents/skills/demo-data")).unwrap();
    fs::write(
        f.repo.join(".agents/skills/demo-data/SKILL.md"),
        "---\nname: demo-data\ndescription: Refresh the demo dataset.\n---\nbody text\n",
    )
    .unwrap();
    let r = f.ok("repo.skills", json!({})).await;
    assert_eq!(
        r["skills"],
        json!([{"name":"demo-data","path":".agents/skills/demo-data/SKILL.md","description":"Refresh the demo dataset.","pathOnly":false}])
    );
    assert!(!r.to_string().contains("body text"));
    f.stop().await;
}
#[tokio::test]
async fn plain_folder_snapshot_init_and_file_operations() {
    let f = Fixture::new().await;
    fs::remove_dir_all(f.repo.join(".git")).unwrap();
    let s = f.snapshot().await;
    assert_eq!(s["hasGit"], false);
    assert_eq!(s["tree"], json!(["a.txt"]));
    assert_eq!(
        f.ok("repo.file", json!({"file":"a.txt"})).await["text"],
        "one\ntwo\n"
    );
    assert_eq!(
        f.ok("repo.locate", json!({"file":"a.txt"})).await["path"],
        json!(f.repo.join("a.txt"))
    );
    f.ok("git_init", json!({})).await;
    assert_eq!(f.snapshot().await["commits"], json!([]));
    assert_eq!(
        f.call("git_init", json!({})).await["code"],
        "repository_exists"
    );
    f.stop().await;
}
#[tokio::test]
async fn plain_folder_opens_with_git_absent() {
    const CHILD: &str = "CORTEX_TEST_PLAIN_FOLDER_NO_GIT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // Isolate PATH in a child process instead of mutating the shared test environment.
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "plain_folder_opens_with_git_absent"])
            .env(CHILD, "1")
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let f = Fixture::plain().await;
    assert_eq!(
        Command::new("git")
            .current_dir(&f.repo)
            .arg("--version")
            .output()
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    f.write("a.txt", b"read without git");
    let snapshot = f.snapshot().await;
    assert_eq!(snapshot["hasGit"], false);
    assert_eq!(snapshot["tree"], json!(["a.txt"]));
    assert_eq!(
        f.ok("repo.file", json!({"file":"a.txt"})).await["text"],
        "read without git"
    );
    assert_eq!(
        f.call("git_init", json!({})).await["code"],
        "git_unavailable"
    );

    fs::create_dir(f.repo.join(".git")).unwrap();
    assert_eq!(
        f.call("repo.snapshot", json!({})).await["code"],
        "git_unavailable"
    );
    f.stop().await;
}

#[tokio::test]
async fn branches_worktree_identity_dirty_deletion_and_revert_commit() {
    let f = Fixture::new().await;
    assert_eq!(
        f.call("git", json!({"action":"branch","name":"--force"}))
            .await["code"],
        "invalid_branch"
    );
    f.action(json!({"action":"branch","name":"work"})).await;
    let work = f.base.join("work");
    f.git(&["worktree", "add", work.to_str().unwrap(), "work"]);
    let s = f.snapshot().await;
    let id = s["checkouts"][1]["id"].clone();
    assert_eq!(s["checkouts"][1]["kind"], "worktree");
    fs::write(work.join("new"), b"new").unwrap();
    assert_eq!(
        f.call(
            "git",
            json!({"action":"delete_worktree","checkoutId":id,"confirmed":true})
        )
        .await["code"],
        "dirty_worktree"
    );
    fs::remove_file(work.join("new")).unwrap();
    f.action(json!({"action":"delete_worktree","checkoutId":id,"confirmed":true}))
        .await;
    assert!(!work.exists());
    f.write("a.txt", b"changed");
    f.git(&["commit", "-am", "second"]);
    let sha = f.git(&["rev-parse", "HEAD"]);
    f.action(json!({"action":"revert_commit","sha":sha,"confirmed":true}))
        .await;
    assert_eq!(fs::read(f.repo.join("a.txt")).unwrap(), b"one\ntwo\n");
    f.stop().await;
}
#[tokio::test]
async fn inspection_of_required_missing_filters_is_raw_and_writes_keep_policy() {
    let f = Fixture::new().await;
    f.write(".gitattributes", b"*.dat filter=missing\n");
    f.write("managed.dat", b"raw");
    f.git(&["add", ".gitattributes", "managed.dat"]);
    f.git(&["commit", "-m", "managed"]);
    f.git(&[
        "config",
        "filter.missing.clean",
        "/definitely/not/a/program",
    ]);
    f.git(&["config", "filter.missing.required", "true"]);
    f.write("managed.dat", b"new raw");
    let before = fs::read(f.repo.join(".git/config")).unwrap();
    let s = f.snapshot().await;
    let files = s["checkouts"][0]["files"].as_array().unwrap();
    assert!(files.iter().any(|r| r["path"] == "managed.dat"
        && r["filter"] == "missing"
        && r["hunks"] == json!([])));
    assert!(
        f.ok("repo.file", json!({"file":"managed.dat"})).await["reason"]
            .as_str()
            .unwrap()
            .contains("filter")
    );
    assert_eq!(
        f.call(
            "git",
            json!({"action":"stage","path":"managed.dat","staged":true})
        )
        .await["code"],
        "git_failed"
    );
    assert_eq!(fs::read(f.repo.join(".git/config")).unwrap(), before);
    f.stop().await;
}
#[tokio::test]
async fn file_revision_conflicts_absent_save_exact_bytes_mode_and_concurrent_saves() {
    let f = Fixture::new().await;
    let read = f.ok("repo.file", json!({"file":"a.txt"})).await;
    assert_eq!(read["readOnly"], Value::Null);
    assert_eq!(read["writer"], Value::Null);
    let base = read["revision"].clone();
    let token = f.lease("a.txt").await;
    f.write("a.txt", b"worker");
    assert_eq!(
        f.ok(
            "repo.file.save",
            json!({"file":"a.txt","text":"owner","revision":base,"leaseToken":token})
        )
        .await["outcome"],
        "conflict"
    );
    fs::remove_file(f.repo.join("a.txt")).unwrap();
    assert_eq!(
        f.ok(
            "repo.file.save",
            json!({"file":"a.txt","text":"owner","revision":base,"leaseToken":token})
        )
        .await["revision"],
        Value::Null
    );
    let saved = f
        .ok(
            "repo.file.save",
            json!({"file":"a.txt","text":"owner","revision":null,"leaseToken":token}),
        )
        .await;
    assert_eq!(saved["outcome"], "saved");
    let mut jobs = Vec::new();
    for i in 0..6 {
        let service = f.service.clone();
        let id = f.id.clone();
        let base = saved["revision"].clone();
        let token = token.clone();
        jobs.push(tokio::spawn(async move {
            service
                .machine(
                    serde_json::from_value(json!({"op":"repo.file.save","projectId":id,
            "params":{"file":"a.txt","text":format!("writer {i}"),"revision":base,"leaseToken":token}}))
                    .unwrap(),
                )
                .await
                .unwrap()
        }));
    }
    let mut won = 0;
    for job in jobs {
        if job.await.unwrap()["result"]["outcome"] == "saved" {
            won += 1;
        }
    }
    assert_eq!(won, 1);
    f.write("run.sh", b"#!/bin/sh\r\necho hi\r\n");
    fs::set_permissions(f.repo.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let base = f.ok("repo.file", json!({"file":"run.sh"})).await["revision"].clone();
    let script_token = f.lease("run.sh").await;
    f.ok(
        "repo.file.save",
        json!({"file":"run.sh","text":"#!/bin/sh\r\necho bye\r\n\u{feff}","revision":base,"leaseToken":script_token}),
    )
    .await;
    assert_eq!(
        fs::metadata(f.repo.join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        fs::read(f.repo.join("run.sh")).unwrap(),
        "#!/bin/sh\r\necho bye\r\n\u{feff}".as_bytes()
    );
    f.stop().await;
}
#[tokio::test]
async fn code_lease_gates_save_and_heartbeat_without_persisting_the_token() {
    let f = Fixture::plain().await;
    f.write("a.txt", b"original");
    let rev = f.ok("repo.file", json!({"file":"a.txt"})).await["revision"].clone();
    assert_eq!(f.ok("repo.file.lease.status", json!({"file":"a.txt"})).await["status"], "available");
    let save = json!({"file":"a.txt","text":"new","revision":rev});
    assert_eq!(f.call("repo.file.save", save.clone()).await["code"], "edit_lease_lost");
    let lease = f.ok("repo.file.lease.acquire", json!({"file":"a.txt"})).await;
    assert_eq!(lease["expiresInMs"], 60000);
    assert_eq!(lease["renewAfterMs"], 20000);
    let token = &lease["leaseToken"];
    let store = f.store();
    let hash: String = store.read(|db| Ok(db.query_row(
        "SELECT token_hash FROM code_owner_file_leases", [], |r| r.get(0),
    )?)).unwrap();
    use sha2::{Digest, Sha256};
    assert!(token.as_str().is_some_and(|t| hash == format!("{:x}", Sha256::digest(t.as_bytes())) && hash != t));
    assert_eq!(f.ok("repo.file.lease.status", json!({"file":"a.txt"})).await["status"], "held");
    assert_eq!(f.call("repo.file.lease.acquire", json!({"file":"a.txt"})).await["code"], "owner_file_lease_held");
    let held = json!({"file":"a.txt","leaseToken":token});
    assert_eq!(f.ok("repo.file.lease.status", held.clone()).await["status"], "yours");
    assert_eq!(f.ok("repo.file.lease.renew", held.clone()).await["expiresInMs"], 60000);
    let mut fenced = save;
    fenced["leaseToken"] = token.clone();
    assert_eq!(f.call("repo.file.save", json!({"file":"other.txt","leaseToken":token,
        "revision":null,"text":"wrong file"})).await["code"], "edit_lease_lost");
    store.write(|db| {
        db.execute("UPDATE code_owner_file_leases SET expires_at=0", [])?;
        Ok(())
    }).unwrap();
    assert_eq!(f.call("repo.file.lease.renew", held).await["code"], "edit_lease_lost");
    assert_eq!(f.call("repo.file.save", fenced.clone()).await["code"], "edit_lease_lost");
    assert_eq!(fs::read(f.repo.join("a.txt")).unwrap(), b"original");
    let new_token = f.lease("a.txt").await;
    fenced["leaseToken"] = new_token.clone();
    assert_eq!(f.ok("repo.file.save", fenced.clone()).await["outcome"], "saved");
    assert_eq!(f.ok("repo.file.lease.release", json!({"file":"a.txt","leaseToken":new_token})).await["released"], true);
    assert_eq!(f.call("repo.file.save", fenced).await["code"], "edit_lease_lost");
    assert_eq!(fs::read(f.repo.join("a.txt")).unwrap(), b"new");
    drop(store);
    f.stop().await;
}

#[tokio::test]
async fn files_refuse_escapes_symlinks_binary_oversize_and_missing_base() {
    let f = Fixture::new().await;
    for name in [
        "../outside",
        "/etc/passwd",
        ".git/config",
        ".cortex/db",
        "a/../../b",
        "sub/.git/config",
        "sub/.cortex/x",
    ] {
        assert_eq!(
            f.call(
                "repo.file.save",
                json!({"file":name,"text":"x","revision":null})
            )
            .await["code"],
            "invalid_path"
        );
    }
    fs::create_dir(f.base.join("outside")).unwrap();
    fs::write(f.base.join("outside/secret"), b"secret").unwrap();
    symlink(f.base.join("outside"), f.repo.join("dir-link")).unwrap();
    symlink("a.txt", f.repo.join("inner-link")).unwrap();
    for name in ["dir-link/new", "inner-link"] {
        assert_eq!(
            f.call(
                "repo.file.lease.acquire",
                json!({"file":name})
            )
            .await["code"],
            "unsafe_edit_path"
        );
    }
    let read = f.ok("repo.file", json!({"file":"inner-link"})).await;
    assert_eq!(read["text"], "one\ntwo\n");
    assert!(read["readOnly"].as_str().unwrap().contains("link"));
    assert_eq!(
        f.call("repo.file", json!({"file":"dir-link/secret"})).await["code"],
        "invalid_path"
    );
    let token = f.lease("a.txt").await;
    assert_eq!(
        f.call("repo.file.save", json!({"file":"a.txt","text":"x","leaseToken":token}))
            .await["code"],
        "invalid_request"
    );
    assert_eq!(
        f.call(
            "repo.file.save",
            json!({"file":"a.txt","text":"x".repeat(512*1024+1),"revision":null,"leaseToken":token})
        )
        .await["code"],
        "file_too_large"
    );
    assert_eq!(
        f.call(
            "repo.file.save",
            json!({"file":"a.txt","text":"x\0y","revision":null,"leaseToken":token})
        )
        .await["code"],
        "binary_file"
    );
    f.write("blob", b"x\0y");
    f.write("latin", b"caf\xe9\n");
    f.write("big", &vec![b'x'; 512 * 1024 + 1]);
    assert_eq!(
        f.ok("repo.file", json!({"file":"blob"})).await["text"],
        Value::Null
    );
    let read = f.ok("repo.file", json!({"file":"latin"})).await;
    assert!(read["readOnly"].as_str().unwrap().contains("UTF-8"));
    let latin_token = f.lease("latin").await;
    assert_eq!(
        f.call(
            "repo.file.save",
            json!({"file":"latin","text":"cafe","revision":read["revision"],"leaseToken":latin_token})
        )
        .await["code"],
        "not_text"
    );
    let big_token = f.lease("big").await;
    assert_eq!(
        f.call(
            "repo.file.save",
            json!({"file":"big","text":"x","revision":null,"leaseToken":big_token})
        )
        .await["code"],
        "file_too_large"
    );
    assert!(
        f.call(
            "repo.file",
            json!({"checkoutId":"foreign:main","file":"a.txt"})
        )
        .await["ok"]
            == false
    );
    f.stop().await;
}
#[tokio::test]
async fn settings_validation_private_logs_and_diagnostics() {
    let f = Fixture::new().await;
    let defaults = f.ok("overview", json!({})).await;
    assert_eq!(defaults["autoArchiveDays"], 7);
    assert_eq!(defaults["retentionDays"], 30);
    assert_eq!(defaults["automaticArchivePurge"], true);
    assert_eq!(defaults["unreferencedAttachmentRetentionDays"], 1);
    for patch in [
        json!({"retentionDays":0}),
        json!({"retentionDays":true}),
        json!({"retentionDays":3651}),
        json!({"autoArchiveDays":0}),
        json!({"autoArchiveDays":3651}),
        json!({"automaticArchivePurge":"false"}),
        json!({"unreferencedAttachmentRetentionDays":0}),
        json!({"unreferencedAttachmentRetentionDays":1.5}),
        json!({"unreferencedAttachmentRetentionDays":3651}),
        json!({"unknown":1}),
        json!({"worktreeRoot":"relative"}),
        json!({"worktreeRoot":f.repo}),
    ] {
        assert_eq!(
            f.call("machine.update", json!({"patch":patch})).await["ok"],
            false
        );
    }
    let root = f.base.join("trees");
    f.ok(
        "machine.update",
        json!({"patch":{"retentionDays":14,"automaticArchivePurge":false,"unreferencedAttachmentRetentionDays":3650,"worktreeRoot":root,"name":"Desk"}}),
    )
    .await;
    let view = f.ok("overview", json!({})).await;
    assert_eq!(view["retentionDays"], 14);
    assert_eq!(view["automaticArchivePurge"], false);
    assert_eq!(view["unreferencedAttachmentRetentionDays"], 3650);
    assert_eq!(view["name"], "Desk");
    assert_eq!(view["worktreeRoot"], json!(root));
    assert!(
        f.ok("diagnostics.run", json!({})).await["checks"]
            .as_u64()
            .unwrap()
            >= 5
    );
    // The tool search path setting: owner folders are validated before they are saved;
    // folders inside a project are refused. This test service never reads a shell.
    let tools = f.base.join("tools-bin");
    fs::create_dir(&tools).unwrap();
    for bad in [json!("relative/bin"), json!(f.repo.join("bin"))] {
        fs::create_dir_all(f.repo.join("bin")).unwrap();
        assert_eq!(
            f.call(
                "maintenance",
                json!({"action":"tool_path.set_extra","directories":[bad]})
            )
            .await["ok"],
            false
        );
    }
    f.ok(
        "maintenance",
        json!({"action":"tool_path.set_extra","directories":[tools]}),
    )
    .await;
    assert_eq!(
        f.call("maintenance", json!({"action":"tool_path.reread"}))
            .await["ok"],
        false,
        "only the installed service reads the owner's shell"
    );
    let diagnostics = f.ok("diagnostics.run", json!({})).await;
    let npm = diagnostics["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "npm")
        .unwrap()
        .clone();
    assert!(
        !npm["detail"].as_str().unwrap().contains("desktop service"),
        "{npm}"
    );
    let logs = f.ok("logs.folder", json!({})).await["folder"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        fs::metadata(&logs).unwrap().permissions().mode() & 0o777,
        0o700
    );
    fs::write(Path::new(&logs).join("service.log"), vec![b' '; 65536]).unwrap();
    f.ok("logs.folder", json!({})).await;
    assert!(Path::new(&logs).join("service.log.1").exists());
    let row = fs::read_to_string(Path::new(&logs).join("service.log")).unwrap();
    for line in row.lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 2);
        assert_eq!(v["event"], "helper_ok");
    }
    f.stop().await;
}
#[tokio::test]
async fn concurrent_git_reads_need_no_helper_launch() {
    let f = Fixture::new().await;
    let mut jobs = Vec::new();
    for _ in 0..8 {
        let s = f.service.clone();
        let id = f.id.clone();
        jobs.push(tokio::spawn(async move {
            s.machine(serde_json::from_value(json!({"op":"repo.snapshot","projectId":id})).unwrap())
                .await
                .unwrap()
        }));
    }
    for j in jobs {
        assert_eq!(j.await.unwrap()["ok"], true);
    }
    assert!(f
        .service
        .machine(serde_json::from_value(json!({"op":"unknown"})).unwrap())
        .await
        .is_err());
    f.stop().await;
}

#[tokio::test]
async fn sprint_close_guards_and_archives_members_atomically() {
    let f = Fixture::new().await;
    let sprint = f.ok("product.sprint", json!({"action":"create","description":"Sprint"})).await["sprintId"].clone();
    let path = f.state.join("product").join(format!("{}.json", f.id));
    let read = || serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap();
    let write = |product: &Value| fs::write(&path, serde_json::to_vec(product).unwrap()).unwrap();
    let mut product = read();
    product["sprints"][0]["taskIds"] = json!(["working", "waiting", "review", "failed", "draft", "finished", "old"]);
    product["sprints"][0]["queueState"] = json!("running");
    let day = 86_400_000;
    let done = 2 * day;
    product["tasks"] = json!({
        "working":{"status":"working","title":"Running member"},
        "waiting":{"status":"needs_you","title":"Waiting member"},
        "review":{"status":"review","title":"Review member"},
        "failed":{"status":"failed","title":"Failed member"},
        "draft":{"status":"draft","sprintId":sprint,"title":"Queued member"},
        "reverse":{"status":"draft","sprintId":sprint},
        "finished":{"status":"done","doneAt":done,"work":"merged"},
        "old":{"status":"done","doneAt":0,"archive":{"at":1,"previousStatus":"done"}}
    });
    write(&product);
    let close = json!({"action":"close","sprintId":sprint,"confirmed":true});
    let refusal = f.call("product.sprint", close.clone()).await;
    assert_eq!(refusal["code"], "sprint_close_blocked", "{refusal}");
    for name in ["Running member (working)", "Waiting member (waiting)", "Review member (review)", "Failed member (failed)"] {
        assert!(refusal["message"].as_str().unwrap().contains(name), "{refusal}");
    }
    assert_eq!(read(), product, "a refusal must leave every record unchanged");
    for task in ["working", "waiting", "review", "failed"] {
        product["tasks"][task]["status"] = json!("done");
    }
    product["tasks"]["review"]["closedAs"] = json!("unconfirmed");
    write(&product);
    let refusal = f.call("product.sprint", close.clone()).await;
    assert_eq!(refusal["code"], "sprint_close_blocked", "{refusal}");
    assert!(refusal["message"].as_str().unwrap().contains("Review member (review): task records disagree"));
    assert_eq!(read(), product);
    product["tasks"]["review"]["closedAs"] = Value::Null;
    write(&product);
    f.ok("product.sprint", close.clone()).await;
    let closed = read();
    let at = closed["sprints"][0]["archivedAt"].as_u64().unwrap();
    assert_eq!(closed["sprints"][0]["queueState"], "paused");
    assert_eq!(closed["tasks"]["old"], product["tasks"]["old"]);
    assert!(closed["sprints"][0]["taskIds"].as_array().unwrap().contains(&json!("reverse")));
    for draft in ["draft", "reverse"] {
        assert_eq!(closed["tasks"][draft]["status"], "draft");
        assert_eq!(closed["tasks"][draft]["archive"], json!({"at":at,"previousStatus":"draft"}));
        assert!(closed["tasks"][draft]["doneAt"].is_null());
        assert!(!cortex_core::store::expired_closed_archive(&closed["tasks"][draft], at + 100 * day, 30));
    }
    let finished = &closed["tasks"]["finished"];
    assert_eq!(finished["doneAt"], done);
    assert_eq!(finished["archive"], json!({"at":at,"previousStatus":"done","retentionAt":done + 7 * day}));
    let expired = cortex_core::store::expired_closed_archive;
    assert!(!expired(finished, done + 37 * day - 1, 30));
    assert!(expired(finished, done + 37 * day, 30));
    assert!(!expired(&closed["tasks"]["working"], at + 100 * day, 30), "unknown completion is not a purge clock");
    let head = f.git(&["rev-parse", "HEAD"]);
    let purge = f.call("product.task", json!({"action":"purge","taskId":"draft","confirmed":true})).await;
    assert_eq!(purge["code"], "unmerged_work", "{purge}");
    assert_eq!(read(), closed);
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
    f.ok("product.task", json!({"action":"restore","taskId":"draft"})).await;
    let restored = read();
    assert_eq!(restored["tasks"]["draft"]["status"], "draft");
    assert!(restored["tasks"]["draft"]["archive"].is_null());
    assert!(restored["tasks"]["draft"]["sprintId"].is_null());
    assert!(!restored["sprints"][0]["taskIds"].as_array().unwrap().contains(&json!("draft")));
    f.ok("product.sprint", close).await;
    assert_eq!(read(), restored, "repeating Close must not rearchive any restored task");
    f.stop().await;
}

#[tokio::test]
async fn private_queue_comments_sprints_and_drafts_are_durable_without_core() {
    let f = Fixture::new().await;
    let message = |id: &str, task: &str| {
        json!({"id":id,"taskId":task,"workerId":null,"text":id,"mode":"queue",
        "delivery":"queued","at":1,"deliveredAt":null,"failure":null})
    };
    for (id, task) in [("one", "task"), ("two", "task"), ("foreign", "other")] {
        f.ok(
            "product.message",
            json!({"action":"add","message":message(id,task)}),
        )
        .await;
    }
    f.ok(
        "product.message",
        json!({"action":"add","message":message("one","task")}),
    )
    .await;
    f.ok(
        "product.message",
        json!({"action":"move","messageId":"two","before":"one"}),
    )
    .await;
    let path = f.state.join("product").join(format!("{}.json", f.id));
    let product = || serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(product()["messages"][0]["id"], "two");
    assert_eq!(product()["messages"].as_array().unwrap().len(), 3);
    assert_eq!(
        f.call(
            "product.message",
            json!({"action":"move","messageId":"two","before":"foreign"})
        )
        .await["code"],
        "unknown_message"
    );
    assert_eq!(
        f.call(
            "product.message",
            json!({"action":"add","message":{"id":"one","text":"changed","delivery":"queued"}})
        )
        .await["code"],
        "payload_conflict"
    );
    f.ok(
        "product.message",
        json!({"action":"edit","messageId":"two","text":"edited"}),
    )
    .await;
    f.ok(
        "product.message",
        json!({"action":"remove","messageId":"one"}),
    )
    .await;
    let comment = json!({"id":"c","path":"a.txt","line":2,"text":"Please fix","sent":false});
    f.ok(
        "product.comment",
        json!({"action":"add","taskId":"task","comment":comment}),
    )
    .await;
    f.ok(
        "product.comment",
        json!({"action":"mark_sent","taskId":"task","ids":["c"]}),
    )
    .await;
    assert_eq!(
        f.call(
            "product.comment",
            json!({"action":"edit","taskId":"task","commentId":"c","text":"no"})
        )
        .await["code"],
        "already_sent"
    );
    let sp = f
        .ok(
            "product.sprint",
            json!({"action":"create","description":"Plan work"}),
        )
        .await["sprintId"]
        .clone();
    f.ok(
        "product.sprint",
        json!({"action":"note","sprintId":sp,"text":"note"}),
    )
    .await;
    // A planner question keeps its choices as selectable options (Python parity).
    f.ok(
        "product.sprint",
        json!({"action":"note","sprintId":sp,"kind":"question","text":"Which database?","options":["SQLite","Postgres"]}),
    )
    .await;
    for (kind, options) in [
        ("note", json!(["A"])),
        ("question", json!([" "])),
        ("question", json!("A")),
    ] {
        assert_eq!(
            f.call(
                "product.sprint",
                json!({"action":"note","sprintId":sp,"kind":kind,"text":"Q","options":options})
            )
            .await["code"],
            "invalid_options"
        );
    }
    let asked = &product()["sprints"][0]["conversation"][1];
    assert_eq!(asked["kind"], "question");
    assert_eq!(asked["text"], "Which database?");
    assert_eq!(asked["options"], json!(["SQLite", "Postgres"]));
    f.ok(
        "product.sprint",
        json!({"action":"patch","sprintId":sp,"patch":{"title":"Plan","queueState":"paused"}}),
    )
    .await;
    assert_eq!(
        f.call(
            "product.sprint",
            json!({"action":"patch","sprintId":sp,"patch":{"unknown":1}})
        )
        .await["code"],
        "invalid_sprint"
    );
    f.ok("git", json!({"action":"draft","message":"saved draft"}))
        .await;
    assert_eq!(f.snapshot().await["checkouts"][0]["draft"], "saved draft");
    f.ok(
        "product.sprint",
        json!({"action":"close","sprintId":sp,"confirmed":true}),
    )
    .await;
    assert_eq!(
        f.call(
            "product.sprint",
            json!({"action":"note","sprintId":sp,"text":"no"})
        )
        .await["code"],
        "sprint_closed"
    );
    assert_eq!(product()["sprints"][0]["title"], "Plan");
    f.stop().await;
}

#[tokio::test]
async fn private_conversations_validate_attachment_bytes_and_canonical_encoding() {
    let f = Fixture::new().await;
    // The upload validates the bytes; a saved comment takes only the reference it answers.
    for data in ["!bad", "YQ=", "YR==", "YQ==junk", "YQ==YQ=="] {
        assert_eq!(
            f.call(
                "product.attachment",
                json!({"action":"upload","files":[{"name":"x.txt","mimeType":"text/plain","data":data}]})
            )
            .await["code"],
            "invalid_attachments"
        );
    }
    let file = json!({"name":"x.txt","mimeType":"text/plain","data":"YQ=="});
    let refs = f
        .ok(
            "product.attachment",
            json!({"action":"upload","files":[file]}),
        )
        .await;
    assert_eq!(
        f.call(
            "product.comment",
            json!({"action":"add","taskId":"t","comment":{"id":"c","text":"x","attachments":[file]}})
        )
        .await["code"],
        "inline_attachment_refused"
    );
    let saved = f
        .ok(
            "product.comment",
            json!({"action":"add","taskId":"t","comment":{"id":"c","text":"x","attachments":refs}}),
        )
        .await;
    assert!(!saved.to_string().contains("\"data\""), "{saved}");
    let body = f
        .ok(
            "product.attachment",
            json!({"action":"read","sha256":saved[0]["attachments"][0]["sha256"]}),
        )
        .await;
    assert_eq!(body["data"], "YQ==");
    f.stop().await;
}

#[tokio::test]
async fn git_receipt_replay_never_repeats_commit_and_different_payload_conflicts() {
    let f = Fixture::new().await;
    f.write("a.txt", b"second");
    let p = json!({"action":"commit","message":"second","requestId":"commit-once"});
    f.action(p.clone()).await;
    let head = f.git(&["rev-parse", "HEAD"]);
    f.write("a.txt", b"third");
    f.action(p.clone()).await;
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fs::read(f.repo.join("a.txt")).unwrap(), b"third");
    assert_eq!(
        f.call(
            "git",
            json!({"action":"commit","message":"different","requestId":"commit-once"})
        )
        .await["code"],
        "payload_conflict"
    );
    for entry in fs::read_dir(f.state.join("operations")).unwrap() {
        let entry = entry.unwrap();
        let data = fs::read_to_string(entry.path()).unwrap();
        let record: Value = serde_json::from_str(&data).unwrap();
        assert_eq!(record["state"], "finished");
        assert!(record.get("params").is_none());
        assert!(record.get("message").is_none());
        assert_eq!(
            fs::metadata(entry.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    f.stop().await;
}
#[tokio::test]
async fn unavailable_repository_is_not_a_plain_folder_and_loose_untracked_overflow_is_explicit() {
    let f = Fixture::new().await;
    for i in 0..2001 {
        f.write(&format!("loose-{i}"), b"x");
    }
    assert_eq!(
        f.call("repo.snapshot", json!({})).await["code"],
        "repo_too_large"
    );
    fs::write(f.repo.join(".git/HEAD"), b"invalid HEAD").unwrap();
    assert_eq!(
        f.call("repo.snapshot", json!({})).await["code"],
        "git_failed"
    );
    f.stop().await;
}
#[tokio::test]
async fn snapshot_gets_sqlite_only_task_baselines() {
    let f = Fixture::new().await;
    let head = f.git(&["rev-parse", "HEAD"]);
    let store = f.store();
    store.write(|db| {
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES ('task',?,'Fixture',?)",
            (&f.id,cortex_core::now()))?;
        db.execute("INSERT INTO workspace_task_options VALUES ('task',?)",
            [json!({"checkout":{"kind":"main"},"baseRevision":head}).to_string()])?;
        Ok(())
    }).unwrap();
    f.write("a.txt", b"changed\n");
    let snapshot = f.snapshot().await;
    assert_eq!(snapshot["taskDiffs"]["task"]["files"][0]["path"], "a.txt");
    assert!(
        !f.state
            .join("product")
            .join(format!("{}.json", f.id))
            .exists(),
        "Core task metadata must not need a duplicate private JSON record"
    );
    f.stop().await;
}
#[tokio::test]
async fn core_context_editor_and_attachments_work() {
    let f = Fixture::new().await;
    let store = f.store();
    store.write(|db| {
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES ('task',?,'Fixture',?)",
            (&f.id,cortex_core::now()))?;
        db.execute("INSERT INTO workspace_task_options VALUES ('task',?)",
            [json!({"checkout":{"kind":"main"},"draftTitle":"Saved title","settings":{"mode":"guarded"},
                "attachments":[{"name":"a.txt","mimeType":"text/plain","data":"YQ=="}]}).to_string()])?;
        Ok(())
    }).unwrap();
    let product = f.ok("repo.context", json!({})).await;
    assert_eq!(product["tasks"]["task"]["title"], "Saved title");
    assert_eq!(product["tasks"]["task"]["checkout"]["branch"], "main");
    assert_eq!(
        f.ok("product.read", json!({"attachments":true,"taskId":"task"}))
            .await[0]["sha256"],
        "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb"
    );
    assert_eq!(
        f.call(
            "product.read",
            json!({"attachments":true,"taskId":"missing"})
        )
        .await["code"],
        "unknown_task"
    );
    let base = f.ok("repo.file", json!({"file":"a.txt"})).await["revision"].clone();
    cortex_core::lifecycle::TaskMachine::new(&store)
        .create("runtime-holder", "Scoped holder", "owner").unwrap();
    store.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='runtime-holder'",[])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('runtime-holder',1,'write','worker-backend-dev','Write','[]','[\"a.txt\"]','execution','running','active-assignment',NULL)",[])?;
        Ok(())
    }).unwrap();
    let busy = f.call("repo.file.lease.acquire", json!({"file":"a.txt"})).await;
    assert_eq!(busy["code"], "task_file_lease_held");
    assert_eq!(busy["holderTaskId"], "runtime-holder");
    assert_eq!(f.ok("repo.file", json!({"file":"a.txt"})).await["text"], "one\ntwo\n");
    // An unrelated file can be edited without stopping the task.
    assert!(f.lease("other.txt").await.is_string());
    assert_eq!(fs::read(f.repo.join("a.txt")).unwrap(), b"one\ntwo\n");
    store.write(|db| {
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id='runtime-holder'", [])?;
        Ok(())
    }).unwrap();
    let token = f.lease("a.txt").await;
    assert_eq!(
        f.ok("repo.file.save", json!({"file":"a.txt","text":"saved","revision":base,"leaseToken":token}))
            .await["outcome"],
        "saved"
    );
    drop(store);
    f.stop().await;
}

#[tokio::test]
async fn core_context_reads_do_not_initialize_and_overview_resolves_linked_store() {
    let f = Fixture::new().await;
    let key = cortex_core::ProjectKey::new(
        &f.repo,
        if workspace_protocol::channel::current().is_dev() {
            cortex_core::StoreChannel::Dev
        } else {
            cortex_core::StoreChannel::Release
        },
    )
    .unwrap();
    let entries = || {
        let mut paths = fs::read_dir(key.directory())
            .into_iter()
            .flatten()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    };
    let before = entries();
    assert!(!key.database().exists());
    f.ok("repo.context", json!({})).await;
    assert!(
        !key.database().exists(),
        "A context read must not initialize a missing database"
    );
    assert_eq!(
        entries(),
        before,
        "A context read must not create locks or sidecars"
    );
    let linked = f.base.join("linked");
    f.git(&["worktree", "add", "--detach", linked.to_str().unwrap()]);
    f.service.register(&linked, "Linked").await.unwrap();
    fs::create_dir_all(key.directory()).unwrap();
    fs::set_permissions(key.directory(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(key.directory().join("fixture.bin"), vec![0u8; 1048576]).unwrap();
    let overview = f.ok("overview", json!({})).await;
    assert!(
        overview["storage"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["label"] == "Linked" && s["mb"].as_f64().unwrap_or(0.0) >= 1.0),
        "{overview}"
    );
    assert!(!key.database().exists());
    f.stop().await;
}

#[tokio::test]
async fn core_context_rejects_unreadable_store_instead_of_assuming_no_writer() {
    let f = Fixture::new().await;
    let store = f.store();
    let database = store.key().database();
    drop(store);
    fs::write(&database, b"not a database").unwrap();
    for op in [
        "repo.context",
        "repo.snapshot",
        "repo.file",
        "repo.file.save",
    ] {
        let reply = f
            .call(op, json!({"file":"a.txt","text":"unsafe","revision":null}))
            .await;
        assert_eq!(reply["ok"], false, "{op}: {reply}");
    }
    assert_eq!(fs::read(f.repo.join("a.txt")).unwrap(), b"one\ntwo\n");
    f.stop().await;
}

/// Every repository read starts Git with the channel's private settings folders. One
/// created group-writable by `mkdir -p` under a 002 umask is made private instead of
/// failing each read with a generic filesystem error; a folder no check can trust is
/// named in the refusal, and repository reads recover in the same service once the
/// owner fixes it.
#[tokio::test]
async fn repository_reads_repair_or_name_an_unsafe_private_settings_folder() {
    let f = Fixture::new().await;
    if !workspace_protocol::channel::current().is_dev() {
        f.stop().await;
        return;
    }
    f.snapshot().await;
    let settings = f.state.join("config");
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&settings), 0o700);
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o775)).unwrap();
    f.ok("repo.context", json!({})).await;
    f.snapshot().await;
    assert_eq!(mode(&settings), 0o700);
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o777)).unwrap();
    for op in ["repo.context", "repo.snapshot"] {
        let reply = f.call(op, json!({})).await;
        assert_eq!(reply["ok"], false, "{op}: {reply}");
        assert_eq!(reply["code"], "private_directory_unsafe", "{op}: {reply}");
        let message = reply["message"].as_str().unwrap();
        assert!(message.contains(settings.to_str().unwrap()), "{message}");
        assert!(message.contains("writable by every account"), "{message}");
    }
    assert_eq!(mode(&settings), 0o777);
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o700)).unwrap();
    f.snapshot().await;
    f.stop().await;
}

#[tokio::test]
async fn automatic_archive_never_asks_and_keeps_the_work() {
    let f = Fixture::new().await;
    f.ok(
        "product.sprint",
        json!({"action":"create","description":"fixture"}),
    )
    .await;
    let file = f.state.join("product").join(format!("{}.json", f.id));
    let mut value: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    // One unmerged task, and one that still carries the retired question fields.
    value["tasks"]["work"] = json!({"status":"done","work":"unmerged","doneAt":1});
    value["tasks"]["stale"] = json!({"status":"done","work":"unmerged","doneAt":1,
        "archiveQuestion":true,"archiveAskAfter":5});
    fs::write(&file, value.to_string()).unwrap();
    let cleanup = f.ok("cleanup.run", json!({})).await;
    assert_eq!(cleanup["asked"], 0);
    assert_eq!(cleanup["removed"], 0);
    let value: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    for task in ["work", "stale"] {
        let t = &value["tasks"][task];
        assert!(t["archive"]["at"].is_u64(), "{task}: {t}");
        assert_ne!(t["archiveQuestion"], true, "{task}: {t}");
        assert!(t.get("archiveAskAfter").is_none(), "{task}: {t}");
        assert_eq!(t["work"], "unmerged", "the work record is untouched");
    }
    f.stop().await;
}
/// Names of one `repo.list` folder reply, with each entry's ignored mark.
fn listed(folder: &Value) -> Vec<(String, Value)> {
    folder["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("{folder}"))
        .iter()
        .map(|e| (e["name"].as_str().unwrap().to_owned(), e["ignored"].clone()))
        .collect()
}
#[tokio::test]
async fn code_tree_lists_ignored_and_dot_files_one_folder_level_at_a_time() {
    let f = Fixture::new().await;
    // The project store exists inside the folder, as it does for every activated project.
    drop(f.store());
    assert!(f.repo.join(".cortex").is_dir() || f.repo.join(".cortex-dev").is_dir());
    f.write(".gitignore", b"target/\n.env\n");
    f.git(&["add", ".gitignore"]);
    f.git(&["commit", "-m", "ignore"]);
    f.write(".env", b"API_KEY=fake-fixture-value\n");
    f.write(".env.local", b"untracked\n");
    f.write("target/debug/deep/x.o", b"obj");
    f.write(".hidden/notes.md", b"hi");
    f.write("sub/.git/config", b"[core]\n");
    f.write("sub/kept.txt", b"kept");
    f.write(".cortex-workspace.json", b"{}");
    for i in 0..600 {
        f.write(&format!("target/many/{i:04}"), b"");
    }
    fs::create_dir(f.base.join("outside")).unwrap();
    fs::write(f.base.join("outside/secret"), b"secret").unwrap();
    symlink(f.base.join("outside"), f.repo.join("dir-link")).unwrap();
    // The snapshot tree and the Git page stay what Git reports: nothing ignored is in them.
    let s = f.snapshot().await;
    let tree = s["tree"].as_array().unwrap();
    assert!(
        tree.iter().all(|p| {
            let p = p.as_str().unwrap();
            p != ".env" && !p.starts_with("target/")
        }),
        "{tree:?}"
    );
    let changed = s["checkouts"][0]["files"].as_array().unwrap();
    assert!(
        changed
            .iter()
            .all(|c| c["path"] != ".env" && !c["path"].as_str().unwrap().starts_with("target")),
        "{changed:?}"
    );
    // The checkout's own folder: dot-files and ignored entries alike, marked; one level only.
    let r = f
        .ok("repo.list", json!({"dirs":[""],"overview":true}))
        .await;
    let root = listed(&r["folders"][0]);
    let names: Vec<_> = root.iter().map(|(n, _)| n.as_str()).collect();
    for want in [
        ".env",
        ".env.local",
        ".gitignore",
        ".hidden",
        "a.txt",
        "dir-link",
        "sub",
        "target",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    for private in [".git", ".cortex", ".cortex-dev", ".cortex-workspace.json"] {
        assert!(!names.contains(&private), "{private} listed");
    }
    assert!(!names.iter().any(|n| n.contains('/')), "{names:?}");
    let mark = |name: &str| root.iter().find(|(n, _)| n == name).unwrap().1.clone();
    assert_eq!(mark(".env"), true);
    assert_eq!(mark("target"), true);
    assert_eq!(mark(".env.local"), false);
    assert_eq!(mark("a.txt"), false);
    let entry = |name: &str| {
        r["folders"][0]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(
        (
            entry("target")["dir"].clone(),
            entry("dir-link")["dir"].clone(),
            entry("dir-link")["link"].clone()
        ),
        (json!(true), json!(false), json!(true))
    );
    assert_eq!(r["folders"][0]["more"], 0);
    // The overview names what the snapshot leaves out, collapsed: the ignored folder, not its files.
    let overview: Vec<_> = r["overview"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        overview.contains(&".env".into()) && overview.contains(&"target".into()),
        "{overview:?}"
    );
    assert!(
        overview.iter().all(|p| !p.starts_with("target/")
            && !p.starts_with(".cortex")
            && !p.starts_with(".git/")),
        "{overview:?}"
    );
    // A folder read is one level: `target` shows `debug` and `many`, never what is below them.
    let r = f
        .ok(
            "repo.list",
            json!({"dirs":["target", "target/debug", "sub"]}),
        )
        .await;
    assert_eq!(
        listed(&r["folders"][0]),
        vec![("debug".into(), json!(true)), ("many".into(), json!(true))]
    );
    assert_eq!(listed(&r["folders"][1]), vec![("deep".into(), json!(true))]);
    assert_eq!(
        listed(&r["folders"][2]),
        vec![("kept.txt".into(), json!(false))],
        "a nested repository's .git stays out"
    );
    assert_eq!(r["overview"], Value::Null);
    // Git cannot judge a path inside a submodule: that folder's marks are unknown, the others' are kept.
    let head = f.git(&["rev-parse", "HEAD"]);
    f.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},module"),
    ]);
    f.write("module/inner.txt", b"x");
    let r = f
        .ok("repo.list", json!({"dirs":["module", "target"]}))
        .await;
    assert_eq!(
        listed(&r["folders"][0]),
        vec![("inner.txt".into(), Value::Null)]
    );
    assert_eq!(
        listed(&r["folders"][1]),
        vec![("debug".into(), json!(true)), ("many".into(), json!(true))]
    );
    f.git(&["rm", "-q", "-f", "--cached", "module"]);
    // Bounded: the first 500 names, and how many more there are.
    let r = f.ok("repo.list", json!({"dirs":["target/many"]})).await;
    assert_eq!(r["folders"][0]["entries"].as_array().unwrap().len(), 500);
    assert_eq!(
        (
            r["folders"][0]["more"].clone(),
            r["folders"][0]["exact"].clone()
        ),
        (json!(100), json!(true))
    );
    assert_eq!(r["folders"][0]["entries"][0]["name"], "0000");
    let many: Vec<String> = (0..33).map(|i| format!("d{i}")).collect();
    assert_eq!(
        f.call("repo.list", json!({"dirs":many})).await["code"],
        "invalid_request"
    );
    // Nothing outside the folder or private is read: escapes and private names are refused,
    // and a linked folder is not followed.
    for bad in [
        "..",
        "../outside",
        "/etc",
        "a/../..",
        ".git",
        "sub/.git",
        ".cortex",
        ".cortex-dev",
    ] {
        assert_eq!(
            f.call("repo.list", json!({"dirs":[bad]})).await["code"],
            "invalid_path",
            "{bad}"
        );
    }
    let r = f
        .ok("repo.list", json!({"dirs":["dir-link", "gone"]}))
        .await;
    assert_eq!(r["folders"][0]["code"], "symlink_refused");
    assert!(r["folders"][0].get("entries").is_none());
    assert_eq!(r["folders"][1]["code"], "folder_missing");
    // `.env` opens and saves like any other file, with the same hash check.
    let read = f.ok("repo.file", json!({"file":".env"})).await;
    assert_eq!(read["text"], "API_KEY=fake-fixture-value\n");
    assert_eq!(read["readOnly"], Value::Null);
    let token = f.lease(".env").await;
    let stale = f
        .call(
            "repo.file.save",
            json!({"file":".env","text":"x","revision":"0".repeat(64),"leaseToken":token}),
        )
        .await;
    assert_eq!(stale["result"]["outcome"], "conflict");
    let saved = f
        .ok(
            "repo.file.save",
            json!({"file":".env","text":"API_KEY=other-fake\n","revision":read["revision"],"leaseToken":token}),
        )
        .await;
    assert_eq!(saved["outcome"], "saved");
    assert_eq!(
        fs::read(f.repo.join(".env")).unwrap(),
        b"API_KEY=other-fake\n"
    );
    assert_eq!(
        f.ok("repo.file", json!({"file":"target/debug/deep/x.o"}))
            .await["text"],
        "obj"
    );
    // Cortex's private names stay refused for reads and saves.
    for private in [".cortex-workspace.json", ".git/config", "sub/.git/config"] {
        assert_eq!(
            f.call("repo.file", json!({"file":private})).await["code"],
            "invalid_path",
            "{private}"
        );
        assert_eq!(
            f.call(
                "repo.file.save",
                json!({"file":private,"text":"x","revision":null})
            )
            .await["code"],
            "invalid_path",
            "{private}"
        );
    }
    f.stop().await;
}
#[tokio::test]
async fn code_tree_of_a_folder_without_git_lists_its_unwalked_folders_on_demand() {
    let f = Fixture::new().await;
    fs::remove_dir_all(f.repo.join(".git")).unwrap();
    f.write(".env", b"X=fake\n");
    f.write("node_modules/pkg/index.js", b"x");
    f.write("app/__pycache__/m.pyc", b"x");
    let s = f.snapshot().await;
    assert!(s["tree"].as_array().unwrap().contains(&json!(".env")));
    let r = f
        .ok(
            "repo.list",
            json!({"dirs":["", "node_modules"],"overview":true}),
        )
        .await;
    let overview: Vec<_> = r["overview"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["path"].as_str().unwrap().to_owned(), e["ignored"].clone()))
        .collect();
    assert_eq!(
        overview,
        vec![
            ("app/__pycache__".into(), json!(true)),
            ("node_modules".into(), json!(true))
        ]
    );
    let root = listed(&r["folders"][0]);
    assert!(
        root.contains(&(".env".into(), json!(false)))
            && root.contains(&("node_modules".into(), json!(true))),
        "{root:?}"
    );
    assert_eq!(listed(&r["folders"][1]), vec![("pkg".into(), json!(true))]);
    f.stop().await;
}
