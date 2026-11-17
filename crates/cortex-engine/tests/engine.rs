use cortex_engine::{
    engine::{Engine, Event, PermissionDecision, PermissionPolicy},
    environment::{EngineKind, LaunchSpec},
    Error, FutureResult,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
const D: Duration = Duration::from_secs(3);
struct Policy {
    calls: AtomicUsize,
    option: &'static str,
}
impl PermissionPolicy for Policy {
    fn authority(&self) -> FutureResult<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn decide<'a>(
        &'a self,
        _: &'a str,
        _: &'a Value,
        _: u64,
        _: &'a Value,
    ) -> FutureResult<'a, PermissionDecision> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok(PermissionDecision::Selected(self.option.into())) })
    }
}
fn spec(mode: &str, engine: EngineKind) -> LaunchSpec {
    let home = std::env::current_dir().unwrap();
    LaunchSpec {
        engine,
        executable: PathBuf::from(env!("CARGO_BIN_EXE_fake-engine")),
        args: vec![mode.into()],
        cwd: home.clone(),
        native_home: home.clone(),
        credential_directory: home,
        base_environment: BTreeMap::new(),
        codex_launcher: None,
    }
}
async fn launch(mode: &str) -> Engine {
    Engine::launch(
        spec(mode, EngineKind::Codex),
        Arc::new(Policy {
            calls: AtomicUsize::new(0),
            option: "yes",
        }),
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn sessions_configure_stream_and_cancel_without_replay() {
    let e = launch("").await;
    e.initialize(D).await.unwrap();
    let s = e
        .start_session(&std::env::current_dir().unwrap(), vec![], D)
        .await
        .unwrap();
    assert!(matches!(
        e.configure(&s, "model", "invented", D).await,
        Err(Error::Invalid)
    ));
    e.configure(&s, "model", "fixture-model", D).await.unwrap();
    assert_eq!(s.config_options()[0]["currentValue"], "fixture-model");
    assert_eq!(
        e.prompt(&s, "héllo").await.unwrap()["stopReason"],
        "end_turn"
    );
    assert!(matches!(e.next_event().await, Some(Event::Update(_))));
    {
        let prompt = e.prompt(&s, "hang");
        tokio::pin!(prompt);
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(30))=>{},r=&mut prompt=>panic!("{r:?}")}
        e.cancel(&s).await.unwrap();
        assert_eq!(
            tokio::time::timeout(D, &mut prompt).await.unwrap().unwrap()["stopReason"],
            "cancelled"
        );
        // A successful terminal reply, not cancel delivery, allows the next turn.
        e.prompt(&s, "after cancel").await.unwrap();
    }
    e.shutdown().await.unwrap();
}
/// A route's `thought_level` effort is sent under each engine's own advertised
/// option id (codex-acp `reasoning_effort`, claude-agent-acp `effort`), as
/// Python's codex/claude option mappings do; never under the category name.
#[tokio::test]
async fn route_effort_uses_each_engines_advertised_option_id() {
    for (kind, id) in [
        (EngineKind::Codex, "reasoning_effort"),
        (EngineKind::Claude, "effort"),
    ] {
        let e = Engine::launch(
            spec("", kind),
            Arc::new(Policy {
                calls: AtomicUsize::new(0),
                option: "yes",
            }),
        )
        .await
        .unwrap();
        e.initialize(D).await.unwrap();
        let s = e
            .start_session(&std::env::current_dir().unwrap(), vec![], D)
            .await
            .unwrap();
        e.configure(&s, "model", "fixture-model", D).await.unwrap();
        let effort = |s: &cortex_engine::engine::Session| {
            s.config_options()
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["id"] == id)
                .map(|o| o["currentValue"].clone())
        };
        assert_eq!(effort(&s).unwrap(), "medium", "{kind:?}");
        e.configure(&s, "thought_level", "high", D)
            .await
            .unwrap_or_else(|err| panic!("{kind:?}: {err:?}"));
        assert_eq!(effort(&s).unwrap(), "high", "{kind:?}");
        assert_eq!(s.config_options()[0]["currentValue"], "fixture-model");
        // Only advertised values; an unknown parameter name is never mapped.
        assert!(matches!(
            e.configure(&s, "thought_level", "invented", D).await,
            Err(Error::Invalid)
        ));
        assert!(matches!(
            e.configure(&s, "model_config", "on", D).await,
            Err(Error::Invalid)
        ));
        e.shutdown().await.unwrap();
    }
}
/// claude-agent-acp 0.107.0 sends a new session's `available_commands_update` right
/// behind its `session/new` reply. The reader registers the session from that reply
/// before delivering it, so the update is a session event, not an update for an
/// unknown session that closes the transport (seen as `configure` failing with
/// `transport_closed_outcome_unknown`). Both frames arrive in one write here, on
/// both runtime flavours, many times.
#[test]
fn session_update_right_behind_the_session_new_reply_keeps_the_transport() {
    async fn once() {
        let e = launch("commands-after-new").await;
        e.initialize(D).await.unwrap();
        let s = e
            .start_session(&std::env::current_dir().unwrap(), vec![], D)
            .await
            .unwrap();
        e.configure(&s, "model", "fixture-model", D)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "configure after session/new: {err:?} {:?}",
                    e.diagnostics().snapshot()
                )
            });
        match e.next_event().await {
            Some(Event::Update(params)) => {
                assert_eq!(params["sessionId"], s.id());
                assert_eq!(
                    params["update"]["sessionUpdate"],
                    "available_commands_update"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(e.diagnostics().snapshot().transport_close, None);
        e.shutdown().await.unwrap();
    }
    for _ in 0..20 {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(once());
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(once());
    }
}
/// A process that dies leaves a bounded, redacted stderr tail, its exit status and
/// the transport close reason behind; the handle outlives `shutdown`.
#[tokio::test]
async fn failed_engine_leaves_bounded_redacted_diagnostics() {
    let e = launch("stderr-exit").await;
    let diagnostics = e.diagnostics();
    assert!(matches!(e.initialize(D).await, Err(Error::Closed)));
    let _ = e.shutdown().await;
    let d = diagnostics.snapshot();
    assert_eq!(d.exit.as_deref(), Some("exit:5"));
    assert!(d.exited_unprompted);
    assert!(
        matches!(d.transport_close, Some("eof" | "process_exited")),
        "{d:?}"
    );
    assert!(d.stderr_bytes > cortex_engine::engine::STDERR_TAIL as u64);
    assert!(d.stderr_tail.len() <= cortex_engine::engine::STDERR_TAIL + 64);
    assert!(d.stderr_tail.contains("fixture crash"), "{}", d.stderr_tail);
    for secret in [
        "sk-fixture",
        "abcdefghijklmnop0123",
        "fixture-bearer-value",
        "\u{1b}",
    ] {
        assert!(
            !d.stderr_tail.contains(secret),
            "{secret}: {}",
            d.stderr_tail
        );
    }
    // A session id stays readable for correlation.
    assert!(d
        .stderr_tail
        .contains("89a921a6-3f1e-42ed-9a9e-aaec73cb7ed6"));
}
#[test]
fn redaction_keeps_ordinary_diagnostics_readable() {
    use cortex_engine::engine::redact;
    let text = "[session/create] sessionId=89a921a6-3f1e-42ed-9a9e-aaec73cb7ed6 phase=sdk-initialize durationMs=17645\n\
        token=abc Authorization: Bearer xyz.123 \"apiKey\":\"plain\" sk-ant-api03-ABCDEFGH ghp_0123456789abcdef\n\
        /usr/lib/cortex-workspace/engines/node_modules/x/index.js:12 Error: invalid transport\n\
        digest 0123456789abcdef0123456789abcdef0123";
    let out = redact(text);
    for kept in [
        "sessionId=89a921a6-3f1e-42ed-9a9e-aaec73cb7ed6",
        "phase=sdk-initialize",
        "durationMs=17645",
        "/usr/lib/cortex-workspace/engines/node_modules/x/index.js:12",
        "Error: invalid transport",
        "token=[redacted]",
        "Authorization: Bearer [redacted]",
        "\"apiKey\":\"[redacted]\"",
    ] {
        assert!(out.contains(kept), "{kept}: {out}");
    }
    for gone in [
        "abc ",
        "xyz",
        "plain",
        "sk-ant",
        "ghp_",
        "0123456789abcdef0123456789abcdef0123",
    ] {
        assert!(!out.contains(gone), "{gone}: {out}");
    }
}
#[tokio::test]
async fn abandoned_prompt_poison_and_session_ownership() {
    let e = launch("").await;
    e.initialize(D).await.unwrap();
    let s = e
        .start_session(&std::env::current_dir().unwrap(), vec![], D)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(40), e.prompt(&s, "hang"))
            .await
            .is_err()
    );
    assert!(matches!(
        e.prompt(&s, "must not replay").await,
        Err(Error::Busy)
    ));
    let other = launch("").await;
    other.initialize(D).await.unwrap();
    assert!(matches!(
        other.prompt(&s, "wrong owner").await,
        Err(Error::Forbidden)
    ));
    other.shutdown().await.unwrap();
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn permission_policy_selects_only_offered_option() {
    for option in ["yes", "invented"] {
        let p = Arc::new(Policy {
            calls: AtomicUsize::new(0),
            option,
        });
        let e = Engine::launch(spec("", EngineKind::Claude), p.clone())
            .await
            .unwrap();
        e.initialize(D).await.unwrap();
        let s = e
            .start_session(&std::env::current_dir().unwrap(), vec![], D)
            .await
            .unwrap();
        e.prompt(&s, "permission").await.unwrap();
        let Some(Event::Update(v)) = e.next_event().await else {
            panic!()
        };
        assert_eq!(
            v["update"]["content"]["text"],
            if option == "yes" {
                "approved"
            } else {
                "denied"
            }
        );
        assert_eq!(p.calls.load(Ordering::SeqCst), 1);
        e.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn unknown_creation_and_initialize_never_retry() {
    let e = launch("session-hang").await;
    e.initialize(D).await.unwrap();
    assert!(matches!(
        e.start_session(
            &std::env::current_dir().unwrap(),
            vec![],
            Duration::from_millis(30)
        )
        .await,
        Err(Error::Unknown)
    ));
    assert!(matches!(
        e.start_session(&std::env::current_dir().unwrap(), vec![], D)
            .await,
        Err(Error::Busy)
    ));
    e.shutdown().await.unwrap();
    let e = launch("initialize-hang").await;
    assert!(matches!(
        e.initialize(Duration::from_millis(30)).await,
        Err(Error::Unknown)
    ));
    assert!(matches!(e.initialize(D).await, Err(Error::Busy)));
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn closed_malformed_and_overflow_are_not_success() {
    for input in ["exit", "malformed", "flood"] {
        let e = launch("").await;
        e.initialize(D).await.unwrap();
        let s = e
            .start_session(&std::env::current_dir().unwrap(), vec![], D)
            .await
            .unwrap();
        assert!(tokio::time::timeout(D, e.prompt(&s, input))
            .await
            .unwrap()
            .is_err());
        tokio::time::timeout(D, e.shutdown())
            .await
            .unwrap()
            .unwrap();
    }
}
#[tokio::test]
async fn drop_supervises_owned_process() {
    let e = launch("").await;
    let pid = e.pid();
    drop(e);
    tokio::time::timeout(D, async {
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn channel_revocation_terminates_native_engine_and_pending_prompt() {
    struct Toggle(std::sync::atomic::AtomicBool);
    impl PermissionPolicy for Toggle {
        fn authority(&self) -> FutureResult<'_, ()> {
            Box::pin(async move {
                if self.0.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(Error::Forbidden)
                }
            })
        }
        fn decide<'a>(
            &'a self,
            _: &'a str,
            _: &'a Value,
            _: u64,
            _: &'a Value,
        ) -> FutureResult<'a, PermissionDecision> {
            Box::pin(async { Ok(PermissionDecision::Cancelled) })
        }
    }
    let policy = Arc::new(Toggle(std::sync::atomic::AtomicBool::new(true)));
    let e = Engine::launch(spec("", EngineKind::Claude), policy.clone())
        .await
        .unwrap();
    e.initialize(D).await.unwrap();
    let s = e
        .start_session(&std::env::current_dir().unwrap(), vec![], D)
        .await
        .unwrap();
    {
        let prompt = e.prompt(&s, "hang");
        tokio::pin!(prompt);
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(20))=>{},_=&mut prompt=>panic!()}
        policy.0.store(false, Ordering::SeqCst);
        assert!(tokio::time::timeout(D, &mut prompt).await.unwrap().is_err());
    }
    tokio::time::timeout(D, e.shutdown())
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn cancellation_invalidates_pending_permission() {
    struct Gate(Arc<tokio::sync::Notify>);
    impl PermissionPolicy for Gate {
        fn authority(&self) -> FutureResult<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn decide<'a>(
            &'a self,
            _: &'a str,
            _: &'a Value,
            _: u64,
            _: &'a Value,
        ) -> FutureResult<'a, PermissionDecision> {
            Box::pin(async move {
                self.0.notified().await;
                Ok(PermissionDecision::Selected("yes".into()))
            })
        }
    }
    let gate = Arc::new(tokio::sync::Notify::new());
    let e = Engine::launch(spec("", EngineKind::Codex), Arc::new(Gate(gate.clone())))
        .await
        .unwrap();
    e.initialize(D).await.unwrap();
    let s = e
        .start_session(&std::env::current_dir().unwrap(), vec![], D)
        .await
        .unwrap();
    {
        let prompt = e.prompt(&s, "permission");
        tokio::pin!(prompt);
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(20))=>{},_=&mut prompt=>panic!()}
        e.cancel(&s).await.unwrap();
        gate.notify_one();
        tokio::time::timeout(D, &mut prompt).await.unwrap().unwrap();
    }
    let Some(Event::Update(v)) = e.next_event().await else {
        panic!()
    };
    assert_eq!(v["update"]["content"]["text"], "denied");
    e.shutdown().await.unwrap();
}
#[tokio::test]
async fn opencode_fake_session_and_native_data_home() {
    let directory = std::env::temp_dir().join(format!(
        "cortex-opencode-fixture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let credentials = directory.join("opencode");
    std::fs::create_dir(&credentials).unwrap();
    let mut launch_spec = spec("", EngineKind::OpenCode);
    launch_spec.native_home = directory.clone();
    launch_spec.credential_directory = credentials;
    let env = launch_spec.environment().unwrap();
    assert_eq!(env["XDG_DATA_HOME"], directory.to_str().unwrap());
    let e = Engine::launch(
        launch_spec,
        Arc::new(Policy {
            calls: AtomicUsize::new(0),
            option: "yes",
        }),
    )
    .await
    .unwrap();
    e.initialize(D).await.unwrap();
    let s = e
        .start_session(&std::env::current_dir().unwrap(), vec![], D)
        .await
        .unwrap();
    e.prompt(&s, "opencode fixture").await.unwrap();
    e.shutdown().await.unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn native_home_environment_narrows_secrets_and_relative_path() {
    for kind in [EngineKind::Codex, EngineKind::Claude] {
        let mut s = spec("", kind);
        s.base_environment = BTreeMap::from([
            ("PATH".into(), "/usr/bin:.:relative:/bin".into()),
            ("CORTEX_CHANNEL_SECRET".into(), "do-not-forward".into()),
            ("ANTHROPIC_API_KEY".into(), "do-not-forward".into()),
            ("HOME".into(), "/wrong".into()),
        ]);
        let env = s.environment().unwrap();
        assert_eq!(env["PATH"], "/usr/bin:/bin");
        assert!(!env.contains_key("CORTEX_CHANNEL_SECRET"));
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
        assert_eq!(env["HOME"], s.native_home.to_str().unwrap());
        assert_eq!(
            env[if kind == EngineKind::Codex {
                "CODEX_HOME"
            } else {
                "CLAUDE_CONFIG_DIR"
            }],
            s.credential_directory.to_str().unwrap()
        );
    }
}
/// The installed claude-agent-acp (0.107.0) through this engine host: initialize,
/// `session/new` and the live routes' configure calls (`haiku`/`high` worker,
/// `fable`/`medium` coordinator, then the `default` mode floor), four sessions at
/// once, twice. No prompt is sent; the key is synthetic, the Claude config directory
/// disposable and the network cut off (`unshare -rn`). Every adapter sends
/// `available_commands_update` right behind its `session/new` reply; none of those
/// may close the transport. Explicitly ignored unless `CORTEX_REAL_CLAUDE_BUNDLE`
/// names an installed engine bundle; run with `--ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs CORTEX_REAL_CLAUDE_BUNDLE (an installed engine bundle) and unshare; run with --ignored"]
async fn real_claude_sessions_configure_concurrently_without_transport_loss() {
    let bundle = PathBuf::from(
        std::env::var_os("CORTEX_REAL_CLAUDE_BUNDLE")
            .expect("CORTEX_REAL_CLAUDE_BUNDLE must name the installed engine bundle"),
    );
    let root = std::env::temp_dir().join(format!("cortex-real-claude-{}", std::process::id()));
    for dir in ["home", "claude", "project"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    let spec = |_: usize| LaunchSpec {
        engine: EngineKind::Claude,
        executable: PathBuf::from("/usr/bin/unshare"),
        args: vec![
            "-rn".into(),
            "--".into(),
            "/usr/bin/env".into(),
            "ANTHROPIC_API_KEY=sk-ant-synthetic-offline-not-a-key".into(),
            bundle.join("node/bin/node").to_string_lossy().into_owned(),
            bundle
                .join("node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js")
                .to_string_lossy()
                .into_owned(),
        ],
        cwd: root.join("project"),
        native_home: root.join("home"),
        credential_directory: root.join("claude"),
        base_environment: BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ]),
        codex_launcher: None,
    };
    let deadline = Duration::from_secs(90);
    let mut summary = Vec::new();
    for round in 0..2 {
        let mut runs = Vec::new();
        for n in 0..4 {
            let (model, effort) = if n % 2 == 0 {
                ("haiku", "high")
            } else {
                ("fable", "medium")
            };
            let launch = spec(n);
            let cwd = root.join("project");
            runs.push(tokio::spawn(async move {
                let e = Engine::launch(
                    launch,
                    Arc::new(Policy {
                        calls: AtomicUsize::new(0),
                        option: "no",
                    }),
                )
                .await
                .unwrap();
                let diagnostics = e.diagnostics();
                let result = async {
                    e.initialize(deadline).await?;
                    let s = e.start_session(&cwd, vec![], deadline).await?;
                    e.configure(&s, "model", model, deadline).await?;
                    e.configure(&s, "thought_level", effort, deadline).await?;
                    e.configure(&s, "mode", "default", deadline).await?;
                    let mut updates = 0;
                    for event in e.drain_events().await {
                        if matches!(event, Event::Update(_)) {
                            updates += 1;
                        }
                    }
                    Ok::<_, Error>(updates)
                }
                .await;
                let _ = e.shutdown().await;
                (round, n, model, effort, result, diagnostics.snapshot())
            }));
        }
        for run in runs {
            let (round, n, model, effort, result, d) = run.await.unwrap();
            assert!(
                result.is_ok() && d.transport_close == Some("host_shutdown"),
                "round {round} session {n} ({model}/{effort}): {result:?} {d:?}"
            );
            summary.push(format!(
                "{round}.{n} {model}/{effort} updates={}",
                result.unwrap()
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    println!("real claude-agent-acp sessions configured: {summary:?}");
}
