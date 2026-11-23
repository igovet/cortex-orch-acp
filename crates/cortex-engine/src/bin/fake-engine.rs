//! No-paid test peer. Never starts a model, engine or network connection.
use cortex_engine::wire;
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::io::{AsyncWriteExt, BufReader};

pub const CHECKPOINT_SCENARIO: &str = "rolling-checkpoint";
pub const CHECKPOINT_ANSWER: &str = "Keep the restart checkpoint output";
pub const CHECKPOINT_ANSWER_LOG: &str = "checkpoint-answer-deliveries";
pub const CHECKPOINT_COMMAND: &str = "checkpoint-broker-command";

fn config(value: &str) -> Value {
    config_with(value, "medium")
}
/// Effort option ids exactly as the bundled engines advertise them, selected by
/// the native account variable the launcher sets: codex-acp 2.3.0/2.6.0
/// `reasoning_effort`, claude-agent-acp 0.97.0/0.107.0 `effort`; both use the
/// `thought_level` category. OpenCode fixtures advertise no effort option.
fn effort_option() -> Option<&'static str> {
    if std::env::var_os("CODEX_HOME").is_some() {
        Some("reasoning_effort")
    } else if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        Some("effort")
    } else {
        None
    }
}
fn config_with(value: &str, effort: &str) -> Value {
    let select = |id: &str, category: &str, current: &str, values: &[&str]| {
        json!({"id":id,"name":id,"category":category,"type":"select","currentValue":current,
            "options":values.iter().map(|v| json!({"value":v,"name":v})).collect::<Vec<_>>()})
    };
    let mut options = json!([{"id":"model","type":"select","currentValue":value,"options":[{"value":"fixture-model","name":"Fixture"},{"value":"fixture-other","name":"Other fixture"}]}, {"id":"mode","type":"select","currentValue":"agent","options":(["agent","agent-full-access","workspace-write","default","acceptEdits","bypassPermissions","cortex","read-only","plan"].map(|m|json!({"value":m,"name":m})))}]);
    // A project holding `no-restricted-modes` gets an adapter that offers only
    // wide modes (no `read-only`/`plan`), like one without a read-only preset.
    if std::path::Path::new("no-restricted-modes").exists() {
        options[1]["options"] = json!([
            "agent",
            "agent-full-access",
            "acceptEdits",
            "bypassPermissions"
        ]
        .map(|m| json!({"value":m,"name":m})));
    }
    let list = options.as_array_mut().unwrap();
    // Each shape also advertises the real model id its owner catalog lists.
    let real_model = match effort_option() {
        Some("reasoning_effort") => Some("gpt-6.1-sol"),
        Some(_) => Some("claude-sonnet-5"),
        None => None,
    };
    if let Some(model) = real_model {
        list[0]["options"]
            .as_array_mut()
            .unwrap()
            .push(json!({"value":model,"name":model}));
    }
    // A project holding `model-alias` gets the Claude adapter's `default` alias: an entry named
    // "Default (recommended)" whose description names the concrete model.
    if std::path::Path::new("model-alias").exists() {
        list[0]["options"].as_array_mut().unwrap().push(
            json!({"value":"default","name":"Default (recommended)","description":"Opus 5.5"}),
        );
    }
    match effort_option() {
        Some("reasoning_effort") => {
            list.push(select(
                "collaboration_mode",
                "collaboration_mode",
                "default",
                &["default", "plan"],
            ));
            list.push(select(
                "reasoning_effort",
                "thought_level",
                effort,
                &["low", "medium", "high", "xhigh"],
            ));
            list.push(json!({"id":"fast-mode","name":"Fast mode","category":"model_config","type":"boolean","currentValue":false}));
        }
        Some(_) => {
            list.push(select(
                "effort",
                "thought_level",
                effort,
                &["low", "medium", "high"],
            ));
            list.push(select("fast", "model_config", "off", &["on", "off"]));
            list.push(select("ultracode", "model_config", "off", &["on", "off"]));
        }
        None => {}
    }
    options
}
/// The runtime sends its full recovery capsule once per context generation and only a
/// reference after that. Like a model, the fixture then reads the current capsule with
/// `read_capsule` through the session's own Cortex MCP relay.
async fn read_capsule(server: &Value) -> Value {
    let Some(command) = server["command"].as_str() else {
        return Value::Null;
    };
    let mut relay = tokio::process::Command::new(command)
        .args(
            server["args"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str()),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = relay.stdin.take().unwrap();
    let mut stdout = BufReader::new(relay.stdout.take().unwrap());
    wire::write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"read_capsule","arguments":{}}}),
    )
    .await
    .unwrap();
    let reply = wire::read_frame(&mut stdout).await.unwrap().unwrap();
    assert!(reply.get("error").is_none(), "{reply}");
    let result = &reply["result"];
    if result["structuredContent"]["original_goal"].is_string() {
        return result["structuredContent"].clone();
    }
    result["content"][0]["text"]
        .as_str()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null)
}
fn reconciliation_fixture(mode: &str, scenario: &str, context: &Value) -> Option<Value> {
    if mode != "desktop-coordinator" || !scenario.starts_with("reconcile-") {
        return None;
    }
    let gap = json!([{"item":"Live behavior","description":"Live behavior","blocker":"paid live check not requested by the owner"}]);
    let events = context["retained_decisions"].as_array().unwrap();
    let has = |kind: &str| events.iter().any(|e| e["type"] == kind);
    if context["state"] == "PLANNING" && has("USER_ANSWER_RECEIVED") {
        assert!(context["questions"]
            .to_string()
            .contains("Explain and finish"));
        assert!(context["original_goal"]
            .as_str()
            .unwrap()
            .contains("Never lose constraint α"));
        return Some(
            json!({"name":"answer_submit","arguments":{"text":"Owner decision explained.","not_verified":gap,"request_key":"released-answer"}}),
        );
    }
    if context["state"] == "PLANNING" && has("VERIFICATION_FAILED") && !has("VERIFICATION_REVISED")
    {
        return Some(
            // A repair that replaces the failed check must be a real command: a
            // trivial one such as `true` is refused (KI-03).
            json!({"name":"verification_submit","arguments":{"verification":[["/usr/bin/cmp","/dev/null","/dev/null"]],"request_key":"repair"}}),
        );
    }
    if context["state"] != "RECONCILING" {
        return None;
    }
    assert!(context["original_goal"]
        .as_str()
        .unwrap()
        .contains("Never lose constraint α"));
    assert!(has("VERIFICATION_PASSED"));
    let incomplete = context["current_nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["status"] != "completed");
    assert!(!incomplete);
    // The replaced failed check stays an open obligation until the owner answers.
    // "reconcile-repair-late" asks only once the runtime returned the refused
    // completion to the coordinator (ASK_USER_REQUIRED).
    if (scenario == "reconcile-question"
        || scenario == "reconcile-repair" && has("VERIFICATION_REVISED")
        || scenario == "reconcile-repair-late"
            && has("VERIFICATION_REVISED")
            && has("ASK_USER_REQUIRED"))
        && !has("USER_ANSWER_RECEIVED")
    {
        return Some(
            json!({"name":"user_question_request","arguments":{"question":"How should we finish?","options":["Explain and finish","Plan changes"],"request_key":"reconcile-question"}}),
        );
    }
    if context["revision"] == 1 && !has("REPLAN_REQUEST_ACCEPTED") {
        let operation = match scenario {
            "reconcile-replan" => Some("replan_request"),
            "reconcile-scope" => Some("scope_request"),
            "reconcile-escalation" => Some("escalation_request"),
            _ => None,
        };
        if let Some(operation) = operation {
            let mut args = json!({"reason":"Independent retained evidence needs a new decomposition","request_key":"reconcile-replan"});
            if operation == "scope_request" {
                args["scope"] = json!(["work.txt"]);
            }
            if operation == "escalation_request" {
                args["report_ids"] = json!(["r_000000000000"]);
                args["question"] = json!("What should change?");
            }
            return Some(json!({"name":operation,"arguments":args}));
        }
    }
    let continuation_count = events
        .iter()
        .rev()
        .take_while(|e| e["type"] != "VERIFICATION_PASSED")
        .filter(|e| e["type"] == "NATIVE_COORDINATOR_CONTINUED")
        .count();
    if scenario == "reconcile-reject" && continuation_count == 1 {
        return Some(
            json!({"name":"synthesis_submit","arguments":{"text":"Omitted gaps must refuse."},"__fixture_expect":{
                "/result/isError":true,
                "/result/structuredContent/code":"synthesis_gaps_undisclosed",
                "/result/structuredContent/recorded":false,
                "/result/structuredContent/required_not_verified":["Live behavior"]}}),
        );
    }
    Some(
        json!({"name":"synthesis_submit","arguments":{"text":"Verified work reconciled.","not_verified":gap},
        "__fixture_expect":{"/result/structuredContent/recorded":true}}),
    )
}
#[tokio::main]
pub async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode.starts_with('/') {
        cortex_engine::broker::relay(std::path::Path::new(&mode))
            .await
            .unwrap();
        return;
    }
    let mut servers = HashMap::new();
    let mut external_entries = HashMap::new();
    // Each session's `session/new` `_meta`, checked by the `session-meta:` prompts.
    let mut session_metas: HashMap<String, Value> = HashMap::new();
    let mut input = BufReader::new(tokio::io::stdin());
    let mut output = tokio::io::stdout();
    let mut count = 0;
    let mut permissions: HashMap<String, (Value, String)> = HashMap::new();
    let mut held = HashMap::new();
    let mut prompts = HashMap::<String, usize>::new();
    // `desktop-auto` infers a session's role from its first prompt only. Later prompts of that
    // session (an owner answer or steering text delivered as the whole prompt tail) carry no
    // assignment, so re-inferring the role per prompt turns a resumed worker into a coordinator.
    let mut auto_roles = HashMap::<String, &'static str>::new();
    let mut models = HashMap::<String, String>::new();
    let mut efforts = HashMap::<String, String>::new();
    // Frames come from one reader task, so the loop can also wake on a timer: a worker
    // prompt held by the plain `hold` scenario finishes its turn normally once
    // `scenario.txt` no longer names a hold, like a real worker that keeps running,
    // and stays cancellable until then.
    let (frames_in, mut frames) = tokio::sync::mpsc::channel::<Value>(64);
    tokio::spawn(async move {
        while let Ok(Some(r)) = wire::read_frame(&mut input).await {
            if frames_in.send(r).await.is_err() {
                break;
            }
        }
    });
    let mut releasable = HashMap::<String, Value>::new();
    let mut released = std::collections::VecDeque::<Value>::new();
    loop {
        let r = if let Some(r) = released.pop_front() {
            r
        } else if releasable.is_empty() {
            match frames.recv().await {
                Some(r) => r,
                None => break,
            }
        } else {
            tokio::select! {
                r = frames.recv() => match r {
                    Some(r) => r,
                    None => break,
                },
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                    let scenario = std::fs::read_to_string("scenario.txt").unwrap_or_default();
                    if !scenario.contains("hold")
                        && !matches!(scenario.trim(), "rolling" | CHECKPOINT_SCENARIO)
                    {
                        for (sid, mut frame) in releasable.drain() {
                            held.remove(&sid);
                            frame["__fixture_released"] = json!(true);
                            released.push_back(frame);
                        }
                    }
                    continue;
                }
            }
        };
        let replayed = r.get("__fixture_released").is_some();
        let id = r.get("id").cloned().unwrap_or(Value::Null);
        let method = r["method"].as_str().unwrap_or("");
        let sid = r["params"]["sessionId"].as_str().unwrap_or("").to_owned();
        if method.is_empty() {
            if let Some((prompt_id, sid)) = permissions.remove(&id.to_string()) {
                // Only the offered allow option approves; a selected reject denies.
                let selected = r["result"]["outcome"]["outcome"] == "selected"
                    && r["result"]["outcome"]["optionId"] != "no";
                wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":if selected{"approved"}else{"denied"}}}}})).await.unwrap();
                wire::write_frame(
                    &mut output,
                    &json!({"jsonrpc":"2.0","id":prompt_id,"result":{"stopReason":"end_turn"}}),
                )
                .await
                .unwrap();
            }
            continue;
        }
        if mode == "external" || mode == "mismatch" {
            if method == "notifications/initialized" {
                continue;
            }
            if method == "tools/call" && r["params"]["name"] == "hang" {
                continue;
            }
            let result = match method {
                "initialize" => {
                    json!({"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}})
                }
                "tools/list" => {
                    json!({"tools":[{"name":"lookup","inputSchema":{"type":"object"}},{"name":"write","inputSchema":{"type":"object"}}]})
                }
                _ => json!({"content":[{"type":"text","text":"fixture"}]}),
            };
            wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","id":if mode=="mismatch"{json!(999)}else{id},"result":result})).await.unwrap();
            continue;
        }
        match method {
            "initialize" => {
                if mode == "initialize-hang" {
                    continue;
                }
                if mode == "stderr-exit" {
                    // More than the retained tail, then a synthetic secret in the last line.
                    eprintln!("{}", "x".repeat(20_000));
                    eprintln!("fixture crash: apiKey=\"sk-fixture-abcdefghijklmnop0123\" Bearer fixture-bearer-value \u{1b}[0m session 89a921a6-3f1e-42ed-9a9e-aaec73cb7ed6");
                    std::process::exit(5);
                }
                // A project holding `initialize-after` answers only once the marker
                // path it names exists, plus a settle margin, to order host events.
                if let Ok(marker) = std::fs::read_to_string("initialize-after") {
                    let marker = std::path::Path::new(marker.trim()).to_owned();
                    for _ in 0..1000 {
                        if marker.exists() {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":1,"agentCapabilities":
                    if std::fs::read_to_string("prompt-caps.txt").is_ok_and(|v|v.trim()=="none"){json!({})}
                    else{json!({"promptCapabilities":{"image":true,"embeddedContext":true}})}}})).await.unwrap();
            }
            "session/new" => {
                if mode == "session-hang" {
                    continue;
                }
                if mode == "relay" {
                    let entry = &r["params"]["mcpServers"][0];
                    let args = entry["args"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap())
                        .collect::<Vec<_>>();
                    let mut child =
                        tokio::process::Command::new(entry["command"].as_str().unwrap())
                            .args(args)
                            .stdin(std::process::Stdio::piped())
                            .stdout(std::process::Stdio::piped())
                            .kill_on_drop(true)
                            .spawn()
                            .unwrap();
                    let mut stdin = child.stdin.take().unwrap();
                    let mut stdout = BufReader::new(child.stdout.take().unwrap());
                    wire::write_frame(
                        &mut stdin,
                        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
                    )
                    .await
                    .unwrap();
                    let reply = wire::read_frame(&mut stdout).await.unwrap().unwrap();
                    assert!(reply["result"]["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|v| v["name"] == "result_submit"));
                    wire::write_frame(&mut stdin,&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_read","arguments":{"path":"fixture.txt"}}})).await.unwrap();
                    let reply = wire::read_frame(&mut stdout).await.unwrap().unwrap();
                    assert!(reply.get("result").is_some());
                    child.kill().await.unwrap();
                    child.wait().await.unwrap();
                }
                count += 1;
                let sid = format!("fixture-{count}");
                // Behind Cortex's native Codex launcher the relays reach App Server as its
                // own config (the launcher hand-over), never also as ACP servers.
                let acp = r["params"]["mcpServers"].as_array().unwrap().clone();
                let listed = match std::env::var("CORTEX_CODEX_MCP_SERVERS") {
                    Ok(handed) => {
                        assert!(
                            acp.is_empty(),
                            "relays sent both through the launcher and ACP"
                        );
                        // codex-acp spreads this config's features into every thread's
                        // `features` table, which replaces the launcher's feature layers.
                        assert_eq!(
                            serde_json::from_str::<Value>(&std::env::var("CODEX_CONFIG").unwrap())
                                .unwrap()["features"],
                            serde_json::from_str::<Value>(
                                &cortex_engine::environment::codex_thread_config()
                            )
                            .unwrap()["features"],
                            "a launcher-backed session without its thread feature config"
                        );
                        serde_json::from_str::<Vec<Value>>(&handed).unwrap()
                    }
                    Err(_) => acp,
                };
                session_metas.insert(sid.clone(), r["params"]["_meta"].clone());
                servers.insert(sid.clone(), listed.first().cloned().unwrap_or(Value::Null));
                external_entries.insert(
                    sid.clone(),
                    listed.iter().skip(1).cloned().collect::<Vec<_>>(),
                );
                let reply = json!({"jsonrpc":"2.0","id":id,"result":{"sessionId":sid,"configOptions":config("initial")}});
                // claude-agent-acp 0.107.0 (`afterSetupResponse`) sends the new session's
                // `available_commands_update` right behind the `session/new` reply. One write
                // puts both frames in the reader's buffer at once.
                if mode == "commands-after-new"
                    || std::path::Path::new("commands-after-new").exists()
                {
                    let update = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,
                        "update":{"sessionUpdate":"available_commands_update","availableCommands":[]}}});
                    output
                        .write_all(format!("{reply}\n{update}\n").as_bytes())
                        .await
                        .unwrap();
                    output.flush().await.unwrap();
                } else {
                    wire::write_frame(&mut output, &reply).await.unwrap();
                }
            }
            "session/set_config_option" => {
                // An adapter that dies while configuring: once (`configure-crash-once`,
                // remembered by `configure-crashed`) or every time (`configure-crash`).
                // Its stderr carries a synthetic secret that diagnostics must redact.
                let once = std::path::Path::new("configure-crash-once").exists()
                    && !std::path::Path::new("configure-crashed").exists();
                if once || std::path::Path::new("configure-crash").exists() {
                    if once {
                        std::fs::write("configure-crashed", "").unwrap();
                    }
                    eprintln!("fixture adapter: configure crashed; token=sk-fixture-0123456789abcdefghijklmnop Authorization: Bearer fixture-bearer-value");
                    std::process::exit(3);
                }
                let value = r["params"]["value"].as_str().unwrap().to_owned();
                if effort_option().is_some_and(|id| r["params"]["configId"] == id) {
                    efforts.insert(sid.clone(), value.clone());
                } else if r["params"]["configId"] != "mode" {
                    models.insert(sid.clone(), value.clone());
                }
                let mut options = config_with(
                    models
                        .get(&sid)
                        .map(String::as_str)
                        .unwrap_or("fixture-model"),
                    efforts.get(&sid).map(String::as_str).unwrap_or("medium"),
                );
                if r["params"]["configId"] == "mode" {
                    options[1]["currentValue"] = json!(value);
                }
                let scenario = std::fs::read_to_string("scenario.txt").unwrap_or_default();
                if scenario == "mode-not-confirmed" && r["params"]["configId"] == "mode" {
                    options[1]["currentValue"] = json!("wrong");
                }
                wire::write_frame(
                    &mut output,
                    &json!({"jsonrpc":"2.0","id":id,"result":{"configOptions":options}}),
                )
                .await
                .unwrap();
            }
            "session/prompt" => {
                // A project that holds `record-prompts` gets every received prompt
                // appended to `prompt-log.jsonl`, so tests can inspect the exact blocks.
                // A read-only session (a reviewer) cannot write there and is simply not recorded.
                // A reviewer never writes it, even where its sandbox would allow the write:
                // a reviewer's own project change refuses its review.
                if std::path::Path::new("record-prompts").exists()
                    && !replayed
                    && mode != "desktop-reviewer"
                {
                    use std::io::Write as _;
                    if let Ok(mut log) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open("prompt-log.jsonl")
                    {
                        let _ = writeln!(
                            log,
                            "{}",
                            json!({"session":sid,"prompt":r["params"]["prompt"]})
                        );
                    }
                }
                let raw = r["params"]["prompt"][0]["text"].as_str().unwrap_or("");
                let base = if let Some(text) = session_metas
                    .get(&sid)
                    .and_then(|v| v["systemPrompt"].as_str())
                {
                    text.to_owned()
                } else {
                    std::env::var("CODEX_CONFIG")
                        .ok()
                        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                        .and_then(|v| {
                            std::fs::read_to_string(v["model_instructions_file"].as_str()?).ok()
                        })
                        .unwrap_or_default()
                };
                if mode == "mcp-host" {
                    let expectation = raw.rsplit_once("\n\n").map_or(raw, |(_, v)| v);
                    let entries = &external_entries[&sid];
                    if expectation.starts_with("review-") {
                        let meta = &session_metas[&sid];
                        assert_eq!(meta["claudeCode"]["options"]["strictMcpConfig"], true);
                        assert_eq!(servers[&sid]["name"], "cortex");
                        let boundary = meta["systemPrompt"]
                            .as_str()
                            .and_then(|prompt| prompt.rsplit_once("## Native capability boundary"))
                            .map_or("", |(_, boundary)| boundary);
                        std::fs::write("review-mcp-boundary", boundary).unwrap();
                    }
                    if matches!(expectation, "none" | "review-none") {
                        assert!(entries.is_empty());
                    } else {
                        assert_eq!(entries.len(), 1);
                        let entry = &entries[0];
                        assert_eq!(entry["name"], "fixture");
                        assert!(entry["env"].as_array().unwrap().is_empty());
                        assert!(!entry.to_string().contains("external-fixture"));
                        let mut child =
                            tokio::process::Command::new(entry["command"].as_str().unwrap())
                                .args(
                                    entry["args"]
                                        .as_array()
                                        .unwrap()
                                        .iter()
                                        .map(|v| v.as_str().unwrap()),
                                )
                                .stdin(std::process::Stdio::piped())
                                .stdout(std::process::Stdio::piped())
                                .kill_on_drop(true)
                                .spawn()
                                .unwrap();
                        let mut stdin = child.stdin.take().unwrap();
                        let mut stdout = BufReader::new(child.stdout.take().unwrap());
                        for (id, method, params) in [
                            (
                                1,
                                "initialize",
                                json!({"protocolVersion":"2025-11-25","capabilities":{}}),
                            ),
                            (2, "tools/list", json!({})),
                            (3, "tools/call", json!({"name":"lookup","arguments":{}})),
                            (3, "tools/call", json!({"name":"lookup","arguments":{}})),
                            (
                                3,
                                "tools/call",
                                json!({"name":"lookup","arguments":{"changed":true}}),
                            ),
                            (4, "tools/call", json!({"name":"write","arguments":{}})),
                        ] {
                            wire::write_frame(
                                &mut stdin,
                                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
                            )
                            .await
                            .unwrap();
                            let reply = wire::read_frame(&mut stdout).await.unwrap().unwrap();
                            std::fs::write(
                                "mcp-probe",
                                json!({"id":id,"method":method,"reply":reply}).to_string(),
                            )
                            .unwrap();
                            match (id, method) {
                                (2, _) => {
                                    if expectation == "review-read" {
                                        assert_eq!(
                                            reply["result"]["tools"],
                                            json!([{"name":"lookup","description":"Look up entries in the fixture index.",
                                                "inputSchema":{"type":"object","additionalProperties":false}}])
                                        );
                                    }
                                    let names: Vec<_> = reply["result"]["tools"]
                                        .as_array()
                                        .unwrap()
                                        .iter()
                                        .map(|t| t["name"].as_str().unwrap())
                                        .collect();
                                    assert_eq!(
                                        names,
                                        if expectation == "write" {
                                            vec!["lookup", "write"]
                                        } else {
                                            vec!["lookup"]
                                        }
                                    );
                                }
                                (3, _) => {
                                    if params["arguments"]["changed"] == true {
                                        assert!(reply.get("error").is_some());
                                    } else {
                                        assert_eq!(reply["result"]["count"], 1);
                                    }
                                }
                                (4, _) => {
                                    if expectation == "write" {
                                        assert_eq!(reply["result"]["count"], 2);
                                    } else {
                                        assert!(reply.get("error").is_some());
                                    }
                                }
                                _ => assert!(reply.get("result").is_some()),
                            }
                        }
                        child.kill().await.unwrap();
                        child.wait().await.unwrap();
                    }
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}),
                    )
                    .await
                    .unwrap();
                    continue;
                }
                if std::fs::read_to_string("scenario.txt")
                    .unwrap_or_default()
                    .trim()
                    == "attachments"
                {
                    let blocks = r["params"]["prompt"].as_array().unwrap();
                    assert!(blocks.iter().any(|b| b["type"] == "image"
                        && b["data"] == "YWJj"
                        && b["mimeType"] == "image/png"));
                    // input.txt is text: embedded as text, never as base64.
                    assert!(blocks
                        .iter()
                        .any(|b| b["type"] == "resource" && b["resource"]["text"] == "snapshot"));
                }
                if std::fs::read_to_string("scenario.txt")
                    .unwrap_or_default()
                    .trim()
                    == "followup-attachments"
                {
                    // Start's image (revision 1), then the follow-up's (revision 2).
                    let blocks = r["params"]["prompt"].as_array().unwrap();
                    let images: Vec<_> = blocks
                        .iter()
                        .filter(|b| b["type"] == "image")
                        .map(|b| b["data"].as_str().unwrap())
                        .collect();
                    assert_eq!(images, ["YWJj", "ZGVm"]);
                    assert!(blocks.iter().any(|b| b["type"] == "resource"
                        && b["resource"]["text"] == "ghi"
                        && b["resource"]["uri"] == "file:///notes.txt"));
                }
                let mode = if mode == "desktop-auto" {
                    let tail = raw.rsplit_once("\n\n").map_or(raw, |(_, text)| text);
                    let inferred = match serde_json::from_str::<Value>(tail).ok() {
                        Some(v) if v["stage"] == "review" => "desktop-reviewer",
                        Some(v) if v["assignment_ref"].is_string() => "desktop-worker",
                        _ => "desktop-coordinator",
                    };
                    *auto_roles.entry(sid.clone()).or_insert(inferred)
                } else {
                    mode.as_str()
                };
                let assignment = serde_json::from_str::<Value>(
                    raw.rsplit_once("\n\n").map_or(raw, |(_, tail)| tail),
                )
                .unwrap_or(Value::Null);
                let is_second = assignment["node"] == "second";
                let is_third = assignment["node"] == "third";
                let n = prompts.entry(sid.clone()).or_default();
                if !replayed {
                    *n += 1;
                }
                let full_scenario = std::fs::read_to_string("scenario.txt").unwrap_or_default();
                let checkpoint = full_scenario.trim() == CHECKPOINT_SCENARIO;
                let rolling = checkpoint || full_scenario.trim() == "rolling";
                let checkpoint_answer = mode.starts_with("desktop-")
                    && raw.rsplit("\n\n").next() == Some(CHECKPOINT_ANSWER)
                    && (checkpoint || std::path::Path::new(CHECKPOINT_ANSWER_LOG).exists());
                if checkpoint_answer {
                    // Record actual ACP prompt delivery, not just the runtime's claim.
                    // Also observe a delivery on a fresh session after the scenario changes.
                    use std::io::Write as _;
                    let mut log = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(CHECKPOINT_ANSWER_LOG)
                        .unwrap();
                    writeln!(log, "delivered").unwrap();
                }
                // Reports whether this worker or reviewer session was given the owner's
                // task language, in words the test can find in the retained transcript.
                if full_scenario.trim() == "language-ru"
                    && matches!(mode, "desktop-worker" | "desktop-reviewer")
                {
                    let observed = base.contains("Owner-selected response language: ru.");
                    wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":format!("{mode} language observed: {observed}.")}}}})).await.unwrap();
                }
                let context = raw
                    .lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .find(|v| v["original_goal"].is_string() && v["recipient"].is_object())
                    .unwrap_or(Value::Null);
                let context = if context.is_null()
                    && mode.starts_with("desktop-")
                    && raw.contains("the full capsule was already delivered in this context")
                {
                    read_capsule(servers.get(&sid).unwrap_or(&Value::Null)).await
                } else {
                    context
                };
                if mode == "desktop-coordinator" && full_scenario.trim() == "usage-refusal-twice" {
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,
                            "result":{"stopReason":"refusal","usage":{"totalTokens":2}}}),
                    )
                    .await
                    .unwrap();
                    continue;
                }
                // A degenerate coordinator turn: the model streams its own prompt text back
                // (`degenerate-once`, only the first coordinator session; every later one
                // works normally) or loops on one handle (`degenerate-twice`, every
                // session), and keeps generating until the runtime cancels the turn.
                // `degenerate-worker`: the first worker session writes one message of more than
                // 16,000 characters without a tool call; every later worker session works.
                if (mode == "desktop-coordinator"
                    && matches!(full_scenario.trim(), "degenerate-once" | "degenerate-twice"))
                    || (mode == "desktop-worker" && full_scenario.trim() == "degenerate-worker")
                {
                    let turns: u32 = std::fs::read_to_string("degenerate-turns")
                        .ok()
                        .and_then(|s| s.trim().parse().ok())
                        .unwrap_or(0);
                    let twice = full_scenario.trim() == "degenerate-twice";
                    if twice || turns == 0 {
                        std::fs::write("degenerate-turns", (turns + 1).to_string()).unwrap();
                        let looped = "guner@".repeat(400);
                        let chunks: Vec<String> = if mode == "desktop-worker" {
                            let mut n: u64 = 11;
                            (0..60)
                                .map(|_| {
                                    (0..18)
                                        .map(|_| {
                                            n = n
                                                .wrapping_mul(6364136223846793005)
                                                .wrapping_add(1442695040888963407);
                                            format!("{n:016x} ")
                                        })
                                        .collect()
                                })
                                .collect()
                        } else if twice {
                            looped
                                .chars()
                                .collect::<Vec<_>>()
                                .chunks(60)
                                .map(|c| c.iter().collect())
                                .collect()
                        } else {
                            vec![
                                "Understood. Continuing.\n<system-".into(),
                                "reminder>\nRuntime status: PLANNING\n</system-reminder>".into(),
                                "Further echoed prompt text.".into(),
                            ]
                        };
                        for text in chunks {
                            wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,
                                "update":{"sessionUpdate":"agent_message_chunk","messageId":format!("degenerate-{turns}"),
                                "content":{"type":"text","text":text}}}})).await.unwrap();
                        }
                        held.insert(sid, id);
                        continue;
                    }
                }
                if matches!(
                    full_scenario.as_str(),
                    "compaction-start-only" | "compaction-title-only"
                ) && mode == "desktop-coordinator"
                {
                    let mut update = json!({"sessionUpdate":"tool_call","toolCallId":"not-completed","title":"Compact conversation","status":"in_progress"});
                    if full_scenario == "compaction-start-only" {
                        update["_meta"] = json!({"contextCompaction":{"version":1}});
                    }
                    wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":update}})).await.unwrap();
                }
                let compact = (matches!(
                    full_scenario.as_str(),
                    "compaction-coordinator" | "compaction-claude"
                ) && mode == "desktop-coordinator")
                    || (full_scenario == "compaction-worker" && mode == "desktop-worker");
                if compact && *n == 1 {
                    assert!(raw.contains("Never lose constraint α"));
                    for (kind, status) in [
                        ("tool_call", "in_progress"),
                        ("tool_call_update", "completed"),
                        ("tool_call_update", "completed"),
                    ] {
                        let meta = if full_scenario == "compaction-claude" {
                            json!({"claudeCode":{"toolName":"compact"}})
                        } else {
                            json!({"contextCompaction":{"version":1}})
                        };
                        wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,
                            "update":{"sessionUpdate":kind,"toolCallId":"compact-once","title":"Not ordinary tool evidence",
                                "status":status,"_meta":meta}}})).await.unwrap();
                    }
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}),
                    )
                    .await
                    .unwrap();
                    continue;
                }
                if compact {
                    assert_eq!(*n, 2);
                    assert!(raw.contains("Never lose constraint α"));
                    assert!(raw.contains("NATIVE_CONTEXT_COMPACTED"));
                    if full_scenario != "compaction-claude" {
                        assert!(base.contains("Derived from this channel's served schemas"));
                    }
                }
                if mode == "desktop-coordinator" && full_scenario.trim() == "saved-settings" {
                    // Record what Start configured for this coordinator, then hold it.
                    std::fs::write(
                        "observed-settings.json",
                        json!({"model":models.get(&sid),"effort":efforts.get(&sid),"option":effort_option(),
                            "language":base.contains("Owner-selected response language: ru.")})
                        .to_string(),
                    )
                    .unwrap();
                    held.insert(sid, id);
                    continue;
                }
                let scenario = std::fs::read_to_string("scenario.txt").unwrap_or_default();
                let scenario = if mode == "desktop-worker"
                    || mode == "desktop-reviewer" && scenario.trim() == "review-hold"
                {
                    scenario
                } else {
                    String::new()
                };
                if mode == "desktop-reviewer" && scenario.trim() == "review-hold" {
                    std::fs::write("review-entered", "ready").unwrap();
                    held.insert(sid, id);
                    continue;
                }
                if scenario.trim() == "hold" {
                    std::fs::write("worker-entered", "ready").unwrap();
                }
                if scenario.trim() == "separate-task-hold" {
                    std::fs::write("separate-worker-entered", "ready").unwrap();
                }
                if matches!(scenario.trim(), "hold" | "separate-task-hold")
                    || scenario.trim() == "guidance" && *n == 1
                    || scenario.trim() == "guidance-preempt" && *n <= 2
                    || scenario.trim() == "stop-siblings" && !is_second
                    || mode == "desktop-worker" && rolling && is_second
                {
                    if scenario.trim() == "hold" || rolling && is_second {
                        releasable.insert(sid.clone(), r.clone());
                    }
                    held.insert(sid, id);
                    continue;
                }
                if scenario.trim() == "incoming-permission" {
                    let pid = json!(format!("permission-{sid}"));
                    permissions.insert(pid.to_string(), (id, sid.clone()));
                    wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","id":pid,"method":"session/request_permission",
                        "params":{"sessionId":sid,"toolCall":{"toolCallId":"fixture-permission","title":"Fixture permission"},
                        "options":[{"optionId":"yes","name":"Allow once","kind":"allow_once"},{"optionId":"no","name":"Reject once","kind":"reject_once"}]}})).await.unwrap();
                    continue;
                }
                if scenario.trim() == "exit" {
                    std::process::exit(17);
                }
                let automatic = if mode.starts_with("desktop-") {
                    let proposal = std::fs::read_to_string("proposal.json").ok();
                    let params = if let Some(params) =
                        reconciliation_fixture(mode, &full_scenario, &context)
                    {
                        params
                    } else if mode == "desktop-coordinator" && full_scenario == "repair-failed-test"
                    {
                        if context["state"] == "RECONCILING" {
                            json!({"name":"synthesis_submit","arguments":{"text":"Repaired the output and reran the failed check."}})
                        } else {
                            let repair =
                                context["retained_decisions"]
                                    .as_array()
                                    .is_some_and(|events| {
                                        events.iter().any(|e| e["type"] == "VERIFICATION_FAILED")
                                    });
                            let node = if repair { "fix-output" } else { "work" };
                            json!({"name":"plan_submit","arguments":{"request_key":node,"plan":{
                                "tasks":[{"id":node,"role":"backend-dev","objective":"Write the expected output","dependencies":[],"scope":["work.txt"],"model":"auto"}],
                                "verification":[["/usr/bin/cmp","expected.txt","work.txt"]]}}})
                        }
                    } else if mode == "desktop-coordinator"
                        && full_scenario == "slow-check"
                        && context["state"] == "PLANNING"
                        && context["retained_decisions"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|e| e["type"] == "VERIFICATION_FAILED"))
                    {
                        json!({"name":"user_question_request","arguments":{"question":"The owner stopped verification. How should we continue?",
                            "options":["Keep the stopped result","Plan another check"],"request_key":"stopped-verification"}})
                    } else if mode == "desktop-coordinator" && proposal.is_some() {
                        if *n == 1 {
                            json!({"name":"user_question_request","arguments":serde_json::from_str::<Value>(&proposal.unwrap()).unwrap()})
                        } else {
                            let expected =
                                std::fs::read_to_string("expected-decision.txt").unwrap();
                            if expected == "approved" {
                                assert!(raw.contains("/skill-management Create the project skill"));
                                assert!(raw.contains(
                                    "Read the coordinator-only shared skill skill-management"
                                ));
                            } else {
                                assert!(raw.contains("Do not create, update or link it"));
                                assert!(raw.contains(expected.trim()));
                                assert!(!raw.contains("/skill-management Create"));
                            }
                            json!({"name":"answer_submit","arguments":{"text":"Recorded decision; fixture leaves files unchanged.","request_key":"answer"}})
                        }
                    } else if mode == "desktop-coordinator" {
                        if std::fs::read_to_string("scenario.txt").unwrap_or_default()
                            == "recovery-context"
                        {
                            assert!(raw.contains("Coordinator recovery on a new engine session."));
                            assert!(raw.contains("Retained startup goal"));
                            assert!(raw.contains("Retained recovery instruction"));
                        }
                        if std::fs::read_to_string("scenario.txt")
                            .unwrap_or_default()
                            .trim()
                            == "settings-ru"
                        {
                            assert!(base.contains("Owner-selected response language: ru."));
                            assert_eq!(models.get(&sid).map(String::as_str), Some("fixture-other"));
                            wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Observed owner task model and language."}}}})).await.unwrap();
                        }
                        let slow = std::fs::read_to_string("scenario.txt")
                            .unwrap_or_default()
                            .trim()
                            == "slow-check";
                        let mut tasks = json!([{"id":"work","role":"backend-dev","objective":"Change work.txt","dependencies":[],"scope":["work.txt"],"model":"auto"}]);
                        if full_scenario == "separate-task-hold" {
                            tasks[0]["scope"] = json!(["separate.txt"]);
                        }
                        if checkpoint {
                            tasks[0]["scope"] = json!(["work.txt", CHECKPOINT_ANSWER_LOG]);
                        }
                        if full_scenario.trim() == "stop-siblings"
                            || rolling
                            || full_scenario == "reconcile-worker-replan"
                        {
                            tasks.as_array_mut().unwrap().push(json!({"id":"second","role":"backend-dev","objective":"Change other.txt","dependencies":[],"scope":["other.txt"],"model":"auto"}));
                        }
                        if rolling {
                            tasks.as_array_mut().unwrap().push(json!({"id":"third","role":"backend-dev","objective":"Change third.txt after work.txt","dependencies":["work"],"scope":["third.txt"],"model":"auto"}));
                        }
                        // Owner messages handed over after verified work: this fixture's
                        // messages need no further change, so it answers directly.
                        let verified_guidance = raw
                            .contains("Owner messages received while the work ran")
                            && context["state"] == "PLANNING"
                            && context["retained_decisions"].as_array().is_some_and(|a| {
                                a.iter().any(|e| e["type"] == "VERIFICATION_PASSED")
                            });
                        if verified_guidance {
                            // The answer names the first message it was handed, so a test can
                            // see what reached this turn.
                            let first = raw
                                .split("\n\n1. ")
                                .nth(1)
                                .and_then(|s| s.lines().next())
                                .unwrap_or_default();
                            json!({"name":"answer_submit","arguments":{"text":format!("Owner guidance applied ({first}); the verified work needs no further change."),"request_key":"guided-answer"}})
                        } else {
                            json!({"name":"plan_submit","arguments":{"request_key":"plan","plan":{
                            "tasks":tasks,
                            "verification":if slow {json!([["/usr/bin/cmp","/dev/zero","/dev/zero"]])} else if (full_scenario=="reconcile-repair" || full_scenario=="reconcile-repair-late") && context["revision"]==0 { json!([["/bin/false"]]) } else {json!([["/usr/bin/cmp","/dev/null","/dev/null"]])}}}})
                        }
                    } else if mode == "desktop-worker"
                        && full_scenario == "reconcile-worker-replan"
                        && context["revision"] == 1
                    {
                        json!({"name":"replan_request","arguments":{"reason":"Replan before launching the already claimed sibling","request_key":"worker-replan"}})
                    } else if mode == "desktop-worker"
                        && scenario.trim() == "command-scratch-hold"
                        && *n == 1
                    {
                        json!({"name":"command_run","arguments":{"argv":["/bin/sh","-ec",
                            "printf '%s\\n' scratch-control-ready; exec /bin/sleep 120"],
                            "timeout_seconds":120,"request_key":"scratch-command-control"}})
                    } else if mode == "desktop-worker"
                        && scenario.starts_with("command-restart-")
                        && *n == 1
                    {
                        json!({"name":"command_run","arguments":{"argv":["/bin/sh","-c",
                            "printf 'launch\\n' >> command-launches; printf 'before\\n'; head -c 70000 /dev/zero; touch command-entered; while [ ! -f command-release ]; do sleep 0.1; done; printf 'after\\n'; exit 7"],
                            "timeout_seconds":60,"request_key":"restart-command"}})
                    } else if scenario.trim() == "outside-write" {
                        json!({"name":"outside_write","arguments":{"path":std::fs::read_to_string("outside-target").unwrap(),
                            "content":"client-approved","request_key":"client-write"}})
                    } else if scenario.trim() == "access-retry" && *n == 1 {
                        json!({"name":"access_request","arguments":{"paths":["extra"],"reason":"Fixture reservation","request_key":"access-retry"}})
                    } else if scenario.trim() == "guidance" && *n == 2
                        || scenario.trim() == "guidance-preempt" && *n == 3
                    {
                        assert!(
                            raw.contains("Owner guidance")
                                && (raw.contains("Use the owner output")
                                    || raw.contains("Second guidance"))
                        );
                        wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Guidance observed in same worker session."}}}})).await.unwrap();
                        json!({"name":"ask_question","arguments":{"question":"Guidance received; next?","options":["Continue","Wait"],"request_key":"guidance-question"}})
                    } else if mode == "desktop-worker" && checkpoint
                        && assignment["node"] == "work" && *n == 1
                    {
                        json!({"name":"ask_question","arguments":{
                            "question":"Keep this output before pausing?",
                            "options":[CHECKPOINT_ANSWER,"Discard the checkpoint output"],
                            "request_key":"checkpoint-question"}})
                    } else if scenario.trim() == "batch" && *n == 1 {
                        json!({"name":"ask_question","arguments":{"questions":[
                            {"question":"Which output?","options":["Detailed","Brief"]},
                            {"question":"Which language?","options":["English","Russian"]}],"request_key":"batch"}})
                    } else if matches!(scenario.trim(), "question" | "question-exit") && *n == 1 {
                        json!({"name":"ask_question","arguments":{"question":"Which output?","options":["Detailed","Brief"],"request_key":"question"}})
                    } else {
                        if scenario.trim() == "access-retry" {
                            assert_eq!(*n, 2);
                            let answer: Value =
                                serde_json::from_str(raw.rsplit_once("\n\n").unwrap().1).unwrap();
                            assert!(matches!(
                                answer["status"].as_str(),
                                Some("granted" | "denied")
                            ));
                        }
                        let consultant = context["current_nodes"].as_array().is_some_and(|a| {
                            a.iter().any(|n| {
                                n["assignment"] == context["recipient"]["assignment"]
                                    && n["node"] == "_consultation"
                            })
                        });
                        if consultant {
                            assert!(raw.contains("CONSULTATION_REQUIRED"));
                        }
                        if mode == "desktop-worker" && !consultant {
                            std::fs::write(
                                if is_second {
                                    "other.txt"
                                } else if is_third {
                                    "third.txt"
                                } else {
                                    "work.txt"
                                },
                                if full_scenario == "repair-failed-test"
                                    && assignment["node"] == "fix-output"
                                {
                                    "expected"
                                } else {
                                    "after"
                                },
                            )
                            .unwrap();
                        }
                        // The report follows the brief's skeleton (`report.sections`) when this
                        // prompt carries the brief, otherwise every required section heading.
                        let skeleton: Vec<String> = raw
                            .lines()
                            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                            .find(|v| v["report"]["sections"].is_array())
                            .map(|b| {
                                b["report"]["sections"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .filter_map(|s| s["heading"].as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_else(|| {
                                [
                                    "Evidence and work",
                                    "Verification",
                                    "Risks and remaining work",
                                    "Changes",
                                    "Evidence examined",
                                    "Findings",
                                    "Gaps and next steps",
                                    "Executed checks",
                                    "Failures and gaps",
                                    "Assessment",
                                    "Evidence reviewed",
                                    "Proposed work graph",
                                    "Risks and open questions",
                                    "Documentation changes",
                                    "Validation",
                                    "Limits and follow-up",
                                    "Verdict",
                                    "Coverage",
                                ]
                                .map(|h| format!("## {h}"))
                                .to_vec()
                            });
                        let mut report =
                            String::from("# Fixture report\n\nObserved disposable fixture.");
                        for heading in skeleton {
                            report.push_str(&format!(
                                "\n\n{heading}\n\nObserved disposable fixture."
                            ));
                        }
                        let mut arguments = json!({"report":report,"summary":"Fixture result.","checks":[],"request_key":"result"});
                        if mode == "desktop-worker"
                            && full_scenario.starts_with("reconcile-")
                            && !consultant
                        {
                            arguments["checks"] = json!([{"name":"Live behavior","outcome":"not_run","blocker":"paid live check not requested by the owner"}]);
                        }
                        if mode == "desktop-reviewer" {
                            arguments["verdict"] = json!("APPROVE");
                            arguments["findings"] = json!([]);
                        }
                        json!({"name":"result_submit","arguments":arguments})
                    };
                    Some(format!("call:{params}"))
                } else {
                    None
                };
                let text = if let Some(text) = automatic.as_deref() {
                    text
                } else if mode == "native-host" {
                    raw.rsplit_once("\n\n").map_or(raw, |(_, text)| text)
                } else {
                    raw
                };
                if mode == "native-host" && text.starts_with("fixture-usage-") {
                    if text == "fixture-usage-hang" {
                        continue;
                    }
                    let result = match text {
                        "fixture-usage-reported" => json!({
                            "stopReason":"end_turn",
                            "usage":{"inputTokens":4,"outputTokens":2,"cachedReadTokens":9,
                                "totalTokens":15,"cost":99,"cumulative":{"totalTokens":900}}
                        }),
                        "fixture-usage-zero" => {
                            json!({"stopReason":"end_turn","usage":{"totalTokens":0}})
                        }
                        "fixture-usage-invalid" => {
                            json!({"stopReason":"end_turn","usage":{"totalTokens":-5}})
                        }
                        "fixture-usage-refusal" => {
                            json!({"stopReason":"refusal","usage":{"outputTokens":3}})
                        }
                        _ => panic!("unknown usage fixture"),
                    };
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":result}),
                    )
                    .await
                    .unwrap();
                    continue;
                }
                if (mode == "native-host" || mode.starts_with("desktop-"))
                    && text.starts_with("call:")
                {
                    let mut params: Value = serde_json::from_str(&text[5..]).unwrap();
                    let expected = params.as_object_mut().unwrap().remove("__fixture_expect");
                    // Fixture-only message chunks streamed before the tool call and after its reply.
                    let stream = |key: &str, params: &mut Value| {
                        params
                            .as_object_mut()
                            .unwrap()
                            .remove(key)
                            .map(|v| {
                                v.as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|c| c.as_str().unwrap().to_owned())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    };
                    let (before, after) = (
                        stream("__fixture_before", &mut params),
                        stream("__fixture_after", &mut params),
                    );
                    let chunk = |text: &str| {
                        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk",
                        "messageId":"fixture-midstream","content":{"type":"text","text":text}}}})
                    };
                    for text in &before {
                        wire::write_frame(&mut output, &chunk(text)).await.unwrap();
                    }
                    if mode.starts_with("desktop-") {
                        for update in [
                            json!({"sessionUpdate":"tool_call","toolCallId":"fixture-call","title":"Fixture step","status":"in_progress","rawInput":{"private":"not-public"}}),
                            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Native fixture progress."}}),
                            json!({"sessionUpdate":"tool_call_update","toolCallId":"fixture-call","status":"completed"}),
                        ] {
                            wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":update}})).await.unwrap();
                        }
                    }
                    let entry = &servers[&sid];
                    let mut relay =
                        tokio::process::Command::new(entry["command"].as_str().unwrap())
                            .args(
                                entry["args"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|v| v.as_str().unwrap()),
                            )
                            .stdin(std::process::Stdio::piped())
                            .stdout(std::process::Stdio::piped())
                            .kill_on_drop(true)
                            .spawn()
                            .unwrap();
                    let mut stdin = relay.stdin.take().unwrap();
                    let mut stdout = BufReader::new(relay.stdout.take().unwrap());
                    wire::write_frame(
                        &mut stdin,
                        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
                    )
                    .await
                    .unwrap();
                    let listing = wire::read_frame(&mut stdout).await.unwrap().unwrap();
                    let tools = listing["result"]["tools"].as_array().unwrap();
                    assert!(tools.iter().all(|v| [
                        "read_skill",
                        "answer_submit",
                        "synthesis_submit",
                        "verification_submit",
                        "replan_request",
                        "scope_request",
                        "escalation_request",
                        "ask_question",
                        "user_question_request",
                        "plan_submit",
                        "sprint_plan_submit",
                        "list_route_options",
                        "result_submit",
                        "create_draft",
                        "draft_write",
                        "write_report",
                        "list_reports",
                        "read_report",
                        "read_draft",
                        "tools_discover",
                        "outside_command",
                        "outside_write",
                        "list_sources",
                        "read_source",
                        "project_read",
                        "project_write",
                        "project_edit",
                        "project_git",
                        "command_run",
                        "command_status",
                        "read_large_output",
                        "access_request",
                        "project_mkdir",
                        "project_delete",
                        "project_skill_link",
                        "task_get_context",
                        "read_capsule",
                        "check_preflight",
                        "sprint_graph",
                        "sprint_task_result",
                        "read_batch"
                    ]
                    .contains(&v["name"].as_str().unwrap())));
                    if mode == "desktop-coordinator" && scenario.trim() == "routing-menu" {
                        assert!(tools.iter().any(|t| t["name"] == "list_route_options"));
                        wire::write_frame(
                            &mut stdin,
                            &json!({"jsonrpc":"2.0","id":11,"method":"tools/call",
                            "params":{"name":"list_route_options","arguments":{}}}),
                        )
                        .await
                        .unwrap();
                        let menu = wire::read_frame(&mut stdout).await.unwrap().unwrap();
                        assert!(menu.get("error").is_none(), "{menu}");
                        assert!(
                            menu["result"]["structuredContent"]["tiers"].is_object(),
                            "{menu}"
                        );
                    }
                    if mode == "desktop-coordinator" && full_scenario.trim() == "sprint-cross-task"
                    {
                        let call = async |input: &mut tokio::process::ChildStdin,
                                          output: &mut BufReader<tokio::process::ChildStdout>,
                                          id: u64,
                                          name: &str,
                                          arguments: Value| {
                            wire::write_frame(input, &json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})).await.unwrap();
                            wire::read_frame(output).await.unwrap().unwrap()
                        };
                        let graph =
                            call(&mut stdin, &mut stdout, 12, "sprint_graph", json!({})).await;
                        let graph = &graph["result"]["structuredContent"];
                        let member = graph["tasks"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|m| m["taskId"] == graph["taskId"])
                            .unwrap();
                        let predecessor = member["dependencies"][0].as_str().unwrap();
                        let dependent = &member["dependents"][0];
                        let child = graph["tasks"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|m| m["taskId"] == *dependent)
                            .unwrap();
                        assert_eq!(child["dependencies"].as_array().unwrap().len(), 2);
                        let result = call(
                            &mut stdin,
                            &mut stdout,
                            13,
                            "sprint_task_result",
                            json!({"member_id":predecessor}),
                        )
                        .await;
                        let catalogue = &result["result"]["structuredContent"]["reports"]["items"];
                        let report = catalogue
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|r| r["title"] == "Graph handoff")
                            .unwrap();
                        let mut args = json!({"member_id":predecessor,"report_id":report["report_id"],"limit":4000});
                        let first = call(
                            &mut stdin,
                            &mut stdout,
                            14,
                            "sprint_task_result",
                            args.clone(),
                        )
                        .await;
                        let page = &first["result"]["structuredContent"]["report"];
                        assert_eq!(page["sha256"], report["sha256"]);
                        assert!(page["next_cursor"].is_string());
                        args["cursor"] = page["next_cursor"].clone();
                        let last =
                            call(&mut stdin, &mut stdout, 15, "sprint_task_result", args).await;
                        let page = &last["result"]["structuredContent"]["report"];
                        assert_eq!(page["sha256"], report["sha256"]);
                        assert!(page["next_cursor"].is_null());
                        assert!(page["markdown"]
                            .as_str()
                            .unwrap()
                            .contains("Decision: graph handoff"));
                        let denied=call(&mut stdin,&mut stdout,16,"sprint_task_result",json!({"member_id":std::fs::read_to_string(".git/other-member").unwrap()})).await;
                        assert!(denied["error"].is_object() || denied["result"]["isError"] == true);
                        std::fs::write(".git/sprint-tool-result", "result-ok").unwrap();
                    }
                    if mode == "desktop-worker" || mode == "desktop-reviewer" {
                        assert!(!tools.iter().any(|t| t["name"] == "list_route_options"));
                    }
                    if checkpoint && checkpoint_answer {
                        assert_eq!(mode, "desktop-worker");
                        assert_eq!(*n, 2, "answer must continue the original worker");
                        let call = async |input: &mut tokio::process::ChildStdin,
                                          output: &mut BufReader<tokio::process::ChildStdout>,
                                          id: u64, name: &str, arguments: Value| {
                            wire::write_frame(input, &json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
                                "params":{"name":name,"arguments":arguments}})).await.unwrap();
                            let reply = wire::read_frame(output).await.unwrap().unwrap();
                            assert!(reply.get("error").is_none() && reply["result"]["isError"] != true, "{reply}");
                            reply["result"]["structuredContent"].clone()
                        };
                        let mut receipt = call(&mut stdin, &mut stdout, 50, "command_run", json!({
                            "argv":["/bin/echo",CHECKPOINT_COMMAND],
                            "timeout_seconds":10,"request_key":CHECKPOINT_COMMAND
                        })).await;
                        let reference = receipt["tool_call_ref"].as_str().unwrap().to_owned();
                        tokio::time::timeout(std::time::Duration::from_secs(10), async {
                            let mut id = 50;
                            while receipt["settled"] == false {
                                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                                id += 1;
                                receipt = call(&mut stdin, &mut stdout, id, "command_status",
                                    json!({"tool_call_ref":reference})).await;
                            }
                        }).await.expect("checkpoint broker command did not settle");
                        assert_eq!(receipt["settled"], true, "{receipt}");
                        assert_eq!(receipt["exit_code"], 0, "{receipt}");
                        assert_eq!(receipt["output"], format!("{CHECKPOINT_COMMAND}\n"), "{receipt}");
                    }
                    let plain_pending_turn = scenario.trim() == "pending-end-turn"
                        && params["name"] == "result_submit"
                        && *n == 1;
                    if (scenario.trim() == "pending-result"
                        || scenario.trim() == "pending-end-turn")
                        && params["name"] == "result_submit"
                    {
                        if *n == 1 {
                            wire::write_frame(&mut stdin,&json!({"jsonrpc":"2.0","id":40,"method":"tools/call",
                                "params":{"name":"command_run","arguments":{"argv":["/bin/sh","-c","while [ ! -f release-command ]; do sleep 0.1; done"],"timeout_seconds":25,"request_key":"pending-command"}}})).await.unwrap();
                            let pending = wire::read_frame(&mut stdout).await.unwrap().unwrap();
                            assert_eq!(
                                pending["result"]["structuredContent"]["settled"], false,
                                "{pending}"
                            );
                        } else {
                            assert!(
                                raw.contains("final receipts") && raw.contains("\"exit_code\":0")
                            );
                        }
                    }
                    if plain_pending_turn {
                        // Deliberately end the model turn without submitting a result while
                        // the command is still running. The native supervisor must observe the
                        // command from this turn, wait for its receipt, and continue this worker
                        // session instead of declaring its outcome unknown.
                        wire::write_frame(
                            &mut output,
                            &json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}),
                        )
                        .await
                        .unwrap();
                        relay.kill().await.unwrap();
                        relay.wait().await.unwrap();
                        continue;
                    }
                    if mode == "desktop-reviewer" && params["name"] == "result_submit" {
                        // A reviewer finishes its coverage before its verdict: read every
                        // visible project file completely through the channel.
                        let mut stack = vec![std::path::PathBuf::from(".")];
                        let mut call = 100;
                        while let Some(dir) = stack.pop() {
                            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                                if entry.file_name().to_string_lossy().starts_with('.') {
                                    continue;
                                }
                                let Ok(kind) = entry.file_type() else {
                                    continue;
                                };
                                let path = entry.path();
                                if kind.is_dir() {
                                    stack.push(path);
                                    continue;
                                }
                                if !kind.is_file() {
                                    continue;
                                }
                                let rel = path
                                    .strip_prefix(".")
                                    .unwrap_or(&path)
                                    .to_string_lossy()
                                    .into_owned();
                                call += 1;
                                wire::write_frame(
                                    &mut stdin,
                                    &json!({"jsonrpc":"2.0","id":call,"method":"tools/call",
                                    "params":{"name":"project_read","arguments":{"path":rel}}}),
                                )
                                .await
                                .unwrap();
                                wire::read_frame(&mut stdout).await.unwrap().unwrap();
                            }
                        }
                        // ...and every report id its brief names, to the last page.
                        let bytes = raw.as_bytes();
                        let mut reports = std::collections::BTreeSet::new();
                        for start in 0..bytes.len().saturating_sub(13) {
                            let id = &bytes[start..start + 14.min(bytes.len() - start)];
                            if id.len() == 14
                                && id.starts_with(b"r_")
                                && id[2..]
                                    .iter()
                                    .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
                                && (start == 0 || !bytes[start - 1].is_ascii_alphanumeric())
                            {
                                reports.insert(String::from_utf8_lossy(id).into_owned());
                            }
                        }
                        for report in reports {
                            let mut cursor = Value::Null;
                            for _ in 0..64 {
                                call += 1;
                                let mut arguments = json!({"report_id":report});
                                if cursor.is_string() {
                                    arguments["cursor"] = cursor.clone();
                                }
                                wire::write_frame(
                                    &mut stdin,
                                    &json!({"jsonrpc":"2.0","id":call,"method":"tools/call",
                                    "params":{"name":"read_report","arguments":arguments}}),
                                )
                                .await
                                .unwrap();
                                let page = wire::read_frame(&mut stdout).await.unwrap().unwrap();
                                cursor = page["result"]["structuredContent"]["next_cursor"].clone();
                                if !cursor.is_string() {
                                    break;
                                }
                            }
                        }
                    }
                    wire::write_frame(
                        &mut stdin,
                        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":params}),
                    )
                    .await
                    .unwrap();
                    let reply = wire::read_frame(&mut stdout).await.unwrap().unwrap();
                    if let Some(expected) = expected {
                        for (pointer, value) in expected.as_object().unwrap() {
                            assert_eq!(
                                reply.pointer(pointer),
                                Some(value),
                                "fixture reply at {pointer}: {reply}"
                            );
                        }
                    }
                    for text in &after {
                        wire::write_frame(&mut output, &chunk(text)).await.unwrap();
                    }
                    // A refused call (a JSON-RPC refusal or a structured tool error) opened nothing.
                    let accepted =
                        reply.get("error").is_none() && reply["result"]["isError"] != true;
                    if mode == "native-host"
                        && full_scenario.trim() == "usage-exit-after-result"
                        && params["name"] == "result_submit"
                    {
                        assert!(accepted, "{reply}");
                        relay.kill().await.unwrap();
                        relay.wait().await.unwrap();
                        return; // No ACP response for this already accepted result.
                    }
                    if params["name"] == "user_question_request" && accepted {
                        wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update",
                            "params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk",
                            "content":{"type":"text","text":"Post-question coordinator text"}}}})).await.unwrap();
                    }
                    if scenario.trim() == "stop-siblings" {
                        // Result commit precedes the ACP reply and owned process wait.
                        // Let the parent observe this boundary without scheduler timing.
                        tokio::time::timeout(std::time::Duration::from_secs(5), async {
                            while !std::path::Path::new("release-sibling").exists() {
                                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                            }
                        })
                        .await
                        .expect("sibling release signal missing");
                    }
                    // A real engine reports a failed tool call as that call's result and ends
                    // the turn normally; ACP `refusal` means the model declined, which the
                    // runtime's turn loop treats as a degenerate turn. The driver-level
                    // `native-host` fixture keeps `refusal` as its tool-error signal.
                    let stop = if reply.get("error").is_some() && !mode.starts_with("desktop-") {
                        "refusal"
                    } else {
                        "end_turn"
                    };
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":stop}}),
                    )
                    .await
                    .unwrap();
                    relay.kill().await.unwrap();
                    relay.wait().await.unwrap();
                    // The worker process dies while its question is still open.
                    if scenario.trim() == "question-exit"
                        && params["name"] == "ask_question"
                        && accepted
                    {
                        std::process::exit(1);
                    }
                    continue;
                }
                if mode == "native-host" && text == "native-activity" {
                    assert!(base.contains("## Skills already included"));
                    assert!(base.contains("# Cortex tool cheat sheet (code mode)"));
                    for update in [
                        json!({"sessionUpdate":"tool_call","toolCallId":"same-call","title":"Fixture step","status":"in_progress","rawInput":{"secret":"private-fixture-value"}}),
                        json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"private-fixture-value"}}),
                        json!({"sessionUpdate":"tool_call_update","toolCallId":"same-call","status":"completed","rawOutput":"private-fixture-value"}),
                        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Native public response."}}),
                    ] {
                        wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":update}})).await.unwrap();
                    }
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}),
                    )
                    .await
                    .unwrap();
                    continue;
                }
                if mode == "native-host" && text == "native-chunked-next" {
                    // The next turn's text directly follows the previous turn's last chunk.
                    let update = json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Next turn."}});
                    wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":update}})).await.unwrap();
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}),
                    )
                    .await
                    .unwrap();
                    continue;
                }
                if mode == "native-host" && text == "native-chunked" {
                    // A streamed message arrives as many small chunks; a tool row
                    // between chunks ends the message the next chunk begins anew.
                    for update in [
                        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Вы выб"}}),
                        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"рали `div"}}),
                        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ide`."}}),
                        json!({"sessionUpdate":"tool_call","toolCallId":"chunk-call","title":"Fixture step","status":"in_progress"}),
                        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Review "}}),
                        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"text."}}),
                    ] {
                        wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":update}})).await.unwrap();
                    }
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}),
                    )
                    .await
                    .unwrap();
                    continue;
                }
                if text == "exit" {
                    std::process::exit(17)
                }
                if text == "malformed" {
                    output
                        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"id\":2}\n")
                        .await
                        .unwrap();
                    output.flush().await.unwrap();
                    continue;
                }
                if text == "flood" {
                    for _ in 0..300 {
                        wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":{}}})).await.unwrap();
                    }
                    continue;
                }
                if text == "hang" {
                    held.insert(sid, id);
                    continue;
                }
                // `session-meta:<json>`: the turn ends normally when every given
                // JSON pointer of the session's `session/new` `_meta` holds the given
                // value (a missing `_meta` is `null`), with `refusal` otherwise.
                if let Some(body) = text.strip_prefix("session-meta:") {
                    let expected: serde_json::Map<String, Value> =
                        serde_json::from_str(body).unwrap();
                    let meta = session_metas.get(&sid).cloned().unwrap_or(Value::Null);
                    let held = expected.iter().all(|(pointer, value)| {
                        if pointer.is_empty() {
                            &meta == value
                        } else if let Some(tool) = pointer.strip_prefix("disallowed:") {
                            meta["claudeCode"]["options"]["disallowedTools"]
                                .as_array()
                                .is_some_and(|a| a.iter().any(|t| t == tool))
                                == value.as_bool().unwrap_or(false)
                        } else {
                            meta.pointer(pointer).unwrap_or(&Value::Null) == value
                        }
                    });
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,
                        "result":{"stopReason":if held {"end_turn"} else {"refusal"}}}),
                    )
                    .await
                    .unwrap();
                    continue;
                }
                // `native-write:<json>`: an engine-native file edit made inside the
                // turn without any Cortex tool (as codex-acp's sandboxed patch can),
                // relative to the engine's working directory.
                if let Some(body) = text.strip_prefix("native-write:") {
                    let edit: Value = serde_json::from_str(body).unwrap();
                    std::fs::write(
                        std::env::current_dir()
                            .unwrap()
                            .join(edit["path"].as_str().unwrap()),
                        edit["content"].as_str().unwrap(),
                    )
                    .unwrap();
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}),
                    )
                    .await
                    .unwrap();
                    continue;
                }
                if text == "permission" {
                    let pid = json!(format!("permission-{sid}"));
                    permissions.insert(pid.to_string(), (id, sid.clone()));
                    wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","id":pid,"method":"session/request_permission","params":{"sessionId":sid,"toolCall":{"title":"Fixture"},"options":[{"optionId":"yes","name":"Allow","kind":"allow_once"}]}})).await.unwrap();
                    continue;
                }
                // An exact engine-shaped request body (for example claude-agent-acp's).
                if let Some(body) = text.strip_prefix("permission-request:") {
                    let mut params: Value = serde_json::from_str(body).unwrap();
                    params["sessionId"] = json!(sid);
                    // Engines never reuse a request id; the client rejects replays.
                    static NEXT: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(0);
                    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let pid = json!(format!("permission-{sid}-{n}"));
                    permissions.insert(pid.to_string(), (id, sid.clone()));
                    wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","id":pid,"method":"session/request_permission","params":params})).await.unwrap();
                    continue;
                }
                wire::write_frame(&mut output,&json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}})).await.unwrap();
                wire::write_frame(
                    &mut output,
                    &json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}),
                )
                .await
                .unwrap();
            }
            "session/cancel" => {
                releasable.remove(&sid);
                if let Some(id) = held.remove(&sid) {
                    wire::write_frame(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"cancelled"}}),
                    )
                    .await
                    .unwrap();
                }
            }
            _ => {}
        }
    }
}
