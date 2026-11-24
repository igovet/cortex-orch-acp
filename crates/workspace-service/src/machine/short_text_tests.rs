//! Injected runs only: no engine, CLI or model starts here. The ACP run itself
//! is covered with the no-model fake engine in `tests/short_text_engine.rs`.
use super::*;
use std::os::unix::fs::DirBuilderExt;
use std::sync::{Arc, Mutex as StdMutex};

fn temp(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "cw-short-text-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&root)
        .unwrap();
    root.canonicalize().unwrap()
}
fn route() -> ShortTextRoute {
    ShortTextRoute {
        engine: "claude-agent-acp".into(),
        model: "haiku".into(),
        connection: "cn_000000000001".into(),
        tier: "light",
        used: Some(10.0),
        reason: "least_used",
    }
}
/// A run that answers `answer` and keeps the prompt it was given.
fn answering(
    answer: std::result::Result<&'static str, &'static str>,
) -> (Chosen, Arc<StdMutex<String>>) {
    let seen = Arc::new(StdMutex::new(String::new()));
    let keep = seen.clone();
    let chosen = Chosen {
        route: route(),
        run: Box::new(move |prompt, _| {
            *keep.lock().unwrap() = prompt.to_owned();
            answer.map(str::to_owned)
        }),
    };
    (chosen, seen)
}
fn log_text(state: &Path) -> String {
    std::fs::read_to_string(state.join("logs/short-text.log")).unwrap_or_default()
}

