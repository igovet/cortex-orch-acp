//! Scripted external ACP clients and real no-paid engine/relay child processes.
// harness = false keeps cfg(test) but strips #[test], so the reused sandbox's unit-test helpers look unused.
#[allow(dead_code, unused_imports)]
#[path = "../../workspace-service/tests/support/namespace_host.rs"]
mod namespace_host;
#[path = "../../cortex-engine/src/bin/fake-engine.rs"]
mod peer;
use cortex_engine::{
    environment::{EngineKind, LaunchSpec},
    wire,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::BufReader,
    process::{Child, ChildStdin, ChildStdout, Command},
};
use workspace_service::{
    incoming::IncomingHost,
    native_runtime::NativeRoutes,
    native_sessions::{NativeHost, SessionRoute},
};
struct Routes;
impl NativeRoutes for Routes {
    fn options(&self, root: &Path, settings: &Value) -> Value {
        let route = self.task_route(root, settings).unwrap();
        let mut options = json!([{"id":"model","name":"Model","category":"model","type":"select",
            "currentValue":route.model,"options":[{"value":route.model,"name":route.model}]}]);
        if route.parameters.contains_key("thought_level") {
            options.as_array_mut().unwrap().push(
                json!({"id":"thought_level","name":"Thought level",
                "category":"thought_level","type":"select","currentValue":"high",
                "options":[{"value":"high","name":"high"}]}),
            );
        }
        options
    }
    fn menu(&self, _: &Path, _: &cortex_core::routing::Evidence) -> cortex_core::CoreResult<Value> {
        Ok(
            json!({"tiers":{"standard":[{"route":"codex-acp/fixture-model","efforts":[],"auto_eligible":true}]},"preferred":{},"preferred_when":{},"roles":{},"flagship":{"routes":[]}}),
        )
    }
    fn mcp_registry(&self) -> cortex_core::CoreResult<workspace_service::native_mcp::Registry> {
        match std::env::var_os("FIXTURE_MCP_ROOT") {
            Some(root) => {
                let root = PathBuf::from(root);
                workspace_service::native_mcp::Registry::load(
                    &workspace_service::native_routes::RouteEnvironment {
                        home: root.clone(),
                        config: root.clone(),
                        state: root,
                        routing_override: None,
                    },
                )
            }
            None => Ok(workspace_service::native_mcp::Registry::default()),
        }
    }
    fn route(&self, root: &Path, role: &str) -> cortex_core::CoreResult<SessionRoute> {
        let exe = std::env::current_exe()?;
        let mode = match role {
            "orchestrator" => "desktop-coordinator",
            "worker-code-reviewer" => "desktop-reviewer",
            _ => "desktop-worker",
        };
        Ok(SessionRoute {
            launch: LaunchSpec {
                engine: if matches!(
                    std::fs::read_to_string(root.join("scenario.txt")).as_deref(),
                    Ok("compaction-claude"
                        | "outside-write"
                        | "incoming-permission"
                        | "command-restart-guarded")
                ) {
                    EngineKind::Claude
                } else {
                    EngineKind::Codex
                },
                executable: exe.clone(),
                args: vec![mode.into()],
                cwd: root.into(),
                native_home: root.into(),
                credential_directory: root.into(),
                base_environment: BTreeMap::new(),
                codex_launcher: None,
                confinement: None,
            },
            relay: exe,
            broker_parent: root.join(if workspace_protocol::channel::current().is_dev() {
                ".cortex-dev"
            } else {
                ".cortex"
            }),
            connection: "qualified-fake".into(),
            connection_limit: 4,
            model: "fixture-model".into(),
            parameters: if std::fs::read_to_string(root.join("scenario.txt")).unwrap_or_default()
                == "slash-config"
            {
                BTreeMap::from([("thought_level".into(), "high".into())])
            } else {
                BTreeMap::new()
            },
            instructions: "No-paid fixture".into(),
            deadline: Duration::from_secs(3),
        })
    }
}
struct Fixture(PathBuf);
impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!("ia-{}-{label}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("work.txt"), "before").unwrap();
        assert!(std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        Self(root)
    }
    fn key(&self) -> cortex_core::ProjectKey {
        cortex_core::ProjectKey::new(
            &self.0,
            if workspace_protocol::channel::current().is_dev() {
                cortex_core::StoreChannel::Dev
            } else {
                cortex_core::StoreChannel::Release
            },
        )
        .unwrap()
    }
    fn query(&self, sql: &str) -> i64 {
        cortex_core::Store::open(self.key(), cortex_core::OpenMode::LiveReadOnly)
            .unwrap()
            .read(|db| Ok(db.query_row(sql, [], |r| r.get(0))?))
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    updates: Vec<Value>,
}
impl Client {
    fn new() -> Self {
        Self::with_mcp(None)
    }
    fn with_mcp(config: Option<&Path>) -> Self {
        Self::spawn(config, false)
    }
    /// An agent launched as Paseo launches it, with Paseo's question cards.
    fn paseo_questions() -> Self {
        Self::spawn(None, true)
    }
    fn spawn(config: Option<&Path>, paseo_questions: bool) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.arg("incoming-agent");
        command.env_remove("FIXTURE_MCP_ROOT");
        command.env_remove("CORTEX_PASEO_QUESTION_UI");
        if paseo_questions {
            command.env("CORTEX_PASEO_QUESTION_UI", "1");
        }
        if let Some(path) = config {
            command.env("FIXTURE_MCP_ROOT", path);
        }
        let mut child = command
            .env_remove("PASEO_HOME")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            updates: Vec::new(),
        }
    }
    async fn send(&mut self, id: i64, method: &str, params: Value) {
        wire::write_frame(
            self.input.as_mut().unwrap(),
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )
        .await
        .unwrap();
    }
    async fn notification(&mut self, method: &str, params: Value) {
        wire::write_frame(
            self.input.as_mut().unwrap(),
            &json!({"jsonrpc":"2.0","method":method,"params":params}),
        )
        .await
        .unwrap();
    }
    async fn recv(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(20), wire::read_frame(&mut self.output))
            .await
            .expect("client timeout")
            .unwrap()
            .expect("agent closed")
    }
    async fn response(&mut self, id: i64) -> Value {
        loop {
            let f = self.recv().await;
            if f["id"] == id {
                return f;
            }
            assert!(f.get("id").is_none(), "{f}");
            self.updates.push(f);
        }
    }
    async fn open(&mut self, root: &Path) -> String {
        self.open_as(root, "scripted-client").await
    }
    async fn initialize(&mut self, client: &str) {
        self.send(
            1,
            "initialize",
            json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":client}}),
        )
        .await;
        let init = self.response(1).await;
        assert_eq!(init["result"]["protocolVersion"], 1);
    }
    async fn open_as(&mut self, root: &Path, client: &str) -> String {
        self.initialize(client).await;
        self.send(2, "session/new", json!({"cwd":root,"mcpServers":[]}))
            .await;
        let r = self.response(2).await;
        assert!(r.get("error").is_none(), "{r}");
        r["result"]["sessionId"].as_str().unwrap().into()
    }
    async fn close(mut self) {
        self.input.take();
        let exit = tokio::time::timeout(Duration::from_secs(15), self.child.wait())
            .await
            .expect("agent shutdown timeout")
            .unwrap();
        assert!(exit.success(), "{exit}");
    }
}
async fn rollover_and_reconciliation() {
    for scenario in [
        "compaction-coordinator",
        "compaction-worker",
        "compaction-claude",
        "compaction-start-only",
        "compaction-title-only",
        "reconcile-gap",
        "reconcile-reject",
        "reconcile-repair",
        "reconcile-repair-late",
        "reconcile-question",
        "reconcile-replan",
        "reconcile-worker-replan",
        "reconcile-scope",
        "reconcile-escalation",
    ] {
        let f = Fixture::new(scenario);
        std::fs::write(f.0.join("scenario.txt"), scenario).unwrap();
        let mut c = Client::new();
        let sid = c.open(&f.0).await;
        c.send(3,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":"Never lose constraint α; keep all owner decisions."}]})).await;
        if matches!(
            scenario,
            "reconcile-question" | "reconcile-repair" | "reconcile-repair-late"
        ) {
            loop {
                let frame = c.recv().await;
                if frame["method"] == "session/request_permission" {
                    assert_eq!(frame["params"]["_meta"]["cortexQuestion"], true);
                    assert!(
                        frame.to_string().contains("How should we finish?"),
                        "{scenario}: unexpected question {:?}",
                        frame["params"]["toolCall"]["title"]
                    );
                    assert_eq!(
                        f.query("SELECT count(*) FROM runtime_tasks WHERE state='WAITING_USER'"),
                        1
                    );
                    assert_eq!(
                        f.query("SELECT count(*) FROM runtime_tasks WHERE synthesis IS NOT NULL"),
                        0
                    );
                    let q = frame["params"]["toolCall"]["rawInput"]["questionId"]
                        .as_str()
                        .unwrap();
                    c.send(
                        4,
                        "session/set_config_option",
                        json!({"sessionId":sid,"configId":"cortex_question_answer",
                        "value":json!({"questionId":q,"answer":"Explain and finish"}).to_string()}),
                    )
                    .await;
                    assert!(c.response(4).await.get("error").is_none());
                    break;
                }
                assert!(frame.get("id").is_none(), "{scenario}: {frame}");
                c.updates.push(frame);
            }
        }
        let response = c.response(3).await;
        assert!(response.get("error").is_none(), "{scenario}: {response}");
        assert_eq!(
            f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
            1,
            "{scenario}"
        );
        // The incoming task has its one runtime pipeline: the initial edition first,
        // a newer edition for each later boundary, the completed state on top.
        assert_eq!(
            f.query("SELECT count(*) FROM reports WHERE kind='pipeline'"),
            1,
            "{scenario}"
        );
        assert_eq!(
            f.query("SELECT count(*) FROM editions e JOIN reports r ON r.id=e.report_id WHERE r.kind='pipeline'
                AND e.sequence=(SELECT min(sequence) FROM editions WHERE report_id=r.id) AND e.title='Initial pipeline'"),
            1,
            "{scenario}"
        );
        assert_eq!(
            f.query("SELECT count(*) FROM editions e JOIN reports r ON r.id=e.report_id WHERE r.kind='pipeline'
                AND e.sequence=(SELECT max(sequence) FROM editions WHERE report_id=r.id) AND e.title='Runtime COMPLETED'"),
            1,
            "{scenario}"
        );
        assert_eq!(
            f.query("SELECT count(*) FROM capacity_reservations WHERE state!='released'"),
            0,
            "{scenario}"
        );
        if matches!(scenario, "compaction-start-only" | "compaction-title-only") {
            assert_eq!(
                f.query(
                    "SELECT count(*) FROM context_generations WHERE cause=\'observed_compaction\'"
                ),
                0
            );
        } else if scenario.starts_with("compaction") {
            assert_eq!(
                f.query(
                    "SELECT count(*) FROM context_generations WHERE cause='observed_compaction'"
                ),
                1
            );
            assert_eq!(f.query("SELECT count(*) FROM compaction_events_seen"), 1);
            assert_eq!(
                f.query(
                    "SELECT count(*) FROM capsule_deliveries WHERE state='continuation_observed'"
                ),
                1
            );
            assert_eq!(f.query("SELECT count(*) FROM capsule_deliveries"), 4);
        } else {
            assert!(
                f.query(
                    "SELECT count(*) FROM context_generations WHERE cause='session_replacement'"
                ) >= 1
            );
            assert_eq!(f.query("SELECT count(*) FROM runtime_tasks WHERE synthesis LIKE '%paid live check not requested by the owner%'"),1);
            if scenario == "reconcile-repair" {
                assert_eq!(
                    f.query(
                        "SELECT count(*) FROM runtime_events WHERE type='VERIFICATION_REVISED'"
                    ),
                    1
                );
                assert_eq!(
                    f.query("SELECT count(*) FROM runtime_events WHERE type='VERIFICATION_FAILED'"),
                    1
                );
            }
            if scenario == "reconcile-repair" {
                // The replaced failed command is recorded as dropped, and only the
                // owner's answer to a later question released completion.
                assert_eq!(
                    f.query("SELECT count(*) FROM runtime_events WHERE type='VERIFICATION_REVISED' AND json_array_length(json_extract(payload_json,'$.dropped_failed'))=1"),
                    1
                );
            }
            if scenario == "reconcile-question" || scenario == "reconcile-repair" {
                assert_eq!(
                    f.query("SELECT count(*) FROM runtime_events WHERE type='ANSWER_SUBMITTED'"),
                    1
                );
                assert_eq!(
                    f.query("SELECT count(*) FROM runtime_events WHERE type='SYNTHESIS_SUBMITTED'"),
                    0
                );
            }
            if scenario == "reconcile-worker-replan" {
                assert_eq!(f.query("SELECT count(*) FROM runtime_nodes WHERE revision=1 AND node_id=\'second\' AND status=\'cancelled\'"),1);
                // The sibling of a batch runs concurrently, so it may already have started when the
                // replan landed; whatever started must be settled, never left live.
                assert_eq!(f.query("SELECT count(*) FROM delegation_provisioning p JOIN runtime_nodes n ON n.assignment_ref=p.idempotency_key WHERE n.revision=1 AND n.node_id=\'second\' AND p.status!=\'terminated\'"),0);
            }
            if scenario == "reconcile-escalation" {
                assert_eq!(f.query("SELECT count(*) FROM runtime_nodes WHERE node_id='_consultation' AND status='completed'"),1);
                assert_eq!(
                    f.query(
                        "SELECT count(*) FROM runtime_events WHERE type='CONSULTATION_REQUIRED'"
                    ),
                    1
                );
            }
        }
        c.close().await;
    }
}
async fn configured_mcp_ingress() {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let project = Fixture::new("mcp-ingress");
    let config = std::env::temp_dir().join(format!("mcp-incoming-config-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&config)
        .unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(config.join("cortex"))
        .unwrap();
    let path = config.join("cortex/mcp-servers.json");
    std::fs::write(
        &path,
        json!({"servers":{"configured":{"command":"/bin/true","allowed_tools":["lookup"]}}})
            .to_string(),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut c = Client::with_mcp(Some(&config));
    c.send(1, "initialize", json!({"protocolVersion":1})).await;
    c.response(1).await;
    c.send(2,"session/new",json!({"cwd":project.0,"mcpServers":[{"name":"configured","command":"/untrusted-ignored"}]})).await;
    assert!(c.response(2).await["result"]["sessionId"].is_string());
    c.send(
        3,
        "session/new",
        json!({"cwd":project.0,"mcpServers":[{"name":"unconfigured","command":"/bin/true"}]}),
    )
    .await;
    assert!(c.response(3).await["error"]["message"]
        .as_str()
        .unwrap()
        .contains("external_mcp_server_unconfigured"));
    assert_eq!(project.query("SELECT count(*) FROM acp_host_sessions"), 1);
    c.close().await;
    std::fs::remove_dir_all(config).unwrap();
}
async fn slash_configuration() {
    let f = Fixture::new("slash-config");
    std::fs::write(f.0.join("scenario.txt"), "slash-config").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    for (id, text) in [
        (3, "/cortex-language ru"),
        (4, "/cortex-model fixture-model"),
        (5, "/cortex-effort high"),
    ] {
        c.send(
            id,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]}),
        )
        .await;
        assert_eq!(c.response(id).await["result"]["stopReason"], "end_turn");
    }
    // Python's replies: a refused command is an ordinary recorded reply.
    for (id, text, reply) in [
        (6, "/cortex-language xx", "Usage: /cortex-language auto|en|ru"),
        (7, "/cortex-model invented", "No model is selectable yet. Use /cortex-setup to configure an authorized engine connection."),
        (8, "/cortex-effort low", "Could not apply cortex-effort: 'low' is not a valid value for configId 'thought_level'."),
        (9, "/cortex-language", "Usage: /cortex-language auto|en|ru"),
        (10, "/cortex-stop", "There is no active task on this session to stop."),
    ] {
        c.send(
            id,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]}),
        )
        .await;
        assert_eq!(c.response(id).await["result"]["stopReason"], "end_turn");
        let message = format!("assistant:command:{sid}:{id}");
        assert!(
            c.updates.iter().any(|u| u["params"]["update"]["messageId"] == message
                && u["params"]["update"]["content"]["text"] == reply),
            "{text}"
        );
    }
    c.send(
        11,
        "session/load",
        json!({"cwd":f.0,"sessionId":sid,"mcpServers":[]}),
    )
    .await;
    let reply = c.response(11).await;
    let options = reply["result"]["configOptions"].as_array().unwrap();
    for (name, value) in [
        ("model", "fixture-model"),
        ("thought_level", "high"),
        ("language", "ru"),
    ] {
        assert_eq!(
            options.iter().find(|o| o["id"] == name).unwrap()["currentValue"],
            value
        );
    }
    c.send(
        12,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"/cortex-status"}]}),
    )
    .await;
    assert_eq!(c.response(12).await["result"]["stopReason"], "end_turn");
    assert!(c
        .updates
        .iter()
        .any(|u| u["params"]["update"]["content"]["text"]
            .as_str()
            .is_some_and(
                |s| s.contains("- model: fixture-model") && s.contains("- language: Russian")
            )));
    assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 0);
    assert_eq!(f.query("SELECT count(*) FROM delegation_provisioning"), 0);
    c.close().await;
}
async fn outside_client_consent() {
    for approve in [true, false] {
        let f = Fixture::new(if approve {
            "outside-approved"
        } else {
            "outside-denied"
        });
        let outside = Fixture::new(if approve {
            "outside-destination-approved"
        } else {
            "outside-destination-denied"
        });
        let path = outside.0.join("output.txt");
        std::fs::write(f.0.join("scenario.txt"), "outside-write").unwrap();
        std::fs::write(f.0.join("outside-target"), path.to_str().unwrap()).unwrap();
        let mut c = Client::new();
        let sid = c.open(&f.0).await;
        c.send(
            3,
            "session/set_mode",
            json!({"sessionId":sid,"modeId":"guarded"}),
        )
        .await;
        assert!(c.response(3).await.get("error").is_none());
        c.send(
            4,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"Ask before outside write"}]}),
        )
        .await;
        loop {
            let frame = c.recv().await;
            if frame["method"] == "session/request_permission" {
                // Every ask is one owner-decision card naming role, operation, path and reason.
                assert_eq!(
                    frame["params"]["toolCall"]["title"],
                    format!(
                        "Owner decision: worker-general wants to write {} because it writes outside the project",
                        path.display()
                    )
                );
                wire::write_frame(c.input.as_mut().unwrap(),&json!({"jsonrpc":"2.0","id":frame["id"],
                    "result":{"outcome":{"outcome":"selected","optionId":if approve {"allow"} else {"deny"}}}})).await.unwrap();
                break;
            }
            assert!(frame.get("id").is_none(), "{frame}");
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            while f.query("SELECT count(*) FROM runtime_events WHERE type='NATIVE_OUTSIDE_SETTLED'")
                != 1
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        if approve {
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "client-approved");
        } else {
            assert!(!path.exists());
        }
        c.notification("session/cancel", json!({"sessionId":sid}))
            .await;
        loop {
            let frame = c.recv().await;
            if frame["id"] == 4 {
                break;
            }
        }
        c.close().await;
    }
}
async fn reconnect_resume() {
    for queued in [false, true] {
        let f = Fixture::new(if queued {
            "resume-queue"
        } else {
            "resume-prompt"
        });
        std::fs::write(f.0.join("scenario.txt"), "hold").unwrap();
        let mut c = Client::new();
        let sid = c.open(&f.0).await;
        c.send(3,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":"Initial work before disconnect"}]})).await;
        tokio::time::timeout(Duration::from_secs(10), async {
            while !f.0.join("worker-entered").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        c.notification("session/cancel", json!({"sessionId":sid}))
            .await;
        assert_eq!(c.response(3).await["result"]["stopReason"], "cancelled");
        c.close().await;
        let before = f.query("SELECT count(*) FROM runtime_events");
        let mut c = Client::new();
        c.send(
            1,
            "initialize",
            json!({"protocolVersion":1,"clientInfo":{"name":"paseo"}}),
        )
        .await;
        c.response(1).await;
        c.send(
            2,
            "session/load",
            json!({"cwd":f.0,"sessionId":sid,"mcpServers":[]}),
        )
        .await;
        assert!(c.response(2).await.get("error").is_none());
        assert_eq!(f.query("SELECT count(*) FROM runtime_events"), before);
        assert_eq!(
            f.query("SELECT count(*) FROM runtime_tasks WHERE state='PAUSED'"),
            1
        );
        std::fs::write(f.0.join("scenario.txt"), "").unwrap();
        let payload=json!({"clientMessageId":"resume-once","content":[{"type":"text","text":"Explicit owner resume"}]}).to_string();
        if queued {
            c.send(
                3,
                "session/set_config_option",
                json!({"sessionId":sid,"configId":"cortex_queued_message","value":payload}),
            )
            .await;
            assert!(c.response(3).await.get("error").is_none());
        } else {
            c.send(
                3,
                "session/prompt",
                json!({"sessionId":sid,"prompt":[{"type":"text","text":"Explicit owner resume"}]}),
            )
            .await;
            let r = c.response(3).await;
            assert_eq!(r["result"]["stopReason"], "end_turn", "{r}");
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            while f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'") != 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            f.query("SELECT count(*) FROM runtime_events WHERE type='OWNER_TASK_RESUMED'"),
            1
        );
        assert_eq!(f.query("SELECT count(*) FROM runtime_events WHERE type='USER_STEERING' AND json_extract(payload_json,'$.text')='Explicit owner resume'"),1);
        assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 1);
        if queued {
            let before = f.query("SELECT count(*) FROM runtime_events");
            c.send(
                4,
                "session/set_config_option",
                json!({"sessionId":sid,"configId":"cortex_queued_message","value":payload}),
            )
            .await;
            assert!(c.response(4).await.get("error").is_none());
            assert_eq!(f.query("SELECT count(*) FROM runtime_events"), before);
        }
        c.close().await;
    }
}
async fn access_modes_and_startup_recovery() {
    let f = Fixture::new("access-modes");
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    c.send(
        20,
        "session/load",
        json!({"cwd":f.0,"sessionId":sid,"mcpServers":[]}),
    )
    .await;
    let loaded = c.response(20).await;
    // Python's load reply carries `modes`, null outside Paseo.
    assert!(loaded["result"]["modes"].is_null());
    let access = loaded["result"]["configOptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "access_mode")
        .unwrap();
    assert_eq!(access["currentValue"], "development");
    assert_eq!(access["options"].as_array().unwrap().len(), 3);
    for (id, mode) in [(21, "auto"), (22, "guarded"), (23, "development")] {
        c.send(
            id,
            "session/set_mode",
            json!({"sessionId":sid,"modeId":mode}),
        )
        .await;
        assert!(c.response(id).await.get("error").is_none());
    }
    c.send(
        24,
        "session/set_config_option",
        json!({"sessionId":sid,"configId":"access_mode","value":"auto"}),
    )
    .await;
    assert!(c.response(24).await.get("error").is_none());
    c.close().await;
    let mut c = Client::new();
    c.send(
        1,
        "initialize",
        json!({"protocolVersion":1,"clientInfo":{"name":"paseo"}}),
    )
    .await;
    c.response(1).await;
    c.send(
        2,
        "session/load",
        json!({"cwd":f.0,"sessionId":sid,"mcpServers":[]}),
    )
    .await;
    assert_eq!(
        c.response(2).await["result"]["modes"]["currentModeId"],
        "auto"
    );
    assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 0);
    std::fs::write(f.0.join("scenario.txt"), "hold").unwrap();
    c.send(
        3,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"Retained startup goal"}]}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !f.0.join("worker-entered").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.query("SELECT count(*) FROM runtime_events WHERE type='NATIVE_ACCESS_SWITCHED' AND json_extract(payload_json,'$.mode')='auto'"),1);
    assert_eq!(
        f.query(
            "SELECT count(*) FROM runtime_events e JOIN trusted_channels c
        ON c.channel_id=json_extract(e.payload_json,'$.channel_id')
        WHERE e.type='NATIVE_ACCESS_MODE_CONFIRMED' AND c.role='worker'
        AND json_extract(e.payload_json,'$.mode')='workspace-write'"
        ),
        1
    );
    c.send(
        4,
        "session/set_mode",
        json!({"sessionId":sid,"modeId":"guarded"}),
    )
    .await;
    assert!(c.response(4).await.get("error").is_none());
    c.notification("session/cancel", json!({"sessionId":sid}))
        .await;
    assert_eq!(c.response(3).await["result"]["stopReason"], "cancelled");
    c.close().await;
    // A fully settled planning checkpoint. The fixture deliberately constructs
    // this crash boundary; no live process or unknown effect is relabelled.
    let store = cortex_core::Store::open(f.key(), cortex_core::OpenMode::RuntimeValidate).unwrap();
    store.write(|db|{
        db.execute("UPDATE runtime_tasks SET state='RECOVERING'",[])?;
        let task:String=db.query_row("SELECT task_id FROM runtime_tasks",[],|r|r.get(0))?;
        cortex_core::lifecycle::domain::native_continue_tx(db,&task)?;
        db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,created_at)
            SELECT session_id,10000,'user_message','Retained recovery instruction',? FROM acp_host_sessions
            WHERE session_id=?", (cortex_core::now(),&sid))?;
        Ok(())
    }).unwrap();
    drop(store);
    std::fs::write(f.0.join("scenario.txt"), "recovery-context").unwrap();
    let before = f.query("SELECT count(*) FROM runtime_events");
    let mut c = Client::new();
    c.send(
        1,
        "initialize",
        json!({"protocolVersion":1,"clientInfo":{"name":"paseo"}}),
    )
    .await;
    c.response(1).await;
    c.send(
        2,
        "session/load",
        json!({"cwd":f.0,"sessionId":sid,"mcpServers":[]}),
    )
    .await;
    let loaded = c.response(2).await;
    assert_eq!(loaded["result"]["modes"]["currentModeId"], "guarded");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events"),
        before,
        "load must not recover or dispatch"
    );
    assert!(c
        .updates
        .iter()
        .filter(|v| v["params"]["update"]["sessionUpdate"] == "user_message_chunk")
        .all(|v| v["params"]["update"]["_meta"]["cortexReplay"]["sessionId"] == sid));
    c.send(3, "session/new", json!({"cwd":f.0,"mcpServers":[]}))
        .await;
    let fresh = c.response(3).await;
    assert_eq!(fresh["result"]["modes"]["currentModeId"], "development");
    tokio::time::timeout(Duration::from_secs(10),async{
        while f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'")!=1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap_or_else(|_| {
        let store=cortex_core::Store::open(f.key(),cortex_core::OpenMode::LiveReadOnly).unwrap();
        let facts=store.read(|db| {
            let mut q=db.prepare("SELECT type,coalesce(json_extract(payload_json,'$.reason'),'') FROM runtime_events WHERE type LIKE 'NATIVE_RECOVERY%' OR type='NATIVE_SESSION_START_FAILED'")?;
            let rows=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
            Ok(rows)
        }).unwrap();
        panic!("recovery checkpoint facts: {facts:?}");
    });
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='NATIVE_RECOVERY_RESUMED'"),
        1
    );
    c.send(4, "session/new", json!({"cwd":f.0,"mcpServers":[]}))
        .await;
    c.response(4).await;
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='NATIVE_RECOVERY_RESUMED'"),
        1
    );
    assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 1);
    c.close().await;
}
async fn completed_session_refuses_rebinding() {
    let f = Fixture::new("completed-refusal");
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    c.send(
        3,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"First task"}]}),
    )
    .await;
    assert_eq!(c.response(3).await["result"]["stopReason"], "end_turn");
    c.send(
        4,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"Second task"}]}),
    )
    .await;
    assert_eq!(
        c.response(4).await["error"]["message"],
        "completed_task_requires_new_session"
    );
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
        1
    );
    assert_eq!(
        f.query("SELECT count(*) FROM host_transcript WHERE kind='user_message'"),
        1
    );
    c.close().await;
}
async fn journey() {
    let f = Fixture::new("complete");
    std::fs::write(f.0.join("scenario.txt"), "routing-menu").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    c.send(3, "authenticate", json!({"methodId":"unused"}))
        .await;
    assert_eq!(c.response(3).await["error"]["code"], -32601);
    c.send(
        4,
        "session/set_config_option",
        json!({"sessionId":sid,"configId":"language","value":"ru"}),
    )
    .await;
    assert!(c.response(4).await.get("error").is_none());
    c.send(
        5,
        "session/set_mode",
        json!({"sessionId":sid,"modeId":"invalid"}),
    )
    .await;
    assert_eq!(
        c.response(5).await["error"]["message"],
        "'invalid' is not a valid mode; expected one of ['development', 'guarded', 'auto']."
    );
    c.send(
        6,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"Implement fixture"}]}),
    )
    .await;
    let r = c.response(6).await;
    assert_eq!(r["result"]["stopReason"], "end_turn", "{r}");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
        1
    );
    assert!(c
        .updates
        .iter()
        .any(|f| f["params"]["update"]["sessionUpdate"] == "agent_message_chunk"));
    let live = tool_activity(&c.updates[..], &sid);
    c.send(
        6,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"replay"}]}),
    )
    .await;
    assert_eq!(
        c.response(6).await["error"]["message"],
        "duplicate_request_id"
    );
    assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 1);
    let previous = c.updates.len();
    c.send(
        7,
        "session/load",
        json!({"cwd":f.0,"sessionId":sid,"mcpServers":[]}),
    )
    .await;
    assert!(c.response(7).await.get("error").is_none());
    assert!(c.updates.len() > previous);
    // Reload replays the same tool activity, in order, marked as replay.
    assert!(c.updates[previous..]
        .iter()
        .all(
            |f| f["params"]["update"]["_meta"]["cortexReplay"]["sessionId"] == sid.as_str()
                || f["params"]["update"]["sessionUpdate"] == "available_commands_update"
        ));
    assert_eq!(tool_activity(&c.updates[previous..], &sid), live);
    // An async completion must not discard a partially consumed next frame.
    use tokio::io::AsyncWriteExt;
    c.send(
        9,
        "session/set_config_option",
        json!({"sessionId":sid,"configId":"model","value":"fixture-model"}),
    )
    .await;
    let frame = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":10,"method":"session/prompt",
        "params":{"sessionId":sid,"prompt":[{"type":"text","text":"/cortex-status"}]}}))
    .unwrap();
    let middle = frame.len() / 2;
    c.input
        .as_mut()
        .unwrap()
        .write_all(&frame[..middle])
        .await
        .unwrap();
    assert!(c.response(9).await.get("error").is_none());
    c.input
        .as_mut()
        .unwrap()
        .write_all(&frame[middle..])
        .await
        .unwrap();
    c.input.as_mut().unwrap().write_all(b"\n").await.unwrap();
    assert_eq!(c.response(10).await["result"]["stopReason"], "end_turn");
    c.send(
        8,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"A second task"}]}),
    )
    .await;
    let second = c.response(8).await;
    assert_eq!(
        second["error"]["message"], "completed_task_requires_new_session",
        "{second}"
    );
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
        1
    );
    c.close().await;
    // A new transport can replay its exact retained native ingress session.
    let mut c = Client::new();
    c.send(1, "initialize", json!({"protocolVersion":1})).await;
    c.response(1).await;
    c.send(
        2,
        "session/load",
        json!({"cwd":f.0,"sessionId":sid,"mcpServers":[]}),
    )
    .await;
    assert!(c.response(2).await.get("error").is_none());
    assert!(c
        .updates
        .iter()
        .any(|f| f["params"]["update"]["sessionUpdate"] == "user_message_chunk"));
    assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 1);
    c.close().await;
}
/// The tool activity a client saw, checked against the native display contract:
/// one `tool_call` per engine call before any `tool_call_update` of it, ids
/// namespaced by attempt, role-prefixed titles, statuses moving from
/// in_progress to completed, the call's arguments with secret-named values masked, and never
/// engine outputs or content.
fn tool_activity(frames: &[Value], sid: &str) -> Vec<Value> {
    let mut activity = Vec::new();
    let mut calls: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for frame in frames {
        assert_eq!(frame["method"], "session/update", "{frame}");
        assert_eq!(frame["params"]["sessionId"], sid);
        assert!(!frame.to_string().contains("not-public"), "{frame}");
        let mut update = frame["params"]["update"].clone();
        let kind = update["sessionUpdate"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if !matches!(kind.as_str(), "tool_call" | "tool_call_update") {
            continue;
        }
        update.as_object_mut().unwrap().remove("_meta");
        let id = update["toolCallId"].as_str().unwrap().to_owned();
        let (attempt, call) = id.rsplit_once(':').unwrap();
        assert!(!attempt.is_empty() && call == "fixture-call", "{update}");
        let mut keys: Vec<_> = update.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        let seen = calls.entry(id).or_default();
        if kind == "tool_call" {
            assert!(seen.is_empty(), "a card is created once, first: {update}");
            assert_eq!(keys, ["rawInput", "sessionUpdate", "status", "title", "toolCallId"]);
            // The owner sees each call's arguments; secret-named values stay masked.
            assert_eq!(update["rawInput"]["path"], "fixture.txt", "{update}");
            let title = update["title"].as_str().unwrap();
            assert!(
                title.ends_with(": Fixture step") && !title.starts_with(':'),
                "{title}"
            );
            assert_eq!(update["status"], "in_progress");
        } else {
            assert_eq!(seen.as_slice(), ["in_progress"], "{update}");
            assert_eq!(keys, ["sessionUpdate", "status", "toolCallId"]);
            assert_eq!(update["status"], "completed");
        }
        seen.push(update["status"].as_str().unwrap().to_owned());
        activity.push(update);
    }
    assert!(!calls.is_empty());
    assert!(
        calls.values().all(|s| s == &["in_progress", "completed"]),
        "{calls:?}"
    );
    activity
}
async fn cancel() {
    let f = Fixture::new("cancel");
    std::fs::write(f.0.join("scenario.txt"), "hold").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    c.send(
        3,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"hold worker"}]}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f.0.join("worker-entered").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    c.notification("session/cancel", json!({"sessionId":sid}))
        .await;
    let r = c.response(3).await;
    assert_eq!(r["result"]["stopReason"], "cancelled", "{r}");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='PAUSED'"),
        1
    );
    assert_eq!(
        f.query("SELECT count(*) FROM capacity_reservations WHERE state!='released'"),
        0
    );
    c.close().await;
}
/// The agent-message text of one command reply.
fn command_reply(c: &Client, sid: &str, id: i64) -> String {
    let message = format!("assistant:command:{sid}:{id}");
    c.updates
        .iter()
        .filter(|u| u["params"]["update"]["messageId"] == message)
        .filter_map(|u| u["params"]["update"]["content"]["text"].as_str())
        .collect()
}
/// Python's task commands on a session whose turn is running, then bound.
async fn task_commands(language: &str) {
    let ru = language == "ru";
    let f = Fixture::new("task-commands");
    std::fs::write(f.0.join("scenario.txt"), "hold").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    let prompt = |text: &str| json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]});
    c.send(
        20,
        "session/set_config_option",
        json!({"sessionId":sid,"configId":"language","value":language}),
    )
    .await;
    assert!(c.response(20).await.get("error").is_none());
    c.send(3, "session/prompt", prompt("hold worker")).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f.0.join("worker-entered").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // A command stop is control input even while the turn runs.
    let kill = format!("/cortex-kill gateway:{} not running", "0".repeat(32));
    c.send(4, "session/prompt", prompt(&kill)).await;
    assert_eq!(c.response(4).await["result"]["stopReason"], "end_turn");
    let reply = command_reply(&c, &sid, 4);
    let refusal = if ru { "Остановка команды отклонена: " } else { "Command stop refused: " };
    assert_eq!(reply, format!("{refusal}native_command_not_live."));
    for (id, text, message) in [
        (
            5,
            "/cortex-clear 0",
            "Cortex cleanup requires an idle session; no deletion started.",
        ),
        (
            6,
            "steer",
            "A prompt is already in flight for this session.",
        ),
    ] {
        c.send(id, "session/prompt", prompt(text)).await;
        let r = c.response(id).await;
        assert_eq!(
            (r["error"]["code"].as_i64(), r["error"]["message"].as_str()),
            (Some(-32602), Some(message))
        );
    }
    // An explicit stop interrupts the running turn, then stops the task.
    c.send(7, "session/prompt", prompt("/cortex-stop")).await;
    let mut done = std::collections::HashMap::new();
    while done.len() < 2 {
        let frame = c.recv().await;
        match frame["id"].as_i64() {
            Some(id @ (3 | 7)) => {
                done.insert(id, frame);
            }
            Some(_) => panic!("{frame}"),
            None => c.updates.push(frame),
        }
    }
    assert_eq!(
        done[&3]["result"]["stopReason"], "cancelled",
        "{}",
        done[&3]
    );
    assert_eq!(done[&7]["result"]["stopReason"], "end_turn", "{}", done[&7]);
    assert_eq!(
        command_reply(&c, &sid, 7),
        if ru {
            "Задача остановлена: все задания, открытия в очереди и ожидающие ответа вопросы отменены."
        } else {
            "Stopped this task: every assignment, queued open, and pending question was cancelled."
        }
    );
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='PAUSED'"),
        1
    );
    // The task-bound status reply renders the host's own option prose in the task language.
    c.send(13, "session/prompt", prompt("/cortex-status")).await;
    assert_eq!(c.response(13).await["result"]["stopReason"], "end_turn");
    let status = command_reply(&c, &sid, 13);
    if ru {
        assert!(status.starts_with("Настройки Cortex:"), "{status}");
        assert!(status.contains("- language: Русский Язык собственных строк Cortex"), "{status}");
        assert!(status.contains("- access_mode: "), "{status}");
        for english in ["Language for Cortex", "Workers ", " mode "] {
            assert!(!status.contains(english), "{english}: {status}");
        }
    } else {
        assert!(status.starts_with("Cortex configuration status:"), "{status}");
        assert!(status.contains("- language: English Language for Cortex's own status/question/stop lines; auto detects from your messages."), "{status}");
        assert!(status.contains("- access_mode: ") && status.contains(" mode Workers "), "{status}");
    }
    for (id, text, reply) in [
        (8, "/cortex-answer 1 yes", if ru { "Сейчас нет открытых вопросов для ответа." } else { "There is no open question to answer right now." }),
        (11, "/cortex-language xx", if ru { "Использование: /cortex-language auto|en|ru" } else { "Usage: /cortex-language auto|en|ru" }),
        (12, "/cortex-new-task", if ru { "Использование: /cortex-new-task <task text>" } else { "Usage: /cortex-new-task <task text>" }),
        (9, "/cortex-finish", if ru { "Задача закрыта по вашему запросу. Активная работа отменена. Незавершённые ревью, проверка и подготовка итогов не считаются успешно пройденными." } else { "Closed this task at your request. Active work was cancelled; unfinished review, verification, and synthesis are not certified as passed." }),
    ] {
        c.send(id, "session/prompt", prompt(text)).await;
        assert_eq!(c.response(id).await["result"]["stopReason"], "end_turn");
        assert_eq!(command_reply(&c, &sid, id), reply);
    }
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='CANCELLED'"),
        1
    );
    c.close().await;
    // A separate task: the invoking session stays task-less.
    let g = Fixture::new("new-task");
    std::fs::write(g.0.join("scenario.txt"), "routing-menu").unwrap();
    let mut c = Client::new();
    let sid = c.open(&g.0).await;
    let prompt = |text: &str| json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]});
    c.send(
        20,
        "session/set_config_option",
        json!({"sessionId":sid,"configId":"language","value":language}),
    )
    .await;
    assert!(c.response(20).await.get("error").is_none());
    c.send(
        3,
        "session/prompt",
        prompt("/cortex-new-task Implement fixture"),
    )
    .await;
    let r = c.response(3).await;
    assert_eq!(r["result"]["stopReason"], "end_turn", "{r}");
    let reply = command_reply(&c, &sid, 3);
    // The coordinator's streamed text, then the separate task's identity.
    let (prefix, suffix) = if ru {
        ("Создана отдельная задача (t_", "); активная задача не изменилась.")
    } else {
        ("Started a new, separate task (t_", "); the active task is unaffected.")
    };
    assert!(reply.contains(prefix) && reply.ends_with(suffix), "{reply}");
    assert_eq!(
        g.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
        1
    );
    assert_eq!(
        g.query(&format!(
            "SELECT count(*) FROM acp_host_sessions WHERE session_id='{sid}' AND task_id IS NULL"
        )),
        1
    );
    c.send(4, "session/prompt", prompt("/cortex-stop")).await;
    assert_eq!(c.response(4).await["result"]["stopReason"], "end_turn");
    assert_eq!(
        command_reply(&c, &sid, 4),
        "There is no active task on this session to stop."
    );
    c.close().await;
}
async fn guarded_new_task() {
    let f = Fixture::new("guarded-new-task");
    // Reuse the real worker command fixture, releasing it immediately.
    std::fs::write(f.0.join("scenario.txt"), "command-restart-guarded").unwrap();
    std::fs::write(f.0.join("command-release"), "").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    c.send(
        20,
        "session/set_mode",
        json!({"sessionId":sid,"modeId":"guarded"}),
    )
    .await;
    assert!(c.response(20).await.get("error").is_none());
    c.send(
        21,
        "session/set_config_option",
        json!({"sessionId":sid,"configId":"language","value":"ru"}),
    )
    .await;
    assert!(c.response(21).await.get("error").is_none());
    c.send(3, "session/prompt", json!({"sessionId":sid,"prompt":[{"type":"text","text":"/cortex-new-task Guarded fixture"}]})).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        while f.query("SELECT count(*) FROM runtime_requests WHERE operation='command_run' AND json_extract(result_json,'$.complete')=1") == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    // The restart fixture deliberately returns without a worker result. End
    // this journey once its real command receipt exists, rather than claiming
    // that a command-only worker completed the task.
    c.notification("session/cancel", json!({"sessionId":sid}))
        .await;
    let r = c.response(3).await;
    assert_eq!(r["result"]["stopReason"], "cancelled", "{r}");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='PAUSED'"),
        1
    );
    assert_eq!(f.query("SELECT count(*) FROM acp_host_sessions s JOIN acp_session_access_defaults a USING(session_id) WHERE s.task_id IS NOT NULL AND a.mode='guarded'"), 1);
    assert_eq!(f.query("SELECT count(*) FROM runtime_events WHERE type IN ('NATIVE_ACCESS_INITIALIZED','NATIVE_ACCESS_SWITCHED') AND json_extract(payload_json,'$.mode')!='guarded'"), 0);
    assert!(f.query("SELECT count(*) FROM runtime_events WHERE type IN ('NATIVE_ACCESS_INITIALIZED','NATIVE_ACCESS_SWITCHED') AND json_extract(payload_json,'$.mode')='guarded'") > 0);
    assert_eq!(f.query("SELECT count(*) FROM host_updates u JOIN acp_host_sessions s USING(session_id), json_each(u.update_json,'$.configOptions') o WHERE s.task_id IS NOT NULL AND json_extract(o.value,'$.id')='language' AND json_extract(o.value,'$.currentValue')='ru'"), 1);
    // This is the worker's executed command receipt, not just picker text.
    assert!(f.query("SELECT count(*) FROM runtime_requests WHERE operation='command_run' AND json_extract(result_json,'$.profile')='guarded' AND json_extract(result_json,'$.access_mode')='guarded' AND json_extract(result_json,'$.complete')=1 AND json_extract(result_json,'$.exit_code')=7") > 0);
    assert_eq!(f.query("SELECT count(*) FROM runtime_requests WHERE operation='command_run' AND json_extract(result_json,'$.profile')!='guarded'"), 0);
    assert_eq!(
        f.query(&format!(
            "SELECT count(*) FROM acp_host_sessions WHERE session_id='{sid}' AND task_id IS NULL"
        )),
        1
    );
    c.close().await;
}

