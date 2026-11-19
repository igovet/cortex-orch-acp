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
    assert_eq!(env.overlay()["changesBaseRevision"], main_head);
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
    let task_head = git(&env.root, &["rev-parse", "HEAD"]);
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
    assert_eq!(saved["merge"]["sourceRevision"], task_head);
    assert_eq!(saved["merge"]["targetRevision"], main_head);
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
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho 'Creating pull request for task into main'\necho 'https://github.com/owner/repo/pull/7'\n",
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
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
    );
    // Pushes go to the local bare repository; nothing contacts the remote host.
    git(
        &env.root,
        &[
            "remote",
            "set-url",
            "--push",
            "origin",
            remote.to_str().unwrap(),
        ],
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
    // The address gh printed is recorded, because it is https on the remote's own host.
    assert_eq!(
        saved["integration"]["url"],
        "https://github.com/owner/repo/pull/7"
    );
    let mut g = Git::new();
    let found = pull_request_address(
        &env.root,
        "p1",
        &env.state,
        &json!({"pullRequest":"t1"}),
        &mut g,
    )
    .await
    .unwrap();
    assert_eq!(found, json!({"url":"https://github.com/owner/repo/pull/7"}));
    // Checked again when it is opened: a remote moved to another host no longer vouches for it.
    git(
        &env.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.example.invalid:owner/repo.git",
        ],
    );
    let refused = pull_request_address(
        &env.root,
        "p1",
        &env.state,
        &json!({"pullRequest":"t1"}),
        &mut g,
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, "pull_request_unsafe");
    // A task without a recorded pull request has nothing to open.
    let mut saved = super::super::native::product(&env.state, "p1").unwrap();
    saved["tasks"]["t1"]["integration"]
        .as_object_mut()
        .unwrap()
        .remove("url");
    settings::write_json(&product_path(&env.state, "p1").unwrap(), &saved).unwrap();
    assert_eq!(
        pull_request_address(
            &env.root,
            "p1",
            &env.state,
            &json!({"pullRequest":"t1"}),
            &mut g
        )
        .await
        .unwrap_err()
        .code,
        "pull_request_unknown"
    );
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

