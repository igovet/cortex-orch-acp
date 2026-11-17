//! Native integration paths against real Git repositories in temporary directories.
use super::*;
use crate::machine::settings;
use cortex_core::{OpenMode, Store};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, process::Command};

const REPORT: &str = "# Offline reference\n\nCompleted work evidence.\n";
const REPORT_DIGEST: &str = "75823ac83556de6d2c89fac435d243bbf7303a601c4d352687c07e366e82840c";

struct Env {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
impl Env {
    /// main: a.txt=base. Branch `task` adds b.txt and changes a.txt; root is on `task`.
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir()
            .join(format!("cw-native-int-{name}-{}", settings::unique()))
            .canonicalize_or_self();
        let root = base.join("repo");
        let state = base.join("state");
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Fixture"),
            ("user.email", "fixture@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            git(&root, &["config", k, v]);
        }
        fs::write(root.join(".gitignore"), ".cortex/\n.cortex-dev/\n").unwrap();
        fs::write(root.join("a.txt"), "base\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "Base"]);
        git(&root, &["checkout", "-q", "-b", "task"]);
        fs::write(root.join("b.txt"), "task file\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "Task"]);
        Self { base, root, state }
    }
    fn base_revision(&self) -> String {
        git(&self.root, &["rev-parse", "main"])
    }
    /// Completed, verified runtime evidence bound to the current project folder digest.
    async fn evidence(&self, receipts: &[(&str, &str)]) -> String {
        // The project folder digest of a task whose verification predates the native runtime.
        let digest = crate::machine::legacy_source::capture(&self.root)
            .unwrap()
            .expect("observable project folder");
        let key = context::key(&self.root).unwrap();
        let store = Store::open(key, OpenMode::Initialize).unwrap();
        let root = self.root.to_str().unwrap().to_owned();
        let report = store.key().directory().join("r1/result.md");
        fs::create_dir_all(report.parent().unwrap()).unwrap();
        fs::write(&report, REPORT).unwrap();
        store.write(|db| {
            db.execute("INSERT INTO workspace_projects VALUES ('p1',?,'fixture','now',1)", [&root])?;
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('t1','p1','Build task',1,'e1','now')", [])?;
            db.execute("INSERT INTO tasks VALUES ('r1',?,'now')", [&root])?;
            db.execute("INSERT INTO runtime_tasks(task_id,state,goal,synthesis,created_at,updated_at) VALUES ('r1','COMPLETED','Goal','Answer','now','now')", [])?;
            db.execute("INSERT INTO task_owners VALUES ('r1','owner',1,'now')", [])?;
            db.execute("INSERT INTO acp_host_sessions VALUES ('s1',?,'r1','{}','{}','[]','now',NULL)", [&root])?;
            db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',1,1,'now')", [])?;
            db.execute("INSERT INTO reports VALUES ('report1','r1','report','result.md','now')", [])?;
            let node = format!("{{\"report_ref\": \"report1\", \"report_digest\": \"{REPORT_DIGEST}\"}}");
            let review = format!("{{\"report_ref\": \"report1\", \"report_digest\": \"{REPORT_DIGEST}\", \"verdict\": \"APPROVE\"}}");
            db.execute("INSERT INTO runtime_nodes VALUES ('r1',0,'n0','worker-backend-dev','Objective','[]','[]','execution','completed','a0',?)", [&node])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('r1',0,'n1','worker-backend-dev','Objective','[]','[]','review','completed','a1',?)", [&review])?;
            db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','VERIFICATION_PASSED',?,'runtime','now')",
                [json!({"revision":0,"revision_digest":digest,"checks":[]}).to_string()])?;
            if !receipts.is_empty() {
                db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,owner_epoch,issued_at) VALUES ('c1','h','r1','p','worker',1,'now')", [])?;
                for (path, sha) in receipts {
                    db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','TOOL_COMPLETED',?,'c1','2026-01-01T00:00:00Z')",
                        [json!({"operation":"project_write","path":path,"result":{"full_content_sha256":sha}}).to_string()])?;
                }
            }
            Ok(())
        }).unwrap();
        // The store directory is helper data; a project folder digest must not move.
        assert_eq!(
            crate::machine::legacy_source::capture(&self.root)
                .unwrap()
                .as_deref(),
            Some(digest.as_str()),
            "evidence setup changed the project folder digest"
        );
        digest
    }
    fn save(&self, record: Value) {
        let mut tasks = serde_json::Map::new();
        tasks.insert("t1".into(), record);
        let value = json!({"tasks":tasks,"sprints":[],"drafts":{},"messages":[],"comments":{}});
        settings::write_json(&product_path(&self.state, "p1").unwrap(), &value).unwrap();
    }
    fn overlay(&self) -> Value {
        super::super::native::product(&self.state, "p1").unwrap()["tasks"]["t1"].clone()
    }
    async fn run(&self, params: Value) -> Result<Value> {
        let mut g = Git::new();
        super::run(&self.root, "p1", &self.state, &params, &mut g).await
    }
}
trait CanonicalOrSelf {
    fn canonicalize_or_self(self) -> PathBuf;
}
impl CanonicalOrSelf for PathBuf {
    fn canonicalize_or_self(self) -> PathBuf {
        fs::create_dir_all(&self).unwrap();
        self.canonicalize().unwrap()
    }
}
fn record(env: &Env, digest: &str, extra: Value) -> Value {
    let mut r = json!({"status":"ready_to_merge","acceptance":{"digest":digest},
        "checkout":{"kind":"main"},"targetBranch":"main","baseRevision":env.base_revision()});
    for (k, v) in extra.as_object().unwrap() {
        r[k] = v.clone();
    }
    r
}
fn squash(digest: &str) -> Value {
    json!({"confirmed":true,"taskId":"t1","digest":digest,"method":"squash","message":"Integrate completed task","requestId":"one"})
}
fn assert_clean(dir: &Path) {
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn uncommitted_source_is_committed_then_squashed_and_is_not_repeated() {
    let env = Env::new("dirty");
    let target = env.base.join("target");
    git(
        &env.root,
        &["worktree", "add", target.to_str().unwrap(), "main"],
    );
    fs::write(env.root.join("a.txt"), "edited by task\n").unwrap();
    fs::write(env.root.join("c.txt"), "new file\n").unwrap();
    let digest = env.evidence(&[]).await;
    env.save(record(&env, &digest, json!({})));
    assert_eq!(
        env.run(squash(&digest)).await.unwrap(),
        json!({"done":true})
    );
    // The task's own dirty files became a source commit first; main got one squash commit.
    assert_eq!(
        git(&env.root, &["log", "-1", "--format=%s", "task"]),
        "Integrate completed task"
    );
    assert_clean(&env.root);
    assert_eq!(
        git(&target, &["log", "-1", "--format=%s"]),
        "Integrate completed task"
    );
    assert_eq!(git(&target, &["rev-list", "--count", "main"]), "2");
    for (file, text) in [
        ("a.txt", "edited by task\n"),
        ("b.txt", "task file\n"),
        ("c.txt", "new file\n"),
    ] {
        assert_eq!(fs::read_to_string(target.join(file)).unwrap(), text);
    }
    assert_clean(&target);
    let saved = env.overlay();
    assert_eq!(saved["status"], "done");
    assert_eq!(
        saved["integration"]["sourceHead"],
        git(&env.root, &["rev-parse", "task"])
    );
    let head = git(&target, &["rev-parse", "main"]);
    // The task is done: a repeat is stale and Git is not touched again.
    assert_eq!(
        env.run(squash(&digest)).await.unwrap_err().code,
        "stale_result"
    );
    assert_eq!(git(&target, &["rev-parse", "main"]), head);
}

#[tokio::test]
async fn target_that_is_not_checked_out_uses_a_temporary_worktree_that_is_removed() {
    let env = Env::new("temporary");
    git(&env.root, &["branch", "release", "main"]);
    let digest = env.evidence(&[]).await;
    env.save(record(&env, &digest, json!({"targetBranch":"release"})));
    assert_eq!(
        env.run(squash(&digest)).await.unwrap(),
        json!({"done":true})
    );
    assert_eq!(
        git(&env.root, &["log", "-1", "--format=%s", "release"]),
        "Integrate completed task"
    );
    assert_eq!(git(&env.root, &["show", "release:b.txt"]), "task file");
    assert_eq!(git(&env.root, &["rev-parse", "main"]), env.base_revision());
    assert!(!env.state.join("merge-targets/p1/t1").exists());
    assert_eq!(
        git(&env.root, &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
    assert_eq!(env.overlay()["integration"]["targetBranch"], "release");
}

#[tokio::test]
async fn separate_task_worktree_is_merged_and_deleted_only_on_request() {
    for (delete, expected) in [(true, "removed"), (false, "")] {
        let env = Env::new("worktree");
        // Move the task branch into its own checkout; the project folder returns to main.
        git(&env.root, &["checkout", "-q", "main"]);
        let wt = env.base.join("task-wt");
        git(
            &env.root,
            &["worktree", "add", wt.to_str().unwrap(), "task"],
        );
        fs::write(wt.join("d.txt"), "uncommitted in worktree\n").unwrap();
        let digest = env.evidence(&[]).await;
        let checkout = json!({"kind":"worktree","path":wt.to_str().unwrap(),"branch":"task"});
        env.save(record(&env, &digest, json!({"checkout":checkout})));
        let mut params = squash(&digest);
        params["deleteWorktree"] = json!(delete);
        assert_eq!(env.run(params).await.unwrap(), json!({"done":true}));
        assert_eq!(
            git(&env.root, &["show", "main:d.txt"]),
            "uncommitted in worktree"
        );
        assert_eq!(git(&env.root, &["show", "main:b.txt"]), "task file");
        assert_clean(&env.root);
        let saved = env.overlay();
        assert_eq!(saved["status"], "done");
        if delete {
            assert_eq!(saved["integration"]["worktreeCleanup"], expected);
            assert!(!wt.exists());
        } else {
            assert!(saved["integration"].get("worktreeCleanup").is_none());
            assert!(wt.exists());
        }
    }
}

#[tokio::test]
async fn newer_target_updates_the_task_branch_and_requires_a_fresh_review() {
    let env = Env::new("moved");
    let target = env.base.join("target");
    git(
        &env.root,
        &["worktree", "add", target.to_str().unwrap(), "main"],
    );
    fs::write(target.join("other.txt"), "main moved\n").unwrap();
    git(&target, &["add", "."]);
    git(&target, &["commit", "-q", "-m", "Main moved"]);
    let main_head = git(&target, &["rev-parse", "main"]);
    let digest = env.evidence(&[]).await;
    env.save(record(&env, &digest, json!({})));
    let error = env.run(squash(&digest)).await.unwrap_err();
    assert_eq!(error.code, "review_required");
    // Main is untouched; the task branch now contains it; acceptance is withdrawn.
    assert_eq!(git(&target, &["rev-parse", "main"]), main_head);
    git(&env.root, &["merge-base", "--is-ancestor", "main", "task"]);
    let saved = env.overlay();
    assert_eq!(saved["acceptance"], Value::Null);
    assert_eq!(saved["automaticReady"], false);
    assert!(saved["requiresFreshResult"]["resultId"].is_string());
    assert_eq!(saved["status"], "ready_to_merge");
    // A retry is stale now, not another merge.
    assert_eq!(
        env.run(squash(&digest)).await.unwrap_err().code,
        "stale_result"
    );
}

#[tokio::test]
async fn conflicting_target_records_the_conflict_in_the_task_checkout_only() {
    let env = Env::new("conflict");
    fs::write(env.root.join("a.txt"), "task side\n").unwrap();
    git(&env.root, &["commit", "-q", "-am", "Task edits a"]);
    let target = env.base.join("target");
    git(
        &env.root,
        &["worktree", "add", target.to_str().unwrap(), "main"],
    );
    fs::write(target.join("a.txt"), "main side\n").unwrap();
    git(&target, &["commit", "-q", "-am", "Main edits a"]);
    let main_head = git(&target, &["rev-parse", "main"]);
    let digest = env.evidence(&[]).await;
    env.save(record(&env, &digest, json!({})));
    let error = env.run(squash(&digest)).await.unwrap_err();
    assert_eq!(error.code, "merge_conflict");
    assert_eq!(git(&target, &["rev-parse", "main"]), main_head);
    assert_clean(&target);
    assert_eq!(
        git(&env.root, &["diff", "--name-only", "--diff-filter=U"]),
        "a.txt"
    );
    let saved = env.overlay();
    assert_eq!(saved["conflict"], "conflict");
    assert_eq!(saved["acceptance"], Value::Null);
    let file = &saved["merge"]["files"][0];
    assert_eq!(file["path"], "a.txt");
    assert_eq!(file["blocks"][0]["ours"]["lines"], json!(["task side"]));
    assert_eq!(file["blocks"][0]["theirs"]["lines"], json!(["main side"]));
    assert_eq!(
        file["blocks"][0]["results"]["both"]["lines"],
        json!(["task side", "main side"])
    );
    assert_eq!(saved["merge"]["into"], "task");
    assert_eq!(saved["merge"]["source"], "main");
    // Acceptance was withdrawn, so a repeat is stale and changes nothing.
    assert_eq!(
        env.run(squash(&digest)).await.unwrap_err().code,
        "stale_result"
    );
    assert_eq!(git(&target, &["rev-parse", "main"]), main_head);
}

#[tokio::test]
async fn target_equal_to_source_commits_only_the_task_owned_files() {
    let env = Env::new("same");
    git(&env.root, &["checkout", "-q", "main"]);
    git(&env.root, &["branch", "-D", "task"]);
    fs::write(env.root.join("a.txt"), "task change\n").unwrap();
    fs::write(env.root.join("unrelated.txt"), "someone else\n").unwrap();
    let sha = format!("{:x}", Sha256::digest(b"task change\n"));
    let digest = env.evidence(&[("a.txt", &sha)]).await;
    env.save(record(&env, &digest, json!({})));
    // Squash has no branch integration on the shared branch.
    assert_eq!(
        env.run(squash(&digest)).await.unwrap_err().code,
        "same_branch"
    );
    let mut params = squash(&digest);
    params["method"] = json!("commit");
    params["message"] = json!("Task owned change");
    assert_eq!(env.run(params).await.unwrap(), json!({"done":true}));
    assert_eq!(
        git(&env.root, &["log", "-1", "--format=%s"]),
        "Task owned change"
    );
    assert_eq!(
        git(&env.root, &["show", "--name-only", "--format=", "HEAD"]),
        "a.txt"
    );
    // The unrelated file stays untracked and uncommitted.
    assert_eq!(
        git(&env.root, &["status", "--porcelain"]),
        "?? unrelated.txt"
    );
    assert_eq!(env.overlay()["integration"]["method"], "commit");
}

#[tokio::test]
async fn shared_branch_commit_without_task_evidence_is_refused_before_any_effect() {
    let env = Env::new("same-unknown");
    git(&env.root, &["checkout", "-q", "main"]);
    fs::write(env.root.join("a.txt"), "changed\n").unwrap();
    let digest = env.evidence(&[]).await;
    env.save(record(&env, &digest, json!({})));
    let mut params = squash(&digest);
    params["method"] = json!("commit");
    let head = git(&env.root, &["rev-parse", "HEAD"]);
    assert_eq!(
        env.run(params).await.unwrap_err().code,
        "task_content_unknown"
    );
    assert_eq!(git(&env.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&env.root, &["status", "--porcelain"]), "M a.txt");
}

#[tokio::test]
async fn unsupported_method_and_helper_data_in_the_index_are_refused_before_effects() {
    let env = Env::new("refusals");
    let target = env.base.join("target");
    git(
        &env.root,
        &["worktree", "add", target.to_str().unwrap(), "main"],
    );
    let digest = env.evidence(&[]).await;
    env.save(record(&env, &digest, json!({})));
    let mut params = squash(&digest);
    params["method"] = json!("rebase");
    assert_eq!(
        env.run(params).await.unwrap_err().code,
        "unsupported_action"
    );
    params = squash(&digest);
    params["method"] = json!(["squash"]);
    assert_eq!(
        env.run(params).await.unwrap_err().code,
        "unsupported_action"
    );
    // Helper data staged by an older app is never committed. Bind fresh evidence to it.
    let env = Env::new("private");
    let target = env.base.join("target");
    git(
        &env.root,
        &["worktree", "add", target.to_str().unwrap(), "main"],
    );
    fs::write(env.root.join(".cortex-workspace.json"), "{}").unwrap();
    git(&env.root, &["add", "-f", ".cortex-workspace.json"]);
    let digest = env.evidence(&[]).await;
    env.save(record(&env, &digest, json!({})));
    let before = git(&env.root, &["rev-parse", "task"]);
    assert_eq!(
        env.run(squash(&digest)).await.unwrap_err().code,
        "private_index"
    );
    assert_eq!(git(&env.root, &["rev-parse", "task"]), before);
    assert_eq!(
        git(&env.root, &["diff", "--cached", "--name-only"]),
        ".cortex-workspace.json"
    );
    assert_eq!(git(&target, &["rev-parse", "main"]), env.base_revision());
}
#[tokio::test]
async fn pull_request_requires_a_pushed_branch_and_records_one_attempt() {
    let env = Env::new("pr");
    let bin = env.base.join("bin");
    fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            env.base.join("gh-args").display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
    let remote = env.base.join("remote.git");
    git(
        &env.base,
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(
        &env.root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let digest = env.evidence(&[]).await;
    env.save(record(&env, &digest, json!({})));
    let mut params = squash(&digest);
    params["method"] = json!("pr");
    params["body"] = json!("Body text");
    params["draft"] = json!(true);
    let old = std::env::var_os("PATH").unwrap();
    let mut paths = vec![bin.clone()];
    paths.extend(std::env::split_paths(&old));
    std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
    // Not pushed: refused before the attempt is recorded.
    let error = env.run(params.clone()).await.unwrap_err();
    assert_eq!(error.code, "push_required");
    assert!(env.overlay().get("prAttempt").is_none());
    git(&env.root, &["push", "-q", "-u", "origin", "task"]);
    params["requestId"] = json!("two");
    assert_eq!(env.run(params.clone()).await.unwrap(), json!({"done":true}));
    std::env::set_var("PATH", old);
    let args = fs::read_to_string(env.base.join("gh-args")).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "pr",
            "create",
            "--base",
            "main",
            "--head",
            "task",
            "--title",
            "Integrate completed task",
            "--body",
            "Body text",
            "--draft"
        ]
    );
    let saved = env.overlay();
    assert_eq!(saved["integration"]["method"], "pr");
    assert_eq!(saved["integration"]["title"], "Integrate completed task");
    assert_eq!(saved["prAttempt"]["state"], "unknown");
    // Status is not "done": a pull request only starts review.
    assert_eq!(saved["status"], "ready_to_merge");
}
#[tokio::test]
async fn private_index_sees_non_ascii_helper_names() {
    // Git quotes non-ASCII names by default; a quoted name must not hide helper data.
    let env = Env::new("quoted");
    fs::create_dir_all(env.root.join(".cortex")).unwrap();
    fs::write(env.root.join(".cortex").join("é.json"), "{}").unwrap();
    git(&env.root, &["add", "-f", ".cortex/é.json"]);
    assert!(git(&env.root, &["diff", "--cached", "--name-only"]).starts_with('"'));
    let mut g = Git::new();
    assert_eq!(
        effects::private_index(&env.root, &mut g)
            .await
            .unwrap_err()
            .code,
        "private_index"
    );
}