#[test]
fn a_model_title_is_saved_as_generated_and_the_choice_is_logged_without_text() {
    let dir = temp("title");
    let state = dir.join("state");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&state)
        .unwrap();
    let (c, seen) = answering(Ok("\"Title: Retry\u{202E} failed\u{200B} uploads.\""));
    let reply = title_job(
        &state,
        "project",
        "task_1",
        "PRIVATE uploads sometimes fail, please add a retry",
        Ok(c),
        || false,
        Duration::from_secs(10),
    );
    assert_eq!(reply["source"], "model");
    assert_eq!(reply["saved"], true);
    assert_eq!(reply["route"]["model"], "haiku");
    assert_eq!(reply["route"]["reason"], "least_used");
    assert!(seen
        .lock()
        .unwrap()
        .contains("PRIVATE uploads sometimes fail"));
    let saved = super::super::native::product(&state, "project").unwrap();
    // Bidirectional overrides and zero-width characters never reach a title.
    assert_eq!(saved["tasks"]["task_1"]["title"], "Retry failed uploads");
    assert_eq!(saved["tasks"]["task_1"]["titleState"], "generated");
    let log = log_text(&state);
    assert!(
        log.contains("short_text_selected") && log.contains("short_text_generated"),
        "{log}"
    );
    assert!(
        log.contains("\"model\":\"haiku\"") && log.contains("\"connection\":\"cn_000000000001\"")
    );
    assert!(
        !log.contains("PRIVATE") && !log.contains("Retry failed"),
        "{log}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn without_a_route_or_an_answer_the_fallback_title_is_saved_as_not_generated() {
    let dir = temp("fallback");
    let state = dir.join("state");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&state)
        .unwrap();
    let reply = title_job(
        &state,
        "project",
        "task_1",
        "make the \u{2066}upload\u{2069} retry on failure. Then log it.",
        Err("no_route"),
        || false,
        Duration::from_secs(1),
    );
    assert_eq!(reply["source"], "fallback");
    assert_eq!(reply["reason"], "no_route");
    let saved = super::super::native::product(&state, "project").unwrap();
    assert_eq!(
        saved["tasks"]["task_1"]["title"],
        "Make the upload retry on failure"
    );
    assert_eq!(saved["tasks"]["task_1"]["titleState"], "failed");
    assert!(log_text(&state).contains("\"outcome\":\"no_route\""));
    // A run that times out, asks for a permission or is refused falls back too.
    for reason in [
        "timeout",
        "permission_requested",
        "codex_hardening_unavailable",
    ] {
        let (c, _) = answering(Err(reason));
        let reply = title_job(
            &state,
            "project",
            "task_2",
            "x",
            Ok(c),
            || false,
            Duration::from_secs(1),
        );
        assert_eq!(
            (reply["source"].as_str(), reply["reason"].as_str()),
            (Some("fallback"), Some(reason))
        );
        assert!(log_text(&state).contains(&format!("\"outcome\":\"{reason}\"")));
    }
    // A retry may then replace the fallback.
    let (c, _) = answering(Ok("Upload retry"));
    let reply = title_job(
        &state,
        "project",
        "task_1",
        "x",
        Ok(c),
        || false,
        Duration::from_secs(10),
    );
    assert_eq!(reply["saved"], true);
    let saved = super::super::native::product(&state, "project").unwrap();
    assert_eq!(saved["tasks"]["task_1"]["titleState"], "generated");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_owner_title_or_a_held_machine_lock_is_never_overwritten_or_waited_on() {
    let dir = temp("owner");
    let state = dir.join("state");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&state)
        .unwrap();
    assert!(!save_title(
        &state,
        "project",
        "t",
        "Model",
        true,
        || true,
        Duration::from_secs(1)
    )
    .unwrap());
    settings::write_json(
        &product_path(&state, "project").unwrap(),
        &json!({"tasks":{"u":{"title":"Mine","titleState":"user"}}}),
    )
    .unwrap();
    assert!(!save_title(
        &state,
        "project",
        "u",
        "Model",
        true,
        || false,
        Duration::from_secs(1)
    )
    .unwrap());
    let held = settings::Lock::new(&state, "machine.lock").unwrap();
    let started = Instant::now();
    assert!(!save_title(
        &state,
        "project",
        "t",
        "Model",
        true,
        || false,
        Duration::from_millis(300)
    )
    .unwrap());
    assert!(started.elapsed() < Duration::from_secs(3));
    drop(held);
    let saved = super::super::native::product(&state, "project").unwrap();
    assert_eq!(saved["tasks"]["u"]["title"], "Mine");
    assert!(saved["tasks"]["t"].is_null());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn answers_are_shaped_and_unusable_ones_rejected() {
    assert_eq!(
        title_of("```\nAdd dark mode\n```").as_deref(),
        Some("Add dark mode")
    );
    assert_eq!(
        title_of("# Task title: «Добавить тёмную тему»").as_deref(),
        Some("Добавить тёмную тему")
    );
    assert_eq!(title_of("   \n "), None);
    assert_eq!(title_of("\u{FEFF}\u{200B}\u{202A}"), None);
    assert!(title_of(&"word ".repeat(60)).unwrap().chars().count() <= 80);
    assert_eq!(fallback_title(""), "Untitled task");
    assert!(fallback_title(&"a".repeat(100)).ends_with('…'));
    for c in [
        '\u{202A}',
        '\u{202E}',
        '\u{2066}',
        '\u{2069}',
        '\u{200B}',
        '\u{200F}',
        '\u{FEFF}',
        '\u{061C}',
        '\u{E0001}',
    ] {
        assert!(is_format(c), "{c:?}");
    }
    for c in ['a', 'é', 'Ж', '…', ' ', '\u{00A0}'] {
        assert!(!is_format(c), "{c:?}");
    }
}

#[test]
fn integration_templates_use_git_identifiers_not_prose() {
    let changes = Changes {
        untracked: vec!["src/échantillon`\nя.rs".into()],
        ..Changes::default()
    };
    let draft = integration_text("pr", &changes);
    assert_eq!(draft["title"], "Update 1 file");
    assert_eq!(
        draft["body"],
        "Changed files:\n- `src/\\u{e9}chantillon\\u{60}\\u{a}\\u{44f}.rs` (new)"
    );
    let saved = saved_pr(None, "Texte saisi par le propriétaire".into());
    assert_eq!(saved["text"]["title"], "Apply task changes");
    assert_eq!(saved["text"]["body"], "Texte saisi par le propriétaire");
    assert_eq!(saved["source"], "saved");
}