/// One Git-changing integration per repository: a second one, of another task, is
/// refused at once while the first is in Git, and the first completes correctly.
#[tokio::test]
async fn a_second_integration_in_the_same_repository_is_refused_at_once() {
    let env = Env::new("serial");
    let target = env.base.join("target");
    git(
        &env.root,
        &["worktree", "add", target.to_str().unwrap(), "main"],
    );
    // A slow commit hook keeps the first integration inside Git.
    let hooks = env.base.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    fs::write(hooks.join("pre-commit"), "#!/bin/sh\nsleep 2\n").unwrap();
    fs::set_permissions(
        hooks.join("pre-commit"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    git(
        &env.root,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    fs::write(env.root.join("a.txt"), "edited by task\n").unwrap();
    let digest = env.evidence(&[]).await;
    let mut product = json!({"tasks":{},"sprints":[],"drafts":{},"messages":[],"comments":{}});
    product["tasks"]["t1"] = record(&env, &digest, json!({}));
    product["tasks"]["t2"] = record(&env, &digest, json!({}));
    settings::write_json(&product_path(&env.state, "p1").unwrap(), &product).unwrap();
    let second = async {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        let started = std::time::Instant::now();
        let mut g = Git::new();
        let mut p = squash(&digest);
        p["taskId"] = json!("t2");
        p["requestId"] = json!("two");
        let refused = super::run(&env.root, "p1", &env.state, &p, &mut g)
            .await
            .unwrap_err();
        (refused, started.elapsed())
    };
    let (first, (refused, waited)) = tokio::join!(env.run(squash(&digest)), second);
    assert_eq!(first.unwrap(), json!({"done":true}));
    assert_eq!(
        (refused.code, refused.message.as_str()),
        ("integration_in_progress", REPO_BUSY)
    );
    assert!(waited < std::time::Duration::from_secs(1), "{waited:?}");
    // Exactly one squash landed, with only the first task's change, and nothing is left staged.
    assert_eq!(git(&target, &["rev-list", "--count", "main"]), "2");
    assert_eq!(
        fs::read_to_string(target.join("a.txt")).unwrap(),
        "edited by task\n"
    );
    assert_clean(&target);
    assert_clean(&env.root);
    let saved = super::super::native::product(&env.state, "p1").unwrap();
    assert_eq!(saved["tasks"]["t1"]["status"], "done");
    assert_eq!(saved["tasks"]["t2"]["status"], "ready_to_merge");
}

/// The project folder on main and the task branch in a worktree of its own, with main
/// moved on by `main_file` (another file, or a conflicting a.txt) after the task began.
fn update_env(name: &str, main_file: (&str, &str)) -> (Env, PathBuf) {
    let env = Env::new(name);
    fs::write(env.root.join("a.txt"), "task side\n").unwrap();
    git(&env.root, &["commit", "-q", "-am", "Task edits a"]);
    git(&env.root, &["checkout", "-q", "main"]);
    let wt = env.base.join("task-wt");
    git(
        &env.root,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "task"],
    );
    fs::write(env.root.join(main_file.0), main_file.1).unwrap();
    git(&env.root, &["add", "."]);
    git(&env.root, &["commit", "-q", "-m", "Main moved"]);
    (env, wt)
}
fn task_in(env: &Env, wt: &Path, digest: &str) -> Value {
    record(
        env,
        digest,
        json!({"checkout":{"kind":"worktree","path":wt.to_str().unwrap(),"branch":"task"},
            "baseRevision":git(wt, &["merge-base", "task", "main"])}),
    )
}
async fn assert_task_changes(env: &Env) {
    let snapshot = Git::new()
        .snapshot(&env.root, "p1", &env.state, None)
        .await
        .unwrap();
    let paths: std::collections::BTreeSet<_> = snapshot["taskDiffs"]["t1"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["a.txt", "b.txt"].into_iter().collect());
}
fn update(digest: &str, request: &str) -> Value {
    json!({"confirmed":true,"taskId":"t1","digest":digest,"method":"update","requestId":request})
}

#[tokio::test]
async fn update_from_target_merges_the_target_into_the_task_branch_and_withdraws_the_acceptance() {
    let (env, wt) = update_env("update", ("other.txt", "main moved\n"));
    let main = git(&env.root, &["rev-parse", "main"]);
    let task_head = git(&wt, &["rev-parse", "HEAD"]);
    let digest = env.evidence(&[]).await;
    let record = task_in(&env, &wt, &digest);
    let original = record["baseRevision"].clone();
    env.save(record);
    assert_task_changes(&env).await;
    // Confirmation first, as for every integration.
    let mut unconfirmed = update(&digest, "u0");
    unconfirmed["confirmed"] = json!(false);
    assert_eq!(
        env.run(unconfirmed).await.unwrap_err().code,
        "confirmation_required"
    );
    assert_eq!(
        env.run(update(&digest, "u1")).await.unwrap(),
        json!({"done":true,"updated":true})
    );
    // The target is untouched; the task branch has a merge of it on top of its own commit.
    assert_eq!(git(&env.root, &["rev-parse", "main"]), main);
    assert_eq!(
        git(&wt, &["show", "-s", "--format=%P", "HEAD"]),
        format!("{task_head} {main}")
    );
    assert_eq!(
        fs::read_to_string(wt.join("other.txt")).unwrap(),
        "main moved\n"
    );
    assert_clean(&wt);
    let saved = env.overlay();
    assert_eq!(saved["acceptance"], Value::Null);
    assert_eq!(saved["automaticReady"], false);
    assert!(saved["requiresFreshResult"]["resultId"].is_string());
    assert_eq!(saved["status"], "ready_to_merge");
    assert_eq!(saved["baseRevision"], original);
    assert_eq!(saved["changesBaseRevision"], main);
    assert_task_changes(&env).await;
    // Nothing left to bring in, and the withdrawn acceptance cannot be used to merge.
    assert_eq!(
        env.run(update(&digest, "u2")).await.unwrap_err().code,
        "up_to_date"
    );
    assert_eq!(
        env.run(squash(&digest)).await.unwrap_err().code,
        "stale_result"
    );
}

#[tokio::test]
async fn update_from_target_is_refused_before_any_change_when_it_could_lose_or_race_work() {
    let (env, wt) = update_env("update-refused", ("other.txt", "main moved\n"));
    let head = git(&wt, &["rev-parse", "HEAD"]);
    let digest = env.evidence(&[]).await;
    // Uncommitted changes in the task's worktree: never stashed, never merged over.
    fs::write(wt.join("a.txt"), "uncommitted\n").unwrap();
    env.save(task_in(&env, &wt, &digest));
    let refused = env.run(update(&digest, "d1")).await.unwrap_err();
    assert_eq!(refused.code, "dirty_checkout", "{}", refused.message);
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read_to_string(wt.join("a.txt")).unwrap(),
        "uncommitted\n"
    );
    git(&wt, &["checkout", "-q", "--", "a.txt"]);
    // Not finished yet, or working in the shared project folder.
    let mut working = task_in(&env, &wt, &digest);
    working["status"] = json!("working");
    env.save(working);
    assert_eq!(
        env.run(update(&digest, "d2")).await.unwrap_err().code,
        "not_finished"
    );
    env.save(record(&env, &digest, json!({})));
    assert_eq!(
        env.run(update(&digest, "d3")).await.unwrap_err().code,
        "same_branch"
    );
    // A result other than the one reviewed is stale.
    env.save(task_in(&env, &wt, &digest));
    assert_eq!(
        env.run(update(&"0".repeat(64), "d4"))
            .await
            .unwrap_err()
            .code,
        "stale_result"
    );
    // Workers running again: the run is not complete, so nothing is merged under them.
    let store = Store::open(context::key(&env.root).unwrap(), OpenMode::Initialize).unwrap();
    store
        .write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET state='RUNNING' WHERE task_id='r1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let running = env.run(update(&digest, "d5")).await.unwrap_err();
    assert_eq!(running.code, "result_unavailable");
    assert!(
        running.message.contains("still running"),
        "{}",
        running.message
    );
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
    assert_clean(&wt);
}

#[tokio::test]
async fn an_update_conflict_is_the_tasks_own_and_abort_returns_the_reviewed_tree_exactly() {
    let (env, wt) = update_env("update-conflict", ("a.txt", "main side\n"));
    let main = git(&env.root, &["rev-parse", "main"]);
    let head = git(&wt, &["rev-parse", "HEAD"]);
    let index = git(&wt, &["write-tree"]);
    let digest = env.evidence(&[]).await;
    let mut record = task_in(&env, &wt, &digest);
    let original = record["baseRevision"].clone();
    record["changesBaseRevision"] = original.clone();
    env.save(record);
    let refused = env.run(update(&digest, "c1")).await.unwrap_err();
    assert_eq!(refused.code, "merge_conflict", "{}", refused.message);
    assert!(
        refused.message.contains("1 conflicting file"),
        "{}",
        refused.message
    );
    assert_eq!(git(&env.root, &["rev-parse", "main"]), main);
    let saved = env.overlay();
    assert_eq!(saved["conflict"], "conflict");
    assert_eq!(saved["merge"]["kind"], "update");
    assert_eq!(saved["merge"]["source"], "main");
    assert_eq!(saved["merge"]["into"], "task");
    assert_eq!(saved["merge"]["files"][0]["path"], "a.txt");
    assert!(saved["requiresFreshResult"]["resultId"].is_string());
    // The task's Abort merge puts back the reviewed tree, and with it a current result.
    let mut product = super::super::native::product(&env.state, "p1").unwrap();
    let lock = Lock::new(&env.state, "machine.lock").unwrap();
    let mut g = Git::new();
    crate::machine::task_merge::run(
        &env.root,
        "p1",
        "t1",
        &env.state,
        &mut product,
        &json!({"action":"abort_merge","taskId":"t1","requestId":"c2"}),
        &mut g,
        lock,
    )
    .await
    .unwrap();
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&wt, &["write-tree"]), index);
    assert_clean(&wt);
    let saved = env.overlay();
    assert_eq!(saved["conflict"], "none");
    assert_eq!(saved["merge"], Value::Null);
    assert_eq!(saved["requiresFreshResult"], Value::Null);
    assert_eq!(saved["baseRevision"], original);
    assert_eq!(saved["changesBaseRevision"], original);
    assert_task_changes(&env).await;
}

