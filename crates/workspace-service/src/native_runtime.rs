//! Service-owned native Start composition. No model or socket supplied launch authority.
use crate::{
    actor, failure,
    native_route_hold::{Attempt as RouteAttempt, RouteKey, LIMITED},
    native_sessions::{
        LaunchInterrupts, LaunchTicket, NativeHost, SessionRole, SessionRoute, WorkerLaunch,
        WorkerLaunchOrder,
    },
    Service,
};
use cortex_core::{
    ownership::OwnerEpoch, CoreError, CoreResult, OpenMode, ProjectKey, Store, StoreChannel,
};
use futures::{stream::FuturesUnordered, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{watch, Mutex};

static RETRY_WINDOW: std::sync::Mutex<Option<cortex_core::scheduler::retry::RetryWindow>> =
    std::sync::Mutex::new(None);
/// Python reads its lost-worker retry window at call time
/// (`engine/retry_policy.py::default_window`, `runtime._engine_retry_window`)
/// so its tests can shorten it. This is the same seam; production never sets it.
#[doc(hidden)]
pub fn set_retry_window(window: Option<cortex_core::scheduler::retry::RetryWindow>) {
    *RETRY_WINDOW.lock().unwrap_or_else(|e| e.into_inner()) = window;
}

#[cfg(test)]
mod refill_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    async fn fixture() -> (
        Arc<Runtime>,
        cortex_core::ownership::OwnerEpoch,
        String,
        std::path::PathBuf,
    ) {
        fixture_with_routes(Arc::new(UnqualifiedRoutes)).await
    }

    async fn fixture_with_routes(
        routes: Arc<dyn NativeRoutes>,
    ) -> (Arc<Runtime>, OwnerEpoch, String, PathBuf) {
        let root = std::env::temp_dir().join(format!("refill-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || Runtime::open(&path, "project", routes))
                .await
                .unwrap()
                .unwrap(),
        );
        let owner = runtime.host.executor.publication(|store| {
            let owner = cortex_core::lifecycle::TaskMachine::new(store).create("t", "g", "owner")?;
            store.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'next','worker-devops-engineer','Work','[]','[\"src\"]','execution','pending',NULL,NULL)", [])?;
                Ok(())
            })?;
            Ok(owner)
        }).unwrap().await.unwrap().unwrap();
        (runtime, owner, "t".into(), root)
    }

    struct CapacityPeer;
    impl NativeRoutes for CapacityPeer {
        fn worker_route(
            &self,
            root: &Path,
            _: &str,
            node: &Value,
            _: &cortex_core::routing::Evidence,
            _: bool,
            _: Option<&str>,
        ) -> CoreResult<SessionRoute> {
            self.route(root, node["id"].as_str().unwrap())
        }

        fn route(&self, root: &Path, label: &str) -> CoreResult<SessionRoute> {
            use cortex_engine::environment::{EngineKind, LaunchSpec};
            let script = r#"
printf '%s\n' "$1" >> "$0/starts"
model=${2:-fixture-model}
mode=${3:-workspace-write}
config=$(printf '[{"id":"model","type":"select","currentValue":"%s","options":[{"value":"%s","name":"Fixture"}]},{"id":"mode","type":"select","currentValue":"%s","options":[{"value":"%s","name":"Fixture Mode"}]}]' "$model" "$model" "$mode" "$mode")
while IFS= read -r line; do
    id=${line#*\"id\":}
    id=${id%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{}}}\n' "$id" ;;
        *'"method":"session/new"'*)
            if [ -f "$0/handshake-$1" ]; then
                IFS= read -r error < "$0/handshake-$1"
                printf '{"jsonrpc":"2.0","id":%s,"error":%s}\n' "$id" "$error"
                continue
            fi
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"worker","configOptions":%s}}\n' "$id" "$config" ;;
        *'"method":"session/set_config_option"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"configOptions":%s}}\n' "$id" "$config" ;;
        *'"method":"session/prompt"'*)
            : > "$0/entered-$1"
            if [ -f "$0/defer-refusal-$1" ]; then
                while [ ! -f "$0/release-$1" ]; do /usr/bin/sleep .02; done
            fi
            if [ -f "$0/interrupt-$1" ]; then
                exit 0
            fi
            if [ -f "$0/refuse-$1" ]; then
                IFS= read -r error < "$0/refuse-$1"
                if [ -f "$0/notice-$1" ]; then
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"worker","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"untrusted provider prose\\n\\n"}}}}\n'
                fi
                if [ -f "$0/partial-$1" ]; then
                    printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"worker","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Partial work"}}}}\n'
                fi
                printf '{"jsonrpc":"2.0","id":%s,"error":%s}\n' "$id" "$error"
                continue
            fi
            while [ ! -f "$0/release-$1" ]; do /usr/bin/sleep .02; done
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id" ;;
    esac