async fn separate_task_cancel_preserves_parent() {
    let f = Fixture::new("separate-cancel");
    std::fs::write(f.0.join("scenario.txt"), "question").unwrap();
    let mut c = Client::new();
    let origin = c.open(&f.0).await;
    c.send(3, "session/prompt", json!({"sessionId":origin,"prompt":[{"type":"text","text":"/cortex-new-task Parent fixture"}]})).await;
    assert_eq!(c.response(3).await["result"]["stopReason"], "end_turn");
    let (sid, parent): (String, String) =
        cortex_core::Store::open(f.key(), cortex_core::OpenMode::LiveReadOnly)
            .unwrap()
            .read(|db| {
                Ok(db.query_row(
                    "SELECT session_id,task_id FROM acp_host_sessions WHERE task_id IS NOT NULL",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap();
    // Load a nonterminal task, then resume its worker independently of a prompt
    // lane. It must keep running while this session follows a separate task.
    c.send(
        4,
        "session/load",
        json!({"cwd":f.0,"sessionId":sid,"mcpServers":[]}),
    )
    .await;
    assert!(c.response(4).await.get("error").is_none());
    std::fs::write(f.0.join("scenario.txt"), "hold").unwrap();
    // A message no longer cancels the worker's open question; answering it resumes the
    // same worker, which then holds its turn.
    let question: String = cortex_core::Store::open(f.key(), cortex_core::OpenMode::LiveReadOnly)
        .unwrap()
        .read(|db| {
            Ok(db.query_row(
                "SELECT id FROM pending_questions WHERE status='open'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    c.send(
        5,
        "session/set_config_option",
        json!({"sessionId":sid,"configId":"cortex_question_answer",
        "value":json!({"questionId":question,"answer":"Detailed"}).to_string()}),
    )
    .await;
    assert!(c.response(5).await.get("error").is_none());
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f.0.join("worker-entered").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let snapshot = || {
        cortex_core::Store::open(f.key(), cortex_core::OpenMode::LiveReadOnly).unwrap()
            .read(|db| {
                let state: (String, i64) = db.query_row("SELECT r.state,e.version FROM runtime_tasks r JOIN workspace_executions e ON e.runtime_task_id=r.task_id WHERE r.task_id=?", [&parent], |r| Ok((r.get(0)?,r.get(1)?)))?;
                let nodes: String = db.query_row("SELECT json_group_array(json_object('assignment',assignment_ref,'status',status)) FROM (SELECT * FROM runtime_nodes WHERE task_id=? ORDER BY revision,node_id)", [&parent], |r| r.get(0))?;
                let capacity: String = db.query_row("SELECT json_group_array(json_object('attempt',attempt_id,'state',state)) FROM (SELECT * FROM capacity_reservations WHERE task_id=? ORDER BY reservation_id)", [&parent], |r| r.get(0))?;
                let provisioning: String = db.query_row("SELECT json_group_array(json_object('attempt',attempt_id,'status',status,'session',engine_session_ref)) FROM (SELECT * FROM delegation_provisioning WHERE task_id=? ORDER BY idempotency_key)", [&parent], |r| r.get(0))?;
                Ok((state, nodes, capacity, provisioning))
            }).unwrap()
    };
    let before = snapshot();
    assert_eq!(before.0 .0, "EXECUTING");
    assert!(
        f.query(&format!(
            "SELECT count(*) FROM runtime_nodes WHERE task_id='{parent}' AND status='running'"
        )) > 0
    );
    let reserved = f.query(&format!(
        "SELECT count(*) FROM capacity_reservations WHERE task_id='{parent}' AND state='reserved'"
    ));
    assert!(reserved > 0);
    // The parent polls scenario.txt; an in-place truncate would briefly release it.
    let next_scenario = f.0.join("scenario-next.txt");
    std::fs::write(&next_scenario, "separate-task-hold").unwrap();
    std::fs::rename(&next_scenario, f.0.join("scenario.txt")).unwrap();
    c.send(6, "session/prompt", json!({"sessionId":sid,"prompt":[{"type":"text","text":"/cortex-new-task Separate fixture"}]})).await;
    // Disjoint scopes keep both real worker processes running until cancellation.
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f.0.join("separate-worker-entered").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 2);
    c.notification("session/cancel", json!({"sessionId":sid}))
        .await;
    assert_eq!(c.response(6).await["result"]["stopReason"], "cancelled");
    assert_eq!(
        snapshot(),
        before,
        "parent state, version, assignments and live capacity changed"
    );
    assert_eq!(
        f.query(&format!(
            "SELECT count(*) FROM runtime_tasks WHERE task_id!='{parent}' AND state='PAUSED'"
        )),
        1
    );
    assert_eq!(f.query(&format!("SELECT count(*) FROM capacity_reservations WHERE task_id!='{parent}' AND state!='released'")), 0);
    assert_eq!(f.query(&format!("SELECT count(*) FROM delegation_provisioning WHERE task_id!='{parent}' AND status!='terminated'")), 0);
    // Idle cancel must not become an implicit stop of the loaded parent either.
    c.notification("session/cancel", json!({"sessionId":sid}))
        .await;
    c.send(
        7,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"/cortex-status"}]}),
    )
    .await;
    assert_eq!(c.response(7).await["result"]["stopReason"], "end_turn");
    assert_eq!(snapshot(), before);
    // Explicit stop is still task control, and does terminate the parent.
    c.send(
        8,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"/cortex-stop"}]}),
    )
    .await;
    assert_eq!(c.response(8).await["result"]["stopReason"], "end_turn");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='PAUSED'"),
        2
    );
    assert_eq!(
        f.query("SELECT count(*) FROM capacity_reservations WHERE state!='released'"),
        0
    );
    c.close().await;
}

