use super::*;
use super::external_probe::probe_external_with_tcp;
use crate::native_route_hold::{Attempt as RouteAttempt, RouteKey};
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

fn owner_cards(runtime: &Arc<Runtime>, root: &Path) -> crate::native_owner_actions::Place {
    crate::native_owner_actions::Place {
        project: "project".into(),
        name: "Fixture".into(),
        root: root.to_owned(),
        executor: runtime.host.executor.clone(),
        runtime: Some(runtime.clone()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn native_obstacle_card_resolution_dispatches_only_the_blocked_node() {
    use crate::native_owner_actions::{act, list, Act};
    let (runtime, owner, task, root) = fixture().await;
    runtime.host.executor.write(|db| {
        let result = json!({"blocker":{"kind":"missing_access","detail":"HTTP 403 from https://registry.npmjs.org/"}});
        db.execute("UPDATE runtime_nodes SET status='failed',assignment_ref='old-work',result_json=? WHERE node_id='next'", [result.to_string()])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'sibling','worker-backend-dev','Done','[]','[]','execution','completed','kept','{}')", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'dependent','worker-backend-dev','Wait','[\"next\"]','[]','execution','pending',NULL,NULL)", [])?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    let wait = runtime.host.advance(task.clone(), owner.clone()).await.unwrap();
    assert_eq!(wait["obstacleWait"], true);
    assert_eq!(wait["state"], "EXECUTING");
    let place = owner_cards(&runtime, &root);
    let cards = list(std::slice::from_ref(&place)).await;
    let card = &cards["actions"][0];
    assert_eq!(card["service"], "registry.npmjs.org");
    let id = card["actionId"].as_str().unwrap();
    assert_eq!(act(&place, id, Act::Dismiss).await.unwrap()["outcome"], "resolved");
    assert!(act(&place, id, Act::Dismiss).await.is_err());
    let retry = runtime.host.advance(task.clone(), owner).await.unwrap();
    let assignments = retry["assignments"].as_array().unwrap();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0]["node"], "next");
    assert_ne!(assignments[0]["assignment_ref"], "old-work");
    assert_eq!(list(&[place]).await["actions"], json!([]));
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_decision_fallback_after_idle_failure_and_live_timeout() {
    use cortex_core::lifecycle::{event_tx, obstacles};
    use std::time::Duration;
    for (language, failure, live) in [
        ("en", "turn_failed", false), ("ru", "timeout", true), ("en", "no_decision", false),
    ] {
        let (mut runtime, owner, task, root) = fixture_with_routes(Arc::new(CapacityPeer)).await;
        runtime.host.executor.write(move |db| {
            let result = json!({"blocker":{"kind":"owner_decision","detail":"Retain the old API?"}});
            db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id='t'",
                [json!({"verification":[["verify-api","--strict"]]}).to_string()])?;
            db.execute("UPDATE runtime_nodes SET dependencies_json='[\"audit\"]' WHERE node_id='next'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'audit','worker-qa-engineer','Audit','[]','[]','execution','completed','audit-worker',?)",
                [json!({"report_ref":"r_0a1b2c3d4e5f"}).to_string()])?;
            db.execute("UPDATE runtime_nodes SET status='failed',assignment_ref='old-work',result_json=? WHERE node_id='next'",
                [result.to_string()])?;
            if live {
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'other','worker-devops-engineer','Keep working','[]','[\"lib\"]','execution','pending',NULL,NULL)", [])?;
            }
            if failure == "no_decision" {
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'technical','worker-backend-dev','Unrelated failure','[]','[]','execution','failed','old-technical','{}')", [])?;
            }
            event_tx(db, "t", "NATIVE_TASK_SETTINGS", json!({"settings":{"language":language}}))?;
            obstacles::classify_failed_tx(db, "t")?;
            assert!(obstacles::decision_trigger(db, "t", None)?.is_some());
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        if failure == "turn_failed" {
            // Restart with a durable trigger and no channel issued yet.
            runtime.shutdown().await;
            drop(runtime);
            let path = root.clone();
            runtime = Arc::new(tokio::task::spawn_blocking(move ||
                Runtime::open(&path, "project", Arc::new(CapacityPeer))
            ).await.unwrap().unwrap());
            std::fs::write(root.join("interrupt-orchestrator"), "").unwrap();
        } else if failure == "no_decision" {
            std::fs::write(root.join("release-orchestrator"), "").unwrap();
        }
        std::fs::write(root.join("owner-decision-fixture"), language).unwrap();
        let (cancel, cancelled) = watch::channel(false);
        let pump = {
            let (runtime, owner, task) = (runtime.clone(), owner.clone(), task.clone());
            tokio::spawn(async move { runtime.progress(&task, &owner, cancelled).await })
        };
        let (question, starts, siblings) = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let result = runtime.host.executor.read(move |db| {
                    use rusqlite::OptionalExtension;
                    let question: Option<String> = db.query_row(
                        "SELECT e.payload_json FROM runtime_events e JOIN pending_questions q
                         ON q.task_id=e.task_id AND q.id=json_extract(e.payload_json,'$.question_id')
                         WHERE e.type='USER_QUESTION_REQUESTED' AND q.assignment_ref='old-work' AND q.status='open'",
                        [], |r| r.get(0),
                    ).optional()?;
                    let starts: i64 = db.query_row("SELECT count(*) FROM runtime_events WHERE type='OWNER_DECISION_COORDINATOR_STARTED'",
                        [], |r| r.get(0))?;
                    let siblings: i64 = db.query_row("SELECT count(*) FROM capacity_reservations WHERE kind='worker' AND state='reserved'",
                        [], |r| r.get(0))?;
                    Ok((question, starts, siblings))
                }).unwrap().await.unwrap().unwrap();
                if let (Some(question), starts, siblings) = result {
                    break (serde_json::from_str::<Value>(&question).unwrap(), starts, siblings);
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }).await.unwrap();
        assert!(root.join("entered-orchestrator").exists(), "the peer did not receive the decision prompt for {failure}: {question}");
        assert_eq!(question["fallback_reason"], failure);
        assert_eq!(question["language"], language);
        assert_eq!(starts, 1);
        assert_eq!(siblings, i64::from(live));
        if language == "ru" {
            assert!(question["question"].as_str().unwrap().starts_with("Как продолжить эту работу?"));
            assert!(question["question"].as_str().unwrap().contains("«Retain the old API?»"));
            assert_eq!(question["options"][0], "Продолжить только уже разрешённую работу");
        } else {
            assert!(question["question"].as_str().unwrap().starts_with("How should this work continue?"));
            assert!(question["question"].as_str().unwrap().contains("“Retain the old API?”"));
            assert_eq!(question["options"][0], "Continue only with work already authorized");
        }
        assert_eq!(question["options"][1]["recommended"], true);
        if failure == "no_decision" {
            // Another node's failure must not move the task to reconciliation
            // while this exact question still awaits an owner.
            let step = runtime.host.advance(task.clone(), owner.clone()).await.unwrap();
            assert_eq!(step["obstacleWait"], true, "{step}");
            assert_eq!(step["state"], "EXECUTING", "{step}");
        }
        cancel.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), pump).await.unwrap().unwrap();
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn native_sprint_plan_review_dispatches_with_an_active_writer() {
    use cortex_core::{lifecycle, scheduler};
    let (runtime, owner, task, root) = fixture().await;
    runtime.host.executor.publication(|store| {
        let peer = lifecycle::TaskMachine::new(store).create("peer", "Other work", "peer-owner")?;
        store.write(|db| {
            let policy = scheduler::policy::NativePolicy::default();
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='peer'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('peer',1,'write','worker-backend-dev','Work','[]','[\"backend/\",\"dashboard/\",\"docs/\"]','execution','pending',NULL,NULL)", [])?;
            assert_eq!(scheduler::claim_ready_tx(db, "peer", &peer, "fixture", &policy)?.len(), 1);
            db.execute("DELETE FROM runtime_nodes WHERE task_id='t'", [])?;
            db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'", [])?;
            lifecycle::event_tx(db, "t", "SPRINT_PLANNING_BOUND", json!({"policy":lifecycle::sprint_planning::POLICY}))?;
            let tasks = json!([{"key":"first","title":"First","description":"Do the work","dependsOn":[]}]);
            let args = json!({"request_key":"submit","tasks":tasks});
            assert_eq!(lifecycle::sprint_planning::submit_tx(db, "t", "coordinator", "submit", &args, &tasks)?["review"], "scheduled");
            assert!(scheduler::policy::write_conflict_tx(db, "t")?.is_none());
            let obstacles = scheduler::policy::claim_obstacles_tx(db, "t", policy.now_seconds)?;
            assert_eq!(obstacles[0]["reason"], "claimable");
            // The plan review does not reserve the checkout in the other direction.
            db.execute("INSERT INTO runtime_nodes VALUES ('peer',1,'also-write','worker-backend-dev','Work','[]','[\"frontend/\"]','execution','pending',NULL,NULL)", [])?;
            assert_eq!(scheduler::claim_ready_tx(db, "peer", &peer, "fixture", &policy)?.len(), 1);
            Ok(())
        })
    }).unwrap().await.unwrap().unwrap();

    let step = runtime.host.advance(task, owner).await.unwrap();
    assert_eq!(step["state"], "REVIEWING");
    assert_eq!(step["writeConflict"], false);
    assert_eq!(step["assignments"].as_array().unwrap().len(), 1);
    assert_eq!(step["assignments"][0]["node"], "_plan_review_1");
    assert_eq!(step["assignments"][0]["stage"], "review");
    runtime.host.executor.read(|db| {
        assert_eq!(db.query_row("SELECT status FROM runtime_nodes WHERE task_id='peer' AND node_id='write'", [], |r| r.get::<_, String>(0))?, "running");
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn native_sprint_revision_review_does_not_bind_checkout_bytes() {
    use cortex_core::lifecycle::{self, completion::EvidenceSnapshot};
    let (runtime, owner, task, root) = fixture().await;
    runtime.host.executor.publication(|store| {
        lifecycle::TaskMachine::new(store).create("peer", "Other work", "peer-owner")?;
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='peer'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('peer',1,'write','worker-backend-dev','Write','[]','[\"docs/\"]','execution','running','peer-work',NULL)", [])?;
            db.execute("UPDATE runtime_tasks SET state='REVIEWING' WHERE task_id='t'", [])?;
            db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id='t'", [])?;
            lifecycle::event_tx(db, "t", "NATIVE_PROJECT_WRITTEN", json!({"path":"src/file.rs"}))?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'_sprint_revision_review_1','worker-code-reviewer','Review proposal','[]','[\"**\"]','review','pending',NULL,NULL)", [])?;
            Ok(())
        })
    }).unwrap().await.unwrap().unwrap();
    let step = runtime.host.advance(task, owner).await.unwrap();
    assert_eq!(step["writeConflict"], false);
    let item = &step["assignments"][0];
    assert_eq!(item["node"], "_sprint_revision_review_1");
    let assignment = item["assignment_ref"].as_str().unwrap().to_owned();
    runtime.host.executor.write(move |db| {
        assert_eq!(lifecycle::completion::review_bound_files(db, "t", &assignment)?, Some(vec![]));
        assert!(lifecycle::completion::review_change_set_limits_tx(db, "t", &assignment)?.is_empty());
        let unrelated_checkout = EvidenceSnapshot {
            project_digest: Some("f".repeat(64)),
            ..Default::default()
        };
        assert!(lifecycle::review::preflight_result_tx(
            db, "t", &assignment, "review-result", &unrelated_checkout,
        )?.is_none());
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn native_obstacle_review_retry_gets_a_fresh_binding_on_the_same_node() {
    use crate::native_owner_actions::{act, list, Act};
    let (runtime, owner, task, root) = fixture().await;
    assert!(std::process::Command::new("/usr/bin/git").args(["init", "-q"])
        .current_dir(&root).status().unwrap().success());
    runtime.host.executor.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='REVIEWING',review_cycle=1 WHERE task_id='t'", [])?;
        db.execute("UPDATE runtime_nodes SET status='completed',assignment_ref='old-work',result_json='{}' WHERE node_id='next'", [])?;
        cortex_core::lifecycle::event_tx(db, "t", "NATIVE_PROJECT_WRITTEN", json!({"path":"src/main.rs"}))?;
        let result = json!({"verdict":"INCOMPLETE","findings":[],"report_ref":"r_aaaaaaaaaaaa",
            "not_covered":[{"item":"registry evidence","blocker":"missing_access",
                "reason":"HTTP 403 from https://registry.npmjs.org/"}]});
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'_review_1','worker-code-reviewer','Review','[\"next\"]','[\"**\"]','review','completed','old-review',?)",
            [result.to_string()])?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    let waiting = runtime.host.advance(task.clone(), owner.clone()).await.unwrap();
    assert_eq!(waiting["obstacleWait"], true);
    assert_eq!(waiting["state"], "REVIEWING");
    runtime.host.executor.publication(|store| {
        let peer = cortex_core::lifecycle::TaskMachine::new(store).create("peer", "Other work", "peer-owner")?;
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='peer'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('peer',1,'write','worker-backend-dev','Write','[]','[\"src\"]','execution','pending',NULL,NULL)", [])?;
            let claimed = cortex_core::scheduler::claim_ready_tx(
                db, "peer", &peer, "fixture", &cortex_core::scheduler::policy::NativePolicy::default(),
            )?;
            assert_eq!(claimed.len(), 1, "a parked review must not retain the checkout");
            Ok(())
        })
    }).unwrap().await.unwrap().unwrap();
    let place = owner_cards(&runtime, &root);
    let cards = list(std::slice::from_ref(&place)).await;
    let card = cards["actions"][0]["actionId"].as_str().unwrap();
    act(&place, card, Act::Dismiss).await.unwrap();
    let blocked = runtime.host.advance(task.clone(), owner.clone()).await.unwrap();
    assert_eq!(blocked["writeConflict"], true, "owner resolution does not bypass the writer");
    assert_eq!(blocked["assignments"], json!([]));
    runtime.host.executor.write(|db| {
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id='peer'", [])?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    let retry = runtime.host.advance(task, owner).await.unwrap();
    assert_eq!(retry["state"], "REVIEWING");
    assert_eq!(retry["revision"], 1);
    let item = &retry["assignments"][0];
    assert_eq!(item["node"], "_review_1");
    assert_eq!(item["stage"], "review");
    assert_eq!(item["role"], "worker-code-reviewer");
    assert_ne!(item["assignment_ref"], "old-review");
    assert!(item["review_packet"].is_object());
    let fresh = item["assignment_ref"].as_str().unwrap().to_owned();
    runtime.host.executor.read(move |db| {
        let events: Vec<(String, String)> = db.prepare(
            "SELECT type,json_extract(payload_json,'$.assignment_ref') FROM runtime_events
             WHERE task_id='t' AND type IN ('REVIEW_STARTED','REVIEW_PACKET') ORDER BY id",
        )?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_,_>>()?;
        assert_eq!(events, [("REVIEW_PACKET".into(), fresh.clone()), ("REVIEW_STARTED".into(), fresh)]);
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    drop(place);
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn native_obstacle_recheck_observes_a_real_loopback_listener_down_then_up() {
    use cortex_core::lifecycle::obstacles;
    let (runtime, _, task, root) = fixture().await;
    let reserved = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = reserved.local_addr().unwrap();
    drop(reserved);
    let due = runtime.host.executor.write(|db| {
        let found = obstacles::classify(&obstacles::Signals {
            blocker_kind: Some("tool_unavailable".into()),
            blocker_detail: Some("Failed to connect to api.github.com port 443: Connection refused".into()),
            ..Default::default()
        });
        assert_eq!(found.class, obstacles::EXTERNAL_OUTAGE);
        obstacles::open_obstacle_tx(db, "t", &found, Some("next"), 1)?;
        Ok(obstacles::open_waits(db, "t")?[0]["next_check_at"].as_i64().unwrap())
    }).unwrap().await.unwrap().unwrap();
    let probe = |wait: Value| async move {
        probe_external_with_tcp(&wait, |host| {
            assert_eq!(host, "api.github.com");
            std::future::ready(Ok(["1.1.1.1:443".parse::<std::net::SocketAddr>().unwrap()]))
        }, |pinned| async move {
            assert_eq!(pinned, "1.1.1.1:443".parse::<std::net::SocketAddr>().unwrap());
            // Replace only the fixture's socket dial. Runtime address validation
            // still runs; production never permits a loopback destination.
            tokio::net::TcpStream::connect(address).await.map(drop)
        }).await
    };
    runtime.recheck_obstacles_at(&task, due, probe).await.unwrap();
    runtime.host.executor.read(move |db| {
        let waits = obstacles::open_waits(db, "t")?;
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0]["attempt"], 1);
        assert_eq!(waits[0]["next_check_at"], due + 120);
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    runtime.recheck_obstacles_at(&task, due + 119, |_| async {
        panic!("the probe is not due yet");
    }).await.unwrap();
    runtime.recheck_obstacles_at(&task, due + 120, probe).await.unwrap();
    let (_, peer) = tokio::time::timeout(std::time::Duration::from_secs(2), listener.accept())
        .await.unwrap().unwrap();
    assert!(peer.ip().is_loopback());
    runtime.host.executor.read(|db| {
        assert!(obstacles::open_waits(db, "t")?.is_empty());
        let events = db.prepare("SELECT json_extract(payload_json,'$.ok') FROM runtime_events
            WHERE task_id='t' AND type='EXTERNAL_WAIT_RECHECKED' ORDER BY id")?
            .query_map([], |r| r.get::<_, bool>(0))?.collect::<Result<Vec<_>,_>>()?;
        assert_eq!(events, [false, true]);
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    drop(listener);
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

struct CapacityPeer;
impl NativeRoutes for CapacityPeer {
    fn owner_decision_timeout(&self, _root: &Path) -> std::time::Duration {
        std::time::Duration::from_secs(2)
    }

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
if [ -f "$0/observe-owner-language" ] || { [ "$1" = orchestrator ] && [ -f "$0/owner-decision-fixture" ]; }; then mode=read-only; fi
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
    *'"method":"session/cancel"'*)
        if [ -n "${held_prompt:-}" ]; then
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"cancelled"}}\n' "$held_prompt"
            held_prompt=
        fi ;;
    *'"method":"session/prompt"'*)
        if [ "$1" = orchestrator ] && [ -f "$0/owner-decision-fixture" ]; then
            language=$(/usr/bin/cat "$0/owner-decision-fixture")
            case "$line" in
                *"Owner-selected response language: $language."*'owner_decisions_so_far'*'Blocked execution node (runtime data)'*'assignment_brief'*'verify-api'*'--strict'*'r_0a1b2c3d4e5f'*'Retain the old API?'*) : ;;
                *) exit 22 ;;
            esac
        fi
        if [ -f "$0/observe-guidance-load" ]; then
            case "$line" in
                *'Coordinator guidance, oldest first'*)
                    : > "$0/guided-$1" ;;
                *'\n\nShort"'*)
                    : > "$0/answered-$1" ;;
                *'Your turn ended, but this assignment has no recorded result yet.'*)
                    : ;;
                *)
                    : > "$0/entered-$1"
                    if [ "$1" = next ] && [ ! -f "$0/observe-open-question" ]; then
                        held_prompt=$id
                        continue
                    fi
                    while [ ! -f "$0/release-$1" ]; do /usr/bin/sleep .02; done ;;
            esac
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            continue
        fi
        if [ -f "$0/observe-owner-language" ]; then
            instructions=$(printf '%s\n' "$CODEX_CONFIG" | /usr/bin/sed -n 's/.*"model_instructions_file":"\([^"]*\)".*/\1/p')
            directive=no
            reminder=no
            if /usr/bin/grep -Fq 'Owner-selected response language: ru. Write in it everything the owner reads directly:' "$instructions"; then directive=yes; fi
            if /usr/bin/grep -Fq 'After submitting a plan or reading English tool output, do not recap the plan to the owner.' "$instructions"; then reminder=yes; fi
            case "$line" in
                *'Live steering snapshot'*'steering fixture'*) kind=steering ;;
                *'Recorded owner answer: Short'*) kind=answer ;;
                *'Continue from the retained-state capsule'*) kind=reconciliation ;;
                *'Initial language fixture'*) kind=initial ;;
                *) kind=unexpected ;;
            esac
            printf '%s %s %s %s\n' "$kind" "$$" "$directive" "$reminder" >> "$0/owner-language"
        fi
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