done
"#;
            Ok(SessionRoute {
                launch: LaunchSpec {
                    engine: EngineKind::Codex,
                    executable: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        script.into(),
                        root.display().to_string(),
                        label.into(),
                    ],
                    cwd: root.into(),
                    native_home: root.into(),
                    credential_directory: root.into(),
                    base_environment: Default::default(),
                    codex_launcher: None,
                    confinement: None,
                },
                relay: "/bin/false".into(),
                broker_parent: root.join(if workspace_protocol::channel::current().is_dev() {
                    ".cortex-dev"
                } else {
                    ".cortex"
                }),
                connection: if label == "other" { "other" } else { "limited" }.into(),
                connection_limit: 1,
                model: "fixture-model".into(),
                parameters: Default::default(),
                instructions: "Local capacity fixture".into(),
                deadline: std::time::Duration::from_secs(5),
            })
        }
    }

    async fn seed_capacity_nodes(runtime: &Runtime) {
        runtime.host.executor.write(|db| {
            for id in ["second", "third", "other"] {
                db.execute(
                    "INSERT INTO runtime_nodes VALUES ('t',1,?,'worker-devops-engineer','Work','[]',?,'execution','pending',NULL,NULL)",
                    (id, json!([id]).to_string()),
                )?;
            }
            Ok(())
        }).unwrap().await.unwrap().unwrap();
    }

    async fn capacity_claims(runtime: &Runtime, owner: &OwnerEpoch, task: &str) -> Vec<Value> {
        seed_capacity_nodes(runtime).await;
        let claims = runtime
            .host
            .refill_execution(task.into(), owner.clone(), 1)
            .await
            .unwrap();
        assert_eq!(
            claims
                .iter()
                .map(|c| c["node"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["next", "second", "third", "other"]
        );
        claims
    }

    async fn capacity_wait_count(runtime: &Runtime) -> (i64, bool) {
        runtime.host.executor.read(|db| {
            Ok((db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='NATIVE_WORKER_CAPACITY_WAITING'", [], |row| row.get(0))?,
                cortex_core::projections::worker_capacity_wait(db, "t")?))
        }).unwrap().await.unwrap().unwrap()
    }

    struct CapacityBatch {
        order: Arc<WorkerLaunchOrder>,
        batch: watch::Sender<bool>,
        worker: watch::Sender<bool>,
    }
    impl CapacityBatch {
        fn new() -> Self {
            Self {
                order: Arc::default(),
                batch: watch::channel(false).0,
                worker: watch::channel(false).0,
            }
        }
        fn launch(
            &self,
            runtime: &Runtime,
            owner: &OwnerEpoch,
            claim: &Value,
        ) -> tokio::task::JoinHandle<CoreResult<crate::native_sessions::SessionDriver>> {
            let assignment: String = claim["assignment_ref"].as_str().unwrap().into();
            let ticket = self
                .order
                .push(assignment.clone(), runtime.host.capacity_changed.clone());
            let launch = WorkerLaunch::new(
                ticket,
                runtime.stop.subscribe(),
                self.batch.subscribe(),
                self.worker.subscribe(),
            );
            let route = CapacityPeer
                .route(&runtime.host.root, claim["node"].as_str().unwrap())
                .unwrap();
            let host = runtime.host.clone();
            let owner = owner.clone();
            tokio::spawn(async move {
                host.dispatch_with_wait(
                    "t".into(),
                    owner,
                    SessionRole::Worker { assignment },
                    route,
                    Some(launch),
                )
                .await
            })
        }
    }

    async fn launched(
        job: tokio::task::JoinHandle<CoreResult<crate::native_sessions::SessionDriver>>,
    ) -> crate::native_sessions::SessionDriver {
        tokio::time::timeout(std::time::Duration::from_secs(8), job)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    }

    async fn observe_waits(runtime: &Runtime, count: i64) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if capacity_wait_count(runtime).await == (count, true) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    fn starts(root: &Path) -> Vec<String> {
        std::fs::read_to_string(root.join("starts"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn provider_error(root: &Path, file: &str, code: i64, data: Value) {
        std::fs::write(root.join(file), format!("{}\n", json!({
            "code":code,"message":"untrusted provider prose","data":data,
        }))).unwrap();
    }

    fn provider_turn(
        runtime: &Arc<Runtime>,
        owner: &OwnerEpoch,
        claim: &Value,
        batch: &CapacityBatch,
        alternate_model: bool,
    ) -> tokio::task::JoinHandle<CoreResult<()>> {
        let assignment: String = claim["assignment_ref"].as_str().unwrap().into();
        let label = claim["node"].as_str().unwrap();
        let ticket = batch.order.push(assignment.clone(), runtime.host.capacity_changed.clone());
        let mut route = CapacityPeer.route(&runtime.host.root, label).unwrap();
        if alternate_model {
            route.connection = "limited".into();
            route.model = "fixture-alt".into();
            route.launch.args.push("fixture-alt".into());
        }
        let runtime = runtime.clone();
        let owner = owner.clone();
        let cancelled = batch.batch.subscribe();
        let prompt = claim.to_string();
        tokio::spawn(async move {
            runtime.turn_once("t", &owner, SessionRole::Worker { assignment }, route,
                &prompt, cancelled, Some(ticket)).await
        })
    }

    async fn provider_wait_facts(runtime: &Runtime) -> (i64, Option<&'static str>) {
        runtime.host.executor.read(|db| {
            Ok((db.query_row(
                "SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='NATIVE_PROVIDER_ROUTE_WAITING'",
                [], |row| row.get(0),
            )?, cortex_core::projections::provider_route_wait(db, "t")?))
        }).unwrap().await.unwrap().unwrap()
    }

    async fn observe_provider_wait(runtime: &Runtime, count: i64, reason: &'static str) {
        tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                if provider_wait_facts(runtime).await == (count, Some(reason)) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
    }

    async fn entered(root: &Path, label: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(8), async {
            while !root.join(format!("entered-{label}")).exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
    }

    async fn end_provider_turn(job: tokio::task::JoinHandle<CoreResult<()>>) -> CoreResult<()> {
        tokio::time::timeout(std::time::Duration::from_secs(8), job)
            .await.unwrap().unwrap()
    }

    const RATE_WAIT: &str = "Waiting for the provider's rate limit to reset";
    const QUOTA_WAIT: &str = "Waiting for the provider's usage quota to reset";

    fn expire_provider_route(runtime: &Runtime) {
        runtime.host.route_holds.expire_for_test(&RouteKey(
            "codex-acp", "limited".into(), "fixture-model".into(),
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn provider_hold_retries_handshake_and_first_prompt_on_same_route_without_blocking_sibling() {
        let (runtime, owner, task, root) = fixture().await;
        let claims = capacity_claims(&runtime, &owner, &task).await;
        let batch = CapacityBatch::new();
        provider_error(&root, "handshake-next", -32603,
            json!({"codexErrorInfo":"rateLimitExceeded","retryAfterSeconds":1800}));
        let next = provider_turn(&runtime, &owner, &claims[0], &batch, false);
        observe_provider_wait(&runtime, 1, RATE_WAIT).await;
        assert_eq!(starts(&root), ["next"]);
        // A different model on the very same one-slot connection can use it.
        let other = provider_turn(&runtime, &owner, &claims[3], &batch, true);
        entered(&root, "other").await;
        assert_eq!(starts(&root), ["next", "other"]);
        std::fs::write(root.join("release-other"), "").unwrap();
        end_provider_turn(other).await.unwrap();

        // The first probe reaches prompt and is temporarily refused again.
        std::fs::remove_file(root.join("handshake-next")).unwrap();
        provider_error(&root, "refuse-next", -32603,
            json!({"codexErrorInfo":"usageLimitExceeded","retryAfterSeconds":1800,
                "message":"untrusted provider prose"}));
        std::fs::write(root.join("notice-next"), "").unwrap();
        expire_provider_route(&runtime);
        observe_provider_wait(&runtime, 2, QUOTA_WAIT).await;
        assert_eq!(starts(&root), ["next", "other", "next"]);

        // An additional same-route assignment waits rather than launching.
        let second = provider_turn(&runtime, &owner, &claims[1], &batch, false);
        observe_provider_wait(&runtime, 3, QUOTA_WAIT).await;
        assert_eq!(starts(&root), ["next", "other", "next"]);
        std::fs::remove_file(root.join("refuse-next")).unwrap();
        for label in ["next", "second"] {
            std::fs::write(root.join(format!("release-{label}")), "").unwrap();
        }
        expire_provider_route(&runtime);
        end_provider_turn(next).await.unwrap();
        end_provider_turn(second).await.unwrap();
        assert_eq!(provider_wait_facts(&runtime).await, (3, None));
        assert_eq!(starts(&root), ["next", "other", "next", "next", "second"]);
        let retries = runtime.host.executor.read(|db| Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='NATIVE_PROVIDER_RETRY_CLAIMED'",
            [], |row| row.get::<_, i64>(0),
        )?)).unwrap().await.unwrap().unwrap();
        assert_eq!(retries, 2);
        tokio::task::spawn_blocking(move || drop(runtime)).await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn provider_hold_extension_blocks_probe_waiting_for_local_slot() {
        let (runtime, owner, task, root) = fixture().await;
        let claims = capacity_claims(&runtime, &owner, &task).await;
        let batch = CapacityBatch::new();
        std::fs::write(root.join("defer-refusal-next"), "").unwrap();
        provider_error(&root, "refuse-next", -32603,
            json!({"codexErrorInfo":"usageLimitExceeded","retryAfterSeconds":1800}));
        let running = provider_turn(&runtime, &owner, &claims[0], &batch, false);
        entered(&root, "next").await;

        // Seed an earlier refusal, then expire it without sleeping. The probe
        // takes ownership but cannot reserve the only slot on this connection.
        let mut earlier = RouteAttempt::new(runtime.host.route_holds.clone(),
            RouteKey("codex-acp", "limited".into(), "fixture-model".into()));
        earlier.limited(cortex_engine::ProviderLimit {
            kind: cortex_engine::LimitKind::Rate,
            retry_after_seconds: Some(1800),
            resets_at_unix_seconds: None,
        }, "earlier-refusal".into());
        expire_provider_route(&runtime);
        let probe = provider_turn(&runtime, &owner, &claims[1], &batch, false);
        observe_waits(&runtime, 1).await;
        assert_eq!(starts(&root), ["next"]);

        // The still-running request extends the hold before releasing its slot.
        // Both the probe and the refused worker's new claim must now wait.
        std::fs::write(root.join("release-next"), "").unwrap();
        observe_provider_wait(&runtime, 2, QUOTA_WAIT).await;
        assert_eq!(starts(&root), ["next"]);
        let other = provider_turn(&runtime, &owner, &claims[3], &batch, true);
        entered(&root, "other").await;
        assert_eq!(starts(&root), ["next", "other"]);
        std::fs::write(root.join("release-other"), "").unwrap();
        end_provider_turn(other).await.unwrap();

        std::fs::remove_file(root.join("refuse-next")).unwrap();
        std::fs::write(root.join("release-second"), "").unwrap();
        expire_provider_route(&runtime);
        end_provider_turn(probe).await.unwrap();
        end_provider_turn(running).await.unwrap();
        assert_eq!(starts(&root), ["next", "other", "second", "next"]);
        assert_eq!(provider_wait_facts(&runtime).await, (2, None));
        let assignment = claims[1]["assignment_ref"].as_str().unwrap().to_owned();
        let admitted = runtime.host.executor.read(move |db| {
            Ok(db.query_row(
                "SELECT count(*) FROM runtime_events WHERE task_id='t'
                 AND type='NATIVE_WORKER_CAPACITY_WAIT_ENDED'
                 AND json_extract(payload_json,'$.assignment_ref')=?
                 AND json_extract(payload_json,'$.outcome')='admitted'",
                [assignment], |row| row.get::<_, i64>(0),
            )?)
        }).unwrap().await.unwrap().unwrap();
        assert_eq!(admitted, 1);
        assert!(!capacity_wait_count(&runtime).await.1);
        tokio::task::spawn_blocking(move || drop(runtime)).await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn provider_hold_cancellation_and_durable_pause_prevent_retry_after_expiry() {
        for pause in [false, true] {
            let (runtime, owner, task, root) = fixture().await;
            let claims = capacity_claims(&runtime, &owner, &task).await;
            let batch = CapacityBatch::new();
            provider_error(&root, "refuse-next", -32603,
                json!({"codexErrorInfo":"rateLimitExceeded","retryAfterSeconds":1800}));
            let next = provider_turn(&runtime, &owner, &claims[0], &batch, false);
            observe_provider_wait(&runtime, 1, RATE_WAIT).await;
            if pause {
                runtime.host.executor.write(|db| {
                    db.execute("UPDATE runtime_tasks SET state='PAUSED' WHERE task_id='t'", [])?;
                    Ok(())
                }).unwrap().await.unwrap().unwrap();
            } else {
                batch.batch.send_replace(true);
            }
            expire_provider_route(&runtime);
            end_provider_turn(next).await.unwrap();
            assert_eq!(provider_wait_facts(&runtime).await, (1, None));
            assert_eq!(starts(&root), ["next"]);
            tokio::task::spawn_blocking(move || drop(runtime)).await.unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn provider_hold_resumes_a_coordinator_on_its_original_route() {
        let (runtime, owner, task, root) = fixture().await;
        runtime.host.executor.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'", [])?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        let mut route = CapacityPeer.route(&root, "coordinator").unwrap();
        route.launch.args.extend(["fixture-model".into(), "read-only".into()]);
        provider_error(&root, "refuse-coordinator", -32603,
            json!({"codexErrorInfo":"usageLimitExceeded","retryAfterSeconds":1800}));
        let (_cancel, cancelled) = watch::channel(false);
        let worker = runtime.clone();
        let job = tokio::spawn(async move {
            worker.turn_once(&task, &owner, SessionRole::Coordinator, route,
                "plan the work", cancelled, None).await
        });
        observe_provider_wait(&runtime, 1, QUOTA_WAIT).await;
        assert_eq!(starts(&root), ["coordinator"]);
        std::fs::remove_file(root.join("refuse-coordinator")).unwrap();
        std::fs::write(root.join("release-coordinator"), "").unwrap();
        expire_provider_route(&runtime);
        end_provider_turn(job).await.unwrap();
        assert_eq!(provider_wait_facts(&runtime).await, (1, None));
        assert_eq!(starts(&root), ["coordinator", "coordinator"]);
        tokio::task::spawn_blocking(move || drop(runtime)).await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn coordinator_transport_loss_stops_only_when_the_turn_did_not_commit_a_checkpoint() {
        enum Checkpoint {
            None,
            Plan,
            Synthesis,
        }

        for (checkpoint, initial, expected, expected_revision, expected_stop, expected_synthesis) in [
            (Checkpoint::None, "PLANNING", "RECOVERING", 1, 1, 0),
            (Checkpoint::Plan, "PLANNING", "PLAN_READY", 2, 0, 0),
            (Checkpoint::Synthesis, "RECONCILING", "RECONCILING", 1, 0, 1),
        ] {
            let (runtime, owner, task, root) = fixture().await;
            runtime
                .host
                .executor
                .write(move |db| {
                    db.execute(
                        "UPDATE runtime_tasks SET state=? WHERE task_id='t'",
                        [initial],
                    )?;
                    Ok(())
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            let mut route = CapacityPeer.route(&root, "coordinator").unwrap();
            route
                .launch
                .args
                .extend(["fixture-model".into(), "read-only".into()]);
            std::fs::write(root.join("interrupt-coordinator"), "").unwrap();
            std::fs::write(root.join("defer-refusal-coordinator"), "").unwrap();
            let (_cancel, cancelled) = watch::channel(false);
            let job = {
                let (runtime, owner, task, cancelled) = (
                    runtime.clone(),
                    owner.clone(),
                    task.clone(),
                    cancelled.clone(),
                );
                tokio::spawn(async move {
                    runtime
                        .turn(
                            &task,
                            &owner,
                            SessionRole::Coordinator,
                            route,
                            "plan the work",
                            cancelled,
                        )
                        .await
                })
            };
            entered(&root, "coordinator").await;
            // Commit the checkpoint (if any) before the peer drops the ACP reply.
            runtime
                .host
                .executor
                .write(move |db| {
                    match checkpoint {
                        Checkpoint::None => {}
                        Checkpoint::Plan => {
                            db.execute(
                                "UPDATE runtime_tasks SET state='PLAN_READY',revision=2 WHERE task_id='t'",
                                [],
                            )?;
                        }
                        Checkpoint::Synthesis => {
                            cortex_core::lifecycle::reconciliation::submit_synthesis_tx(
                                db,
                                "t",
                                &json!({"text":"Final synthesis","not_verified":[]}),
                            )?;
                        }
                    }
                    Ok(())
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            std::fs::write(root.join("release-coordinator"), "").unwrap();
            if let Err(error) = end_provider_turn(job).await {
                assert_eq!(error.code(), "native_turn_outcome_unknown");
                runtime
                    .stop_after_error(&task, &owner, error, cancelled)
                    .await;
            }
            let (state, revision, stops, syntheses): (String, i64, i64, i64) = runtime
                .host
                .executor
                .read(|db| {
                    Ok(db.query_row(
                        "SELECT state, revision,
                            (SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='LOOP_OBSERVED'
                             AND json_extract(payload_json,'$.stop')='native_turn_outcome_unknown'),
                            (SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='SYNTHESIS_SUBMITTED')
                         FROM runtime_tasks WHERE task_id='t'",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )?)
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                (state.as_str(), revision, stops, syntheses),
                (expected, expected_revision, expected_stop, expected_synthesis),
                "initial state: {initial}"
            );
            assert_eq!(starts(&root), ["coordinator"]);
            tokio::task::spawn_blocking(move || drop(runtime))
                .await
                .unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn provider_hold_does_not_replay_partial_or_hard_and_untyped_refusals() {
        for (file, code, data, partial) in [
            ("handshake-next", -32000, json!({"codexErrorInfo":"rateLimitExceeded"}), false),
            ("refuse-next", -32603, json!({"codexErrorInfo":"modelNotFound"}), false),
            ("refuse-next", -32603, json!({"errorKind":"billing_error"}), false),
            ("refuse-next", -32603, json!({"status":429,"retryAfterSeconds":1800}), false),
            ("refuse-next", -32603, json!({"codexErrorInfo":"rateLimitExceeded"}), true),
        ] {
            let (runtime, owner, task, root) = fixture().await;
            let claims = capacity_claims(&runtime, &owner, &task).await;
            let batch = CapacityBatch::new();
            provider_error(&root, file, code, data);
            if partial {
                std::fs::write(root.join("partial-next"), "").unwrap();
            }
            let next = provider_turn(&runtime, &owner, &claims[0], &batch, false);
            let result = end_provider_turn(next).await;
            assert_eq!(result.is_err(), file == "handshake-next");
            assert_eq!(provider_wait_facts(&runtime).await, (0, None));
            assert_eq!(starts(&root), ["next"]);
            tokio::task::spawn_blocking(move || drop(runtime)).await.unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_capacity_queue_starts_once_in_claim_order_and_leaves_other_connection_running() {
        let (runtime, owner, task, root) = fixture().await;
        let claims = capacity_claims(&runtime, &owner, &task).await;
        let batch = CapacityBatch::new();
        // The first contender is held at the service boundary until explicitly shut down.
        let first = launched(batch.launch(&runtime, &owner, &claims[0])).await;
        // Establish an inconclusive observation before any contender is polled.
        // Both wait notices must therefore be produced while it owns the slot.
        runtime.host.executor.write(|db| {
            let id: String = db.query_row(
                "SELECT reservation_id FROM capacity_reservations WHERE connection_id='limited'", [], |row| row.get(0)
            )?;
            assert!(cortex_core::scheduler::settle_tx(db, &id, cortex_core::ports::ProcessObservation::Unknown)?);
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        let second = batch.launch(&runtime, &owner, &claims[1]);
        let third = batch.launch(&runtime, &owner, &claims[2]);
        let other = launched(batch.launch(&runtime, &owner, &claims[3])).await;
        observe_waits(&runtime, 2).await;
        assert!(!second.is_finished() && !third.is_finished());
        assert_eq!(starts(&root), ["next", "other"]);

        first.shutdown().await.unwrap();
        let second = launched(second).await;
        assert_eq!(starts(&root), ["next", "other", "second"]);
        assert_eq!(capacity_wait_count(&runtime).await, (2, true));
        assert!(!third.is_finished());
        second.shutdown().await.unwrap();
        let third = launched(third).await;
        assert_eq!(starts(&root), ["next", "other", "second", "third"]);
        assert_eq!(capacity_wait_count(&runtime).await, (2, false));
        // 'other' was never shut down/restarted to admit either contender.
        let active: i64 = runtime.host.executor.read(|db| {
            Ok(db.query_row("SELECT count(*) FROM capacity_reservations WHERE connection_id='other' AND state='reserved'", [], |row| row.get(0))?)
        }).unwrap().await.unwrap().unwrap();
        assert_eq!(active, 1);
        third.shutdown().await.unwrap();
        other.shutdown().await.unwrap();
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_capacity_queue_cancellation_or_pause_prevents_launch_after_release() {
        for pause in [false, true] {
            let (runtime, owner, task, root) = fixture().await;
            let claims = capacity_claims(&runtime, &owner, &task).await;
            let batch = CapacityBatch::new();
            let first = launched(batch.launch(&runtime, &owner, &claims[0])).await;
            let waiter = batch.launch(&runtime, &owner, &claims[1]);
            observe_waits(&runtime, 1).await;
            if pause {
                // Put the durable pause fence in place without signalling the local
                // watch: this also covers release racing the parent's cancellation.
                runtime
                    .host
                    .executor
                    .write(|db| {
                        db.execute(
                            "UPDATE runtime_tasks SET state='PAUSED' WHERE task_id='t'",
                            [],
                        )?;
                        Ok(())
                    })
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();
            } else {
                batch.batch.send_replace(true);
            }
            first.shutdown().await.unwrap();
            match tokio::time::timeout(std::time::Duration::from_secs(8), waiter)
                .await
                .unwrap()
                .unwrap()
            {
                Err(error) => assert_eq!(error.code(), "native_task_interrupted"),
                Ok(driver) => {
                    driver.shutdown().await.unwrap();
                    panic!("a fenced contender started");
                }
            }
            assert_eq!(starts(&root), ["next"]);
            assert_eq!(capacity_wait_count(&runtime).await, (1, false));
            if pause {
                // After the owner resumes, the same claim waits again, visibly, and
                // is admitted exactly once when the new holder releases the slot.
                runtime
                    .host
                    .executor
                    .write(|db| {
                        db.execute(
                            "UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='t'",
                            [],
                        )?;
                        Ok(())
                    })
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();
                let resumed = CapacityBatch::new();
                let holder = launched(resumed.launch(&runtime, &owner, &claims[2])).await;
                let again = resumed.launch(&runtime, &owner, &claims[1]);
                observe_waits(&runtime, 2).await;
                assert_eq!(starts(&root), ["next", "third"]);
                holder.shutdown().await.unwrap();
                let again = launched(again).await;
                assert_eq!(starts(&root), ["next", "third", "second"]);
                assert_eq!(capacity_wait_count(&runtime).await, (2, false));
                again.shutdown().await.unwrap();
            }
            runtime.shutdown().await;
            drop(runtime);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_capacity_queue_keeps_the_live_scheduler_batch_running() {
        let (runtime, owner, task, root) = fixture_with_routes(Arc::new(CapacityPeer)).await;
        seed_capacity_nodes(&runtime).await;
        let (stop, cancelled) = watch::channel(false);
        let run = {
            let runtime = runtime.clone();
            tokio::spawn(async move { runtime.progress(&task, &owner, cancelled).await })
        };
        observe_waits(&runtime, 2).await;
        let prompt_entered = |node: &str| root.join(format!("entered-{node}")).is_file();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !(prompt_entered("next") && prompt_entered("other")) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        // Finish the first fake prompt without releasing the unrelated sibling.
        std::fs::write(root.join("release-next"), "").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !prompt_entered("second") {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let live: i64 = runtime.host.executor.read(|db| {
            Ok(db.query_row("SELECT count(*) FROM capacity_reservations WHERE connection_id='other' AND state='reserved'",
                [], |r| r.get(0))?)
        }).unwrap().await.unwrap().unwrap();
        assert_eq!(live, 1);
        assert!(!run.is_finished() && !prompt_entered("third"));

        stop.send_replace(true);
        let end = tokio::time::timeout(std::time::Duration::from_secs(8), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            end.code(),
            "native_task_interrupted" | "native_service_stopped"
        ));
        let mut seen = starts(&root);
        seen.sort();
        assert_eq!(seen, ["next", "other", "second"]);
        assert_eq!(capacity_wait_count(&runtime).await, (2, false));
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_capacity_queue_does_not_queue_the_top_level_start_cap() {
        let root =
            std::env::temp_dir().join(format!("capacity-cap-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(CapacityPeer))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let (release, wait) = watch::channel(false);
        runtime.jobs.lock().await.extend((0..32).map(|_| {
            let mut wait = wait.clone();
            tokio::spawn(async move {
                let _ = wait.changed().await;
            })
        }));
        let request = json!({"operation":"tasks.start","target":"draft","requestId":"cap",
            "projectId":"project","expectedVersion":"1","payload":{}});
        let rejected = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            runtime.client_command(
                "project".into(),
                request,
                Some(("incoming".into(), json!({}), None)),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(
            rejected.to_string().contains("native_capacity_unavailable"),
            "{rejected}"
        );
        assert_eq!(runtime.jobs.lock().await.len(), 32);
        release.send_replace(true);
        runtime.shutdown().await;
        assert!(!root.join("starts").exists());
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn strict_kind_route_refusal_is_audited_and_waits_without_launching() {
        use cortex_core::lifecycle::{automatic, domain};

        struct BlockedRoutes;
        impl NativeRoutes for BlockedRoutes {
            fn route(&self, _: &Path, _: &str) -> CoreResult<SessionRoute> {
                panic!("a blocked assignment must not launch or substitute a route")
            }

            fn worker_selection(
                &self,
                _: &Path,
                _: &str,
                _: &Value,
                _: &cortex_core::routing::Evidence,
                _: bool,
                _: Option<&str>,
                context: &mut crate::native_routes::WorkerRouteContext,
            ) -> CoreResult<(SessionRoute, Value)> {
                context.block = json!({"work_kind":"operations","target_tier":"standard",
                    "owner_list_exists":true,
                    "message":"No usable model is listed for operations work (target tier: standard)."});
                Err(CoreError::Refused(automatic::NO_MODEL_LISTED_FOR_WORK_KIND))
            }
        }

        let (runtime, owner, task, root) = fixture_with_routes(Arc::new(BlockedRoutes)).await;
        let assignments = runtime
            .host
            .refill_execution(task.clone(), owner.clone(), 1)
            .await
            .unwrap();
        let assignment = &assignments[0];
        let assignment_ref = assignment["assignment_ref"].as_str().unwrap();
        let order = Arc::new(WorkerLaunchOrder::default());
        let ticket = order.push(assignment_ref.into(), runtime.host.capacity_changed.clone());
        let (_stop, cancelled) = watch::channel(false);
        let refusal = runtime
            .run_assignment(&task, &owner, assignment, cancelled, ticket)
            .await;
        assert_eq!(
            refusal.unwrap_err().code(),
            automatic::NO_MODEL_LISTED_FOR_WORK_KIND
        );

        runtime.host.executor.write(move |db| {
            assert_eq!(
                domain::stop_tx(db, &task, automatic::NO_MODEL_LISTED_FOR_WORK_KIND)?,
                cortex_core::TaskState::Recovering
            );
            let owed = automatic::owed(db, &task)?.expect("durable route wait");
            assert!(owed.external);
            assert_eq!(owed.evidence["strategy"], "retry_unblocked");
            let probe = &owed.evidence["worker_route"];
            assert_eq!(probe["escalated"], false);
            assert_eq!(probe["work_kind_block"]["work_kind"], "operations");
            assert_eq!(probe["work_kind_block"]["target_tier"], "standard");
            assert_eq!(
                crate::notifier::progress::reasons(
                    &[crate::notifier::progress::Event {
                        kind: "NATIVE_WORKER_ROUTE_BLOCKED".into(),
                        payload: probe.clone(),
                        at: 0,
                    }],
                    None,
                ),
                ["no usable model listed for operations work (target tier: standard)"]
            );
            let selections: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='NATIVE_ROUTE_SELECTED'",
                [&task], |row| row.get(0),
            )?;
            assert_eq!(selections, 0);
            Ok(())
        }).unwrap().await.unwrap().unwrap();

        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refill_rejects_stale_revision_without_issuing() {
        let (runtime, owner, task, root) = fixture().await;
        let out = runtime
            .host
            .refill_execution(task.clone(), owner, 2)
            .await
            .unwrap();
        assert!(out.is_empty());
        let status = runtime
            .host
            .executor
            .read(move |db| {
                Ok(db.query_row(
                    "SELECT status FROM runtime_nodes WHERE task_id='t' AND node_id='next'",
                    [],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status, "pending");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refill_claims_once_and_spends_budget_only_for_claim() {
        let (runtime, owner, task, root) = fixture().await;
        let first = runtime
            .host
            .refill_execution(task.clone(), owner.clone(), 1)
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        let second = runtime
            .host
            .refill_execution(task.clone(), owner, 1)
            .await
            .unwrap();
        assert!(second.is_empty());
        let used: i64 = runtime.host.executor.read(|db| Ok(db.query_row(
            "SELECT json_extract(result_json,'$.used') FROM runtime_requests WHERE task_id='t' AND actor='native-progress'", [], |r| r.get(0))?)).unwrap().await.unwrap().unwrap();
        assert_eq!(used, 1);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refill_does_not_claim_while_steering_is_pending() {
        let (runtime, owner, task, root) = fixture().await;
        runtime
            .host
            .executor
            .write(|db| {
                cortex_core::lifecycle::event_tx(
                    db,
                    "t",
                    "USER_STEERING",
                    json!({"request":"r","delivery":"boundary"}),
                )
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let out = runtime.host.refill_execution(task, owner, 1).await.unwrap();
        assert!(out.is_empty());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
pub(crate) fn retry_window() -> cortex_core::scheduler::retry::RetryWindow {
    RETRY_WINDOW
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unwrap_or_default()
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The models (`{engine, model}`) on which `node`'s lineage (the node and its review
/// fixes, in every plan revision since the task was last reopened) was lost to
/// infrastructure at least twice: a fresh attempt is routed away from them.
pub fn lost_models(db: &rusqlite::Connection, task: &str, node: &str) -> CoreResult<Vec<Value>> {
    use cortex_core::scheduler::retry::{infrastructure_class, node_base};
    let rows = db
        .prepare(
            "SELECT json_extract(r.payload_json,'$.node'),coalesce(json_extract(r.payload_json,'$.failure_class'),''),
                json_extract(s.payload_json,'$.route_engine'),json_extract(s.payload_json,'$.model')
             FROM runtime_events r JOIN runtime_events s ON s.task_id=r.task_id AND s.type='NATIVE_ROUTE_SELECTED'
               AND json_extract(s.payload_json,'$.assignment_ref')=json_extract(r.payload_json,'$.previous_assignment')
             WHERE r.task_id=?1 AND r.type='ASSIGNMENT_RETRIED' AND json_extract(r.payload_json,'$.failed_at') IS NOT NULL
               AND r.id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)",
        )?
        .query_map([task], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let base = node_base(node);
    let mut counts: std::collections::BTreeMap<(Option<String>, String), usize> =
        Default::default();
    for (lost, class, engine, model) in rows {
        let (Some(lost), Some(model)) = (lost, model) else {
            continue;
        };
        if node_base(&lost) != base
            || !infrastructure_class(&class)
            || matches!(class.as_str(), "owner_paused" | "runtime_restart")
        {
            continue;
        }
        *counts.entry((engine, model)).or_default() += 1;
    }
    Ok(counts
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|((engine, model), n)| json!({"engine":engine,"model":model,"losses":n}))
        .collect())
}
/// At most this many characters (the last lines) of an engine's already redacted
/// stderr tail are kept with an attempt's end.
pub const ENDING_STDERR_CHARS: usize = 400;
/// The last lines of an engine's redacted stderr tail, bounded by
/// [`ENDING_STDERR_CHARS`] characters.
pub fn stderr_tail(redacted: &str) -> String {
    let chars: Vec<char> = redacted.trim_end().chars().collect();
    let start = chars.len().saturating_sub(ENDING_STDERR_CHARS);
    chars[start..].iter().collect()
}
/// Facts the store holds about how one worker attempt ended: the results it had
/// refused (count and the last code), a runtime cancel of one of its permission
/// requests under the read-only rule, a degenerate turn, an owner pause after it
/// started, and whether its last command could not get a sandbox.
pub fn worker_ending_facts(
    db: &rusqlite::Connection,
    task: &str,
    channel: &str,
    attempt: &str,
) -> CoreResult<Value> {
    use rusqlite::OptionalExtension;
    // A refusal because the checkout kept changing (`review::DRIFT_REFUSALS`) is the
    // environment's, never the worker's quality.
    let (refused, drift, last): (i64, i64, Option<String>) = db.query_row(
        "SELECT coalesce(sum(coalesce(json_extract(payload_json,'$.code'),'') NOT IN ('review_project_revision_changed','review_project_revision_unstable','review_resubmit_required')),0),
            coalesce(sum(coalesce(json_extract(payload_json,'$.code'),'') IN ('review_project_revision_changed','review_project_revision_unstable','review_resubmit_required')),0),
            (SELECT json_extract(payload_json,'$.code') FROM runtime_events WHERE task_id=?1
            AND type='WORKER_RESULT_REFUSED' AND json_extract(payload_json,'$.attempt')=?2 ORDER BY id DESC LIMIT 1)
         FROM runtime_events WHERE task_id=?1 AND type='WORKER_RESULT_REFUSED' AND json_extract(payload_json,'$.attempt')=?2",
        (task, attempt),
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let cancelled: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_PERMISSION_DECIDED'
         AND json_extract(payload_json,'$.channel_id')=? AND json_extract(payload_json,'$.decider')='runtime'
         AND json_extract(payload_json,'$.outcome.optionId')='cancel')",
        (task, channel),
        |r| r.get(0),
    )?;
    let degenerate: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_TURN_DEGENERATE'
         AND json_extract(payload_json,'$.channel_id')=?)",
        (task, channel),
        |r| r.get(0),
    )?;
    let paused: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events p WHERE p.task_id=?1 AND p.type='OWNER_TASK_PAUSED'
         AND p.id>coalesce((SELECT min(id) FROM runtime_events WHERE task_id=?1 AND type='NATIVE_ATTEMPT_DELIVERY'
            AND json_extract(payload_json,'$.attempt')=?2),
          (SELECT max(id) FROM runtime_events WHERE task_id=?1)))",
        (task, attempt),
        |r| r.get(0),
    )?;
    let last_failure: Option<String> = db
        .query_row(
            "SELECT coalesce(json_extract(result_json,'$.failure'),json_extract(result_json,'$.reply.structuredContent.failure'))
             FROM runtime_requests WHERE task_id=? AND actor=? AND operation IN ('command_run','outside_command')
             ORDER BY rowid DESC LIMIT 1",
            (task, channel),
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let sandbox = last_failure.as_deref().is_some_and(|f| {
        f.contains("sandbox_unavailable") || f.starts_with("git_context_unavailable")
    });
    Ok(
        json!({"refused_results":refused,"drift_refusals":drift,"last_refusal":last,
        "runtime_cancel":if cancelled {json!("read_only_permission")} else {Value::Null},
        "degenerate":degenerate,"owner_paused":paused,"sandbox_unavailable":sandbox}),
    )
}
/// The failure class of a worker attempt that ended without an accepted result, from
/// how its turn ended (`cause`) and [`worker_ending_facts`]. Only `result_refused`,
/// `degenerate_turn` and `indeterminate` are the attempt's own quality; every other
/// class is infrastructure (`cortex_core::scheduler::retry::INFRASTRUCTURE_CLASSES`).
pub fn worker_ending_class(cause: &str, facts: &Value) -> &'static str {
    let refused = facts["refused_results"].as_i64().unwrap_or(0) > 0;
    let sandbox = facts["sandbox_unavailable"] == true;
    match cause {
        "native_turn_degenerate" => "degenerate_turn",
        _ if facts["degenerate"] == true => "degenerate_turn",
        "owner_stopped" => "owner_paused",
        "native_task_interrupted" if facts["owner_paused"] == true => "owner_paused",
        "native_service_stopped" | "native_task_interrupted" => "runtime_restart",
        // The model ended its turn: refused results without an accepted one are its own.
        "turn_ended" if refused => "result_refused",
        "turn_ended" if sandbox => "sandbox_unavailable",
        // Only refused for another actor's checkout drift: retried on the runtime's budget.
        "turn_ended" if facts["drift_refusals"].as_i64().unwrap_or(0) > 0 => "worker_lost",
        "turn_ended" => "indeterminate",
        // The process vanished without the model ending its turn.
        "process_exited" if sandbox => "sandbox_unavailable",
        "process_exited" => "worker_lost",
        code if code.ends_with("outcome_unknown") || code.contains("transport") => {
            "transport_failure"
        }
        code if code.contains("authentication") => "authorization_loss",
        _ => "engine_failure",
    }
}

/// Per-task backoff of environment rechecks for open owner actions (in memory: a
/// restart rechecks once at once, which is harmless).
static ENVIRONMENT_RECHECKS: std::sync::Mutex<
    Option<std::collections::HashMap<String, (std::time::Instant, usize)>>,
> = std::sync::Mutex::new(None);

fn environment_recheck_due(task: &str) -> bool {
    let mut map = ENVIRONMENT_RECHECKS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(Default::default);
    let now = std::time::Instant::now();
    match map.get(task).copied() {
        Some((at, n))
            if now.duration_since(at).as_secs()
                < cortex_core::lifecycle::obstacles::backoff(n.saturating_sub(1)) as u64 =>
        {
            false
        }
        Some((_, n)) => {
            map.insert(task.to_owned(), (now, n + 1));
            true
        }
        None => {
            map.insert(task.to_owned(), (now, 1));
            true
        }
    }
}

/// One public address for this attempt, or refuse the entire DNS answer if
/// it contains a non-public destination. Do not reinterpret mapped/tunnel
/// addresses or scope identifiers as public IPv6 addresses.
fn pinned_external_address(
    addresses: impl IntoIterator<Item = std::net::SocketAddr>,
) -> Result<std::net::SocketAddr, &'static str> {
    use std::net::SocketAddr;
    let mut first = None;
    for address in addresses {
        let public = match address {
            SocketAddr::V4(addr) => {
                let ip = addr.ip();
                let [a, b, c, _] = ip.octets();
                !ip.is_private()
                    && !ip.is_loopback()
                    && !ip.is_link_local()
                    && !ip.is_documentation()
                    && !matches!(
                        (a, b, c),
                        (0 | 224..=255, _, _)
                            | (100, 64..=127, _)
                            | (192, 0, 0)
                            | (192, 88, 99)
                            | (198, 18..=19, _)
                    )
            }
            SocketAddr::V6(addr) => {
                let [a, b, ..] = addr.ip().segments();
                addr.scope_id() == 0
                    && (0x2000..=0x3fff).contains(&a)
                    // Protocol assignments (incl. Teredo), documentation, and 6to4.
                    && !matches!((a, b), (0x2001, 0..=0x01ff | 0x0db8) | (0x2002, _) | (0x3fff, _))
            }
        };
        if !public || address.port() != 443 {
            return Err("outage probe refused a non-public or mismatched DNS address");
        }
        first.get_or_insert(address);
    }
    first.ok_or("outage probe DNS returned no addresses")
}

fn configure_external_head(
    command: &mut tokio::process::Command,
    host: &str,
    address: std::net::SocketAddr,
) {
    let port = address.port();
    let ip = match address.ip() {
        std::net::IpAddr::V4(ip) => ip.to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
    };
    command
        .env_clear()
        .args([
            "-q", // Must be first: even with no HOME, curl may otherwise read curlrc.
            "--silent",
            "--head",
            "--output",
            "/dev/null",
            "--max-time",
            "10",
            "--proto",
            "=https",
            "--noproxy",
            "*",
            "--no-location",
            "--resolve",
            &format!("{host}:{port}:{ip}"),
            "--write-out",
            "%{http_code}",
            "--",
            &format!("https://{host}:{port}/"),
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
}

/// A wait (including a retained wait from a previous version) is not proof of
/// destination authority. Validate before any DNS lookup and bound the whole
/// probe, including DNS, to 10 seconds.
pub(crate) async fn probe_external(wait: &Value) -> (bool, String) {
    probe_external_with_lookup(wait, |host| tokio::net::lookup_host((host, 443))).await
}

async fn probe_external_with_lookup<A, L, F>(wait: &Value, lookup: L) -> (bool, String)
where
    A: IntoIterator<Item = std::net::SocketAddr>,
    L: FnOnce(&'static str) -> F,
    F: std::future::Future<Output = io::Result<A>>,
{
    use cortex_core::lifecycle::obstacles::external_probe_host;
    let host = wait["host"].as_str().unwrap_or("");
    let scheme = wait["scheme"].as_str().unwrap_or("");
    let port = wait["port"].as_u64().and_then(|p| u16::try_from(p).ok());
    let Some(host) = port.and_then(|p| external_probe_host(host, scheme, p)) else {
        return (false, "outage probe destination unsupported".into());
    };
    let Some(probe @ ("tcp" | "http")) = wait["probe"].as_str() else {
        return (false, "outage probe kind unsupported".into());
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let addresses = lookup(host).await.map_err(|_| "outage probe DNS failed")?;
        let address = pinned_external_address(addresses)?;
        if probe == "tcp" {
            tokio::net::TcpStream::connect(address)
                .await
                .map_err(|_| "outage probe connect failed")?;
            return Ok((true, format!("{host}:443 accepts connections")));
        }
        // The URL retains the fixed TLS server name; --resolve supplies the
        // exact checked address, so curl neither looks it up again nor uses a proxy.
        let mut command = tokio::process::Command::new("/usr/bin/curl");
        configure_external_head(&mut command, host, address);
        let out = command
            .output()
            .await
            .map_err(|_| "outage HEAD unavailable")?;
        if !out.status.success() {
            return Err("outage HEAD failed");
        }
        let code = std::str::from_utf8(&out.stdout)
            .ok()
            .and_then(|status| status.trim().parse::<u16>().ok())
            .filter(|code| (200..600).contains(code))
            .ok_or("outage HEAD returned no final HTTP status")?;
        Ok((code < 500 && code != 429, format!("HTTP {code}")))
    })
    .await;
    match result {
        Ok(Ok(result)) => result,
        Ok(Err(detail)) => (false, detail.into()),
        Err(_) => (false, "outage probe timed out".into()),
    }
}

#[cfg(test)]
mod external_probe_tests {
    use super::*;
    use std::{future::ready, net::SocketAddr};

    #[tokio::test]
    async fn unsupported_stored_targets_do_not_reach_dns() {
        for (host, scheme, port, probe) in [
            ("untrusted.example", "https", 443, "http"),
            ("api.github.com.untrusted.example", "https", 443, "tcp"),
            ("api.github.com", "http", 443, "http"),
            ("api.github.com", "https", 8443, "tcp"),
            ("api.github.com", "https", 65_979, "http"),
            ("api.github.com", "https", 443, "unknown"),
        ] {
            let wait = json!({"host":host,"scheme":scheme,"port":port,"probe":probe});
            let result = probe_external_with_lookup(
                &wait,
                |_| -> std::future::Ready<io::Result<Vec<SocketAddr>>> {
                    panic!("unsupported wait must not resolve");
                },
            )
            .await;
            assert!(!result.0);
            assert!(result.1.ends_with("unsupported"));
        }
    }

    #[tokio::test]
    async fn allowed_hostname_with_non_public_dns_cannot_connect() {
        let wait = json!({"host":"api.github.com","scheme":"https","port":443,"probe":"http"});
        for address in [
            "0.0.0.0:443",
            "10.0.0.1:443",
            "127.0.0.1:443",
            "169.254.169.254:443",
            "172.16.0.1:443",
            "192.168.0.1:443",
            "100.64.0.1:443",
            "[::]:443",
            "[::1]:443",
            "[::ffff:127.0.0.1]:443",
            "[fe80::1]:443",
            "[fc00::1]:443",
            "[ff02::1]:443",
            "[64:ff9b::a00:1]:443",
        ] {
            let result = probe_external_with_lookup(&wait, |host| {
                assert_eq!(host, "api.github.com");
                // Even a mixed response cannot authorize a connection.
                ready(Ok([
                    "1.1.1.1:443".parse::<SocketAddr>().unwrap(),
                    address.parse::<SocketAddr>().unwrap(),
                ]))
            })
            .await;
            assert_eq!(
                result,
                (
                    false,
                    "outage probe refused a non-public or mismatched DNS address".into()
                ),
                "{address}"
            );
        }
    }

    #[test]
    fn external_probe_selects_only_a_numeric_public_address() {
        let first: SocketAddr = "[2606:4700::1]:443".parse().unwrap();
        let second: SocketAddr = "1.1.1.1:443".parse().unwrap();
        assert_eq!(pinned_external_address([first, second]), Ok(first));
        assert!(pinned_external_address([]).is_err());
        assert!(pinned_external_address(["1.1.1.1:8443".parse().unwrap()]).is_err());
    }

    #[tokio::test]
    async fn head_uses_a_config_free_client_with_an_exact_dns_pin() {
        for (address, pin) in [
            ("1.1.1.1:443", "api.github.com:443:1.1.1.1"),
            ("[2606:4700::1]:443", "api.github.com:443:[2606:4700::1]"),
        ] {
            let mut command = tokio::process::Command::new("/usr/bin/curl");
            configure_external_head(&mut command, "api.github.com", address.parse().unwrap());
            let command = command.as_std();
            assert_eq!(command.get_program(), "/usr/bin/curl");
            assert_eq!(command.get_envs().count(), 0);
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [
                    "-q",
                    "--silent",
                    "--head",
                    "--output",
                    "/dev/null",
                    "--max-time",
                    "10",
                    "--proto",
                    "=https",
                    "--noproxy",
                    "*",
                    "--no-location",
                    "--resolve",
                    pin,
                    "--write-out",
                    "%{http_code}",
                    "--",
                    "https://api.github.com:443/",
                ]
            );
        }

        // Execute only a local environment oracle, not curl. printenv treats
        // our HEAD arguments as further names after its own end-of-options.
        let mut oracle = tokio::process::Command::new("/usr/bin/printenv");
        oracle
            .args(["--", "HOME", "PATH", "HTTPS_PROXY", "CORTEX_TEST_SENTINEL"])
            .env("CORTEX_TEST_SENTINEL", "synthetic");
        configure_external_head(
            &mut oracle,
            "api.github.com",
            "1.1.1.1:443".parse().unwrap(),
        );
        let out = oracle.output().await.unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
    }
}

/// Trusted embedding authorization, not a route candidate or desktop JSON.
/// Implementations must qualify every returned engine/account/model and role.
/// Generate the absent first-run profile from routing/defaults/MCP/connections
/// bytes. The caller owns the configuration lock and no-replacement publication;
/// this function has no filesystem or engine effects. Custom settings return None.
pub fn seed_default_profile(
    bytes: &[Option<Vec<u8>>; 4],
    now: &str,
) -> io::Result<Option<Vec<u8>>> {
    crate::machine::seed_default_profile(bytes, now)
}

pub trait NativeRoutes: Send + Sync {
    fn default_language(&self) -> Option<String> {
        None
    }
    /// Persist a pre-task language choice as the user-wide default. Returns
    /// false when this provider owns no user defaults (the choice stays
    /// session-local); an error means nothing was persisted.
    fn persist_default_language(&self, _language: Option<&str>) -> CoreResult<bool> {
        Ok(false)
    }
    /// Persist a pre-task coordinator `model`/`thought_level` choice (`option`)
    /// as the user-default route, from the session's `previous` and new
    /// `settings`. Same contract as `persist_default_language`.
    fn persist_default_route(
        &self,
        _root: &Path,
        _previous: &Value,
        _settings: &Value,
        _option: &str,
    ) -> CoreResult<bool> {
        Ok(false)
    }
    fn route(&self, root: &Path, role: &str) -> CoreResult<SessionRoute>;
    /// The exact coordinator route a task starting with `settings` resolves to, as
    /// a durable pin `{route_id, parameters}`. Persisted at start so a later change
    /// of the owner default never re-routes a running task. None: not pinnable.
    fn coordinator_pin(&self, _root: &Path, _settings: &Value) -> CoreResult<Option<Value>> {
        Ok(None)
    }
    /// The session route of a pin from `coordinator_pin`. `effort` is the task's
    /// explicit thought-level choice, if any.
    fn pinned_route(
        &self,
        root: &Path,
        _pin: &Value,
        _effort: Option<&str>,
    ) -> CoreResult<SessionRoute> {
        self.route(root, "orchestrator")
    }
    /// A durable coordinator pin `{route_id, parameters}` for another qualified model
    /// than `current` (preferably another engine or account), for the no-stop policy's
    /// change of approach (a degenerate model, the `reassign` rung, an unavailable
    /// coordinator route). None: no other model qualifies.
    fn alternate_coordinator_pin(
        &self,
        _root: &Path,
        _current: &SessionRoute,
    ) -> CoreResult<Option<Value>> {
        Ok(None)
    }
    fn worker_route(
        &self,
        root: &Path,
        role: &str,
        _node: &Value,
        _evidence: &cortex_core::routing::Evidence,
        _escalated: bool,
        _previous: Option<&str>,
    ) -> CoreResult<SessionRoute> {
        self.route(root, role)
    }
    fn worker_selection(
        &self,
        root: &Path,
        role: &str,
        node: &Value,
        evidence: &cortex_core::routing::Evidence,
        escalated: bool,
        previous: Option<&str>,
        _context: &mut crate::native_routes::WorkerRouteContext,
    ) -> CoreResult<(SessionRoute, Value)> {
        Ok((
            self.worker_route(root, role, node, evidence, escalated, previous)?,
            json!({}),
        ))
    }
    fn menu(&self, _root: &Path, _evidence: &cortex_core::routing::Evidence) -> CoreResult<Value> {
        Err(CoreError::Refused("native_route_menu_unavailable"))
    }
    fn previews(
        &self,
        _root: &Path,
        _plan: &Value,
        _evidence: &cortex_core::routing::Evidence,
    ) -> CoreResult<Value> {
        Ok(json!([]))
    }
    fn mcp_registry(&self) -> CoreResult<crate::native_mcp::Registry> {
        Ok(crate::native_mcp::Registry::default())
    }
    /// Only host-qualified identities may be exposed to an incoming client.
    fn options(&self, root: &Path, settings: &Value) -> Value {
        match self.task_route(root, settings) {
            Ok(route) => json!([{"id":"model","name":"Model","category":"model","type":"select",
                "currentValue":route.model,"options":[{"value":route.model,"name":route.model}]}]),
            Err(_) => json!([]),
        }
    }
    /// Selectable routes `{route, model, label, disambiguated_label}` for
    /// `/cortex-model` resolution.
    fn listing(&self, _root: &Path) -> Vec<Value> {
        Vec::new()
    }
    /// Every registered connection's persisted state, in registry order.
    fn connection_states(&self) -> Vec<String> {
        Vec::new()
    }
    /// Registered connection ids, in registry order.
    fn connection_ids(&self) -> Vec<String> {
        Vec::new()
    }
    /// A new, prompt-free qualified observation of the same connection.
    fn authentication_observed_after(
        &self,
        _root: &Path,
        _connection: &str,
        _after: &str,
    ) -> CoreResult<bool> {
        Ok(false)
    }
    /// Automatic coordination respects the same observed quota as automatic worker routing.
    fn automatic_coordinator_health(&self, _root: &Path, _route: &SessionRoute) -> CoreResult<()> {
        Ok(())
    }
    /// One prompt-free catalogue refresh of a registered connection (Python's
    /// per-session `_refresh_catalog_for_session`); true only when a new
    /// observation was published. Never a model prompt.
    fn refresh_catalog(&self, _connection: &str) -> bool {
        false
    }
    /// The route a `provider`/`model`/`thought_level` choice selects (Python
    /// `_apply_config_selection`): `{route, model, connection, echo}`, where
    /// `echo` marks a pre-task re-selection of the saved or pending route.
    fn select(&self, _root: &Path, settings: &Value, id: &str, value: &str) -> CoreResult<Value> {
        let route = if id == "thought_level" {
            settings["model"].clone()
        } else {
            json!(value)
        };
        Ok(json!({"route":route,"model":route,"connection":null,"echo":false}))
    }
    fn task_route(&self, root: &Path, settings: &Value) -> CoreResult<SessionRoute> {
        let route = self.route(root, "orchestrator")?;
        if settings["model"]
            .as_str()
            .is_some_and(|m| !m.is_empty() && m != route.model)
            || settings["effort"].as_str().is_some_and(|e| {
                !e.is_empty()
                    && route.parameters.get("thought_level").map(String::as_str) != Some(e)
            })
        {
            return Err(CoreError::Refused("native_task_route_unavailable"));
        }
        Ok(route)
    }
}
/// `history.page` is served from the core projection (see `Service::workspace`
/// and `native.query`); without it a client never reads the task conversation.
pub(crate) fn capabilities() -> io::Result<Value> {
    let mut value = workspace_protocol::capabilities(
        "workspace-service-native-1.0.6",
        "1",
        &[
            "capabilities.get",
            "tasks.create",
            "tasks.start",
            "decisions.list",
            "decisions.get",
            "decisions.answer",
            "history.page",
            "reports.read",
        ],
    )
    .map_err(|_| failure("invalid_capabilities"))?;
    // Retained, delivered files: Start, owner messages (also Request changes) and answers
    // keep the bodies privately and the engine prompt carries them (`native_attachments`).
    // Worker guidance does not carry files; its composer says so itself.
    value["attachments"] = json!(true);
    workspace_protocol::validate("Capabilities", &value)
        .map_err(|_| failure("invalid_capabilities"))?;
    Ok(value)
}
pub struct UnqualifiedRoutes;
impl NativeRoutes for UnqualifiedRoutes {
    fn route(&self, _: &Path, _: &str) -> CoreResult<SessionRoute> {
        Err(CoreError::Refused("native_route_authorization_unavailable"))
    }
}
/// The answer to one request to send an Automatic task back to its coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepairAdmission {
    /// The task was reopened and its coordinator continues it.
    Started,
    /// Nothing was decided now (another control, maintenance, or the service stopping).
    Busy,
    /// Not admitted: already spent, bounded, or the task is not in the expected state.
    Declined,
}
pub(crate) struct Runtime {
    pub(super) host: Arc<NativeHost>,
    pub(super) routes: Arc<dyn NativeRoutes>,
    pub(super) controls: Mutex<()>,
    pub(super) command_controls: Mutex<()>,
    pumps: Mutex<HashMap<String, Pump>>,
    pub(super) guidance: Mutex<HashMap<String, tokio::sync::mpsc::Sender<Value>>>,
    pub(super) worker_stops: Mutex<HashMap<String, (watch::Sender<bool>, watch::Receiver<bool>)>>,
    pub(super) wake: tokio::sync::Notify,
    pub(crate) workspace_updates: Option<tokio::sync::broadcast::Sender<Value>>,
    jobs: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    pub(super) stop: watch::Sender<bool>,
    /// Backoff of the live re-check of unconfirmed exits (`native_recovery`).
    pub(super) exit_recheck: std::sync::Mutex<crate::native_recovery::ExitRecheck>,
    /// Backoff of the live re-settle of orphaned command receipts (`native_recovery`).
    pub(super) command_recheck: std::sync::Mutex<crate::native_recovery::CommandRecheck>,
    /// Whether a driver's end starts a continuation follower (the no-stop policy).
    /// Only scheduler fixtures that drive every pass themselves turn it off.
    pub(super) followers: std::sync::atomic::AtomicBool,
}
struct Pump {
    cancel: watch::Sender<bool>,
    done: watch::Receiver<bool>,
}
struct TurnMarker {
    state_and_revision: (String, i64),
    has_synthesis: bool,
}
fn err(e: CoreError) -> io::Error {
    failure(e.code())
}
pub(crate) fn key(root: &Path) -> CoreResult<ProjectKey> {
    ProjectKey::new(
        root,
        if workspace_protocol::channel::current().is_dev() {
            StoreChannel::Dev
        } else {
            StoreChannel::Release
        },
    )
}
/// Incoming history-only ingress still refuses unadopted legacy projects.
pub(crate) fn legacy(root: &Path) -> CoreResult<bool> {
    let key = key(root)?;
    if !key.database().try_exists()? {
        return Ok(false);
    }
    Store::open(key, OpenMode::LiveReadOnly)?.read(|db| Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_tasks t WHERE NOT EXISTS(SELECT 1 FROM runtime_events e
         WHERE e.task_id=t.task_id AND e.type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED') AND json_extract(e.payload_json,'$.execution_host')='rust'))",
        [], |r| r.get(0))?))
}
impl Runtime {
    #[cfg(test)]
    pub(crate) fn open(
        root: &Path,
        project: &str,
        routes: Arc<dyn NativeRoutes>,
    ) -> CoreResult<Self> {
        Self::open_with(root, project, routes, false, None)
    }
    /// Incoming ACP agents may share a project: see `StoreExecutor::open_shared`.
    pub(crate) fn open_shared(
        root: &Path,
        project: &str,
        routes: Arc<dyn NativeRoutes>,
    ) -> CoreResult<Self> {
        Self::open_with(root, project, routes, true, None)
    }
    fn open_with(
        root: &Path,
        project: &str,
        routes: Arc<dyn NativeRoutes>,
        shared: bool,
        workspace_state: Option<PathBuf>,
    ) -> CoreResult<Self> {
        let host = match (shared, workspace_state) {
            (true, None) => NativeHost::open_shared_for_recovery(key(root)?, routes.mcp_registry()?)?,
            (false, Some(state)) => NativeHost::open_workspace(key(root)?, routes.mcp_registry()?, state)?,
            (false, None) => NativeHost::open_for_recovery(key(root)?, routes.mcp_registry()?)?,
            (true, Some(_)) => return Err(CoreError::Refused("invalid_workspace_host")),
        };
        *host.routes.write().unwrap() = Some(routes.clone());
        let project = project.to_owned();
        let root = root.to_string_lossy().into_owned();
        host.executor.write(move |db| {
            let actor = actor();
            db.execute("INSERT OR IGNORE INTO workspace_projects VALUES (?,?,?, ?,1)",
                (&project,&root,&actor,cortex_core::now()))?;
            let valid: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM workspace_projects WHERE project_id=? AND project_root=? AND activated_by=?)",
                (&project,&root,&actor), |r| r.get(0))?;
            if !valid { return Err(CoreError::Refused("project_context_conflict")); }
            Ok(())
        })?.blocking_recv().map_err(|_| CoreError::Refused("native_host_closed"))??;
        let (stop, _) = watch::channel(false);
        Ok(Self {
            host: Arc::new(host),
            routes,
            controls: Mutex::new(()),
            command_controls: Mutex::new(()),
            pumps: Mutex::new(HashMap::new()),
            guidance: Mutex::new(HashMap::new()),
            worker_stops: Mutex::new(HashMap::new()),
            wake: tokio::sync::Notify::new(),
            workspace_updates: None,
            jobs: Mutex::new(Vec::new()),
            stop,
            exit_recheck: Default::default(),
            followers: std::sync::atomic::AtomicBool::new(true),
            command_recheck: Default::default(),
        })
    }
    pub(crate) fn open_workspace(
        root: &Path,
        project: &str,
        routes: Arc<dyn NativeRoutes>,
        state: PathBuf,
    ) -> CoreResult<Self> {
        Self::open_with(root, project, routes, false, Some(state))
    }
    pub(crate) async fn draft_attachments(
        &self,
        project: String,
        task: String,
    ) -> CoreResult<Value> {
        self.host
            .executor
            .publication(move |store| {
                cortex_core::workspace::draft_attachments(store, &project, &actor(), &task)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    pub(crate) async fn command(
        self: &Arc<Self>,
        project: String,
        request: Value,
    ) -> io::Result<Value> {
        self.client_command(project, request, None).await
    }
    /// Native draft creation, optionally classified by the trusted sprint driver.
    /// The owner-facing command route always passes None.
    pub(super) async fn create_draft(
        &self,
        project: String,
        request: Value,
        planning_sprint: Option<String>,
    ) -> io::Result<Value> {
        if *self.stop.borrow() {
            return Err(failure("service_stopping"));
        }
        self.host
            .executor
            .publication_wait(move |store| {
                use cortex_core::workspace;
                let receipt = match planning_sprint.as_deref() {
                    Some(sprint) => workspace::create_sprint_planning_draft(
                        store, &project, &actor(), &request, sprint,
                    )?,
                    None => workspace::create_draft(store, &project, &actor(), &request)?,
                };
                Ok(receipt.receipt)
            })
            .await
            .map_err(err)
    }
    pub(super) async fn start_automatic(
        self: &Arc<Self>,
        project: String,
        request: Value,
    ) -> io::Result<Value> {
        use cortex_core::workspace::NativeExecutionPolicy;
        if request["operation"] != "tasks.start" {
            return Err(failure("native_operation_unavailable"));
        }
        self.command_with_policy(
            project,
            request,
            None,
            Some(NativeExecutionPolicy::AutomaticContinuation),
        )
        .await
    }
    /// Start a sprint's read-only planning task: bound in the Start transaction.
    pub(super) async fn start_sprint_planning(
        self: &Arc<Self>,
        project: String,
        request: Value,
    ) -> io::Result<Value> {
        use cortex_core::workspace::NativeExecutionPolicy;
        if request["operation"] != "tasks.start" {
            return Err(failure("native_operation_unavailable"));
        }
        self.command_with_policy(
            project,
            request,
            None,
            Some(NativeExecutionPolicy::SprintPlanning),
        )
        .await
    }
    pub(super) async fn client_command(
        self: &Arc<Self>,
        project: String,
        request: Value,
        client: Option<(String, Value, Option<cortex_core::input::Capture>)>,
    ) -> io::Result<Value> {
        self.command_with_policy(project, request, client, None)
            .await
    }
    async fn command_with_policy(
        self: &Arc<Self>,
        project: String,
        request: Value,
        client: Option<(String, Value, Option<cortex_core::input::Capture>)>,
        policy: Option<cortex_core::workspace::NativeExecutionPolicy>,
    ) -> io::Result<Value> {
        if *self.stop.borrow() {
            return Err(failure("service_stopping"));
        }
        let host = self.host.clone();
        let operation = request["operation"].as_str().unwrap_or("");
        if operation == "capabilities.get" {
            return capabilities();
        }
        if operation == "tasks.create" {
            return self.create_draft(project, request, None).await;
        }
        if matches!(operation, "decisions.list" | "decisions.get") {
            return self.decisions(project, request).await;
        }
        if operation == "decisions.answer" {
            return self.answer_command(project, request).await;
        }
        if operation == "reports.read" {
            let report = request["target"].as_str().unwrap_or("").to_owned();
            let cursor = request["cursor"].as_str().map(str::to_owned);
            let limit = request["limit"].as_u64().unwrap_or(4000).clamp(1, 4000) as usize;
            let value = host
                .executor
                .publication(move |store| {
                    cortex_core::workspace::read_report_page(
                        store,
                        &project,
                        &report,
                        cursor.as_deref(),
                        limit,
                    )
                })
                .map_err(err)?
                .await
                .map_err(|_| failure("native_host_closed"))?
                .map_err(err)?;
            return workspace_protocol::validate("ReportPage", &value)
                .map(|_| value)
                .map_err(|_| failure("invalid_report_page"));
        }
        if operation != "tasks.start" {
            return Err(failure("native_operation_unavailable"));
        }
        let _control = self.controls.lock().await;
        let replay_project = project.clone();
        let replay_request = request.clone();
        if let Some(receipt) = host
            .executor
            .publication({
                let incoming = client.is_some();
                move |store| {
                    if incoming {
                        return Ok(None);
                    }
                    cortex_core::workspace::replay_native_start_with_policy(
                        store,
                        &replay_project,
                        &actor(),
                        &replay_request,
                        policy,
                    )
                }
            })
            .map_err(err)?
            .await
            .map_err(|_| failure("native_host_closed"))?
            .map_err(err)?
        {
            return Ok(receipt);
        }
        // Candidate parsing is deliberately not authorization. Refuse before admission/effects.
        // A desktop draft's saved model/effort is qualified exactly as its coordinator
        // turns will route it (`task_route`), never against the owner default alone.
        let saved = if client.is_none() {
            let (p, t) = (
                project.clone(),
                request["target"].as_str().unwrap_or("").to_owned(),
            );
            host.executor
                .publication(move |store| {
                    cortex_core::workspace::draft_settings(store, &p, &actor(), &t)
                })
                .map_err(err)?
                .await
                .map_err(|_| failure("native_host_closed"))?
                .map_err(err)?
        } else {
            None
        };
        // A task's own route never needs a resolvable owner default (incoming
        // clients carry their settings; a desktop draft its saved ones).
        let pin_settings = saved
            .clone()
            .or_else(|| client.as_ref().map(|(_, settings, _)| settings.clone()));
        let coordinator = match &pin_settings {
            Some(settings) => self.routes.task_route(&host.root, settings),
            None => self.routes.route(&host.root, "orchestrator"),
        }
        .map_err(err)?;
        let pin = match &pin_settings {
            Some(settings) => self.routes.coordinator_pin(&host.root, settings),
            None => self.routes.coordinator_pin(&host.root, &json!({})),
        }
        .map_err(err)?;
        let mut jobs = self.jobs.lock().await;
        if *self.stop.borrow() {
            return Err(failure("service_stopping"));
        }
        jobs.retain(|j| !j.is_finished());
        if jobs.len() >= 32 {
            return Err(failure("native_capacity_unavailable"));
        }
        // Own admission before awaiting SQLite. Losing a window/request must not
        // strand a committed Start or cause a retry to launch a second process.
        let (send, reply) = tokio::sync::oneshot::channel();
        let runtime = self.clone();
        jobs.push(tokio::spawn(async move {
            let maintenance = runtime.host.workspace_state.clone();
            let admitted = async {
                host.executor
                    .publication(move |store| {
                        let _guard = maintenance
                            .as_deref()
                            .map(crate::machine::maintenance::admission)
                            .transpose()
                            .map_err(|e| CoreError::Refused(e.code))?;
                        if client.is_none()
                            && cortex_core::workspace::draft_worktree_selected(
                                store, &project, &actor(), request["target"].as_str().unwrap_or(""),
                            )?
                        {
                            if policy.is_some() {
                                return Err(CoreError::Refused("native_worktree_unavailable"));
                            }
                            let state = maintenance.as_deref()
                                .ok_or(CoreError::Refused("native_worktree_unavailable"))?;
                            return crate::machine::start_worktree(store, state, &project, &actor(), &request);
                        }
                        let start = || cortex_core::workspace::start_native_input_with_policy(
                            store, &project, &actor(), &request,
                            client.as_ref().map(|(session, settings, _)| (session.as_str(), settings)),
                            client.as_ref().and_then(|(_, _, capture)| capture.as_ref()),
                            policy,
                        );
                        if let Some(state) = maintenance.as_deref() {
                            crate::machine::task_rework::with_plain_start_guard(state, &project, &request, start)
                        } else {
                            start()
                        }
                    })
                    .map_err(err)?
                    .await
                    .map_err(|_| failure("native_host_closed"))?
                    .map_err(err)
            }
            .await;
            match admitted {
                Err(error) => {
                    let _ = send.send(Err(error));
                }
                Ok(admitted) => {
                    if admitted["fresh"] != true {
                        let _ = send.send(Ok(admitted["receipt"].clone()));
                        return;
                    }
                    let task = admitted["task"].as_str().unwrap().to_owned();
                    let owner = OwnerEpoch {
                        owner: admitted["owner"]["owner"].as_str().unwrap().into(),
                        epoch: admitted["owner"]["epoch"].as_i64().unwrap(),
                    };
                    let goal = admitted["goal"].as_str().unwrap().to_owned();
                    // Pin the resolved coordinator route with the task's start.
                    if let Some(pin) = pin {
                        let t = task.clone();
                        if let Ok(reply) = runtime.host.executor.write(move |db| {
                            cortex_core::lifecycle::event_tx(
                                db,
                                &t,
                                "NATIVE_COORDINATOR_ROUTE",
                                pin,
                            )?;
                            Ok(())
                        }) {
                            let _ = reply.await;
                        }
                    }
                    // Resolve the committed binding before preflight. On a lost
                    // checkout the driver handles recovery; never observe main for it.
                    if let Ok(checkout) = runtime.host.execution_checkout(&task, &owner).await {
                        crate::native_preflight::record(
                            &runtime.host.executor, &task, checkout.path(),
                        ).await;
                    }
                    let (cancel, cancelled) = watch::channel(false);
                    let (done, finished) = watch::channel(false);
                    runtime.install_pump(task.clone(), cancel, finished).await;
                    let _ = send.send(Ok(admitted["receipt"].clone()));
                    if let Err(error) = runtime
                        .drive(&task, &owner, &goal, coordinator, cancelled.clone(), None)
                        .await
                    {
                        if *cancelled.borrow() || *runtime.stop.borrow() {
                            done.send_replace(true);
                            return;
                        }
                        runtime
                            .stop_after_error(&task, &owner, error, cancelled.clone())
                            .await;
                    }
                    runtime.driver_ended(&task, &cancelled).await;
                    done.send_replace(true);
                }
            }
        }));
        drop(jobs);
        reply
            .await
            .map_err(|_| failure("native_start_outcome_unknown"))?
    }
    /// One runtime turn with the degenerate-turn watchdog (see
    /// `native_sessions::degenerate`). A coordinator turn that degenerated without
    /// changing the task's state or plan revision runs once more, with the same
    /// prompt, in a fresh engine session; when that degenerates too, the coordinator
    /// moves to another qualified model (`NATIVE_COORDINATOR_ESCALATED`, preferably
    /// the other engine) and runs the turn there. Only when no other model exists, or
    /// it degenerates as well, does the turn end with `native_turn_degenerate`, which
    /// the no-stop policy turns into a continuation on an escalated rung, never an
    /// owner hold. A worker's degenerate turn ends with its node still open and its
    /// session terminated, so the lost-worker path replaces it in a fresh session on
    /// an escalated route (another model or engine) at the next step, every time.
    async fn turn(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        role: SessionRole,
        route: SessionRoute,
        prompt: &str,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        const DEGENERATE: &str = "native_turn_degenerate";
        let assignment = match &role {
            SessionRole::Worker { assignment } => Some(assignment.clone()),
            SessionRole::Coordinator | SessionRole::ActiveSteering => None,
        };
        let retry = route.clone();
        let before = self.turn_marker(task).await?;
        match self
            .turn_once(task, owner, role, route, prompt, cancelled.clone(), None)
            .await
        {
            Err(error) if error.code() == DEGENERATE => {}
            other => return self.keep_committed_turn(task, &before, other).await,
        }
        if assignment.is_some() {
            // The replacement routes as an escalated redispatch of the same node.
            return Ok(());
        }
        // A degenerate turn that still changed the task (a plan, a question) stands;
        // the driver continues from that state as after any other turn.
        if self.turn_marker(task).await?.state_and_revision != before.state_and_revision {
            return Ok(());
        }
        match self
            .turn_once(
                task,
                owner,
                SessionRole::Coordinator,
                retry.clone(),
                prompt,
                cancelled.clone(),
                None,
            )
            .await
        {
            Err(error) if error.code() == DEGENERATE => {}
            other => return self.keep_committed_turn(task, &before, other).await,
        }
        if self.turn_marker(task).await?.state_and_revision != before.state_and_revision {
            return Ok(());
        }
        // Never the same model a third time: move the coordinator to another one.
        let current = self.coordinator_route(task, Some(retry)).await?;
        let Some(alternate) = self
            .escalate_coordinator(task, &current, DEGENERATE)
            .await?
        else {
            return Err(CoreError::Refused(DEGENERATE));
        };
        match self
            .turn_once(
                task,
                owner,
                SessionRole::Coordinator,
                alternate,
                prompt,
                cancelled,
                None,
            )
            .await
        {
            Err(error) if error.code() == DEGENERATE => Err(CoreError::Refused(DEGENERATE)),
            other => self.keep_committed_turn(task, &before, other).await,
        }
    }

    /// A tool can commit a plan, result or question before the prompt reply is
    /// lost. Keep that checkpoint rather than stopping or replaying the turn.
    async fn keep_committed_turn(
        &self,
        task: &str,
        before: &TurnMarker,
        result: CoreResult<()>,
    ) -> CoreResult<()> {
        if matches!(&result, Err(error) if error.code() == "native_turn_outcome_unknown") {
            let after = self.turn_marker(task).await?;
            // Submitting the final synthesis can commit without changing state or revision.
            if after.state_and_revision != before.state_and_revision
                || (!before.has_synthesis && after.has_synthesis)
            {
                return Ok(());
            }
        }
        result
    }

    /// Move the task's coordinator to another qualified model than `current`
    /// (preferably another engine or account), recorded as
    /// `NATIVE_COORDINATOR_ESCALATED`. Later coordinator turns of the task use it
    /// until the owner chooses a model again. `None` when no other model qualifies.
    pub(super) async fn escalate_coordinator(
        &self,
        task: &str,
        current: &SessionRoute,
        reason: &str,
    ) -> CoreResult<Option<SessionRoute>> {
        let Some(pin) = self
            .routes
            .alternate_coordinator_pin(&self.host.root, current)?
        else {
            return Ok(None);
        };
        let route = match self.routes.pinned_route(&self.host.root, &pin, None) {
            Ok(route) => route,
            Err(_) => return Ok(None),
        };
        let t = task.to_owned();
        let record = json!({"pin":pin,"reason":reason,"from":{"connection":current.connection,"model":current.model},
            "to":{"connection":route.connection,"model":route.model}});
        self.host
            .executor
            .write(move |db| {
                cortex_core::lifecycle::event_tx(db, &t, "NATIVE_COORDINATOR_ESCALATED", record)?;
                Ok(())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        Ok(Some(route))
    }
    /// Read the durable checkpoint indicators together, without loading synthesis text.
    async fn turn_marker(&self, task: &str) -> CoreResult<TurnMarker> {
        let t = task.to_owned();
        self.host
            .executor
            .read(move |db| {
                Ok(db.query_row(
                    "SELECT state,revision,synthesis IS NOT NULL FROM runtime_tasks WHERE task_id=?",
                    [&t],
                    |r| {
                        Ok(TurnMarker {
                            state_and_revision: (r.get(0)?, r.get(1)?),
                            has_synthesis: r.get(2)?,
                        })
                    },
                )?)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    async fn turn_once(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        role: SessionRole,
        route: SessionRoute,
        prompt: &str,
        cancelled: watch::Receiver<bool>,
        launch: Option<LaunchTicket>,
    ) -> CoreResult<()> {
        self.turn_once_scoped(task, owner, role, route, prompt, cancelled, launch, None).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn turn_once_scoped(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        mut role: SessionRole,
        mut route: SessionRoute,
        prompt: &str,
        cancelled: watch::Receiver<bool>,
        mut launch: Option<LaunchTicket>,
        merge_handoff: Option<&str>,
    ) -> CoreResult<()> {
        // The task's settings must still be readable before any turn starts.
        self.task_settings(task).await?;
        let active_steering = matches!(role, SessionRole::ActiveSteering);
        let coordinator = active_steering || matches!(role, SessionRole::Coordinator);
        if coordinator {
            route = self.coordinator_route(task, Some(route)).await?;
            let t = task.to_owned();
            if self
                .host
                .executor
                .read(move |db| cortex_core::lifecycle::automatic::enabled(db, &t))?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??
            {
                self.routes
                    .automatic_coordinator_health(&self.host.root, &route)?;
            }
        }
        // The owner's language (an explicit en/ru setting, otherwise the goal's language)
        // is recorded in task_language and stated per role: owner-facing text follows it,
        // worker-to-worker material stays English.
        let t = task.to_owned();
        let language = self
            .host
            .executor
            .write(move |db| cortex_core::assignment_brief::record_language_tx(db, &t))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        route.instructions.push_str(&if coordinator {
            format!("\nOwner-selected response language: {language}. Write in it everything the owner reads directly: \
your messages and questions to the owner (question text and option labels), the final result text and its not_verified entries. \
Write plans, objectives and every instruction to workers in English; worker reports stay English, so compose the owner-facing text \
from them in {language}.")
        } else {
            format!("\nOwner-selected response language: {language}. user_question_language: {language}. It applies only to \
a question to the owner (question text and option labels). Write reports, report headings, summaries and every other message in English.")
        });
        let mut prompt = prompt.to_owned();
        if coordinator && !active_steering {
            if let Some(state) = self.host.workspace_state.clone() {
                let t = task.to_owned();
                let summary = self
                    .host
                    .executor
                    .read(move |db| {
                        crate::machine::sprint_graph::Scope::load(db, &state, &t)?
                            .map(|scope| scope.start(db))
                            .transpose()
                    })?
                    .await
                    .map_err(|_| CoreError::Refused("native_host_closed"))?;
                match summary {
                    Ok(Some(summary)) => prompt.push_str(&format!("\n\nSprint dependencies (Workspace records). Use sprint_graph to explore the graph and sprint_task_result to read a member’s outcome.\n{summary}")),
                    Ok(None) => {},
                    Err(error) => prompt.push_str(&format!("\n\nSprint context unavailable ({}).", error.code())),
                }
            }
            let t = task.to_owned();
            let instruction = self
                .host
                .executor
                .read(move |db| {
                    cortex_core::lifecycle::skill_proposals::planning_instruction(db, &t)
                })?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??;
            if !instruction.is_empty() {
                prompt.push_str(&format!("\n\n{instruction}"));
            }
        }
        let order = launch.as_ref().map(LaunchTicket::order);
        loop {
            let mut attempt = RouteAttempt::new(
                self.host.route_holds.clone(),
                RouteKey(route.launch.engine.name(), route.connection.clone(), route.model.clone()),
            );
            let result = self.turn_once_attempt(
                task, owner, role.clone(), route.clone(), &prompt, cancelled.clone(),
                launch.take(), &mut attempt, merge_handoff,
            ).await;
            if active_steering || merge_handoff.is_some() || !matches!(&result, Err(error) if error.code() == LIMITED) {
                return result;
            }
            if *self.stop.borrow() || *cancelled.borrow() {
                return Ok(());
            }
            let channel = attempt.refused_channel.take().expect("confirmed provider refusal");
            if let SessionRole::Worker { assignment } = &mut role {
                let t = task.to_owned();
                let o = owner.clone();
                let old = assignment.clone();
                let result = self.host.executor.write(move |db| {
                    cortex_core::scheduler::replace_provider_refused_claim_tx(db, &t, &o, &old, &channel)
                })?.await.map_err(|_| CoreError::Refused("native_host_closed"))?;
                match result {
                    Ok(new) => {
                        if let Ok(mut brief) = serde_json::from_str::<Value>(&prompt) {
                            if brief.is_object() && brief["assignment_ref"] == json!(*assignment) {
                                brief["assignment_ref"] = json!(new);
                                prompt = brief.to_string();
                            }
                        }
                        *assignment = new;
                        let order = order.clone().unwrap_or_default();
                        launch = Some(order.push(assignment.clone(), self.host.capacity_changed.clone()));
                    }
                    // A durable pause, replan, or result wins over a retry.
                    Err(error) if error.code() == "native_provider_retry_claim_changed" => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
        }
    }

    async fn turn_once_attempt(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        role: SessionRole,
        route: SessionRoute,
        prompt: &str,
        mut cancelled: watch::Receiver<bool>,
        launch: Option<LaunchTicket>,
        attempt: &mut RouteAttempt,
        merge_handoff: Option<&str>,
    ) -> CoreResult<()> {
        let assignment = match &role {
            SessionRole::Worker { assignment } => Some(assignment.clone()),
            _ => None,
        };
        let is_worker = assignment.is_some();
        let active_steering = matches!(role, SessionRole::ActiveSteering);
        // Register owner guidance/stop before dispatch: dispatch already reports the
        // worker ready, and a message sent at that moment must reach it.
        let (sender, mut guidance) = tokio::sync::mpsc::channel(8);
        let (worker_stop, mut worker_cancelled) = watch::channel(false);
        let (worker_done, worker_finished) = watch::channel(false);
        if let Some(a) = &assignment {
            self.guidance.lock().await.insert(a.clone(), sender);
            self.worker_stops
                .lock()
                .await
                .insert(a.clone(), (worker_stop, worker_finished));
        }
        let mut stop = self.stop.subscribe();
        let launch = launch.map(|ticket| {
            WorkerLaunch::new(ticket, stop.clone(), cancelled.clone(), worker_cancelled.clone())
        });
        let driver = match self
            .host
            .dispatch_managed(task.into(), owner.clone(), role, route, launch, Some((attempt, LaunchInterrupts {
                service: stop.clone(),
                batch: cancelled.clone(),
                worker: worker_cancelled.clone(),
            })))
            .await
        {
            Ok(driver) => driver,
            Err(error) => {
                if let Some(a) = &assignment {
                    self.guidance.lock().await.remove(a);
                    self.worker_stops.lock().await.remove(a);
                }
                match error.code() {
                    // No launch to settle. The batch reconciles the current task state.
                    "native_task_interrupted" => return Ok(()),
                    "native_worker_stopped_before_launch" => {
                        worker_done.send_replace(true);
                        return Ok(());
                    }
                    _ => return Err(error),
                }
            }
        };
        driver.treat_refusal_as_degenerate();
        // How the turn ended, as the runtime observed it; recorded for a worker below.
        let turn: CoreResult<&'static str> = if *stop.borrow_and_update() || *cancelled.borrow() {
            Err(CoreError::Refused("native_service_stopped"))
        } else if *worker_cancelled.borrow() {
            Ok("owner_stopped")
        } else {
            tokio::select! {
                result = async {
                    if active_steering {
                        let turn = driver.prompt(prompt).await?;
                        if turn.stop_reason == "cancelled" {
                            return Err(CoreError::Refused("active_steering_unconfirmed"));
                        }
                        attempt.succeeded();
                        return Ok("turn_ended");
                    }
                    let mut handled_refusal = if is_worker { driver.event_cursor().await? } else { 0 };
                    let mut continuations = 0;
                    self.guided_prompt(&driver, prompt, &mut guidance, is_worker).await?;
                    if let Some(request) = merge_handoff {
                        let (t, request, channel) = (
                            task.to_owned(), request.to_owned(), driver.channel_id().to_owned(),
                        );
                        self.host.executor.write(move |db| {
                            cortex_core::lifecycle::event_tx(db, &t, "WORKSPACE_MERGE_HANDOFF_DELIVERED",
                                json!({"request":request,"channel_id":channel})).map(|_| ())
                        })?.await.map_err(|_| CoreError::Refused("native_host_closed"))??;
                    }
                    attempt.succeeded();
                    loop {
                        let wake = self.wake.notified();
                        tokio::pin!(wake);
                        wake.as_mut().enable();
                        use crate::native_sessions::SessionWait;
                        match driver.wait_state().await? {
                            SessionWait::Ready => { driver.continue_answer().await?; },
                            SessionWait::Open(retry) => tokio::select! {
                                _ = &mut wake => {},
                                _ = tokio::time::sleep(retry.unwrap_or_default()), if retry.is_some() => {},
                                // A worker process that dies while its question is open is
                                // noticed now, not when the answer arrives. The turn ends
                                // and the exited process is settled; the lost-worker path
                                // then replaces the assignment and retires its wait.
                                _ = driver.process_exited(), if is_worker => return Ok("process_exited"),
                                next = driver.coordinator_guidance(true), if is_worker => {
                                    driver.forward_coordinator_guidance(next?).await?;
                                },
                                Some(message) = guidance.recv() => {
                                    let result = driver.prompt(&crate::native_owner_controls::guidance_prompt(&message)).await;
                                    driver.guidance_delivery(message, result.is_ok(), false).await?;
                                    result?;
                                }
                            },
                            _ if is_worker && continuations < 2 => {
                                let next=tokio::select!{
                                    result=driver.command_continuation(handled_refusal)=>result?,
                                    _=driver.process_exited()=>return Ok("process_exited"),
                                };
                                let Some((id,message))=next else {break;};
                                handled_refusal=id;
                                continuations+=1;
                                self.guided_prompt(&driver,&message,&mut guidance,true).await?;
                            }
                            _ => break,
                        }
                    }
                    Ok("turn_ended")
                } => result,
                _ = stop.changed() => Err(CoreError::Refused("native_service_stopped")),
                _ = cancelled.changed() => Err(CoreError::Refused("native_task_interrupted")),
                _ = worker_cancelled.changed() => Ok("owner_stopped"),
            }
        };
        // Always observe the owned wait, including prompt failure or service shutdown.
        if let Some(a) = &assignment {
            self.guidance.lock().await.remove(a);
        }
        let diagnostics = driver.diagnostics();
        let channel = driver.channel_id().to_owned();
        let retryable_prompt = matches!(&turn, Err(error) if error.code() == "native_turn_outcome_unknown");
        let settled = driver.shutdown_managed(retryable_prompt.then_some(&mut *attempt)).await;
        if settled.is_ok() {
            worker_done.send_replace(true);
        }
        if let Some(a) = assignment.as_ref().filter(|_| attempt.refused_channel.is_none()) {
            // Why this attempt ended, for the lost-worker path's classification.
            let cause = match &turn {
                Ok(cause) => *cause,
                Err(error) => error.code(),
            };
            if let Err(error) = self
                .record_worker_ending(task, a, &channel, cause, diagnostics.snapshot())
                .await
            {
                eprintln!(
                    "workspace-service: the end of a worker attempt was not recorded ({})",
                    error.code()
                );
            }
        }
        if let Some(a) = assignment {
            self.worker_stops.lock().await.remove(&a);
        }
        settled?;
        if attempt.refused_channel.is_some() && !*worker_cancelled.borrow() {
            return Err(CoreError::Refused(LIMITED));
        }
        // Worker endings are consumed by the worker retry path. A coordinator
        // interruption instead reaches the task driver, unless the turn committed
        // a checkpoint (checked by `keep_committed_turn`).
        if let Err(e) = turn {
            if !is_worker || e.code() != "native_turn_outcome_unknown" {
                return Err(e);
            }
        }
        Ok(())
    }
    /// Records why a worker attempt that has no accepted result ended
    /// (`scheduler::retry::record_attempt_ending_tx`): the turn's end as the runtime
    /// observed it, with the engine's exit status or signal, transport close, a runtime
    /// cancel of a permission, an owner pause and a short redacted stderr tail. A turn
    /// that ended after refused results is the attempt's own quality
    /// (`result_refused`), as is a degenerate one (`degenerate_turn`); a process that
    /// vanished without the model ending its turn is `worker_lost`; the owner's pause
    /// or stop is `owner_paused`, a service stop `runtime_restart`, a command sandbox
    /// that could not be prepared `sandbox_unavailable`.
    async fn record_worker_ending(
        &self,
        task: &str,
        assignment: &str,
        channel: &str,
        cause: &str,
        diagnostics: cortex_engine::engine::DiagnosticSnapshot,
    ) -> CoreResult<()> {
        let (t, a, ch, cause) = (
            task.to_owned(),
            assignment.to_owned(),
            channel.to_owned(),
            cause.to_owned(),
        );
        self.host
            .executor
            .write(move |db| {
                use rusqlite::OptionalExtension;
                let Some(attempt) = db
                    .query_row(
                        "SELECT attempt_id FROM trusted_channels WHERE task_id=? AND channel_id=?",
                        (&t, &ch),
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten()
                else {
                    return Ok(());
                };
                let open: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND assignment_ref=?
                     AND result_json IS NULL AND status IN ('running','failed'))",
                    (&t, &a),
                    |r| r.get(0),
                )?;
                if !open {
                    return Ok(());
                }
                let facts = worker_ending_facts(db, &t, &ch, &attempt)?;
                let class = worker_ending_class(&cause, &facts);
                let mut ending = facts;
                ending["cause"] = json!(cause);
                ending["exit"] = json!(diagnostics.exit);
                ending["exited_unprompted"] = json!(diagnostics.exited_unprompted);
                ending["transport_close"] = json!(diagnostics.transport_close);
                ending["stderr_bytes"] = json!(diagnostics.stderr_bytes);
                ending["stderr_tail"] = json!(stderr_tail(&diagnostics.stderr_tail));
                cortex_core::scheduler::retry::record_attempt_ending_tx(
                    db, &t, &a, &attempt, &cause, class, &ending,
                )?;
                Ok(())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// Record why the driver stopped and let the automatic policy decide whether
    /// to resume, cool down or ask the owner. The driver's end follows only an
    /// owed continuation; an open runtime question is not owed until answered.
    pub(super) async fn stop_after_error(
        self: &Arc<Self>,
        task: &str,
        owner: &OwnerEpoch,
        mut error: CoreError,
        cancelled: watch::Receiver<bool>,
    ) {
        for round in 0..3 {
            if *cancelled.borrow() || *self.stop.borrow() {
                return;
            }
            let code = error.code().to_owned();
            let gap = code.starts_with("completion_");
            let t = task.to_owned();
            let owner_for_stop = owner.clone();
            let state = match self.host.executor.write(move |db| {
                cortex_core::lifecycle::event_tx(
                    db,
                    &t,
                    "NATIVE_DRIVER_STOPPED",
                    json!({"reason":code}),
                )?;
                // A task already waiting on an earlier version's open runtime stop
                // question keeps it: its answer relaunches the coordinator.
                let (current, _) = cortex_core::lifecycle::domain::task_tx(db, &t)?;
                let asking: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events e JOIN pending_questions q
                        ON q.task_id=e.task_id AND q.id=json_extract(e.payload_json,'$.question_id') AND q.status='open'
                     WHERE e.task_id=? AND e.type='USER_QUESTION_REQUESTED' AND json_extract(e.payload_json,'$.runtime_stop')=1)",
                    [&t],
                    |r| r.get(0),
                )?;
                if current == cortex_core::TaskState::WaitingUser && asking {
                    return Ok(current);
                }
                cortex_core::ownership::check_tx(db, &t, &owner_for_stop)?;
                cortex_core::lifecycle::domain::stop_tx(db, &t, &code)
            }) {
                Ok(reply) => reply.await.ok().and_then(|r| r.ok()),
                Err(_) => None,
            };
            // A completion gap that needs the owner's decision about the work (a
            // replaced failed check) stays RECONCILING for its coordinator, which asks:
            // drive that turn here, or nothing would ask. Every other stop is a
            // continuation the driver's end follows.
            if round == 2 || !gap || state != Some(cortex_core::TaskState::Reconciling) {
                return;
            }
            match self.progress(task, owner, cancelled.clone()).await {
                Ok(()) => return,
                Err(next) => error = next,
            }
        }
    }
    /// A driver of `task` ended on its own (not cancelled, the service not stopping):
    /// continue whatever the no-stop policy now owes it.
    pub(super) async fn driver_ended(
        self: &Arc<Self>,
        task: &str,
        cancelled: &watch::Receiver<bool>,
    ) {
        if !*cancelled.borrow() && !*self.stop.borrow() {
            self.follow_continuation(task.to_owned()).await;
        }
    }
    /// The runtime's own recheck of what a parked task waits for, without any model
    /// call: each due wait is checked against the built-in endpoint policy before
    /// any TCP connect or HTTPS HEAD, and the outcome is recorded
    /// (`EXTERNAL_WAIT_RECHECKED`, `EXTERNAL_WAIT_ENDED`); an open owner action that an
    /// environment preflight can show fixed (GitHub CLI, Codex or Claude login,
    /// passwordless sudo) is rechecked with the same backoff and resolved
    /// (`OWNER_ACTION_RESOLVED`, `by: environment_recheck`) once the fact is present.
    pub(super) async fn recheck_obstacles(&self, task: &str) -> CoreResult<()> {
        let now = unix_seconds();
        let t = task.to_owned();
        let (due, services) = self
            .host
            .executor
            .read(move |db| {
                Ok((
                    cortex_core::lifecycle::obstacles::due_waits(db, &t, now)?,
                    cortex_core::lifecycle::obstacles::environment_services(db, &t)?,
                ))
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        for wait in due {
            let (ok, detail) = probe_external(&wait).await;
            let (t, id) = (
                task.to_owned(),
                wait["id"].as_str().unwrap_or("").to_owned(),
            );
            self.host
                .executor
                .write(move |db| {
                    cortex_core::lifecycle::obstacles::record_recheck_tx(
                        db, &t, &id, ok, now, &detail,
                    )
                    .map(|_| ())
                })?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??;
        }
        if !services.is_empty() && environment_recheck_due(task) {
            let t = task.to_owned();
            let owner = self.host.executor.read(move |db| {
                Ok(db.query_row("SELECT owner_ref,epoch FROM task_owners WHERE task_id=?", [&t],
                    |r| Ok(OwnerEpoch {owner:r.get(0)?,epoch:r.get(1)?}))?)
            })?.await.map_err(|_| CoreError::Refused("native_host_closed"))??;
            let checkout = self.host.execution_checkout(task, &owner).await?;
            let facts = crate::native_preflight::facts(checkout.path()).await;
            let t = task.to_owned();
            self.host
                .executor
                .write(move |db| {
                    cortex_core::lifecycle::obstacles::resolve_by_environment_tx(db, &t, &facts)
                        .map(|_| ())
                })?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??;
        }
        Ok(())
    }
    async fn drive(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        goal: &str,
        route: SessionRoute,
        cancelled: watch::Receiver<bool>,
        merge_handoff: Option<String>,
    ) -> CoreResult<()> {
        if *self.stop.borrow() {
            return Err(CoreError::Refused("native_service_stopped"));
        }
        // Owner messages recorded for the next boundary (for example before a restart
        // or a later message's relaunch), and queued ones, reach this coordinator turn.
        self.take_queued(task).await?;
        let t = task.to_owned();
        let messages = self
            .host
            .executor
            .write(move |db| cortex_core::lifecycle::steering::deliver_tx(db, &t))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        let goal = if messages.is_empty() {
            goal.to_owned()
        } else {
            format!(
                "{goal}\n\n{}",
                cortex_core::lifecycle::steering::guidance_prompt(&messages)
            )
        };
        // The initial (or next) runtime pipeline edition precedes the coordinator turn.
        crate::native_pipeline::publish(&self.host.executor, task, Some(&route)).await;
        let turn = if let Some(request) = merge_handoff.as_deref() {
            // Unlike ordinary continuation prompts, a Git handoff is not a
            // retryable input, even when a provider responds with a refusal.
            self.turn_once_scoped(
                task, owner, SessionRole::Coordinator, route, &goal, cancelled.clone(), None, Some(request),
            ).await
        } else {
            self.turn(task, owner, SessionRole::Coordinator, route, &goal, cancelled.clone()).await
        };
        self.confirm_messages(task).await?;
        turn?;
        self.progress(task, owner, cancelled).await
    }
    /// The owner's queued messages for this task (the machine's `product.message`
    /// queue) are taken by the service, window open or not, when the task is at a
    /// boundary of the coordinator's work: each is recorded as a boundary message under
    /// its own id, in queue order, so it reaches the next coordinator turn. Until taken a
    /// message stays editable; a take interrupted by a restart is finished by the next one
    /// (the message id makes it idempotent).
    pub(super) async fn take_queued(&self, task: &str) -> CoreResult<()> {
        let Some(state) = self.host.workspace_state.clone() else {
            return Ok(());
        };
        let t = task.to_owned();
        let bound: Option<(String, String, String, String)> = self
            .host
            .executor
            .read(move |db| {
                use rusqlite::OptionalExtension;
                Ok(db
                    .query_row(
                        "SELECT w.project_id,w.task_id,e.execution_id,r.state FROM workspace_executions e
                         JOIN workspace_tasks w ON w.task_id=e.task_id AND w.execution_id=e.execution_id
                         JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE e.runtime_task_id=?",
                        [&t],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        let Some((project, draft, execution, current)) = bound else {
            return Ok(());
        };
        if !["PLANNING", "PLAN_READY", "RECONCILING", "ANSWER_READY"].contains(&current.as_str()) {
            return Ok(());
        }
        let (s, p, d) = (state.clone(), project.clone(), draft.clone());
        let taken =
            tokio::task::spawn_blocking(move || crate::machine::take_queued_messages(&s, &p, &d))
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))?
                .map_err(|e| CoreError::Refused(e.code))?;
        for message in taken {
            let id = message["id"].as_str().unwrap_or_default().to_owned();
            let outcome = self
                .record_queued(&state, &project, &execution, &message)
                .await;
            let settled = outcome.map_err(|e| e.code().to_owned());
            let (s, p) = (state.clone(), project.clone());
            tokio::task::spawn_blocking(move || {
                crate::machine::settle_taken_message(&s, &p, &id, settled)
            })
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
            .map_err(|e| CoreError::Refused(e.code))?;
        }
        Ok(())
    }
    /// Admit and finish one taken queued message as a boundary message, idempotently.
    async fn record_queued(
        &self,
        state: &std::path::Path,
        project: &str,
        execution: &str,
        message: &Value,
    ) -> CoreResult<()> {
        let mut files = json!({"attachments":message["attachments"].clone()});
        if !files["attachments"].is_array() {
            files["attachments"] = json!([]);
        }
        crate::machine::attachment_files::inline_for_delivery(state, &mut files);
        let capture = match files["attachments"].as_array() {
            Some(items) if !items.is_empty() => Some(cortex_core::input::capture_from_wire(
                &files["attachments"],
            )?),
            _ => None,
        };
        let request = cortex_core::workspace::control::canonical_request(
            json!({"projectId":project,"executionId":execution,"requestId":message["id"],"action":"prompt",
                "text":message["text"],"attachments":[],"queued":true}),
            capture.as_ref(),
        )?;
        let p = project.to_owned();
        self.host
            .executor
            .publication(move |s| {
                use cortex_core::workspace::control;
                let actor = actor();
                match control::replay(s, &p, &actor, &request) {
                    Ok(Some(_)) => return Ok(()),
                    // Admitted by an earlier take that stopped before its finish.
                    Err(e) if e.code() == "native_control_outcome_unknown" => {}
                    Err(e) if e.code() == "payload_conflict" => return Ok(()),
                    Err(e) => return Err(e),
                    Ok(None) => {
                        match control::admit_input(s, &p, &actor, &request, capture.as_ref()) {
                            Ok(_) => {}
                            // Sent by the owner (Send now) under the same id meanwhile.
                            Err(e)
                                if matches!(
                                    e.code(),
                                    "native_control_already_recorded" | "payload_conflict"
                                ) =>
                            {
                                return Ok(())
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
                control::finish(s, &p, &actor, &request, false).map(|_| ())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// Record the owner messages a finished coordinator turn's confirmed prompt carried.
    async fn confirm_messages(&self, task: &str) -> CoreResult<()> {
        let t = task.to_owned();
        self.host
            .executor
            .write(move |db| cortex_core::lifecycle::steering::confirm_tx(db, &t).map(|_| ()))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// Whether an owner message waits for the next coordinator turn.
    pub(super) async fn recover_planning(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        // SessionDriver rebuilds the capsule on its serial writer lane immediately
        // before delivery; never reuse a snapshot captured before replacement.
        let mut goal=String::from("Coordinator recovery on a new engine session. The prior process has a confirmed terminal observation. Do not replay commands, permissions or claimed deliveries. Continue from the fresh retained-state capsule; missing outcomes remain unresolved.");
        goal.push_str(&self.recovery_failure_guidance(task).await?);
        let route = self.coordinator_route(task, None).await?;
        self.drive(task, owner, &goal, route, cancelled, None).await
    }

    /// Point the coordinator at durable failed reports. These are historical
    /// references; the coordinator reads the actual checks before proposing a repair.
    async fn recovery_failure_guidance(&self, task: &str) -> CoreResult<String> {
        let task = task.to_owned();
        self.host.executor.read(move |db| {
            let mut stmt = db.prepare(
                "SELECT json_extract(result_json,'$.report_ref') FROM runtime_nodes WHERE task_id=?
                 AND status='failed' AND json_type(result_json,'$.report_ref')='text'
                 ORDER BY revision DESC,rowid DESC LIMIT 9",
            )?;
            let mut reports = stmt.query_map([&task], |r| r.get::<_,String>(0))?
                .collect::<Result<Vec<_>,_>>()?;
            if reports.is_empty() { return Ok(String::new()); }
            let more = reports.len() > 8;
            reports.truncate(8);
            Ok(format!("\nPreviously failed worker reports: {}. {}Read relevant reports, assign in-scope fixes to a writer, and rerun each outstanding check with the same argv and working directory. A passing final check can cover a project command at the same working directory.", reports.join(", "),
                if more { "More reports are available in the task catalogue. " } else { "" }))
        })?.await.map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    pub(super) async fn install_pump(
        &self,
        task: String,
        cancel: watch::Sender<bool>,
        done: watch::Receiver<bool>,
    ) {
        // The map lock is held across the wait so two installers cannot interleave:
        // a task never has two drivers. A dropped sender means the prior driver is gone.
        let mut pumps = self.pumps.lock().await;
        if let Some(prior) = pumps.remove(&task) {
            prior.cancel.send_replace(true);
            let mut done = prior.done;
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        pumps.insert(task, Pump { cancel, done });
    }
    pub(super) async fn spawn_recovery(
        &self,
        job: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let mut jobs = self.jobs.lock().await;
        jobs.retain(|j| !j.is_finished());
        jobs.push(tokio::spawn(job));
    }
    pub(super) async fn progress(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        loop {
            if *self.stop.borrow() || *cancelled.borrow() {
                return Err(CoreError::Refused("native_service_stopped"));
            }
            let t = task.to_owned();
            if self
                .host
                .executor
                .read(move |db| cortex_core::lifecycle::automatic::held(db, &t))?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??
            {
                return Ok(());
            }
            self.spend_progress(task).await?;
            loop {
                if *self.stop.borrow() || *cancelled.borrow() {
                    return Err(CoreError::Refused("native_service_stopped"));
                }
                let t = task.to_owned();
                let waiting = self
                    .host
                    .executor
                    .write(move |db| {
                        // A lost worker's wait that can never continue is closed
                        // here, or the driver would wait on it forever.
                        cortex_core::lifecycle::waits::cancel_lost_tx(db, &t)?;
                        Ok(cortex_core::lifecycle::domain::task_tx(db, &t)?.0
                            == cortex_core::TaskState::WaitingUser
                            || cortex_core::lifecycle::waits::projection_tx(db, &t)?
                                .iter()
                                .any(|w| {
                                    w["status"] == "open" && w["continuation_required"] == true
                                }))
                    })?
                    .await
                    .map_err(|_| CoreError::Refused("native_host_closed"))??;
                if !waiting {
                    break;
                }
                tokio::select! {
                    _ = self.wake.notified() => {},
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
                }
            }
            let step = loop {
                if *self.stop.borrow() || *cancelled.borrow() {
                    return Err(CoreError::Refused("native_service_stopped"));
                }
                // Messages the owner queued are taken at a boundary, before the step
                // that could otherwise start a plan or complete past them.
                self.take_queued(task).await?;
                let step = self
                    .host
                    .advance_step(task.into(), owner.clone(), true)
                    .await?;
                // Verification just passed: take queued messages before reconciling.
                if step["automaticHold"] == true {
                    return Ok(());
                }
                if step["recheck"] == true {
                    continue;
                }
                if !step["assignments"].as_array().is_some_and(|a| a.is_empty()) {
                    break step;
                }
                // A settled replacement waits out its retained retry backoff.
                // Waiting does not consume the bounded progress/dispatch budget.
                let t = task.to_owned();
                let delayed = self.host.executor.read(move |db| {
                    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                        .map_err(|_|CoreError::Refused("clock_unavailable"))?.as_secs_f64();
                    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_nodes n
                        JOIN runtime_tasks t USING(task_id) WHERE n.task_id=? AND n.revision=t.revision
                        AND n.status='pending' AND EXISTS(SELECT 1 FROM runtime_events e
                            WHERE e.id=(SELECT max(x.id) FROM runtime_events x WHERE x.task_id=n.task_id
                                AND x.type='ASSIGNMENT_RETRIED' AND json_extract(x.payload_json,'$.node')=n.node_id
                                AND json_extract(x.payload_json,'$.revision')=n.revision)
                            AND json_extract(e.payload_json,'$.retry_not_before')>?))",
                        (&t,now),|r|r.get::<_,bool>(0))?)
                })?.await.map_err(|_|CoreError::Refused("native_host_closed"))??;
                if !delayed {
                    // A ready step whose files another task's worker still holds is
                    // queued behind that holding, never stopped as a dead end. The wait is
                    // recorded once per holder; it ends when the holding is released.
                    let waited = self.wait_for_write_holding(task, owner, &cancelled).await?;
                    // The holder may finish between the failed claim and this lookup.
                    if !waited && step["writeConflict"] != true {
                        break step;
                    }
                    continue;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            };
            // Each boundary publishes the pipeline edition its durable state calls for.
            crate::native_pipeline::publish(&self.host.executor, task, None).await;
            // The runtime itself stopped the task with its own question while advancing
            // (a blocking review, a failed final check, a failed worker): that question is
            // the stop, and its answer relaunches the coordinator (`continue_owed_tasks`).
            // This driver ends here; it is not a step that could not continue, and it
            // never replaces the owner's question with a second, different stop.
            if matches!(
                step["state"].as_str(),
                Some("COMPLETED" | "FAILED" | "CANCELLED" | "WAITING_USER")
            ) {
                return Ok(());
            }
            if matches!(
                step["state"].as_str(),
                Some("RECOVERING" | "PLANNING" | "RECONCILING")
            ) {
                // Continue on the task's coordinator regardless of mode or steering.
                // Resolve it before recording delivery of any owner messages.
                self.take_queued(task).await?;
                let route = self.coordinator_route(task, None).await?;
                let t = task.to_owned();
                let o = owner.clone();
                let messages = self.host.executor.write(move|db|{
                    cortex_core::ownership::check_tx(db,&t,&o)?;
                    if cortex_core::lifecycle::domain::task_tx(db,&t)?.0==cortex_core::TaskState::Recovering {
                        cortex_core::lifecycle::domain::settle_recovery_tx(db,&t,cortex_core::TaskState::Planning)?;
                    }
                    let count:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='NATIVE_COORDINATOR_CONTINUED'
                        AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=? AND type IN
                        ('PLAN_ACCEPTED','VERIFICATION_PASSED','VERIFICATION_FAILED','REPLAN_REQUEST_ACCEPTED','USER_ANSWER_RECEIVED','USER_STEERING','AUTOMATIC_RESUMED')),0)",(&t,&t),|r|r.get(0))?;
                    if count>=3 { return Err(CoreError::Refused("coordinator_no_progress_limit")); }
                    let messages=cortex_core::lifecycle::steering::deliver_tx(db,&t)?;
                    cortex_core::lifecycle::event_tx(db,&t,"NATIVE_COORDINATOR_CONTINUED",json!({"state":cortex_core::lifecycle::domain::task_tx(db,&t)?.0}))?;
                    Ok(messages)
                })?.await.map_err(|_|CoreError::Refused("native_host_closed"))??;
                let mut prompt = String::from("Continue from the retained-state capsule. Reconcile verified work with synthesis_submit, ask a structured user question only for a decision about the work itself (never about runtime failures, retries or limits) and keep working on everything it does not block, propose a scoped repair plan for failing project code and keep each unresolved verification command in it. Use verification_submit only to correct the verification procedure or rerun the reviewed work. Never certify missing evidence.");
                if step["state"] != "RECONCILING" {
                    prompt.push_str(&self.recovery_failure_guidance(task).await?);
                }
                if !messages.is_empty() {
                    prompt.push_str("\n\n");
                    prompt.push_str(&cortex_core::lifecycle::steering::guidance_prompt(
                        &messages,
                    ));
                }
                let turn = self
                    .turn(
                        task,
                        owner,
                        SessionRole::Coordinator,
                        route,
                        &prompt,
                        cancelled.clone(),
                    )
                    .await;
                self.confirm_messages(task).await?;
                turn?;
                continue;
            }
            let assignments = step["assignments"]
                .as_array()
                .filter(|a| !a.is_empty())
                .ok_or(CoreError::Refused("native_continuation_unavailable"))?;
            // Claimed workers of one batch run their turns concurrently, each with its own
            // session, channel, write reservation and result. One failing turn, a stale plan
            // or a cancellation stops the others through `batch`; nothing is dropped mid-turn.
            let (batch, batch_cancelled) = watch::channel(false);
            let stale = std::sync::atomic::AtomicBool::new(false);
            let revision = step["revision"]
                .as_i64()
                .ok_or(CoreError::Refused("native_revision_missing"))?;
            let order = Arc::new(WorkerLaunchOrder::default());
            let turn = |assignment: Value| {
                let ticket = order.push(
                    assignment["assignment_ref"].as_str().unwrap().into(),
                    self.host.capacity_changed.clone(),
                );
                let stop = batch_cancelled.clone();
                let batch = &batch;
                let stale = &stale;
                async move {
                    if *stop.borrow() {
                        return Ok(());
                    }
                    let result = self.run_assignment(task, owner, &assignment, stop, ticket).await;
                    self.settle_batch_turn(task, result, batch, stale).await
                }
            };
            let mut turns: FuturesUnordered<_> = assignments.iter().cloned().map(turn).collect();
            let mut parent = cancelled.clone();
            let mut first: Option<CoreError> = None;
            let execution = step["state"] == "EXECUTING";
            let mut check_steering = execution;
            let mut failed_through = 0;
            let mut started_through = 0;
            let mut live_steer = None;
            let wake = self.wake.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            while !turns.is_empty() || live_steer.is_some() {
                if *parent.borrow() || *self.stop.borrow() {
                    batch.send_replace(true);
                }
                if check_steering && live_steer.is_none() && !turns.is_empty()
                    && !*batch_cancelled.borrow()
                {
                    check_steering = false;
                    match self.active_steering_watermark(task, revision).await {
                        Ok(Some(at)) if at > failed_through => {
                            started_through = at;
                            live_steer = Some(Box::pin(self.steer_running(task, owner, batch_cancelled.clone())));
                        }
                        Ok(_) => {},
                        Err(error) => {
                            eprintln!("workspace-service: could not check active steering ({})", error.code());
                        }
                    }
                }
                let result = tokio::select! {
                    biased;
                    Ok(()) = parent.changed(), if !*batch_cancelled.borrow() => continue,
                    result = async { live_steer.as_mut().unwrap().await }, if live_steer.is_some() => {
                        live_steer = None;
                        check_steering = true;
                        if let Err(error) = result {
                            // A missing reply leaves the durable input pending. Do not
                            // restart workers or spin paid retries; wait for newer owner
                            // input or the safe boundary.
                            failed_through = started_through;
                            eprintln!("workspace-service: active steering did not settle ({})", error.code());
                        }
                        Ok(())
                    },
                    _ = &mut wake, if execution && !*batch_cancelled.borrow()
                        && !turns.is_empty() && live_steer.is_none() => {
                        wake.set(self.wake.notified());
                        wake.as_mut().enable();
                        check_steering = true;
                        continue;
                    },
                    Some(result) = turns.next() => result,
                };
                if let Err(error) = result {
                    if first
                        .as_ref()
                        .is_none_or(|e| e.code() == "native_task_interrupted")
                    {
                        first = Some(error);
                    }
                    batch.send_replace(true);
                }
                // Always settle every launched session, including on a refill error.
                if turns.is_empty()
                    || *batch_cancelled.borrow()
                    || *parent.borrow()
                    || *self.stop.borrow()
                {
                    continue;
                }
                match self
                    .host
                    .refill_execution(task.into(), owner.clone(), revision)
                    .await
                {
                    Ok(next) => turns.extend(next.into_iter().map(turn)),
                    Err(error) => {
                        first = Some(error);
                        batch.send_replace(true);
                    }
                }
            }
            if stale.load(std::sync::atomic::Ordering::SeqCst)
                && first
                    .as_ref()
                    .is_some_and(|e| e.code() == "native_task_interrupted")
            {
                first = None;
            }
            if let Some(error) = first {
                return Err(error);
            }
        }
    }
    async fn active_steering_watermark(&self, task: &str, revision: i64) -> CoreResult<Option<i64>> {
        let t = task.to_owned();
        self.host.executor.read(move |db| {
            let executing: bool = db.query_row("SELECT state='EXECUTING' AND revision=? FROM runtime_tasks WHERE task_id=?",
                (revision, &t), |r| r.get(0))?;
            if !executing || cortex_core::lifecycle::automatic::held(db, &t)?
                || cortex_core::lifecycle::steering::pending(db, &t)?.is_empty()
            {
                return Ok(None);
            }
            Ok(db.query_row("SELECT max(id) FROM runtime_events WHERE task_id=? AND type='USER_STEERING'",
                [&t], |r| r.get(0))?)
        })?.await.map_err(|_| CoreError::Refused("native_host_closed"))?
    }

    async fn steer_running(&self, task: &str, owner: &OwnerEpoch, cancelled: watch::Receiver<bool>) -> CoreResult<()> {
        let route = self.coordinator_route(task, None).await?;
        self.turn_once(task, owner, SessionRole::ActiveSteering, route,
            "Handle the owner messages in this live steering snapshot and reply directly to the owner in chat.              Execution continues; this is not a completion or a planning turn.              Use the advertised steering tools only when an owner message warrants a change.              Keep unaffected work running. Do not claim worker instructions were applied from a queue receipt alone.",
            cancelled, None).await
    }

    /// Close one worker turn of a batch. Every exit path, including a failed state read,
    /// signals the batch, so siblings never keep running after this turn is lost.
    ///
    /// A worker question normally leaves the task Executing/Reviewing (only the asking
    /// worker waits). The task moves to WAITING_USER only when the worker has no engine
    /// session reference; then the whole task waits and sibling channels are no longer
    /// authorised (workers act only while EXECUTING/REVIEWING), so they are stopped.
    async fn settle_batch_turn(
        &self,
        task: &str,
        result: CoreResult<()>,
        batch: &watch::Sender<bool>,
        stale: &std::sync::atomic::AtomicBool,
    ) -> CoreResult<()> {
        if result.is_err() {
            batch.send_replace(true);
            return result;
        }
        let t = task.to_owned();
        let state = async {
            Ok::<_, CoreError>(
                self.host
                    .executor
                    .write(move |db| cortex_core::lifecycle::domain::task_tx(db, &t))?
                    .await
                    .map_err(|_| CoreError::Refused("native_host_closed"))??
                    .0,
            )
        }
        .await;
        match state {
            Ok(state) => {
                // A worker proposal can cancel siblings already claimed in this batch.
                if !matches!(
                    state,
                    cortex_core::TaskState::Executing | cortex_core::TaskState::Reviewing
                ) {
                    stale.store(true, std::sync::atomic::Ordering::SeqCst);
                    batch.send_replace(true);
                }
                Ok(())
            }
            Err(error) => {
                batch.send_replace(true);
                Err(error)
            }
        }
    }
    /// The progress budget is persisted (`cortex_core::lifecycle::progress`), so a
    /// restarted driver continues the count instead of resetting it.
    async fn spend_progress(&self, task: &str) -> CoreResult<()> {
        let t = task.to_owned();
        self.host
            .executor
            .write(move |db| {
                cortex_core::lifecycle::progress::spend_tx(
                    db,
                    &t,
                    cortex_core::lifecycle::progress::PROGRESS_BUDGET,
                )
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// Whether a ready step of the task waited for another task's write holding
    /// (`cortex_core::scheduler::policy::write_conflict_tx`). Returns `false` at once
    /// when nothing is held; otherwise records `NATIVE_WRITE_CONFLICT_WAITING` once for
    /// the holder, waits until the holding is gone (or the driver is stopped) and
    /// returns `true` so the caller claims again. Waiting spends no progress budget.
    async fn wait_for_write_holding(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        cancelled: &watch::Receiver<bool>,
    ) -> CoreResult<bool> {
        let mut waited = false;
        loop {
            if *self.stop.borrow() || *cancelled.borrow() {
                return Err(CoreError::Refused("native_service_stopped"));
            }
            let (t, o) = (task.to_owned(), owner.clone());
            let conflict = self
                .host
                .executor
                .write(move |db| {
                    cortex_core::ownership::check_tx(db, &t, &o)?;
                    // Another task's open request for these files is queued ahead of this
                    // step the same way: it waits for that request, never stops.
                    let queued = || -> CoreResult<Option<Value>> {
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                            .map_err(|_| CoreError::Refused("clock_unavailable"))?.as_secs_f64();
                        Ok(cortex_core::scheduler::policy::claim_obstacles_tx(db, &t, now)?.into_iter()
                            .find(|o| o["reason"] == "access_wait" && o["holder_task"].as_str().is_some_and(|h| h != t))
                            .map(|o| json!({"node":o["node"],"holder_task":o["holder_task"],"paths":o["scope"],"access_request":true})))
                    };
                    let Some(conflict) = cortex_core::scheduler::policy::write_conflict_tx(db, &t)?.map(Ok).or_else(|| queued().transpose()).transpose()? else {
                        return Ok(None);
                    };
                    // Record each owner-file blocker distinctly; it has no task ID.
                    let recorded: bool = db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND type='NATIVE_WRITE_CONFLICT_WAITING'
                         AND json_extract(payload_json,'$.holder_task') IS ?2
                         AND json_extract(payload_json,'$.reason') IS ?3
                         AND (?3 IS NULL OR (json_extract(payload_json,'$.node')=?4
                            AND json_extract(payload_json,'$.paths')=?5))
                         AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type IN
                            ('PLAN_ACCEPTED','USER_STEERING','USER_ANSWER_RECEIVED','ASSIGNMENT_CREATED')),0))",
                        (&t, conflict["holder_task"].as_str(), conflict["reason"].as_str(),
                            conflict["node"].as_str(), conflict["paths"].to_string()),
                        |r| r.get(0),
                    )?;
                    let retry = conflict["retry_after_ms"].as_u64();
                    if !recorded {
                        cortex_core::lifecycle::event_tx(db, &t, "NATIVE_WRITE_CONFLICT_WAITING", conflict)?;
                    }
                    Ok(Some(retry))
                })?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??;
            let Some(retry_after_ms) = conflict else {
                return Ok(waited);
            };
            waited = true;
            let wait = std::time::Duration::from_millis(retry_after_ms.unwrap_or(1000).clamp(1, 1000));
            tokio::select! {
                _ = self.wake.notified() => {},
                _ = tokio::time::sleep(wait) => {},
            }
        }
    }
    /// Select a route for one claimed assignment, record it and run the worker's turn.
    async fn run_assignment(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        assignment: &Value,
        cancelled: watch::Receiver<bool>,
        ticket: LaunchTicket,
    ) -> CoreResult<()> {
        let role = assignment["role"]
            .as_str()
            .ok_or(CoreError::Refused("native_role_missing"))?;
        let t = task.to_owned();
        let a = assignment["assignment_ref"].as_str().unwrap().to_owned();
        let root = self.host.root.clone();
        let assigned_role = role.to_owned();
        let (node,evidence,escalated,previous,revision)=self.host.executor.read(move |db| {
            let (plan,node_id,stage,revision):(Option<String>,String,String,i64)=db.query_row(
                "SELECT t.plan_json,n.node_id,n.stage,t.revision FROM runtime_nodes n JOIN runtime_tasks t USING(task_id) WHERE n.task_id=? AND n.assignment_ref=?",
                (&t,&a),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            let plan:Value=serde_json::from_str(plan.as_deref().unwrap_or("{}"))?;
            // Planned nodes route by their own fields; fix and review nodes by the
            // routing recorded when the runtime created them.
            let mut node=cortex_core::lifecycle::plan::node_routing(&plan,&node_id);
            if !node.is_object(){node=json!({});}
            node["id"]=json!(node_id);
            // Only work that fell short escalates (`automatic::node_escalated`): this
            // node's own failure, a repeated review fix, or the one plan revision right
            // after an escalating ladder rung. A retry after an infrastructure failure
            // keeps its normal tier. An escalated dispatch moves away from the engine of
            // the attempt that fell short.
            let escalated=cortex_core::lifecycle::automatic::node_escalated(db,&t,&node_id,revision)?;
            let mut previous=None;
            if escalated {
                previous=cortex_core::routing::previous_dispatch_engine(db,&t,&node_id,revision)?;
                if previous.is_none() {
                    previous=cortex_core::lifecycle::automatic::escalation_engine(db,&t,&node_id,&assigned_role)?;
                }
            }
            if stage=="review"{
                // Independence: the reviewer should run on a model other than the
                // reviewed execution nodes' models (and preferably another engine).
                // Each author carries its node and the number of files its sessions
                // recorded writing or changing, which decides the dominant author.
                let mut stmt=db.prepare("SELECT json_extract(e.payload_json,'$.route_engine'),json_extract(e.payload_json,'$.model'),json_extract(e.payload_json,'$.node'),
                    (SELECT count(DISTINCT json_extract(w.payload_json,'$.path')) FROM runtime_events w JOIN trusted_channels c
                        ON c.task_id=w.task_id AND c.channel_id=json_extract(w.payload_json,'$.channel_id')
                     WHERE w.task_id=e.task_id AND w.type IN ('NATIVE_PROJECT_WRITTEN','NATIVE_PROJECT_CHANGED')
                     AND c.assignment_id=json_extract(e.payload_json,'$.assignment_ref'))
                    FROM runtime_events e WHERE e.task_id=? AND e.type='NATIVE_ROUTE_SELECTED' AND json_extract(e.payload_json,'$.revision')=?
                    AND json_extract(e.payload_json,'$.node') IN (SELECT node_id FROM runtime_nodes WHERE task_id=e.task_id AND revision=? AND stage='execution') ORDER BY e.id")?;
                let reviewed=stmt.query_map((&t,revision,revision),|r|Ok(json!({"engine":r.get::<_,Option<String>>(0)?,"model":r.get::<_,Option<String>>(1)?,
                    "node":r.get::<_,Option<String>>(2)?,"changes":r.get::<_,i64>(3)?})))?
                    .collect::<Result<Vec<_>,_>>()?;
                node["avoid_routes"]=json!(reviewed);
                // Earlier reviews of this revision, and which rerun this review is.
                let previous=db.prepare("SELECT json_extract(e.payload_json,'$.route_engine'),json_extract(e.payload_json,'$.model') FROM runtime_events e
                    WHERE e.task_id=? AND e.type='NATIVE_ROUTE_SELECTED' AND json_extract(e.payload_json,'$.revision')=? AND json_extract(e.payload_json,'$.node')!=?
                    AND json_extract(e.payload_json,'$.node') IN (SELECT node_id FROM runtime_nodes WHERE task_id=e.task_id AND revision=? AND stage='review') ORDER BY e.id")?
                    .query_map((&t,revision,&node_id,revision),|r|Ok(json!({"engine":r.get::<_,Option<String>>(0)?,"model":r.get::<_,Option<String>>(1)?})))?
                    .collect::<Result<Vec<_>,_>>()?;
                node["previous_reviewers"]=json!(previous);
                let rerun:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=?1 AND type='REVIEW_RERUN' AND json_extract(payload_json,'$.revision')=?2
                    AND id<=coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='REVIEW_RERUN' AND json_extract(payload_json,'$.rerun')=?3),0)",
                    (&t,revision,&node_id),|r|r.get(0))?;
                node["review_rerun"]=json!(rerun);
            }
            // A model that lost this node's lineage twice to infrastructure (the session
            // vanished, the engine failed) is routed away from, without escalating the
            // tier: another model when one is usable, preferably another engine.
            let lost=lost_models(db,&t,&node_id)?;
            if !lost.is_empty(){
                let field=if stage=="review"{"previous_reviewers"}else{"avoid_routes"};
                let mut list=node[field].as_array().cloned().unwrap_or_default();
                list.extend(lost.iter().cloned());
                node[field]=json!(list);
                node["lost_routes"]=json!(lost);
            }
            // What a restart left with an unknown outcome is re-verified by the fresh attempt.
            let reverify:Option<String>=db.query_row("SELECT json_extract(payload_json,'$.reverify') FROM runtime_events
                WHERE task_id=? AND type='ASSIGNMENT_RETRIED' AND json_extract(payload_json,'$.node')=? AND json_extract(payload_json,'$.revision')=?
                ORDER BY id DESC LIMIT 1",(&t,&node_id,revision),|r|r.get(0)).or_else(|e|match e{rusqlite::Error::QueryReturnedNoRows=>Ok(None),e=>Err(e)})?;
            if let Some(reverify)=reverify.and_then(|r|serde_json::from_str::<Value>(&r).ok()).filter(|r|r.as_array().is_some_and(|a|!a.is_empty())){
                node["reverify_effects"]=reverify;
            }
            Ok((node,cortex_core::routing::evidence(db,&t,&root)?,escalated,previous,revision))
        })?.await.map_err(|_|CoreError::Refused("native_host_closed"))??;
        let mut context = crate::native_routes::WorkerRouteContext {
            held: self.host.route_holds.active_routes(),
            ..Default::default()
        };
        let (route, mut decision) = match self.routes.worker_selection(
            &self.host.root,
            role,
            &node,
            &evidence,
            escalated,
            previous.as_deref(),
            &mut context,
        ) {
            Ok(selected) => selected,
            Err(error) => {
                if cortex_core::lifecycle::automatic::route_blocked(error.code()) {
                    let t = task.to_owned();
                    let mut probe = json!({"reason":error.code(),"revision":revision,"role":role,
                        "node":node,"escalated":escalated,"previous":previous});
                    if !context.block.is_null() {
                        probe["work_kind_block"] = context.block;
                    }
                    self.host
                        .executor
                        .write(move |db| {
                            cortex_core::lifecycle::event_tx(
                                db,
                                &t,
                                "NATIVE_WORKER_ROUTE_BLOCKED",
                                probe,
                            )?;
                            Ok(())
                        })?
                        .await
                        .map_err(|_| CoreError::Refused("native_host_closed"))??;
                }
                return Err(error);
            }
        };
        // Review independence facts come from the route actually chosen, whoever
        // produced it: the same model as the reviewed work is a visible warning.
        if let Some(avoid) = node["avoid_routes"].as_array().filter(|a| !a.is_empty()) {
            let same_model = avoid.iter().any(|a| a["model"] == json!(route.model));
            let same_engine = !decision["route_engine"].is_null()
                && avoid
                    .iter()
                    .any(|a| a["engine"] == decision["route_engine"]);
            decision["independence"] = if same_model {
                json!({"status":"warning","mode":"same_model_fresh_session","model":route.model,"same_model_as_reviewed":true,
                    "same_engine_as_reviewed":same_engine,
                    "warning":"The reviewer runs on a model that also wrote part of the work it reviews, in a fresh session: no other model the owner prefers for review was usable."})
            } else {
                json!({"status":"independent","model":route.model,"same_engine_as_reviewed":same_engine})
            };
        }
        let t = task.to_owned();
        let a = assignment.clone();
        let selected = json!({"assignment_ref":a["assignment_ref"],"node":a["node"],"revision":revision,"route_engine":decision["route_engine"],"engine":match route.launch.engine {
            cortex_engine::environment::EngineKind::Codex=>"codex-acp",
            cortex_engine::environment::EngineKind::Claude=>"claude-agent-acp",
            cortex_engine::environment::EngineKind::OpenCode=>"opencode"},
            "model":route.model,"connection_id":route.connection,"parameters":route.parameters,"decision":decision});
        self.host
            .executor
            .write(move |db| {
                cortex_core::lifecycle::event_tx(db, &t, "NATIVE_ROUTE_SELECTED", selected)?;
                Ok(())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        // A restart left effects of the previous attempt with an unknown outcome: the
        // fresh attempt re-verifies them from the files, Git state and receipts first.
        let mut brief = assignment.clone();
        if let Some(effects) = node.get("reverify_effects") {
            brief["reverify_effects"] = effects.clone();
            brief["reverify_instruction"] = json!("The service restarted while the previous attempt of this assignment ran. These effects of that attempt may or may not have taken place: re-verify each one from the files, Git state and receipts before redoing it; never replay it blindly.");
        }
        let result = self.turn_once(
            task,
            owner,
            SessionRole::Worker {
                assignment: assignment["assignment_ref"].as_str().unwrap().into(),
            },
            route,
            &brief.to_string(),
            cancelled,
            Some(ticket),
        )
        .await;
        // A degenerate worker turn is replaced by the ordinary lost-worker path.
        match result {
            Err(error) if error.code() == "native_turn_degenerate" => Ok(()),
            other => other,
        }
    }
    /// Whether a driver of this task still runs (installed and not finished).
    pub(super) async fn driven(&self, task: &str) -> bool {
        self.pumps
            .lock()
            .await
            .get(task)
            .is_some_and(|pump| pump.done.has_changed().is_ok() && !*pump.done.borrow())
    }
    pub(super) async fn settle_task(&self, task: &str) -> CoreResult<()> {
        let done = {
            let pumps = self.pumps.lock().await;
            pumps.get(task).map(|pump| {
                pump.cancel.send_replace(true);
                pump.done.clone()
            })
        };
        // A dropped sender means the driver job is gone (it ended without signalling,
        // e.g. it panicked): no driver of this task runs any more. This only fences
        // drivers; whether their engine processes exited is decided separately by the
        // durable provisioning/reservation rows that `control::finish` checks.
        if let Some(mut done) = done {
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        Ok(())
    }
    pub(super) async fn continue_task(
        self: &Arc<Self>,
        task: String,
        owner: OwnerEpoch,
        prompt: String,
        route: SessionRoute,
    ) -> CoreResult<()> {
        self.continue_with_handoff(task, owner, prompt, route, None).await
    }

    pub(super) async fn continue_with_handoff(
        self: &Arc<Self>,
        task: String,
        owner: OwnerEpoch,
        prompt: String,
        route: SessionRoute,
        merge_handoff: Option<String>,
    ) -> CoreResult<()> {
        let (cancel, cancelled) = watch::channel(false);
        let (done, finished) = watch::channel(false);
        self.install_pump(task.clone(), cancel, finished).await;
        let runtime = self.clone();
        let mut jobs = self.jobs.lock().await;
        jobs.retain(|j| !j.is_finished());
        jobs.push(tokio::spawn(async move {
            if let Err(error) = runtime
                .drive(&task, &owner, &prompt, route, cancelled.clone(), merge_handoff)
                .await
            {
                runtime
                    .stop_after_error(&task, &owner, error, cancelled.clone())
                    .await;
            }
            runtime.driver_ended(&task, &cancelled).await;
            done.send_replace(true);
            runtime.reconcile_after_merge_driver(&task).await;
        }));
        Ok(())
    }

    /// Relaunch an Automatic task after a runtime-owned result repair. This path
    /// deliberately bypasses the owner-input admission used by `control_input`.
    /// A scheduler repair is runtime evidence, never an owner message, permission
    /// grant, or an acknowledgement of an unresolved outcome.
    ///
    /// With an expected result version, it reopens exactly that completed result.
    /// Without one, it continues a run that ended as failed with every effect known.
    /// Every distinct failure fingerprint gets one repair (no repair cap); an owner
    /// consent on the queue hold (`owner`) is recorded the same way.
    ///
    /// `Busy` (another control, maintenance, or the service stopping) decides nothing:
    /// the caller asks again on its next pass. `Declined` and an error are refusals.
    pub(crate) async fn continue_automatic_repair(
        self: &Arc<Self>,
        project: &str,
        execution: &str,
        expected_source_version: Option<&str>,
        prompt: String,
        fingerprint: &str,
        owner: bool,
    ) -> CoreResult<RepairAdmission> {
        let Ok(_controls) = self.controls.try_lock() else {
            return Ok(RepairAdmission::Busy);
        };
        if *self.stop.borrow() {
            return Ok(RepairAdmission::Busy);
        }
        let _maintenance = match self
            .host
            .workspace_state
            .as_ref()
            .map(|state| crate::machine::maintenance::admission(state))
            .transpose()
        {
            Ok(guard) => guard,
            Err(_) => return Ok(RepairAdmission::Busy),
        };
        let project_id = project.to_owned();
        let binding_project = project_id.clone();
        let execution_id = execution.to_owned();
        let binding = self
            .host
            .executor
            .publication(move |s| {
                s.read(|db| {
                    cortex_core::workspace::control::binding(
                        db,
                        &s.key().root().to_string_lossy(),
                        &binding_project,
                        &actor(),
                        &execution_id,
                    )
                })
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        let task_id = binding.task.clone();
        let expected_event: Option<i64> = expected_source_version
            .map(|v| v.parse().map_err(|_| CoreError::Refused("stale_result")))
            .transpose()?;
        if expected_event.is_some_and(|e| e <= 0) {
            return Err(CoreError::Refused("stale_result"));
        }
        let route = self.coordinator_route(&task_id, None).await?;
        let fp = fingerprint.to_owned();
        let guidance = prompt.clone();
        let root = self.host.root.clone();
        let project_for_settled = project.to_owned();
        let draft = binding.draft.clone();
        let current_execution = execution.to_owned();
        let epoch = binding.owner.epoch;
        let runtime_task = task_id.clone();
        let reopened = self
            .host
            .executor
            .write(move |db| {
                // The caller (`task_auto_repair::dispatch`) admitted this task under the
                // machine lock as a member of a queue-driven sprint (Hybrid or Automatic).
                let already: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_REPAIR_REQUESTED' AND json_extract(payload_json,'$.fingerprint')=?)",
                    (&runtime_task, &fp),
                    |r| r.get(0),
                )?;
                if already {
                    return Ok(false);
                }
                // No repair cap (no-stop policy): each distinct failure fingerprint gets
                // its repair; only an identical one is never repeated.
                let _ = owner;
                let (state, _) = cortex_core::lifecycle::domain::task_tx(db, &runtime_task)?;
                let expected_state = if expected_event.is_some() {
                    cortex_core::TaskState::Completed
                } else {
                    cortex_core::TaskState::Failed
                };
                if state != expected_state {
                    return Ok(false);
                }
                let current_event: i64 = db.query_row(
                    "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=?",
                    [&runtime_task],
                    |r| r.get(0),
                )?;
                if expected_event.is_some_and(|e| current_event != e) {
                    return Ok(false);
                }
                crate::machine::task_lifecycle::settled(
                    db,
                    &root,
                    &project_for_settled,
                    &draft,
                )?;
                let current_binding = cortex_core::workspace::control::binding(
                    db, &root.to_string_lossy(), &project_for_settled, &actor(), &current_execution,
                )?;
                if current_binding.task != runtime_task || current_binding.owner.epoch != epoch {
                    return Ok(false);
                }
                cortex_core::lifecycle::automatic::require_settled_execution(db, &runtime_task)?;
                let waiting: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pending_questions WHERE task_id=? AND status='open')",
                    [&runtime_task], |r| r.get(0),
                )?;
                if waiting { return Ok(false); }
                let reopened = cortex_core::lifecycle::domain::reopen_tx(db, &runtime_task)?;
                if !reopened {
                    return Ok(false);
                }
                cortex_core::lifecycle::domain::settle_recovery_tx(db, &runtime_task, cortex_core::TaskState::Planning)?;
                cortex_core::lifecycle::event_tx(
                    db,
                    &runtime_task,
                    "AUTOMATIC_REPAIR_REQUESTED",
                    serde_json::json!({"fingerprint": fp, "prompt": guidance, "ownerAllowed": owner}),
                )?;
                Ok(true)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        if !reopened {
            return Ok(RepairAdmission::Declined);
        }
        self.continue_task(task_id, binding.owner, prompt, route)
            .await?;
        Ok(RepairAdmission::Started)
    }
    pub(crate) async fn shutdown(&self) {
        self.stop.send_replace(true);
        let _serial = self.controls.lock().await;
        // Jobs are taken out before they are awaited: a job that ends by starting a
        // continuation follower (which registers itself) must not wait on this lock.
        loop {
            let jobs: Vec<_> = self.jobs.lock().await.drain(..).collect();
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                let _ = job.await;
            }
        }
    }
}
impl Service {
    /// Native embedding/test entry. It accepts host authority, never wire launch fields.
    pub async fn bind_native(
        mut config: crate::Config,
        routes: Arc<dyn NativeRoutes>,
    ) -> io::Result<Arc<Self>> {
        config.launch.native_routes = Some(routes);
        Self::bind(config).await
    }
}
#[cfg(test)]
mod pump_tests {
    use super::*;
    use std::time::Duration;
    // A turn whose state read fails still signals the batch (siblings must not keep running).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_state_read_after_a_turn_still_signals_the_batch() {
        let root = std::env::temp_dir().join(format!("batch-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let (batch, sibling) = watch::channel(false);
        let stale = std::sync::atomic::AtomicBool::new(false);
        let outcome = runtime
            .settle_batch_turn("no-such-task", Ok(()), &batch, &stale)
            .await;
        assert!(outcome.is_err());
        assert!(
            *sibling.borrow(),
            "siblings must be signalled on every exit path"
        );
        // A failed turn signals the batch too.
        let (batch, sibling) = watch::channel(false);
        let _ = runtime
            .settle_batch_turn("no-such-task", Err(CoreError::Refused("x")), &batch, &stale)
            .await;
        assert!(*sibling.borrow());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    // Installing a pump for a task that already has one cancels it and waits for it to
    // finish; the new driver is never visible while the old one may still be running.
    #[tokio::test(flavor = "multi_thread")]
    async fn installing_a_pump_cancels_and_awaits_the_prior_driver() {
        let root = std::env::temp_dir().join(format!("pump-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let (cancel1, mut cancelled1) = watch::channel(false);
        let (done1, finished1) = watch::channel(false);
        runtime.install_pump("t".into(), cancel1, finished1).await;
        let (cancel2, _cancelled2) = watch::channel(false);
        let (_done2, finished2) = watch::channel(false);
        let r = runtime.clone();
        let second =
            tokio::spawn(async move { r.install_pump("t".into(), cancel2, finished2).await });
        cancelled1.changed().await.unwrap();
        assert!(*cancelled1.borrow());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !second.is_finished(),
            "replacement must wait for the prior driver"
        );
        done1.send_replace(true);
        second.await.unwrap();
        assert_eq!(runtime.pumps.lock().await.len(), 1);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    // A driver job that ended without signalling (its sender dropped, e.g. a panic)
    // no longer runs: settling the task must not fail every later owner control.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_driver_gone_without_signalling_is_settled() {
        let root = std::env::temp_dir().join(format!("pump-gone-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let (cancel, mut cancelled) = watch::channel(false);
        let (done, finished) = watch::channel(false);
        runtime.install_pump("t".into(), cancel, finished).await;
        let job = tokio::spawn(async move {
            let _ = cancelled.changed().await;
            drop(done);
        });
        runtime.settle_task("t").await.unwrap();
        job.await.unwrap();
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    // A ready step whose file another unfinished task's worker holds waits for it (one
    // recorded wait), instead of stopping the task as `native_continuation_unavailable`;
    // it claims again once the holder's task is cancelled.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_step_held_by_another_tasks_writer_waits_until_the_holding_ends() {
        let root = std::env::temp_dir().join(format!("held-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let owners = runtime.host.executor.publication(|store| {
            let machine = cortex_core::lifecycle::TaskMachine::new(store);
            let waiting = machine.create("waiting", "Reset test", "owner")?;
            machine.create("holder", "formatAll", "owner")?;
            store.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('holder',1,'formatAll','worker-backend-dev','o','[]','[\"test/counter.test.js\"]','execution','running','ha',NULL)", [])?;
                db.execute("INSERT INTO delegation_provisioning VALUES ('ha','holder',NULL,'hx','worker-backend-dev','ready','ambiguous',NULL,'hc',NULL,NULL,NULL,NULL,NULL,1,'0','0')", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('waiting',1,'reset','worker-backend-dev','o','[]','[\"test\"]','execution','pending',NULL,NULL)", [])?;
                Ok(())
            })?;
            Ok(waiting)
        }).unwrap().await.unwrap().unwrap();
        let (_cancel, cancelled) = watch::channel(false);
        let r = runtime.clone();
        let wait = tokio::spawn(async move {
            r.wait_for_write_holding("waiting", &owners, &cancelled)
                .await
        });
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!wait.is_finished(), "the held step did not wait");
        let count = |runtime: Arc<Runtime>| async move {
            runtime.host.executor.read(|db| Ok(db.query_row(
                "SELECT count(*)||':'||coalesce(max(json_extract(payload_json,'$.holder_task')),'') FROM runtime_events WHERE type='NATIVE_WRITE_CONFLICT_WAITING' AND task_id='waiting'",
                [], |r| r.get::<_, String>(0))?)).unwrap().await.unwrap().unwrap()
        };
        assert_eq!(count(runtime.clone()).await, "1:holder");
        runtime
            .host
            .executor
            .write(|db| {
                db.execute(
                    "UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id='holder'",
                    [],
                )?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .unwrap()
            .unwrap()
            .unwrap());
        assert_eq!(count(runtime.clone()).await, "1:holder");
        // Nothing held: no wait at all.
        let (_cancel, cancelled) = watch::channel(false);
        let owner = runtime
            .host
            .executor
            .read(|db| {
                Ok(db.query_row(
                    "SELECT owner_ref,epoch FROM task_owners WHERE task_id='waiting'",
                    [],
                    |r| {
                        Ok(OwnerEpoch {
                            owner: r.get(0)?,
                            epoch: r.get(1)?,
                        })
                    },
                )?)
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(!runtime
            .wait_for_write_holding("waiting", &owner, &cancelled)
            .await
            .unwrap());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn owner_file_lease_wait_is_visible_and_rechecks_release_and_expiry() {
        use cortex_core::store::owner_file_leases as leases;
        let root = std::env::temp_dir().join(format!("lease-wait-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(tokio::task::spawn_blocking(move || {
            Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
        }).await.unwrap().unwrap());
        for (task, expire) in [("release", false), ("expire", true)] {
            let (owner, epoch, checkout, file, token) = runtime.host.executor.publication(move |store| {
                let checkout = leases::Checkout::identify(store.key().root())?;
                let file = leases::RelativePath::new("file.txt")?;
                let epoch = leases::EditorEpoch::new();
                let editor = leases::Editor { epoch: &epoch, session: "fixture-owner" };
                let owner = cortex_core::lifecycle::TaskMachine::new(store).create(task, "Lease wait", "owner")?;
                let token = store.write(|db| {
                    leases::activate_editor_tx(db, &epoch)?;
                    let token = leases::acquire_tx(db, editor, &checkout, &file)?;
                    db.execute("UPDATE runtime_tasks SET state='EXECUTING', revision=1 WHERE task_id=?", [task])?;
                    db.execute("INSERT INTO runtime_nodes VALUES (?,1,'write','worker-backend-dev','Write','[]','[\"file.txt\"]','execution','pending',NULL,NULL)", [task])?;
                    assert!(cortex_core::scheduler::claim_ready_tx(db, task, &owner, "fixture",
                        &cortex_core::scheduler::policy::NativePolicy::default())?.is_empty());
                    Ok(token)
                })?;
                Ok((owner, epoch, checkout, file, token))
            }).unwrap().await.unwrap().unwrap();
            let r = runtime.clone();
            let waiting_owner = owner.clone();
            let wait = tokio::spawn(async move {
                let (_cancel, cancelled) = watch::channel(false);
                r.wait_for_write_holding(task, &waiting_owner, &cancelled).await
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let observed = runtime.host.executor.read(move |db| Ok(db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_WRITE_CONFLICT_WAITING'
                         AND json_extract(payload_json,'$.reason')='owner_file_lease_held'
                         AND json_extract(payload_json,'$.paths[0]')='file.txt')", [task], |r| r.get::<_, bool>(0))?
                    )).unwrap().await.unwrap().unwrap();
                    if observed { break; }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("owner lease wait was not visible");
            assert!(!wait.is_finished());
            runtime.host.executor.write(move |db| {
                if expire {
                    db.execute("UPDATE code_owner_file_leases SET expires_at=0", [])?;
                } else {
                    let editor = leases::Editor { epoch: &epoch, session: "fixture-owner" };
                    leases::release_tx(db, editor, &checkout, &file, token.expose())?;
                }
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            if !expire { runtime.wake.notify_waiters(); }
            assert!(tokio::time::timeout(Duration::from_secs(5), wait).await.unwrap().unwrap().unwrap());
            runtime.host.executor.write(move |db| {
                let claimed = cortex_core::scheduler::claim_ready_tx(db, task, &owner, "fixture",
                    &cortex_core::scheduler::policy::NativePolicy::default())?;
                assert_eq!(claimed.len(), 1);
                db.execute("UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id=?", [task])?;
                db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=?", [task])?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
        }
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    // A blocking review while the driver advances is a continuation on the strategy
    // ladder (no-stop policy): the runtime asks the owner nothing, the driver ends
    // without recording the step as one that could not continue, and the continuation
    // cites the review report. A later driver error never records a second stop.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_blocking_review_is_a_continuation_and_the_driver_ends() {
        let root = std::env::temp_dir().join(format!("blocked-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        // The review is bound to an observed project revision.
        assert!(std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let owner = runtime.host.executor.publication(|store| {
            let owner = cortex_core::lifecycle::TaskMachine::new(store).create("t", "Serve HTTPS", "owner")?;
            store.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=4,review_cycle=1", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',4,'trace','worker-security-auditor','Trace the proxy path','[]','[\"traefik/**\"]','execution','completed','a1','{}')", [])?;
                let review = json!({"verdict":"BLOCK","summary":"The request chain was not verified.","report_ref":"r_2413b9f526b9","checks":[],
                    "findings":[{"node":"trace","files":["traefik/tls.yml"],"cross_cutting":false,"fix":"Forward the visitor address"}]});
                db.execute("INSERT INTO runtime_nodes VALUES ('t',4,'_review_1','worker-code-reviewer','Review','[\"trace\"]','[\"**\"]','review','completed','rv',?)",
                    [review.to_string()])?;
                Ok(())
            })?;
            Ok(owner)
        }).unwrap().await.unwrap().unwrap();
        let (_cancel, cancelled) = watch::channel(false);
        tokio::time::timeout(
            Duration::from_secs(20),
            runtime.progress("t", &owner, cancelled),
        )
        .await
        .expect("the driver kept running after the continuation")
        .unwrap();
        let facts = runtime
            .host
            .executor
            .read(|db| {
                let stopped: i64 = db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='NATIVE_DRIVER_STOPPED'",
                    [],
                    |r| r.get(0),
                )?;
                let questions: i64 = db.query_row(
                    "SELECT count(*) FROM pending_questions WHERE task_id='t'",
                    [],
                    |r| r.get(0),
                )?;
                let (state, blocker): (String, String) = db.query_row(
                    "SELECT state,blocker FROM runtime_tasks WHERE task_id='t'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let owed = cortex_core::lifecycle::automatic::owed(db, "t")?.map(|o| o.evidence);
                Ok((stopped, questions, state, blocker, owed))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            facts.0, 0,
            "the driver recorded a stop after the continuation"
        );
        assert_eq!(facts.1, 0, "the owner was asked");
        assert_eq!(
            (facts.2.as_str(), facts.3.as_str()),
            ("RECOVERING", "automatic_continuation_pending")
        );
        let evidence = facts.4.expect("a continuation is owed");
        assert_eq!(evidence["reason"], "review_blocked");
        assert_eq!(evidence["strategy"], "repair");
        assert!(evidence["reports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reportId"] == "r_2413b9f526b9"));
        // A driver error arriving now records no second continuation.
        runtime
            .stop_after_error(
                "t",
                &owner,
                CoreError::Refused("native_continuation_unavailable"),
                watch::channel(false).1,
            )
            .await;
        let count = runtime
            .host
            .executor
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='AUTOMATIC_CONTINUATION'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count, 1);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[cfg(test)]
mod discover_probe_tests {
    use super::*;
    // A writing worker's catalogue discovery without arguments (as Codex workers call
    // it) answers with the catalogue's first page, never an authority refusal.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_worker_discovers_its_tool_catalogue_without_arguments() {
        let root = std::env::temp_dir().join(format!("discover-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let outcome = runtime.host.executor.publication(|store| {
            let owner = cortex_core::lifecycle::TaskMachine::new(store).create("t", "g", "owner")?;
            let c = store.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-devops-engineer','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
                let c = cortex_core::channels::issue_tx(db, "t", &owner, cortex_core::channels::Principal::Worker { assignment: "a" })?;
                cortex_core::channels::bind_session_tx(db, &c, "engine")?;
                Ok(c)
            })?;
            let i = store.read(|db| cortex_core::channels::authenticate(db, &c.channel_id, c.secret(), true))?;
            let mut out = Vec::new();
            for args in [json!({}), json!({"limit":20}), json!({"limit":50})] {
                out.push(crate::native_documents::execute(store, &c, &i, "tools_discover", &args).map(|v| v["tools"].as_array().map(Vec::len)).map_err(|e| e.code()));
            }
            Ok(out)
        }).unwrap().await.unwrap().unwrap();
        for o in &outcome {
            assert!(matches!(o, Ok(Some(n)) if *n > 0), "{outcome:?}");
        }
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