/// Select the Guarded access mode, in which an engine permission request asks the client.
async fn set_guarded(c: &mut Client, sid: &str) {
    c.send(
        30,
        "session/set_mode",
        json!({"sessionId":sid,"modeId":"guarded"}),
    )
    .await;
    let r = c.response(30).await;
    assert!(r.get("error").is_none(), "{r}");
}
async fn permission() {
    // The engine's request reaches the client with its tool call id and offered
    // options unchanged, the client session id and the owner-decision title
    // (role, operation, subject, reason); the engine receives exactly the chosen
    // offered one-shot option, and its own reply text follows the decision.
    for (label, option, reply) in [
        ("permission", "yes", "approved"),
        ("reject", "no", "denied"),
    ] {
        let f = Fixture::new(label);
        std::fs::write(f.0.join("scenario.txt"), "incoming-permission").unwrap();
        let mut c = Client::new();
        let sid = c.open(&f.0).await;
        // Auto (the default) allows the request itself; only an asking mode shows the card.
        set_guarded(&mut c, &sid).await;
        c.send(
            3,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"ask permission"}]}),
        )
        .await;
        loop {
            let request = c.recv().await;
            if request["method"] == "session/request_permission" {
                assert!(request["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("cortex-permission-")));
                assert_eq!(
                    request["params"],
                    json!({"sessionId":sid,
                    "toolCall":{"toolCallId":"fixture-permission","title":"Owner decision: worker-general wants to run Fixture permission because it is not provably contained in the project"},
                    "options":[{"optionId":"yes","name":"Allow once","kind":"allow_once"},
                        {"optionId":"no","name":"Reject once","kind":"reject_once"}]})
                );
                wire::write_frame(
                    c.input.as_mut().unwrap(),
                    &json!({"jsonrpc":"2.0","id":request["id"],
                    "result":{"outcome":{"outcome":"selected","optionId":option}}}),
                )
                .await
                .unwrap();
                break;
            }
            assert!(request.get("id").is_none(), "{request}");
            c.updates.push(request);
        }
        let decided = format!("SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_DECIDED' AND json_extract(payload_json,'$.outcome.optionId')='{option}'");
        tokio::time::timeout(Duration::from_secs(10), async {
            while f.query(&decided) != 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        // The worker's own reply to the decision is streamed to this session.
        let mut seen = Vec::new();
        let streamed = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let frame = c.recv().await;
                if frame["params"]["update"]["content"]["text"] == reply {
                    assert_eq!(
                        frame["params"]["update"]["sessionUpdate"],
                        "agent_message_chunk"
                    );
                    break;
                }
                assert_ne!(frame["id"], 3, "{frame}");
                seen.push(frame);
            }
        })
        .await;
        assert!(streamed.is_ok(), "{label}: {seen:?}");
        assert_eq!(
            f.query("SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_CANCELLED'"),
            0
        );
        c.notification("session/cancel", json!({"sessionId":sid}))
            .await;
        loop {
            let frame = c.recv().await;
            if frame["id"] == 3 {
                assert_eq!(frame["result"]["stopReason"], "cancelled", "{frame}");
                break;
            }
        }
        c.close().await;
    }
}
async fn stale_permission_and_eof() {
    for cancel_first in [false, true] {
        let f = Fixture::new(if cancel_first {
            "late"
        } else {
            "invalid-option"
        });
        std::fs::write(f.0.join("scenario.txt"), "incoming-permission").unwrap();
        let mut c = Client::new();
        let sid = c.open(&f.0).await;
        set_guarded(&mut c, &sid).await;
        c.send(
            3,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"Ask exact permission"}]}),
        )
        .await;
        let request = loop {
            let f = c.recv().await;
            if f["method"] == "session/request_permission" {
                break f;
            }
            assert!(f.get("id").is_none(), "{f}");
        };
        if cancel_first {
            c.notification("session/cancel", json!({"sessionId":sid}))
                .await;
        }
        wire::write_frame(c.input.as_mut().unwrap(),&json!({"jsonrpc":"2.0","id":request["id"],
            "result":{"outcome":{"outcome":"selected","optionId":if cancel_first{"yes"}else{"not-offered"}}}})).await.unwrap();
        if !cancel_first {
            // An answer that is not the card's offered Allow is the owner's Deny: the
            // engine receives its own reject option, never an allowance.
            tokio::time::timeout(Duration::from_secs(10), async {
                while f.query(
                    "SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_DECIDED' AND json_extract(payload_json,'$.outcome.optionId')='no'",
                ) == 0
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            c.notification("session/cancel", json!({"sessionId":sid}))
                .await;
        }
        c.response(3).await;
        assert_eq!(f.query("SELECT count(*) FROM runtime_events WHERE type='NATIVE_PERMISSION_DECIDED' AND json_extract(payload_json,'$.outcome.optionId')='yes'"),0);
        c.close().await;
    }
    for terminated in [false, true] {
        let f = Fixture::new(if terminated { "sigterm" } else { "eof" });
        std::fs::write(f.0.join("scenario.txt"), "hold").unwrap();
        let mut c = Client::new();
        let sid = c.open(&f.0).await;
        c.send(
            3,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"Hold until EOF"}]}),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(15), async {
            while !f.0.join("worker-entered").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        if terminated {
            assert!(std::process::Command::new("/bin/kill")
                .args(["-TERM", &c.child.id().unwrap().to_string()])
                .status()
                .unwrap()
                .success());
            // Keep stdin open: settlement must not depend on the client closing it.
            assert!(
                tokio::time::timeout(Duration::from_secs(10), c.child.wait())
                    .await
                    .unwrap()
                    .unwrap()
                    .success()
            );
        }
        c.close().await;
        assert_eq!(
            f.query("SELECT count(*) FROM capacity_reservations WHERE state!='released'"),
            0
        );
        let key = f.key();
        tokio::task::spawn_blocking(move || drop(NativeHost::open(key).unwrap()))
            .await
            .unwrap();
    }
}
async fn skill_proposals() {
    use cortex_core::store::documents::{Publication, Publisher};
    for (label, answer, decision) in [
        ("approve", "1", "approved"),
        ("typed", "1", "approved"),
        ("decline", "Not now", "declined"),
        ("unclear", "yes, but first discuss", "unclear"),
        ("invalid", "", "invalid"),
    ] {
        let f = Fixture::new(label);
        // Ordinary immutable published reports, not synthetic evidence identities.
        let evidence = {
            let store =
                cortex_core::Store::open(f.key(), cortex_core::OpenMode::Initialize).unwrap();
            store
                .write(|db| {
                    db.execute(
                        "INSERT INTO tasks(id,project_root,created_at) VALUES (\'cases\',?,?)",
                        (f.0.to_str(), cortex_core::now()),
                    )?;
                    Ok(())
                })
                .unwrap();
            let publisher = Publisher::new(&store);
            let mut evidence = Vec::new();
            for key in ["one", "two"] {
                let draft = publisher
                    .create_draft("cases", "fixture", "general", key)
                    .unwrap();
                publisher.write_draft("cases", "fixture", &draft.draft_id,
                    &format!("Cortex draft ID: `{}`\n\n# Repeated case\n\nObserved fixture procedure.", draft.draft_id)).unwrap();
                let published = publisher
                    .publish(
                        "cases",
                        "fixture",
                        &draft.draft_id,
                        key,
                        Publication {
                            title: "Repeated case",
                            summary: "Fixture evidence",
                            author: "fixture",
                            source_revision: 0,
                            artifacts: json!([]),
                        },
                    )
                    .unwrap();
                evidence.push(json!({"report_id":published.report_id,"case":key}));
            }
            evidence
        };
        let options = if decision == "invalid" {
            json!(["Not now", "Create the skill"])
        } else {
            json!(["Create the skill", "Not now"])
        };
        std::fs::write(f.0.join("proposal.json"), json!({"request_key":"proposal","question":"Create reusable skill?",
            "options":options,"skill_proposal":{"name":"fixture-skill","action":"create","purpose":"Repeat this procedure","evidence":evidence}}).to_string()).unwrap();
        std::fs::write(f.0.join("expected-decision.txt"), decision).unwrap();
        let typed = label == "typed";
        let mut c = if typed {
            Client::paseo_questions()
        } else {
            Client::new()
        };
        let sid = c.open(&f.0).await;
        c.send(
            3,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"Consider repeated work"}]}),
        )
        .await;
        if decision == "invalid" {
            // The fake peer reports refusal; the invalid proposal must never persist.
            // No-stop policy: the runtime asks no stop question; it continues the task
            // with a changed approach, and the owner's cancel ends the prompt.
            tokio::time::timeout(Duration::from_secs(20), async {
                while f.query(
                    "SELECT count(*) FROM runtime_events WHERE type='AUTOMATIC_CONTINUATION'",
                ) == 0
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("the refused proposal did not continue the task");
            c.notification("session/cancel", json!({"sessionId":sid}))
                .await;
            loop {
                let frame = c.recv().await;
                assert_ne!(
                    frame["method"], "session/request_permission",
                    "the runtime asked a stop question: {frame}"
                );
                if frame["id"] == 3 {
                    break;
                }
            }
            assert_eq!(f.query("SELECT count(*) FROM runtime_events WHERE type='USER_QUESTION_REQUESTED' AND json_extract(payload_json,'$.skill_proposal') IS NOT NULL"),0);
        } else {
            loop {
                let request = c.recv().await;
                if request["method"] == "session/request_permission" {
                    assert_eq!(request["params"]["_meta"]["cortexQuestion"], true);
                    let q = request["params"]["toolCall"]["rawInput"]["questionId"]
                        .as_str()
                        .unwrap();
                    assert!(request["params"]["toolCall"]["title"]
                        .as_str()
                        .unwrap()
                        .contains("Repeated case"));
                    assert_eq!(
                        f.query("SELECT count(*) FROM runtime_tasks WHERE state='WAITING_USER'"),
                        1
                    );
                    if typed {
                        // Python's Paseo card waiter: a message typed into the busy
                        // session answers the coordinator's open card; the numbered
                        // command form does not (no worker question is open).
                        let prompt = |text: &str| json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]});
                        c.send(4, "session/prompt", prompt("/cortex-answer 1 1"))
                            .await;
                        let refused = c.response(4).await;
                        assert_eq!(
                            refused["error"]["message"],
                            "A prompt is already in flight for this session.",
                            "{refused}"
                        );
                        c.send(5, "session/prompt", prompt(answer)).await;
                        let typed = c.response(5).await;
                        assert_eq!(typed["result"]["stopReason"], "end_turn", "{typed}");
                        break;
                    }
                    c.send(
                        4,
                        "session/set_config_option",
                        json!({"sessionId":sid,"configId":"cortex_question_answer",
                        "value":json!({"questionId":q,"answer":answer}).to_string()}),
                    )
                    .await;
                    let response = c.response(4).await;
                    assert!(response.get("error").is_none(), "{label}: {response}");
                    break;
                }
                assert!(request.get("id").is_none(), "{request}");
            }
            assert_eq!(c.response(3).await["result"]["stopReason"], "end_turn");
            assert_eq!(f.query(&format!("SELECT count(*) FROM runtime_events WHERE type='SKILL_PROPOSAL_DECIDED' AND json_extract(payload_json,'$.decision')='{decision}'")),1);
            assert_eq!(
                f.query(
                    "SELECT count(*) FROM runtime_events WHERE type='COORDINATOR_ANSWER_CLAIMED'"
                ),
                1
            );
        }
        c.close().await;
        // Coordinator text the engine emits after its own question is recorded in the
        // incoming ACP session and in the task's Workspace display session alike.
        let after = |name: &str| {
            f.query(&format!("SELECT count(*) FROM host_transcript h JOIN acp_host_sessions s USING(session_id)
            WHERE h.text LIKE '%Post-question coordinator text%' AND json_extract(s.client_info,'$.name')='{name}'"))
        };
        let asked = f.query("SELECT count(*) FROM runtime_events WHERE type='USER_QUESTION_REQUESTED' AND json_extract(payload_json,'$.coordinator')=1");
        assert_eq!(after("cortex-rust-acp"), asked.min(1), "{label}");
        assert_eq!(after("cortex-native"), asked.min(1), "{label}");
        assert!(!f.0.join(".agents").exists());
        assert_eq!(
            std::fs::read_to_string(f.0.join("work.txt")).unwrap(),
            "before"
        );
    }
}
async fn worker_entered(f: &Fixture) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f.0.join("worker-entered").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
/// Python's Paseo question card and `_CancelSteeringHold`. The card carries the
/// marker, tool call id and option ids the Paseo plugin recognises; a card
/// choice answers the question. Paseo's default send is `session/cancel` plus a
/// prompt: that cancel ends the turn without pausing the task, so a following
/// `/cortex-answer` still finds the open question and a following message
/// steers. Only a lone cancel pauses, once the grace window expires.
async fn paseo_cancel_hold() {
    let prompt =
        |sid: &str, text: &str| json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]});
    for answer_by_command in [true, false] {
        let f = Fixture::new(if answer_by_command {
            "paseo-answer-command"
        } else {
            "paseo-answer-card"
        });
        std::fs::write(f.0.join("scenario.txt"), "reconcile-question").unwrap();
        let mut c = Client::paseo_questions();
        let sid = c.open_as(&f.0, "paseo").await;
        // The fake coordinator asks this reconciliation question for this goal.
        c.send(
            3,
            "session/prompt",
            prompt(&sid, "Never lose constraint α; keep all owner decisions."),
        )
        .await;
        let card = loop {
            let frame = c.recv().await;
            if frame["method"] == "session/request_permission" {
                break frame;
            }
            assert!(frame.get("id").is_none(), "{frame}");
        };
        let call = &card["params"]["toolCall"];
        let q = call["rawInput"]["questionId"].as_str().unwrap().to_owned();
        assert_eq!(call["toolCallId"], format!("cortex-question:{q}"));
        assert_eq!(call["rawInput"]["__cortex_question"], true);
        assert_eq!(call["rawInput"]["question"], "How should we finish?");
        assert_eq!(
            card["params"]["options"],
            json!([{"optionId":"cortex-question-1","name":"Explain and finish (Recommended)","kind":"allow_once"},
                {"optionId":"cortex-question-2","name":"Plan changes","kind":"allow_once"}])
        );
        if answer_by_command {
            // No wait between the two, exactly like Paseo.
            c.notification("session/cancel", json!({"sessionId":sid}))
                .await;
            c.send(
                4,
                "session/prompt",
                prompt(&sid, "/cortex-answer 1 Explain and finish"),
            )
            .await;
            assert_eq!(c.response(3).await["result"]["stopReason"], "cancelled");
            let answered = c.response(4).await;
            assert_eq!(answered["result"]["stopReason"], "end_turn", "{answered}");
        } else {
            wire::write_frame(
                c.input.as_mut().unwrap(),
                &json!({"jsonrpc":"2.0","id":card["id"],
                "result":{"outcome":{"outcome":"selected","optionId":"cortex-question-1"}}}),
            )
            .await
            .unwrap();
            assert_eq!(c.response(3).await["result"]["stopReason"], "end_turn");
        }
        assert_eq!(
            f.query(&format!("SELECT count(*) FROM pending_questions WHERE id='{q}' AND status='answered' AND answer_text='Explain and finish'")),
            1
        );
        assert_eq!(
            f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
            1
        );
        // Past the grace window: no deferred pause either.
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(
            f.query("SELECT count(*) FROM runtime_events WHERE type='OWNER_TASK_PAUSED'"),
            0
        );
        c.close().await;
    }
    // A message sent while the task runs steers it; the cancel pauses nothing.
    let f = Fixture::new("paseo-steer");
    std::fs::write(f.0.join("scenario.txt"), "hold").unwrap();
    let mut c = Client::paseo_questions();
    let sid = c.open_as(&f.0, "paseo").await;
    c.send(3, "session/prompt", prompt(&sid, "Initial work"))
        .await;
    worker_entered(&f).await;
    c.notification("session/cancel", json!({"sessionId":sid}))
        .await;
    c.send(4, "session/prompt", prompt(&sid, "New direction"))
        .await;
    assert_eq!(c.response(3).await["result"]["stopReason"], "cancelled");
    tokio::time::timeout(Duration::from_secs(15), async {
        while f.query("SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'") != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // The message interrupts nothing: the held worker keeps running in EXECUTING.
    assert_eq!(f.query("SELECT count(*) FROM runtime_events WHERE type IN ('TURN_INTERRUPTED','LIVE_ASSIGNMENTS_CANCELLED')"), 0);
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='EXECUTING'"),
        1
    );
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_nodes WHERE status='running'"),
        1
    );
    assert_eq!(f.query("SELECT count(*) FROM runtime_events WHERE type='USER_STEERING' AND json_extract(payload_json,'$.delivery')='boundary'"), 1);
    // The worker finishes its own turn; the coordinator then receives the message once.
    std::fs::write(f.0.join("scenario.txt"), "").unwrap();
    assert_eq!(c.response(4).await["result"]["stopReason"], "end_turn");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='USER_STEERING' AND json_extract(payload_json,'$.text')='New direction'"),
        1
    );
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='STEERING_DELIVERED'"),
        1
    );
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='TURN_INTERRUPTED'"),
        0
    );
    assert_eq!(f.query("SELECT count(*) FROM runtime_nodes WHERE revision=1 AND stage='execution' AND status='completed'"), 1);
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='OWNER_TASK_PAUSED'"),
        0
    );
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
        1
    );
    c.close().await;
    // A lone cancel: the turn ends at once and the task pauses after the window.
    let f = Fixture::new("paseo-lone-cancel");
    std::fs::write(f.0.join("scenario.txt"), "hold").unwrap();
    let mut c = Client::paseo_questions();
    let sid = c.open_as(&f.0, "paseo").await;
    c.send(3, "session/prompt", prompt(&sid, "Initial work"))
        .await;
    worker_entered(&f).await;
    c.notification("session/cancel", json!({"sessionId":sid}))
        .await;
    assert_eq!(c.response(3).await["result"]["stopReason"], "cancelled");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='PAUSED'"),
        0
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        while f.query("SELECT count(*) FROM runtime_tasks WHERE state='PAUSED'") != 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='OWNER_TASK_PAUSED'"),
        1
    );
    std::fs::write(f.0.join("scenario.txt"), "").unwrap();
    c.close().await;
}
async fn question_and_queue() {
    let f = Fixture::new("question");
    std::fs::write(f.0.join("scenario.txt"), "question").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    c.send(
        3,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"Ask the owner"}]}),
    )
    .await;
    loop {
        let request = c.recv().await;
        if request["method"] == "session/request_permission" {
            assert_eq!(request["params"]["_meta"]["cortexQuestion"], true);
            let q = request["params"]["toolCall"]["rawInput"]["questionId"]
                .as_str()
                .unwrap();
            c.send(
                4,
                "session/set_config_option",
                json!({"sessionId":sid,"configId":"cortex_question_answer",
                "value":json!({"questionId":q,"answer":"Detailed"}).to_string()}),
            )
            .await;
            let answer = c.response(4).await;
            assert!(answer.get("error").is_none(), "{answer}");
            break;
        }
        assert!(request.get("id").is_none(), "{request}");
    }
    assert_eq!(c.response(3).await["result"]["stopReason"], "end_turn");
    assert_eq!(f.query(&format!("SELECT count(*) FROM runtime_events WHERE type='NATIVE_QUESTION_ANSWERED' AND json_extract(payload_json,'$.session_id')='{}'",sid)),1);
    c.close().await;
    let f = Fixture::new("queue");
    std::fs::write(f.0.join("scenario.txt"), "hold").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    c.send(
        3,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"Wait for steering"}]}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f.0.join("worker-entered").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    std::fs::write(f.0.join("scenario.txt"), "").unwrap();
    c.send(4,"session/set_config_option",json!({"sessionId":sid,"configId":"cortex_queued_message",
        "value":json!({"clientMessageId":"owner-message","content":[{"type":"text","text":"New direction"}]}).to_string()})).await;
    assert!(c.response(4).await.get("error").is_none());
    assert_eq!(c.response(3).await["result"]["stopReason"], "end_turn");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'"),
        1
    );
    c.send(5,"session/set_config_option",json!({"sessionId":sid,"configId":"cortex_queued_message",
        "value":json!({"clientMessageId":"owner-message","content":[{"type":"text","text":"New direction"}]}).to_string()})).await;
    assert!(c.response(5).await.get("error").is_none());
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'"),
        1
    );
    c.close().await;
}
/// Python `_session_prompt` on a busy session: a typed message answers the
/// running turn's one open worker question; malformed numbered answers and
/// other commands refuse; the retired card's late selection is ignored.
async fn busy_answers() {
    let f = Fixture::new("busy-answer");
    std::fs::write(f.0.join("scenario.txt"), "question").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    let prompt = |text: &str| json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]});
    c.send(3, "session/prompt", prompt("Ask the owner")).await;
    let card = loop {
        let frame = c.recv().await;
        if frame["method"] == "session/request_permission" {
            break frame;
        }
        assert!(frame.get("id").is_none(), "{frame}");
    };
    assert_eq!(card["params"]["_meta"]["cortexQuestion"], true);
    for (id, text, message) in [
        (
            4,
            "/cortex-answer 2 Brief",
            "Use /cortex-answer <question-number> <answer text>.",
        ),
        (
            5,
            "/cortex-answer one Brief",
            "Use /cortex-answer <question-number> <answer text>.",
        ),
        (
            6,
            "/cortex-status",
            "A prompt is already in flight for this session.",
        ),
    ] {
        c.send(id, "session/prompt", prompt(text)).await;
        let r = c.response(id).await;
        assert_eq!(
            (r["error"]["code"].as_i64(), r["error"]["message"].as_str()),
            (Some(-32602), Some(message)),
            "{r}"
        );
    }
    assert_eq!(
        f.query("SELECT count(*) FROM pending_questions WHERE status='answered'"),
        0
    );
    // A number names an offered option, as on Python's card.
    // An image typed with the answer is retained for the engine, not dropped.
    c.send(
        7,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[
        {"type":"text","text":" 2 "},{"type":"image","mimeType":"image/png","data":"YWJj"},
        {"type":"image","mimeType":"image/svg+xml","data":"PHN2Zy8+"}]}),
    )
    .await;
    let typed = c.response(7).await;
    assert_eq!(typed["result"]["stopReason"], "end_turn", "{typed}");
    wire::write_frame(c.input.as_mut().unwrap(),&json!({"jsonrpc":"2.0","id":card["id"],
        "result":{"outcome":{"outcome":"selected","optionId":card["params"]["options"][0]["optionId"]}}})).await.unwrap();
    let r = c.response(3).await;
    assert_eq!(r["result"]["stopReason"], "end_turn", "{r}");
    assert_eq!(f.query("SELECT count(*) FROM pending_questions WHERE status='answered' AND answer_text='Brief'"),1);
    assert_eq!(f.query("SELECT count(*) FROM source_blocks WHERE kind='image' AND availability='stored' AND mime='image/png'"),1);
    assert_eq!(f.query("SELECT count(*) FROM source_blocks WHERE availability='inaccessible' AND reason='unsupported_image_type'"),1);
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='USER_ANSWER_ATTACHMENTS'"),
        1
    );
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'"),
        0
    );
    assert_eq!(
        f.query("SELECT count(*) FROM pending_questions WHERE status='answered'"),
        1
    );
    assert_eq!(f.query(&format!("SELECT count(*) FROM runtime_events WHERE type='NATIVE_QUESTION_ANSWERED' AND json_extract(payload_json,'$.session_id')='{sid}'")),1);
    // Python records the typed answer in the session transcript.
    assert_eq!(f.query(&format!("SELECT count(*) FROM host_updates WHERE session_id='{sid}'
        AND json_extract(update_json,'$.sessionUpdate')='user_message_chunk' AND json_extract(update_json,'$.content.text')=' 2 '")),1);
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
        1
    );
    c.close().await;
}
/// Python `append_source_event`: attachments of a follow-up prompt to an
/// admitted task are retained as its next source revision, listed under the
/// prompt's transcript row and given to later engine prompts. A follow-up that
/// would exceed one prompt's retained-attachment cap refuses with no effect.
async fn followup_attachments() {
    let f = Fixture::new("followup-attachments");
    std::fs::write(f.0.join("scenario.txt"), "hold").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    c.send(3,"session/prompt",json!({"sessionId":sid,"prompt":[
        {"type":"text","text":"Hold with attachment"},{"type":"image","mimeType":"image/png","data":"YWJj"}]})).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f.0.join("worker-entered").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    c.notification("session/cancel", json!({"sessionId":sid}))
        .await;
    assert_eq!(c.response(3).await["result"]["stopReason"], "cancelled");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='PAUSED'"),
        1
    );
    let large = "a".repeat(cortex_core::input::MAX_BYTES - 2);
    c.send(4,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":"Too much"},
        {"type":"resource","resource":{"uri":"file:///large.txt","mimeType":"text/plain","text":large}}]})).await;
    let refused = c.response(4).await;
    assert_eq!(
        refused["error"]["message"], "task_attachment_total_exceeds_10mib_cap",
        "{refused}"
    );
    drop(large);
    assert_eq!(f.query("SELECT count(*) FROM source_revisions"), 1);
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'"),
        0
    );
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='PAUSED'"),
        1
    );
    std::fs::write(f.0.join("scenario.txt"), "followup-attachments").unwrap();
    c.send(5,"session/prompt",json!({"sessionId":sid,"prompt":[{"type":"text","text":"Use this too"},
        {"type":"image","mimeType":"image/png","data":"ZGVm"},
        {"type":"resource","resource":{"uri":"file:///notes.txt","mimeType":"text/plain","blob":"Z2hp"}}]})).await;
    let r = c.response(5).await;
    assert_eq!(r["result"]["stopReason"], "end_turn", "{r}");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
        1
    );
    assert_eq!(f.query("SELECT count(*) FROM source_revisions"), 2);
    assert_eq!(f.query("SELECT max(revision) FROM source_revisions"), 2);
    assert_eq!(f.query("SELECT count(*) FROM source_blocks b JOIN source_revisions r ON r.report_id=b.report_id
        WHERE r.revision=2 AND b.availability='stored' AND b.trust_label='user_direct'"),2);
    assert_eq!(f.query("SELECT count(*) FROM runtime_events WHERE type='USER_STEERING' AND json_array_length(payload_json,'$.attachments')=2"),1);
    assert_eq!(
        f.query(&format!(
            "SELECT count(*) FROM host_transcript WHERE session_id='{sid}' AND kind='user_message'
        AND text LIKE 'Use this too%' AND json_array_length(attachments_json)=2"
        )),
        1
    );
    c.close().await;
    // A reload lists the follow-up's attachments under its row, as Python does.
    let mut c = Client::new();
    c.send(1, "initialize", json!({"protocolVersion":1})).await;
    c.response(1).await;
    c.send(
        2,
        "session/load",
        json!({"cwd":f.0,"sessionId":sid,"mcpServers":[]}),
    )
    .await;
    assert!(c.response(2).await.get("error").is_none());
    let row = c
        .updates
        .iter()
        .filter_map(|u| u["params"]["update"]["content"]["text"].as_str())
        .find(|t| t.starts_with("Use this too"))
        .unwrap()
        .to_owned();
    assert_eq!(row.matches("\n[attachment: ").count(), 2, "{row}");
    assert!(row.contains("(stored) source_ref=r_"), "{row}");
    c.close().await;
}
async fn prompt_attachments() {
    let f = Fixture::new("attachments");
    std::fs::write(f.0.join("scenario.txt"), "attachments").unwrap();
    std::fs::write(f.0.join("input.txt"), "snapshot").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    c.send(
        3,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"image","mimeType":"image/png","data":"%%%"}]}),
    )
    .await;
    // Python's protocol validation rejects malformed base64 before capture.
    assert_eq!(
        c.response(3).await["error"]["message"],
        "An image content block data field must be standard base64."
    );
    assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 0);
    c.send(4,"session/prompt",json!({"sessionId":sid,"prompt":[
        {"type":"text","text":"Inspect attachments"},{"type":"image","mimeType":"image/png","data":"YWJj"},
        {"type":"resource_link","name":"input.txt","uri":format!("file://{}",f.0.join("input.txt").display())},
        {"type":"resource_link","name":"remote","uri":"https://example.invalid/unfetched"},
        {"type":"resource_link","name":"missing","uri":format!("file://{}",f.0.join("absent").display())}
    ]})).await;
    let response = c.response(4).await;
    assert_eq!(response["result"]["stopReason"], "end_turn", "{response}");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
        1
    );
    assert_eq!(f.query("SELECT count(*) FROM source_blocks WHERE availability='stored' AND trust_label='user_direct'"),2);
    assert_eq!(f.query("SELECT count(*) FROM source_blocks WHERE availability='reference_only' AND reason IS NULL"),1);
    assert_eq!(f.query("SELECT count(*) FROM source_blocks WHERE availability='inaccessible' AND reason='unreadable'"),1);
    let store = cortex_core::Store::open(f.key(), cortex_core::OpenMode::LiveReadOnly).unwrap();
    store
        .read(|db| {
            let mut q = db.prepare(
                "SELECT task_id,body_ref,size_bytes FROM source_blocks WHERE availability='stored'",
            )?;
            for r in q.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })? {
                let (task, name, size) = r?;
                let p = store
                    .key()
                    .directory()
                    .join(task)
                    .join(".source_blocks")
                    .join(name);
                use std::os::unix::fs::MetadataExt;
                assert_eq!(p.metadata()?.mode() & 0o777, 0o600);
                assert_eq!(std::fs::read(p)?.len(), size as usize);
            }
            Ok(())
        })
        .unwrap();
    drop(store);
    c.close().await;
}
async fn batch_questions() {
    let f = Fixture::new("batch");
    std::fs::write(f.0.join("scenario.txt"), "batch").unwrap();
    let mut c = Client::new();
    let sid = c.open(&f.0).await;
    c.send(
        3,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"Ask related questions"}]}),
    )
    .await;
    let request = loop {
        let frame = c.recv().await;
        if frame["method"] == "session/request_permission" {
            break frame;
        }
        assert!(frame.get("id").is_none(), "{frame}");
    };
    let input = &request["params"]["toolCall"]["rawInput"];
    let batch = input["batchId"].as_str().unwrap();
    let members = input["questions"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    let answers: Vec<_> = members
        .iter()
        .map(|m| json!({"questionId":m["questionId"],"answer":m["options"][0]}))
        .collect();
    // Partial, duplicate, cross-question, and single-question submissions cannot
    // partially answer a batch or wake the worker.
    for (i,payload) in [
        json!({"batchId":batch,"answers":[answers[0].clone()],"requestKey":"partial"}),
        json!({"batchId":batch,"answers":[answers[0].clone(),answers[0].clone()],"requestKey":"duplicate"}),
        json!({"batchId":"other","answers":answers,"requestKey":"other"}),
        json!({"questionId":members[0]["questionId"],"answer":"Detailed"})
    ].into_iter().enumerate() {
        let id=10+i as i64;
        c.send(id,"session/set_config_option",json!({"sessionId":sid,"configId":"cortex_question_answer","value":payload.to_string()})).await;
        assert!(c.response(id).await.get("error").is_some());
        assert_eq!(f.query("SELECT count(*) FROM pending_questions WHERE status='answered'"),0);
    }
    // Typed text cannot pick among several open questions (Python's
    // refusal), and one numbered member cannot answer a native batch.
    let prompt = |text: &str| json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]});
    for (id, text, message) in [
        (
            30,
            "Detailed",
            "Multiple questions are open; use /cortex-answer <question-number> <answer text>.",
        ),
        (
            31,
            "/cortex-answer 1 Detailed",
            "question_batch_requires_complete_answer",
        ),
        (
            32,
            "/cortex-answer 3 Detailed",
            "Use /cortex-answer <question-number> <answer text>.",
        ),
    ] {
        c.send(id, "session/prompt", prompt(text)).await;
        let r = c.response(id).await;
        assert_eq!(r["error"]["message"], message, "{r}");
    }
    assert_eq!(
        f.query("SELECT count(*) FROM pending_questions WHERE status='answered'"),
        0
    );
    assert_eq!(
        f.query(&format!(
            "SELECT count(*) FROM host_updates WHERE session_id='{sid}'
        AND json_extract(update_json,'$.sessionUpdate')='user_message_chunk'"
        )),
        0
    );
    // A valid one-option ACP reply does not answer the batch.
    wire::write_frame(c.input.as_mut().unwrap(),&json!({"jsonrpc":"2.0","id":request["id"],
        "result":{"outcome":{"outcome":"selected","optionId":request["params"]["options"][0]["optionId"]}}})).await.unwrap();
    c.send(20,"session/set_config_option",json!({"sessionId":sid,"configId":"cortex_question_answer",
        "value":json!({"batchId":batch,"answers":answers,"requestKey":"complete-batch"}).to_string()})).await;
    assert!(c.response(20).await.get("error").is_none());
    assert_eq!(c.response(3).await["result"]["stopReason"], "end_turn");
    assert_eq!(
        f.query("SELECT count(*) FROM pending_questions WHERE status='answered'"),
        2
    );
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_events WHERE type='WORKER_CONTINUATION_STARTED'"),
        1
    );
    c.close().await;
}
async fn owner_fence() {
    let f = Fixture::new("fence");
    let key = f.key();
    let native = tokio::task::spawn_blocking(move || NativeHost::open(key).unwrap())
        .await
        .unwrap();
    let mut c = Client::new();
    c.send(1, "initialize", json!({"protocolVersion":1})).await;
    c.response(1).await;
    c.send(2, "session/new", json!({"cwd":f.0,"mcpServers":[]}))
        .await;
    assert_eq!(
        c.response(2).await["error"]["message"],
        "scheduler_owner_busy"
    );
    c.close().await;
    tokio::task::spawn_blocking(move || drop(native))
        .await
        .unwrap();
}
/// Paseo starts one `cortex-acp` process per agent, and a finished agent's
/// process stays alive. A second agent on the same project must still work:
/// the idle first process hands the scheduler lease over, the second runs its
/// own task, and the first can start another session once the second is idle.
async fn shared_project_agents() {
    let f = Fixture::new("shared");
    std::fs::write(f.0.join("scenario.txt"), "").unwrap();
    let mut first = Client::new();
    let first_session = first.open(&f.0).await;
    first
        .send(
            3,
            "session/prompt",
            json!({"sessionId":first_session,"prompt":[{"type":"text","text":"Implement fixture"}]}),
        )
        .await;
    assert_eq!(first.response(3).await["result"]["stopReason"], "end_turn");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
        1
    );
    // The fixture worker changes work.txt; restore it so the next task also has work.
    std::fs::write(f.0.join("work.txt"), "before").unwrap();
    // The first process is still alive; a second agent opens, runs and completes.
    let mut second = Client::new();
    let second_session = second.open(&f.0).await;
    assert_eq!(f.query("SELECT count(*) FROM workspace_projects"), 1);
    second
        .send(
            3,
            "session/prompt",
            json!({"sessionId":second_session,"prompt":[{"type":"text","text":"Implement fixture"}]}),
        )
        .await;
    let reply = second.response(3).await;
    assert_eq!(reply["result"]["stopReason"], "end_turn", "{reply}");
    assert_eq!(
        f.query("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'"),
        2
    );
    // Once the second agent is idle it hands the lease back to the first.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    first
        .send(4, "session/new", json!({"cwd":f.0,"mcpServers":[]}))
        .await;
    let again = first.response(4).await;
    assert!(again.get("error").is_none(), "{again}");
    first.close().await;
    second.close().await;
}
async fn no_routes() {
    let f = Fixture::new("no-routes");
    let root = f.0.clone();
    let host = tokio::task::spawn_blocking(move || {
        IncomingHost::open(
            &root,
            Arc::new(workspace_service::native_runtime::UnqualifiedRoutes),
        )
        .unwrap()
    })
    .await
    .unwrap();
    let sid = host.new_session(json!({}), json!({})).await.unwrap();
    let opts = host.config_options(&sid).await.unwrap();
    assert_eq!(opts.as_array().unwrap().len(), 2);
    assert!(host.start(&sid, "must refuse").await.is_err());
    assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 0);
    host.shutdown().await;
    tokio::task::spawn_blocking(move || drop(host))
        .await
        .unwrap();
}
fn assert_legacy_refusal(reply: Value) {
    assert_eq!(reply["error"]["code"], -32602, "{reply}");
    assert_eq!(
        reply["error"]["data"]["reason"], "legacy_session_read_only",
        "{reply}"
    );
    assert_eq!(
        reply["error"]["message"],
        "This chat was created by an earlier version of Cortex. It is read-only history and no longer supports new work. To continue, open a new chat and create a new task there."
    );
}