#[test]
fn private_files_are_named_and_never_read() {
    for path in [
        ".env",
        ".env.local",
        "deploy/id_rsa",
        "id_dsa.pub",
        "keys/id_ecdsa",
        "id_ed25519",
        "tls/server.pem",
        "a.key",
        "store.p12",
        "cert.pfx",
        ".netrc",
        "web/.npmrc",
        ".pypirc",
        "infra/prod.tfvars",
        "terraform.tfstate",
        "terraform.tfstate.backup",
        "kubeconfig",
        "kubeconfig.yaml",
        ".kube/config",
        "home/.kube/config",
        "credentials",
        "aws/credentials.json",
        "gcp/service-account-prod.json",
        "service-account.json",
        ".htpasswd",
        "app.keystore",
        "release.jks",
        "my_secret.txt",
        "API_TOKEN",
        "Passwords.csv",
        // A well-known key name is a prefix: copies and public halves are still keys.
        "id_rsa_fake",
        "keys/ID_RSA.pub",
    ] {
        assert!(private_name(path), "{path}");
    }
    for path in [
        "src/main.rs",
        "README.md",
        "config/settings.toml",
        "kube/deploy.yaml",
        "docs/keys.md",
        "Cargo.toml",
        // Exact well-known names, never a substring of another name.
        "fake_id_rsa",
        "tests/rsa_test.py",
        "docs/notes.env.md",
    ] {
        assert!(!private_name(path), "{path}");
    }
}

fn sh(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}
fn repository(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
    std::fs::create_dir(&repo).unwrap();
    sh(&repo, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/upload.rs"), "fn upload() {}\n").unwrap();
    std::fs::write(repo.join("README.md"), "readme\n").unwrap();
    sh(&repo, &["add", "."]);
    sh(&repo, &["commit", "-q", "-m", "base"]);
    sh(&repo, &["checkout", "-q", "-b", "cw/task"]);
    std::fs::write(
        repo.join("src/upload.rs"),
        "fn upload() { retry(3); }\nfn retry(n: u32) {}\n",
    )
    .unwrap();
    sh(&repo, &["commit", "-q", "-am", "work"]);
    std::fs::write(repo.join("README.md"), "readme\nRetries uploads.\n").unwrap();
    std::fs::write(repo.join(".env.local"), "TOKEN=do-not-send\n").unwrap();
    sh(&repo, &["add", ".env.local"]);
    std::fs::write(repo.join("notes.txt"), "untracked\n").unwrap();
    repo
}

#[tokio::test]
async fn squash_draft_uses_actual_text_but_not_private_files() {
    let dir = temp("integration");
    let state = dir.join("state");
    let repo = repository(&dir);
    let changes = changes(&repo, "squash", "main", &[], &mut Git::new())
        .await
        .unwrap();
    let (chosen, seen) = answering(Ok("Subject: Retry uploads\n\nBody: Retry a failed upload up to three times."));
    let reply = integration_job(&state, "squash", Ok(changes), || Ok(chosen));
    assert_eq!(reply["source"], "model");
    assert!(reply["reason"].is_null());
    assert_eq!(reply["route"]["model"], "haiku");
    assert_eq!(reply["text"]["message"], "Retry uploads\n\nRetry a failed upload up to three times.");
    let prompt = seen.lock().unwrap();
    assert!(prompt.contains("src/upload.rs") && prompt.contains("+fn upload() { retry(3); }"));
    assert!(prompt.contains("notes.txt") && !prompt.contains("+untracked"));
    assert!(!prompt.contains(".env.local") && !prompt.contains("do-not-send"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn commit_fallback_has_a_reason_and_keeps_the_selected_paths() {
    let dir = temp("integration-fallback");
    let state = dir.join("state");
    let repo = repository(&dir);
    let pending = vec!["README.md".to_owned(), "notes.txt".to_owned()];
    for (chosen, reason) in [
        (Err("no_route"), "no_route"),
        (Ok(answering(Err("timeout")).0), "timeout"),
        (Ok(answering(Ok("Subject with no body")).0), "unusable_answer"),
    ] {
        let changes = changes(&repo, "commit", "", &pending, &mut Git::new()).await.unwrap();
        assert!(changes.excerpt.contains("+Retries uploads."));
        assert!(!changes.excerpt.contains("retry(3)") && !changes.excerpt.contains("do-not-send"));
        let reply = integration_job(&state, "commit", Ok(changes), || chosen);
        assert_eq!(reply["source"], "fallback");
        assert_eq!(reply["reason"], reason);
        assert_eq!(reply["text"]["message"], "Update 2 files\n\nChanged files:\n- `README.md` (+1 -0)\n- `notes.txt` (new)");
    }
    let changes = changes(&repo, "pr", "main", &[], &mut Git::new()).await.unwrap();
    let reply = integration_job(&state, "pr", Ok(changes), || panic!("PR uses its existing draft path"));
    assert_eq!(reply["source"], "template");
    assert_eq!(reply["text"]["title"], "Update 4 files");
    let reply = integration_job(&state, "commit", Err(refuse("git_failed", "x")), || panic!("no input"));
    assert_eq!(reply["reason"], "changes_unreadable");
    assert_eq!(reply["text"], json!({}));
    std::fs::remove_dir_all(&dir).unwrap();
}

fn git_routes() -> &'static StdMutex<Vec<(PathBuf, Chosen)>> {
    static ROUTES: OnceLock<StdMutex<Vec<(PathBuf, Chosen)>>> = OnceLock::new();
    ROUTES.get_or_init(Default::default)
}

pub(in crate::machine) fn set_git_route(root: &Path, chosen: Chosen) {
    git_routes().lock().unwrap().push((root.to_owned(), chosen));
}

pub(super) fn git_route(root: &Path) -> Option<std::result::Result<Chosen, &'static str>> {
    let mut routes = git_routes().lock().unwrap();
    let index = routes.iter().position(|(path, _)| path == root)?;
    Some(Ok(routes.swap_remove(index).1))
}

fn draft_config(base: &Path) -> crate::Config {
    crate::Config {
        runtime_dir: base.join("run"),
        state_dir: base.join("state"),
        allowed_root: base.to_owned(),
        launch: crate::LaunchSpec::bare(),
        request_timeout: Duration::from_secs(5),
        shutdown_timeout: Duration::from_millis(150),
        event_capacity: 128,
    }
}

async fn poll_draft(root: &Path, config: &crate::Config, job: &Value) -> Value {
    let poll = json!({"action":"short_text_poll","jobId":job});
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let result = run(root, "p", &config.state_dir, &poll, config).await.unwrap();
            if result["state"] == "done" { break result; }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }).await.unwrap()
}

