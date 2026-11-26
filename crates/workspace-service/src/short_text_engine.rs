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
//!
//! [`side_answer`] is the same run for the task side chat, with one difference:
//! on Claude it works in the task's checkout so Claude Code's native read tools
//! (Read, Grep, Glob) can read it, still read-only and without MCP, shell,
//! edit, network or delegation tools (see its documentation).
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

/// Denies everything and remembers that it was asked. A side-chat session (`.1`)
/// is allowed the tools of its own read-only project files server only.
struct DenyAll(Arc<AtomicBool>, bool);
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
        if self.1 && side_files_call(&request["toolCall"]) {
            if let Some(allow) = cortex_engine::permission::allow_option(request) {
                return Box::pin(async move { Ok(PermissionDecision::Selected(allow)) });
            }
        }
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

/// The engine's built-in read-only mode, matched by its exact value. Once the
/// native Codex launcher defines the worker permission profile, codex-acp
/// (2.6.0) groups its mode list (`sandbox-modes` holding `read-only` and the
/// other built-in modes, `permission-profiles` holding
/// `permission-profile:cortex-workspace`), so the values are searched inside
/// groups too. Only the exact built-in value is taken, never a profile mode:
/// codex-acp sends `read-only` as a per-turn `readOnly` sandbox policy with
/// approval on request, whatever `default_permissions` the launch layers set.
fn read_only_mode(options: &Value) -> Option<&'static str> {
    fn listed(values: &Value, mode: &str) -> bool {
        values.as_array().is_some_and(|values| {
            values.iter().any(|v| {
                v["value"] == mode || (v.get("group").is_some() && listed(&v["options"], mode))
            })
        })
    }
    ["read-only", "plan"].into_iter().find(|m| {
        options
            .as_array()
            .is_some_and(|a| a.iter().any(|o| o["id"] == "mode" && listed(&o["options"], m)))
    })
}

/// One prompt on `launch` with `model` and `parameters`. The answer text, or the
/// reason there is none: `codex_hardening_unavailable`, `no_private_directory`,
/// `engine_unavailable`, `engine_failed`, `no_read_only_mode`, `timeout`,
/// `permission_requested`, `tool_call`, `model_refused`, `no_answer`.
pub async fn complete(
    launch: LaunchSpec,
    model: &str,
    parameters: &BTreeMap<String, String>,
    prompt: &str,
    budget: Duration,
) -> Result<String, &'static str> {
    let system = "Generate the requested short editable text. Use the supplied context. Respond with text only.";
    turn(launch, model, parameters, system, prompt, budget, None).await
}

/// Native Claude Code tools a side-chat session also goes without: it reads
/// the checkout and nothing else, so it fetches and searches no network.
pub const SIDE_CHAT_DISALLOWED_TOOLS: &[&str] = &["WebFetch", "WebSearch"];

/// Whether a side-chat run on `engine` reads the checkout: Claude through its
/// native Read, Grep and Glob, Codex (whose native shell is off) through the
/// session's own read-only project files server ([`crate::side_chat_files`]).
pub fn side_chat_reads_files(engine: EngineKind) -> bool {
    matches!(engine, EngineKind::Claude | EngineKind::Codex)
}

/// A tool call (or a permission request's `toolCall`) of the side chat's own
/// read-only project files server.
fn side_files_call(call: &Value) -> bool {
    let server = crate::side_chat_files::SERVER;
    let named = |v: &Value| {
        v.as_str().is_some_and(|n| {
            n.starts_with(&format!("mcp__{server}__"))
                || n.starts_with(&format!("mcp.{server}."))
                || n.starts_with(&format!("{server}."))
        })
    };
    let raw = call.get("rawInput").or_else(|| call.get("raw_input")).unwrap_or(&Value::Null);
    raw["server"] == server
        || named(&call["name"])
        || named(&call["title"])
        || named(&call["_meta"]["claudeCode"]["toolName"])
}

