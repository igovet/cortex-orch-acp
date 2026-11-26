//! Short model texts through the ACP engine route, against the no-model fake
//! engine. The test executable doubles as that engine (`fake-engine.rs`): no
//! model, network or real engine is ever started.
#[path = "support/hermetic.rs"]
mod hermetic;
#[path = "../../cortex-engine/src/bin/fake-engine.rs"]
mod peer;

use cortex_engine::environment::{EngineKind, LaunchSpec};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
use workspace_service::short_text_engine::{complete, side_answer};

fn private_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "cw-short-text-engine-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&path)
        .unwrap();
    path.canonicalize().unwrap()
}
fn launch(engine: EngineKind, home: &std::path::Path) -> LaunchSpec {
    LaunchSpec {
        engine,
        executable: std::env::current_exe().unwrap(),
        args: vec!["short-text-peer".into()],
        // Replaced by the run's own scratch directory.
        cwd: home.to_owned(),
        native_home: home.to_owned(),
        credential_directory: home.to_owned(),
        base_environment: BTreeMap::new(),
        codex_launcher: None,
        confinement: None,
    }
}
/// Children of this process that are still alive or unreaped.
fn children() -> Vec<String> {
    let mut found = Vec::new();
    for task in std::fs::read_dir("/proc/self/task").unwrap().flatten() {
        let list = std::fs::read_to_string(task.path().join("children")).unwrap_or_default();
        found.extend(list.split_whitespace().map(str::to_owned));
    }
    found
}
fn scratch_dirs() -> usize {
    std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("cortex-short-text-")
        })
        .count()
}

