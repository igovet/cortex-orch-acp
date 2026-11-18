//! One short model text (a task title, a commit message, a pull request text)
//! through an ordinary ACP engine session, off the task lifecycle.
//!
//! The launch is the route's [`crate::native_routes::SelectedRoute::session_launch`],
//! the same hardened launch every engine session uses: a cleared environment with
//! the real `HOME` and the connection's own native directory, and for Codex the
//! pinned native launcher, which turns every owner MCP server off, checks that
//! with `codex mcp list` and the disabled features with `codex features list`
//! before App Server starts (`native_codex_runtime::verify`), and gives App
//! Server only a non-persistent trust layer. A Codex route without that launcher
//! is refused, never run unchecked.
//!
//! The session works in an empty private scratch directory that is removed
//! afterwards, gets no ACP MCP servers, is set to the route's model, the route's
//! parameters and the engine's read-only mode (`read-only` or `plan`; an engine
//! offering neither is refused), and receives one text prompt. Every permission
//! request is answered with the reject option, or cancelled when none is
//! offered, and makes the answer unusable; so does any tool call. Only the
//! session's `agent_message_chunk` text is kept. The whole run is bounded by its
//! budget, then the engine is shut down and its process group reaped. Nothing is
//! written to a project store, no runtime task or channel exists, and the
//! session is never shown in a task's history.
use cortex_engine::{
    engine::{Engine, Event, PermissionDecision, PermissionPolicy},
    environment::{EngineKind, LaunchSpec},
    FutureResult,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

/// The longest answer kept.
const MAX_ANSWER: usize = 64 * 1024;
/// One ACP request inside the run (initialize, session/new, configure).
const STEP: Duration = Duration::from_secs(15);

/// Denies everything and remembers that it was asked.
struct DenyAll(Arc<AtomicBool>);
impl PermissionPolicy for DenyAll {
    fn authority(&self) -> FutureResult<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn decide<'a>(
        &'a self,
        _: &'a str,
        _: &'a Value,
        _: u64,
        request: &'a Value,
    ) -> FutureResult<'a, PermissionDecision> {
        self.0.store(true, Ordering::SeqCst);
        let reject = cortex_engine::permission::reject_option(request);
        Box::pin(async move {
            Ok(match reject {
                Some(option) => PermissionDecision::Selected(option),
                None => PermissionDecision::Cancelled,
            })
        })
    }
}

/// An empty private directory, removed with everything in it when dropped.
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let path = std::env::temp_dir().join(format!("cortex-short-text-{}", crate::identity()?));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read_only_mode(options: &Value) -> Option<&'static str> {
    ["read-only", "plan"].into_iter().find(|m| {
        options.as_array().is_some_and(|a| {
            a.iter().any(|o| {
                o["id"] == "mode"
                    && o["options"]
                        .as_array()
                        .is_some_and(|v| v.iter().any(|v| v["value"] == *m))
            })
        })
    })
}

/// One prompt on `launch` with `model` and `parameters`. The answer text, or the
/// reason there is none: `codex_hardening_unavailable`, `no_private_directory`,
/// `engine_unavailable`, `engine_failed`, `no_read_only_mode`, `timeout`,
/// `permission_requested`, `tool_call`, `model_refused`, `no_answer`.
pub async fn complete(
    mut launch: LaunchSpec,
    model: &str,
    parameters: &BTreeMap<String, String>,
    prompt: &str,
    budget: Duration,
) -> Result<String, &'static str> {
    if launch.engine == EngineKind::Codex && launch.codex_launcher.is_none() {
        return Err("codex_hardening_unavailable");
    }
    if !matches!(launch.engine, EngineKind::Codex | EngineKind::Claude) {
        return Err("engine_unavailable");
    }
    let scratch = Scratch::new().map_err(|_| "no_private_directory")?;
    launch.cwd = scratch.0.clone();
    let claude = launch.engine == EngineKind::Claude;
    let asked = Arc::new(AtomicBool::new(false));
    let engine = Engine::launch(launch, Arc::new(DenyAll(asked.clone())))
        .await
        .map_err(|_| "engine_unavailable")?;
    let step = STEP.min(budget);
    let turn = async {
        engine.initialize(step).await.map_err(|_| "engine_failed")?;
        let meta = claude.then(crate::native_engine_policy::claude_session_meta);
        let session = engine
            .start_session_with_meta(&scratch.0, vec![], meta, step)
            .await
            .map_err(|_| "engine_failed")?;
        engine
            .configure(&session, "model", model, step)
            .await
            .map_err(|_| "engine_failed")?;
        for (key, value) in parameters {
            if key != "mode" {
                engine
                    .configure(&session, key, value, step)
                    .await
                    .map_err(|_| "engine_failed")?;
            }
        }
        let mode = read_only_mode(&session.config_options()).ok_or("no_read_only_mode")?;
        engine
            .configure(&session, "mode", mode, step)
            .await
            .map_err(|_| "engine_failed")?;
        let mut text = String::new();
        let mut acted = false;
        let mut collect = |params: &Value| {
            let update = &params["update"];
            match update["sessionUpdate"].as_str() {
                Some("agent_message_chunk") if update["content"]["type"] == "text" => {
                    if text.len() <= MAX_ANSWER {
                        text.push_str(update["content"]["text"].as_str().unwrap_or(""));
                    }
                }
                Some("tool_call" | "tool_call_update") => acted = true,
                _ => {}
            }
        };
        let pending = engine.prompt(&session, prompt);
        tokio::pin!(pending);
        let reply = loop {
            tokio::select! {
                reply = &mut pending => break reply,
                event = engine.next_event() => match event {
                    Some(Event::Update(params)) => collect(&params),
                    Some(Event::TransportClosed) | None => break Err(cortex_engine::Error::Closed),
                    _ => {}
                }
            }
        };
        for event in engine.drain_events().await {
            if let Event::Update(params) = event {
                collect(&params);
            }
        }
        let reply = reply.map_err(|_| "engine_failed")?;
        if asked.load(Ordering::SeqCst) {
            return Err("permission_requested");
        }
        if acted {
            return Err("tool_call");
        }
        if reply["stopReason"] != "end_turn" {
            return Err("model_refused");
        }
        let text = text.trim().to_owned();
        if text.is_empty() || text.len() > MAX_ANSWER {
            return Err("no_answer");
        }
        Ok(text)
    };
    let outcome = tokio::time::timeout(budget, turn).await;
    let _ = engine.shutdown().await;
    drop(scratch);
    outcome.unwrap_or(Err("timeout"))
}