#[tokio::test]
async fn a_resolved_update_conflict_finishes_as_a_merge_and_needs_a_fresh_result() {
    assert_update_conflict_finishes(false).await;
}

#[tokio::test]
async fn an_externally_committed_update_conflict_finishes_against_the_merged_target() {
    assert_update_conflict_finishes(true).await;
}

async fn assert_update_conflict_finishes(externally_committed: bool) {
    let (env, wt) = update_env("update-finish", ("a.txt", "main side\n"));
    fs::write(env.root.join("other.txt"), "target only\n").unwrap();
    git(&env.root, &["add", "other.txt"]);
    git(&env.root, &["commit", "-q", "-m", "Target adds other file"]);
    let head = git(&wt, &["rev-parse", "HEAD"]);
    let main = git(&env.root, &["rev-parse", "main"]);
    let digest = env.evidence(&[]).await;
    let record = task_in(&env, &wt, &digest);
    let original = record["baseRevision"].clone();
    env.save(record);
    assert_eq!(
        env.run(update(&digest, "f1")).await.unwrap_err().code,
        "merge_conflict"
    );
    assert_eq!(env.overlay()["changesBaseRevision"], Value::Null);
    // Main can advance while this merge is stopped; Finish must use its actual parent.
    fs::write(env.root.join("later.txt"), "not merged\n").unwrap();
    git(&env.root, &["add", "later.txt"]);
    git(&env.root, &["commit", "-q", "-m", "Target moves again"]);
    fs::write(wt.join("a.txt"), "task side\nmain side\n").unwrap();
    git(&wt, &["add", "a.txt"]);
    let mut product = super::super::native::product(&env.state, "p1").unwrap();
    let lock = Lock::new(&env.state, "machine.lock").unwrap();
    let mut g = Git::new();
    let mut mismatched = product.clone();
    mismatched["tasks"]["t1"]["merge"]["targetRevision"] = json!(head);
    let error = crate::machine::task_merge::run(
        &env.root,
        "p1",
        "t1",
        &env.state,
        &mut mismatched,
        &json!({"action":"finish_merge","taskId":"t1","requestId":"wrong-target"}),
        &mut g,
        lock,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "unsafe_conflict");
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
    assert_eq!(env.overlay()["changesBaseRevision"], Value::Null);
    let external_head = externally_committed.then(|| {
        git(&wt, &["commit", "-q", "-m", "Resolve the update in Git"]);
        git(&wt, &["rev-parse", "HEAD"])
    });
    let lock = Lock::new(&env.state, "machine.lock").unwrap();
    crate::machine::task_merge::run(
        &env.root,
        "p1",
        "t1",
        &env.state,
        &mut product,
        &json!({"action":"finish_merge","taskId":"t1","requestId":"f2"}),
        &mut g,
        lock,
    )
    .await
    .unwrap();
    assert_eq!(
        git(&wt, &["show", "-s", "--format=%P", "HEAD"]),
        format!("{head} {main}")
    );
    if let Some(external_head) = external_head {
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), external_head);
    }
    assert_clean(&wt);
    let saved = env.overlay();
    assert_eq!(saved["conflict"], "none");
    assert_eq!(saved["acceptance"], Value::Null);
    assert_eq!(saved["baseRevision"], original);
    assert_eq!(saved["changesBaseRevision"], main);
    assert_task_changes(&env).await;
    // The tree changed: the reviewed result is not current, so it cannot be accepted again as is.
    assert!(saved["requiresFreshResult"]["resultId"].is_string());
}