async fn conflicting_project_registration_is_refused() {
    use cortex_core::{OpenMode, Store};

    for (label, change) in [
        (
            "project-actor-conflict",
            "UPDATE workspace_projects SET activated_by=
             CASE activated_by WHEN 'uid:0' THEN 'uid:1' ELSE 'uid:0' END",
        ),
        (
            "project-root-conflict",
            "UPDATE workspace_projects SET project_root='/'",
        ),
    ] {
        let f = Fixture::new(label);
        let mut seed = Client::new();
        seed.open(&f.0).await;
        seed.close().await;
        let store = Store::open(f.key(), OpenMode::Initialize).unwrap();
        store
            .write(|db| {
                assert_eq!(db.execute(change, [])?, 1);
                Ok(())
            })
            .unwrap();
        drop(store);

        let mut c = Client::new();
        c.initialize("scripted-client").await;
        c.send(2, "session/new", json!({"cwd":f.0,"mcpServers":[]}))
            .await;
        let reply = c.response(2).await;
        assert_eq!(
            reply["error"]["data"]["reason"], "project_context_conflict",
            "{reply}"
        );
        assert_eq!(reply["error"]["data"]["retryable"], false, "{reply}");
        c.close().await;
        assert_eq!(f.query("SELECT count(*) FROM workspace_projects"), 1);
        assert_eq!(f.query("SELECT count(*) FROM acp_host_sessions"), 1);
        assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 0);
    }
}