/// One side-chat answer: [`complete`]'s hardened, read-only session with its
/// own `system` instructions, reading the task's `checkout`.
///
/// On Claude the session works in `checkout` (Claude Code's native Read, Grep
/// and Glob read it without asking; Cortex private stores stay denied,
/// credential files and every path outside the checkout ask, and every such
/// request is rejected; Bash, Edit, Write and the other mutating or delegating
/// tools, plus WebFetch and WebSearch, are removed from the session).
///
/// On Codex the session works in a private scratch directory and its App
/// Server gets exactly one MCP server, the read-only project files server of
/// [`crate::side_chat_files`] rooted at `checkout`, through the hardened
/// launcher's verified relay layers (never ACP `session/new` servers); its
/// permission requests are allowed, every other one is rejected.
///
/// On both, only `read` and `search` calls and calls of that server are
/// accepted; any other tool call makes the answer unusable (`tool_call`). A
/// rejected permission request does not: the model is told no and answers
/// without that file.
pub async fn side_answer(
    launch: LaunchSpec,
    model: &str,
    parameters: &BTreeMap<String, String>,
    system: &str,
    prompt: &str,
    budget: Duration,
    checkout: &std::path::Path,
) -> Result<String, &'static str> {
    if !checkout.is_absolute() || !checkout.is_dir() {
        return Err("no_checkout");
    }
    turn(launch, model, parameters, system, prompt, budget, Some(checkout)).await
}

