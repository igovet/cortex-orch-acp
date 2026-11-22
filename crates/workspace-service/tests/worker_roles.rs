//! Every worker profile, end to end through the real runtime, gateway and relay.
//!
//! Custom harness: this executable is also its own recording no-model ACP peer
//! (`role-peer`) and the MCP relay. The peer never starts a model or a network
//! connection: it records the exact prompt it was given, the tool catalogue it was
//! served and every tool reply, under its private HOME, outside the project.
//!
//! Each step of each role is classified and printed as one row:
//! PASS (the current contract holds), FAIL (it does not; the run fails), or
//! NOT_RUN (not executable with the no-model peer, with the reason). Nothing that
//! was not observed is reported as a pass.
#[path = "support/hermetic.rs"]
mod hermetic;
#[allow(dead_code, unused_imports)]
#[path = "support/namespace_host.rs"]
mod namespace_host;
#[allow(unused_imports)]
use namespace_host::native_effects;
#[allow(unused_imports)]
use workspace_service::tool_path;

use cortex_core::{ownership::OwnerEpoch, OpenMode, ProjectKey, Store, StoreChannel, TaskState};
use cortex_engine::environment::{EngineKind, LaunchSpec};
use cortex_engine::wire;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::BufReader;
use workspace_service::native_sessions::{NativeHost, SessionDriver, SessionRole, SessionRoute};

const PEER: &str = "role-peer";
const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const FIXTURE_INSTRUCTIONS: &str = "Fixture route instructions; no model or credentials.";
/// Every planner-assignable specialist (`lifecycle::plan::role`).
const PLANNED: [&str; 20] = [
    "explorer",
    "accessibility-auditor",
    "accessibility-fixer",
    "architect",
    "backend-dev",
    "data-engineer",
    "database-architect",
    "debugger",
    "devops-engineer",
    "frontend-dev",
    "fullstack-dev",
    "general",
    "mobile-dev",
    "performance-engineer",
    "planner",
    "qa-engineer",
    "refactorer",
    "security-auditor",
    "technical-writer",
    "ux-designer",
];
/// Profiles that never write (`scheduler::policy::read_only`), stated here
/// independently so the mode check does not reuse the code under test.
const READ_ONLY: [&str; 12] = [
    "worker-architect",
    "worker-ux-designer",
    "worker-code-reviewer",
    "worker-senior-consultant",
    "senior-consultant",
    "worker-explorer",
    "worker-planner",
    "worker-security-auditor",
    "worker-accessibility-auditor",
    "worker-database-architect",
    "worker-performance-engineer",
    "worker-build-verification",
];
const PROJECT_SKILL: &str =
    "---\nname: topic-research\ndescription: Research a topic.\n---\n# Topic research fixture\n";

// ---------------------------------------------------------------- recording peer

fn options(engine: &str, model: &str, mode: &str, effort: &str) -> Value {
    let select = |id: &str, category: &str, current: &str, values: &[&str]| {
        json!({"id":id,"name":id,"category":category,"type":"select","currentValue":current,
            "options":values.iter().map(|v| json!({"value":v,"name":v})).collect::<Vec<_>>()})
    };
    let modes: Vec<Value> = [
        "agent",
        "agent-full-access",
        "workspace-write",
        "default",
        "acceptEdits",
        "bypassPermissions",
        "cortex",
        "read-only",
        "plan",
    ]
    .iter()
    .map(|m| json!({"value":m,"name":m}))
    .collect();
    let mut list = vec![
        json!({"id":"model","type":"select","currentValue":model,"options":[
            {"value":"fixture-model","name":"Fixture"},{"value":"fixture-other","name":"Other fixture"}]}),
        json!({"id":"mode","type":"select","currentValue":mode,"options":modes}),
    ];
    if engine == "codex" {
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
    } else {
        list.push(select(
            "effort",
            "thought_level",
            effort,
            &["low", "medium", "high"],
        ));
        list.push(select("fast", "model_config", "off", &["on", "off"]));
        list.push(select("ultracode", "model_config", "off", &["on", "off"]));
    }
    json!(list)
}

/// `call:[{"name":..,"arguments":..},..]` runs every call over one relay connection;
/// anything else ends the turn. Every prompt is recorded as `<HOME>/turn-<n>.json`.
async fn role_peer() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let engine = if std::env::var_os("CODEX_HOME").is_some() {
        "codex"
    } else {
        "claude"
    };
    let mut input = BufReader::new(tokio::io::stdin());
    let mut output = tokio::io::stdout();
    let mut servers = HashMap::<String, Value>::new();
    let (mut sessions, mut turns) = (0, 0);
    let mut meta = Value::Null;
    let (mut model, mut mode, mut effort) = (
        "fixture-model".to_owned(),
        "agent".to_owned(),
        "medium".to_owned(),
    );
    while let Ok(Some(r)) = wire::read_frame(&mut input).await {
        let id = r.get("id").cloned().unwrap_or(Value::Null);
        let sid = r["params"]["sessionId"].as_str().unwrap_or("").to_owned();
        let reply = |result: Value| json!({"jsonrpc":"2.0","id":id,"result":result});
        match r["method"].as_str().unwrap_or("") {
            "initialize" => {
                wire::write_frame(&mut output, &reply(json!({"protocolVersion":1,
                    "agentCapabilities":{"promptCapabilities":{"image":true,"embeddedContext":true}}})))
                .await
                .unwrap();
            }
            "session/new" => {
                sessions += 1;
                let sid = format!("role-{sessions}");
                meta = r["params"]["_meta"].clone();
                let listed = r["params"]["mcpServers"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                servers.insert(sid.clone(), listed.first().cloned().unwrap_or(Value::Null));
                wire::write_frame(
                    &mut output,
                    &reply(json!({"sessionId":sid,
                    "configOptions":options(engine,&model,&mode,&effort)})),
                )
                .await
                .unwrap();
            }
            "session/set_config_option" => {
                let value = r["params"]["value"].as_str().unwrap_or_default().to_owned();
                match r["params"]["configId"].as_str().unwrap_or("") {
                    "mode" => mode = value,
                    "model" => model = value,
                    "reasoning_effort" | "effort" => effort = value,
                    _ => {}
                }
                wire::write_frame(
                    &mut output,
                    &reply(json!({"configOptions":options(engine,&model,&mode,&effort)})),
                )
                .await
                .unwrap();
            }
            "session/prompt" => {
                turns += 1;
                let raw = r["params"]["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                let tail = raw
                    .rsplit_once("\n\n")
                    .map_or(raw.as_str(), |(_, t)| t)
                    .to_owned();
                let system = meta["systemPrompt"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| {
                        let v: Value =
                            serde_json::from_str(&std::env::var("CODEX_CONFIG").ok()?).ok()?;
                        std::fs::read_to_string(v["model_instructions_file"].as_str()?).ok()
                    });
                let mut record = json!({"system":system,"prompt":raw,"blocks":r["params"]["prompt"].as_array().map_or(0,|a|a.len()),
                    "mode":mode,"meta":meta});
                let mut refused = false;
                if let Some(calls) = tail.strip_prefix("call:") {
                    let calls: Vec<Value> = serde_json::from_str(calls).unwrap();
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
                    record["tools"] = wire::read_frame(&mut stdout).await.unwrap().unwrap();
                    let mut replies = Vec::new();
                    for (n, call) in calls.iter().enumerate() {
                        wire::write_frame(
                            &mut stdin,
                            &json!({"jsonrpc":"2.0","id":n+2,"method":"tools/call","params":call}),
                        )
                        .await
                        .unwrap();
                        let reply = match wire::read_frame(&mut stdout).await {
                            Ok(Some(v)) => v,
                            other => json!({"fixture_transport":format!("{other:?}")}),
                        };
                        refused |= reply.get("error").is_some();
                        replies.push(reply);
                    }
                    record["replies"] = json!(replies);
                    relay.kill().await.ok();
                    relay.wait().await.ok();
                }
                std::fs::write(home.join(format!("turn-{turns}.json")), record.to_string())
                    .unwrap();
                wire::write_frame(
                    &mut output,
                    &reply(json!({"stopReason":if refused {"refusal"} else {"end_turn"}})),
                )
                .await
                .unwrap();
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- fixture

struct Project {
    root: PathBuf,
    homes: PathBuf,
}
impl Project {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("worker-roles-{}-{label}-{n}", std::process::id()));
        let root = base.join("project");
        let homes = base.join("peers");
        for dir in [&base, &root, &homes] {
            std::fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
        }
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn fixture() -> u8 { 1 }\n").unwrap();
        std::fs::write(root.join("README.md"), "# Fixture project\n").unwrap();
        std::fs::create_dir_all(root.join(".agents/skills/topic-research")).unwrap();
        std::fs::write(
            root.join(".agents/skills/topic-research/SKILL.md"),
            PROJECT_SKILL,
        )
        .unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("/usr/bin/git")
                .args([
                    "-c",
                    "user.name=fixture",
                    "-c",
                    "user.email=fixture@invalid",
                    "-c",
                    "commit.gpgsign=false"
                ])
                .args(args)
                .current_dir(&root)
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "fixture"]);
        Self { root, homes }
    }
    fn key(&self) -> ProjectKey {
        ProjectKey::new(&self.root, StoreChannel::Dev).unwrap()
    }
    fn store(&self) -> Store {
        Store::open(self.key(), OpenMode::RuntimeValidate).unwrap()
    }
    async fn host(&self) -> NativeHost {
        let key = self.key();
        tokio::task::spawn_blocking(move || NativeHost::open(key))
            .await
            .unwrap()
            .unwrap()
    }
    /// One private peer HOME per session: its recorded turns are that session's only.
    fn route(&self, engine: EngineKind, label: &str) -> (SessionRoute, PathBuf) {
        let home = self.homes.join(label);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&home)
            .unwrap();
        let executable = std::env::current_exe().unwrap();
        (
            SessionRoute {
                launch: LaunchSpec {
                    engine,
                    executable: executable.clone(),
                    args: vec![PEER.into()],
                    cwd: self.root.clone(),
                    native_home: home.clone(),
                    credential_directory: home.clone(),
                    base_environment: BTreeMap::new(),
                    codex_launcher: None,
                },
                relay: executable,
                broker_parent: self.root.join(".cortex-dev"),
                connection: "fake-only".into(),
                connection_limit: 4,
                model: "fixture-model".into(),
                parameters: BTreeMap::new(),
                instructions: FIXTURE_INSTRUCTIONS.into(),
                deadline: Duration::from_secs(5),
            },
            home,
        )
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.root.parent().unwrap());
    }
}

