//! Pull with fast-forward, merge or rebase, and a stopped pull's Continue and Abort,
//! against real repositories in temporary folders with a local bare remote.
use super::*;

struct Repo {
    base: PathBuf,
    /// The owner's checkout (the project folder).
    local: PathBuf,
    /// Another clone that moves the remote on.
    other: PathBuf,
    state: PathBuf,
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn run(dir: &Path, a: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(a)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {a:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
impl Repo {
    /// main on the remote and both clones: a.txt with three lines.
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!("cw-pull-{tag}-{}", settings::unique()));
        let origin = base.join("origin.git");
        let (local, other, state) = (base.join("local"), base.join("other"), base.join("state"));
        fs::create_dir_all(&state).unwrap();
        run(
            &base,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        for clone in [&local, &other] {
            run(
                &base,
                &[
                    "clone",
                    "-q",
                    origin.to_str().unwrap(),
                    clone.to_str().unwrap(),
                ],
            );
            for (k, v) in [
                ("user.name", "Fixture"),
                ("user.email", "fixture@example.invalid"),
                ("commit.gpgsign", "false"),
                ("core.hooksPath", "/dev/null"),
            ] {
                run(clone, &["config", k, v]);
            }
            run(clone, &["checkout", "-q", "-B", "main"]);
        }
        fs::write(local.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        run(&local, &["add", "."]);
        run(&local, &["commit", "-q", "-m", "Base"]);
        run(&local, &["push", "-q", "-u", "origin", "main"]);
        run(&other, &["pull", "-q", "origin", "main"]);
        run(&other, &["branch", "-q", "--set-upstream-to=origin/main"]);
        Self {
            base,
            local,
            other,
            state,
        }
    }
    fn commit(dir: &Path, file: &str, text: &str, message: &str) {
        fs::write(dir.join(file), text).unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "-q", "-m", message]);
    }
    /// The remote moves on with a commit the owner's checkout does not have.
    fn remote(&self, file: &str, text: &str) {
        Self::commit(&self.other, file, text, "Remote change");
        run(&self.other, &["push", "-q"]);
    }
    fn head(&self) -> String {
        run(&self.local, &["rev-parse", "HEAD"])
    }
    async fn act(&self, p: Value) -> Result<Value> {
        Git::new().action(&self.local, "p", &self.state, &p).await
    }
    async fn pull(&self, mode: &str) -> Result<Value> {
        self.act(json!({"action":"pull","mode":mode})).await
    }
    async fn operation(&self) -> Value {
        Git::new().operation(&self.local).await.unwrap()
    }
    /// Both sides edit the first line of a.txt.
    fn conflicting(&self) {
        self.remote("a.txt", "remote\ntwo\nthree\n");
        Self::commit(&self.local, "a.txt", "local\ntwo\nthree\n", "Local change");
        run(&self.local, &["fetch", "-q"]);
    }
    fn status(&self) -> String {
        run(&self.local, &["status", "--porcelain"])
    }
}

#[tokio::test]
async fn behind_only_fast_forwards_and_diverged_refuses_a_fast_forward() {
    let r = Repo::new("ff");
    r.remote("b.txt", "remote\n");
    run(&r.local, &["fetch", "-q"]);
    r.act(json!({"action":"pull"})).await.unwrap();
    assert_eq!(r.head(), run(&r.local, &["rev-parse", "origin/main"]));
    // Diverged: the fast-forward pull changes nothing and says why.
    r.remote("c.txt", "remote\n");
    Repo::commit(&r.local, "d.txt", "local\n", "Local");
    let before = r.head();
    let refused = r.pull("ff").await.unwrap_err();
    assert_eq!(refused.code, "pull_not_fast_forward", "{}", refused.message);
    assert!(refused.message.contains("Pull…"), "{}", refused.message);
    assert_eq!(r.head(), before);
    assert_eq!(r.status(), "");
    assert_eq!(
        r.pull("squash").await.unwrap_err().code,
        "unsupported_action"
    );
}

#[tokio::test]
async fn diverged_pull_with_merge_makes_one_merge_commit_of_both_sides() {
    let r = Repo::new("merge");
    r.remote("b.txt", "remote\n");
    Repo::commit(&r.local, "c.txt", "local\n", "Local");
    let local = r.head();
    r.pull("merge").await.unwrap();
    let remote = run(&r.local, &["rev-parse", "origin/main"]);
    assert_eq!(
        run(&r.local, &["show", "-s", "--format=%P", "HEAD"]),
        format!("{local} {remote}")
    );
    assert_eq!(
        fs::read_to_string(r.local.join("b.txt")).unwrap(),
        "remote\n"
    );
    assert_eq!(
        fs::read_to_string(r.local.join("c.txt")).unwrap(),
        "local\n"
    );
    assert_eq!(r.status(), "");
    assert!(r.operation().await.is_null());
    // Merging never rewrote the local commit, so pushing is a fast-forward now.
    run(&r.local, &["merge-base", "--is-ancestor", &local, "HEAD"]);
}

#[tokio::test]
async fn diverged_pull_with_rebase_replays_the_local_commit_on_the_remote() {
    let r = Repo::new("rebase");
    r.remote("b.txt", "remote\n");
    Repo::commit(&r.local, "c.txt", "local\n", "Local");
    r.pull("rebase").await.unwrap();
    let remote = run(&r.local, &["rev-parse", "origin/main"]);
    assert_eq!(run(&r.local, &["rev-parse", "HEAD^"]), remote);
    assert_eq!(run(&r.local, &["log", "-1", "--format=%s"]), "Local");
    assert_eq!(run(&r.local, &["branch", "--show-current"]), "main");
    assert_eq!(r.status(), "");
    assert!(r.operation().await.is_null());
}

#[tokio::test]
async fn a_conflicted_pull_aborts_back_to_exactly_where_it_started() {
    for mode in ["merge", "rebase"] {
        let r = Repo::new(&format!("abort-{mode}"));
        r.conflicting();
        fs::write(r.local.join("untracked.txt"), "kept\n").unwrap();
        let (head, index) = (r.head(), run(&r.local, &["write-tree"]));
        let refused = r.pull(mode).await.unwrap_err();
        assert_eq!(refused.code, "pull_conflict", "{mode}: {}", refused.message);
        assert!(
            refused
                .message
                .contains(&format!("The {mode} stopped at 1 conflicting file")),
            "{}",
            refused.message
        );
        let stopped = r.operation().await;
        assert_eq!(stopped["kind"], mode);
        assert_eq!(stopped["conflicts"], json!(["a.txt"]));
        // The snapshot shows the stopped pull on its checkout.
        let snapshot = Git::new()
            .snapshot(&r.local, "p", &r.state, Some(&json!({"tasks":{}})))
            .await
            .unwrap();
        assert_eq!(snapshot["checkouts"][0]["operation"]["kind"], mode);
        // Another pull, and Continue before the conflict is resolved, are refused.
        assert_eq!(
            r.pull(mode).await.unwrap_err().code,
            "operation_in_progress"
        );
        assert_eq!(
            r.act(json!({"action":"pull_continue"}))
                .await
                .unwrap_err()
                .code,
            "unresolved_conflict"
        );
        assert_eq!(
            r.act(json!({"action":"pull_abort"}))
                .await
                .unwrap_err()
                .code,
            "confirmation_required"
        );
        r.act(json!({"action":"pull_abort","confirmed":true}))
            .await
            .unwrap();
        assert_eq!(r.head(), head, "{mode}");
        assert_eq!(run(&r.local, &["write-tree"]), index, "{mode}");
        assert_eq!(run(&r.local, &["branch", "--show-current"]), "main");
        assert_eq!(r.status(), "?? untracked.txt", "{mode}");
        assert_eq!(
            fs::read_to_string(r.local.join("a.txt")).unwrap(),
            "local\ntwo\nthree\n"
        );
        assert_eq!(
            fs::read_to_string(r.local.join("untracked.txt")).unwrap(),
            "kept\n"
        );
        assert!(r.operation().await.is_null());
        assert_eq!(
            r.act(json!({"action":"pull_abort","confirmed":true}))
                .await
                .unwrap_err()
                .code,
            "no_operation"
        );
    }
}

#[tokio::test]
async fn a_resolved_conflict_continues_to_a_merge_commit_or_the_end_of_the_rebase() {
    for mode in ["merge", "rebase"] {
        let r = Repo::new(&format!("continue-{mode}"));
        r.conflicting();
        let local = r.head();
        assert_eq!(r.pull(mode).await.unwrap_err().code, "pull_conflict");
        // Marked resolved with its markers still in: refused, nothing committed.
        run(&r.local, &["add", "a.txt"]);
        assert_eq!(
            r.act(json!({"action":"pull_continue"}))
                .await
                .unwrap_err()
                .code,
            "conflict_markers"
        );
        fs::write(r.local.join("a.txt"), "both\ntwo\nthree \n").unwrap();
        run(&r.local, &["add", "a.txt"]);
        // Trailing whitespace is not a conflict marker and does not block.
        r.act(json!({"action":"pull_continue"})).await.unwrap();
        let remote = run(&r.local, &["rev-parse", "origin/main"]);
        if mode == "merge" {
            assert_eq!(
                run(&r.local, &["show", "-s", "--format=%P", "HEAD"]),
                format!("{local} {remote}")
            );
        } else {
            assert_eq!(run(&r.local, &["rev-parse", "HEAD^"]), remote);
            assert_eq!(run(&r.local, &["branch", "--show-current"]), "main");
        }
        assert_eq!(
            fs::read_to_string(r.local.join("a.txt")).unwrap(),
            "both\ntwo\nthree \n"
        );
        assert_eq!(r.status(), "");
        assert!(r.operation().await.is_null());
    }
}

#[tokio::test]
async fn uncommitted_changes_no_upstream_and_a_detached_head_are_refused_before_any_change() {
    let r = Repo::new("refusals");
    r.remote("b.txt", "remote\n");
    Repo::commit(&r.local, "c.txt", "local\n", "Local");
    let head = r.head();
    fs::write(r.local.join("a.txt"), "uncommitted\n").unwrap();
    for mode in ["merge", "rebase"] {
        let refused = r.pull(mode).await.unwrap_err();
        assert_eq!(refused.code, "dirty_checkout", "{}", refused.message);
        assert!(
            refused.message.contains("does not stash"),
            "{}",
            refused.message
        );
    }
    assert_eq!(r.head(), head);
    assert_eq!(
        fs::read_to_string(r.local.join("a.txt")).unwrap(),
        "uncommitted\n"
    );
    run(&r.local, &["checkout", "-q", "--", "a.txt"]);
    run(&r.local, &["checkout", "-q", "-b", "loose"]);
    assert_eq!(r.pull("merge").await.unwrap_err().code, "upstream_required");
    run(&r.local, &["checkout", "-q", "--detach", "main"]);
    assert_eq!(r.pull("merge").await.unwrap_err().code, "detached_checkout");
    assert_eq!(r.head(), head);
}

#[tokio::test]
async fn a_task_worktree_pulls_alone_and_a_task_merge_stays_with_the_task() {
    let r = Repo::new("worktree");
    // A task branch with an upstream of its own, in a worktree beside the project folder.
    run(&r.local, &["branch", "task"]);
    run(&r.local, &["push", "-q", "-u", "origin", "task"]);
    let wt = r.base.join("task-wt");
    run(
        &r.local,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "task"],
    );
    run(&r.other, &["fetch", "-q"]);
    run(&r.other, &["checkout", "-q", "-b", "task", "origin/task"]);
    Repo::commit(
        &r.other,
        "a.txt",
        "remote\ntwo\nthree\n",
        "Remote task change",
    );
    run(&r.other, &["push", "-q", "origin", "task"]);
    Repo::commit(&wt, "a.txt", "local\ntwo\nthree\n", "Local task change");
    let main = r.head();
    let id = checkout_id("p", &wt.to_string_lossy(), false);
    let stopped = r
        .act(json!({"action":"pull","mode":"merge","checkoutId":id}))
        .await
        .unwrap_err();
    assert_eq!(stopped.code, "pull_conflict");
    // The project folder is untouched.
    assert_eq!(r.head(), main);
    assert_eq!(r.status(), "");
    assert!(r.operation().await.is_null());
    // While a task's own retained merge is recorded for that worktree, it is the task's to finish or abort.
    let product = json!({"tasks":{"t1":{"title":"Fix it","conflict":"conflict","merge":{"finished":false},
        "checkout":{"kind":"worktree","path":wt.to_str().unwrap(),"branch":"task"}}}});
    settings::write_json(&product_path(&r.state, "p").unwrap(), &product).unwrap();
    let held = r
        .act(json!({"action":"pull_abort","confirmed":true,"checkoutId":id}))
        .await
        .unwrap_err();
    assert_eq!(held.code, "task_merge", "{}", held.message);
    assert!(held.message.contains("“Fix it”"), "{}", held.message);
    settings::write_json(&product_path(&r.state, "p").unwrap(), &json!({"tasks":{}})).unwrap();
    let before = run(&wt, &["rev-parse", "HEAD"]);
    r.act(json!({"action":"pull_abort","confirmed":true,"checkoutId":id}))
        .await
        .unwrap();
    assert_eq!(run(&wt, &["rev-parse", "HEAD"]), before);
    assert_eq!(run(&wt, &["status", "--porcelain"]), "");
}