async fn legacy_project_open() {
    use cortex_core::{lifecycle::TaskMachine, OpenMode, Store};

    for method in ["session/new", "session/load"] {
        let f = Fixture::new(if method == "session/new" {
            "legacy-new"
        } else {
            "legacy-load"
        });
        let mut seed = Client::new();
        let rust_sid = seed.open(&f.0).await;
        seed.close().await;
        let store = Store::open(f.key(), OpenMode::Initialize).unwrap();
        for task in ["held", "finished"] {
            TaskMachine::new(&store)
                .create(task, "Old project fixture", "python-owner")
                .unwrap();
        }
        store.write(|db| {
            // Model a Python Workspace registration without changing its archived fixtures.
            assert_eq!(db.execute(
                "UPDATE workspace_projects SET project_id='wp_historical', activated_at='historical-registration', version=7",
                [],
            )?, 1);
            // An explicitly paused task keeps its recovery barrier under the
            // shared policy, rather than entering background continuation.
            db.execute("UPDATE runtime_tasks SET state='PAUSED' WHERE task_id='held'", [])?;
            db.execute("UPDATE runtime_tasks SET state='COMPLETED',synthesis='Old result' WHERE task_id='finished'", [])?;
            db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at)
                VALUES ('held','CHECK_EXECUTION_STARTED','runtime',?,'0')",
                [json!({"command_ref":"unsettled-old-check","revision":0}).to_string()])?;
            db.execute("INSERT INTO acp_host_sessions VALUES ('python-saved',?,'finished','{}','{}','[]','0',NULL)", [f.0.to_str()])?;
            db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,created_at)
                VALUES ('python-saved',0,'agent_message','Saved Python answer','0')", [])?;
            db.pragma_update(None, "user_version", 11)?;
            Ok(())
        }).unwrap();
        drop(store);
        let key = f.key();
        let backups = key.directory().join(".migration-backups");
        let before = std::fs::read(key.database()).unwrap();
        let mut c = Client::new();
        c.initialize("scripted-client").await;
        for (id, sid) in [(2, "python-saved"), (3, "unknown")] {
            c.send(
                id,
                "session/load",
                json!({"cwd":f.0,"sessionId":sid,"mcpServers":[]}),
            )
            .await;
            let error = c.response(id).await["error"].clone();
            assert_eq!(
                error["data"]["reason"], "legacy_storage_migration_required",
                "{error}"
            );
            assert!(error["message"]
                .as_str()
                .unwrap()
                .contains("private backup"));
        }
        assert!(!backups.exists());
        assert_eq!(std::fs::read(key.database()).unwrap(), before);

        c.send(
            4,
            method,
            json!({"cwd":f.0,"sessionId":rust_sid,"mcpServers":[],"projectId":"untrusted-client-id"}),
        )
        .await;
        let opened = c.response(4).await;
        assert!(opened.get("result").is_some(), "{opened}");
        let sid = opened["result"]["sessionId"].as_str().unwrap_or(&rust_sid);
        assert_eq!(f.query("PRAGMA user_version"), 12);
        let history = "SELECT count(*) FROM format_history WHERE from_version=11 AND to_version=12";
        assert_eq!(f.query(history), 1);
        assert_eq!(std::fs::read_dir(&backups).unwrap().count(), 1);
        assert_eq!(
            f.query(
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_TASK_ADOPTED'
            AND task_id='held' AND json_extract(payload_json,'$.replayed')=0"
            ),
            1
        );
        assert_eq!(
            f.query(
                "SELECT count(*) FROM runtime_tasks WHERE task_id='held'
            AND state='PAUSED' AND blocker='legacy_effect_outcome_unknown'"
            ),
            1
        );
        assert_eq!(
            f.query(
                "SELECT count(*) FROM runtime_events WHERE task_id='held'
            AND type='NATIVE_RECOVERY_REQUIRED' AND json_extract(payload_json,'$.replayed')=0"
            ),
            1
        );
        assert_eq!(
            f.query(
                "SELECT count(*) FROM runtime_tasks WHERE task_id='finished'
            AND state='COMPLETED' AND synthesis='Old result'"
            ),
            1
        );

        c.send(
            5,
            "session/load",
            json!({"cwd":f.0,"sessionId":"python-saved","mcpServers":[]}),
        )
        .await;
        assert!(c.response(5).await.get("result").is_some());
        assert!(c
            .updates
            .iter()
            .any(|update| update["params"]["sessionId"] == "python-saved"
                && update["params"]["update"]["content"]["text"] == "Saved Python answer"));
        let prompt = json!([{"type":"text","text":"Implement fixture"}]);
        c.send(
            6,
            "session/prompt",
            json!({"sessionId":"python-saved","prompt":prompt}),
        )
        .await;
        assert_legacy_refusal(c.response(6).await);
        c.send(
            7,
            "session/set_mode",
            json!({"sessionId":"python-saved","modeId":"auto"}),
        )
        .await;
        assert_legacy_refusal(c.response(7).await);
        assert_eq!(f.query("SELECT count(*) FROM runtime_tasks"), 2);
        assert_eq!(f.query("SELECT count(*) FROM workspace_tasks"), 0);
        assert_eq!(f.query("SELECT count(*) FROM workspace_projects"), 1);
        assert_eq!(
            f.query(
                "SELECT count(*) FROM workspace_projects WHERE project_id='wp_historical'
                 AND activated_at='historical-registration' AND version=7"
            ),
            1
        );
        // An informational task must complete through the same real fake-engine
        // relay as work tasks; it has no new command or workspace effects.
        std::fs::write(f.0.join("scenario.txt"), "incoming-legacy-open").unwrap();
        c.send(
            8,
            "session/prompt",
            json!({"sessionId":sid,"prompt":[
            {"type":"text","text":"Can this new session answer?"}]}),
        )
        .await;
        let reply = c.response(8).await;
        assert_eq!(reply["result"]["stopReason"], "end_turn", "{reply}");
        assert_eq!(
            f.query(
                "SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'
            AND synthesis='The new session can answer without changing the old project.'"
            ),
            1
        );
        assert_eq!(
            std::fs::read_to_string(f.0.join("work.txt")).unwrap(),
            "before"
        );
        assert_eq!(f.query("SELECT count(*) FROM runtime_events WHERE task_id='held' AND type='CHECK_EXECUTION_STARTED'"), 1);
        assert_eq!(f.query("SELECT count(*) FROM workspace_projects"), 1);
        assert_eq!(
            f.query(
                "SELECT count(*) FROM workspace_tasks WHERE project_id='wp_historical'"
            ),
            1
        );
        c.close().await;
    }
}