// ---------------------------------------------------------------- results

#[derive(Default)]
struct Ledger {
    rows: Vec<(String, String, &'static str, String)>,
}
impl Ledger {
    fn check(&mut self, role: &str, step: &str, ok: bool, detail: impl Into<String>) -> bool {
        self.rows.push((
            role.into(),
            step.into(),
            if ok { "PASS" } else { "FAIL" },
            detail.into(),
        ));
        ok
    }
    /// A contradiction between what the role is told and what the runtime does that
    /// the runtime does not promise to enforce: printed and counted, never a pass.
    /// No current step is a gap; kept for the next contradiction found.
    #[allow(dead_code)]
    fn gap(&mut self, role: &str, step: &str, ok: bool, detail: impl Into<String>) {
        self.rows.push((
            role.into(),
            step.into(),
            if ok { "PASS" } else { "GAP" },
            detail.into(),
        ));
    }
    fn not_run(&mut self, role: &str, step: &str, why: &str) {
        self.rows
            .push((role.into(), step.into(), "NOT_RUN", why.into()));
    }
    fn print_and_assert(&self) {
        let mut roles = BTreeMap::<&str, [usize; 4]>::new();
        for (role, step, status, detail) in &self.rows {
            if *status != "PASS" {
                println!("{status:7} {role:28} {step:34} {detail}");
            }
            let e = roles.entry(role).or_default();
            e[["PASS", "FAIL", "GAP", "NOT_RUN"]
                .iter()
                .position(|s| s == status)
                .unwrap()] += 1;
        }
        for (role, [pass, fail, gap, not_run]) in &roles {
            println!("role {role:30} pass={pass:3} fail={fail:2} gap={gap:2} not_run={not_run:2}");
        }
        let failed = self.rows.iter().filter(|r| r.2 == "FAIL").count();
        assert_eq!(
            failed, 0,
            "{failed} worker-role checks failed; see the FAIL rows above"
        );
    }
}

/// A reply is `ok` (no JSON-RPC error, not isError), or carries its precise error.
fn classify(reply: &Value) -> Result<&Value, String> {
    if let Some(error) = reply.get("error") {
        return Err(format!("rpc {}", compact(error)));
    }
    if reply.get("fixture_transport").is_some() {
        return Err(format!("transport {}", reply["fixture_transport"]));
    }
    if reply["result"]["isError"] == true {
        return Err(format!(
            "isError {}",
            compact(&reply["result"]["structuredContent"])
        ));
    }
    Ok(&reply["result"])
}
fn compact(v: &Value) -> String {
    let s = v.to_string();
    s.chars().take(220).collect()
}
/// A precise error names its code and something more than a bare authority refusal.
fn precise_error(reply: &Value) -> Result<String, String> {
    let structured = &reply["result"]["structuredContent"];
    if reply["result"]["isError"] == true && structured["code"].is_string() {
        let code = structured["code"].as_str().unwrap().to_owned();
        let extra = structured.as_object().map_or(0, |o| o.len());
        return if code == "authority_refused" && extra <= 1 {
            Err(format!("bare {code}"))
        } else {
            Ok(code)
        };
    }
    if let Some(error) = reply.get("error") {
        let message = error["message"].as_str().unwrap_or("");
        let data = &error["data"];
        if !data.is_null() && data != &json!({}) && message != "authority_refused" {
            return Ok(format!("rpc {message}"));
        }
        return Err(format!("rpc without detail: {}", compact(error)));
    }
    Err(format!("not an error: {}", compact(reply)))
}

struct Turn {
    system: String,
    prompt: String,
    tools: Vec<Value>,
    replies: Vec<Value>,
    stop: String,
    /// The engine mode the session was configured with and its `session/new` `_meta`.
    mode: String,
    meta: Value,
}
async fn turn(d: &SessionDriver, home: &Path, calls: &[Value]) -> Turn {
    let before = std::fs::read_dir(home)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("turn-"))
        .count();
    let text = if calls.is_empty() {
        "inspect".to_owned()
    } else {
        format!("call:{}", Value::Array(calls.to_vec()))
    };
    let stop = match d.prompt(&text).await {
        Ok(t) => t.stop_reason,
        Err(e) => format!("error:{}", e.code()),
    };
    let file = home.join(format!("turn-{}.json", before + 1));
    let record: Value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    Turn {
        system: record["system"].as_str().unwrap_or("").to_owned(),
        prompt: record["prompt"].as_str().unwrap_or("").to_owned(),
        tools: record["tools"]["result"]["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        replies: record["replies"].as_array().cloned().unwrap_or_default(),
        stop,
        mode: record["mode"].as_str().unwrap_or("").to_owned(),
        meta: record["meta"].clone(),
    }
}
fn call(name: &str, arguments: Value) -> Value {
    json!({"name":name,"arguments":arguments})
}

// ---------------------------------------------------------------- shared contract data

fn instructions() -> &'static Value {
    static DATA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(include_str!("../schema/instructions.json")).unwrap())
}
/// profiles.json `report_template` per engine-facing role.
fn report_template(role: &str) -> String {
    let profiles: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{REPO}/profiles.json")).unwrap())
            .unwrap();
    let name = role.trim_start_matches("worker-").replace('-', "_");
    profiles["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name.as_str())
        .and_then(|p| p["report_template"].as_str())
        .unwrap_or_else(|| panic!("no profile {name}"))
        .to_owned()
}
fn template_headings(template: &str) -> Vec<String> {
    std::fs::read_to_string(format!("{REPO}/report-templates/{template}.md"))
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("## "))
        .map(str::to_owned)
        .collect()
}
/// A report written into the runtime's skeleton for this role and stage, optionally
/// without one heading.
fn report_body(
    role: &str,
    profile: &str,
    stage: &str,
    without: Option<&str>,
) -> (String, Vec<String>) {
    let template = cortex_core::assignment_brief::template_for(profile, stage);
    let headings: Vec<String> = cortex_core::assignment_brief::sections(template, stage)
        .iter()
        .map(|s| format!("## {}", s.heading))
        .filter(|h| Some(h.as_str()) != without)
        .collect();
    let mut body = format!("# {role} fixture report\n\n");
    for h in &headings {
        body.push_str(&format!(
            "{h}\n\nObserved disposable fixture evidence for {role}.\n\n"
        ));
    }
    (body, headings)
}
fn names(tools: &[Value]) -> BTreeSet<String> {
    tools
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect()
}

// ---------------------------------------------------------------- per-role battery

struct Work<'a> {
    role: String,
    stage: &'a str,
    assignment: String,
    node: String,
    objective: String,
    scope: Vec<String>,
    predecessors: Vec<String>,
}

