//! Short model texts through the ACP engine route, against the no-model fake
//! engine. The test executable doubles as that engine (`fake-engine.rs`): no
//! model, network or real engine is ever started.
#[path = "support/hermetic.rs"]
mod hermetic;
#[path = "../../cortex-engine/src/bin/fake-engine.rs"]
mod peer;

use cortex_engine::environment::{EngineKind, LaunchSpec};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
use workspace_service::short_text_engine::complete;

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

    // Every run's private scratch directory is gone.
    assert_eq!(scratch_dirs(), before);
    std::fs::remove_dir_all(&home).unwrap();
}

fn main() {
    // Started by a run as the engine. The fake has no mode of this name, so it echoes prompts.
    if std::env::args().nth(1).as_deref() == Some("short-text-peer") {
        peer::main();
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        tokio::time::timeout(Duration::from_secs(120), scenarios())
            .await
            .expect("short text engine scenarios exceeded deadline");
    });
    println!("short text engine: answer, refused model, denied permission, timeout, exit, missing engine and unhardened Codex passed");
}