async fn incompatible_projects_are_explained_without_migration() {
    use cortex_core::{OpenMode, Store};

    for (version, label, explanation) in [
        (
            10,
            "format-ten",
            "format 10 from the retired Python Cortex runtime",
        ),
        (99, "format-unknown", "does not recognize this database"),
    ] {
        let f = Fixture::new(label);
        let key = f.key();
        let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
        store
            .write(|db| Ok(db.pragma_update(None, "user_version", version)?))
            .unwrap();
        drop(store);
        let before = std::fs::read(key.database()).unwrap();
        let mut c = Client::new();
        c.initialize("scripted-client").await;
        for (id, method) in [(2, "session/new"), (3, "session/load")] {
            c.send(
                id,
                method,
                json!({"cwd":f.0,"sessionId":"saved","mcpServers":[]}),
            )
            .await;
            let error = c.response(id).await["error"].clone();
            assert_eq!(error["code"], -32602);
            assert_eq!(
                error["data"],
                json!({"reason":"unsupported_storage","retryable":false})
            );
            assert!(
                error["message"].as_str().unwrap().contains(explanation),
                "{error}"
            );
        }
        c.close().await;
        assert_eq!(std::fs::read(key.database()).unwrap(), before);
        assert!(!key.directory().join(".migration-backups").exists());
    }
}