#[tokio::test]
async fn an_aborted_or_replaced_update_conflict_cannot_advance_the_changes_base() {
    let (env, wt) = update_env("update-external-abort", ("a.txt", "main side\n"));
    let target = git(&env.root, &["rev-parse", "main"]);
    let digest = env.evidence(&[]).await;
    let mut record = task_in(&env, &wt, &digest);
    record["changesBaseRevision"] = record["baseRevision"].clone();
    env.save(record);
    assert_eq!(
        env.run(update(&digest, "stop-update"))
            .await
            .unwrap_err()
            .code,
        "merge_conflict"
    );
    let stopped = env.overlay();
    git(&wt, &["merge", "--abort"]);

    for (request, replaced) in [("aborted-update", false), ("different-merge", true)] {
        if replaced {
            // Same target, but not the stopped update: a different first parent.
            git(
                &wt,
                &["commit", "--allow-empty", "-q", "-m", "New task HEAD"],
            );
            git(&wt, &["merge", "-q", "-s", "ours", "--no-edit", &target]);
        }
        let head = git(&wt, &["rev-parse", "HEAD"]);
        let mut product = super::super::native::product(&env.state, "p1").unwrap();
        let lock = Lock::new(&env.state, "machine.lock").unwrap();
        let error = crate::machine::task_merge::run(
            &env.root,
            "p1",
            "t1",
            &env.state,
            &mut product,
            &json!({"action":"finish_merge","taskId":"t1","requestId":request}),
            &mut Git::new(),
            lock,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "unsafe_conflict");
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
        assert_clean(&wt);
        assert_eq!(env.overlay(), stopped);
    }
}

