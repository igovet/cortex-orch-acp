//! Real Bubblewrap views over synthetic homes, accounts and projects. Without
//! user namespaces (inside a Cortex command view) the positive checks are not
//! run unless `CORTEX_REQUIRE_NAMESPACE_FIXTURES` demands them; preparation and
//! refusal checks always run.
use super::*;
use std::os::unix::fs::PermissionsExt;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "engine-view-fixture-{}",
            crate::native_effects::identity().unwrap()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let home = root.join("home");
        let write = |path: &str, body: &str| {
            let path = home.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        };
        write(".claude/.credentials.json", "synthetic-claude-credential\n");
        write(".claude/history.jsonl", "owner-history-sentinel\n");
        write(".claude/projects/other/session.jsonl", "owner-session-sentinel\n");
        write(".claude/.claude.json", "{\"owner\":\"sentinel\"}\n");
        write(".codex/auth.json", "{\"synthetic\":1}");
        write(".codex/config.toml", "model = \"synthetic\"\n");
        write(".codex/sessions/2026/11/22/rollout.jsonl", "owner-rollout-sentinel\n");
        write(".local/share/opencode/opencode.db", "synthetic-database\n");
        write(".local/share/opencode/neighbor", "owner-neighbor-sentinel\n");
        write(".config/opencode/opencode.json", "{\"synthetic\":true}\n");
        write(".ssh/id_synthetic", "owner-key-sentinel\n");
        write("other-project/notes", "other-project-sentinel\n");
        write("project/source.txt", "usable project input\n");
        write("project/.env", "project-secret-sentinel\n");
        write("project/.cortex/private", "store-sentinel\n");
        fs::create_dir_all(root.join("state")).unwrap();
        Self(root)
    }
    fn home(&self) -> PathBuf {
        self.0.join("home")
    }
    fn state(&self) -> PathBuf {
        self.0.join("state")
    }
    fn view(&self, engine: EngineKind, credential: &str) -> io::Result<Confinement> {
        let home = self.home();
        let private = private_root(&self.state(), "cn_000000000001")?;
        let mut view = Builder::new()?;
        view.home(&home, &private)?;
        view.service_executable()?;
        view.account(engine, Account::Native(&home.join(credential)), &private)?;
        view.engine_config(engine, &home)?;
        view.workdir(Workdir::Project(&home.join("project")), &home)?;
        Ok(view.finish())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn namespaces() -> bool {
    let available = std::process::Command::new("/usr/bin/bwrap")
        .args(["--unshare-pid", "--ro-bind", "/usr", "/usr", "--symlink", "usr/lib", "/lib"])
        .args(["--symlink", "usr/lib64", "/lib64", "--symlink", "usr/bin", "/bin"])
        .args(["--proc", "/proc", "--dev", "/dev", "--", "/usr/bin/true"])
        .env_clear()
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_some() {
        assert!(available, "engine view fixtures require user namespaces");
    }
    available
}

/// Run `script` in the view with HOME set; lines starting `! ` must fail.
fn run(view: &Confinement, home: &Path, script: &str) -> String {
    let script = format!(
        "denied() {{ if \"$@\" 2>/dev/null; then echo \"unexpected success: $*\" >&2; exit 97; fi; }}\n{}",
        script
            .lines()
            .map(|l| l.trim_start().strip_prefix("! ").map_or(l.to_owned(), |c| format!("denied {c}")))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let out = view
        .command(Path::new("/bin/sh"), ["-ec", &script, "view"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .envs(view.environment())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "view script failed: {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Checks every engine view shares: the owner's other files, other projects and
/// Cortex stores are absent, the checkout is read-only with its private inputs
/// masked, the PID namespace is private, and a descendant sees the same view.
const COMMON: &str = r#"
! cat "$HOME/.ssh/id_synthetic"
! cat "$HOME/other-project/notes"
! ls "$HOME/other-project"
test "$(cat "$HOME/project/source.txt")" = "usable project input"
! sh -c 'echo changed > "$HOME/project/source.txt"'
test -z "$(cat "$HOME/project/.env" 2>/dev/null)"
! cat "$HOME/project/.cortex/private"
test "$(ls /proc | grep -c '^[0-9]')" -lt 8
sh -c '! cat "$HOME/.ssh/id_synthetic" 2>/dev/null'
echo private > /tmp/scratch && test "$(cat /tmp/scratch)" = private
echo persisted > "$HOME/.cache/engine-file"
echo launch-only > "$HOME/scratch-file"
"#;

#[test]
fn engine_views_show_only_their_grants() {
    if !namespaces() {
        println!("NOT_RUN engine view positives: user namespaces unavailable");
        return;
    }
    let f = Fixture::new();
    let home = f.home();

    // Claude: the native directory in place, other entries private substitutes.
    let view = f.view(EngineKind::Claude, ".claude").unwrap();
    run(
        &view,
        &home,
        &format!(
            "{COMMON}
test \"$(cat \"$HOME/.claude/.credentials.json\")\" = synthetic-claude-credential
printf 'refreshed\\n' > \"$HOME/.claude/.credentials.json.tmp\"
mv \"$HOME/.claude/.credentials.json.tmp\" \"$HOME/.claude/.credentials.json\"
mkdir \"$HOME/.claude/.oauth_refresh.lock\" && rmdir \"$HOME/.claude/.oauth_refresh.lock\"
test -z \"$(cat \"$HOME/.claude/history.jsonl\")\"
echo cortex-history >> \"$HOME/.claude/history.jsonl\"
test -z \"$(ls \"$HOME/.claude/projects\")\"
test -z \"$(cat \"$HOME/.claude/.claude.json\")\"
! cat \"$HOME/.codex/auth.json\"
! cat \"$HOME/.local/share/opencode/opencode.db\"
"
        ),
    );
    // The staged replacement reached the native file; nothing else did.
    assert_eq!(
        fs::read_to_string(home.join(".claude/.credentials.json")).unwrap(),
        "refreshed\n"
    );
    assert_eq!(
        fs::read_to_string(home.join(".claude/history.jsonl")).unwrap(),
        "owner-history-sentinel\n"
    );
    assert!(!home.join(".cache/engine-file").exists());
    assert!(!home.join("scratch-file").exists());
    let private = f.state().join("cortex/engine-private/cn_000000000001");
    assert_eq!(
        fs::read_to_string(private.join("claude-config/f/history.jsonl")).unwrap(),
        "cortex-history\n"
    );
    // The private cache persists; the rest of the private HOME lives only as
    // long as one launch.
    assert!(private.join("cache/engine-file").is_file());
    assert!(!private.join("home").exists());

    // Codex: a private CODEX_HOME with the native auth file and lock bound in.
    let view = f.view(EngineKind::Codex, ".codex").unwrap();
    run(
        &view,
        &home,
        &format!(
            "{COMMON}
test \"$(cat \"$HOME/.codex/auth.json\")\" = '{{\"synthetic\":1}}'
printf '{{\"synthetic\":2}}' > \"$HOME/.codex/auth.json\"
test -f \"$HOME/.codex/auth.lock\"
test \"$(cat \"$HOME/.codex/config.toml\")\" = 'model = \"synthetic\"'
! sh -c 'echo x >> \"$HOME/.codex/config.toml\"'
! ls \"$HOME/.codex/sessions\"
mkdir -p \"$HOME/.codex/sessions/cortex\"
! cat \"$HOME/.claude/.credentials.json\"
"
        ),
    );
    assert_eq!(
        fs::read_to_string(home.join(".codex/auth.json")).unwrap(),
        "{\"synthetic\":2}"
    );
    assert!(home.join(".codex/auth.lock").is_file());
    assert!(!home.join(".codex/sessions/cortex").exists());

    // OpenCode: the database and its companions in place, neighbours private.
    let view = f.view(EngineKind::OpenCode, ".local/share/opencode").unwrap();
    run(
        &view,
        &home,
        &format!(
            "{COMMON}
D=\"$HOME/.local/share/opencode\"
test \"$(cat \"$D/opencode.db\")\" = synthetic-database
echo wal > \"$D/opencode.db-wal\" && echo shm > \"$D/opencode.db-shm\"
rm \"$D/opencode.db-shm\"
test -z \"$(cat \"$D/neighbor\")\"
echo cortex > \"$D/neighbor\"
test \"$(cat \"$HOME/.config/opencode/opencode.json\")\" = '{{\"synthetic\":true}}'
! sh -c 'echo x > \"$HOME/.config/opencode/opencode.json\"'
! cat \"$HOME/.claude/.credentials.json\"
"
        ),
    );
    let data = home.join(".local/share/opencode");
    assert_eq!(fs::read_to_string(data.join("opencode.db-wal")).unwrap(), "wal\n");
    assert!(!data.join("opencode.db-shm").exists());
    assert_eq!(
        fs::read_to_string(data.join("neighbor")).unwrap(),
        "owner-neighbor-sentinel\n"
    );
}

#[test]
fn broker_directories_are_shown_read_only_at_the_view_path() {
    if !namespaces() {
        println!("NOT_RUN broker view positives: user namespaces unavailable");
        return;
    }
    let f = Fixture::new();
    let home = f.home();
    let broker = f.0.join("cortex-mcp-0123456789abcdef01234567");
    fs::DirBuilder::new().mode(0o700).create(&broker).unwrap();
    fs::write(broker.join("relay.token"), "token").unwrap();
    let private = private_root(&f.state(), "cn_000000000001").unwrap();
    let mut view = Builder::new().unwrap();
    view.home(&home, &private).unwrap();
    view.brokers(std::slice::from_ref(&broker)).unwrap();
    view.inputs(&[broker.join("relay.token")]).unwrap();
    view.workdir(Workdir::Project(&home.join("project")), &home).unwrap();
    let view = view.finish();
    assert_eq!(
        view.view_path(&broker.join("model-instructions.md")),
        Path::new("/run/cortex/mcp/cortex-mcp-0123456789abcdef01234567/model-instructions.md")
    );
    run(
        &view,
        &home,
        "test \"$(cat /run/cortex/mcp/cortex-mcp-0123456789abcdef01234567/relay.token)\" = token
! sh -c 'echo x > /run/cortex/mcp/cortex-mcp-0123456789abcdef01234567/new'
",
    );
}

#[test]
fn preparation_refuses_unsafe_or_missing_inputs() {
    let f = Fixture::new();
    let home = f.home();
    let private = private_root(&f.state(), "cn_000000000001").unwrap();
    assert!(private_root(&f.state(), "../escape").is_err());
    assert!(private_root(Path::new("relative"), "cn_000000000001").is_err());
    let Ok(mut view) = Builder::new() else {
        // No bubblewrap at all: every launch is refused before it starts.
        return;
    };
    assert!(view.home(Path::new("/"), &private).is_err());
    assert!(view.home(Path::new("/usr/share"), &private).is_err());
    // A missing native account directory refuses instead of falling back.
    assert!(view
        .account(
            EngineKind::Claude,
            Account::Native(&home.join(".missing")),
            &private
        )
        .is_err());
    // A project that contains HOME is not a working directory.
    assert!(view.workdir(Workdir::Project(&f.0), &home).is_err());
    // An install that would expose HOME is refused.
    assert!(view.install([home.join("x").as_path()].into_iter().chain([f.0.as_path()]), &home).is_err());
    // A broker directory that is not a private cortex-mcp directory is refused.
    assert!(view.brokers(&[home.join("project")]).is_err());
    assert!(view.inputs(&[home.join("missing-input")]).is_err());
}

struct NoPrompts;
impl cortex_engine::engine::PermissionPolicy for NoPrompts {
    fn authority(&self) -> cortex_engine::FutureResult<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn decide<'a>(
        &'a self,
        _: &'a str,
        _: &'a serde_json::Value,
        _: u64,
        _: &'a serde_json::Value,
    ) -> cortex_engine::FutureResult<'a, cortex_engine::engine::PermissionDecision> {
        Box::pin(async { Ok(cortex_engine::engine::PermissionDecision::Cancelled) })
    }
}

/// The installed engine bundle's real adapters start inside their production
/// route view over a synthetic home with no usable sign-in, answer `initialize`
/// and open a session. No prompt is sent, so no model is called. Run on a host
/// with user namespaces: `CORTEX_REAL_ENGINE_BUNDLE=/usr/lib/<package>/engines
/// cargo test ... real_engine_adapters_start_in_their_route_view -- --ignored`.
#[tokio::test]
#[ignore = "requires an installed engine bundle and user namespaces"]
async fn real_engine_adapters_start_in_their_route_view() {
    let bundle = PathBuf::from(std::env::var_os("CORTEX_REAL_ENGINE_BUNDLE").unwrap());
    assert!(namespaces());
    let f = Fixture::new();
    let home = f.home();
    let node = bundle.join("node/bin/node");
    let adapters = bundle.join("node_modules/@agentclientprotocol");
    // A synthetic API key that is never sent with a prompt.
    fs::write(
        home.join(".codex/auth.json"),
        "{\"auth_mode\":\"apikey\",\"OPENAI_API_KEY\":\"sk-synthetic-offline-not-a-key\"}",
    )
    .unwrap();
    for (engine, entry, credential) in [
        (EngineKind::Claude, "claude-agent-acp/dist/index.js", ".claude"),
        (EngineKind::Codex, "codex-acp/dist/index.js", ".codex"),
    ] {
        let launch = LaunchSpec {
            engine,
            executable: node.canonicalize().unwrap(),
            args: vec![adapters.join(entry).to_string_lossy().into_owned()],
            cwd: home.join("project"),
            native_home: home.clone(),
            credential_directory: home.join(credential),
            base_environment: BTreeMap::from([
                ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
                ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ]),
            codex_launcher: None,
            confinement: Some(RouteConfiner::request(&f.state(), "cn_000000000001")),
        };
        let engine_process =
            cortex_engine::engine::Engine::launch(launch, Arc::new(NoPrompts)).await.unwrap();
        let deadline = std::time::Duration::from_secs(30);
        let initialized = engine_process.initialize(deadline).await;
        let session = match &initialized {
            Ok(_) => engine_process
                .start_session_with_meta(
                    &home.join("project"),
                    vec![],
                    (engine == EngineKind::Claude)
                        .then(crate::native_engine_policy::claude_session_meta),
                    deadline,
                )
                .await
                .map(|s| s.id().to_owned()),
            Err(e) => Err(e.clone()),
        };
        let diagnostics = engine_process.diagnostics().snapshot();
        println!(
            "REAL_ENGINE_VIEW {} initialize={} session={} exit={:?}",
            engine.name(),
            initialized.is_ok(),
            session.is_ok(),
            diagnostics.exit
        );
        assert!(
            initialized.is_ok() && session.is_ok(),
            "{}: {initialized:?} {session:?}\n{}",
            engine.name(),
            diagnostics.stderr_tail
        );
        engine_process.shutdown().await.unwrap();
    }
    // Nothing of the owner's other files or sessions changed.
    assert_eq!(
        fs::read_to_string(home.join(".claude/history.jsonl")).unwrap(),
        "owner-history-sentinel\n"
    );
    assert!(fs::read_dir(home.join(".codex/sessions/2026/11/22")).unwrap().count() == 1);
    assert_eq!(
        fs::read_to_string(home.join(".ssh/id_synthetic")).unwrap(),
        "owner-key-sentinel\n"
    );
}

#[test]
fn only_the_test_peer_beside_this_binary_starts_unconfined() {
    assert!(!beside_test_binary(Path::new("/bin/sh")));
    assert!(!beside_test_binary(Path::new("/nonexistent/fake-engine")));
    assert_eq!(
        beside_test_binary(&std::env::current_exe().unwrap()),
        cfg!(debug_assertions)
    );
}