// A stored task route that is no longer allowed never stops the task: it runs
// on the default coordinator and a single quiet fallback event is recorded.
#[tokio::test(flavor = "multi_thread")]
async fn stored_route_with_removed_model_falls_back_to_default_coordinator() {
    let (runtime, _owner, task, root) = fixture_with_routes(Arc::new(CapacityPeer)).await;
    runtime
        .host
        .executor
        .write(|db| {
            cortex_core::lifecycle::event_tx(db, "t", "NATIVE_TASK_SETTINGS",
                json!({"settings":{"model":"xr_removed","effort":"high","mode":"auto","language":"en"}}))?;
            Ok(())
        })
        .unwrap().await.unwrap().unwrap();
    for _ in 0..2 {
        let route = runtime.coordinator_route(&task, None).await.unwrap();
        assert_eq!(route.model, "fixture-model");
    }
    let fallbacks: i64 = runtime.host.executor
        .read(|db| Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='NATIVE_ROUTE_FALLBACK'", [], |r| r.get(0))?))
        .unwrap().await.unwrap().unwrap();
    assert_eq!(fallbacks, 1);
    let (route, _) = start_coordinator(&CapacityPeer, &root,
        Some(&json!({"model":"xr_removed","effort":"high"}))).unwrap();
    assert_eq!(route.model, "fixture-model");
}

// The fixture drives the real coordinator session path and inspects what the
// fake Codex peer receives on each prompt. Seed checkpoints in the disposable
// store so no project command or worker namespace is required.
#[tokio::test(flavor = "multi_thread")]
async fn coordinator_owner_language_survives_answer_reconciliation_and_steering() {
    use cortex_core::lifecycle::{event_tx, skill_proposals, waits};

    let (runtime, owner, task, root) = fixture_with_routes(Arc::new(CapacityPeer)).await;
    let executor = &runtime.host.executor;
    executor
        .write(|db| {
            db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'", [])?;
            event_tx(db, "t", "NATIVE_TASK_SETTINGS", json!({"settings":{"language":"ru"}}))?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    std::fs::write(root.join("observe-owner-language"), "").unwrap();

    let (_cancel, cancelled) = watch::channel(false);
    let initial = {
        let (runtime, task, owner, cancelled) =
            (runtime.clone(), task.clone(), owner.clone(), cancelled.clone());
        let route = runtime.coordinator_route(&task, None).await.unwrap();
        tokio::spawn(async move {
            runtime
                .turn_once(
                    &task,
                    &owner,
                    SessionRole::Coordinator,
                    route,
                    "Initial language fixture",
                    cancelled,
                    None,
                )
                .await
        })
    };
    entered(&root, "orchestrator").await;

    let fixture_root = root.clone();
    executor
        .write(move |db| {
            let channel: String = db.query_row(
                "SELECT channel_id FROM delegation_provisioning WHERE task_id='t' AND status='ready'",
                [],
                |r| r.get(0),
            )?;
            let receipt = skill_proposals::request(db, &fixture_root, "t", &channel, "format",
                &json!({"question":"Какой формат?","options":["Long","Short"],"recommended":1}))?;
            waits::answer_tx(db, "t", receipt["question_id"].as_str().unwrap(), "Short", "text", None)?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    std::fs::write(root.join("release-orchestrator"), "").unwrap();
    end_provider_turn(initial).await.unwrap();
    assert_eq!(starts(&root), ["orchestrator"]);

    executor
        .write(|db| {
            db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id='t'", [])?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let route = runtime.coordinator_route(&task, None).await.unwrap();
    runtime
        .turn_once(
            &task,
            &owner,
            SessionRole::Coordinator,
            route,
            "Continue from the retained-state capsule.",
            cancelled.clone(),
            None,
        )
        .await
        .unwrap();

    executor
        .write(|db| {
            let plan = json!({"tasks":[{"id":"next","role":"worker-devops-engineer",
                "dependencies":[]}],"verification":[]});
            db.execute(
                "UPDATE runtime_tasks SET state='EXECUTING',plan_json=? WHERE task_id='t'",
                [plan.to_string()],
            )?;
            event_tx(db, "t", "USER_STEERING",
                json!({"request":"steer","text":"steering fixture","delivery":"boundary"}))?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let through = runtime.active_steering_watermark(&task, 1).await.unwrap().unwrap();
    assert!(matches!(
        runtime.steer_running(&task, &owner, cancelled, 0, through).await.unwrap(),
        TurnOutcome::SteeringUnconfirmed
    ));

    assert_eq!(starts(&root), ["orchestrator", "orchestrator", "orchestrator"]);
    let observations = std::fs::read_to_string(root.join("owner-language")).unwrap();
    let rows: Vec<Vec<_>> = observations
        .lines()
        .map(|line| line.split_whitespace().collect())
        .collect();
    assert_eq!(rows.len(), 4, "{observations}");
    for (row, kind) in rows.iter().zip(["initial", "answer", "reconciliation", "steering"]) {
        let [observed_kind, _, "yes", "yes"] = row.as_slice() else {
            panic!("missing language directive or reminder: {observations}");
        };
        assert_eq!(*observed_kind, kind, "{observations}");
    }
    assert_eq!(
        rows[0][1], rows[1][1],
        "answer must reuse the healthy coordinator process"
    );
    tokio::task::spawn_blocking(move || drop(runtime)).await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn steering_guidance_reader_backpressure_preserves_running_prompt() {
    guidance_reader_backpressure(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn steering_guidance_reader_backpressure_preserves_question_wait() {
    guidance_reader_backpressure(true).await;
}

async fn guidance_reader_backpressure(question: bool) {
    use cortex_core::lifecycle::{event_tx, waits};
    use std::time::Duration;

    let (runtime, owner, task, root) = fixture_with_routes(Arc::new(CapacityPeer)).await;
    let executor = &runtime.host.executor;
    let claims = capacity_claims(&runtime, &owner, &task).await;
    std::fs::write(root.join("observe-guidance-load"), "").unwrap();
    if question {
        std::fs::write(root.join("observe-open-question"), "").unwrap();
    }
    let assignment = claims[0]["assignment_ref"].as_str().unwrap().to_owned();
    let sibling = CapacityBatch::new();
    let peer = launched(sibling.launch(&runtime, &owner, &claims[3])).await;
    let sibling_job = tokio::spawn(async move {
        let turn = peer.prompt("Hold sibling prompt").await;
        let settled = peer.shutdown().await;
        settled.unwrap();
        turn.unwrap().stop_reason
    });
    entered(&root, "other").await;

    let (_cancel, cancelled) = watch::channel(false);
    let mut job = {
        let (runtime, task, owner, assignment) = (
            runtime.clone(), task.clone(), owner.clone(), assignment.clone(),
        );
        let route = CapacityPeer.route(&root, "next").unwrap();
        tokio::spawn(async move {
            runtime.turn_once(&task, &owner, SessionRole::Worker { assignment }, route,
                "Hold target prompt", cancelled, None).await
        })
    };
    entered(&root, "next").await;
    tokio::time::timeout(Duration::from_secs(5), runtime.host.guidance_read_idle.notified())
        .await.expect("target's live prompt did not park its guidance observer");
    let target = executor.write(move |db| {
        let (channel, attempt): (String, String) = db.query_row(
            "SELECT channel_id,attempt_id FROM delegation_provisioning WHERE task_id='t' AND idempotency_key=?",
            [&assignment], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let question_id = if question {
            let form = waits::QuestionForm {
                question: "Which output?".into(), options: vec![json!("Short"), json!("Long")],
            };
            waits::request_tx_bound(db, "t", &channel, "guidance-load-question", &[form])?
                ["question_id"].as_str().map(str::to_owned)
        } else {
            None
        };
        Ok((attempt, question_id))
    }).unwrap().await.unwrap().unwrap();
    if question {
        assert!(target.1.is_some());
        std::fs::write(root.join("release-next"), "").unwrap();
        tokio::time::timeout(Duration::from_secs(5), runtime.host.guidance_question_wait.notified())
            .await.expect("target did not reach its assignment-local question wait");
        tokio::time::timeout(Duration::from_secs(5), runtime.host.guidance_read_idle.notified())
            .await.expect("target's open question did not park its guidance observer");
    }

    // Pin all four actual reader lanes, then fill each queue. Blocking
    // readers release on timeout even if an assertion aborts the test.
    let (entered, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    // The worker can enqueue a read between our submissions, so a
    // single rotation of submissions need not occupy four different lanes.
    for _ in 0..8 {
        let (gate, entered) = (gate.clone(), entered.clone());
        executor.read(move |_| {
            let _ = entered.send(());
            let (lock, wake) = &*gate;
            let _guard = wake.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(8), |open| !*open).unwrap();
            Ok(())
        }).unwrap();
    }
    for _ in 0..4 {
        tokio::time::timeout(Duration::from_secs(5), observed.recv()).await.unwrap().unwrap();
    }
    for _ in 0..528 {
        if let Err(error) = executor.read(|_| Ok(())) {
            assert_eq!(error.code(), "busy");
        }
    }
    for _ in 0..4 {
        assert_eq!(executor.read(|_| Ok(())).unwrap_err().code(), "busy");
    }
    let (attempt, question_id) = target;
    let guidance_id = executor.write(move |db| {
        event_tx(db, "t", "NATIVE_COORDINATOR_GUIDANCE_QUEUED",
            json!({"attempt":attempt,"text":"Use the short output","source_request":"steer-load"}))
    }).unwrap().await.unwrap().unwrap();
    runtime.host.guidance_changed.notify_waiters();
    tokio::time::timeout(Duration::from_secs(5), runtime.host.guidance_read_busy.notified())
        .await.expect("guidance observer did not hit the saturated reader queue");
    assert!(!job.is_finished());
    assert!(!sibling_job.is_finished());
    assert!(!root.join("guided-next").exists());
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();

    tokio::select! {
        biased;
        _ = async {
            while !root.join("guided-next").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => {},
        result = &mut job => panic!("worker ended before guidance reached it: {result:?}"),
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            panic!("guidance did not resume on the original worker");
        },
    }
    if let Some(id) = question_id {
        executor.write(move |db| {
            let status: String = db.query_row(
                "SELECT status FROM pending_questions WHERE id=?", [&id], |row| row.get(0),
            )?;
            assert_eq!(status, "open");
            waits::answer_tx(db, "t", &id, "Short", "text", None)?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        runtime.wake.notify_waiters();
    }
    end_provider_turn(job).await.unwrap();
    assert_eq!(root.join("answered-next").exists(), question);
    assert!(!sibling_job.is_finished());
    std::fs::write(root.join("release-other"), "").unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), sibling_job).await.unwrap().unwrap(), "end_turn");
    assert_eq!(starts(&root), ["other", "next"]);
    executor.read(move |db| {
        let counts: (i64, i64, i64) = db.query_row(
            "SELECT (SELECT count(*) FROM runtime_events WHERE type='NATIVE_COORDINATOR_GUIDANCE_SETTLED'
                    AND json_extract(payload_json,'$.guidance_id')=? AND json_extract(payload_json,'$.outcome')='delivered'),
                    (SELECT count(*) FROM runtime_events WHERE type='ASSIGNMENT_RETRIED'),
                    (SELECT count(*) FROM capacity_reservations WHERE state='released')",
            [guidance_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(counts, (1, 0, 2));
        let resumed: i64 = db.query_row(
            "SELECT count(*) FROM runtime_events WHERE type='WORKER_WAIT_CHANGED'
                AND json_extract(payload_json,'$.status')='resumed'", [], |row| row.get(0),
        )?;
        assert_eq!(resumed, i64::from(question));
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    tokio::task::spawn_blocking(move || drop(runtime)).await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
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
                            &json!({"text":"Final synthesis","not_verified":[],"outcomes":[{"id":"goal","status":"done","evidence":"fixture"}]}),
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
        // The owner sees the recorded code (a review block reads as its own code).
        assert_eq!(
            automatic::projection(db, &task)?.unwrap()["pause"]["reason"],
            automatic::NO_MODEL_LISTED_FOR_WORK_KIND
        );
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