async fn turn(
    mut launch: LaunchSpec,
    model: &str,
    parameters: &BTreeMap<String, String>,
    system: &str,
    prompt: &str,
    budget: Duration,
    checkout: Option<&std::path::Path>,
) -> Result<String, &'static str> {
    if launch.engine == EngineKind::Codex && launch.codex_launcher.is_none() {
        return Err("codex_hardening_unavailable");
    }
    if !matches!(launch.engine, EngineKind::Codex | EngineKind::Claude) {
        return Err("engine_unavailable");
    }
    let scratch = Scratch::new().map_err(|_| "no_private_directory")?;
    let claude = launch.engine == EngineKind::Claude;
    // A Claude side chat reads in the checkout; everything else works in the scratch.
    let side = checkout.is_some();
    let reading = checkout.filter(|_| claude).map(std::path::Path::to_path_buf);
    if let Some(checkout) = checkout.filter(|_| !claude) {
        let server = crate::side_chat_files::entry(checkout).ok_or("no_file_server")?;
        let launcher = launch.codex_launcher.take().ok_or("codex_hardening_unavailable")?;
        launch.codex_launcher = Some(launcher.with_mcp_servers(vec![server]));
    }
    // Codex's code mode does not list MCP tools in any tool description: say where they are.
    let codex_system;
    let system = if side && !claude {
        codex_system = format!("{system}\n\n{}", crate::side_chat_files::codex_cheat_sheet());
        codex_system.as_str()
    } else {
        system
    };
    let cwd = reading.clone().unwrap_or_else(|| scratch.0.clone());
    launch.cwd = cwd.clone();
    let asked = Arc::new(AtomicBool::new(false));
    let prompt_dir = Scratch::new().map_err(|_| "no_private_directory")?;
    let path = prompt_dir.0.join("model-instructions.md");
    if !claude {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| f.write_all(system.as_bytes()))
            .map_err(|_| "no_private_directory")?;
    }
    let engine = Engine::launch_with_model_instructions(
        launch,
        Arc::new(DenyAll(asked.clone(), side)),
        (!claude).then_some(path.as_path()),
    )
    .await
    .map_err(|_| "engine_unavailable")?;
    let step = STEP.min(budget);
    let turn = async {
        engine.initialize(step).await.map_err(|_| "engine_failed")?;
        let meta = claude.then(|| {
            let mut meta = crate::native_engine_policy::claude_session_meta();
            meta["systemPrompt"] = Value::from(system);
            if side {
                if let Some(tools) =
                    meta["claudeCode"]["options"]["disallowedTools"].as_array_mut()
                {
                    tools.extend(SIDE_CHAT_DISALLOWED_TOOLS.iter().map(|t| Value::from(*t)));
                }
            }
            meta
        });
        let session = engine
            .start_session_with_meta(&cwd, vec![], meta, step)
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
        // Read and search calls a side chat made, by id; their updates may omit the kind.
        let mut reads = std::collections::BTreeSet::<String>::new();
        let mut collect = |params: &Value| {
            let update = &params["update"];
            match update["sessionUpdate"].as_str() {
                Some("agent_message_chunk") if update["content"]["type"] == "text" => {
                    if text.len() <= MAX_ANSWER {
                        text.push_str(update["content"]["text"].as_str().unwrap_or(""));
                    }
                }
                Some(kind @ ("tool_call" | "tool_call_update")) if side => {
                    let id = update["toolCallId"].as_str().unwrap_or("").to_owned();
                    // The project files server only reads, whatever kind the engine gives its calls.
                    let read = matches!(update["kind"].as_str(), Some("read" | "search"));
                    match update.get("kind").filter(|k| !k.is_null()) {
                        _ if side_files_call(update) && !id.is_empty() => {
                            reads.insert(id);
                        }
                        Some(_) if read && !id.is_empty() => {
                            reads.insert(id);
                        }
                        None if kind == "tool_call_update" && reads.contains(&id) => {}
                        _ => acted = true,
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
        // A side chat's rejected request (a credential file, a path outside the
        // checkout) was answered no; the model answers without it.
        if asked.load(Ordering::SeqCst) && !side {
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

#[cfg(test)]
mod tests {
    use super::read_only_mode;
    use serde_json::json;

    /// The built-in read-only mode is found in codex-acp's (2.6.0) flat mode list
    /// and in the grouped one it reports once the launcher defines the worker
    /// permission profile (shapes observed with the bundled codex-acp 2.6.0 and
    /// Codex 0.166.0); a profile mode is never taken for it.
    #[test]
    fn read_only_mode_is_the_built_in_value_in_flat_and_grouped_lists() {
        let built_in = ["read-only", "workspace-write", "agent", "agent-full-access"]
            .map(|v| json!({"value": v, "name": v}));
        let flat = json!([{"id":"model","options":[{"value":"read-only"}]},
            {"id":"mode","type":"select","currentValue":"agent","options":built_in}]);
        assert_eq!(read_only_mode(&flat), Some("read-only"));
        let grouped = json!([{"id":"mode","type":"select","currentValue":"agent","options":[
            {"group":"sandbox-modes","name":"Sandbox Modes","options":built_in},
            {"group":"permission-profiles","name":"Permission Profiles",
             "options":[{"value":"permission-profile:cortex-workspace","name":"cortex-workspace"}]}]}]);
        assert_eq!(read_only_mode(&grouped), Some("read-only"));
        // Claude's `plan` stays the fallback.
        let plan = json!([{"id":"mode","options":[{"value":"default"},{"value":"plan"}]}]);
        assert_eq!(read_only_mode(&plan), Some("plan"));
        // Wide modes only, a profile named like a read-only mode, or a value
        // outside the mode option: none is a read-only mode.
        for options in [
            json!([{"id":"mode","options":[{"value":"agent"},{"value":"agent-full-access"}]}]),
            json!([{"id":"mode","options":[{"group":"permission-profiles","options":[
                {"value":"permission-profile:read-only"},{"value":"permission-profile::read-only"}]}]}]),
            json!([{"id":"mode","options":[{"group":"sandbox-modes","options":[{"value":"agent"}]}]},
                {"id":"other","options":[{"value":"read-only"}]}]),
            json!([{"id":"mode","options":[{"value":"agent","options":[{"value":"read-only"}]}]}]),
        ] {
            assert_eq!(read_only_mode(&options), None, "{options}");
        }
    }
}