#[tokio::test]
async fn checkout_commit_draft_is_taskless_and_uses_distinct_index_or_worktree_data() {
    let base = temp("checkout-draft");
    let root = repository(&base);
    let config = draft_config(&base);
    sh(&root, &["reset", "--", ".env.local"]);
    sh(&root, &["add", "--", "README.md"]);
    std::fs::write(root.join("README.md"), "readme\nAdds preview before upload.\n").unwrap();

    let mut prompts = Vec::new();
    for (expected, excluded) in [("Retries uploads.", "Adds preview"), ("Adds preview", "Retries uploads.")] {
        let (chosen, seen) = answering(Ok("Explain upload behavior\n\nDescribe the upload change in detail."));
        set_git_route(&root, chosen);
        let started = run(&root, "p", &config.state_dir, &json!({"action":"draft_checkout_commit","checkoutId":"p:main"}), &config).await.unwrap();
        let wrong = json!({"action":"short_text_poll","jobId":started["jobId"],"taskId":"t"});
        assert_eq!(run(&root, "p", &config.state_dir, &wrong, &config).await.unwrap_err().code, "unknown_job");
        let result = poll_draft(&root, &config, &started["jobId"]).await;
        assert_eq!(result["source"], "model");
        assert_eq!(result["text"]["message"], "Explain upload behavior\n\nDescribe the upload change in detail.");
        let prompt = seen.lock().unwrap().clone();
        assert!(prompt.contains(expected) && !prompt.contains(excluded));
        assert!(!prompt.contains(".env.local"));
        prompts.push(prompt);
        sh(&root, &["reset", "--", "README.md"]);
    }
    assert_ne!(prompts[0], prompts[1]);
    assert!(!prompts[0].contains("notes.txt") && prompts[1].contains("notes.txt"));
    std::fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn git_diff_prompt_is_bounded_and_filters_credential_like_lines() {
    let base = temp("bounded-draft");
    let root = repository(&base);
    sh(&root, &["reset", "--", ".env.local"]);
    let text = format!("readme\npassword = \"example-do-not-send\"\n{}", "é fairly long line\n".repeat(1000));
    std::fs::write(root.join("README.md"), text).unwrap();
    let changes = changes(&root, "checkout_commit", "", &[], &mut Git::new()).await.unwrap();
    assert!(changes.excerpt.len() <= MAX_PATCH);
    assert!(changes.excerpt.contains("[truncated]") && changes.excerpt.contains("+é fairly long line"));
    let prompt = commit_prompt(&changes);
    assert!(prompt.contains("[credential-like line omitted]") && !prompt.contains("example-do-not-send"));
    let generated = format!("  Markdown\n\n- {}\u{202E}", "é".repeat(6000));
    let body = push_body(&generated);
    assert!(body.starts_with("  Markdown\n\n- "));
    assert!(body.len() <= MAX_PUSH_SUMMARY && !body.contains('\u{202E}'));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn confirmed_push_summary_is_kept_whole_or_refused_never_shortened() {
    // Multi-byte text exactly at the byte limit, with an invisible control the
    // limit does not count, is kept whole without the control.
    let at_limit = format!("  Markdown\n\n- {}", "é".repeat((MAX_PUSH_SUMMARY - 14) / 2));
    assert_eq!(at_limit.len(), MAX_PUSH_SUMMARY);
    assert_eq!(confirmed_push_body(&format!("\u{202E}{at_limit}")).unwrap(), at_limit);
    let over = format!("{at_limit}é");
    assert_eq!(confirmed_push_body(&over).unwrap_err().code, "push_summary_too_long");
    assert_eq!(confirmed_push_body("").unwrap(), "");
}

#[tokio::test]
async fn push_summary_model_gets_outgoing_subjects_bodies_and_stats_only() {
    let base = temp("outgoing");
    let root = repository(&base);
    let config = draft_config(&base);
    sh(&root, &["remote", "add", "origin", "./unused.git"]);
    assert_eq!(outgoing(&mut Git::new(), &root, "origin", "cw/task").await.err().unwrap().code, "push_baseline_unavailable");
    sh(&root, &["update-ref", "refs/remotes/origin/cw/task", "main"]);
    sh(&root, &["commit", "--only", "-m", "Corriger le chargement", "-m", "Le chargement est réparé.", "-m", "token=not-for-the-model", "--", "README.md"]);

    let (chosen, seen) = answering(Ok("## Changes\n\nRetry uploads and explain loading behavior."));
    set_git_route(&root, chosen);
    let started = run(&root, "p", &config.state_dir, &json!({"action":"draft_push_summary","checkoutId":"p:main","remote":"origin","branch":"cw/task"}), &config).await.unwrap();
    let reply = poll_draft(&root, &config, &started["jobId"]).await;
    assert_eq!(reply["source"], "model");
    assert_eq!(reply["text"], json!({"body":"## Changes\n\nRetry uploads and explain loading behavior."}));
    let prompt = seen.lock().unwrap();
    for expected in ["Outgoing commit count: 2", "Subject: work", "Subject: Corriger le chargement", "Le chargement est réparé.", "1 file changed"] {
        assert!(prompt.contains(expected), "missing {expected}");
    }
    for excluded in ["Subject: base", "not-for-the-model", "./unused.git", "+Retries uploads."] {
        assert!(!prompt.contains(excluded), "unexpected {excluded}");
    }
    sh(&root, &["update-ref", "refs/remotes/origin/cw/task", "HEAD"]);
    assert_eq!(outgoing(&mut Git::new(), &root, "origin", "cw/task").await.err().unwrap().code, "no_outgoing_commits");
    let reply = push_job(&config.state_dir, Ok("outgoing".to_owned()), || Err("engine_unavailable"));
    assert_eq!(reply["source"], "fallback");
    assert_eq!(reply["reason"], "engine_unavailable");
    assert_eq!(reply["text"], json!({}));
    std::fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn push_context_caps_commit_count_and_utf8_bytes() {
    let base = temp("outgoing-bound");
    let root = repository(&base);
    sh(&root, &["remote", "add", "origin", "./unused.git"]);
    // New destination: use the other local refs of that remote, not all history.
    sh(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    sh(&root, &["reset", "--", ".env.local"]);
    for index in 0..=MAX_OUTGOING {
        sh(&root, &["commit", "--allow-empty", "-qm", &format!("Outgoing fixture {index:02}")]);
    }
    let mut git = Git::new().local_draft();
    let read = outgoing(&mut git, &root, "origin", "cw/task").await.unwrap();
    assert!(read.contains("Outgoing commit count: 21"));
    assert!(read.contains("Subject: Outgoing fixture 20"));
    assert!(!read.contains("Subject: Outgoing fixture 00"));
    assert!(read.ends_with(TRUNCATED));
    sh(&root, &["commit", "--amend", "--allow-empty", "-qm", "Outgoing fixture 20", "-m", &"évidence for the summary\n".repeat(1200)]);
    let read = outgoing(&mut git, &root, "origin", "cw/task").await.unwrap();
    let context = read.split_once("Outgoing commit count: 21\n").unwrap().1;
    assert!(context.len() <= MAX_PUSH_CONTEXT && context.ends_with(TRUNCATED));
    assert!(context.contains("évidence for the summary"));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_job_runs_once_per_task_and_purpose_and_is_polled_by_id() {
    let (a, fresh) = begin("p-jobs", "t", "title");
    assert!(fresh);
    let (b, again) = begin("p-jobs", "t", "title");
    assert_eq!((a.as_str(), again), (b.as_str(), false));
    let (c, other) = begin("p-jobs", "t", "commit");
    assert!(other && c != a);
    finish(&a, json!({"state":"done"}));
    let (d, after) = begin("p-jobs", "t", "title");
    assert!(after && d != a);
}

#[test]
fn draft_jobs_are_shared_only_for_the_same_method_target_paths_and_remote() {
    let paths = vec!["a.rs".to_owned(), "b.rs".to_owned()];
    let base = integration_key("squash", "main", &[], "");
    for other in [
        integration_key("commit", "main", &[], ""),
        integration_key("pr", "main", &[], ""),
        integration_key("squash", "dev", &[], ""),
        integration_key("squash", "main", &paths, ""),
        integration_key("squash", "main", &[], "origin"),
    ] {
        assert_ne!(base, other);
    }
    assert_ne!(
        integration_key("commit", "main", &paths, ""),
        integration_key("commit", "main", &paths[..1], "")
    );
    assert_eq!(base, integration_key("squash", "main", &[], ""));
    // A squash and a commit draft of one task run as separate jobs.
    let (squash, _) = begin("p-keys", "t", &base);
    let (commit, fresh) = begin(
        "p-keys",
        "t",
        &integration_key("commit", "main", &paths, ""),
    );
    assert!(fresh && squash != commit);
    let (again, fresh) = begin("p-keys", "t", &base);
    assert_eq!((again.as_str(), fresh), (squash.as_str(), false));
}

// ---------------------------------------------------------------- a model that never answers

/// Projects whose title jobs run on a model that answers only once released, as an engine that never answers would.
fn held() -> &'static StdMutex<Vec<(PathBuf, Arc<std::sync::atomic::AtomicBool>)>> {
    static HELD: std::sync::OnceLock<StdMutex<Vec<(PathBuf, Arc<std::sync::atomic::AtomicBool>)>>> =
        std::sync::OnceLock::new();
    HELD.get_or_init(Default::default)
}
/// The route a title job of `root` takes when a test holds that project's model.
pub(super) fn held_route(root: &Path) -> Option<std::result::Result<Chosen, &'static str>> {
    let release = held()
        .lock()
        .unwrap()
        .iter()
        .find(|(r, _)| r == root)
        .map(|(_, release)| release.clone())?;
    Some(Ok(Chosen {
        route: route(),
        run: Box::new(move |_, budget| {
            let until = Instant::now() + budget;
            while !release.load(std::sync::atomic::Ordering::SeqCst) {
                if Instant::now() >= until {
                    return Err("timeout");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err("timeout")
        }),
    }))
}

/// A title job whose model never answers holds nothing the saved task, the board, its facts or the next save need: each
/// answers at once while the job still runs, and the job then ends with the description label as the task's title.
#[tokio::test]
async fn a_title_job_without_an_answer_delays_no_read_and_no_save() {
    use std::os::unix::fs::PermissionsExt;
    let base = temp("held-model");
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = base.join("project");
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let config = crate::Config {
        runtime_dir: base.join("run"),
        state_dir: base.join("state"),
        allowed_root: base.clone(),
        launch: crate::LaunchSpec::bare(),
        request_timeout: Duration::from_secs(5),
        shutdown_timeout: Duration::from_millis(150),
        event_capacity: 128,
    };
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    held().lock().unwrap().push((root.clone(), release.clone()));
    let service =
        crate::Service::bind_native(config, Arc::new(crate::native_runtime::UnqualifiedRoutes))
            .await
            .unwrap();
    let id = service.register(&root, "Held model").await.unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    service.activate(&id).await.unwrap();
    let create = |request: &str, description: &str| {
        json!({"protocol":{"major":1,"minor":8},"projectId":id,"actorId":"owner","requestId":request,
            "operation":"tasks.create","target":id,"expectedVersion":"0","ownerEpoch":null,
            "payload":{"description":description,"checkout":{"kind":"main"}}})
    };
    let receipt = service
        .workspace(create("first", "Held title task. More text."))
        .await
        .unwrap();
    assert_eq!(receipt["outcome"], "recorded");
    let task = receipt["target"].as_str().unwrap().to_owned();
    let call = |op: &str, params: Value| -> super::super::MachineCall {
        serde_json::from_value(json!({"op":op,"projectId":id,"params":params})).unwrap()
    };
    // Starting the job answers at once with its id.
    let started = Instant::now();
    let reply = service
        .machine(call(
            "product.task",
            json!({"action":"generate_title","taskId":task}),
        ))
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    let job = reply["result"]["jobId"].as_str().unwrap().to_owned();
    let poll = || {
        call(
            "product.task",
            json!({"action":"short_text_poll","taskId":task,"jobId":job}),
        )
    };
    // While the model is silent every read and the next save answer promptly.
    for _ in 0..3 {
        for (op, params) in [
            ("product.read", Value::Null),
            ("product.board", Value::Null),
            ("product.facts", json!({"taskIds":[]})),
            ("product.facts", json!({"taskIds":[],"defer":true})),
            (
                "product.task",
                json!({"action":"short_text_poll","taskId":task,"jobId":job}),
            ),
        ] {
            let started = Instant::now();
            let reply = service.machine(call(op, params)).await.unwrap();
            assert_eq!(reply["ok"], true, "{op}: {reply}");
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "{op} took {:?}",
                started.elapsed()
            );
        }
        let started = Instant::now();
        let saved = service
            .workspace(create(
                &format!("next-{}", started.elapsed().as_nanos()),
                "Another task",
            ))
            .await
            .unwrap();
        assert_eq!(saved["outcome"], "recorded");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "tasks.create took {:?}",
            started.elapsed()
        );
        let read = service.machine(poll()).await.unwrap();
        assert_eq!(read["result"]["state"], "running", "{read}");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    // The model never answers: the job ends with the description label, saved as a title not generated.
    release.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut done = Value::Null;
    for _ in 0..200 {
        let read = service.machine(poll()).await.unwrap();
        if read["result"]["state"] == "done" {
            done = read["result"].clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(done["source"], "fallback", "{done}");
    assert_eq!(done["saved"], true, "{done}");
    let product = service
        .machine(call("product.read", Value::Null))
        .await
        .unwrap();
    assert_eq!(
        product["result"]["tasks"][&task]["title"],
        "Held title task"
    );
    assert_eq!(product["result"]["tasks"][&task]["titleState"], "failed");
    held().lock().unwrap().retain(|(r, _)| r != &root);
    service.stop().await.unwrap();
    let _ = std::fs::remove_dir_all(&base);
}
