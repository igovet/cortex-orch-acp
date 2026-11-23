//! Actual service command entry with real no-paid ACP processes and disposable storage.
#[path = "support/hermetic.rs"]
mod hermetic;
// harness = false keeps cfg(test) but strips #[test], so the reused sandbox's unit-test helpers look unused.
#[allow(dead_code, unused_imports)]
#[path = "support/namespace_host.rs"]
mod namespace_host;
#[path = "../../cortex-engine/src/bin/fake-engine.rs"]
mod peer;
#[path = "support/service_regressions.rs"]
mod service_regressions;
use cortex_core::{lifecycle::TaskMachine, CoreResult, OpenMode, Store};
use cortex_engine::environment::{EngineKind, LaunchSpec as EngineLaunch};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use workspace_service::{
    native_runtime::{NativeRoutes, UnqualifiedRoutes},
    native_sessions::SessionRoute,
    Config, LaunchSpec, Service,
};

struct Routes;
impl NativeRoutes for Routes {
    fn task_route(&self, root: &Path, settings: &Value) -> CoreResult<SessionRoute> {
        let mut route = self.route(root, "orchestrator")?;
        let model = settings["model"].as_str().unwrap();
        if !["", "fixture-model", "fixture-other"].contains(&model) {
            return Err(cortex_core::CoreError::Refused(
                "native_task_route_unavailable",
            ));
        }
        if !model.is_empty() {
            route.model = model.into();
        }
        Ok(route)
    }
    fn route(&self, root: &Path, role: &str) -> CoreResult<SessionRoute> {
        let exe = std::env::current_exe()?;
        // The installed gate launches the packaged peer as `desktop-auto`, which infers the role
        // from each prompt; the batch journey exercises that same peer mode.
        let scenario = std::fs::read_to_string(root.join("scenario.txt")).unwrap_or_default();
        let mode = match role {
            "orchestrator" if scenario.starts_with("service-regression-") => {
                "service-regression-peer"
            }
            _ if scenario.trim() == "batch" => "desktop-auto",
            "orchestrator" => "desktop-coordinator",
            "worker-code-reviewer" => "desktop-reviewer",
            _ => "desktop-worker",
        };
        // `worker-route.txt` routes every worker and reviewer to another engine and model
        // than the coordinator's, as runtime routing does on a real machine.
        let other = role != "orchestrator" && root.join("worker-route.txt").exists();
        Ok(SessionRoute {
            launch: EngineLaunch {
                engine: if other
                    || (role != "orchestrator"
                        && matches!(scenario.trim(), "pending-result" | "pending-end-turn"))
                {
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
            model: if other {
                "fixture-other"
            } else {
                "fixture-model"
            }
            .into(),
            parameters: BTreeMap::new(),
            instructions: "No-paid fixture.".into(),
            deadline: Duration::from_secs(3),
        })
    }
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        // Leave room for the service namespace/socket below sandbox TMPDIR roots.
        // Unique per fixture: a lingering service from an earlier journey in this process
        // must never see (and recreate the private store of) a later journey's project.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "nd-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(p.join("project")).unwrap();
        std::fs::write(p.join("project/work.txt"), "before").unwrap();
        assert!(std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(p.join("project"))
            .status()
            .unwrap()
            .success());
        Self(p)
    }
    fn config(&self) -> Config {
        // Were the retained controller invoked, this exact executable would fail the
        // handshake. No Python interpreter or fake legacy adapter is needed.
        Config {
            runtime_dir: self.0.join("runtime"),
            state_dir: self.0.join("state"),
            allowed_root: self.0.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(3),
            shutdown_timeout: Duration::from_secs(1),
            event_capacity: 128,
        }
    }
    fn store(&self) -> Store {
        let channel = if workspace_protocol::channel::current().is_dev() {
            cortex_core::StoreChannel::Dev
        } else {
            cortex_core::StoreChannel::Release
        };
        Store::open(
            cortex_core::ProjectKey::new(self.0.join("project"), channel).unwrap(),
            OpenMode::RuntimeValidate,
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Restart journeys deliberately orphan fixture engine peers; reap every process
        // still working inside this fixture so no run leaves peers behind.
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
                    continue;
                };
                if pid as u32 == std::process::id() {
                    continue;
                }
                if std::fs::read_link(entry.path().join("cwd"))
                    .is_ok_and(|cwd| cwd.starts_with(&self.0))
                {
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
        }
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn command(project: &str, operation: &str, target: Value, request: &str, payload: Value) -> Value {
    json!({"protocol":{"major":1,"minor":8},"projectId":project,"actorId":"forged","requestId":request,
        "operation":operation,"target":target,"expectedVersion":if operation == "tasks.create" {"0"} else {"1"},"ownerEpoch":null,"payload":payload})
}
async fn read_page(service: &Service, query: Value) -> Value {
    for _ in 0..20 {
        match service.native_query(query.clone()).await {
            Ok(page) => return page,
            Err(error) if error.to_string() == "snapshot_expired" => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Err(error) => panic!("native page: {error}"),
        }
    }
    panic!("native page did not stabilize");
}
async fn journey() {
    let f = Fixture::new();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Fixture")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap();
    service.activate(id).await.unwrap();
    let q = json!({"protocol":{"major":2,"mapping":2},"requestId":"page","projectId":id,
        "operation":"tasks.page","payload":{"target":id,"cursor":null,"limit":50}});
    let initial = read_page(&service, q.clone()).await;
    let mut events = service.subscribe();
    let runner = tokio::spawn(service.clone().serve());
    let created = service
        .workspace(command(
            id,
            "tasks.create",
            json!(id),
            "create",
            json!({"description":"Implement disposable fixture","checkout":{"kind":"main"}}),
        ))
        .await
        .unwrap();
    let start = command(
        id,
        "tasks.start",
        created["target"].clone(),
        "start",
        json!({}),
    );
    let attachments = service
        .machine(
            serde_json::from_value(json!({"op":"product.read","projectId":id,
        "params":{"taskId":created["target"],"attachments":true}}))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(attachments, json!({"ok":true,"result":[]}));
    assert!(service
        .machine(
            serde_json::from_value(json!({"op":"product.read","projectId":id,
        "params":{"taskId":"missing","attachments":true}}))
            .unwrap()
        )
        .await
        .is_err());
    // An unconfirmed installer fences native admission before any Start effect.
    let maintenance_id = "a".repeat(32);
    let job = f.0.join("state/maintenance").join(&maintenance_id);
    std::fs::create_dir_all(&job).unwrap();
    for dir in [&job, job.parent().unwrap()] {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(
        job.join("status.json"),
        json!({"kind":"install","phase":"unknown","updated":0}).to_string(),
    )
    .unwrap();
    assert_eq!(
        service
            .workspace(start.clone())
            .await
            .unwrap_err()
            .to_string(),
        "maintenance_busy"
    );
    let store = f.store();
    store
        .read(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM runtime_tasks", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
    let acknowledged = service
        .machine(
            serde_json::from_value(json!({"op":"maintenance",
        "params":{"action":"acknowledge","id":maintenance_id,"confirmed":true}}))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(acknowledged["ok"], true);
    assert_eq!(acknowledged["result"]["phase"], "unknown");
    // Inject failure in the admission transaction. No runtime/binding/start/outbox leaks.
    let before: i64 = store
        .read(|db| Ok(db.query_row("SELECT count(*) FROM workspace_outbox", [], |r| r.get(0))?))
        .unwrap();
    store.write(|db| { db.execute_batch("CREATE TRIGGER fail_start BEFORE INSERT ON workspace_requests WHEN NEW.operation='tasks.start' BEGIN SELECT RAISE(ABORT,'injected'); END;")?; Ok(()) }).unwrap();
    assert!(service.workspace(start.clone()).await.is_err());
    store
        .read(|db| {
            for table in [
                "runtime_tasks",
                "workspace_executions",
                "workspace_starts",
                "task_owners",
            ] {
                assert_eq!(
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
            }
            assert_eq!(
                db.query_row("SELECT count(*) FROM workspace_outbox", [], |r| r
                    .get::<_, i64>(0))?,
                before
            );
            Ok(())
        })
        .unwrap();
    store
        .write(|db| {
            db.execute_batch("DROP TRIGGER fail_start")?;
            Ok(())
        })
        .unwrap();
    let receipt = service.workspace(start.clone()).await.unwrap();
    assert_eq!(receipt["outcome"], "recorded");
    assert_eq!(service.workspace(start.clone()).await.unwrap(), receipt);
    let mut changed = start.clone();
    changed["expectedVersion"] = json!("2");
    assert_eq!(
        service.workspace(changed).await.unwrap_err().to_string(),
        "payload_conflict"
    );
    let final_page = tokio::time::timeout(Duration::from_secs(20),async {
        loop {
            let page = read_page(&service,q.clone()).await;
            let item = &page["data"]["value"]["items"][0];
            if item["runtimeState"] == "COMPLETED" { break page; }
            let diagnostics: String = store.read(|db| Ok(db.query_row("SELECT coalesce(group_concat(type || ':' || payload_json),'') FROM runtime_events WHERE type IN ('NATIVE_DRIVER_STOPPED','NATIVE_SESSION_START_FAILED')",[],|r|r.get(0))?)).unwrap();
            assert!(diagnostics.is_empty(),"driver stopped: {diagnostics}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("native desktop did not complete");
    assert_eq!(
        final_page["data"]["value"]["items"][0]["executionId"],
        receipt["target"]
    );
    assert_eq!(
        std::fs::read_to_string(f.0.join("project/work.txt")).unwrap(),
        "after"
    );
    store.read(|db| {
        assert_eq!(db.query_row("SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'",[],|r|r.get::<_,i64>(0))?,1);
        assert_eq!(db.query_row("SELECT count(*) FROM runtime_nodes WHERE stage='review' AND status='completed'",[],|r|r.get::<_,i64>(0))?,1);
        assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning WHERE status='terminated'",[],|r|r.get::<_,i64>(0))?,3);
        assert_eq!(db.query_row("SELECT count(*) FROM runtime_requests WHERE operation='native_command_output'",[],|r|r.get::<_,i64>(0))?,1);
        Ok(())
    }).unwrap();
    let facts = service
        .machine(
            serde_json::from_value(json!({"op":"product.facts","projectId":id,
        "params":{"taskIds":[created["target"]]}}))
            .unwrap(),
        )
        .await
        .unwrap();
    let visible = &facts["result"]["product"]["tasks"][created["target"].as_str().unwrap()];
    assert!(visible["result"]["summary"]
        .as_str()
        .unwrap()
        .contains("Fixture result."));
    assert_eq!(visible["result"]["digest"], visible["workspaceDigest"]);
    // Live run: after a relaunch a project-folder task created since the private overlay was
    // last written had no checkout, so it read "Ready to merge". The store owns its options.
    assert!(
        !std::fs::read_to_string(f.0.join("state/product").join(format!("{id}.json")))
            .unwrap_or_default()
            .contains(created["target"].as_str().unwrap())
    );
    assert_eq!(visible["checkout"]["kind"], "main", "{visible}");
    assert!(visible["checkout"]["branch"].is_string(), "{visible}");
    assert_eq!(facts["result"]["product"]["hasGit"], true);
    std::fs::write(f.0.join("project/work.txt"), "owner change after result").unwrap();
    let changed = service
        .machine(
            serde_json::from_value(json!({"op":"product.facts","projectId":id,
        "params":{"taskIds":[created["target"]]}}))
            .unwrap(),
        )
        .await
        .unwrap();
    let visible = &changed["result"]["product"]["tasks"][created["target"].as_str().unwrap()];
    assert_ne!(visible["result"]["digest"], visible["workspaceDigest"]);
    std::fs::write(f.0.join("project/work.txt"), "after").unwrap();
    assert_eq!(service.workspace(start).await.unwrap(), receipt);
    let pushed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = service.receive_event(&mut events).await.unwrap();
            if event["type"] == "native_delta"
                && event["batch"]["changes"].as_array().is_some_and(|changes| {
                    changes
                        .iter()
                        .any(|c| c["operation"]["data"]["data"]["runtimeState"] == "COMPLETED")
                })
            {
                break event;
            }
        }
    })
    .await
    .expect("completion delta missing");
    assert_eq!(pushed["batch"]["generation"], initial["generation"]);
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
    drop(store);
    drop(service);
    // Same handler with an unqualified capability refuses before admission and
    // never falls back to the impossible retained launcher.
    let refused = Service::bind_native(f.config(), Arc::new(UnqualifiedRoutes))
        .await
        .unwrap();
    refused.activate(id).await.unwrap();
    let created = refused
        .workspace(command(
            id,
            "tasks.create",
            json!(id),
            "create-refused",
            json!({"description":"Do not dispatch","checkout":{"kind":"main"}}),
        ))
        .await
        .unwrap();
    assert_eq!(
        refused
            .workspace(command(
                id,
                "tasks.start",
                created["target"].clone(),
                "refused",
                json!({})
            ))
            .await
            .unwrap_err()
            .to_string(),
        "native_route_authorization_unavailable"
    );
    refused.stop().await.unwrap();
}
/// Real-shaped owner configuration for the production route provider: the
/// Python-oracle catalog identities of `catalog_route_identity.rs`, launched as
/// this no-paid peer under each engine's own native account variable.
const CODEX: &str = "cn_000000000001";
const CLAUDE: &str = "cn_000000000002";
const CODEX_ROUTE: &str = "xr_fd4ad9c738fd1736c931b007";
const CLAUDE_ROUTE: &str = "xr_b067806096f03b639ac4fcf2";
fn owner_routes(f: &Fixture) -> workspace_service::native_routes::ConfiguredRoutes {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::DirBuilderExt;
    let base = f.0.join("owner");
    for path in [
        "",
        "home",
        "config",
        "config/cortex",
        "state",
        "state/cortex",
        "state/cortex/catalog",
        "state/cortex/catalog/cn_000000000001",
        "state/cortex/catalog/cn_000000000002",
        "credentials",
    ] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(base.join(path))
            .unwrap();
    }
    let write = |path: &str, value: &Value| {
        let path = base.join(path);
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    };
    let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    let record = |id: &str, engine: &str| {
        let mut r = json!({"id":id,"name":engine,"engine":engine,"provider":"fixture","endpoint":null,
            "launch_command":[exe, "desktop-coordinator"],"auth_mode":"native_account","key_kind":null,
            "qualified":true,"state":"configured",
            "credential_ref":{"kind":"engine_native_dir","path":base.join("credentials"),"owned":false},
            "max_concurrent_sessions":4,"created_at":"2026-11-15T00:00:00Z","updated_at":"2026-11-15T00:00:00Z"});
        let approved = serde_json::to_vec(&json!({"engine":r["engine"],"provider":r["provider"],
            "endpoint":r["endpoint"],"launch_command":r["launch_command"]}))
        .unwrap();
        r["approved_fingerprint"] = json!(format!("{:x}", Sha256::digest(approved)));
        r
    };
    write(
        "config/cortex/connections.json",
        &json!({"schema_version":1,"connections":{
            CODEX:record(CODEX,"codex-acp"),CLAUDE:record(CLAUDE,"claude-agent-acp")}}),
    );
    write(
        "config/cortex/routing.json",
        &json!({"model_policy_version":2,
            "enabled":{"codex-acp":["gpt-6.1-sol"],"claude-agent-acp":["claude-sonnet-5"]},
            "models":{"codex-acp":{"auto":["gpt-6.1-sol"],"tiers":{"gpt-6.1-sol":["standard"]}},
                "claude-agent-acp":{"auto":["claude-sonnet-5"],"tiers":{"claude-sonnet-5":["standard"]}}}}),
    );
    // The owner default is Codex; a saved Claude choice must not be judged by it.
    write(
        "config/cortex/defaults.json",
        &json!({"defaults":{"coordinator":{"engine":"codex-acp","model":"gpt-6.1-sol",
            "parameters":{"thought_level":"medium"}}}}),
    );
    let pairs = |claude: bool| {
        let mut pairs = if claude {
            json!([
                ["mode", "acceptEdits"],
                ["mode", "auto"],
                ["mode", "bypassPermissions"],
                ["mode", "default"],
                ["mode", "plan"],
                ["model_config", "off"],
                ["model_config", "off"],
                ["model_config", "on"],
                ["model_config", "on"]
            ])
        } else {
            json!([["mode", "agent"]])
        };
        for effort in ["high", "low", "medium"] {
            pairs
                .as_array_mut()
                .unwrap()
                .push(json!(["thought_level", effort]));
        }
        pairs
    };
    for (connection, engine, model, route) in [
        (CODEX, "codex-acp", "gpt-6.1-sol", CODEX_ROUTE),
        (CLAUDE, "claude-agent-acp", "claude-sonnet-5", CLAUDE_ROUTE),
    ] {
        let provenance = json!({"source":"discovery","adapter_version":"unknown",
            "observed_at":"2026-11-15T00:00:00Z","evidence_ref":null});
        let label = format!("{engine} · fixture · {model}");
        write(
            &format!("state/cortex/catalog/{connection}/cs_00000000000a.json"),
            &json!({"snapshot_id":"cs_00000000000a","created_at":"2026-11-15T00:00:00Z",
                "connection_registry_fingerprint":"fixture","setup_action":null,"unavailable_reason":null,
                "entries":[{"route":{"id":route,"engine":engine,"connection_id":connection,"model":model,
                    "supported_parameters":pairs(engine == "claude-agent-acp")},"connection_id":connection,
                    "label":label,"disambiguated_label":label,"catalog_snapshot_id":"cs_00000000000a",
                    "model_name":model,"model_description":null,"context_window_tokens":null,
                    "evidence":{"catalog_obtained":provenance,"configured":provenance}}]}),
        );
    }
    workspace_service::native_routes::ConfiguredRoutes {
        environment: workspace_service::native_routes::RouteEnvironment {
            home: base.join("home"),
            config: base.join("config"),
            state: base.join("state"),
            routing_override: None,
        },
        relay: exe,
    }
}
async fn machine(service: &Service, op: &str, project: &str, params: Value) -> Value {
    let reply = service
        .machine(
            serde_json::from_value(json!({"op":op,"projectId":project,"params":params})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply["ok"], true, "{op}: {reply}");
    reply["result"].clone()
}

/// Exercise the new sprint graph through the real coordinator broker.  The fake
/// engine makes an authenticated `tools/call`, so this covers channel authority,
/// tool advertisement and the runtime's sprint binding together.
async fn sprint_cross_task_gateway() -> CoreResult<()> {
    let f = Fixture::new();
    std::fs::write(f.0.join("project/scenario.txt"), "sprint-cross-task-first").unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let id = service
        .register(&f.0.join("project"), "Sprint gateway")
        .await
        .unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    service.activate(&id).await.unwrap();
    let sprint = machine(
        &service,
        "product.sprint",
        &id,
        json!({"action":"create","description":"Gateway graph fixture","runMode":"manual","checkout":{"kind":"main"}}),
    )
    .await["sprintId"]
        .clone();
    let make = |key: &str| {
        command(
            &id,
            "tasks.create",
            json!(id),
            key,
            json!({"description":format!("Sprint member {key}"),"checkout":{"kind":"main"}}),
        )
    };
    let first = service.workspace(make("sprint-a")).await.unwrap()["target"].clone();
    let second = service.workspace(make("sprint-b")).await.unwrap()["target"].clone();
    let later = service.workspace(make("sprint-c")).await.unwrap()["target"].clone();
    let third = service
        .workspace(make("other-sprint-member"))
        .await
        .unwrap()["target"]
        .clone();
    let other=machine(&service,"product.sprint",&id,json!({"action":"create","description":"Separate sprint","runMode":"manual","checkout":{"kind":"main"}})).await["sprintId"].clone();
    machine(
        &service,
        "product.task",
        &id,
        json!({"action":"sprint","taskId":third,"sprintId":other,"dependencies":[]}),
    )
    .await;
    machine(&service,"product.sprint",&id,json!({"action":"patch","sprintId":other,"patch":{"planning":"created","materializedVersion":1,"taskIds":[third]}})).await;
    std::fs::write(
        f.0.join("project/.git/other-member"),
        third.as_str().unwrap(),
    )?;
    machine(
        &service,
        "product.task",
        &id,
        json!({"action":"sprint","taskId":first,"sprintId":sprint,"dependencies":[]}),
    )
    .await;
    machine(
        &service,
        "product.task",
        &id,
        json!({"action":"sprint","taskId":second,"sprintId":sprint,"dependencies":[first.clone()]}),
    )
    .await;
    machine(
        &service,
        "product.task",
        &id,
        json!({"action":"sprint","taskId":later,"sprintId":sprint,"dependencies":[first,second]}),
    )
    .await;
    machine(
        &service,
        "product.sprint",
        &id,
        json!({"action":"patch","sprintId":sprint,"patch":{"planning":"created","materializedVersion":1,"taskIds":[first.clone(),second,later]}}),
    )
    .await;
    let start = command(
        &id,
        "tasks.start",
        first.clone(),
        "sprint-gateway-start",
        json!({}),
    );
    let receipt = service.workspace(start).await.unwrap();
    assert_eq!(receipt["outcome"], "recorded", "sprint start: {receipt}");
    let runtime_task = {
        let store = f.store();
        store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id WHERE w.task_id=?",
                    [first.as_str().unwrap()],
                    |row| row.get::<_, String>(0),
                )?)
            })?
    };
    service.stop().await.unwrap();
    drop(service);
    let store = f.store();
    let publisher = cortex_core::store::documents::Publisher::new(&store);
    let draft = publisher.create_draft(&runtime_task, "fixture", "general", "sprint-report")?;
    publisher.write_draft(
        &runtime_task,
        "fixture",
        &draft.draft_id,
        &format!(
            "Cortex draft ID: `{}`\n\n{}\nDecision: graph handoff is authoritative.\n",
            draft.draft_id,
            "evidence ".repeat(520)
        ),
    )?;
    publisher.publish(
        &runtime_task,
        "fixture",
        &draft.draft_id,
        "publish-sprint-report",
        cortex_core::store::documents::Publication {
            title: "Graph handoff",
            summary: "Decision for the dependent task",
            author: "fixture",
            source_revision: 0,
            artifacts: json!([]),
        },
    )?;
    drop(store);
    std::fs::write(f.0.join("project/scenario.txt"), "sprint-cross-task").unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    service.activate(&id).await.unwrap();
    let start = command(
        &id,
        "tasks.start",
        second,
        "sprint-gateway-dependent-start",
        json!({}),
    );
    let receipt = service.workspace(start).await.unwrap();
    assert_eq!(receipt["outcome"], "recorded", "dependent start: {receipt}");
    for _ in 0..240 {
        if f.0.join("project/.git/sprint-tool-result").exists() {
            service.stop().await.unwrap();
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("coordinator did not complete sprint_task_result gateway call");
}
/// Live D3 regression: Start refused every task (single or sprint) saved with a
/// chosen model/effort (`native_start_options_unavailable`). A saved Claude
/// `claude-sonnet-5` or Codex `gpt-6.1-sol` choice with a valid effort now
/// starts through the production route provider, and its coordinator is
/// configured with exactly that model and effort; invalid choices are refused
/// with their own reason before anything is admitted.
async fn saved_settings_start(route: &str, sprint: bool) {
    let f = Fixture::new();
    let routes = owner_routes(&f);
    std::fs::write(f.0.join("project/scenario.txt"), "saved-settings").unwrap();
    let service = Service::bind_native(f.config(), Arc::new(routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Fixture")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap().to_owned();
    service.activate(&id).await.unwrap();
    let settings = json!({"model":route,"effort":"high","mode":"guarded","language":"ru"});
    let create = |request: &str, settings: &Value| {
        command(
            &id,
            "tasks.create",
            json!(id),
            request,
            json!({"description":"Saved settings fixture","settings":settings,"checkout":{"kind":"main"}}),
        )
    };
    let draft = |reply: Value| reply["target"].as_str().unwrap().to_owned();
    let store = f.store();
    let runtime_tasks = || {
        store
            .read(|db| {
                Ok(db.query_row("SELECT count(*) FROM runtime_tasks", [], |r| {
                    r.get::<_, i64>(0)
                })?)
            })
            .unwrap()
    };
    // Sprint planning claimed before the service runs is cancelled, never rerun.
    let sprint_id = if sprint {
        Some(
            machine(
                &service,
                "product.sprint",
                &id,
                json!({"action":"create","description":"Saved sprint",
                "runMode":"hybrid","checkout":{"kind":"main"},"settings":settings}),
            )
            .await["sprintId"]
                .clone(),
        )
    } else {
        None
    };
    let mut events = service.subscribe();
    let runner = tokio::spawn(service.clone().serve());
    if !sprint {
        for (n, (model, effort, reason)) in [
            ("claude-retired-9", "high", "native_task_route_unavailable"),
            (route, "ultra", "unsupported_dependent_setting"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut invalid = settings.clone();
            invalid["model"] = json!(model);
            invalid["effort"] = json!(effort);
            let target = draft(
                service
                    .workspace(create(&format!("create-invalid-{n}"), &invalid))
                    .await
                    .unwrap(),
            );
            let refused = service
                .workspace(command(
                    &id,
                    "tasks.start",
                    json!(target),
                    &format!("start-invalid-{n}"),
                    json!({}),
                ))
                .await
                .unwrap_err()
                .to_string();
            assert_eq!(refused, reason, "{model}/{effort}");
            assert_eq!(runtime_tasks(), 0, "refused Start admitted a task");
        }
        let target = draft(
            service
                .workspace(create("create", &settings))
                .await
                .unwrap(),
        );
        let receipt = service
            .workspace(command(
                &id,
                "tasks.start",
                json!(target),
                "start",
                json!({}),
            ))
            .await
            .unwrap_or_else(|e| panic!("saved {route} task refused: {e}"));
        assert_eq!(receipt["outcome"], "recorded");
    } else {
        let sprint_id = sprint_id.clone().unwrap();
        let sprint_record = || async {
            machine(&service, "product.board", &id, json!({})).await["sprints"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["id"] == sprint_id)
                .unwrap()
                .clone()
        };
        tokio::time::timeout(Duration::from_secs(20), async {
            while sprint_record().await["planning"] != "cancelled" {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("interrupted sprint planning was not cancelled");
        // Live defect: a planner question saved in the background reached the open window
        // only on its slow facts poll, a minute later. A background change to the owner
        // state is announced to subscribers when it is saved.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = service.receive_event(&mut events).await.unwrap();
                if event["type"] == "invalidation" && event["projectId"] == json!(id) {
                    break;
                }
            }
        })
        .await
        .expect("the background sprint change was never announced to subscribers");
        // Materialize exactly like the desktop (`real.ts` materialize_plan).
        let mut task_settings = json!({"mode":"guarded","language":"auto"});
        for (k, v) in settings.as_object().unwrap() {
            task_settings[k] = v.clone();
        }
        let target = draft(
            service
                .workspace(create("create-sprint-task", &task_settings))
                .await
                .unwrap(),
        );
        machine(
            &service,
            "product.task",
            &id,
            json!({"action":"sprint","taskId":target,
            "sprintId":sprint_id,"dependencies":[]}),
        )
        .await;
        machine(
            &service,
            "product.sprint",
            &id,
            json!({"action":"patch","sprintId":sprint_id,
            "patch":{"planning":"created","materializedVersion":1,"taskIds":[target]}}),
        )
        .await;
        machine(
            &service,
            "product.sprint",
            &id,
            json!({"action":"patch","sprintId":sprint_id,
            "patch":{"queueState":"running","pauseReason":null}}),
        )
        .await;
    }
    let observed = f.0.join("project/observed-settings.json");
    let started = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if let Ok(raw) = std::fs::read_to_string(&observed) {
                if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                    break v;
                }
            }
            let diagnostics: String = store.read(|db| Ok(db.query_row("SELECT coalesce(group_concat(type || ':' || payload_json),'') FROM runtime_events WHERE type IN ('NATIVE_DRIVER_STOPPED','NATIVE_SESSION_START_FAILED')",[],|r|r.get(0))?)).unwrap();
            assert!(diagnostics.is_empty(), "coordinator did not start: {diagnostics}");
            if let Some(sprint_id) = &sprint_id {
                let sprints = machine(&service, "product.board", &id, json!({})).await["sprints"].clone();
                let paused = sprints.as_array().unwrap().iter().find(|s| s["id"] == *sprint_id).unwrap()["pauseReason"].clone();
                assert!(paused.is_null(), "sprint queue paused: {paused}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("saved-settings coordinator was never prompted");
    let (model, option) = if route == CLAUDE_ROUTE {
        ("claude-sonnet-5", "effort")
    } else {
        ("gpt-6.1-sol", "reasoning_effort")
    };
    assert_eq!(
        started,
        json!({"model":model,"effort":"high","option":option,"language":true})
    );
    store.read(|db| {
        assert_eq!(db.query_row("SELECT count(*) FROM runtime_tasks",[],|r|r.get::<_,i64>(0))?,1);
        let recorded: String = db.query_row("SELECT json_extract(payload_json,'$.settings.model') FROM runtime_events WHERE type='NATIVE_TASK_SETTINGS'",[],|r|r.get(0))?;
        assert_eq!(recorded, route);
        Ok(())
    }).unwrap();
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
    drop(store);
    drop(service);
    println!("saved settings Start: {model} ({option}=high), sprint={sprint}");
}
/// A worker whose question is still open: owner guidance reaches it as guidance (with the
/// not-authority prefix) and a process that dies while waiting is noticed without an answer.
async fn waiting_worker(variant: &str) {
    let f = Fixture::new();
    std::fs::write(
        f.0.join("project/scenario.txt"),
        match variant {
            "guidance" => "question",
            "preempt" => "guidance-preempt",
            _ => "question-exit",
        },
    )
    .unwrap();
    std::fs::write(f.0.join("project/record-prompts"), "").unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Waiting worker")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap();
    service.activate(id).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    let store = f.store();
    let created = service
        .workspace(command(
            id,
            "tasks.create",
            json!(id),
            "create",
            json!({"description":"Waiting worker","checkout":{"kind":"main"}}),
        ))
        .await
        .unwrap();
    let started = service
        .workspace(command(
            id,
            "tasks.start",
            created["target"].clone(),
            "start",
            json!({}),
        ))
        .await
        .unwrap();
    let execution = started["target"].as_str().unwrap();
    async fn wait_for(store: &Store, sql: &str) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if store
                    .read(|db| Ok(db.query_row(sql, [], |r| r.get::<_, bool>(0))?))
                    .unwrap()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("checkpoint missing: {sql}"));
    }
    if variant == "preempt" {
        wait_for(&store, "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE role_id='worker-backend-dev' AND status='ready')").await;
        let call = serde_json::from_value(json!({"op":"product.facts","projectId":id,"params":{}}))
            .unwrap();
        let facts = service.machine(call).await.unwrap();
        let worker = facts["result"]["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["role"] == "backend-dev")
            .unwrap()["id"]
            .clone();
        let send = |text: &'static str, request: &'static str| {
            service.control_execution(
                json!({"action":"worker_message","workerId":worker,"text":text,
                "attachments":[],"projectId":id,"executionId":execution,"requestId":request}),
            )
        };
        send("First guidance", "guide-1").await.unwrap();
        // The first guidance prompt is active (held by the fixture) when the second arrives.
        tokio::time::timeout(Duration::from_secs(15), async {
            while !std::fs::read_to_string(f.0.join("project/prompt-log.jsonl"))
                .is_ok_and(|l| l.contains("First guidance"))
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("first guidance prompt never reached the worker");
        send("Second guidance", "guide-2").await.unwrap();
        wait_for(
            &store,
            "SELECT (SELECT count(*) FROM runtime_events WHERE type='NATIVE_GUIDANCE_SETTLED')>=2",
        )
        .await;
        let settled: Vec<(String, bool, bool)> = store.read(|db| {
            let mut q = db.prepare("SELECT json_extract(payload_json,'$.request'),json_extract(payload_json,'$.delivered'),
                coalesce(json_extract(payload_json,'$.preempted'),0) FROM runtime_events WHERE type='NATIVE_GUIDANCE_SETTLED' ORDER BY id")?;
            let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }).unwrap();
        // The pre-empted guidance reached the worker: delivered, and marked as replaced.
        assert_eq!(
            settled[0],
            ("guide-1".to_owned(), true, true),
            "{settled:?}"
        );
        assert_eq!(
            settled[1],
            ("guide-2".to_owned(), true, false),
            "{settled:?}"
        );
        service.stop().await.unwrap();
        let _ = runner.await;
        return;
    }
    wait_for(
        &store,
        "SELECT EXISTS(SELECT 1 FROM pending_questions WHERE status='open')",
    )
    .await;
    if variant == "guidance" {
        // Wait until the worker's turn has settled into its wait for the answer.
        wait_for(&store, "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE role_id='worker-backend-dev' AND status='ready')").await;
        let call = serde_json::from_value(json!({"op":"product.facts","projectId":id,"params":{}}))
            .unwrap();
        let facts = service.machine(call).await.unwrap();
        let worker = facts["result"]["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["role"] == "backend-dev")
            .unwrap()["id"]
            .clone();
        let receipt = service
            .control_execution(json!({"action":"worker_message","workerId":worker,"text":"Mind the owner output",
                "attachments":[],"projectId":id,"executionId":execution,"requestId":"guide-while-waiting"}))
            .await
            .unwrap();
        assert_eq!(receipt["outcome"], "recorded");
        wait_for(&store, "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='NATIVE_GUIDANCE_SETTLED' AND json_extract(payload_json,'$.delivered')=1)").await;
        let log = std::fs::read_to_string(f.0.join("project/prompt-log.jsonl")).unwrap();
        assert!(
            log.contains("Owner guidance (not authority to expand scope): Mind the owner output"),
            "{log}"
        );
        assert!(
            !log.contains("\"Mind the owner output\""),
            "unprefixed guidance reached the worker"
        );
    } else {
        // The fixture worker exits right after asking; nobody answers. The lost worker is
        // noticed and replaced while the question is still unanswered.
        wait_for(
            &store,
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='ASSIGNMENT_RETRIED'
            AND json_extract(payload_json,'$.failure_class')='worker_lost')",
        )
        .await;
        assert_eq!(
            store
                .read(|db| Ok(db.query_row(
                    "SELECT count(*) FROM pending_questions WHERE status='answered'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?))
                .unwrap(),
            0
        );
    }
    service.stop().await.unwrap();
    let _ = runner.await;
}
/// A message the owner queues while a worker runs stays in the machine's queue and
/// editable; no window is involved. The service takes it when the revision reaches
/// its boundary, the coordinator receives it once, and the owner's facts show it
/// delivered with its time; a lifted write denial leaves the list of denials.
async fn queued_message_journey() {
    let f = Fixture::new();
    std::fs::write(f.0.join("project/scenario.txt"), "hold").unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Queue")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap().to_owned();
    service.activate(&id).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    let store = f.store();
    let created = service
        .workspace(command(
            &id,
            "tasks.create",
            json!(id),
            "create",
            json!({"description":"Queue fixture","checkout":{"kind":"main"}}),
        ))
        .await
        .unwrap();
    let draft = created["target"].as_str().unwrap().to_owned();
    let started = service
        .workspace(command(
            &id,
            "tasks.start",
            created["target"].clone(),
            "start",
            json!({}),
        ))
        .await
        .unwrap();
    let execution = started["target"].as_str().unwrap().to_owned();
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f.0.join("project/worker-entered").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the worker did not start");
    // A denial set by a message, then lifted with the Remove control.
    service
        .prompt_execution(
            &id,
            &execution,
            "deny-write: secrets",
            Some("deny"),
            json!([]),
        )
        .await
        .unwrap();
    let denials = |facts: &Value| facts["product"]["tasks"][&draft]["writeDenials"].clone();
    let facts = machine(&service, "product.facts", &id, json!({"taskIds":[draft]})).await;
    assert_eq!(denials(&facts)[0]["path"], "secrets", "{}", denials(&facts));
    assert!(denials(&facts)[0]["setAt"].is_i64());
    service.control_execution(json!({"projectId":id,"executionId":execution,"requestId":"lift","action":"allow_write","path":"secrets"})).await.unwrap();
    let facts = machine(&service, "product.facts", &id, json!({"taskIds":[draft]})).await;
    assert_eq!(denials(&facts), json!([]));
    // Queued while the worker runs: kept in the queue, still the owner's to edit.
    machine(&service, "product.message", &id, json!({"action":"add","message":{"id":"queued-1","taskId":draft,"workerId":null,
        "text":"Queued owner note","mode":"queue","delivery":"queued","at":1,"deliveredAt":null,"failure":null}})).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    machine(
        &service,
        "product.message",
        &id,
        json!({"action":"edit","messageId":"queued-1","text":"Queued owner note, edited"}),
    )
    .await;
    let steering = |store: &cortex_core::Store, sql: &str| {
        store
            .read(|db| Ok(db.query_row(sql, [], |r| r.get::<_, i64>(0))?))
            .unwrap()
    };
    assert_eq!(
        steering(
            &store,
            "SELECT count(*) FROM runtime_events WHERE type='USER_STEERING' AND json_extract(payload_json,'$.request')='queued-1'"
        ),
        0
    );
    assert_eq!(
        steering(
            &store,
            "SELECT count(*) FROM runtime_events WHERE type='TURN_INTERRUPTED'"
        ),
        0
    );
    // The worker finishes; the service sends the queued message at the boundary.
    std::fs::write(f.0.join("project/scenario.txt"), "complete").unwrap();
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            let state: String = store
                .read(|db| Ok(db.query_row("SELECT state FROM runtime_tasks", [], |r| r.get(0))?))
                .unwrap();
            if state == "COMPLETED" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the queued message journey did not complete");
    assert_eq!(steering(&store, "SELECT count(*) FROM runtime_events WHERE type='USER_STEERING' AND json_extract(payload_json,'$.request')='queued-1'
        AND json_extract(payload_json,'$.delivery')='boundary' AND json_extract(payload_json,'$.text')='Queued owner note, edited'"), 1);
    assert_eq!(
        steering(
            &store,
            "SELECT count(*) FROM runtime_events WHERE type='STEERING_DELIVERED'"
        ),
        2
    );
    let synthesis: String = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT coalesce(synthesis,'') FROM runtime_tasks",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert!(
        synthesis.contains("Owner guidance applied (deny-write: secrets)"),
        "{synthesis}"
    );
    // The owner's facts: the queue row became the runtime's delivered record.
    let facts = machine(&service, "product.facts", &id, json!({"taskIds":[draft]})).await;
    let row = facts["product"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "queued-1")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(row["delivery"], "delivered", "{row}");
    assert!(row["deliveredAt"].is_i64(), "{row}");
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
/// The degenerate-turn watchdog end to end. `degenerate-once`: the first coordinator
/// session streams its own prompt text back and keeps generating; the runtime cancels
/// that turn, records which rule fired, and runs the turn again in a fresh session,
/// which plans and the task completes. `degenerate-twice`: the retry loops too, and
/// the task stops with the `native_turn_degenerate` blocker.
async fn degenerate_turn_journey(scenario: &str) {
    let f = Fixture::new();
    let refusal = scenario == "usage-refusal-twice";
    let twice = scenario == "degenerate-twice" || refusal;
    let worker = scenario == "degenerate-worker";
    std::fs::write(f.0.join("project/scenario.txt"), scenario).unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Degenerate")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap().to_owned();
    service.activate(&id).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    let store = f.store();
    let created = service
        .workspace(command(
            &id,
            "tasks.create",
            json!(id),
            "create",
            json!({"description":"Degenerate fixture","checkout":{"kind":"main"}}),
        ))
        .await
        .unwrap();
    service
        .workspace(command(
            &id,
            "tasks.start",
            created["target"].clone(),
            "start",
            json!({}),
        ))
        .await
        .unwrap();
    let count = |sql: &str| {
        store
            .read(|db| Ok(db.query_row(sql, [], |r| r.get::<_, i64>(0))?))
            .unwrap()
    };
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let state: String = store
                .read(|db| Ok(db.query_row("SELECT state FROM runtime_tasks", [], |r| r.get(0))?))
                .unwrap();
            let stopped =
                count("SELECT count(*) FROM runtime_events WHERE type='NATIVE_DRIVER_STOPPED'");
            if if twice {
                stopped > 0
            } else {
                state == "COMPLETED"
            } {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{scenario}: the degenerate journey did not settle"));
    let (rule, role) = match scenario {
        "degenerate-twice" => ("repetition", "coordinator"),
        "usage-refusal-twice" => ("refusal", "coordinator"),
        "degenerate-worker" => ("length", "worker"),
        _ => ("echo", "coordinator"),
    };
    // A streamed trip keeps only the rule, message id and size, never the text.
    let detail = if refusal {
        "AND json_extract(payload_json,'$.chars')=0"
    } else {
        "AND json_extract(payload_json,'$.message_id') LIKE 'degenerate-%'
         AND json_extract(payload_json,'$.chars')>0"
    };
    let trips = count(&format!(
        "SELECT count(*) FROM runtime_events WHERE type='NATIVE_TURN_DEGENERATE'
         AND json_extract(payload_json,'$.rule')='{rule}' AND json_extract(payload_json,'$.role')='{role}'
         {detail}"
    ));
    if refusal {
        // The runtime still applies its refusal policy after committing the ACP settle.
        // The original turn and the fresh-session retry each have one correlated row.
        assert_eq!(
            count(
                "SELECT count(*) FROM (
                 SELECT id,task_id,payload_json FROM runtime_events
                 WHERE type='NATIVE_TURN_SETTLED' ORDER BY id LIMIT 2
                 ) e JOIN usage_records u
                 ON u.task_id=e.task_id AND u.id=json_extract(e.payload_json,'$.usage_id')
                 WHERE u.stop_reason='refusal' AND u.failed=1 AND u.call_sequence=1
                   AND u.basis='engine_reported' AND json_extract(u.tokens_json,'$.totalTokens')=2"
            ),
            2
        );
        assert_eq!(
            count(
                "SELECT count(DISTINCT engine_session_ref) FROM (
                 SELECT u.engine_session_ref
                 FROM runtime_events e JOIN usage_records u
                 ON u.task_id=e.task_id AND u.id=json_extract(e.payload_json,'$.usage_id')
                 WHERE e.type='NATIVE_TURN_SETTLED' ORDER BY e.id LIMIT 2
                 )"
            ),
            2
        );
    }
    // Twice: the no-stop policy then continues the task by itself (and the fixture's
    // model keeps degenerating), so later trips may follow the first two.
    if twice {
        assert!(trips >= 2, "{scenario}: {trips}");
    } else {
        assert_eq!(trips, 1, "{scenario}");
    }
    assert_eq!(
        count(
            "SELECT count(*) FROM runtime_events WHERE type='NATIVE_TURN_DEGENERATE' AND instr(payload_json,'guner@guner@')>0"
        ),
        0
    );
    // The text after the trip never reaches the conversation.
    assert_eq!(
        count(
            "SELECT count(*) FROM host_transcript WHERE instr(text,'Runtime status:')>0 OR instr(text,'Further echoed')>0"
        ),
        0
    );
    if twice {
        assert!(
            count(
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_DRIVER_STOPPED' AND json_extract(payload_json,'$.reason')='native_turn_degenerate'"
            ) >= 1
        );
        // No other model qualifies in this fixture: the stop is a continuation on an
        // escalated rung that moves the coordinator, never an owner question or pause.
        assert!(
            count("SELECT count(*) FROM runtime_events WHERE type='AUTOMATIC_CONTINUATION' AND json_extract(payload_json,'$.reason')='native_turn_degenerate'
                AND json_extract(payload_json,'$.strategy')='escalate_route' AND json_extract(payload_json,'$.switch_coordinator')=1")
                >= 1
        );
        assert_eq!(
            count("SELECT count(*) FROM runtime_events WHERE type='AUTOMATIC_PAUSED'"),
            0
        );
        assert_eq!(count("SELECT count(*) FROM pending_questions"), 0);
        assert_eq!(
            count("SELECT count(*) FROM runtime_events WHERE type='PLAN_ACCEPTED'"),
            0
        );
    } else if worker {
        // The node keeps no result from the degenerate turn: it was retried as a
        // `degenerate_turn` (a quality retry), and the fresh worker session finished it.
        assert_eq!(
            count(
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_TURN_DEGENERATE' AND json_extract(payload_json,'$.node')='work' AND json_extract(payload_json,'$.revision')=1"
            ),
            1
        );
        assert_eq!(
            count(
                "SELECT count(*) FROM runtime_events WHERE type='ASSIGNMENT_RETRIED' AND json_extract(payload_json,'$.failure_class')='degenerate_turn'"
            ),
            1
        );
        assert_eq!(
            count("SELECT count(*) FROM runtime_events WHERE type='NATIVE_DRIVER_STOPPED'"),
            0
        );
    } else {
        // The retry ran in a fresh engine session, configured after the degenerate one.
        assert!(
            count(
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_SESSION_CONFIGURED' AND id<(SELECT min(id) FROM runtime_events WHERE type='PLAN_ACCEPTED')"
            ) >= 2
        );
        assert_eq!(
            count("SELECT count(*) FROM runtime_events WHERE type='NATIVE_DRIVER_STOPPED'"),
            0
        );
    }
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
async fn controls(scenario: &str, action: &str) {
    let f = Fixture::new();
    std::fs::write(f.0.join("project/scenario.txt"), scenario).unwrap();
    if scenario == "repair-failed-test" {
        std::fs::write(f.0.join("project/expected.txt"), "expected").unwrap();
    }
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Fixture")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap();
    service.activate(id).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    let store = f.store();
    let created = service
        .workspace(command(
            id,
            "tasks.create",
            json!(id),
            "create",
            // The fake coordinator's reconcile journeys check that the retained goal
            // keeps the owner's constraint, so those journeys state one.
            json!({"description":if scenario.starts_with("reconcile-") {"Control fixture. Never lose constraint α."} else {"Control fixture"},
                "checkout":{"kind":"main"}}),
        ))
        .await
        .unwrap();
    if scenario == "access-retry" {
        TaskMachine::new(&store)
            .create("access-holder", "Fixture holder", "owner")
            .unwrap();
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='access-holder'", [])?;
            db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status,assignment_ref) VALUES ('access-holder',1,'holder','worker-backend-dev','Hold extra', '[]','[\"extra\"]','execution','running','access-holder-assignment')", [])?;
            db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,channel_id,engine_session_ref,owner_epoch,created_at,updated_at) VALUES ('access-holder-assignment','access-holder','access-holder-attempt','worker-backend-dev','ready','ready','access-holder-channel','access-holder-session',1,?,?)", [cortex_core::now(), cortex_core::now()])?;
            Ok(())
        }).unwrap();
    }
    let started = service
        .workspace(command(
            id,
            "tasks.start",
            created["target"].clone(),
            "start",
            json!({}),
        ))
        .await
        .unwrap();
    let execution = started["target"].as_str().unwrap();
    if scenario == "repair-failed-test" {
        let task: String = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT runtime_task_id FROM workspace_executions WHERE execution_id=?",
                    [execution],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let state: String = store
                    .read(|db| {
                        Ok(db.query_row(
                            "SELECT state FROM runtime_tasks WHERE task_id=?",
                            [&task],
                            |r| r.get(0),
                        )?)
                    })
                    .unwrap();
                if state == "COMPLETED" {
                    break;
                }
                assert!(
                    !matches!(state.as_str(), "WAITING_USER" | "FAILED" | "CANCELLED"),
                    "unexpected repair state {state}"
                );
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .expect("repair did not reach completion");
        let results: Vec<(String, String)> = store
            .read(|db| {
                Ok(db
                    .prepare(
                        "SELECT type,payload_json FROM runtime_events
            WHERE task_id=? AND type IN ('VERIFICATION_FAILED','VERIFICATION_PASSED') ORDER BY id",
                    )?
                    .query_map([&task], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        assert_eq!(results.len(), 2);
        let mut references = std::collections::HashSet::new();
        for ((kind, raw), (expected, revision, code)) in results
            .iter()
            .zip([("VERIFICATION_FAILED", 1, 1), ("VERIFICATION_PASSED", 2, 0)])
        {
            let payload: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(kind, expected);
            assert_eq!(payload["revision"], revision);
            let checks = payload["checks"].as_array().unwrap();
            assert_eq!(checks.len(), 1);
            assert_eq!(
                checks[0]["argv"],
                json!(["/usr/bin/cmp", "expected.txt", "work.txt"])
            );
            assert_eq!(checks[0]["exit_code"], code);
            assert_eq!(checks[0]["complete"], true);
            assert!(references.insert(checks[0]["receipt_ref"].as_str().unwrap().to_owned()));
        }
        let reviews:i64=store.read(|db|Ok(db.query_row("SELECT count(DISTINCT json_extract(payload_json,'$.assignment_ref')) FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'",[&task],|r|r.get(0))?)).unwrap();
        assert_eq!(reviews, 2);
        service.stop().await.unwrap();
        runner.await.unwrap().unwrap();
        return;
    }
    let query = json!({"protocol":{"major":2,"mapping":2},"requestId":"page","projectId":id,"operation":"tasks.page","payload":{"target":id,"cursor":null,"limit":50}});
    let mut events = service.subscribe();
    let initial = read_page(&service, query.clone()).await;
    tokio::time::timeout(Duration::from_secs(10),async {
        loop {
            let ready:bool=store.read(|db| Ok(db.query_row(
                // A crash-looping worker (its engine exits unprompted: an infrastructure
                // failure) is retried up to its lineage cap and then waits without a
                // model call; it never stops the driver or takes a ladder rung.
                if scenario=="exit" {"SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='ASSIGNMENT_RETRIED'
                    AND json_extract(payload_json,'$.infrastructure_wait')=1)
                    AND NOT EXISTS(SELECT 1 FROM runtime_events WHERE type='NATIVE_DRIVER_STOPPED'
                    AND json_extract(payload_json,'$.reason')='worker_retry_limit')"}
                else if scenario=="question" || scenario=="batch" || scenario=="reconcile-question" {"SELECT EXISTS(SELECT 1 FROM pending_questions WHERE status='open')"}
                else {"SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE role_id!='orchestrator' AND status='ready')"},
                [],|r|r.get(0))?)).unwrap();
            // A ready provisioning row precedes capsule preparation and the
            // first prompt. Wait for the held fake turn before testing a
            // versioned owner action; changing its scenario sooner races real
            // worker progress and correctly produces a stale-version refusal.
            if ready && (scenario!="hold" || f.0.join("project/worker-entered").exists()) {break;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap_or_else(|_| {
        let events=store.read(|db|Ok(db.query_row(
            "SELECT (SELECT state FROM runtime_tasks)||' '||coalesce((SELECT group_concat(type||CASE WHEN type IN ('NATIVE_DRIVER_STOPPED','ASSIGNMENT_RETRIED','TASK_STATE_CHANGED') THEN payload_json ELSE '' END,',') FROM (SELECT type,payload_json FROM runtime_events ORDER BY id DESC LIMIT 40)),'')",
            [],|r|r.get::<_,String>(0))?)).unwrap();
        panic!("control fixture did not reach its checkpoint: {events}")
    });
    let current = read_page(&service, query.clone()).await;
    let version = current["data"]["value"]["items"][0]["version"].clone();
    if scenario == "access-retry" {
        let count = |sql: &str| {
            store
                .read(|db| Ok(db.query_row(sql, [], |r| r.get::<_, i64>(0))?))
                .unwrap()
        };
        // No owner action or service notification drives either timed retry.
        while count("SELECT count(*) FROM runtime_events WHERE type='WORKER_WAIT_CHANGED'
            AND json_extract(payload_json,'$.status')='open' AND json_extract(payload_json,'$.retry_attempt')=1")==0 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let granted = action == "release";
        if granted {
            store.write(|db| {
                db.execute("UPDATE delegation_provisioning SET status='terminated' WHERE idempotency_key='access-holder-assignment'", [])?;
                Ok(())
            }).unwrap();
        }
        while count(
            "SELECT count(*) FROM runtime_events WHERE type='WORKER_WAIT_CHANGED' AND json_extract(payload_json,'$.status')='resumed'",
        ) == 0
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let decision: String = store.read(|db| Ok(db.query_row("SELECT payload_json FROM runtime_events WHERE type='WORKER_CONTINUATION_STARTED'", [], |r| r.get(0))?)).unwrap();
        let wait: Value = serde_json::from_str(&decision).unwrap();
        assert_eq!(
            wait["access_decision"]["status"],
            if granted { "granted" } else { "denied" }
        );
        if !granted {
            assert_eq!(wait["access_decision"]["reason"], "active_writer_conflict");
        }
        assert_eq!(
            count("SELECT count(*) FROM runtime_events WHERE type='ACCESS_GRANTED'"),
            i64::from(granted)
        );
        assert_eq!(
            count("SELECT count(*) FROM runtime_events WHERE type='WORKER_CONTINUATION_STARTED'"),
            1
        );
        assert_eq!(
            count(
                "SELECT count(*) FROM runtime_events WHERE type IN ('USER_QUESTION_REQUESTED','ASSIGNMENT_RETRIED','NATIVE_DRIVER_STOPPED')"
            ),
            0
        );
        let recorded: bool = store.read(|db| Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='WORKER_COMPLETED' AND json_extract(payload_json,'$.attempt_id')=?)",
            [wait["attempt_id"].as_str().unwrap()], |r| r.get(0))?)).unwrap();
        assert!(recorded);
        service.stop().await.unwrap();
        runner.await.unwrap().unwrap();
        return;
    }
    if action == "pending-result" || action == "pending-end-turn" {
        let count = |sql: &str| {
            store
                .read(|db| Ok(db.query_row(sql, [], |r| r.get::<_, i64>(0))?))
                .unwrap()
        };
        let refusal = "SELECT count(*) FROM runtime_events t JOIN runtime_events e ON t.task_id=e.task_id AND t.id>e.id AND t.type='NATIVE_TURN_SETTLED'
              AND json_extract(t.payload_json,'$.channel_id')=json_extract(e.payload_json,'$.channel_id')
              WHERE e.type='WORKER_RESULT_REFUSED' AND json_extract(e.payload_json,'$.code')='command_not_settled'";
        let command_started = "SELECT count(*) FROM runtime_events t JOIN runtime_events e ON t.task_id=e.task_id AND t.id>e.id
            WHERE t.type='NATIVE_TURN_SETTLED' AND e.type='NATIVE_COMMAND_PENDING_REPLY'
            AND json_extract(t.payload_json,'$.channel_id')=json_extract(e.payload_json,'$.channel_id')";
        while count(if action == "pending-end-turn" {
            command_started
        } else {
            refusal
        }) == 0
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        std::fs::write(f.0.join("project/release-command"), "ready").unwrap();
        while count("SELECT count(*) FROM runtime_events WHERE type='WORKER_COMPLETED'") == 0 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            count(
                "SELECT count(*) FROM runtime_requests WHERE request_key='pending-command' AND json_extract(result_json,'$.exit_code')=0"
            ),
            1
        );
        assert_eq!(
            count(
                "SELECT count(*) FROM runtime_events WHERE type IN ('ASSIGNMENT_RETRIED','NATIVE_DRIVER_STOPPED','USER_QUESTION_REQUESTED')"
            ),
            0
        );
        service.stop().await.unwrap();
        runner.await.unwrap().unwrap();
        return;
    }
    if action == "revoked" {
        // The runtime itself revokes a live worker (as an accepted escalation or replan
        // request does) with no owner signal to its turn. The held fixture turn never
        // ends on its own: the runtime ends it, settles the attempt and leaves recovery
        // within seconds instead of waiting for the engine to finish by itself.
        let (task, assignment, channel, attempt): (String, String, String, String) = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT task_id,idempotency_key,channel_id,attempt_id FROM delegation_provisioning
                     WHERE role_id!='orchestrator' AND status='ready'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?)
            })
            .unwrap();
        let revoked = std::time::Instant::now();
        store
            .write(|db| cortex_core::lifecycle::domain::interrupt_tx(db, &task))
            .unwrap();
        let settled = "SELECT EXISTS(SELECT 1 FROM capacity_reservations WHERE attempt_id=?1 AND state='released')
            AND EXISTS(SELECT 1 FROM delegation_provisioning WHERE attempt_id=?1 AND status='terminated')
            AND EXISTS(SELECT 1 FROM runtime_events e, json_each(e.payload_json,'$.assignment_refs') a
                WHERE e.type='LIVE_CANCELLATIONS_SETTLED' AND a.value=?2)";
        tokio::time::timeout(Duration::from_secs(10), async {
            while !store
                .read(|db| Ok(db.query_row(settled, (&attempt, &assignment), |r| r.get::<_, bool>(0))?))
                .unwrap()
            {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            let state = store.read(|db| Ok(db.query_row(
                "SELECT (SELECT group_concat(state,',') FROM capacity_reservations WHERE attempt_id=?1)||' '||
                    (SELECT status FROM delegation_provisioning WHERE attempt_id=?1)||' '||
                    (SELECT state||':'||coalesce(blocker,'') FROM runtime_tasks WHERE task_id=?2)",
                (&attempt, &task), |r| r.get::<_, String>(0))?)).unwrap();
            panic!("the revoked worker turn kept running: {state}")
        });
        // The turn ends through the engine's authority monitor (transport closed) or,
        // when that does not observe the revocation, through `session/cancel`.
        let ending: String = store
            .read(|db| Ok(db.query_row(
                "SELECT coalesce((SELECT json_extract(payload_json,'$.stop_reason') FROM runtime_events
                    WHERE type='NATIVE_TURN_SETTLED' AND json_extract(payload_json,'$.channel_id')=?),'terminated')",
                [&channel], |r| r.get(0))?))
            .unwrap();
        assert!(
            matches!(ending.as_str(), "cancelled" | "terminated"),
            "{ending}"
        );
        println!(
            "revoked worker turn ended ({ending}) and settled in {:?}",
            revoked.elapsed()
        );
        service.stop().await.unwrap();
        runner.await.unwrap().unwrap();
        return;
    }
    if action == "ambiguous" {
        store.write(|db| {db.execute_batch("CREATE TRIGGER fail_control_finish BEFORE INSERT ON runtime_requests WHEN NEW.operation='native_desktop_finished' BEGIN SELECT RAISE(ABORT,'injected'); END;")?;Ok(())}).unwrap();
        let request =
            json!({"projectId":id,"executionId":execution,"requestId":"unknown","action":"pause"});
        assert!(service.control_execution(request.clone()).await.is_err());
        assert_eq!(
            service
                .control_execution(request)
                .await
                .unwrap_err()
                .to_string(),
            "native_control_outcome_unknown"
        );
        service.stop().await.unwrap();
        runner.await.unwrap().unwrap();
        return;
    }
    if action == "unavailable" {
        let before: i64 = store
            .read(|db| Ok(db.query_row("SELECT count(*) FROM runtime_requests", [], |r| r.get(0))?))
            .unwrap();
        for (action, payload) in [
            (
                "worker_message",
                json!({"workerId":"worker-fixture","text":"Guidance","attachments":[]}),
            ),
            (
                "worker_stop",
                json!({"workerId":"worker-fixture","reason":"Owner stop"}),
            ),
            (
                "command_stop",
                json!({"toolCallRef":"fixture-call","reason":"Owner stop"}),
            ),
            (
                "rerun_check",
                json!({"checkIndex":0,"checkName":"fixture","digest":"fixture","resultVersion":"1"}),
            ),
            (
                "task_settings",
                json!({"settings":{"model":"fixture-model","effort":"","mode":"unsupported","language":"auto"}}),
            ),
        ] {
            let mut request = payload;
            let fields = request.as_object_mut().unwrap();
            fields.insert("projectId".into(), json!(id));
            fields.insert("executionId".into(), json!(execution));
            fields.insert("requestId".into(), json!(format!("unsupported-{action}")));
            fields.insert("action".into(), json!(action));
            assert_eq!(
                service
                    .control_execution(request)
                    .await
                    .unwrap_err()
                    .to_string(),
                match action {
                    "worker_message" | "worker_stop" => "native_worker_not_live",
                    "command_stop" => "native_command_not_live",
                    "rerun_check" => "native_check_still_running",
                    "task_settings" => "native_access_mode_unavailable",
                    _ => unreachable!(),
                }
            );
        }
        assert_eq!(
            store
                .read(|db| Ok(db
                    .query_row("SELECT count(*) FROM runtime_requests", [], |r| r
                        .get::<_, i64>(0))?))
                .unwrap(),
            before
        );
        std::fs::write(f.0.join("project/scenario.txt"), "complete").unwrap();
        service
            .prompt_execution(
                id,
                execution,
                "Continue fixture",
                Some("continue-fixture"),
                json!([]),
            )
            .await
            .unwrap();
    } else if action == "answer" {
        let ask = json!({"protocol":{"major":1,"minor":8},"projectId":id,"requestId":"questions","operation":"decisions.list","target":execution,"cursor":null,"limit":100});
        let page = service.workspace(ask.clone()).await.unwrap();
        workspace_protocol::validate("DecisionPage", &page).unwrap();
        let d = &page["items"][0];
        assert_eq!(d["status"], "pending");
        assert_eq!(d["allowedOperations"], json!(["decisions.answer"]));
        let mut answer = command(
            id,
            "decisions.answer",
            d["decisionId"].clone(),
            "answer",
            json!({"executionId":execution,"acpSessionId":d["acpSessionId"],"batchId":null,"answers":[{
                "decisionId":d["decisionId"],"questionId":d["questionId"],"expectedVersion":d["version"],"optionId":null,"text":"custom free text"
            }]}),
        );
        answer["expectedVersion"] = d["version"].clone();
        answer["ownerEpoch"] = d["ownerEpoch"].clone();
        let mut wrong_epoch = answer.clone();
        wrong_epoch["ownerEpoch"] = json!("999999");
        assert_eq!(
            service
                .workspace(wrong_epoch)
                .await
                .unwrap_err()
                .to_string(),
            "stale_epoch"
        );
        let mut stale = answer.clone();
        stale["expectedVersion"] = json!("999999");
        assert_eq!(
            service.workspace(stale).await.unwrap_err().to_string(),
            "stale_version"
        );
        let receipt = service.workspace(answer.clone()).await.unwrap();
        assert_eq!(service.workspace(answer.clone()).await.unwrap(), receipt);
        answer["payload"]["answers"][0]["text"] = json!("changed");
        assert_eq!(
            service.workspace(answer).await.unwrap_err().to_string(),
            "payload_conflict"
        );
    } else if action == "pause_resume_question" {
        // Pause preserves an open worker or coordinator question, and Resume
        // continues the retained task: the same question stays open and
        // answerable, and nothing is interrupted, cancelled or replanned.
        let ask = json!({"protocol":{"major":1,"minor":8},"projectId":id,"requestId":"questions","operation":"decisions.list","target":execution,"cursor":null,"limit":100});
        let open = |page: &Value| -> Value {
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["status"] == "pending")
                .cloned()
                .unwrap_or(Value::Null)
        };
        let before = open(&service.workspace(ask.clone()).await.unwrap());
        assert!(before.is_object(), "no pending question before Pause");
        assert_eq!(
            before["scope"] == "task",
            scenario == "reconcile-question",
            "the {scenario} fixture asked the wrong kind of question"
        );
        let interrupts = |store: &cortex_core::Store| {
            store.read(|db| Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE type IN ('TURN_INTERRUPTED','REPLAN_REQUEST_ACCEPTED','RETURNED_TO_COORDINATOR')",
            [], |r| r.get::<_, i64>(0))?)).unwrap()
        };
        let interrupted_before = interrupts(&store);
        let pause = json!({"projectId":id,"executionId":execution,"requestId":"pause-keep","action":"pause","expectedVersion":version});
        service.control_execution(pause).await.unwrap();
        let paused = read_page(&service, query.clone()).await;
        assert_eq!(
            paused["data"]["value"]["items"][0]["runtimeState"],
            "PAUSED"
        );
        // The preserved question is visible but cannot be answered while paused.
        let held = service.workspace(ask.clone()).await.unwrap();
        let held = held["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["questionId"] == before["questionId"])
            .unwrap()
            .clone();
        assert_eq!(held["status"], "pending");
        assert_eq!(held["allowedOperations"], json!([]));
        std::fs::write(f.0.join("project/scenario.txt"), "complete").unwrap();
        let resume = json!({"projectId":id,"executionId":execution,"requestId":"resume-keep","action":"resume","expectedVersion":paused["data"]["value"]["items"][0]["version"]});
        service.control_execution(resume).await.unwrap();
        assert_eq!(
            interrupts(&store),
            interrupted_before,
            "Resume must not interrupt or replan"
        );
        let after = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let page = service.workspace(ask.clone()).await.unwrap();
                let d = open(&page);
                if d.is_object() && d["allowedOperations"] == json!(["decisions.answer"]) {
                    break d;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the preserved question did not stay answerable after Resume");
        assert_eq!(
            after["questionId"], before["questionId"],
            "a different question appeared"
        );
        assert_eq!(
            store
                .read(|db| Ok(db.query_row(
                    "SELECT status FROM pending_questions WHERE id=?",
                    [after["questionId"].as_str().unwrap()],
                    |r| r.get::<_, String>(0)
                )?))
                .unwrap(),
            "open"
        );
        let mut answer = command(
            id,
            "decisions.answer",
            after["decisionId"].clone(),
            "answer-after-resume",
            json!({"executionId":execution,"acpSessionId":after["acpSessionId"],"batchId":null,"answers":[{
                "decisionId":after["decisionId"],"questionId":after["questionId"],"expectedVersion":after["version"],
                "optionId":after["options"][0]["optionId"],"text":after["options"][0]["label"]}]}),
        );
        answer["expectedVersion"] = after["version"].clone();
        answer["ownerEpoch"] = after["ownerEpoch"].clone();
        service.workspace(answer).await.unwrap();
    } else if action == "answer_batch" {
        // The desktop sends one `decisions.answer` carrying every batch member. Every batch
        // question the fixture asks (worker and later reviewer) is answered that way until the
        // task ends; no answer may leave a worker orphaned or cancelled.
        let ask = json!({"protocol":{"major":1,"minor":8},"projectId":id,"requestId":"questions","operation":"decisions.list","target":execution,"cursor":null,"limit":100});
        let mut answered = 0;
        tokio::time::timeout(Duration::from_secs(25), async {
            loop {
                let (state, failed): (String, bool) = store.read(|db| Ok((
                    db.query_row("SELECT state FROM runtime_tasks", [], |r| r.get(0))?,
                    db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type IN ('WORKER_FAILED','NATIVE_DRIVER_STOPPED') OR (type='WORKER_CONTINUATION_SETTLED' AND payload_json LIKE '%unknown%'))", [], |r| r.get(0))?))).unwrap();
                if failed {
                    let events: Vec<String> = store.read(|db| Ok(db.prepare("SELECT type||' '||payload_json FROM runtime_events WHERE type IN ('WORKER_FAILED','NATIVE_DRIVER_STOPPED','ASSIGNMENT_RETRIED')")?
                        .query_map([], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)).unwrap();
                    panic!("worker was failed/stopped after a batch answer: {events:?}");
                }
                if state == "COMPLETED" { break; }
                let page = service.workspace(ask.clone()).await.unwrap();
                let pending: Vec<&Value> = page["items"].as_array().unwrap().iter().filter(|d| d["status"] == "pending").collect();
                if pending.len() == 2 {
                    let batch = pending[0]["batchId"].clone();
                    assert!(batch.is_string() && pending.iter().all(|d| d["batchId"] == batch));
                    let answers: Vec<Value> = pending.iter().map(|d| json!({
                        "decisionId":d["decisionId"],"questionId":d["questionId"],"expectedVersion":d["version"],
                        "optionId":d["options"][0]["optionId"],"text":d["options"][0]["label"]})).collect();
                    answered += 1;
                    let mut answer = command(id, "decisions.answer", batch.clone(), &format!("answer-batch-{answered}"),
                        json!({"executionId":execution,"acpSessionId":pending[0]["acpSessionId"],"batchId":batch,"answers":answers}));
                    answer["expectedVersion"] = pending[0]["version"].clone();
                    answer["ownerEpoch"] = pending[0]["ownerEpoch"].clone();
                    service.workspace(answer).await.unwrap();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.expect("batch answers did not carry the task to completion");
        assert!(answered >= 1);
        println!("native desktop batch answers: {answered} batch(es) delivered, task completed");
    } else if action == "pause" || action == "stop" {
        let pause = json!({"projectId":id,"executionId":execution,"requestId":"pause","action":"pause","expectedVersion":version});
        if action == "stop" {
            service.cancel_execution(id, execution).await.unwrap();
        } else {
            let receipt = service.control_execution(pause.clone()).await.unwrap();
            assert_eq!(service.control_execution(pause).await.unwrap(), receipt);
        }
        let paused = read_page(&service, query.clone()).await;
        assert_eq!(
            paused["data"]["value"]["items"][0]["runtimeState"],
            "PAUSED"
        );
        assert_eq!(
            store
                .read(|db| Ok(db.query_row(
                    "SELECT count(*) FROM capacity_reservations WHERE state!='released'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?))
                .unwrap(),
            0
        );
        std::fs::write(f.0.join("project/scenario.txt"), "complete").unwrap();
        let resume = json!({"projectId":id,"executionId":execution,"requestId":"resume","action":"resume","expectedVersion":paused["data"]["value"]["items"][0]["version"]});
        let mut stale = resume.clone();
        stale["expectedVersion"] = json!("1");
        assert_eq!(
            service
                .control_execution(stale)
                .await
                .unwrap_err()
                .to_string(),
            "stale_version"
        );
        service.control_execution(resume).await.unwrap();
    } else if action == "rolling" {
        // The independent second worker remains held while the first worker
        // releases its slot. A dependent third node must be provisioned before
        // the held sibling is released, proving rolling replenishment.
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let ready: bool = store.read(|db| Ok(db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events
                         WHERE type='NATIVE_ROUTE_SELECTED' AND json_extract(payload_json,'$.node')='third')
                     AND EXISTS(SELECT 1 FROM runtime_nodes WHERE node_id='third' AND status='completed')
                     AND EXISTS(SELECT 1 FROM runtime_nodes WHERE node_id='second' AND status='running')",
                    [], |r| r.get(0))?)).unwrap();
                if ready { break; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("dependent node did not start while sibling was held");
        std::fs::write(f.0.join("project/scenario.txt"), "complete").unwrap();
    } else if action == "prompt" {
        // Sent while the worker still runs: it keeps running, nothing is interrupted,
        // and the task stays EXECUTING. The coordinator receives the message once, after
        // the worker, review and verification finished.
        let worker: String = store.read(|db| Ok(db.query_row(
            "SELECT attempt_id FROM delegation_provisioning WHERE role_id!='orchestrator' AND status='ready'", [], |r| r.get(0))?)).unwrap();
        let receipt = service
            .prompt_execution(
                id,
                execution,
                "Use the other output",
                Some("steer"),
                json!([]),
            )
            .await
            .unwrap();
        assert_eq!(receipt["native"], true);
        assert_eq!(
            service
                .prompt_execution(
                    id,
                    execution,
                    "Use the other output",
                    Some("steer"),
                    json!([])
                )
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(
            store
                .read(|db| Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?))
                .unwrap(),
            1
        );
        let running = |store: &cortex_core::Store| {
            store.read(|db| Ok(db.query_row(
            "SELECT (SELECT state FROM runtime_tasks)||':'||(SELECT status FROM delegation_provisioning WHERE attempt_id=?1)
             ||':'||(SELECT count(*) FROM runtime_nodes WHERE status='running')
             ||':'||(SELECT count(*) FROM runtime_events WHERE type IN ('TURN_INTERRUPTED','LIVE_ASSIGNMENTS_CANCELLED','STEERING_APPLIED'))",
            [&worker], |r| r.get::<_, String>(0))?)).unwrap()
        };
        assert_eq!(
            running(&store),
            "EXECUTING:ready:1:0",
            "the message disturbed the running worker"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            running(&store),
            "EXECUTING:ready:1:0",
            "the message disturbed the running worker"
        );
        // The worker finishes its own turn.
        std::fs::write(f.0.join("project/scenario.txt"), "complete").unwrap();
    } else {
        std::fs::write(f.0.join("project/scenario.txt"), "complete").unwrap();
        let request = json!({"projectId":id,"executionId":execution,"requestId":"control","action":action,"confirmed":true,"expectedVersion":version});
        if action == "mark_resolved" {
            let mut stale = request.clone();
            stale["expectedVersion"] = json!("1");
            assert_eq!(
                service
                    .control_execution(stale)
                    .await
                    .unwrap_err()
                    .to_string(),
                "stale_version"
            );
            let mut unconfirmed = request.clone();
            unconfirmed["confirmed"] = json!(false);
            assert_eq!(
                service
                    .control_execution(unconfirmed)
                    .await
                    .unwrap_err()
                    .to_string(),
                "confirmation_required"
            );
        }
        match service.control_execution(request.clone()).await {
            // No-stop policy: the stopped crash loop is a continuation that the runtime
            // may already have relaunched by itself, so an explicit Retry can find the
            // task running again (and the fixed scenario then completes it).
            Err(error) if action == "retry" && error.to_string() == "retry_not_expected" => {}
            receipt => {
                let receipt = receipt.unwrap();
                assert_eq!(service.control_execution(request).await.unwrap(), receipt);
            }
        }
    }
    let terminal = if action == "mark_resolved" {
        "CANCELLED"
    } else {
        "COMPLETED"
    };
    tokio::time::timeout(Duration::from_secs(15),async {
        loop {
            let page=read_page(&service,query.clone()).await;
            if page["data"]["value"]["items"][0]["runtimeState"]==terminal {
                if action=="mark_resolved" {assert!(page["data"]["value"]["items"][0]["warnings"].as_array().unwrap().contains(&json!("owner_resolved_without_certification")));}
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap_or_else(|_| {
        let diagnostics=store.read(|db|Ok(db.query_row(
            "SELECT (SELECT state FROM runtime_tasks)||' '||coalesce((SELECT group_concat(type,',') FROM (SELECT type FROM runtime_events ORDER BY id DESC LIMIT 40)),'')
              ||' capsules='||coalesce((SELECT group_concat(recipient||':'||state||':'||updated_at,',') FROM capsule_deliveries),'')||' handed='||coalesce((SELECT group_concat(created_at,',') FROM runtime_events WHERE type='STEERING_HANDED'),'')
             ||' '||coalesce(group_concat(type||':'||payload_json),'') FROM runtime_events WHERE type IN ('NATIVE_DRIVER_STOPPED','NATIVE_SESSION_START_FAILED','ASSIGNMENT_RETRIED','WORKER_RESULT_REFUSED')",[],|r|r.get::<_,String>(0))?)).unwrap();
        panic!("native control did not reach terminal state: {diagnostics}");
    });
    if action == "retry" {
        assert!(store.read(|db|Ok(db.query_row("SELECT count(*) FROM delegation_provisioning WHERE role_id='worker-backend-dev'",[],|r|r.get::<_,i64>(0))?)).unwrap()>=2);
    }
    if action == "prompt" {
        // The held worker's own result was kept (revision 1 completed), the message was
        // applied after it and handed to exactly one coordinator turn, and nothing was
        // interrupted on the way.
        let (completed, applied, delivered, interrupted): (i64, i64, i64, i64) = store.read(|db| Ok(db.query_row(
            "SELECT (SELECT count(*) FROM runtime_nodes WHERE revision=1 AND stage='execution' AND status='completed'),
                    (SELECT count(*) FROM runtime_events WHERE type='STEERING_APPLIED'),
                    (SELECT count(*) FROM runtime_events WHERE type='STEERING_DELIVERED'),
                    (SELECT count(*) FROM runtime_events WHERE type='TURN_INTERRUPTED')",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?)).unwrap();
        assert_eq!((completed, applied, delivered, interrupted), (1, 1, 1, 0));
        // The fixture coordinator answers with the first message its prompt listed.
        let (answers, synthesis): (i64, String) = store.read(|db| Ok(db.query_row(
            "SELECT (SELECT count(*) FROM runtime_events WHERE type='ANSWER_SUBMITTED'),(SELECT coalesce(synthesis,'') FROM runtime_tasks)",
            [], |r| Ok((r.get(0)?, r.get(1)?)))?)).unwrap();
        assert_eq!(answers, 1);
        assert!(
            synthesis.contains("Owner guidance applied (Use the other output)"),
            "{synthesis}"
        );
    }
    if action == "answer" {
        store.write(|db| {
            let waits=cortex_core::lifecycle::waits::projection_tx(db,&db.query_row("SELECT task_id FROM runtime_tasks",[],|r|r.get::<_,String>(0))?)?;
            assert_eq!(waits[0]["status"],"resumed");
            assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning WHERE role_id='worker-backend-dev'",[],|r|r.get::<_,i64>(0))?,1);
            Ok(())
        }).unwrap();
    }
    let delta = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = service.receive_event(&mut events).await.unwrap();
            if event["type"] == "native_delta"
                && event["batch"]["changes"].as_array().is_some_and(|cs| {
                    cs.iter()
                        .any(|c| c["operation"]["data"]["data"]["runtimeState"] == terminal)
                })
            {
                break event;
            }
        }
    })
    .await
    .expect("control terminal UI delta missing");
    assert_eq!(delta["batch"]["generation"], initial["generation"]);
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
async fn positive_owner_control(action: &str) {
    let f = Fixture::new();
    std::fs::write(f.0.join("project/other.txt"), "before").unwrap();
    let scenario = match action {
        "worker_message" => "guidance",
        "worker_stop" => "stop-siblings",
        "task_settings" => "hold",
        "command_stop" => "slow-check",
        _ => "complete",
    };
    std::fs::write(f.0.join("project/scenario.txt"), scenario).unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Owner controls")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap();
    service.activate(id).await.unwrap();
    let query = json!({"protocol":{"major":2,"mapping":2},"requestId":"page","projectId":id,"operation":"tasks.page","payload":{"target":id,"cursor":null,"limit":50}});
    let initial = read_page(&service, query.clone()).await;
    let mut events = service.subscribe();
    let runner = tokio::spawn(service.clone().serve());
    let created = service
        .workspace(command(
            id,
            "tasks.create",
            json!(id),
            "create",
            json!({"description":"Positive native controls","checkout":{"kind":"main"}}),
        ))
        .await
        .unwrap();
    let started = service
        .workspace(command(
            id,
            "tasks.start",
            created["target"].clone(),
            "start",
            json!({}),
        ))
        .await
        .unwrap();
    let execution = started["target"].as_str().unwrap();
    let store = f.store();
    let checkpoint = match action {
        "rerun_check" => "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE state='COMPLETED')",
        "command_stop" => {
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='NATIVE_COMMAND_STARTED' AND json_extract(payload_json,'$.argv[0]')='/usr/bin/cmp')"
        }
        _ => {
            "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE role_id='worker-backend-dev' AND status='ready')"
        }
    };
    async fn wait_for(store: &Store, sql: &str) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if store
                    .read(|db| Ok(db.query_row(sql, [], |r| r.get::<_, bool>(0))?))
                    .unwrap()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("checkpoint missing: {sql}"));
    }
    wait_for(&store, checkpoint).await;
    async fn facts(service: &Service, id: &str) -> Value {
        let call = serde_json::from_value(json!({"op":"product.facts","projectId":id,"params":{}}))
            .unwrap();
        let value = service.machine(call).await.unwrap();
        assert_eq!(value["ok"], true);
        value["result"].clone()
    }
    let before = facts(&service, id).await;
    let worker = before["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["role"] == "backend-dev")
        .unwrap()["id"]
        .clone();
    let mut payload = match action {
        "worker_message" => {
            json!({"workerId":worker,"text":"Use the owner output","attachments":[]})
        }
        "worker_stop" => {
            // Workers of one batch run concurrently: the sibling commits its result while
            // this worker's turn is still held.
            wait_for(&store,"SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE node_id='second' AND status='completed')
                AND EXISTS(SELECT 1 FROM runtime_nodes WHERE node_id='work' AND status='running')").await;
            // The page's own shape: the worker id it lists, and no reason (the dialog's
            // reason is optional).
            json!({"workerId":worker})
        }
        "task_settings" => {
            json!({"settings":{"model":"fixture-other","effort":"","mode":"guarded","language":"ru"}})
        }
        "command_stop" => {
            let reference:String=store.read(|db|Ok(db.query_row("SELECT json_extract(payload_json,'$.ref') FROM runtime_events WHERE type='NATIVE_COMMAND_STARTED' AND json_extract(payload_json,'$.argv[0]')='/usr/bin/cmp' ORDER BY id DESC LIMIT 1",[],|r|r.get(0))?)).unwrap();
            // The exact owned gateway ref is visible as a stoppable command, not native activity.
            assert!(before["workers"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|w| w["activity"].as_array().unwrap())
                .any(|a| a["toolCallRef"] == reference && a["command"]["state"] == "running"));
            json!({"toolCallRef":reference,"reason":"Stop the bounded verifier"})
        }
        "rerun_check" => {
            let r = &before["product"]["tasks"][created["target"].as_str().unwrap()]["result"];
            assert_eq!(r["checks"][0]["outcome"], "passed");
            json!({"checkIndex":0,"checkName":r["checks"][0]["name"],"digest":r["digest"],"resultVersion":r["sourceVersion"]})
        }
        _ => unreachable!(),
    };
    payload["action"] = json!(action);
    payload["projectId"] = json!(id);
    payload["executionId"] = json!(execution);
    payload["requestId"] = json!("owner-positive");
    // Foreign/stale targets cannot reach effects.
    let mut foreign = payload.clone();
    foreign["executionId"] = json!("not-this-execution");
    assert_eq!(
        service
            .control_execution(foreign)
            .await
            .unwrap_err()
            .to_string(),
        "native_execution_binding_invalid"
    );
    let receipt = service.control_execution(payload.clone()).await.unwrap();
    assert_eq!(receipt["outcome"], "recorded");
    assert_eq!(receipt["native"], true);
    assert_eq!(
        service.control_execution(payload.clone()).await.unwrap(),
        receipt
    );
    let mut conflict = payload.clone();
    conflict["extra"] = json!(true);
    assert_eq!(
        service
            .control_execution(conflict)
            .await
            .unwrap_err()
            .to_string(),
        "payload_conflict"
    );
    match action {
        "worker_message" => {
            wait_for(&store,"SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='NATIVE_GUIDANCE_SETTLED' AND json_extract(payload_json,'$.delivered')=1)").await;
            store.read(|db|{
                assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning WHERE role_id='worker-backend-dev'",[],|r|r.get::<_,i64>(0))?,1);
                assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM host_transcript WHERE text='Guidance observed in same worker session.')",[],|r|r.get::<_,bool>(0))?);
                Ok(())
            }).unwrap();
            let f = facts(&service, id).await;
            assert!(f["product"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["text"] == "Use the owner output" && m["delivery"] == "delivered"));
        }
        "worker_stop" => {
            // The stop receipt covers this worker, not the independently running
            // sibling. Its owned wait must already be retained without polling.
            store
                .read(|db| {
                    let stopped: (String, String, String) = db.query_row(
                        "SELECT p.status,r.state,r.release_observation
                     FROM delegation_provisioning p JOIN capacity_reservations r USING(attempt_id)
                     WHERE p.idempotency_key=?",
                        [worker.as_str().unwrap()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )?;
                    assert_eq!(
                        stopped,
                        (
                            "terminated".into(),
                            "released".into(),
                            "process_exited".into()
                        )
                    );
                    Ok(())
                })
                .unwrap();
            wait_for(&store,"SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE node_id='second' AND status='completed')").await;
            // The fake peer holds its ACP reply after committing its result,
            // but result_submit revokes its channel. The authority monitor may
            // already have reaped it before this read. Require the owned exit
            // observation if released, never infer absence from node completion
            // or race this assertion against the monitor's 50 ms polling period.
            store.read(|db| {
                let sibling: (String, String, Option<String>) = db.query_row(
                    "SELECT p.status,r.state,r.release_observation FROM delegation_provisioning p
                     JOIN capacity_reservations r USING(attempt_id)
                     JOIN runtime_nodes n ON n.assignment_ref=p.idempotency_key WHERE n.node_id='second'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                match (sibling.0.as_str(),sibling.1.as_str(),sibling.2.as_deref()) {
                    ("ready","reserved",None) | ("terminated","released",Some("process_exited")) => {},
                    _ => panic!("sibling capacity lacks a consistent owned-process observation: {sibling:?}"),
                }
                Ok(())
            }).unwrap();
            std::fs::write(f.0.join("project/release-sibling"), "").unwrap();
            wait_for(&store, "SELECT EXISTS(SELECT 1 FROM delegation_provisioning p
                JOIN capacity_reservations r USING(attempt_id)
                JOIN runtime_nodes n ON n.assignment_ref=p.idempotency_key WHERE n.node_id='second'
                AND p.status='terminated' AND r.state='released' AND r.release_observation='process_exited')").await;
            assert_eq!(
                std::fs::read_to_string(f.0.join("project/other.txt")).unwrap(),
                "after"
            );
            assert_eq!(
                std::fs::read_to_string(f.0.join("project/work.txt")).unwrap(),
                "before"
            );
            store.read(|db|{
                assert_eq!(db.query_row("SELECT count(*) FROM capacity_reservations WHERE state!='released'",[],|r|r.get::<_,i64>(0))?,0);
                assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning WHERE status!='terminated'",[],|r|r.get::<_,i64>(0))?,0);
                assert_eq!(db.query_row("SELECT count(*) FROM runtime_nodes WHERE status='running'",[],|r|r.get::<_,i64>(0))?,0);
                Ok(())
            }).unwrap();
            let f = facts(&service, id).await;
            assert!(f["workers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["id"] == worker && w["status"] == "cancelled"));
            // The stopped worker is not a dead end: once the sibling finished and
            // nothing else can run, the coordinator is asked to replan or finish,
            // not stopped on an internal code.
            wait_for(&store,"SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='RETURNED_TO_COORDINATOR' AND json_extract(payload_json,'$.reason')='worker_stopped_by_owner')").await;
            store.read(|db|{
                assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_DRIVER_STOPPED' AND payload_json LIKE '%native_continuation_unavailable%'",[],|r|r.get::<_,i64>(0))?,0);
                assert_eq!(db.query_row("SELECT count(*) FROM pending_questions WHERE text LIKE '%native_continuation_unavailable%'",[],|r|r.get::<_,i64>(0))?,0);
                Ok(())
            }).unwrap();
        }
        "command_stop" => {
            let f = facts(&service, id).await;
            let r = &f["product"]["tasks"][created["target"].as_str().unwrap()]["result"];
            assert_eq!(r["checks"][0]["outcome"], "failed");
            assert_eq!(r["checks"][0]["detail"], "owner_stopped");
            // A check rerun holds the ordinary control lane; stopping its exact
            // host command must still progress on the independent stop lane.
            wait_for(
                &store,
                "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE state='WAITING_USER')",
            )
            .await;
            let rerun = json!({"projectId":id,"executionId":execution,"requestId":"rerun-stopped","action":"rerun_check",
                "checkIndex":0,"checkName":r["checks"][0]["name"],"digest":r["digest"],"resultVersion":r["sourceVersion"]});
            let running_service = service.clone();
            let running =
                tokio::spawn(async move { running_service.control_execution(rerun).await });
            wait_for(
                &store,
                "SELECT count(*)=2 FROM runtime_events WHERE type='NATIVE_COMMAND_STARTED'",
            )
            .await;
            let reference:String=store.read(|db|Ok(db.query_row("SELECT json_extract(payload_json,'$.ref') FROM runtime_events WHERE type='NATIVE_COMMAND_STARTED' ORDER BY id DESC LIMIT 1",[],|r|r.get(0))?)).unwrap();
            service.control_execution(json!({"projectId":id,"executionId":execution,"requestId":"stop-rerun","action":"command_stop","toolCallRef":reference,"reason":"Stop owner rerun"})).await.unwrap();
            let rerun_receipt = running.await.unwrap().unwrap();
            assert_eq!(rerun_receipt["effect"]["observation"]["complete"], false);
            assert_eq!(
                rerun_receipt["effect"]["observation"]["failure"],
                "owner_stopped"
            );
            assert!(f["workers"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|w| w["activity"].as_array().unwrap())
                .any(|a| a["command"]["state"] == "stopped"));
            assert_eq!(
                store
                    .read(|db| Ok(db.query_row(
                        "SELECT count(*) FROM runtime_tasks WHERE state='COMPLETED'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?))
                    .unwrap(),
                0
            );
        }
        "rerun_check" => {
            let f = facts(&service, id).await;
            let old = &before["product"]["tasks"][created["target"].as_str().unwrap()]["result"];
            let new = &f["product"]["tasks"][created["target"].as_str().unwrap()]["result"];
            assert_ne!(old["sourceVersion"], new["sourceVersion"]);
            assert_eq!(new["checks"][0]["outcome"], "passed");
            assert_eq!(
                store
                    .read(|db| Ok(db.query_row(
                        "SELECT count(*) FROM runtime_events WHERE type='NATIVE_COMMAND_FINISHED'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?))
                    .unwrap(),
                2
            );
            let mut stale = payload.clone();
            stale["requestId"] = json!("stale-rerun");
            assert_eq!(
                service
                    .control_execution(stale)
                    .await
                    .unwrap_err()
                    .to_string(),
                "stale_result"
            );
        }
        "task_settings" => {
            let f = facts(&service, id).await;
            assert_eq!(
                f["product"]["tasks"][created["target"].as_str().unwrap()]["settings"],
                payload["settings"]
            );
        }
        _ => unreachable!(),
    }
    if action == "task_settings" {
        std::fs::write(f.0.join("project/scenario.txt"), "settings-ru").unwrap();
        service
            .prompt_execution(
                id,
                execution,
                "Continue with selected settings",
                Some("settings-continue"),
                json!([]),
            )
            .await
            .unwrap();
        wait_for(&store,"SELECT EXISTS(SELECT 1 FROM host_transcript WHERE text='Observed owner task model and language.')").await;
    }
    let page = read_page(&service, query.clone()).await;
    let version = page["data"]["value"]["items"][0]["version"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = service.receive_event(&mut events).await.unwrap();
            if event["type"] == "native_delta"
                && event["batch"]["changes"].as_array().is_some_and(|cs| {
                    cs.iter().any(|c| {
                        c["id"] == created["target"]
                            && c["operation"]["data"]["data"]["version"]
                                .as_str()
                                .and_then(|s| s.parse::<u64>().ok())
                                .is_some_and(|v| v >= version)
                    })
                })
            {
                assert_eq!(event["batch"]["generation"], initial["generation"]);
                break;
            }
        }
    })
    .await
    .expect("positive owner effect must publish a current task delta");
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
/// A task saved in Russian: every worker and reviewer session receives the same
/// owner-language line as the coordinator. (Workers still narrate in English by their
/// generated profile protocol; this guards only the runtime delivery.)
async fn worker_language() {
    let f = Fixture::new();
    std::fs::write(f.0.join("project/scenario.txt"), "language-ru").unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Language")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap();
    service.activate(id).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    let created = service
        .workspace(command(
            id,
            "tasks.create",
            json!(id),
            "create",
            json!({"description":"Russian task","checkout":{"kind":"main"},
                "settings":{"model":"fixture-model","effort":"","mode":"guarded","language":"ru"}}),
        ))
        .await
        .unwrap();
    service
        .workspace(command(
            id,
            "tasks.start",
            created["target"].clone(),
            "start",
            json!({}),
        ))
        .await
        .unwrap();
    let store = f.store();
    let observed = || {
        store
            .read(|db| {
                let mut q = db.prepare(
                    "SELECT text FROM host_transcript WHERE text LIKE '%language observed:%' ORDER BY id",
                )?;
                let rows = q
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap()
    };
    tokio::time::timeout(Duration::from_secs(25), async {
        while !observed().iter().any(|t| t.starts_with("desktop-reviewer")) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no reviewer session was observed: {:?}", observed()));
    let seen = observed();
    assert!(
        seen.iter().any(|t| t.starts_with("desktop-worker")),
        "{seen:?}"
    );
    assert!(
        seen.iter().all(|t| t.ends_with("language observed: true.")),
        "a worker or reviewer session lacked the owner's language: {seen:?}"
    );
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
/// Workers routed to another engine and model than their coordinator start on exactly
/// that route, and the desktop's worker facts carry that route, never the coordinator's.
async fn worker_route() {
    let f = Fixture::new();
    std::fs::write(f.0.join("project/scenario.txt"), "language-ru").unwrap();
    std::fs::write(f.0.join("project/worker-route.txt"), "").unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Routes")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap();
    service.activate(id).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    let created = service
        .workspace(command(
            id,
            "tasks.create",
            json!(id),
            "create",
            json!({"description":"Routed task","checkout":{"kind":"main"},
                "settings":{"model":"fixture-model","effort":"","mode":"guarded","language":"ru"}}),
        ))
        .await
        .unwrap();
    service
        .workspace(command(
            id,
            "tasks.start",
            created["target"].clone(),
            "start",
            json!({}),
        ))
        .await
        .unwrap();
    let store = f.store();
    // Every configured session: its principal, assignment and the engine and model the
    // engine confirmed in `configure` (the reply must echo the exact value sent).
    let configured = || {
        store.read(|db| {
            let mut q = db.prepare("SELECT c.role,coalesce(c.assignment_id,''),json_extract(e.payload_json,'$.engine'),json_extract(e.payload_json,'$.model')
                FROM runtime_events e JOIN trusted_channels c ON c.channel_id=json_extract(e.payload_json,'$.channel_id')
                WHERE e.type='NATIVE_SESSION_CONFIGURED' ORDER BY e.id")?;
            let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }).unwrap()
    };
    let roles = |role: &str| {
        store.read(|db| Ok(db.query_row(
        "SELECT count(*) FROM delegation_provisioning WHERE role_id=? AND engine_session_ref IS NOT NULL", [role], |r| r.get::<_, i64>(0))?)).unwrap()
    };
    tokio::time::timeout(Duration::from_secs(25), async {
        while roles("worker-backend-dev") == 0 || roles("worker-code-reviewer") == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "no worker and reviewer sessions were configured: {:?}",
            configured()
        )
    });
    let sessions = configured();
    let coordinators: Vec<_> = sessions.iter().filter(|s| s.0 == "coordinator").collect();
    let workers: Vec<_> = sessions.iter().filter(|s| s.0 == "worker").collect();
    assert!(
        !coordinators.is_empty() && workers.len() >= 2,
        "{sessions:?}"
    );
    assert!(
        coordinators
            .iter()
            .all(|s| s.2 == "codex-acp" && s.3 == "fixture-model"),
        "{sessions:?}"
    );
    assert!(
        workers
            .iter()
            .all(|s| s.2 == "claude-agent-acp" && s.3 == "fixture-other"),
        "{sessions:?}"
    );
    // The route routing chose for each worker is the one its session was configured with.
    let selected = store.read(|db| {
        let mut q = db.prepare("SELECT json_extract(payload_json,'$.assignment_ref'),json_extract(payload_json,'$.engine'),json_extract(payload_json,'$.model')
            FROM runtime_events WHERE type='NATIVE_ROUTE_SELECTED' ORDER BY id")?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }).unwrap();
    for w in &workers {
        let chosen = selected
            .iter()
            .rfind(|s| s.0 == w.1)
            .unwrap_or_else(|| panic!("no route selected for {w:?}: {selected:?}"));
        assert_eq!(
            (chosen.1.as_str(), chosen.2.as_str()),
            (w.2.as_str(), w.3.as_str())
        );
    }
    // The desktop's worker facts: each worker shows its own configured route.
    let facts = machine(&service, "product.facts", id, json!({})).await;
    let shown = facts["workers"].as_array().unwrap();
    for role in ["backend-dev", "code-reviewer"] {
        let w = shown
            .iter()
            .find(|w| w["role"] == role)
            .unwrap_or_else(|| panic!("no {role} worker: {shown:?}"));
        assert_eq!(
            w["route"],
            json!({"engine":"claude-agent-acp","model":"fixture-other","effort":"","modelLabel":"Other fixture"}),
            "{role}: {w}"
        );
        assert!(
            workers.iter().any(|s| json!(s.1) == w["id"]),
            "{role} has no configured session of its own"
        );
    }
    service.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
async fn checkpoint_recovery(stage: usize, legacy: bool) {
    use workspace_service::native_sessions::{NativeHost, SessionRole};
    let f = Fixture::new();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Checkpoint")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap();
    let key = cortex_core::ProjectKey::new(
        f.0.join("project"),
        if workspace_protocol::channel::current().is_dev() {
            cortex_core::StoreChannel::Dev
        } else {
            cortex_core::StoreChannel::Release
        },
    )
    .unwrap();
    let host = tokio::task::spawn_blocking(move || NativeHost::open(key))
        .await
        .unwrap()
        .unwrap();
    let owner = if legacy {
        let store = f.store();
        let machine = cortex_core::lifecycle::TaskMachine::new(&store);
        let owner = machine
            .create("checkpoint", "Implement fixture", "python-owner")
            .unwrap();
        machine
            .transition(
                "checkpoint",
                &owner,
                "planning",
                cortex_core::TaskState::Planning,
                None,
                &cortex_core::lifecycle::FoundationGuards,
            )
            .unwrap();
        owner
    } else {
        host.create(
            "checkpoint".into(),
            "Implement fixture".into(),
            "owner".into(),
        )
        .await
        .unwrap()
    };
    if stage != 4 && stage != 5 {
        let coordinator = host
            .dispatch(
                "checkpoint".into(),
                owner.clone(),
                SessionRole::Coordinator,
                Routes.route(&f.0.join("project"), "orchestrator").unwrap(),
            )
            .await
            .unwrap();
        let _ = coordinator.prompt("Implement fixture").await;
        coordinator.shutdown().await.unwrap();
        if stage == 6 {
            std::fs::write(f.0.join("project/scenario.txt"), "question").unwrap();
        }
        for index in 0..if stage == 6 { 1 } else { stage.min(2) } {
            let step = host
                .advance("checkpoint".into(), owner.clone())
                .await
                .unwrap();
            let assignment = &step["assignments"][0];
            let worker = host
                .dispatch(
                    "checkpoint".into(),
                    owner.clone(),
                    SessionRole::Worker {
                        assignment: assignment["assignment_ref"].as_str().unwrap().into(),
                    },
                    Routes
                        .route(&f.0.join("project"), assignment["role"].as_str().unwrap())
                        .unwrap(),
                )
                .await
                .unwrap();
            let _ = worker.prompt(&assignment.to_string()).await;
            worker.shutdown().await.unwrap();
            assert!(index < 2);
        }
    }
    let store = f.store();
    if stage == 6 {
        std::fs::remove_file(f.0.join("project/scenario.txt")).unwrap();
    }
    if stage == 3 {
        store
            .write(|db| cortex_core::lifecycle::review::advance_tx(db, "checkpoint"))
            .unwrap();
    }
    if stage == 5 {
        store.write(|db| {
            db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at)
                VALUES ('q-legacy','checkpoint','legacy-question','Continue?','[\"Yes\",\"No\"]','open','fixture')",[])?;
            cortex_core::lifecycle::event_tx(db,"checkpoint","USER_QUESTION_REQUESTED",
                json!({"question_id":"q-legacy","resume_state":"PLANNING"}))?;
            db.execute("UPDATE runtime_tasks SET state='WAITING_USER' WHERE task_id='checkpoint'",[])?;
            Ok(())
        }).unwrap();
    }
    if stage == 5 || stage == 6 {
        // Existing Python desktop identities survive adoption; only the epoch advances.
        store.write(|db| {
            db.execute("INSERT OR IGNORE INTO workspace_projects VALUES (?,?,?,'fixture',1)",
                (id,f.0.join("project").to_str(),format!("uid:{}",unsafe{libc::getuid()})))?;
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,execution_id,created_at)
                VALUES ('legacy-draft',?,'Fixture','we-legacy','fixture')",[id])?;
            db.execute("INSERT INTO acp_host_sessions VALUES ('as-legacy',?,'checkpoint','{}','{}','[]','fixture',NULL)",
                [f.0.join("project").to_str()])?;
            db.execute("INSERT INTO workspace_executions VALUES ('we-legacy','legacy-draft','checkpoint','as-legacy',?,1,'fixture')",[owner.epoch])?;
            Ok(())
        }).unwrap();
    }
    // The embedding disappeared at a durable, fully settled process boundary.
    tokio::task::spawn_blocking(move || drop(host))
        .await
        .unwrap();
    let activation = service.activate(id).await.unwrap();
    assert_eq!(activation["executionHost"], "rust");
    if legacy {
        store.read(|db| {
            let epoch:i64 = db.query_row("SELECT epoch FROM task_owners WHERE task_id='checkpoint'",[],|r|r.get(0))?;
            assert_eq!(epoch, owner.epoch+1);

            assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_TASK_ADOPTED'",[],|r|r.get::<_,i64>(0))?,1);
            assert!(!db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='TASK_CREATED' AND json_extract(payload_json,'$.execution_host')='rust')",[],|r|r.get::<_,bool>(0))?);
            Ok(())
        }).unwrap();
        store
            .write(|db| {
                assert_eq!(
                    cortex_core::ownership::check_tx(db, "checkpoint", &owner)
                        .unwrap_err()
                        .code(),
                    "stale_owner"
                );
                Ok(())
            })
            .unwrap();
    }
    if stage == 5 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        store
            .read(|db| {
                assert_eq!(
                    db.query_row("SELECT state FROM runtime_tasks", [], |r| r
                        .get::<_, String>(0))?,
                    "WAITING_USER"
                );
                assert_eq!(
                    db.query_row("SELECT count(*) FROM delegation_provisioning", [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
    }
    if stage == 6 {
        store
            .write(|db| {
                let w = cortex_core::lifecycle::waits::projection_tx(db, "checkpoint")?
                    .pop()
                    .unwrap();
                assert_eq!(w["status"], "open");
                assert!(cortex_core::lifecycle::waits::valid_tx(db, &w)?);

                Ok(())
            })
            .unwrap();
    }
    if stage == 5 || stage == 6 {
        let page = service
            .workspace(
                json!({"protocol":{"major":1,"minor":8},"projectId":id,"requestId":"questions",
                "operation":"decisions.list","target":"we-legacy","cursor":null,"limit":100}),
            )
            .await
            .unwrap();
        let d = &page["items"][0];
        assert_eq!(d["status"], "pending");
        assert_eq!(d["ownerEpoch"], (owner.epoch + 1).to_string());
        let mut answer = command(
            id,
            "decisions.answer",
            d["decisionId"].clone(),
            "answer",
            json!({"executionId":"we-legacy","acpSessionId":"as-legacy","batchId":null,"answers":[{
                "decisionId":d["decisionId"],"questionId":d["questionId"],"expectedVersion":d["version"],
                "optionId":d["options"][0]["optionId"],"text":"","attachments":[]}]}),
        );
        answer["expectedVersion"] = d["version"].clone();
        answer["ownerEpoch"] = d["ownerEpoch"].clone();
        service.workspace(answer).await.unwrap();
    }
    restart_signal(
        &store,
        "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE state='COMPLETED')",
    )
    .await;
    store
        .read(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM delegation_provisioning", [], |r| r
                    .get::<_, i64>(0))?,
                if stage == 6 {4} else {3}
            );
            if stage == 6 {
                assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='WORKER_WAIT_CONTINUATION_SCHEDULED')",[],|r|r.get::<_,bool>(0))?);
            }
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='NATIVE_RECOVERY_REQUIRED'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM capacity_reservations WHERE state!='released'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            Ok(())
        })
        .unwrap();
    service.stop().await.unwrap();
    println!(
        "durable checkpoint {stage}: native activation resumed execution/review/verification to completion without replay"
    );
}
async fn restart_signal(store: &Store, sql: &str) {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if store
                .read(|db| Ok(db.query_row(sql, [], |r| r.get::<_, bool>(0))?))
                .unwrap()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("restart signal deadline: {sql}"));
}
/// A completed task the Python host ran is adopted without replay and stays finished.
/// Its questions must still be listed (read-only: a finished task offers no answer),
/// or one such task hides every task's conversation details in the desktop.
async fn completed_legacy_questions_are_readable() {
    let f = Fixture::new();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Legacy")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap().to_owned();
    {
        let store = Store::open(
            cortex_core::ProjectKey::new(
                f.0.join("project"),
                if workspace_protocol::channel::current().is_dev() {
                    cortex_core::StoreChannel::Dev
                } else {
                    cortex_core::StoreChannel::Release
                },
            )
            .unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let machine = cortex_core::lifecycle::TaskMachine::new(&store);
        let owner = machine
            .create("legacy-done", "Old work", "python-owner")
            .unwrap();
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='COMPLETED' WHERE task_id='legacy-done'", [])?;
            db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at)
                VALUES ('q-old','legacy-done','old-question','Keep it?','[\"Yes\",\"No\"]','open','fixture')", [])?;
            db.execute("INSERT OR IGNORE INTO workspace_projects VALUES (?,?,?,'fixture',1)",
                (&id, f.0.join("project").to_str(), format!("uid:{}", unsafe { libc::getuid() })))?;
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,execution_id,created_at)
                VALUES ('legacy-done-draft',?,'Old work','we-old','fixture')", [&id])?;
            db.execute("INSERT INTO acp_host_sessions VALUES ('as-old',?,'legacy-done','{}','{}','[]','fixture',NULL)",
                [f.0.join("project").to_str()])?;
            db.execute("INSERT INTO workspace_executions VALUES ('we-old','legacy-done-draft','legacy-done','as-old',?,1,'fixture')",
                [owner.epoch])?;
            Ok(())
        }).unwrap();
    }
    service.activate(&id).await.unwrap();
    let page = service
        .workspace(
            json!({"protocol":{"major":1,"minor":8},"projectId":id,"requestId":"old-questions",
        "operation":"decisions.list","target":"we-old","cursor":null,"limit":100}),
        )
        .await
        .expect("a completed legacy execution's questions are readable");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{page}");
    assert_eq!(
        items[0]["allowedOperations"],
        json!([]),
        "never answerable here"
    );
    assert_ne!(items[0]["status"], "pending");
    service.stop().await.unwrap();
    println!("completed legacy questions: listed read-only");
}

async fn binary_journey(scenario: &str) {
    use sha2::{Digest, Sha256};
    let hash = |v: &Value| format!("{:x}", Sha256::digest(serde_json::to_vec(v).unwrap()));
    let f = Fixture::new();
    let channel = workspace_protocol::channel::current();
    let config_base = f.0.join("config");
    let state_base = f.0.join("state");
    let config = if channel.is_dev() {
        channel.config_dir(&config_base)
    } else {
        config_base.clone()
    };
    let state = if channel.is_dev() {
        channel.state_dir(&state_base).join("state")
    } else {
        state_base.clone()
    };
    let catalog = state.join("cortex/catalog/cn_fixture");
    for dir in [
        f.0.join("home"),
        f.0.join("bin"),
        f.0.join("runtime"),
        config.join("cortex"),
        catalog.clone(),
    ] {
        std::fs::create_dir_all(&dir).unwrap();
        // Every Cortex-private ancestor must be private.
        for p in dir.ancestors().take_while(|p| p.starts_with(&f.0)) {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let wrapper = f.0.join("bin/fixture-engine");
    let peer = std::env::current_exe().unwrap();
    std::fs::write(
        &wrapper,
        format!("#!/bin/sh\nexec '{}' desktop-auto\n", peer.display()),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let write = |path: PathBuf, v: Value| {
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    };
    // The restart-durability fixture deliberately requires a live-checkout
    // marker and a supervisor that survives service loss. Only the legacy
    // command path still supports that; managed Codex has its own lease test.
    let engine = if scenario.starts_with("command-restart-") {
        "claude-agent-acp"
    } else {
        "codex-acp"
    };
    let mut record = json!({"id":"cn_fixture","engine":engine,"provider":"fixture","endpoint":null,
        "launch_command":["fixture-engine"],"auth_mode":"native_account","qualified":true,"state":"configured",
        "credential_ref":{"kind":"engine_native_dir","path":f.0.join("home"),"owned":false},"max_concurrent_sessions":4});
    record["approved_fingerprint"] = json!(hash(
        &json!({"engine":record["engine"],"provider":record["provider"],
        "endpoint":record["endpoint"],"launch_command":record["launch_command"]})
    ));
    write(
        config.join("cortex/connections.json"),
        json!({"schema_version":1,"connections":{"cn_fixture":record}}),
    );
    write(
        config.join("cortex/defaults.json"),
        json!({"defaults":{"route_connection_id":"cn_fixture","route_model":"fixture-model","parameters":{}}}),
    );
    write(
        config.join("cortex/routing.json"),
        json!({"model_policy_version":2,"enabled":{(engine):["fixture-model"]},
        "models":{(engine):{"auto":["fixture-model"],"tiers":{"fixture-model":["light","standard","deep"]}}},
        "pin":{"*":format!("{engine}/fixture-model")}}),
    );
    let mut route = json!({"engine":engine,"connection_id":"cn_fixture","model":"fixture-model","supported_parameters":[]});
    route["id"] = json!(format!("xr_{}", &hash(&route)[..24]));
    write(
        catalog.join("cs_fixture.json"),
        json!({"snapshot_id":"cs_fixture","entries":[{
        "route":route,"connection_id":"cn_fixture","catalog_snapshot_id":"cs_fixture",
        "evidence":{"catalog_obtained":{"source":"discovery"}}}]}),
    );
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if scenario != "complete" {
        std::fs::write(
            f.0.join("project/scenario.txt"),
            if scenario == "paused" {
                "hold"
            } else {
                scenario
            },
        )
        .unwrap();
    }
    let spawn_service = || {
        tokio::process::Command::new(env!("CARGO_BIN_EXE_workspace-service"))
            .args([f.0.as_os_str(), checkout.as_os_str()])
            .env_clear()
            .env("HOME", f.0.join("home"))
            .env("XDG_CONFIG_HOME", &config_base)
            .env("XDG_STATE_HOME", &state_base)
            .env("XDG_RUNTIME_DIR", f.0.join("runtime"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", f.0.join("bin").display()),
            )
            // The wrapper execs this test binary from the cargo target directory,
            // which no engine filesystem view shows (debug-build opt-out).
            .env("CORTEX_TEST_UNCONFINED_ENGINES", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    };
    let mut child = spawn_service();
    let socket = channel
        .runtime_dir(&f.0.join("runtime"))
        .join("service.sock");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !socket.exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "service exited during startup"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    async fn rpc(socket: &Path, method: &str, params: Value) -> Value {
        let mut stream = workspace_service::connect(socket).await.unwrap();
        workspace_service::write_frame(
            &mut stream,
            &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
        )
        .await
        .unwrap();
        let reply = workspace_service::read_frame(&mut stream).await.unwrap();
        assert!(reply.get("error").is_none(), "{method}: {reply}");
        reply["result"].clone()
    }
    let project = rpc(
        &socket,
        "projects.register",
        json!({"root":f.0.join("project"),"displayName":"Binary fixture"}),
    )
    .await;
    let id = project["projectId"].as_str().unwrap();
    assert_eq!(
        rpc(&socket, "projects.activate", json!({"projectId":id})).await["executionHost"],
        "rust"
    );
    let created = rpc(
        &socket,
        "workspace",
        command(
            id,
            "tasks.create",
            json!(id),
            "create-binary",
            json!({"description":"Binary fixture","checkout":{"kind":"main"}}),
        ),
    )
    .await;
    // A route failure is a request refusal, not a failed connection or admitted task.
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o600)).unwrap();
    let start = command(
        id,
        "tasks.start",
        created["target"].clone(),
        "start-binary",
        json!({}),
    );
    let mut stream = workspace_service::connect(&socket).await.unwrap();
    workspace_service::write_frame(
        &mut stream,
        &json!({"jsonrpc":"2.0","id":7,"method":"workspace","params":start}),
    )
    .await
    .unwrap();
    let refused = workspace_service::read_frame(&mut stream).await.unwrap();
    assert_eq!(
        refused["error"]["message"],
        "native_route_executable_unavailable"
    );
    f.store()
        .read(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM runtime_tasks", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    rpc(
        &socket,
        "workspace",
        command(
            id,
            "tasks.start",
            created["target"].clone(),
            "start-binary",
            json!({}),
        ),
    )
    .await;
    let store = f.store();
    if scenario == "command-scratch-hold" {
        restart_signal(&store,"SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE operation='command_run' AND request_key='scratch-command-control' AND json_extract(result_json,'$.handle.profile')='codex_scratch')").await;
        let receipt = || {
            store.read(|db| {
            let raw: String = db.query_row("SELECT result_json FROM runtime_requests WHERE operation='command_run' AND request_key='scratch-command-control'", [], |r| r.get(0))?;
            Ok(serde_json::from_str::<Value>(&raw)?)
        }).unwrap()
        };
        let started = receipt();
        let handle = &started["handle"];
        let reference = handle["reference"].as_str().unwrap();
        let (task, execution): (String, String) = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT q.task_id,e.execution_id FROM runtime_requests q
                JOIN workspace_executions e ON e.runtime_task_id=q.task_id
                WHERE q.operation='command_run' AND q.request_key='scratch-command-control'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap();
        let output = store
            .key()
            .directory()
            .join(task)
            .join(".native-output")
            .join(handle["output"].as_str().unwrap());
        tokio::time::timeout(Duration::from_secs(10), async {
            while std::fs::read_to_string(&output).as_deref().ok()
                != Some("scratch-control-ready\n")
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            let current = receipt();
            panic!(
                "held scratch payload not observed: settled={}; failure={}; output file bytes={:?}",
                current["settled"],
                current["failure"],
                std::fs::metadata(&output).map(|m| m.len())
            );
        });
        rpc(
            &socket,
            "executions.control",
            json!({"projectId":id,"executionId":execution,
            "requestId":"stop-managed-scratch","action":"command_stop","toolCallRef":reference,
            "reason":"Stop the managed scratch command"}),
        )
        .await;
        restart_signal(&store,"SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE operation='command_run' AND request_key='scratch-command-control' AND json_extract(result_json,'$.settled')=1)").await;
        let stopped = receipt();
        assert_eq!(stopped["tool_call_ref"], reference);
        assert_eq!(stopped["failure"], "owner_stopped");
        assert_eq!(stopped["output"], "scratch-control-ready\n");
        assert!(!f.0.join("project/command-entered").exists());
        child.kill().await.unwrap();
        let _ = child.wait().await;
        println!("managed scratch payload started and stopped through the owner command control");
        return;
    }
    if scenario.starts_with("command-restart-") {
        restart_signal(&store,"SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE operation='command_run' AND json_type(result_json,'$.handle')='object')").await;
        tokio::time::timeout(Duration::from_secs(10), async {
            while !f.0.join("project/command-entered").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let read_intent = || {
            store
                .read(|db| {
                    let raw: String = db.query_row(
                        "SELECT result_json FROM runtime_requests WHERE operation='command_run'",
                        [],
                        |r| r.get(0),
                    )?;
                    Ok(serde_json::from_str::<Value>(&raw)?)
                })
                .unwrap()
        };
        let intent = read_intent();
        let handle = &intent["handle"];
        let reference = handle["reference"].as_str().unwrap();
        let identity = handle["identity"].clone();
        let task: String = store
            .read(|db| Ok(db.query_row("SELECT task_id FROM runtime_tasks", [], |r| r.get(0))?))
            .unwrap();
        let output = store.key().directory().join(task).join(".native-output");
        child.kill().await.unwrap();
        assert!(!child.wait().await.unwrap().success());
        // Only fixture-owned processes and data, never a host service/session.
        if scenario == "command-restart-lost" {
            // The supervisor records its payload's process group right after launch.
            tokio::time::timeout(Duration::from_secs(10), async {
                while serde_json::from_slice::<Value>(
                    &std::fs::read(output.join(handle["status"].as_str().unwrap())).unwrap(),
                )
                .unwrap()["group"]
                    .as_i64()
                    .is_none()
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            unsafe {
                libc::kill(identity["pid"].as_u64().unwrap() as i32, libc::SIGKILL);
            }
        } else if scenario == "command-restart-offline" {
            std::fs::write(f.0.join("project/command-release"), "release").unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                while serde_json::from_slice::<Value>(
                    &std::fs::read(output.join(handle["status"].as_str().unwrap())).unwrap(),
                )
                .unwrap()["state"]
                    != "finished"
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
        }
        child = spawn_service();
        restart_signal(
            &store,
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='NATIVE_RECOVERY_REQUIRED')",
        )
        .await;
        if scenario == "command-restart-lost" {
            restart_signal(&store,"SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='NATIVE_COMMAND_RECOVERED' AND json_extract(payload_json,'$.handle_state')='unknown')").await;
            // The Development payload outlives its lost supervisor (no PID namespace) and
            // still waits in its recorded process group: the receipt stays unknown.
            assert_ne!(read_intent()["settled"], true);
            let execution: String = store
                .read(|db| {
                    Ok(
                        db.query_row("SELECT execution_id FROM workspace_executions", [], |r| {
                            r.get(0)
                        })?,
                    )
                })
                .unwrap();
            let mut stream = workspace_service::connect(&socket).await.unwrap();
            workspace_service::write_frame(&mut stream,&json!({"jsonrpc":"2.0","id":99,"method":"executions.control",
                "params":{"projectId":id,"executionId":execution,"requestId":"lost-stop","action":"command_stop","toolCallRef":reference,"reason":"Stop fixture-owned command"}})).await.unwrap();
            let refused = workspace_service::read_frame(&mut stream).await.unwrap();
            assert_eq!(
                refused["error"]["message"],
                "native_command_outcome_unknown"
            );
            // The command was not stoppable by the runtime after losing its
            // supervisor. Release the disposable payload before shutdown; this
            // does not turn the earlier unknown result into execution evidence.
            std::fs::write(f.0.join("project/command-release"), "release").unwrap();
        } else {
            if scenario == "command-restart-stop" {
                let execution: String = store
                    .read(|db| {
                        Ok(db.query_row(
                            "SELECT execution_id FROM workspace_executions",
                            [],
                            |r| r.get(0),
                        )?)
                    })
                    .unwrap();
                rpc(&socket,"executions.control",json!({"projectId":id,"executionId":execution,
                    "requestId":"recovered-stop","action":"command_stop","toolCallRef":reference,"reason":"Stop fixture-owned command"})).await;
            } else {
                std::fs::write(f.0.join("project/command-release"), "release").unwrap();
            }
            restart_signal(&store,"SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE operation='command_run' AND json_extract(result_json,'$.settled')=1)").await;
            let receipt = read_intent();
            assert_eq!(receipt["tool_call_ref"], reference);
            assert_eq!(receipt["large_output_id"], handle["id"]);
            assert!(
                receipt["revision_digest"].is_null(),
                "recovery cannot invent command revision evidence"
            );
            if scenario == "command-restart-stop" {
                assert_eq!(receipt["failure"], "owner_stopped");
                assert_eq!(receipt["complete"], false);
            } else {
                assert_eq!(receipt["exit_code"], 7);
                assert_eq!(receipt["failure"], Value::Null);
                assert_eq!(receipt["byte_count"], 70013);
                assert_eq!(receipt["output_truncated"], true);
                let bytes = std::fs::read(output.join(handle["output"].as_str().unwrap())).unwrap();
                assert!(bytes.starts_with(b"before\n"));
                assert!(bytes.ends_with(b"after\n"));
                store
                    .read(|db| {
                        let (hash, count, code): (String, i64, i64) = db.query_row(
                            "SELECT source_ref,byte_count,exit_code FROM large_outputs WHERE id=?",
                            [handle["id"].as_str().unwrap()],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                        )?;
                        assert_eq!(hash, format!("{:x}", sha2::Sha256::digest(&bytes)));
                        assert_eq!(count, bytes.len() as i64);
                        assert_eq!(code, 7);
                        Ok(())
                    })
                    .unwrap();
            }
            // Restart once more after settlement: exactly one receipt, no replay.
            child.kill().await.unwrap();
            let _ = child.wait().await.unwrap();
            let held = |store: &Store| {
                store
                    .read(|db| {
                        Ok(db.query_row(
                            "SELECT count(*) FROM runtime_events WHERE type='NATIVE_RECOVERY_REQUIRED'",
                            [],
                            |r| r.get::<_, i64>(0),
                        )?)
                    })
                    .unwrap()
            };
            let before = held(&store);
            child = spawn_service();
            // The project is ready only once this restart's recovery has finished.
            tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    if let Ok(mut stream) = workspace_service::connect(&socket).await {
                        if workspace_service::write_frame(
                            &mut stream,
                            &json!({"jsonrpc":"2.0","id":1,
                            "method":"projects.status","params":{"projectId":id}}),
                        )
                        .await
                        .is_ok()
                        {
                            if let Ok(reply) = workspace_service::read_frame(&mut stream).await {
                                if reply["result"]["ready"] == true {
                                    break;
                                }
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("recovery after the second restart");
            // The task is still held for the same reason: the restart records nothing again.
            assert_eq!(held(&store), before);
            assert_eq!(read_intent(), receipt);
            store.read(|db|{
                assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_COMMAND_FINISHED'",[],|r|r.get::<_,i64>(0))?,1);
                assert_eq!(db.query_row("SELECT count(*) FROM large_outputs WHERE source_kind='terminal'",[],|r|r.get::<_,i64>(0))?,1);Ok(())
            }).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(f.0.join("project/command-launches")).unwrap(),
            "launch\n"
        );
        store
            .read(|db| {
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM runtime_events WHERE type='NATIVE_COMMAND_STARTED'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    1
                );
                Ok(())
            })
            .unwrap();
        unsafe {
            libc::kill(child.id().unwrap() as i32, libc::SIGTERM);
        }
        assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
        println!(
            "service SIGKILL {scenario}: durable same command/output identity, observed status, no replay"
        );
        return;
    }
    if scenario != "complete" {
        let signal = match scenario {
            "hold" | "paused" => {
                "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE role_id='worker-backend-dev' AND status='ready')"
            }
            "review-hold" => {
                "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE role_id='worker-code-reviewer' AND status='ready')"
            }
            "verify-slow" => {
                "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='NATIVE_COMMAND_STARTED')"
            }
            "question" => "SELECT EXISTS(SELECT 1 FROM pending_questions WHERE status='open')",
            _ => unreachable!(),
        };
        restart_signal(&store, signal).await;
        if matches!(scenario, "hold" | "paused" | "review-hold") {
            let marker = if scenario != "review-hold" {
                "worker-entered"
            } else {
                "review-entered"
            };
            tokio::time::timeout(Duration::from_secs(5), async {
                while !f.0.join("project").join(marker).exists() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        if scenario == "paused" {
            let execution: String = store
                .read(|db| {
                    Ok(
                        db.query_row("SELECT execution_id FROM workspace_executions", [], |r| {
                            r.get(0)
                        })?,
                    )
                })
                .unwrap();
            rpc(
                &socket,
                "executions.control",
                json!({"projectId":id,"executionId":execution,
                "requestId":"pause-before-crash","action":"pause"}),
            )
            .await;
            // Pause replies only after the owned pump's processes settled
            // (Python `workspace_api/control.py` answers after `settled`), so a
            // later service death leaves nothing unknown and nothing to replay.
            let settled = |db: &rusqlite::Connection| -> CoreResult<()> {
                let (state, blocker): (String, Option<String>) =
                    db.query_row("SELECT state,blocker FROM runtime_tasks", [], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })?;
                assert_eq!((state.as_str(), blocker), ("PAUSED", None));
                for (sql, what) in [
                    (
                        "SELECT count(*) FROM delegation_provisioning WHERE status!='terminated'",
                        "unsettled session",
                    ),
                    (
                        "SELECT count(*) FROM capacity_reservations WHERE state!='released'",
                        "unreleased capacity",
                    ),
                    (
                        "SELECT count(*) FROM trusted_channels WHERE revoked_at IS NULL",
                        "live channel",
                    ),
                    (
                        "SELECT count(*) FROM runtime_events WHERE type='NATIVE_RECOVERY_REQUIRED'",
                        "recovery barrier",
                    ),
                ] {
                    assert_eq!(
                        db.query_row(sql, [], |r| r.get::<_, i64>(0))?,
                        0,
                        "paused task: {what}"
                    );
                }
                Ok(())
            };
            store.read(settled).unwrap();
            let before: i64 = store
                .read(|db| {
                    Ok(
                        db.query_row("SELECT count(*) FROM delegation_provisioning", [], |r| {
                            r.get(0)
                        })?,
                    )
                })
                .unwrap();
            child.kill().await.unwrap();
            assert!(!child.wait().await.unwrap().success());
            child = spawn_service();
            // Activation reconciles retained tasks before it reports the project ready;
            // `alreadyActive` means the supervisor's activation already did.
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    if let Ok(mut stream) = workspace_service::connect(&socket).await {
                        workspace_service::write_frame(
                            &mut stream,
                            &json!({"jsonrpc":"2.0","id":1,
                            "method":"projects.activate","params":{"projectId":id}}),
                        )
                        .await
                        .unwrap();
                        let reply = workspace_service::read_frame(&mut stream).await.unwrap();
                        if reply["result"]["executionHost"] == "rust"
                            || reply["result"]["alreadyActive"] == true
                        {
                            break;
                        }
                        assert_eq!(
                            reply["error"]["message"], "project_activation_running",
                            "{reply}"
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("restarted service did not activate the project");
            // The hold survives restart unchanged: no session replay, no recovery
            // barrier and no automatic continuation.
            store
                .read(|db| {
                    settled(db)?;
                    assert_eq!(
                        db.query_row("SELECT count(*) FROM delegation_provisioning", [], |r| r
                            .get::<_, i64>(0))?,
                        before,
                        "paused session replayed"
                    );
                    Ok(())
                })
                .unwrap();
            // Only an explicit owner Resume continues, with a fresh coordinator.
            std::fs::write(f.0.join("project/scenario.txt"), "complete").unwrap();
            let version: i64 = store
                .read(|db| {
                    Ok(db.query_row(
                        "SELECT t.version FROM workspace_tasks t JOIN workspace_executions e USING(task_id)",
                        [],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            rpc(
                &socket,
                "executions.control",
                json!({"projectId":id,"executionId":execution,"requestId":"resume-after-crash",
                "action":"resume","expectedVersion":version.to_string()}),
            )
            .await;
            restart_signal(
                &store,
                "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE state='COMPLETED')",
            )
            .await;
            store
                .read(|db| {
                    assert!(
                        db.query_row("SELECT count(*) FROM delegation_provisioning", [], |r| r.get::<_, i64>(0))?
                            > before,
                        "Resume did not start a fresh session"
                    );
                    assert_eq!(
                        db.query_row("SELECT count(*) FROM delegation_provisioning WHERE status!='terminated'", [], |r| r.get::<_, i64>(0))?,
                        0
                    );
                    Ok(())
                })
                .unwrap();
            unsafe {
                libc::kill(child.id().unwrap() as i32, libc::SIGTERM);
            }
            assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .unwrap()
                .unwrap()
                .success());
            println!(
                "service SIGKILL after settled Pause: hold retained without recovery barrier or replay; explicit Resume completed"
            );
            return;
        }
        let before: i64 = store
            .read(|db| {
                Ok(
                    db.query_row("SELECT count(*) FROM delegation_provisioning", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .unwrap();
        child.kill().await.unwrap();
        assert!(!child.wait().await.unwrap().success());
        // A real SIGKILL and observed child wait, not a synthetic host reopen.
        let last_attempt: i64 = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT coalesce(max(rowid),0) FROM delegation_provisioning",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        child = spawn_service();
        // A worker question keeps its own wait (a recovery barrier); anything else
        // resumes in its state on the same rung (a restart is infrastructure), or, while
        // a process or command is unconfirmed, is a continuation the runtime relaunches.
        restart_signal(
            &store,
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type IN ('NATIVE_RECOVERY_REQUIRED','AUTOMATIC_CONTINUATION') OR (type='NATIVE_RECOVERY_RESUMED' AND json_extract(payload_json,'$.restart_resume')=1))",
        )
        .await;
        store.read(|db| {
            // The interrupted sessions are never replayed: no earlier attempt is live again.
            assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning WHERE rowid<=? AND status='ready'",[last_attempt],|r|r.get::<_,i64>(0))?,0,"unknown session replayed");
            assert!(db.query_row("SELECT count(*) FROM delegation_provisioning",[],|r|r.get::<_,i64>(0))?>=before);
            if scenario!="question" {
                let resumed:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_RECOVERY_RESUMED' AND json_extract(payload_json,'$.restart_resume')=1",[],|r|r.get(0))?;
                if resumed==0 {
                    let reason:String=db.query_row("SELECT json_extract(payload_json,'$.reason') FROM runtime_events WHERE type='AUTOMATIC_CONTINUATION' ORDER BY id LIMIT 1",[],|r|r.get(0))?;
                    assert!(matches!(reason.as_str(),"native_verification_outcome_unknown"|"native_session_outcome_unknown"|"native_service_interrupted"),"{scenario}: {reason}");
                }
                assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE type='AUTOMATIC_PAUSED'",[],|r|r.get::<_,i64>(0))?,0);
                assert_eq!(db.query_row("SELECT count(*) FROM pending_questions WHERE status='open'",[],|r|r.get::<_,i64>(0))?,0);
            }
            if scenario=="question" {
                let (state,blocker):(String,String)=db.query_row("SELECT state,blocker FROM runtime_tasks",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
                assert_eq!(state,"RECOVERING");
                assert_eq!(blocker,"native_session_outcome_unknown");
                assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning",[],|r|r.get::<_,i64>(0))?,before,"unknown session replayed");
                assert_eq!(db.query_row("SELECT count(*) FROM trusted_channels WHERE revoked_at IS NULL",[],|r|r.get::<_,i64>(0))?,0);
                assert_eq!(db.query_row("SELECT count(*) FROM pending_questions WHERE status='open'",[],|r|r.get::<_,i64>(0))?,1);
                let payload:String=db.query_row("SELECT payload_json FROM runtime_events WHERE type='WORKER_WAIT_CHANGED' ORDER BY id DESC LIMIT 1",[],|r|r.get(0))?;
                let wait:Value=serde_json::from_str(&payload)?;
                assert_eq!(wait["status"],"open");
                assert_eq!(wait["continuation_required"],true);
            }
            Ok(())
        }).unwrap();
        unsafe {
            libc::kill(child.id().unwrap() as i32, libc::SIGTERM);
        }
        assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
        println!(
            "service SIGKILL during {scenario}: no session/command replay; a continuation (or the question's own wait) carries the unknown outcome"
        );
        return;
    }
    tokio::time::timeout(Duration::from_secs(20),async {
        loop {
            let state:String=store.read(|db|Ok(db.query_row("SELECT state FROM runtime_tasks",[],|r|r.get(0))?)).unwrap();
            if state=="COMPLETED" {break;}
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap_or_else(|_| {
        let diag=store.read(|db|Ok(db.query_row("SELECT coalesce(group_concat(type||':'||payload_json),'') FROM runtime_events WHERE type IN ('NATIVE_DRIVER_STOPPED','NATIVE_SESSION_START_FAILED')",[],|r|r.get::<_,String>(0))?)).unwrap();
        panic!("binary native task did not complete: {diag}");
    });
    store.read(|db| {
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='TASK_CREATED' AND json_extract(payload_json,'$.execution_host')='rust')",[],|r|r.get::<_,bool>(0))?);
        assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning WHERE status!='terminated'",[],|r|r.get::<_,i64>(0))?,0);
        Ok(())
    }).unwrap();
    unsafe {
        libc::kill(child.id().unwrap() as i32, libc::SIGTERM);
    }
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    println!(
        "built service binary: native create/start/review/verify/completion through socket; private HOME/XDG, PATH fake engine"
    );
}
async fn maintenance_admission() {
    let f = Fixture::new();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Fixture")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap();
    service.activate(id).await.unwrap();
    let created = service
        .workspace(command(
            id,
            "tasks.create",
            json!(id),
            "create-maintenance",
            json!({"description":"Held before any engine dispatch"}),
        ))
        .await
        .unwrap();
    let job_id = "b".repeat(32);
    let job = f.0.join("state/maintenance").join(&job_id);
    std::fs::create_dir_all(&job).unwrap();
    for dir in [&job, job.parent().unwrap()] {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(
        job.join("status.json"),
        json!({"kind":"install","phase":"unknown","updated":0}).to_string(),
    )
    .unwrap();
    let start = command(
        id,
        "tasks.start",
        created["target"].clone(),
        "start-maintenance",
        json!({}),
    );
    assert_eq!(
        service.workspace(start).await.unwrap_err().to_string(),
        "maintenance_busy"
    );
    for action in ["resume", "retry", "prompt", "rerun_check"] {
        let request = json!({"projectId":id,"executionId":"unknown","requestId":format!("maintenance-{action}"),"action":action});
        assert_eq!(
            service
                .control_execution(request)
                .await
                .unwrap_err()
                .to_string(),
            "maintenance_busy"
        );
    }
    let store = f.store();
    store
        .read(|db| {
            for table in [
                "runtime_tasks",
                "workspace_starts",
                "workspace_executions",
                "delegation_provisioning",
            ] {
                assert_eq!(
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
            }
            Ok(())
        })
        .unwrap();
    let stop = json!({"projectId":id,"executionId":"unknown","requestId":"maintenance-stop","action":"pause"});
    assert_ne!(
        service
            .control_execution(stop)
            .await
            .unwrap_err()
            .to_string(),
        "maintenance_busy"
    );
    service.stop().await.unwrap();
    println!(
        "native maintenance: Start/resume/retry/prompt/rerun hold, Stop remains available; no engine or task admission"
    );
}
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
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "service-regression-peer" {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(service_regressions::peer());
        return;
    }
    if service_regressions::run(&mode) {
        return;
    }
    if mode.starts_with("desktop-") || mode.starts_with('/') {
        peer::main();
        return;
    }
    if mode.is_empty() || mode == "sprint-cross-task-probe" {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            tokio::time::timeout(Duration::from_secs(45), sprint_cross_task_gateway())
                .await
                .expect("sprint cross-task gateway timeout")
                .expect("sprint cross-task gateway failed");
        });
        println!(
            "sprint graph: authenticated coordinator read its dependency graph through the broker"
        );
        if !mode.is_empty() {
            return;
        }
    }

    if mode == "attachments-probe" {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            tokio::time::timeout(Duration::from_secs(60), attachments_journey())
                .await
                .expect("attachments journey timeout");
            if !namespace_host::assert_unavailable_view() {
                tokio::time::timeout(Duration::from_secs(60), answer_attachments_journey())
                    .await
                    .expect("answer attachments journey timeout");
            }
        });
        println!(
            "owner attachments: draft Start, follow-up message and answer retained and delivered to the engine"
        );
        return;
    }
    if mode.is_empty() || mode == "prompt-usage-refusal" {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            tokio::time::timeout(
                Duration::from_secs(60),
                degenerate_turn_journey("usage-refusal-twice"),
            )
            .await
            .expect("prompt usage refusal journey timeout");
        });
        println!("settled ACP refusals each account once, including the fresh-session retry");
        if mode == "prompt-usage-refusal" {
            return;
        }
    }
    // The degenerate-turn watchdog: a coordinator-only stop runs on every host; the
    // retried journey runs a worker, which needs the namespace sandbox.
    if mode.is_empty() || mode == "degenerate-probe" {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            tokio::time::timeout(
                Duration::from_secs(60),
                degenerate_turn_journey("degenerate-twice"),
            )
            .await
            .expect("degenerate-twice journey timeout");
        });
        if !namespace_host::assert_unavailable_view() {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(60),
                    degenerate_turn_journey("degenerate-once"),
                )
                .await
                .expect("degenerate-once journey timeout");
                tokio::time::timeout(
                    Duration::from_secs(60),
                    degenerate_turn_journey("degenerate-worker"),
                )
                .await
                .expect("degenerate-worker journey timeout");
            });
            println!(
                "degenerate coordinator and worker turns retried once in a fresh session; the tasks completed"
            );
        }
        println!(
            "degenerate coordinator turns: cancelled, recorded, retried once in a fresh session, then continued on an escalated rung (no owner stop)"
        );
        if mode == "degenerate-probe" {
            return;
        }
    }
    if mode.is_empty() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            service_regressions::batch().await;
            service_regressions::sprint().await;
        });
    }
    // Coordinator admission and configure need no namespace sandbox, so the
    // saved-settings Start regression runs on every host.
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for route in [CLAUDE_ROUTE, CODEX_ROUTE] {
            for sprint in [true, false] {
                tokio::time::timeout(Duration::from_secs(60), saved_settings_start(route, sprint))
                    .await
                    .expect("saved settings Start timeout");
            }
        }
    });
    if mode == "saved-settings-probe" {
        return;
    }
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(maintenance_admission());
    if namespace_host::assert_unavailable_view() {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        if mode.is_empty() {
            service_regressions::verification().await;
        }
        if mode == "waiting-worker-probe" {
            for variant in ["guidance", "preempt"] {
                tokio::time::timeout(Duration::from_secs(30), waiting_worker(variant))
                    .await
                    .expect("waiting worker journey timeout");
            }
            println!("waiting worker guidance and pre-empted guidance passed");
            return;
        }
        if mode == "batch-probe" {
            tokio::time::timeout(Duration::from_secs(35), controls("batch", "answer_batch"))
                .await
                .expect("batch control journey timeout");
            return;
        }
        if mode == "repair-probe" { controls("repair-failed-test", "repair").await; return; }
        if mode == "rolling-probe" {
            tokio::time::timeout(Duration::from_secs(35), controls("rolling", "rolling"))
                .await
                .expect("rolling dispatch journey timeout");
            println!("rolling dispatch: dependent worker started while an independent sibling was held");
            return;
        }
        if mode == "access-retry-probe" {
            for action in ["release", "deny"] {
                tokio::time::timeout(Duration::from_secs(30), controls("access-retry", action)).await.expect("access retry journey timeout");
            }
            println!("access retry grant and denial delivered");
            return;
        }
        // One owner-control journey alone: `control-probe=<scenario>:<action>`.
        if let Some((scenario, action)) = mode.strip_prefix("control-probe=").and_then(|s| s.split_once(':')) {
            workspace_service::native_runtime::set_retry_window((scenario == "exit").then_some(
                cortex_core::scheduler::retry::RetryWindow { first_delay: 0.0, max_delay: 0.0, window: 0.5 },
            ));
            tokio::time::timeout(Duration::from_secs(35), controls(scenario, action))
                .await
                .expect("control journey timeout");
            println!("native desktop control {scenario}:{action} passed");
            return;
        }
        if mode == "steering-probe" {
            tokio::time::timeout(Duration::from_secs(35), controls("hold", "prompt"))
                .await
                .expect("steering control journey timeout");
            println!("owner message during a running worker: nothing interrupted, delivered once at the boundary");
            tokio::time::timeout(Duration::from_secs(40), queued_message_journey())
                .await
                .expect("queued message journey timeout");
            println!("queued owner message: editable until taken, sent by the service at the boundary, delivered once");
            return;
        }
        tokio::time::timeout(Duration::from_secs(40), worker_language())
            .await
            .expect("worker language journey timeout");
        if mode == "language-probe" {
            println!("owner task language line: delivered to every worker and reviewer session");
            return;
        }
        tokio::time::timeout(Duration::from_secs(40), worker_route())
            .await
            .expect("worker route journey timeout");
        if mode == "worker-route-probe" {
            println!("worker routes: each worker session configured and shown on its own routed engine and model");
            return;
        }
        if mode == "legacy-recovery-probe" {
            for stage in 0..7 { checkpoint_recovery(stage, true).await; }
            println!("Legacy-owned checkpoints: planning, execution, review, verification and owner-question recovery passed");
            return;
        }
        if mode == "managed-command-stop" {
            tokio::time::timeout(Duration::from_secs(40), binary_journey("command-scratch-hold"))
                .await
                .expect("managed command stop exceeded deadline");
            return;
        }
        if mode == "command-restart-probe" {
            for scenario in [
                "command-restart-exit",
                "command-restart-offline",
                "command-restart-stop",
                "command-restart-lost",
            ] {
                tokio::time::timeout(Duration::from_secs(40), binary_journey(scenario))
                    .await
                    .unwrap();
            }
            return;
        }
        for action in [
            "worker_message",
            "worker_stop",
            "command_stop",
            "rerun_check",
            "task_settings",
        ] {
            println!("positive native owner control: {action}");
            tokio::time::timeout(Duration::from_secs(30), positive_owner_control(action))
                .await
                .expect("positive control timeout");
        }
        for scenario in [
            "command-restart-exit",
            "command-restart-offline",
            "command-restart-stop",
            "command-restart-lost",
        ] {
            tokio::time::timeout(Duration::from_secs(40), binary_journey(scenario))
                .await
                .unwrap();
        }
        for stage in 0..3 {
            checkpoint_recovery(stage, false).await;
        }
        for stage in 0..7 {
            checkpoint_recovery(stage, true).await;
        }
        completed_legacy_questions_are_readable().await;
        for scenario in [
            "complete",
            "hold",
            "review-hold",
            "verify-slow",
            "question",
            "paused",
        ] {
            binary_journey(scenario).await;
        }
        tokio::time::timeout(Duration::from_secs(45), journey())
            .await
            .expect("service journey timeout");
        tokio::time::timeout(Duration::from_secs(120), controls_after_a_paused_task())
            .await
            .expect("controls after a paused task timeout");
        tokio::time::timeout(Duration::from_secs(30), waiting_worker("guidance"))
            .await
            .expect("guidance while waiting timeout");
        tokio::time::timeout(Duration::from_secs(30), waiting_worker("preempt"))
            .await
            .expect("pre-empted guidance timeout");
        workspace_service::native_runtime::set_retry_window(Some(
            cortex_core::scheduler::retry::RetryWindow { first_delay: 0.0, max_delay: 0.0, window: 0.5 },
        ));
        tokio::time::timeout(Duration::from_secs(30), waiting_worker("worker_exit"))
            .await
            .expect("worker exit while waiting timeout");
        workspace_service::native_runtime::set_retry_window(None);
        for (scenario, action) in [
            ("access-retry", "release"),
            ("access-retry", "deny"),
            ("pending-result", "pending-result"),
            ("pending-end-turn", "pending-end-turn"),
            ("hold", "pause"),
            ("hold", "stop"),
            ("hold", "prompt"),
            ("question", "answer"),
            ("question", "pause_resume_question"),
            ("reconcile-question", "pause_resume_question"),
            ("rolling", "rolling"),
            ("repair-failed-test", "repair"),
            ("batch", "answer_batch"),
            ("exit", "retry"),
            ("hold", "mark_resolved"),
            ("hold", "ambiguous"),
            ("hold", "unavailable"),
        ] {
            println!("native desktop control: {action}");
            // Python's tests shorten the lost-worker retry window the same way;
            // the default 600-second window would hold this journey for minutes.
            workspace_service::native_runtime::set_retry_window((scenario == "exit").then_some(
                cortex_core::scheduler::retry::RetryWindow {
                    first_delay: 0.0,
                    max_delay: 0.0,
                    window: 0.5,
                },
            ));
            tokio::time::timeout(Duration::from_secs(35), controls(scenario, action))
                .await
                .expect("control journey timeout");
            workspace_service::native_runtime::set_retry_window(None);
        }
    });
    if mode == "command-restart-probe" {
        println!(
            "brokered-command restart probe: four service SIGKILL/attachment scenarios passed"
        );
    } else {
        println!(
            "native desktop Start: completion, independent review, bounded verification, versioned UI delta; zero retained-controller launches/mutations"
        );
    }
}

/// The owner's controls run on one serial lane. A paused task must release it, so a
/// later worker stop on another task of the same project completes (the rendered
/// journey: guidance, pause, then Stop worker on a new task).
async fn controls_after_a_paused_task() {
    let f = Fixture::new();
    std::fs::write(f.0.join("project/scenario.txt"), "guidance").unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Serial controls")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap().to_owned();
    service.activate(&id).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    let store = f.store();
    async fn wait_for(store: &Store, sql: &str) {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !store.read(|db| Ok(db.query_row(sql, [], |r| r.get::<_, bool>(0))?)).unwrap() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            let dump = store.read(|db| Ok(db.prepare("SELECT t.task_id||' '||t.state||' '||coalesce((SELECT group_concat(type,',') FROM (SELECT type FROM runtime_events e WHERE e.task_id=t.task_id ORDER BY id DESC LIMIT 8)),'') FROM runtime_tasks t")?.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?)).unwrap();
            panic!("checkpoint missing: {sql}\n{dump:#?}")
        });
    }
    let start = |title: &'static str| {
        let service = service.clone();
        let id = id.clone();
        async move {
            let created = service
                .workspace(command(
                    &id,
                    "tasks.create",
                    json!(id),
                    if title.starts_with("Guidance") {
                        "create-a"
                    } else {
                        "create-b"
                    },
                    json!({"description":title,"checkout":{"kind":"main"}}),
                ))
                .await
                .unwrap();
            let started = service
                .workspace(command(
                    &id,
                    "tasks.start",
                    created["target"].clone(),
                    if title.starts_with("Guidance") {
                        "start-a"
                    } else {
                        "start-b"
                    },
                    json!({}),
                ))
                .await
                .unwrap();
            (
                created["target"].as_str().unwrap().to_owned(),
                started["target"].as_str().unwrap().to_owned(),
            )
        }
    };
    let worker_of = |task_title: &'static str| {
        let service = service.clone();
        let id = id.clone();
        async move {
            let call =
                serde_json::from_value(json!({"op":"product.facts","projectId":id,"params":{}}))
                    .unwrap();
            let facts = service.machine(call).await.unwrap();
            let _ = task_title;
            facts["result"]["workers"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|w| w["status"] == "working")
                .map(|w| w["id"].as_str().unwrap().to_owned())
                .last()
                .unwrap()
        }
    };
    let (draft, execution) = start("Guidance then pause").await;
    wait_for(&store, "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE role_id='worker-backend-dev' AND status='ready')").await;
    let worker = worker_of("Guidance then pause").await;
    tokio::time::timeout(Duration::from_secs(20), service.control_execution(json!({"action":"worker_message","workerId":worker,
        "text":"Keep the worker message verbatim","attachments":[],"projectId":id,"executionId":execution,"requestId":"guide"})))
        .await
        .expect("worker message hung")
        .unwrap();
    let query = json!({"protocol":{"major":2,"mapping":2},"requestId":"page","projectId":id,"operation":"tasks.page","payload":{"target":id,"cursor":null,"limit":50}});
    let page = read_page(&service, query).await;
    let version = page["data"]["value"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(draft))
        .map(|t| t["version"].clone())
        .unwrap_or(Value::Null);
    tokio::time::timeout(
        Duration::from_secs(20),
        service.control_execution(json!({"action":"pause","projectId":id,
        "executionId":execution,"requestId":"pause","expectedVersion":version})),
    )
    .await
    .expect("pause hung: the serial control lane stays held")
    .unwrap();
    std::fs::write(f.0.join("project/scenario.txt"), "stop-siblings").unwrap();
    let (_, execution) = start("Stop one worker").await;
    wait_for(
        &store,
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE node_id='second' AND status='completed')
        AND EXISTS(SELECT 1 FROM runtime_nodes WHERE node_id='work' AND status='running')",
    )
    .await;
    let worker = worker_of("Stop one worker").await;
    // The owner reaches Stop worker after the sibling peer has given up waiting for its
    // release signal (5 s): the stop must still complete.
    tokio::time::sleep(Duration::from_secs(7)).await;
    let began = std::time::Instant::now();
    tokio::time::timeout(
        Duration::from_secs(20),
        service.control_execution(json!({"action":"worker_stop","workerId":worker,
        "projectId":id,"executionId":execution,"requestId":"stop-one"})),
    )
    .await
    .expect("worker stop hung after a paused task")
    .unwrap();
    eprintln!("worker stop took {:?}", began.elapsed());
    service.stop().await.unwrap();
    let _ = runner.await;
}

const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nowner screenshot";
fn wire_file(name: &str, mime: &str, bytes: &[u8]) -> Value {
    use base64::Engine as _;
    json!({"name":name,"mimeType":mime,"data":base64::engine::general_purpose::STANDARD.encode(bytes)})
}
/// The one upload of an owner file: its reference is what every later command carries.
async fn sent_file(service: &Service, name: &str, mime: &str, bytes: &[u8]) -> Value {
    let refs = machine(
        service,
        "product.attachment",
        "",
        json!({"action":"upload","files":[wire_file(name, mime, bytes)]}),
    )
    .await;
    assert!(!refs.to_string().contains("\"data\""), "{refs}");
    refs[0].clone()
}
/// The engine's prompt blocks recorded by the fixture peer, oldest first.
fn engine_prompts(f: &Fixture) -> Vec<Value> {
    std::fs::read_to_string(f.0.join("project/prompt-log.jsonl"))
        .map(|log| {
            log.lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap()["prompt"].clone())
                .collect()
        })
        .unwrap_or_default()
}
fn carries_image(prompt: &Value) -> bool {
    use base64::Engine as _;
    prompt.as_array().is_some_and(|blocks| {
        blocks.iter().any(|b| {
            b["type"] == "image"
                && b["mimeType"] == "image/png"
                && b["data"] == base64::engine::general_purpose::STANDARD.encode(PNG_BYTES)
        })
    })
}
fn carries_text(prompt: &Value, needle: &str) -> bool {
    prompt.as_array().is_some_and(|blocks| {
        blocks.iter().any(|b| {
            b["resource"]["text"] == needle
                || b["text"]
                    .as_str()
                    .is_some_and(|t| t.contains(&format!("\n{needle}\n")))
        })
    })
}
/// Owner files, end to end through the real service entry: the service advertises
/// attachments, a draft keeps its files, Start retains them as private bodies and the
/// engine's first prompt carries the image and the text; a later owner message with a
/// file retains and delivers it too; invalid files refuse before anything is kept.
async fn attachments_journey() {
    let f = Fixture::new();
    // The coordinator plans; its worker holds until the follow-up message is recorded.
    std::fs::write(f.0.join("project/scenario.txt"), "hold").unwrap();
    std::fs::write(f.0.join("project/record-prompts"), "").unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Files")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap().to_owned();
    service.activate(&id).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    let store = f.store();
    // The capability the desktop reads (through `projects.status`) is explicit.
    let status = service.project_status(&id).await.unwrap();
    assert_eq!(status["capabilities"]["attachments"], true, "{status}");
    workspace_protocol::validate("Capabilities", &status["capabilities"]).unwrap();
    // Invalid files refuse their upload; a command with inline bytes or an unknown
    // reference refuses the draft; nothing is stored.
    for bad in [
        wire_file("../x.png", "image/png", PNG_BYTES),
        json!({"name":"x.png","mimeType":"image/png","data":"not base64!"}),
    ] {
        let reply = service
            .machine(
                serde_json::from_value(
                    json!({"op":"product.attachment","params":{"action":"upload","files":[bad]}}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reply["code"], "invalid_attachments", "{reply}");
    }
    let unknown = json!({"name":"x.png","mimeType":"image/png","size":3,"sha256":"0".repeat(64)});
    for (n, (bad, code)) in [
        (
            wire_file("x.png", "image/png", PNG_BYTES),
            "inline_attachment_refused",
        ),
        (unknown, "attachment_unavailable"),
    ]
    .into_iter()
    .enumerate()
    {
        let refused = service
            .workspace(command(
                &id,
                "tasks.create",
                json!(id),
                &format!("bad-{n}"),
                json!({"description":"Bad files","attachments":[bad],"checkout":{"kind":"main"}}),
            ))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(refused, code);
    }
    let shot = sent_file(&service, "shot.png", "image/png", PNG_BYTES).await;
    let notes = sent_file(&service, "notes.txt", "text/plain", b"read me first").await;
    let created = service
        .workspace(command(
            &id,
            "tasks.create",
            json!(id),
            "create",
            json!({"description":"Look at the screenshot","checkout":{"kind":"main"},
            "attachments":[shot.clone(), notes]}),
        ))
        .await
        .unwrap();
    // The saved draft reads its files back for editing.
    let saved = machine(
        &service,
        "product.read",
        &id,
        json!({"taskId":created["target"],"attachments":true}),
    )
    .await;
    assert_eq!(saved.as_array().unwrap().len(), 2);
    assert_eq!(saved[0], shot, "the draft reads back references, not bytes");
    let started = service
        .workspace(command(
            &id,
            "tasks.start",
            created["target"].clone(),
            "start",
            json!({}),
        ))
        .await
        .unwrap_or_else(|e| panic!("Start with files refused: {e}"));
    let execution = started["target"].as_str().unwrap().to_owned();
    tokio::time::timeout(Duration::from_secs(25), async {
        while !engine_prompts(&f).iter().any(carries_image) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the engine never received the image block");
    let first = engine_prompts(&f).into_iter().find(carries_image).unwrap();
    assert!(carries_text(&first, "read me first"), "{first}");
    store.read(|db| {
        let mut q = db.prepare("SELECT kind,mime,name,sha256,size_bytes FROM source_blocks WHERE availability='stored' ORDER BY block_index")?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        use sha2::{Digest, Sha256};
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("image".into(), "image/png".into(), "shot.png".into(), format!("{:x}", Sha256::digest(PNG_BYTES)), PNG_BYTES.len() as i64));
        assert_eq!(rows[1].2, "notes.txt");
        let events: String = db.query_row("SELECT coalesce(group_concat(payload_json),'') FROM runtime_events", [], |r| r.get(0))?;
        assert!(!events.contains(&wire_file("x", "x", PNG_BYTES)["data"].as_str().unwrap()[..24].to_owned()), "file bytes reached an event");
        Ok(())
    }).unwrap();
    // A follow-up owner message with a file (also Request changes): refused when invalid, else
    // retained as the next revision and delivered to the engine.
    let refused = service
        .prompt_execution(
            &id,
            &execution,
            "More",
            Some("bad-files"),
            json!([{"name":"b.txt","mimeType":"text/plain","data":"YQ=="}]),
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(refused, "inline_attachment_refused");
    let more = sent_file(&service, "more.txt", "text/plain", b"second file body").await;
    let before = engine_prompts(&f).len();
    let receipt = service
        .prompt_execution(
            &id,
            &execution,
            "Also read this",
            Some("with-file"),
            json!([more.clone()]),
        )
        .await
        .unwrap();
    assert_eq!(receipt["native"], true);
    assert_eq!(
        service
            .prompt_execution(
                &id,
                &execution,
                "Also read this",
                Some("with-file"),
                json!([more.clone()])
            )
            .await
            .unwrap(),
        receipt
    );
    // The message waits for the coordinator's next turn; the work it arrived during finishes.
    std::fs::write(f.0.join("project/scenario.txt"), "complete").unwrap();
    // The coordinator turn that receives the message also carries its file.
    tokio::time::timeout(Duration::from_secs(25), async {
        while !engine_prompts(&f)[before..].iter().any(|p| {
            carries_text(p, "second file body")
                && p[0]["text"].as_str().is_some_and(|t| {
                    t.contains("1. Also read this") && t.contains("Attached: more.txt")
                })
        }) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the follow-up file never reached the engine with its message");
    assert_eq!(
        store
            .read(|db| Ok(
                db.query_row("SELECT count(*) FROM source_revisions", [], |r| r
                    .get::<_, i64>(0))?
            ))
            .unwrap(),
        2
    );
    // Worker guidance still carries no files, with its own reason.
    assert_eq!(service.control_execution(json!({"action":"worker_message","workerId":"w","text":"x","projectId":id,
        "executionId":execution,"requestId":"worker-file","attachments":[sent_file(&service, "a.txt", "text/plain", b"a").await]})).await.unwrap_err().to_string(),
        "native_attachments_unavailable");
    service.stop().await.unwrap();
    let _ = runner.await;
}
/// Files typed with an answer to an open question are retained in the answer's own
/// transaction and reach the engine session that continues after it.
async fn answer_attachments_journey() {
    let f = Fixture::new();
    std::fs::write(f.0.join("project/scenario.txt"), "question").unwrap();
    std::fs::write(f.0.join("project/record-prompts"), "").unwrap();
    let service = Service::bind_native(f.config(), Arc::new(Routes))
        .await
        .unwrap();
    let registered = service
        .register(&f.0.join("project"), "Answer files")
        .await
        .unwrap();
    let id = registered["projectId"].as_str().unwrap().to_owned();
    service.activate(&id).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    let store = f.store();
    let created = service
        .workspace(command(
            &id,
            "tasks.create",
            json!(id),
            "create",
            json!({"description":"Answer files","checkout":{"kind":"main"}}),
        ))
        .await
        .unwrap();
    let started = service
        .workspace(command(
            &id,
            "tasks.start",
            created["target"].clone(),
            "start",
            json!({}),
        ))
        .await
        .unwrap();
    let execution = started["target"].as_str().unwrap().to_owned();
    tokio::time::timeout(Duration::from_secs(20), async {
        while !store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pending_questions WHERE status='open')",
                    [],
                    |r| r.get::<_, bool>(0),
                )?)
            })
            .unwrap()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("no question was asked");
    let page = service
        .workspace(
            json!({"protocol":{"major":1,"minor":8},"projectId":id,"requestId":"questions",
        "operation":"decisions.list","target":execution,"cursor":null,"limit":100}),
        )
        .await
        .unwrap();
    let d = &page["items"][0];
    // The owner chooses an option, adds their own words and attaches a file: one answer.
    let brief = d["options"][1].clone();
    assert_eq!(brief["label"], "Brief", "{d}");
    let answer = |request: &str, files: Value| {
        let mut c = command(
            &id,
            "decisions.answer",
            d["decisionId"].clone(),
            request,
            json!({"executionId":execution,"acpSessionId":d["acpSessionId"],"batchId":null,"answers":[{
                "decisionId":d["decisionId"],"questionId":d["questionId"],"expectedVersion":d["version"],
                "optionId":brief["optionId"],"text":"Brief\n\nsee the screenshot","attachments":files}]}),
        );
        c["expectedVersion"] = d["version"].clone();
        c["ownerEpoch"] = d["ownerEpoch"].clone();
        c
    };
    let refused = service
        .workspace(answer(
            "answer-bad",
            json!([wire_file("shot.png", "image/png", PNG_BYTES)]),
        ))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(refused, "inline_attachment_refused");
    let shot = sent_file(&service, "shot.png", "image/png", PNG_BYTES).await;
    assert_eq!(
        store
            .read(|db| Ok(
                db.query_row("SELECT count(*) FROM source_revisions", [], |r| r
                    .get::<_, i64>(0))?
            ))
            .unwrap(),
        0
    );
    let command = answer("answer", json!([shot.clone()]));
    let receipt = service
        .workspace(command.clone())
        .await
        .unwrap_or_else(|e| panic!("answer with files refused: {e}"));
    assert_eq!(service.workspace(command).await.unwrap(), receipt);
    assert_eq!(
        store
            .read(|db| Ok(
                db.query_row("SELECT count(*) FROM source_revisions", [], |r| r
                    .get::<_, i64>(0))?
            ))
            .unwrap(),
        1
    );
    tokio::time::timeout(Duration::from_secs(25), async {
        while !engine_prompts(&f).iter().any(carries_image) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the answered turn never received the image block");
    // The answered turn's prompt carries the chosen option and the owner's words with the file.
    let delivered = engine_prompts(&f).into_iter().find(carries_image).unwrap();
    assert!(
        delivered
            .to_string()
            .contains(r"Brief\n\nsee the screenshot"),
        "{delivered}"
    );
    // The settled question reads its option, the owner's words and its own file, also on a later read.
    let page = service
        .workspace(
            json!({"protocol":{"major":1,"minor":8},"projectId":id,"requestId":"questions-after",
        "operation":"decisions.list","target":execution,"cursor":null,"limit":100}),
        )
        .await
        .unwrap();
    let settled = &page["items"][0];
    assert_eq!(settled["status"], "answered", "{settled}");
    assert_eq!(settled["answerText"], "Brief\n\nsee the screenshot");
    assert_eq!(settled["answerAttachments"], json!([shot]), "{settled}");
    store
        .read(|db| {
            let payload: String = db.query_row(
                "SELECT payload_json FROM runtime_events WHERE type='USER_ANSWER_ATTACHMENTS'",
                [],
                |r| r.get(0),
            )?;
            assert!(
                payload.contains("shot.png") && !payload.contains("iVBOR"),
                "{payload}"
            );
            let owners: Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(
                owners["question_ids"],
                json!([d["questionId"]]),
                "{payload}"
            );
            let command_json: String = db.query_row(
                "SELECT coalesce(group_concat(payload_json),'') FROM runtime_events",
                [],
                |r| r.get(0),
            )?;
            assert!(
                !command_json.contains("iVBOR"),
                "file bytes reached an event"
            );
            Ok(())
        })
        .unwrap();
    // The answer's file is kept like a message's: a saved product record references it,
    // so the retention sweep keeps it past the unsent grace while it removes a stale
    // upload nothing references, and the settled question's chip still opens.
    let state = f.0.join("state");
    let product_file = state.join("product").join(format!("{id}.json"));
    let saved = |path: &std::path::Path| -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    };
    let task = created["target"].as_str().unwrap();
    let digest = shot["sha256"].as_str().unwrap().to_owned();
    let kept = |p: &Value| {
        p["answerFiles"][task]["attachments"]
            .as_array()
            .is_some_and(|a| a.iter().any(|r| r["sha256"] == json!(digest)))
    };
    assert!(kept(&saved(&product_file)), "{}", saved(&product_file));
    let stray = sent_file(&service, "stray.txt", "text/plain", b"never sent").await;
    let age = |sha: &Value| {
        std::fs::File::options()
            .write(true)
            .open(state.join("attachments").join(sha.as_str().unwrap()))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(25 * 3600))
            .unwrap();
    };
    age(&shot["sha256"]);
    age(&stray["sha256"]);
    machine(&service, "cleanup.run", "", json!({})).await;
    assert!(state.join("attachments").join(&digest).is_file());
    assert!(!state
        .join("attachments")
        .join(stray["sha256"].as_str().unwrap())
        .exists());
    let opened = machine(
        &service,
        "product.attachment",
        "",
        json!({"action":"read","sha256":digest}),
    )
    .await;
    assert_eq!(opened["size"], json!(PNG_BYTES.len()), "{opened}");
    // An answer recorded before answers were kept (no saved reference) becomes
    // referenced from Core's exact record on the next product read, and is then kept.
    let mut legacy = saved(&product_file);
    legacy.as_object_mut().unwrap().remove("answerFiles");
    std::fs::write(&product_file, serde_json::to_vec(&legacy).unwrap()).unwrap();
    machine(&service, "product.read", &id, Value::Null).await;
    assert!(kept(&saved(&product_file)), "{}", saved(&product_file));
    age(&shot["sha256"]);
    machine(&service, "cleanup.run", "", json!({})).await;
    machine(
        &service,
        "product.attachment",
        "",
        json!({"action":"read","sha256":digest}),
    )
    .await;
    service.stop().await.unwrap();
    let _ = runner.await;
}