async fn scenarios() {
    let home = private_dir("home");
    let low = BTreeMap::from([("thought_level".to_owned(), "low".to_owned())]);
    let before = scratch_dirs();

    // One prompt, one answer: the fake echoes the prompt as the agent's message.
    // The route's model, its low effort and the read-only mode are all set first:
    // the fake refuses an option or value it does not advertise.
    let answer = complete(
        launch(EngineKind::Claude, &home),
        "fixture-model",
        &low,
        "Retry failed uploads",
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(answer.as_deref(), Ok("Retry failed uploads"));
    assert!(
        children().is_empty(),
        "engine left running: {:?}",
        children()
    );

    // A model that is not offered is never substituted.
    let answer = complete(
        launch(EngineKind::Claude, &home),
        "not-offered",
        &BTreeMap::new(),
        "x",
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(answer, Err("engine_failed"));

    // An engine asking for a permission is denied, and its answer is not used.
    let answer = complete(
        launch(EngineKind::Claude, &home),
        "fixture-model",
        &BTreeMap::new(),
        "permission",
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(answer, Err("permission_requested"));

    // An engine that never answers is stopped at the budget and reaped.
    let started = std::time::Instant::now();
    let answer = complete(
        launch(EngineKind::Claude, &home),
        "fixture-model",
        &BTreeMap::new(),
        "hang",
        Duration::from_millis(1500),
    )
    .await;
    assert_eq!(answer, Err("timeout"));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(
        children().is_empty(),
        "engine left running: {:?}",
        children()
    );

    // An engine that exits mid-turn gives no text.
    let answer = complete(
        launch(EngineKind::Claude, &home),
        "fixture-model",
        &BTreeMap::new(),
        "exit",
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(answer, Err("engine_failed"));

    // A missing engine is not started at all.
    let mut missing = launch(EngineKind::Claude, &home);
    missing.executable = home.join("no-such-engine");
    let answer = complete(
        missing,
        "fixture-model",
        &BTreeMap::new(),
        "x",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(answer, Err("engine_unavailable"));

    // A Codex route without Cortex's verified native launcher (which turns the
    // owner's MCP servers off and checks it) is refused before any process starts.
    let answer = complete(
        launch(EngineKind::Codex, &home),
        "fixture-model",
        &BTreeMap::new(),
        "x",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(answer, Err("codex_hardening_unavailable"));
    let answer = complete(
        launch(EngineKind::OpenCode, &home),
        "fixture-model",
        &BTreeMap::new(),
        "x",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(answer, Err("engine_unavailable"));

    // A hardened Codex launch whose engine groups its modes (built-in sandbox
    // modes, then the worker permission profile) still gets its built-in
    // read-only mode and answers; the profile mode is never chosen.
    let mut profiled = codex_launch(&home);
    profiled.args = vec!["short-text-peer-profiled".into()];
    let answer = complete(
        profiled,
        "fixture-model",
        &BTreeMap::new(),
        "Retry failed uploads",
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(answer.as_deref(), Ok("Retry failed uploads"));
    // A grouped list without any read-only mode is still refused.
    let mut wide = codex_launch(&home);
    wide.args = vec!["short-text-peer-profiled-wide".into()];
    let answer = complete(
        wide,
        "fixture-model",
        &BTreeMap::new(),
        "Retry failed uploads",
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(answer, Err("no_read_only_mode"));

    side_chat(&home).await;

    // Every run's private scratch directory is gone.
    assert_eq!(scratch_dirs(), before);
    std::fs::remove_dir_all(&home).unwrap();
}

/// The side chat's read-only run: rooted at the task checkout on Claude, with the
/// mutating, network and delegating tools removed, read and search calls only.
async fn side_chat(home: &std::path::Path) {
    let checkout = private_dir("checkout");
    std::fs::write(checkout.join("kept.txt"), "unchanged").unwrap();
    let ask = |prompt: &'static str| {
        let (launch, checkout) = (launch(EngineKind::Claude, home), checkout.clone());
        async move {
            side_answer(
                launch,
                "fixture-model",
                &BTreeMap::new(),
                "Side chat instructions.",
                prompt,
                Duration::from_secs(20),
                &checkout,
            )
            .await
        }
    };
    // The session and the engine process both work in the task's checkout.
    let folder = checkout.display().to_string();
    assert_eq!(ask("working-folder").await, Ok(format!("{folder}|{folder}")));
    // A short text never does: it works in its own scratch directory.
    let short = complete(
        launch(EngineKind::Claude, home),
        "fixture-model",
        &BTreeMap::new(),
        "working-folder",
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert!(!short.contains(&folder), "{short}");
    // Shell, edit, write, delegation and network tools are removed; Read stays; no
    // MCP server beyond the ones Cortex passes (none) and no bypass.
    for pointer in [
        "disallowed:Bash",
        "disallowed:Edit",
        "disallowed:Write",
        "disallowed:Agent",
        "disallowed:WebFetch",
        "disallowed:WebSearch",
    ] {
        let prompt: &'static str = Box::leak(
            format!("session-meta:{{\"{pointer}\":true,\"/claudeCode/options/strictMcpConfig\":true,\"/claudeCode/options/allowDangerouslySkipPermissions\":false}}")
                .into_boxed_str(),
        );
        // The fake ends the turn without text when every pointer holds.
        assert_eq!(ask(prompt).await, Err("no_answer"), "{pointer}");
    }
    assert_eq!(
        ask("session-meta:{\"disallowed:Read\":true}").await,
        Err("model_refused"),
        "Read must stay available"
    );
    // Read and search calls are accepted; any other tool call drops the answer.
    assert_eq!(ask("tool-kind:read").await.as_deref(), Ok("Answered after a tool."));
    assert_eq!(ask("tool-kind:search").await.as_deref(), Ok("Answered after a tool."));
    for kind in ["tool-kind:edit", "tool-kind:execute", "tool-kind:delete", "tool-kind:fetch", "tool-kind:none"] {
        assert_eq!(ask(kind).await, Err("tool_call"), "{kind}");
    }
    // A short text accepts no tool call at all, not even a read.
    let short = complete(
        launch(EngineKind::Claude, home),
        "fixture-model",
        &BTreeMap::new(),
        "tool-kind:read",
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(short, Err("tool_call"));
    // A permission request (a credential file, a path outside the checkout) is
    // answered no, and the model answers without it.
    assert_eq!(ask("permission").await.as_deref(), Ok("denied"));
    // No file of the checkout changed.
    assert_eq!(std::fs::read_to_string(checkout.join("kept.txt")).unwrap(), "unchanged");
    assert_eq!(std::fs::read_dir(&checkout).unwrap().count(), 1);
    // A missing checkout, an unhardened Codex and an unsupported engine are refused before any start.
    let missing = side_answer(
        launch(EngineKind::Claude, home),
        "fixture-model",
        &BTreeMap::new(),
        "x",
        "x",
        Duration::from_secs(5),
        &checkout.join("absent"),
    )
    .await;
    assert_eq!(missing, Err("no_checkout"));
    let codex = side_answer(
        launch(EngineKind::Codex, home),
        "fixture-model",
        &BTreeMap::new(),
        "x",
        "x",
        Duration::from_secs(5),
        &checkout,
    )
    .await;
    assert_eq!(codex, Err("codex_hardening_unavailable"));
    assert!(workspace_service::short_text_engine::side_chat_reads_files(EngineKind::Claude));
    assert!(workspace_service::short_text_engine::side_chat_reads_files(EngineKind::Codex));
    codex_side_chat(home, &checkout).await;
    assert!(children().is_empty(), "engine left running: {:?}", children());
    std::fs::remove_dir_all(&checkout).unwrap();
}

fn main() {
    // Started by a Codex side-chat run as its read-only project files server.
    workspace_service::side_chat_files::dispatch();
    // Started by a run as the engine. The fake has no mode of this name, so it echoes prompts.
    if std::env::args().nth(1).as_deref() == Some("short-text-peer") {
        peer::main();
        return;
    }
    // The same engine reporting codex-acp's grouped mode list, as it does once the
    // native Codex launcher defines the worker permission profile. The marker goes
    // into the run's own scratch working directory, removed with it.
    // `-wide` offers only the wide sandbox modes in that grouped list.
    let profiled = std::env::args().nth(1);
    if matches!(
        profiled.as_deref(),
        Some("short-text-peer-profiled" | "short-text-peer-profiled-wide")
    ) {
        std::fs::write("codex-permission-profile", "").unwrap();
        if profiled.as_deref() == Some("short-text-peer-profiled-wide") {
            std::fs::write("no-restricted-modes", "").unwrap();
        }
        peer::main();
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        tokio::time::timeout(Duration::from_secs(120), scenarios())
            .await
            .expect("short text engine scenarios exceeded deadline");
    });
    println!("short text engine: answer, refused model, denied permission, timeout, exit, missing engine, unhardened Codex and the read-only side chat passed");
}

/// A hardened-launcher Codex launch against the fake (its launcher file stands in
/// for the native Codex launcher, which the fake never runs).
fn codex_launch(home: &std::path::Path) -> LaunchSpec {
    let marker = home.join("codex-launcher");
    std::fs::write(&marker, "#!/bin/false\n").unwrap();
    let mut launch = launch(EngineKind::Codex, home);
    launch.codex_launcher = Some(cortex_engine::environment::CodexLauncher::new(move || Ok(marker.clone())));
    launch
}

/// Codex reads the checkout through the side chat's own read-only MCP server,
/// handed to its App Server by the hardened launcher; short texts and Claude get none.
async fn codex_side_chat(home: &std::path::Path, checkout: &std::path::Path) {
    std::fs::write(checkout.join(".env"), "TOKEN=private\n").unwrap();
    std::fs::create_dir_all(checkout.join(".cortex")).unwrap();
    std::fs::write(checkout.join(".cortex/cortex.sqlite3"), "store").unwrap();
    std::fs::write(checkout.parent().unwrap().join("outside.txt"), "outside").unwrap();
    let ask = |prompt: String| {
        let (launch, checkout) = (codex_launch(home), checkout.to_owned());
        async move {
            side_answer(launch, "fixture-model", &BTreeMap::new(), "Side chat instructions.", &prompt, Duration::from_secs(20), &checkout).await
        }
    };
    let read = |path: &str| format!("mcp-read:read_file:{{\"path\":\"{path}\"}}");
    // A read inside the checkout succeeds, and the server's tool call is accepted.
    let answer = ask(read("kept.txt")).await.unwrap();
    assert!(answer.contains("unchanged") && answer.starts_with("kept.txt: lines 1-1"), "{answer}");
    // Code mode lists no MCP tool in any description: the Codex side chat's instructions say where the tools are.
    let told = ask("echo-instructions".into()).await.unwrap();
    assert!(told.starts_with("Side chat instructions."), "{told}");
    for name in ["read_file", "list_directory", "search_files"] {
        assert!(told.contains(&format!("tools.mcp__project_files__{name}(")), "{told}");
    }
    let listing = ask("mcp-read:list_directory:{\"path\":\".\"}".into()).await.unwrap();
    assert_eq!(listing, "kept.txt");
    // The same read on an engine with the grouped (permission profile) mode list.
    let mut profiled = codex_launch(home);
    profiled.args = vec!["short-text-peer-profiled".into()];
    let answer = side_answer(profiled, "fixture-model", &BTreeMap::new(), "Side chat instructions.", &read("kept.txt"), Duration::from_secs(20), checkout).await.unwrap();
    assert!(answer.contains("unchanged"), "{answer}");
    let found = ask("mcp-read:search_files:{\"pattern\":\"unchanged\"}".into()).await.unwrap();
    assert_eq!(found, "kept.txt:1: unchanged");
    // Escapes, private state, credentials and every write are refused by the server.
    for path in ["../outside.txt", "/etc/hostname", ".env", ".cortex/cortex.sqlite3", ".git/config"] {
        let answer = ask(read(path)).await.unwrap();
        assert!(answer.starts_with("error:Refused:"), "{path}: {answer}");
    }
    for tool in ["write_file", "edit_file", "run_command"] {
        let answer = ask(format!("mcp-read:{tool}:{{\"path\":\"kept.txt\",\"content\":\"x\"}}")).await.unwrap();
        assert!(answer.starts_with("error:Refused: unknown_tool"), "{tool}: {answer}");
    }
    assert_eq!(std::fs::read_to_string(checkout.join("kept.txt")).unwrap(), "unchanged");
    // The Codex side chat itself works in a private scratch directory, not the checkout.
    let folder = ask("working-folder".into()).await.unwrap();
    assert!(!folder.contains(&checkout.display().to_string()), "{folder}");
    // Only side-chat sessions get the server: never a short text, never Claude.
    let short = complete(codex_launch(home), "fixture-model", &BTreeMap::new(), &read("kept.txt"), Duration::from_secs(20)).await;
    assert_eq!(short.as_deref(), Ok("no-mcp"));
    let claude = side_answer(launch(EngineKind::Claude, home), "fixture-model", &BTreeMap::new(), "x", &read("kept.txt"), Duration::from_secs(20), checkout).await;
    assert_eq!(claude.as_deref(), Ok("no-mcp"));
    assert!(children().is_empty(), "engine left running: {:?}", children());
    std::fs::remove_file(checkout.parent().unwrap().join("outside.txt")).unwrap();
}