#[path = "support/python_era.rs"]
mod python_era;

fn main() {
    if std::env::args().nth(1).as_deref()
        == Some(workspace_service::native_command_supervisor::MODE)
    {
        std::process::exit(
            if workspace_service::native_command_supervisor::run().is_ok() {
                0
            } else {
                1
            },
        );
    }
    let arg = std::env::args().nth(1).unwrap_or_default();
    if arg.starts_with("desktop-") || arg.starts_with('/') {
        peer::main();
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    if arg == "incoming-agent" {
        rt.block_on(async {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
            cortex_acp::serve_until(
                BufReader::new(tokio::io::stdin()),
                tokio::io::stdout(),
                Arc::new(Routes),
                async move {
                    term.recv().await;
                },
            )
            .await
            .unwrap();
        });
        std::process::exit(0);
    }
    if arg.is_empty() || matches!(
        arg.as_str(),
        "python-era"
            | "python-era-controls"
            | "python-era-store-11"
            | "python-era-store-12"
            | "python-era-acp-11"
            | "python-era-acp-12"
    ) {
        rt.block_on(python_era::run(&arg));
        if !arg.is_empty() {
            return;
        }
    }
    if arg.is_empty() || arg == "legacy-open" {
        rt.block_on(async {
            incompatible_projects_are_explained_without_migration().await;
            conflicting_project_registration_is_refused().await;
            legacy_project_open().await;
        });
        println!("incoming ACP: storage diagnostics, legacy task recovery and format-11 opens passed");
        if arg == "legacy-open" {
            return;
        }
    }
    if namespace_host::assert_unavailable_view() {
        return;
    }
    if arg == "write-authority" {
        rt.block_on(async {
            outside_client_consent().await;
            access_modes_and_startup_recovery().await;
            permission().await;
            stale_permission_and_eof().await;
        });
        println!("incoming ACP: owner consent and access mode recovery passed");
        return;
    }
    if arg == "owner-chat-language" {
        rt.block_on(async {
            task_commands("en").await;
            task_commands("ru").await;
        });
        println!("incoming ACP: English and Russian runtime chat passed");
        return;
    }
    if arg == "separate-task" {
        rt.block_on(async {
            guarded_new_task().await;
            separate_task_cancel_preserves_parent().await;
            cancel().await;
            task_commands("en").await;
            task_commands("ru").await;
        });
        println!("incoming ACP: separate-task authority, cancellation ownership and explicit stop passed");
        return;
    }
    if arg == "shared-project" {
        rt.block_on(shared_project_agents());
        println!("incoming ACP: shared-project agents passed");
        return;
    }
    if arg == "steering" {
        rt.block_on(async {
            journey().await;
            permission().await;
            busy_answers().await;
            batch_questions().await;
            skill_proposals().await;
            followup_attachments().await;
            prompt_attachments().await;
            paseo_cancel_hold().await;
        });
        println!("incoming ACP: tool activity, permission mapping, busy-session answers and follow-up attachments passed");
        return;
    }
    rt.block_on(async {
        guarded_new_task().await;
        separate_task_cancel_preserves_parent().await;
        rollover_and_reconciliation().await;
        reconnect_resume().await;
        outside_client_consent().await;
        access_modes_and_startup_recovery().await;
        completed_session_refuses_rebinding().await;
        slash_configuration().await;
        configured_mcp_ingress().await;
        journey().await;
        cancel().await;
        task_commands("en").await;
            task_commands("ru").await;
        permission().await;
        stale_permission_and_eof().await;
        skill_proposals().await;
        question_and_queue().await;
        busy_answers().await;
        batch_questions().await;
        paseo_cancel_hold().await;
        prompt_attachments().await;
        followup_attachments().await;
        owner_fence().await;
        shared_project_agents().await;
        no_routes().await;
    });
    println!("incoming ACP: completion, stream, replay, partial frames, configuration, cancel, permission, questions, busy-session answers, durable steering, follow-up attachments, EOF, owner fence and no-route refusal passed");
}
