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
async fn an_integration_text_is_written_from_the_real_changes() {
    let dir = temp("integration");
    let state = dir.join("state");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&state)
        .unwrap();
    let repo = repository(&dir);
    let mut git = Git::new();
    let changes = changes(&repo, "squash", "main", &[], &mut git)
        .await
        .unwrap();
    let names: Vec<_> = changes.files.iter().map(|f| f.0.as_str()).collect();
    assert_eq!(names, [".env.local", "README.md", "src/upload.rs"]);
    assert_eq!(changes.untracked, ["notes.txt"]);
    let reply = integration_job(&state, "squash", Ok(changes));
    assert_eq!(reply["source"], "template");
    assert!(reply["route"].is_null() && reply["reason"].is_null());
    let message = reply["text"]["message"].as_str().unwrap();
    assert!(message.starts_with("Update 4 files\n\nChanged files:"));
    assert!(message.contains("- `src/upload.rs` (+2 -1)"));
    assert!(message.contains("- `notes.txt` (new)"));
    assert!(!message.contains("retry(3)") && !message.contains("do-not-send"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn integration_templates_use_pending_paths_and_keep_an_unreadable_fallback() {
    let dir = temp("integration-fallback");
    let state = dir.join("state");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&state)
        .unwrap();
    let repo = repository(&dir);
    let mut git = Git::new();
    // Commit to the same branch: only the owner's pending paths count.
    let pending = vec!["README.md".to_owned(), "notes.txt".to_owned()];
    let changes = changes(&repo, "commit", "", &pending, &mut git)
        .await
        .unwrap();
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.untracked, ["notes.txt"]);
    let reply = integration_job(&state, "commit", Ok(changes));
    assert_eq!(reply["source"], "template");
    let message = reply["text"]["message"].as_str().unwrap();
    assert_eq!(message, "Update 2 files\n\nChanged files:\n- `README.md` (+1 -0)\n- `notes.txt` (new)");
    let changes = super::changes(&repo, "pr", "main", &[], &mut git)
        .await
        .unwrap();
    let reply = integration_job(&state, "pr", Ok(changes));
    assert_eq!(reply["source"], "template");
    assert_eq!(reply["text"]["title"], "Update 4 files");
    assert!(reply["text"]["body"].as_str().unwrap().starts_with("Changed files:\n"));
    let reply = integration_job(&state, "commit", Err(refuse("git_failed", "x")));
    assert_eq!(reply["reason"], "changes_unreadable");
    assert_eq!(reply["text"]["message"], "Apply task changes");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn checkout_commit_draft_is_taskless_and_reads_the_index_first() {
    let base = temp("checkout-draft");
    let root = repository(&base);
    let state = base.join("state");
    let config = crate::Config {
        runtime_dir: base.join("run"),
        state_dir: state.clone(),
        allowed_root: base.clone(),
        launch: crate::LaunchSpec::bare(),
        request_timeout: Duration::from_secs(5),
        shutdown_timeout: Duration::from_millis(150),
        event_capacity: 128,
    };
    sh(&root, &["reset", "--", ".env.local"]);
    sh(&root, &["add", "--", "README.md"]);
    std::fs::write(root.join("README.md"), "unstaged version\n").unwrap();
    let staged = changes(&root, "checkout_commit", "", &[], &mut Git::new()).await.unwrap();
    assert_eq!(staged.files, [("README.md".into(), Some(1), Some(0))]);
    assert!(staged.untracked.is_empty());
    let approved = format!("  Markdown\n\n- {}\u{202E}", "é".repeat(6000));
    let body = push_body(&approved);
    assert!(body.starts_with("  Markdown\n\n- "));
    assert!(body.len() <= MAX_PUSH_SUMMARY && !body.contains('\u{202E}'));

    held().lock().unwrap().push((root.clone(), Arc::new(std::sync::atomic::AtomicBool::new(true))));
    let started = run(&root, "p", &state, &json!({"action":"draft_checkout_commit","checkoutId":"p:main"}), &config).await.unwrap();
    let mut poll = json!({"action":"short_text_poll","jobId":started["jobId"]});
    poll["taskId"] = json!("t");
    assert_eq!(run(&root, "p", &state, &poll, &config).await.unwrap_err().code, "unknown_job");
    poll.as_object_mut().unwrap().remove("taskId");
    let result = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let result = run(&root, "p", &state, &poll, &config).await.unwrap();
            if result["state"] == "done" { break result; }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }).await.unwrap();
    assert_eq!(result["source"], "template");
    assert!(result["reason"].is_null() && result["route"].is_null());
    assert_eq!(result["text"]["message"], "Update 1 file\n\nChanged files:\n- `README.md` (+1 -0)");
    held().lock().unwrap().retain(|(r, _)| r != &root);

    sh(&root, &["reset", "--", "README.md"]);
    let all = changes(&root, "checkout_commit", "", &[], &mut Git::new()).await.unwrap();
    assert!(all.untracked.contains(&"notes.txt".to_owned()));
    std::fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn push_summary_counts_outgoing_commits_without_copying_their_prose() {
    let base = temp("outgoing");
    let root = repository(&base);
    sh(&root, &["remote", "add", "origin", "./unused.git"]);
    assert_eq!(outgoing(&mut Git::new(), &root, "origin", "cw/task").await.err().unwrap().code, "push_baseline_unavailable");
    sh(&root, &["update-ref", "refs/remotes/origin/cw/task", "main"]);
    let one = outgoing(&mut Git::new(), &root, "origin", "cw/task").await.unwrap();
    assert_eq!(push_job(&base, Ok(one))["text"]["body"], "This push includes 1 outgoing commit.");
    sh(&root, &["commit", "--only", "-m", "Corriger le chargement", "-m", "Le chargement est réparé.", "--", "README.md"]);
    let commits = outgoing(&mut Git::new(), &root, "origin", "cw/task").await.unwrap();
    let reply = push_job(&base, Ok(commits));
    assert_eq!(reply["source"], "template");
    assert!(reply["route"].is_null());
    assert_eq!(reply["text"], json!({
        "title":"Push 2 outgoing commits",
        "body":"This push includes 2 outgoing commits."
    }));
    sh(&root, &["update-ref", "refs/remotes/origin/cw/task", "HEAD"]);
    assert_eq!(outgoing(&mut Git::new(), &root, "origin", "cw/task").await.err().unwrap().code, "no_outgoing_commits");
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