/// Instruction bundle, catalogue, every read tool, writes and the result protocol.
async fn battery(
    ledger: &mut Ledger,
    p: &Project,
    h: &NativeHost,
    o: &OwnerEpoch,
    task: &str,
    w: &Work<'_>,
    engine: EngineKind,
    verdict: Option<(&str, Value)>,
    reviewed: &[(String, String)],
) -> Option<String> {
    let label = format!(
        "{}{}",
        w.role,
        if w.stage == "review" { "@review" } else { "" }
    );
    let role = label.as_str();
    let (route, home) = p.route(engine, &format!("{}-{}", w.assignment, w.node));
    if w.stage == "execution" {
        // native_runtime records the chosen route before every worker dispatch.
        p.store()
            .write(|db| {
                cortex_core::lifecycle::event_tx(db, task, "NATIVE_ROUTE_SELECTED",
                    json!({"assignment_ref":w.assignment,"node":w.node,"revision":1,"route_engine":"fixture","engine":"fixture-engine",
                        "model":"fixture-model","connection_id":"fake-only","parameters":{"effort":"medium"},"decision":{"basis":"fixture_basis"}}))?;
                Ok(())
            })
            .unwrap();
    }
    let d = match h
        .dispatch(
            task.into(),
            o.clone(),
            SessionRole::Worker {
                assignment: w.assignment.clone(),
            },
            route,
        )
        .await
    {
        Ok(d) => d,
        Err(e) => {
            ledger.check(role, "session starts", false, e.code());
            return None;
        }
    };
    let expected: BTreeSet<String> =
        names(&cortex_engine::schema::catalogue("worker", Some(&w.role), Some(w.stage)).unwrap());
    let served = |t: &str| expected.contains(t);

    // 1. Session-start bundle, structurally.
    let first = turn(&d, &home, &[]).await;
    let prompt = first.prompt.clone();
    let system = if engine == EngineKind::OpenCode {
        &prompt
    } else {
        &first.system
    };
    if engine != EngineKind::OpenCode {
        ledger.check(
            role,
            "role instructions only in the system layer",
            !prompt.contains(FIXTURE_INSTRUCTIONS) && system.contains("tools_discover"),
            "",
        );
    }
    ledger.check(role, "turn ends", first.stop == "end_turn", &first.stop);
    // The engine mode follows the channel's capability: a read-only profile or a
    // review-stage session is never in a mode that writes or skips the engine's
    // permission requests; no session runs in bypass/full access.
    let read_only = w.stage == "review" || READ_ONLY.contains(&w.role.as_str());
    let mode = match (engine, read_only) {
        (EngineKind::Codex, true) => "read-only",
        (EngineKind::Codex, false) => "workspace-write",
        _ => "default",
    };
    ledger.check(
        role,
        "engine mode by capability",
        first.mode == mode,
        format!("{} (expected {mode})", first.mode),
    );
    let options = &first.meta["claudeCode"]["options"];
    let denied: BTreeSet<&str> = options["disallowedTools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if engine == EngineKind::Claude {
        let native = ["Bash", "Edit", "Write", "NotebookEdit", "Agent", "Task"];
        ledger.check(
            role,
            "claude native mutation tools disallowed",
            native.iter().all(|t| denied.contains(t))
                && options["allowDangerouslySkipPermissions"] == false
                && options["strictMcpConfig"] == true
                && options["managedSettings"]["disableAllHooks"] == true,
            compact(&first.meta),
        );
    } else {
        ledger.check(
            role,
            "no claude options off claude",
            first.meta.is_null(),
            compact(&first.meta),
        );
    }
    let profile = &instructions()["profiles"][&w.role];
    let body = profile["body"].as_str().unwrap_or("");
    ledger.check(
        role,
        "profile body delivered",
        !body.is_empty() && system.contains(body.trim()),
        "instructions.json body",
    );
    ledger.check(
        role,
        "inlined skills section",
        system.contains("## Skills already included"),
        "",
    );
    ledger.check(
        role,
        "route instructions",
        system.contains(FIXTURE_INSTRUCTIONS),
        "",
    );
    let cheat = system.contains("# Cortex tool cheat sheet (code mode)");
    if engine == EngineKind::Codex {
        let missing: Vec<_> = expected
            .iter()
            .filter(|t| {
                !system.contains(&format!("\n{t}:"))
                    && !system.contains(&format!("- {t}"))
                    && !system.contains(&format!("`{t}`"))
            })
            .cloned()
            .collect();
        ledger.check(role, "codex cheat sheet", cheat, "");
        ledger.check(
            role,
            "cheat sheet names every served tool",
            missing.is_empty(),
            format!("{missing:?}"),
        );
    } else {
        ledger.check(role, "no cheat sheet off codex", !cheat, "");
    }
    let project_skills = served("project_read");
    ledger.check(
        role,
        "project skill index iff project_read",
        prompt.contains("topic-research") == project_skills,
        format!("served project_read={project_skills}"),
    );
    let capsule = prompt
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["original_goal"].is_string() && v["recipient"].is_object())
        .unwrap_or(Value::Null);
    ledger.check(role, "capsule delivered", capsule.is_object(), "");
    ledger.check(
        role,
        "capsule goal",
        capsule["original_goal"]
            .as_str()
            .is_some_and(|g| !g.trim().is_empty()),
        "",
    );
    ledger.check(
        role,
        "capsule names recipient assignment",
        capsule["recipient"]["assignment"] == w.assignment.as_str(),
        compact(&capsule["recipient"]),
    );
    let verification = &capsule["plan_summary"]["verification"];
    if capsule["plan_summary"].is_null() && w.node == "build" {
        ledger.not_run(
            role,
            "acceptance checks in plan_summary",
            "stored-node fixture has no accepted plan",
        );
    } else {
        ledger.check(
            role,
            "acceptance checks in plan_summary",
            verification.as_array().is_some_and(|a| !a.is_empty()) || verification.is_object(),
            compact(verification),
        );
    }
    ledger.check(
        role,
        "constraints field present",
        capsule["constraints"].is_array(),
        "",
    );
    ledger.check(
        role,
        "write denials field present",
        capsule["owner_write_denials"].is_object(),
        "",
    );
    let nodes = capsule["current_nodes"].to_string();
    for pred in &w.predecessors {
        ledger.check(
            role,
            "predecessor report ref in capsule",
            nodes.contains(pred.as_str()),
            pred,
        );
    }
    // What this prompt gave the engine is recorded durably: digests and sizes of the
    // bundle, capsule and brief, the served catalogue digest, once per prompt.
    let attempt = capsule["recipient"]["attempt"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    let delivery: Option<(String, Option<String>)> = p
        .store()
        .read(|db| {
            Ok(db
                .query_row(
                    "SELECT e.payload_json,(SELECT instruction_delivery FROM delegation_provisioning WHERE task_id=e.task_id AND attempt_id=?2)
                     FROM runtime_events e WHERE e.task_id=?1 AND e.type='NATIVE_ATTEMPT_DELIVERY'
                     AND json_extract(e.payload_json,'$.attempt')=?2 ORDER BY e.id LIMIT 1",
                    (task, &attempt),
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok())
        })
        .unwrap();
    let catalogue_digest = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(
                &cortex_engine::schema::catalogue("worker", Some(&w.role), Some(w.stage)).unwrap()
            )
            .unwrap()
        )
    );
    match delivery {
        Some((event, provisioning)) => {
            let event: Value = serde_json::from_str(&event).unwrap();
            let sha = |t: &str| format!("{:x}", Sha256::digest(t.as_bytes()));
            ledger.check(
                role,
                "delivery record: brief digest and size",
                event["brief"]["sha256"] == sha("inspect") && event["brief"]["bytes"] == 7,
                compact(&event["brief"]),
            );
            ledger.check(
                role,
                "delivery record: bundle and capsule digests",
                event["instructions"]["bytes"]
                    .as_u64()
                    .is_some_and(|b| b > 0)
                    && event["capsule"]["sha256"]
                        .as_str()
                        .is_some_and(|s| s.len() == 64)
                    && system.contains(FIXTURE_INSTRUCTIONS),
                "",
            );
            ledger.check(
                role,
                "delivery record: tool catalogue digest",
                event["tool_catalogue_sha256"] == catalogue_digest.as_str(),
                "",
            );
            ledger.check(
                role,
                "delivery record: first delivery fills instruction_delivery",
                event["first"] == true
                    && provisioning
                        .as_deref()
                        .is_some_and(|p| p.contains(&catalogue_digest)),
                compact(&json!(provisioning)),
            );
        }
        None => {
            ledger.check(role, "delivery record written", false, &attempt);
        }
    }
    if w.stage == "execution" {
        // The fixture recorded the route the way native_runtime does before dispatch.
        let audit: Option<(String, String, String, String)> = p
            .store()
            .read(|db| {
                Ok(db
                    .query_row(
                        "SELECT role,engine,model,decision_detail_json FROM attempt_route_audit WHERE task_id=? AND attempt_id=?",
                        (task, &attempt),
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .ok())
            })
            .unwrap();
        ledger.check(
            role,
            "route decision mirrored into attempt_route_audit",
            audit.as_ref().is_some_and(|a| {
                a.0 == w.role
                    && a.1 == "fixture-engine"
                    && a.2 == "fixture-model"
                    && a.3.contains("\"effort\":\"medium\"")
                    && a.3.contains("fixture_basis")
            }),
            format!("{audit:?}"),
        );
    } else {
        ledger.not_run(role, "attempt_route_audit row", "no route is recorded for the fixture reviewer, so review independence stays unverified as before");
    }
    // The language directive belongs to the runtime driver's route instructions,
    // not to this session layer; native_desktop.rs asserts it for both stages.
    ledger.not_run(
        role,
        "language directive",
        "composed by native_runtime route instructions, outside the session layer this test drives",
    );

    // 2. Assignment brief: exactly the fields the runtime promises.
    // The assignment rides the driver prompt in production (the advance step's
    // assignment object, checked by the journeys); here compare the stored node.
    let node: Option<(String, String, String, String)> = p
        .store()
        .read(|db| {
            Ok(db
                .query_row(
                    "SELECT role,objective,scope_json,stage FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
                     WHERE n.task_id=? AND n.revision=t.revision AND n.assignment_ref=?",
                    (task, &w.assignment),
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .ok())
        })
        .unwrap();
    match node {
        Some((r, objective, scope, stage)) => {
            ledger.check(role, "node role", r == w.role, &r);
            ledger.check(role, "node objective", objective == w.objective, "");
            let scope: Vec<String> = serde_json::from_str(&scope).unwrap_or_default();
            ledger.check(
                role,
                "node scope matches plan",
                scope == w.scope,
                format!("{scope:?}"),
            );
            ledger.check(role, "node stage", stage == w.stage, &stage);
        }
        None => {
            ledger.check(role, "node stored", false, "no current node");
        }
    }

    // 3. Catalogue served == catalogue advertised for this role and stage.
    let mut calls = vec![call("tools_discover", json!({}))];
    let tools_turn = turn(&d, &home, &calls).await;
    let served_names = names(&tools_turn.tools);
    ledger.check(
        role,
        "served catalogue == schema catalogue",
        served_names == expected,
        format!(
            "missing {:?} extra {:?}",
            expected.difference(&served_names).collect::<Vec<_>>(),
            served_names.difference(&expected).collect::<Vec<_>>()
        ),
    );

    // 4. Every read tool, one relay connection.
    calls.clear();
    let mut steps: Vec<(String, bool)> = Vec::new(); // (step, expect success)
    let push = |calls: &mut Vec<Value>,
                steps: &mut Vec<(String, bool)>,
                step: &str,
                ok: bool,
                c: Value| {
        calls.push(c);
        steps.push((step.to_owned(), ok));
    };
    for t in &expected {
        push(
            &mut calls,
            &mut steps,
            &format!("tools_discover name={t}"),
            true,
            call("tools_discover", json!({"name":t})),
        );
    }
    if served("project_read") {
        push(
            &mut calls,
            &mut steps,
            "project_read src/lib.rs",
            true,
            call("project_read", json!({"path":"src/lib.rs"})),
        );
    }
    push(
        &mut calls,
        &mut steps,
        "list_reports",
        true,
        call("list_reports", json!({})),
    );
    for pred in &w.predecessors {
        push(
            &mut calls,
            &mut steps,
            "read_report predecessor",
            true,
            call("read_report", json!({"report_id":pred})),
        );
    }
    push(
        &mut calls,
        &mut steps,
        "read_skill tool-discipline",
        true,
        call("read_skill", json!({"skill":"tool-discipline"})),
    );
    // Every id the role's own read_skill enum advertises must be readable.
    let read_skill = cortex_engine::schema::catalogue("worker", Some(&w.role), Some(w.stage))
        .unwrap()
        .into_iter()
        .find(|t| t["name"] == "read_skill");
    let advertised: Vec<String> = read_skill
        .as_ref()
        .and_then(|t| {
            t["inputSchema"]["properties"]["skill"]["anyOf"][0]["enum"]
                .as_array()
                .cloned()
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    for id in &advertised {
        push(
            &mut calls,
            &mut steps,
            &format!("read_skill advertised {id}"),
            true,
            call("read_skill", json!({"skill":id})),
        );
    }
    push(
        &mut calls,
        &mut steps,
        "read_skill project skill",
        project_skills,
        call("read_skill", json!({"skill":"topic-research"})),
    );
    if served("list_sources") {
        push(
            &mut calls,
            &mut steps,
            "list_sources",
            true,
            call("list_sources", json!({})),
        );
    }
    if served("read_capsule") {
        push(
            &mut calls,
            &mut steps,
            "read_capsule",
            true,
            call("read_capsule", json!({})),
        );
    }
    if served("check_preflight") {
        push(
            &mut calls,
            &mut steps,
            "check_preflight",
            true,
            call(
                "check_preflight",
                json!({"argv":["/usr/bin/cmp","/dev/null","/dev/null"]}),
            ),
        );
    }
    if served("project_git") {
        push(
            &mut calls,
            &mut steps,
            "project_git status",
            true,
            call(
                "project_git",
                json!({"kind":"status","request_key":format!("status-{}",w.node)}),
            ),
        );
        push(
            &mut calls,
            &mut steps,
            "project_git diff",
            true,
            call(
                "project_git",
                json!({"kind":"diff","request_key":format!("diff-{}",w.node)}),
            ),
        );
    }
    if served("read_batch") {
        let mut items = vec![json!({"id":"l","tool":"list_reports","arguments":{}})];
        if let Some(pred) = w.predecessors.first() {
            items.push(json!({"id":"r","tool":"read_report","arguments":{"report_id":pred}}));
        }
        push(
            &mut calls,
            &mut steps,
            "read_batch",
            true,
            call("read_batch", json!({"calls":items})),
        );
    }
    if served("create_draft") {
        push(
            &mut calls,
            &mut steps,
            "create_draft role template",
            true,
            call(
                "create_draft",
                json!({"template":report_template(&w.role),"request_key":format!("draft-{}",w.node)}),
            ),
        );
    }
    // Out-of-scope / unserved writes: a precise structured error, never a bare refusal.
    let write_role = served("project_write");
    let private = ".cortex-dev/fixture-escape.txt";
    push(
        &mut calls,
        &mut steps,
        if write_role {
            "project_write private path refused precisely"
        } else {
            "unserved project_write refused precisely"
        },
        false,
        call(
            "project_write",
            json!({"path":private,"content":"x","expected_sha256":"absent","request_key":format!("escape-{}",w.node)}),
        ),
    );
    // access_request reserves write paths; a read-only role is never offered it
    // (it can never succeed there), and a call is refused as not offered.
    if !write_role {
        ledger.check(
            role,
            "access_request not served to a read-only role",
            !served("access_request"),
            "",
        );
        push(
            &mut calls,
            &mut steps,
            "access_request on read-only role",
            false,
            call(
                "access_request",
                json!({"paths":["README.md"],"reason":"fixture","request_key":format!("access-{}",w.node)}),
            ),
        );
    }
    let command_step = "command_run trivial check";
    // Probe independently before dispatch; an unexpected tool error cannot
    // select the restricted-host assertion.
    let namespace_unavailable = served("command_run") && namespace_host::assert_unavailable_view();
    if served("command_run") {
        push(
            &mut calls,
            &mut steps,
            command_step,
            true,
            call(
                "command_run",
                json!({"argv":["/usr/bin/cmp","/dev/null","/dev/null"],"timeout_seconds":30,"request_key":format!("cmd-{}",w.node)}),
            ),
        );
    }
    let reads = turn(&d, &home, &calls).await;
    let mut large_output = None;
    let mut draft = None;
    let mut source = None;
    let mut batch_deferred = Vec::new();
    for (n, (step, want_ok)) in steps.iter().enumerate() {
        let Some(reply) = reads.replies.get(n) else {
            ledger.check(role, step, false, format!("no reply (turn {})", reads.stop));
            continue;
        };
        let batch = &reply["result"]["structuredContent"];
        if *step == command_step {
            if namespace_unavailable {
                ledger.check(
                    role,
                    "command_run fails closed without namespaces",
                    reply.get("error").is_none()
                        && reply["result"]["isError"] == true
                        && batch["code"] == "native_command_sandbox_unavailable",
                    compact(reply),
                );
                ledger.not_run(role, step, "real command sandbox requires namespaces");
            } else {
                ledger.check(
                    role,
                    step,
                    classify(reply).is_ok() && batch["complete"] == true && batch["exit_code"] == 0,
                    compact(reply),
                );
            }
            continue;
        }
        if step == "read_batch" {
            // A member result above the 6144-byte inline bound is retained and deferred:
            // a normal outcome, so the batch is complete and not an MCP error.
            let items = batch["results"].as_array().cloned().unwrap_or_default();
            let deferred = items.iter().filter(|i| i["status"] == "deferred").count();
            let only_ok_or_deferred = items
                .iter()
                .all(|i| i["status"] == "ok" || i["status"] == "deferred");
            ledger.check(
                role,
                step,
                only_ok_or_deferred && batch["outcome"] == "complete",
                compact(batch),
            );
            ledger.check(
                role,
                "read_batch deferral is not an error",
                reply["result"]["isError"] == false,
                format!("{deferred} deferred of {}", items.len()),
            );
            batch_deferred.extend(
                items
                    .iter()
                    .filter_map(|i| i["large_output_id"].as_str().map(str::to_owned)),
            );
            continue;
        }
        if *want_ok {
            match classify(reply) {
                Ok(result) => {
                    ledger.check(role, step, true, "");
                    let sc = &result["structuredContent"];
                    if step.starts_with("project_git") && large_output.is_none() {
                        large_output = sc["large_output_id"].as_str().map(str::to_owned);
                    }
                    if step == "create_draft role template" {
                        draft = sc["draft_id"].as_str().map(str::to_owned);
                    }
                    if step == "list_sources" {
                        source = sc["entries"]
                            .as_array()
                            .and_then(|a| a.first())
                            .and_then(|s| s["source_ref"].as_str())
                            .map(str::to_owned);
                    }
                    if step == "read_report predecessor" {
                        ledger.check(
                            role,
                            "predecessor report has a body",
                            sc.to_string().contains("fixture report"),
                            "",
                        );
                    }
                }
                Err(e) => {
                    ledger.check(role, step, false, e);
                }
            }
        } else {
            match precise_error(reply) {
                Ok(code) => {
                    if step == "access_request on read-only role" {
                        let offered = code.contains("tool_not_offered")
                            || reply["error"]["data"]["code"] == "tool_not_offered";
                        ledger.check(
                            role,
                            "access_request refused as not offered",
                            offered,
                            code.clone(),
                        );
                    }
                    ledger.check(role, step, true, code)
                }
                Err(e) => ledger.check(role, step, false, e),
            };
        }
    }
    let private_written = p.root.join(private).exists();
    ledger.check(role, "private path not written", !private_written, "");

    // 5. Reads that need an identity from step 4, and in-scope writes.
    let mut calls = Vec::new();
    let mut steps = Vec::new();
    match &large_output {
        Some(id) if served("read_large_output") => {
            calls.push(call("read_large_output", json!({"large_output_id":id})));
            steps.push("read_large_output project_git output");
        }
        _ => ledger.not_run(
            role,
            "read_large_output",
            "project_git returned no large_output_id or the tool is not served",
        ),
    }
    for id in &batch_deferred {
        calls.push(call("read_large_output", json!({"large_output_id":id})));
        steps.push("read_large_output deferred batch item");
    }
    match &draft {
        Some(id) if served("read_draft") => {
            calls.push(call("read_draft", json!({"draft_id":id})));
            steps.push("read_draft own draft");
        }
        _ => ledger.not_run(
            role,
            "read_draft",
            "no draft id was returned or the tool is not served",
        ),
    }
    match &source {
        Some(id) if served("read_source") => {
            calls.push(call("read_source", json!({"source_ref":id})));
            steps.push("read_source first source");
        }
        _ => ledger.not_run(
            role,
            "read_source",
            "list_sources returned no source_ref for this task",
        ),
    }
    let own = format!("area/{}/out.txt", w.node);
    if write_role && w.scope.iter().any(|s| own.starts_with(s.as_str())) {
        calls.push(call("project_read", json!({"path":own})));
        steps.push("project_read missing in-scope file");
        calls.push(call("project_write", json!({"path":own,"content":"fixture output\n","expected_sha256":"absent","request_key":format!("write-{}",w.node)})));
        steps.push("project_write in scope");
        calls.push(call(
            "project_mkdir",
            json!({"path":format!("area/{}/dir",w.node),"request_key":format!("mkdir-{}",w.node)}),
        ));
        steps.push("project_mkdir in scope");
    }
    // Reviewer: every changed file and every reviewed node's report.
    for (path, _) in reviewed.iter().filter(|(p, _)| !p.starts_with("r_")) {
        calls.push(call("project_read", json!({"path":path})));
        steps.push("review reads changed file");
    }
    for (report, _) in reviewed.iter().filter(|(p, _)| p.starts_with("r_")) {
        calls.push(call("read_report", json!({"report_id":report})));
        steps.push("review reads implementation report");
    }
    if !calls.is_empty() {
        let second = turn(&d, &home, &calls).await;
        for (n, step) in steps.iter().enumerate() {
            match second.replies.get(n).map(classify) {
                Some(Ok(_)) => ledger.check(role, step, true, ""),
                Some(Err(e)) => ledger.check(role, step, false, e),
                None => ledger.check(role, step, false, format!("no reply ({})", second.stop)),
            };
        }
        if write_role && steps.contains(&"project_write in scope") {
            ledger.check(
                role,
                "in-scope file written",
                p.root.join(&own).is_file(),
                &own,
            );
        }
    }

    // 6. Result through the shared report protocol. The runtime chooses the template
    // (the profile's for execution, the review structure for a review).
    let template = cortex_core::assignment_brief::template_for(&w.role, w.stage).to_owned();
    if w.stage == "execution" {
        ledger.check(
            role,
            "runtime template is the profile's report template",
            template == report_template(&w.role),
            &template,
        );
        let table: Vec<String> = cortex_core::assignment_brief::sections(&template, "execution")
            .iter()
            .map(|s| format!("## {}", s.heading))
            .collect();
        ledger.check(
            role,
            "report structure follows the template file",
            table == template_headings(&template),
            &template,
        );
    }
    let first_required = cortex_core::assignment_brief::sections(&template, w.stage)
        .iter()
        .find(|s| s.required)
        .map(|s| format!("## {}", s.heading))
        .unwrap();
    let (report, headings) = report_body(role, &w.role, w.stage, None);
    let (incomplete, _) = report_body(role, &w.role, w.stage, Some(&first_required));
    let mut args = json!({"report":report,"summary":format!("{role} fixture result."),
        "checks":[{"name":"Live model behavior","outcome":"not_run","blocker":"paid live check not requested by the owner"}],
        "request_key":format!("result-{}",w.node)});
    if let Some((v, findings)) = &verdict {
        args["verdict"] = json!(v);
        args["findings"] = findings.clone();
    }
    // A reviewer finishes its own review: a part it can still cover is refused,
    // naming exactly that part, and the same session continues; only a named dead
    // end is accepted as not covered.
    if verdict.as_ref().is_some_and(|(v, _)| *v == "INCOMPLETE") {
        let part = "live deployment behaviour";
        args["not_covered"] = json!([{"item":part,"blocker":"missing_access",
            "reason":"the deployment host is not reachable from this review"}]);
        // Submitting with the same gap twice for the same version is refused both
        // times on the same channel; no other reviewer is dispatched meanwhile.
        for attempt in 1..=2 {
            let mut open = args.clone();
            open["not_covered"] = json!([{"item":part,"reason":"not inspected yet"}]);
            open["request_key"] = json!(format!("open-{attempt}-{}", w.node));
            let refused = turn(&d, &home, &[call("result_submit", open)]).await;
            let reply = refused.replies.first().cloned().unwrap_or(Value::Null);
            let sc = &reply["result"]["structuredContent"];
            ledger.check(
                role,
                &format!("a part it can still cover is refused, naming it ({attempt})"),
                reply["result"]["isError"] == true
                    && sc["code"] == "review_coverage_unfinished"
                    && sc["missing"] == json!([part])
                    && sc["field"] == "not_covered"
                    && sc["recorded"] == false,
                compact(&reply),
            );
        }
        let s = p.store();
        let (reviews, running): (i64, i64) = s
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*), sum(status='running' AND result_json IS NULL) FROM runtime_nodes
                     WHERE task_id=? AND stage='review'",
                    [task],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap_or((-1, -1));
        ledger.check(
            role,
            "the refused review stays the one running review",
            (reviews, running) == (1, 1),
            format!("reviews {reviews}, running {running}"),
        );
    }
    let key = args["request_key"].as_str().unwrap().to_owned();
    // A report missing a required section is a correctable error naming exactly that
    // heading; the corrected report with the same request_key is then recorded.
    let mut missing_args = args.clone();
    missing_args["report"] = json!(incomplete);
    let refused = turn(&d, &home, &[call("result_submit", missing_args)]).await;
    match refused.replies.first() {
        Some(reply) => {
            let sc = &reply["result"]["structuredContent"];
            ledger.check(
                role,
                "missing section is a correctable error naming it",
                reply["result"]["isError"] == true
                    && sc["code"] == "report_sections_missing"
                    && sc["missing"] == json!([first_required])
                    && sc["field"] == "report"
                    && sc["recorded"] == false,
                compact(reply),
            );
        }
        None => {
            ledger.check(
                role,
                "missing section is a correctable error naming it",
                false,
                format!("no reply ({})", refused.stop),
            );
        }
    }
    let submitted = turn(&d, &home, &[call("result_submit", args)]).await;
    if let Some(reply) = submitted.replies.first() {
        if let Err(e) = classify(reply) {
            ledger.check(role, "result_submit accepted after correction", false, e);
        }
    }
    let receipt = h
        .result_receipt(task.into(), o.clone(), w.assignment.clone(), key)
        .await
        .ok()
        .flatten();
    let report_ref = receipt
        .as_ref()
        .and_then(|r| r["report_ref"].as_str())
        .map(str::to_owned);
    ledger.check(
        role,
        "result receipt recorded",
        receipt.as_ref().is_some_and(|r| {
            r["outcome"] == "recorded" && r["assignment_ref"] == w.assignment.as_str()
        }),
        receipt
            .as_ref()
            .map(compact)
            .unwrap_or_else(|| format!("none ({})", submitted.stop)),
    );
    if let Some(report_ref) = &report_ref {
        let s = p.store();
        let facts: Option<(String, String, String, i64, i64, String)> = s
            .read(|db| {
                Ok(db
                    .query_row(
                        "SELECT r.kind,r.filename,e.digest,e.size_bytes,
                            (SELECT count(*) FROM task_events v WHERE v.task_id=r.task_id AND v.event_type='worker_result'
                             AND v.superseded=0 AND json_extract(v.payload_json,'$.report_ref')=r.id
                             AND v.assignment_ref IN (?2,(SELECT attempt_id FROM delegation_provisioning WHERE task_id=r.task_id AND idempotency_key=?2 ORDER BY rowid DESC LIMIT 1))),
                            coalesce((SELECT d.template FROM drafts d WHERE d.published_report_id=r.id),'')
                         FROM reports r JOIN editions e ON e.report_id=r.id WHERE r.id=?1 AND r.task_id=?3
                         ORDER BY e.sequence DESC LIMIT 1",
                        (report_ref, &w.assignment, task),
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
                    )
                    .ok())
            })
            .unwrap();
        match facts {
            Some((kind, filename, digest, size, bound, draft_template)) => {
                ledger.check(role, "report kind", kind == "report", &kind);
                ledger.check(
                    role,
                    "worker_result binds report to this assignment",
                    bound == 1,
                    format!("{bound} events"),
                );
                let node_ref: Option<String> = s
                    .read(|db| {
                        Ok(db
                            .query_row(
                                "SELECT json_extract(result_json,'$.report_ref') FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                                (task, &w.assignment),
                                |r| r.get(0),
                            )
                            .ok()
                            .flatten())
                    })
                    .unwrap();
                ledger.check(
                    role,
                    "node result names the report",
                    node_ref.as_deref() == Some(report_ref.as_str()),
                    format!("{node_ref:?}"),
                );
                let file = s.key().directory().join(task).join(&filename);
                let bytes = std::fs::read(&file).unwrap_or_default();
                ledger.check(
                    role,
                    "published file exists",
                    !bytes.is_empty(),
                    filename.clone(),
                );
                ledger.check(
                    role,
                    "published file matches edition digest",
                    format!("{:x}", Sha256::digest(&bytes)) == digest && bytes.len() as i64 == size,
                    "",
                );
                let text = String::from_utf8_lossy(&bytes);
                let missing: Vec<_> = headings
                    .iter()
                    .filter(|h| !text.contains(h.as_str()))
                    .collect();
                ledger.check(
                    role,
                    "published report keeps template sections",
                    missing.is_empty(),
                    format!("{missing:?}"),
                );
                // The immutable file is the runtime's draft identity line plus the exact report.
                let exact =
                    text.starts_with("Cortex draft ID: `d_") && text.ends_with(report.as_str());
                ledger.check(
                    role,
                    "published body is the exact submitted report",
                    exact,
                    "",
                );
                // The inline report's draft is created under the runtime-chosen template.
                ledger.check(
                    role,
                    "inline report recorded under the role's template",
                    draft_template == template,
                    format!("draft template {draft_template:?}, runtime template {template:?}"),
                );
                let node_result: Option<String> = s
                    .read(|db| Ok(db.query_row("SELECT result_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                        (task, &w.assignment), |r| r.get(0)).ok()))
                    .unwrap();
                ledger.check(
                    role,
                    "complete report carries no structure marker",
                    node_result
                        .as_deref()
                        .is_some_and(|r| !r.contains("REPORT_STRUCTURE_INCOMPLETE")),
                    "",
                );
            }
            None => {
                ledger.check(role, "published report row", false, report_ref);
            }
        }
    }
    d.shutdown().await.ok();
    report_ref
}

// ---------------------------------------------------------------- journeys

async fn coordinator_plan(
    ledger: &mut Ledger,
    p: &Project,
    h: &NativeHost,
    o: &OwnerEpoch,
    task: &str,
    plan: Value,
    key: &str,
) -> bool {
    let (route, home) = p.route(EngineKind::Codex, &format!("coordinator-{key}"));
    // This layer drives sessions, not `native_runtime::drive`, so it takes the runtime's
    // own pipeline publication steps at the same points: one before the coordinator turn
    // (with its route, as `native_pipeline::publish` names it) ...
    let effort = ["effort", "thought_level", "reasoning_effort"]
        .iter()
        .find_map(|k| route.parameters.get(*k).cloned());
    let named = cortex_core::pipeline::Route {
        engine: route.launch.engine.name(),
        model: &route.model,
        effort: effort.as_deref(),
    };
    cortex_core::pipeline::sync(&p.store(), task, Some(&named)).unwrap();
    let c = h
        .dispatch(task.into(), o.clone(), SessionRole::Coordinator, route)
        .await
        .unwrap();
    let t = turn(
        &c,
        &home,
        &[call("plan_submit", json!({"plan":plan,"request_key":key}))],
    )
    .await;
    let accepted = t.replies.first().map(classify).is_some_and(|r| r.is_ok());
    let detail = t.replies.first().map(compact).unwrap_or_default();
    if !accepted
        && (detail.contains("verification_executable_unavailable")
            || detail.contains("sandbox_unavailable"))
    {
        c.shutdown().await.ok();
        ledger.not_run(
            "coordinator",
            &format!("plan {key}"),
            "runtime verification sandbox unavailable; plan-driven roles not executable here",
        );
        return false;
    }
    ledger.check(
        "coordinator",
        &format!("plan_submit {key}"),
        accepted,
        detail,
    );
    if accepted {
        // ... and the driver boundary after the accepted turn (route `None`).
        cortex_core::pipeline::sync(&p.store(), task, None).unwrap();
        // The coordinator reads its own task's pipeline and catalogue after acceptance.
        let reads = turn(
            &c,
            &home,
            &[
                call("read_report", json!({})),
                call("list_reports", json!({})),
                call("read_skill", json!({"skill":"topic-research"})),
                call("read_skill", json!({"skill":"orchestrator"})),
                call("tools_discover", json!({})),
            ],
        )
        .await;
        for (n, step) in [
            "read_report pipeline (no id)",
            "list_reports",
            "read_skill project skill",
            "read_skill own id",
            "tools_discover",
        ]
        .iter()
        .enumerate()
        {
            match reads.replies.get(n).map(classify) {
                Some(Ok(_)) => ledger.check("coordinator", step, true, ""),
                Some(Err(e)) => ledger.check("coordinator", step, false, e),
                None => ledger.check("coordinator", step, false, "no reply"),
            };
        }
        let pipeline = std::fs::read_dir(p.store().key().directory().join(task))
            .map(|d| {
                d.flatten()
                    .any(|e| e.file_name().to_string_lossy().contains("pipeline"))
            })
            .unwrap_or(false);
        ledger.check(
            "coordinator",
            "pipeline.md published for the task",
            pipeline,
            "after plan acceptance",
        );
    }
    c.shutdown().await.ok();
    accepted
}

/// The assignment brief (the advance step's assignment, which is the worker's prompt
/// in production) carries everything the runtime promises, never truncated away.
fn check_brief(ledger: &mut Ledger, brief: &Value, predecessors: &[String]) {
    let role = brief["role"].as_str().unwrap_or("?").to_owned();
    let stage = brief["stage"].as_str().unwrap_or("execution").to_owned();
    let label = if stage == "review" {
        format!("{role}@review")
    } else {
        role.clone()
    };
    let keys: BTreeSet<String> = brief
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    let mut want: BTreeSet<String> = [
        "assignment_ref",
        "node",
        "objective",
        "role",
        "scope",
        "stage",
        "acceptance",
        "constraints",
        "report",
        "user_question_language",
    ]
    .map(String::from)
    .into();
    if stage == "review" {
        want.insert("review_packet".into());
    } else if !predecessors.is_empty() {
        want.insert("predecessor_reports".into());
    }
    ledger.check(
        &label,
        "assignment brief fields",
        keys == want,
        format!("{keys:?}"),
    );
    ledger.check(
        &label,
        "brief scope",
        brief["scope"].as_array().is_some_and(|s| !s.is_empty()),
        compact(&brief["scope"]),
    );
    ledger.check(
        &label,
        "brief acceptance criteria",
        brief["acceptance"]["verification_commands"]
            .as_array()
            .is_some_and(|c| !c.is_empty()),
        compact(&brief["acceptance"]),
    );
    ledger.check(
        &label,
        "brief constraints",
        brief["constraints"]["write_denials"]["paths"].is_array()
            && brief["constraints"]["owner_messages"].is_array()
            && brief["constraints"]["access_mode"].is_string(),
        compact(&brief["constraints"]),
    );
    ledger.check(
        &label,
        "brief question language",
        matches!(brief["user_question_language"].as_str(), Some("en" | "ru")),
        compact(&brief["user_question_language"]),
    );
    let template = cortex_core::assignment_brief::template_for(&role, &stage);
    let expected: Vec<Value> = cortex_core::assignment_brief::sections(template, &stage)
        .iter()
        .map(
            |s| json!({"heading":format!("## {}",s.heading),"holds":s.holds,"required":s.required}),
        )
        .collect();
    ledger.check(
        &label,
        "brief report template chosen by the runtime",
        brief["report"]["template"] == template,
        compact(&brief["report"]["template"]),
    );
    ledger.check(
        &label,
        "brief report skeleton",
        brief["report"]["sections"] == json!(expected)
            && expected.iter().any(|s| s["required"] == true),
        compact(&brief["report"]),
    );
    if stage != "review" {
        let listed: Vec<&str> = brief["predecessor_reports"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["report_id"].as_str())
            .collect();
        for pred in predecessors {
            let entry = brief["predecessor_reports"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["report_id"] == pred.as_str());
            ledger.check(
                &label,
                "brief predecessor report id",
                listed.contains(&pred.as_str()),
                pred,
            );
            ledger.check(
                &label,
                "brief predecessor role, title, size and pages",
                entry.is_some_and(|e| {
                    e["role"].is_string()
                        && e["title"].is_string()
                        && e["size_bytes"].as_i64().is_some_and(|b| b > 0)
                        && e["pages_at_most"].as_i64().is_some_and(|p| p >= 1)
                }),
                entry.map(compact).unwrap_or_default(),
            );
        }
    }
}

fn assignment_of(step: &Value) -> Vec<(String, String, String, String, Vec<String>, String)> {
    step["assignments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|a| {
            (
                a["assignment_ref"].as_str().unwrap_or("").to_owned(),
                a["node"].as_str().unwrap_or("").to_owned(),
                a["role"].as_str().unwrap_or("").to_owned(),
                a["objective"].as_str().unwrap_or("").to_owned(),
                a["scope"]
                    .as_array()
                    .map(|s| {
                        s.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                a["stage"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}
/// Writers get their own new area; a read-only step must name an existing path.
fn scope_for(role: &str) -> Vec<String> {
    let profiles: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{REPO}/profiles.json")).unwrap())
            .unwrap();
    let name = role.replace('-', "_");
    let read_only = profiles["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == name.as_str() && p["access_class"] == "read_only");
    if read_only {
        vec!["src".into()]
    } else {
        vec![format!("area/{role}")]
    }
}
fn objective_for(role: &str) -> String {
    format!("Fixture objective for {role}: observe the project and report.")
}

/// All 20 plannable specialists behind one explorer predecessor, then the
/// mandatory reviewer over every node with APPROVE.
async fn all_planned_roles(ledger: &mut Ledger) {
    let p = Project::new("planned");
    let h = p.host().await;
    let task = "roles";
    let o = h
        .create(
            task.into(),
            "Exercise every worker profile".into(),
            "owner".into(),
        )
        .await
        .unwrap();
    let mut tasks = vec![
        json!({"id":"explorer","role":"explorer","objective":objective_for("explorer"),
        "dependencies":[],"scope":scope_for("explorer"),"model":"auto"}),
    ];
    for role in &PLANNED[1..] {
        tasks.push(json!({"id":role,"role":role,"objective":objective_for(role),"dependencies":["explorer"],
            "scope":scope_for(role),"model":"auto"}));
    }
    let plan = json!({"tasks":tasks,"verification":[["/usr/bin/cmp","/dev/null","/dev/null"]]});
    if !coordinator_plan(ledger, &p, &h, &o, task, plan, "plan").await {
        close(h).await;
        return;
    }
    let mut predecessor = None;
    let mut reports = Vec::<(String, String)>::new();
    let mut seen = BTreeSet::new();
    for _ in 0..40 {
        let step = match h.advance(task.into(), o.clone()).await {
            Ok(s) => s,
            Err(e) => {
                ledger.check("runtime", "advance", false, e.code());
                break;
            }
        };
        if step["state"] != "EXECUTING" {
            break;
        }
        for brief in step["assignments"].as_array().cloned().unwrap_or_default() {
            let preds: Vec<String> = if brief["node"] == "explorer" {
                vec![]
            } else {
                predecessor.iter().cloned().collect()
            };
            check_brief(ledger, &brief, &preds);
        }
        for (a, node, role, objective, scope, stage) in assignment_of(&step) {
            if !seen.insert(a.clone()) {
                continue;
            }
            let w = Work {
                role: role.clone(),
                stage: if stage == "review" {
                    "review"
                } else {
                    "execution"
                },
                assignment: a,
                node: node.clone(),
                objective: objective.clone(),
                scope: scope.clone(),
                predecessors: if node == "explorer" {
                    vec![]
                } else {
                    predecessor.iter().cloned().collect()
                },
            };
            ledger.check(
                &role,
                "brief objective == plan",
                objective == objective_for(&node),
                "",
            );
            ledger.check(
                &role,
                "brief scope == plan",
                scope == scope_for(&node),
                format!("{scope:?}"),
            );
            if let Some(r) =
                battery(ledger, &p, &h, &o, task, &w, EngineKind::Codex, None, &[]).await
            {
                if node == "explorer" {
                    predecessor = Some(r.clone());
                }
                reports.push((r, role));
            }
        }
    }
    let missing: Vec<_> = PLANNED
        .iter()
        .filter(|r| !reports.iter().any(|(_, x)| x == &format!("worker-{r}")))
        .collect();
    ledger.check(
        "runtime",
        "every planned role ran and reported",
        missing.is_empty(),
        format!("{missing:?}"),
    );
    let step = h.advance(task.into(), o.clone()).await.unwrap();
    ledger.check(
        "runtime",
        "all work done -> REVIEWING",
        step["state"] == "REVIEWING",
        compact(&step["state"]),
    );
    if step["state"] == "REVIEWING" {
        let mut reviewed: Vec<(String, String)> = reports.clone();
        for role in PLANNED {
            let path = format!("area/{role}/out.txt");
            if p.root.join(&path).is_file() {
                reviewed.push((path, role.to_owned()));
            }
        }
        for (a, node, role, objective, scope, _) in assignment_of(&step) {
            let w = Work {
                role,
                stage: "review",
                assignment: a,
                node,
                objective,
                scope,
                predecessors: vec![],
            };
            battery(
                ledger,
                &p,
                &h,
                &o,
                task,
                &w,
                EngineKind::Codex,
                Some(("APPROVE", json!([]))),
                &reviewed,
            )
            .await;
        }
        let after = h
            .advance(task.into(), o.clone())
            .await
            .map(|s| s["state"].clone())
            .unwrap_or(Value::Null);
        ledger.check(
            "worker-code-reviewer@review",
            "APPROVE leaves review",
            after != "REVIEWING",
            compact(&after),
        );
    }
    close(h).await;
}

/// One implementation node; the reviewer answers `verdict`. Returns the state after.
async fn single_review(ledger: &mut Ledger, verdict: &str, engine: EngineKind) {
    let p = Project::new(&verdict.to_ascii_lowercase());
    let h = p.host().await;
    let task = "review";
    let o = h
        .create(task.into(), "Review verdict fixture".into(), "owner".into())
        .await
        .unwrap();
    let plan = json!({"tasks":[{"id":"work","role":"backend-dev","objective":objective_for("work"),"dependencies":[],
        "scope":scope_for("work"),"model":"auto"}],"verification":[["/usr/bin/cmp","/dev/null","/dev/null"]]});
    if !coordinator_plan(ledger, &p, &h, &o, task, plan, &format!("plan-{verdict}")).await {
        close(h).await;
        return;
    }
    let step = h.advance(task.into(), o.clone()).await.unwrap();
    let mut reviewed = Vec::new();
    for (a, node, role, objective, scope, _) in assignment_of(&step) {
        let w = Work {
            role: role.clone(),
            stage: "execution",
            assignment: a,
            node,
            objective,
            scope,
            predecessors: vec![],
        };
        if let Some(r) = battery(ledger, &p, &h, &o, task, &w, engine, None, &[]).await {
            reviewed.push((r, role));
        }
    }
    reviewed.push(("area/work/out.txt".into(), "work".into()));
    let step = h.advance(task.into(), o.clone()).await.unwrap();
    let role = format!("worker-code-reviewer@{verdict}");
    if step["state"] != "REVIEWING" {
        ledger.check(&role, "reached review", false, compact(&step["state"]));
        close(h).await;
        return;
    }
    let findings = if matches!(verdict, "APPROVE" | "INCOMPLETE") {
        json!([])
    } else {
        json!([{"node":"work","files":["area/work/out.txt"],"fix":"Correct the fixture output.","cross_cutting":false}])
    };
    for (a, node, r, objective, scope, _) in assignment_of(&step) {
        let w = Work {
            role: r,
            stage: "review",
            assignment: a,
            node,
            objective,
            scope,
            predecessors: vec![],
        };
        battery(
            ledger,
            &p,
            &h,
            &o,
            task,
            &w,
            engine,
            Some((verdict, findings.clone())),
            &reviewed,
        )
        .await;
    }
    let after = h
        .advance(task.into(), o.clone())
        .await
        .map(|s| s.clone())
        .unwrap_or(Value::Null);
    let state = after["state"].as_str().unwrap_or("").to_owned();
    let expected_ok = match verdict {
        "REQUEST_CHANGES" => {
            state == "EXECUTING" && assignment_of(&after).iter().any(|a| a.5 == "execution")
        }
        // A BLOCK with findings goes back to the coordinator (RECOVERING, then planning).
        "BLOCK" => {
            state == "REVIEWING"
                || state == "STOPPED"
                || state == "PLANNING"
                || state == "RECOVERING"
        }
        // A named dead end (here missing access) is a continuation on the strategy
        // ladder (no-stop policy, RECOVERING), never an owner question and never a
        // second review of the same revision.
        "INCOMPLETE" => {
            let s = p.store();
            let (reviews, reruns, routed): (i64, i64, i64) = s
                .read(|db| {
                    Ok(db.query_row(
                        "SELECT (SELECT count(*) FROM runtime_nodes WHERE task_id=?1 AND stage='review'),
                            (SELECT count(*) FROM runtime_events WHERE task_id=?1 AND type='REVIEW_RERUN'),
                            (SELECT count(*) FROM runtime_events WHERE task_id=?1 AND type='REVIEW_DEAD_END_ROUTED')",
                        [task],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )?)
                })
                .unwrap_or((-1, -1, -1));
            ledger.check(
                &role,
                "no second reviewer for the same revision",
                (reviews, reruns, routed) == (1, 0, 1),
                format!("reviews {reviews}, reruns {reruns}, routed {routed}"),
            );
            state == "RECOVERING"
        }
        _ => state != "REVIEWING",
    };
    ledger.check(
        &role,
        "verdict drives the lifecycle",
        expected_ok,
        format!("state {state}"),
    );
    ledger.check(
        &role,
        "verdict is not completion",
        state != "COMPLETED" || verdict == "APPROVE",
        &state,
    );
    close(h).await;
}

/// A worker escalates; the runtime inserts the consultant ahead of the replan.
async fn consultant(ledger: &mut Ledger) {
    let p = Project::new("consultant");
    let h = p.host().await;
    let task = "consult";
    let o = h
        .create(task.into(), "Consultation fixture".into(), "owner".into())
        .await
        .unwrap();
    let plan = json!({"tasks":[{"id":"work","role":"backend-dev","objective":objective_for("work"),"dependencies":[],
        "scope":scope_for("work"),"model":"auto"}],"verification":[["/usr/bin/cmp","/dev/null","/dev/null"]]});
    if !coordinator_plan(ledger, &p, &h, &o, task, plan.clone(), "plan-consult").await {
        close(h).await;
        return;
    }
    let step = h.advance(task.into(), o.clone()).await.unwrap();
    let Some((a, ..)) = assignment_of(&step).into_iter().next() else {
        ledger.check(
            "worker-senior-consultant",
            "escalating worker assigned",
            false,
            compact(&step),
        );
        close(h).await;
        return;
    };
    let (route, home) = p.route(EngineKind::Codex, "escalating");
    let d = h
        .dispatch(
            task.into(),
            o.clone(),
            SessionRole::Worker { assignment: a },
            route,
        )
        .await
        .unwrap();
    let t = turn(
        &d,
        &home,
        &[call(
            "escalation_request",
            json!({"reason":"Fixture needs an independent opinion.",
        "question":"Is the fixture approach sound?","report_ids":[],"request_key":"escalate"}),
        )],
    )
    .await;
    ledger.check(
        "worker-senior-consultant",
        "escalation_request accepted",
        t.replies.first().map(classify).is_some_and(|r| r.is_ok()),
        t.replies.first().map(compact).unwrap_or_default(),
    );
    d.shutdown().await.ok();
    let mut state = Value::Null;
    for _ in 0..4 {
        state = h
            .advance(task.into(), o.clone())
            .await
            .map(|s| s["state"].clone())
            .unwrap_or(Value::Null);
        if state == "RECOVERING" {
            // The runtime driver (native_runtime.rs) settles recovery into planning
            // before it continues the coordinator; this layer does the same step.
            let settled = p.store().write(|db| {
                cortex_core::lifecycle::domain::settle_recovery_tx(db, task, TaskState::Planning)
            });
            ledger.check(
                "worker-senior-consultant",
                "recovery settles to planning",
                settled.is_ok(),
                format!("{:?}", settled.err().map(|e| e.code())),
            );
            continue;
        }
        if state == "PLANNING" {
            break;
        }
    }
    if state != "PLANNING" {
        ledger.check(
            "worker-senior-consultant",
            "escalation returns to planning",
            false,
            compact(&state),
        );
        close(h).await;
        return;
    }
    if !coordinator_plan(ledger, &p, &h, &o, task, plan, "replan-consult").await {
        close(h).await;
        return;
    }
    let step = h.advance(task.into(), o.clone()).await.unwrap();
    let consult = assignment_of(&step)
        .into_iter()
        .find(|a| a.1 == "_consultation");
    let Some((a, node, role, objective, scope, _)) = consult else {
        ledger.check(
            "worker-senior-consultant",
            "consultant node runs first",
            false,
            compact(&step["assignments"]),
        );
        close(h).await;
        return;
    };
    let w = Work {
        role,
        stage: "execution",
        assignment: a,
        node,
        objective,
        scope,
        predecessors: vec![],
    };
    battery(ledger, &p, &h, &o, task, &w, EngineKind::Codex, None, &[]).await;
    let packet = p
        .store()
        .read(|db| {
            Ok(db
                .query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='CONSULTATION_REQUIRED'", [task], |r| r.get::<_, i64>(0))
                .unwrap_or(0))
        })
        .unwrap();
    ledger.check(
        "worker-senior-consultant",
        "consultation packet recorded",
        packet == 1,
        format!("{packet}"),
    );
    close(h).await;
}

/// Structural refusals are bounded: after two, the worker's report is accepted as it
/// is and marked REPORT_STRUCTURE_INCOMPLETE for the reviewer and the coordinator.
async fn structure_bound(ledger: &mut Ledger) {
    let role = "worker-backend-dev@structure";
    let p = Project::new("structure");
    let h = p.host().await;
    let task = "structure";
    let o = h
        .create(
            task.into(),
            "Report structure fixture".into(),
            "owner".into(),
        )
        .await
        .unwrap();
    let plan = json!({"tasks":[{"id":"work","role":"backend-dev","objective":objective_for("work"),"dependencies":[],
        "scope":scope_for("work"),"model":"auto"}],"verification":[["/usr/bin/cmp","/dev/null","/dev/null"]]});
    if !coordinator_plan(ledger, &p, &h, &o, task, plan, "plan-structure").await {
        close(h).await;
        return;
    }
    let step = h.advance(task.into(), o.clone()).await.unwrap();
    let Some((a, ..)) = assignment_of(&step).into_iter().next() else {
        ledger.check(role, "worker assigned", false, compact(&step));
        close(h).await;
        return;
    };
    let (route, home) = p.route(EngineKind::Codex, "structure");
    let d = h
        .dispatch(
            task.into(),
            o.clone(),
            SessionRole::Worker {
                assignment: a.clone(),
            },
            route,
        )
        .await
        .unwrap();
    let args = json!({"report":"# Bare report\n\nNo template sections.\n","summary":"Bare.","checks":[],"request_key":"bare"});
    // A change to review: without one the runtime skips the review stage.
    turn(
        &d,
        &home,
        &[call(
            "project_write",
            json!({"path":"area/work/out.txt","content":"fixture output\n",
        "expected_sha256":"absent","request_key":"write-structure"}),
        )],
    )
    .await;
    let mut replies = Vec::new();
    for _ in 0..3 {
        let t = turn(&d, &home, &[call("result_submit", args.clone())]).await;
        replies.push(
            t.replies
                .first()
                .cloned()
                .unwrap_or_else(|| json!({"stop":t.stop})),
        );
    }
    let codes: Vec<Value> = replies
        .iter()
        .map(|r| r["result"]["structuredContent"]["code"].clone())
        .collect();
    ledger.check(
        role,
        "two structural refusals",
        codes[0] == "report_sections_missing" && codes[1] == "report_sections_missing",
        compact(&json!(replies)),
    );
    // A recorded result may revoke the channel before its reply arrives: read the
    // exact durable receipt instead of the transport reply.
    let third = h
        .result_receipt(task.into(), o.clone(), a.clone(), "bare".into())
        .await
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    ledger.check(
        role,
        "third submission recorded with the marker",
        third["outcome"] == "recorded"
            && third["report_structure"]["status"] == "REPORT_STRUCTURE_INCOMPLETE"
            && third["report_structure"]["missing"]
                == json!([
                    "## Changes",
                    "## Verification",
                    "## Risks and remaining work"
                ]),
        compact(&third),
    );
    d.shutdown().await.ok();
    let marker: Option<String> = p
        .store()
        .read(|db| Ok(db.query_row("SELECT json_extract(result_json,'$.report_structure.status') FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
            (task, &a), |r| r.get(0)).ok().flatten()))
        .unwrap();
    ledger.check(
        role,
        "node result keeps the marker",
        marker.as_deref() == Some("REPORT_STRUCTURE_INCOMPLETE"),
        format!("{marker:?}"),
    );
    let review = h.advance(task.into(), o.clone()).await.unwrap();
    let packet = review["assignments"][0]["review_packet"]["predecessor_reports"].clone();
    ledger.check(
        role,
        "reviewer sees the marker in its review packet",
        packet.as_array().is_some_and(|r| {
            r.iter()
                .any(|e| e["report_structure"] == "REPORT_STRUCTURE_INCOMPLETE")
        }),
        format!(
            "{} {}",
            compact(&review["state"]),
            compact(&review["assignments"])
        ),
    );
    close(h).await;
}

/// The runtime never dispatches build-verification (it is reserved and no code
/// inserts its node), so its session, tools and report are checked on a stored node.
async fn build_verification(ledger: &mut Ledger) {
    let p = Project::new("build");
    let h = p.host().await;
    let task = "build";
    let o = h
        .create(
            task.into(),
            "Build verification fixture".into(),
            "owner".into(),
        )
        .await
        .unwrap();
    p.store()
        .write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id=?", [task])?;
            db.execute(
                "INSERT INTO runtime_nodes VALUES (?,1,'build','worker-build-verification',?,'[]',?,'execution','running','ab',NULL)",
                (task, objective_for("build"), json!(scope_for("build")).to_string()),
            )?;
            Ok(())
        })
        .unwrap();
    ledger.not_run(
        "worker-build-verification",
        "dispatch through a plan",
        "runtime-reserved role: plan intake refuses it and no runtime path inserts its node",
    );
    let w = Work {
        role: "worker-build-verification".into(),
        stage: "execution",
        assignment: "ab".into(),
        node: "build".into(),
        objective: objective_for("build"),
        scope: scope_for("build"),
        predecessors: vec![],
    };
    battery(ledger, &p, &h, &o, task, &w, EngineKind::Claude, None, &[]).await;
    assert_ne!(h.state(task.into()).await.unwrap(), TaskState::Completed);
    close(h).await;
}

async fn close(host: NativeHost) {
    tokio::task::spawn_blocking(move || drop(host))
        .await
        .unwrap();
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == workspace_service::native_command_supervisor::MODE {
        std::process::exit(
            if workspace_service::native_command_supervisor::run().is_ok() {
                0
            } else {
                1
            },
        );
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    if mode == PEER {
        runtime.block_on(role_peer());
        return;
    }
    if mode.starts_with('/') {
        runtime.block_on(async {
            cortex_engine::broker::relay(Path::new(&mode))
                .await
                .unwrap()
        });
        return;
    }
    let started = std::time::Instant::now();
    let mut ledger = Ledger::default();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(55), async {
            all_planned_roles(&mut ledger).await;
            single_review(&mut ledger, "REQUEST_CHANGES", EngineKind::Codex).await;
            single_review(&mut ledger, "BLOCK", EngineKind::Claude).await;
            single_review(&mut ledger, "INCOMPLETE", EngineKind::Codex).await;
            consultant(&mut ledger).await;
            build_verification(&mut ledger).await;
            structure_bound(&mut ledger).await;
        })
        .await
        .expect("worker role scenarios exceeded the 55 s deadline");
    });
    for (role, step, why) in [
        (
            "all",
            "model decisions and narration",
            "no-model peer: whether a real model follows the instructions needs a paid live run",
        ),
        (
            "all",
            "ask_question / replan_request / scope_request",
            "would suspend or replan the shared task; covered by native_sessions.rs",
        ),
        (
            "all",
            "outside_command / outside_write",
            "need owner consent; covered by native_sessions.rs access_modes_and_outside_routes",
        ),
    ] {
        ledger.not_run(role, step, why);
    }
    println!(
        "worker roles: {} checks in {:?}",
        ledger.rows.len(),
        started.elapsed()
    );
    ledger.print_and_assert();
}