#[test]
fn automatic_no_effect_finish_requires_complete_landing_facts() {
    // Both certified no-op forms are eligible even when unrelated owner work is
    // dirty: the task has no pending paths and no repair paths.
    for facts in [
        json!({"noChanges":true,"alreadyInTarget":true,"pendingPaths":[],"otherUncommitted":true}),
        json!({"alreadyInTarget":true,"pendingPaths":[],"otherUncommitted":true}),
    ] {
        assert!(super::task_already_landed(&facts));
    }
    // Missing/ambiguous attribution and a recorded repair path are never a
    // successful no-op, even if a partial fact says the target contains files.
    for facts in [
        json!({"alreadyInTarget":true}),
        json!({"alreadyInTarget":true,"pendingPaths":["task.txt"]}),
        json!({"alreadyInTarget":true,"pendingPaths":[],"repairPaths":["task.txt"]}),
        json!({"alreadyInTarget":false,"pendingPaths":[],"noChanges":true}),
    ] {
        assert!(!super::task_already_landed(&facts));
    }
}

#[tokio::test]
async fn a_repository_without_commits_finishes_automatic_in_place_and_refuses_a_merge() {
    // `git init` without a first commit: HEAD names the unborn `master`, so no
    // target branch exists and a merge can never succeed on retry.
    let base = std::env::temp_dir()
        .join(format!("cw-native-int-unborn-{}", settings::unique()))
        .canonicalize_or_self();
    let root = base.join("repo");
    let state = base.join("state");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "master"]);
    fs::write(root.join(".gitignore"), ".cortex/\n.cortex-dev/\n").unwrap();
    fs::write(root.join("a.txt"), "task file\n").unwrap();
    let env = Env { base, root, state };
    let digest = env.evidence(&[]).await;
    let mut task = json!({"status":"ready_to_merge","sprintId":"s1",
        "acceptance":{"digest":digest,"automatic":true,"version":"1"},
        "acceptedRevision":digest,"checkout":{"kind":"main"},"targetBranch":"master"});
    let sprint = json!({"id":"s1","runMode":"automatic","queueState":"running","taskIds":["t1"]});
    let product = json!({"tasks":{"t1":task.clone()},"sprints":[sprint],"drafts":{},"messages":[],"comments":{},
        "sprintRunModeVersion":1});
    settings::write_json(&product_path(&env.state, "p1").unwrap(), &product).unwrap();

    // An owner merge is refused with the real cause, not "the branch must exist".
    let error = env
        .run(
            json!({"confirmed":true,"taskId":"t1","digest":digest,"method":"commit",
            "message":"Finish","requestId":"manual"}),
        )
        .await
        .unwrap_err();
    assert!(format!("{error:?}").contains("no_commits"), "{error:?}");
    assert_eq!(env.overlay(), task);

    let mut g = Git::new();
    let done = super::automatic(
        &env.root,
        "p1",
        &env.state,
        &json!({"taskId":"t1","sprintId":"s1"}),
        &mut g,
    )
    .await
    .unwrap();
    assert_eq!(done, json!({"done":true,"inPlace":true}));
    let saved = env.overlay();
    assert_eq!(saved["status"], "done");
    assert_eq!(saved["closedAs"], "as_is");
    assert_eq!(saved["notVerifiedByCortex"], false);
    // No Git effect: the repository still has no commit and the file is untracked.
    assert!(!Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .current_dir(&env.root)
        .status()
        .unwrap()
        .success());
    task["status"] = json!("done");
    assert_eq!(saved["acceptance"], task["acceptance"]);
}
