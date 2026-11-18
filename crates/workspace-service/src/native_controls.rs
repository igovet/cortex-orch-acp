//! Desktop controls for the selected native owner, never a Python mutation fallback.
#[cfg(test)]
mod capture_replay_tests {
    use super::*;
    use crate::native_runtime::UnqualifiedRoutes;
    use cortex_core::{
        input::{Block, Capture},
        workspace,
    };

    fn capture(body: &[u8]) -> Capture {
        Capture {
            blocks: vec![Block {
                index: 0,
                kind: "resource".into(),
                mime: Some("text/plain".into()),
                name: Some("note.txt".into()),
                uri: None,
                availability: "stored".into(),
                reason: None,
                body: Some(body.to_vec()),
            }],
        }
    }

    async fn fixture(finished: bool) -> (std::path::PathBuf, Arc<Runtime>, Value, Option<Value>) {
        let root =
            std::env::temp_dir().join(format!("control-replay-{}", crate::identity().unwrap()));
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
        let (request, receipt) = runtime.host.executor.publication(move |store| {
            let command = json!({"protocol":{"major":1,"minor":8},"requestId":"draft",
                "projectId":"project","actorId":actor(),"operation":"tasks.create","target":"project",
                "expectedVersion":"0","ownerEpoch":null,
                "payload":{"description":"Capture replay","checkout":{"kind":"main"}}});
            let draft = workspace::create_draft(store, "project", &actor(), &command)?.receipt;
            let start = json!({"protocol":{"major":1,"minor":8},"requestId":"start",
                "projectId":"project","actorId":actor(),"operation":"tasks.start",
                "target":draft["target"],"expectedVersion":draft["entityVersion"],
                "ownerEpoch":null,"payload":{}});
            let started = workspace::start_native(store, "project", &actor(), &start)?;
            let request = json!({"projectId":"project","executionId":started["receipt"]["target"],
                "requestId":"followup","action":"incoming_prompt","text":"Read the note"});
            let captured = capture(b"original");
            // Seed a real admitted control and (optionally) its settled receipt,
            // without launching an engine. Replay must precede route lookup.
            let mut canonical = request.clone();
            canonical["capture"] = captured.fingerprint();
            control::admit_input(store, "project", &actor(), &canonical, Some(&captured))?;
            let receipt = if finished {
                Some(control::finish(store, "project", &actor(), &canonical, true)?)
            } else { None };
            Ok((request, receipt))
        }).unwrap().await.unwrap().unwrap();
        (root, runtime, request, receipt)
    }

    async fn snapshot(runtime: &Runtime) -> Value {
        runtime
            .host
            .executor
            .read(|db| {
                let mut result = serde_json::Map::new();
                for table in [
                    "source_revisions",
                    "source_blocks",
                    "host_transcript",
                    "runtime_events",
                    "runtime_requests",
                    "workspace_outbox",
                ] {
                    let n: i64 =
                        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
                    result.insert(table.into(), json!(n));
                }
                let state: String =
                    db.query_row("SELECT state FROM runtime_tasks", [], |r| r.get(0))?;
                result.insert("state".into(), json!(state));
                Ok(Value::Object(result))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap()
    }

    async fn close(root: std::path::PathBuf, runtime: Arc<Runtime>) {
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn identical_capture_retry_returns_receipt_without_new_effects() {
        let (root, runtime, request, receipt) = fixture(true).await;
        let before = snapshot(&runtime).await;
        assert_eq!(before["source_blocks"], 1);
        for _ in 0..2 {
            let replay = runtime
                .control_input(
                    "project".into(),
                    request.clone(),
                    Some(capture(b"original")),
                )
                .await
                .unwrap();
            assert_eq!(Some(replay), receipt);
            assert_eq!(snapshot(&runtime).await, before);
        }
        close(root, runtime).await;
    }

    #[tokio::test]
    async fn changed_capture_retry_conflicts_without_new_effects() {
        let (root, runtime, request, _) = fixture(true).await;
        let before = snapshot(&runtime).await;
        let mut metadata = capture(b"original");
        metadata.blocks[0].name = Some("renamed.txt".into());
        for changed in [Some(capture(b"changed")), Some(metadata), None] {
            let error = runtime
                .control_input("project".into(), request.clone(), changed)
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "payload_conflict");
            assert_eq!(snapshot(&runtime).await, before);
        }
        close(root, runtime).await;
    }

    #[tokio::test]
    async fn unsettled_capture_retry_remains_unknown_without_new_effects() {
        let (root, runtime, request, _) = fixture(false).await;
        let before = snapshot(&runtime).await;
        let error = runtime
            .control_input("project".into(), request, Some(capture(b"original")))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "native_control_outcome_unknown");
        assert_eq!(snapshot(&runtime).await, before);
        close(root, runtime).await;
    }
}

/// D7: a native start failure asks Continue/Finish. Continue must relaunch the
/// coordinator (Python dispatch_answer), never leave RECOVERING with no driver.
#[cfg(test)]
mod stop_continuation_tests {
    use super::*;
    use crate::{native_runtime::NativeRoutes, native_sessions::SessionRoute};
    use cortex_engine::environment::{EngineKind, LaunchSpec};
    use std::{collections::BTreeMap, path::Path, time::Duration};

    /// A real process that exits before initialize: a no-paid start failure.
    struct FailingStart;
    impl NativeRoutes for FailingStart {
        fn route(&self, root: &Path, _: &str) -> CoreResult<SessionRoute> {
            Ok(SessionRoute {
                launch: LaunchSpec {
                    engine: EngineKind::Codex,
                    executable: "/bin/false".into(),
                    args: vec![],
                    cwd: root.into(),
                    native_home: root.into(),
                    credential_directory: root.into(),
                    base_environment: BTreeMap::new(),
                    codex_launcher: None,
                },
                relay: "/bin/false".into(),
                broker_parent: root.join(if workspace_protocol::channel::current().is_dev() {
                    ".cortex-dev"
                } else {
                    ".cortex"
                }),
                connection: "fixture".into(),
                connection_limit: 4,
                model: "fixture-model".into(),
                parameters: BTreeMap::new(),
                instructions: "No-paid fixture.".into(),
                deadline: Duration::from_secs(2),
            })
        }
    }
    async fn facts(runtime: &Runtime) -> (String, i64, i64, bool) {
        runtime
            .host
            .executor
            .write(|db| {
                let state: String =
                    db.query_row("SELECT state FROM runtime_tasks", [], |r| r.get(0))?;
                let failures: i64 = db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='NATIVE_SESSION_START_FAILED'",
                    [],
                    |r| r.get(0),
                )?;
                let open: i64 = db.query_row(
                    "SELECT count(*) FROM pending_questions WHERE status='open'",
                    [],
                    |r| r.get(0),
                )?;
                let task: String =
                    db.query_row("SELECT task_id FROM runtime_tasks", [], |r| r.get(0))?;
                Ok((
                    state,
                    failures,
                    open,
                    domain::stop_continuation_owed_tx(db, &task)?,
                ))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap()
    }
    async fn until(runtime: &Runtime, done: impl Fn(&(String, i64, i64, bool)) -> bool) {
        let mut last = facts(runtime).await;
        for _ in 0..300 {
            if done(&last) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            last = facts(runtime).await;
        }
        panic!("native task did not reach the expected state: {last:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn continue_after_a_start_failure_relaunches_instead_of_stranding_recovery() {
        let root =
            std::env::temp_dir().join(format!("stop-continue-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(FailingStart))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let draft = runtime
            .command(
                "project".into(),
                json!({"protocol":{"major":1,"minor":8},"requestId":"draft","projectId":"project",
                    "actorId":actor(),"operation":"tasks.create","target":"project","expectedVersion":"0",
                    "ownerEpoch":null,"payload":{"description":"Start failure fixture","checkout":{"kind":"main"}}}),
            )
            .await
            .unwrap();
        runtime
            .command(
                "project".into(),
                json!({"protocol":{"major":1,"minor":8},"requestId":"start","projectId":"project",
                    "actorId":actor(),"operation":"tasks.start","target":draft["target"],
                    "expectedVersion":draft["entityVersion"],"ownerEpoch":null,"payload":{}}),
            )
            .await
            .unwrap();
        until(&runtime, |(state, failures, open, _)| {
            state == "WAITING_USER" && *failures == 1 && *open == 1
        })
        .await;
        let execution = runtime
            .host
            .executor
            .read(|db| {
                Ok(
                    db.query_row("SELECT execution_id FROM workspace_tasks", [], |r| {
                        r.get::<_, String>(0)
                    })?,
                )
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let page = runtime
            .decisions(
                "project".into(),
                json!({"operation":"decisions.list","target":execution,"limit":100}),
            )
            .await
            .unwrap();
        let d = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["status"] == "pending")
            .unwrap()
            .clone();
        // The owner reads the reason in plain words, never the raw blocker code.
        let prompt = d["prompt"].as_str().unwrap();
        assert!(
            prompt.contains("the coordinator session could not start"),
            "{prompt}"
        );
        assert!(!prompt.contains("_session_start_failed"), "{prompt}");
        let option = d["options"][0].clone();
        assert!(option["label"].as_str().unwrap().starts_with("Continue"));
        runtime
            .answer_command(
                "project".into(),
                json!({"protocol":{"major":1,"minor":8},"requestId":"answer","projectId":"project",
                    "actorId":actor(),"operation":"decisions.answer","target":d["decisionId"],
                    "expectedVersion":d["version"],"ownerEpoch":d["ownerEpoch"],
                    "payload":{"executionId":execution,"acpSessionId":d["acpSessionId"],"batchId":null,
                        "answers":[{"decisionId":d["decisionId"],"questionId":d["questionId"],
                            "optionId":option["optionId"],"expectedVersion":d["version"]}]}}),
            )
            .await
            .unwrap();
        // The coordinator really relaunched (a second launch attempt), and its
        // repeated failure is a new explicit owner question, not RECOVERING.
        until(&runtime, |(state, failures, open, owed)| {
            state == "WAITING_USER" && *failures == 2 && *open == 1 && !owed
        })
        .await;
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Pause/Resume, bounded unknown outcomes and route ownership, without a paid engine.
#[cfg(test)]
mod resume_tests {
    use super::*;
    use crate::{native_runtime::NativeRoutes, native_sessions::SessionRoute};
    use cortex_core::workspace;
    use cortex_engine::environment::{EngineKind, LaunchSpec};
    use std::{
        collections::BTreeMap,
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    /// A real process that exits before initialize, so a started coordinator fails
    /// without a model. `generation` models a later change of the owner default.
    struct Routes {
        generation: AtomicUsize,
        default_available: bool,
    }
    impl Routes {
        fn session(root: &Path, model: &str) -> SessionRoute {
            SessionRoute {
                launch: LaunchSpec {
                    engine: EngineKind::Codex,
                    executable: "/bin/false".into(),
                    args: vec![],
                    cwd: root.into(),
                    native_home: root.into(),
                    credential_directory: root.into(),
                    base_environment: BTreeMap::new(),
                    codex_launcher: None,
                },
                relay: "/bin/false".into(),
                broker_parent: root.join(if workspace_protocol::channel::current().is_dev() {
                    ".cortex-dev"
                } else {
                    ".cortex"
                }),
                connection: "fixture".into(),
                connection_limit: 4,
                model: model.into(),
                parameters: BTreeMap::new(),
                instructions: "No-paid fixture.".into(),
                deadline: Duration::from_secs(2),
            }
        }
    }
    impl NativeRoutes for Routes {
        fn route(&self, root: &Path, _: &str) -> CoreResult<SessionRoute> {
            if !self.default_available {
                return Err(CoreError::Refused("native_route_authorization_unavailable"));
            }
            Ok(Self::session(
                root,
                &format!("default-{}", self.generation.load(Ordering::SeqCst)),
            ))
        }
        fn task_route(&self, root: &Path, settings: &Value) -> CoreResult<SessionRoute> {
            match settings["model"].as_str().filter(|m| !m.is_empty()) {
                Some(model) => Ok(Self::session(root, model)),
                None => self.route(root, "orchestrator"),
            }
        }
        fn coordinator_pin(&self, root: &Path, settings: &Value) -> CoreResult<Option<Value>> {
            Ok(Some(
                json!({"route_id":self.task_route(root, settings)?.model}),
            ))
        }
        fn pinned_route(
            &self,
            root: &Path,
            pin: &Value,
            _: Option<&str>,
        ) -> CoreResult<SessionRoute> {
            Ok(Self::session(root, pin["route_id"].as_str().unwrap()))
        }
    }

    async fn seeded(routes: Routes) -> (std::path::PathBuf, Arc<Runtime>, String, String) {
        let root = std::env::temp_dir().join(format!("resume-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || Runtime::open(&path, "project", Arc::new(routes)))
                .await
                .unwrap()
                .unwrap(),
        );
        let execution = runtime.host.executor.publication(move |store| {
            let command = json!({"protocol":{"major":1,"minor":8},"requestId":"draft",
                "projectId":"project","actorId":actor(),"operation":"tasks.create","target":"project",
                "expectedVersion":"0","ownerEpoch":null,
                "payload":{"description":"Resume fixture","checkout":{"kind":"main"}}});
            let draft = workspace::create_draft(store, "project", &actor(), &command)?.receipt;
            let start = json!({"protocol":{"major":1,"minor":8},"requestId":"start",
                "projectId":"project","actorId":actor(),"operation":"tasks.start",
                "target":draft["target"],"expectedVersion":draft["entityVersion"],
                "ownerEpoch":null,"payload":{}});
            let started = workspace::start_native(store, "project", &actor(), &start)?;
            Ok(started["receipt"]["target"].as_str().unwrap().to_owned())
        }).unwrap().await.unwrap().unwrap();
        let task = sql(&runtime, "SELECT task_id FROM runtime_tasks").await;
        (root, runtime, execution, task)
    }
    async fn sql(runtime: &Runtime, query: &'static str) -> String {
        runtime
            .host
            .executor
            .read(move |db| {
                Ok(db
                    .query_row(query, [], |r| r.get::<_, rusqlite::types::Value>(0))
                    .map(|v| match v {
                        rusqlite::types::Value::Text(s) => s,
                        rusqlite::types::Value::Integer(i) => i.to_string(),
                        rusqlite::types::Value::Null => String::new(),
                        other => format!("{other:?}"),
                    })?)
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap()
    }
    async fn exec(runtime: &Runtime, query: &'static str) {
        runtime
            .host
            .executor
            .write(move |db| {
                db.execute_batch(query)?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }
    async fn close(root: std::path::PathBuf, runtime: Arc<Runtime>) {
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    fn control(execution: &str, key: &str, action: &str) -> Value {
        json!({"projectId":"project","executionId":execution,"requestId":key,"action":action})
    }
    async fn pause(runtime: &Arc<Runtime>, execution: &str) -> Value {
        runtime
            .control("project".into(), control(execution, "pause", "pause"))
            .await
            .unwrap()
    }
    async fn resume(runtime: &Arc<Runtime>, execution: &str, key: &str) -> io::Result<Value> {
        let version = sql(
            runtime,
            "SELECT t.version FROM workspace_tasks t JOIN workspace_executions e USING(task_id)",
        )
        .await;
        let mut request = control(execution, key, "resume");
        request["expectedVersion"] = json!(version);
        runtime.control("project".into(), request).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refused_resume_commits_nothing_and_stays_retryable() {
        let (root, runtime, execution, _) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        pause(&runtime, &execution).await;
        assert_eq!(
            sql(&runtime, "SELECT state FROM runtime_tasks").await,
            "PAUSED"
        );
        // An owned process that has not been confirmed gone: Resume refuses whole.
        exec(
            &runtime,
            "INSERT INTO capacity_reservations VALUES ('r1','fixture',(SELECT task_id FROM runtime_tasks),NULL,'a1','worker','ambiguous','0','0',NULL)",
        )
        .await;
        let error = resume(&runtime, &execution, "resume-1").await.unwrap_err();
        assert_eq!(error.to_string(), "native_shutdown_unconfirmed");
        assert_eq!(
            sql(&runtime, "SELECT state FROM runtime_tasks").await,
            "PAUSED"
        );
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_requests WHERE request_key='resume-1'"
            )
            .await,
            "0",
            "a refused Resume must not leave an admission behind"
        );
        // Once the process is confirmed gone, the same task resumes (retryable).
        exec(
            &runtime,
            "UPDATE capacity_reservations SET state='released'",
        )
        .await;
        resume(&runtime, &execution, "resume-2").await.unwrap();
        assert_ne!(
            sql(&runtime, "SELECT state FROM runtime_tasks").await,
            "PAUSED"
        );
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='TURN_INTERRUPTED'"
            )
            .await,
            "0",
            "Resume continues; it never interrupts"
        );
        close(root, runtime).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refused_incoming_prompt_on_a_paused_task_commits_nothing() {
        let (root, runtime, execution, _) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        pause(&runtime, &execution).await;
        exec(
            &runtime,
            "INSERT INTO capacity_reservations VALUES ('r1','fixture',(SELECT task_id FROM runtime_tasks),NULL,'a1','worker','ambiguous','0','0',NULL)",
        )
        .await;
        let request = json!({"projectId":"project","executionId":execution,"requestId":"ip-1","action":"incoming_prompt","text":"More"});
        let error = runtime
            .control("project".into(), request.clone())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "native_shutdown_unconfirmed");
        assert_eq!(
            sql(&runtime, "SELECT state FROM runtime_tasks").await,
            "PAUSED"
        );
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'"
            )
            .await,
            "0"
        );
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_requests WHERE request_key='ip-1'"
            )
            .await,
            "0",
            "a refused prompt must not leave an admission behind"
        );
        // Once the process is confirmed gone, the same prompt is accepted and resumes.
        exec(
            &runtime,
            "UPDATE capacity_reservations SET state='released'",
        )
        .await;
        runtime.control("project".into(), request).await.unwrap();
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'"
            )
            .await,
            "1"
        );
        assert_ne!(
            sql(&runtime, "SELECT state FROM runtime_tasks").await,
            "PAUSED"
        );
        close(root, runtime).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resume_never_erases_an_outcome_unknown_barrier() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        pause(&runtime, &execution).await;
        let t = task.clone();
        runtime
            .host
            .executor
            .write(move |db| {
                domain::native_recovery_required_tx(db, &t, "native_session_outcome_unknown")?;
                // A later, different refusal must not overwrite the evidence.
                domain::native_recovery_required_tx(db, &t, "native_shutdown_unconfirmed")
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        resume(&runtime, &execution, "resume-barrier")
            .await
            .unwrap();
        // Resume itself keeps the barrier as evidence (it returns as RECOVERING with it)...
        assert_eq!(
            sql(&runtime, "SELECT json_extract(payload_json,'$.barrier')||':'||json_extract(payload_json,'$.state') FROM runtime_events WHERE type='OWNER_TASK_RESUMED'").await,
            "native_session_outcome_unknown:RECOVERING"
        );
        // ...and, with every owned process confirmed gone, Continue then drives the task
        // like an owner message instead of silently starting nothing.
        coordinator_relaunched(&runtime).await;
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='TURN_INTERRUPTED'"
            )
            .await,
            "1"
        );
        close(root, runtime).await;
    }

    /// Paused while a restart recorded an unknown session outcome (no owned process left):
    /// a desktop message is refused as paused, so Continue is the owner's only way on.
    #[tokio::test(flavor = "multi_thread")]
    async fn continue_on_a_paused_task_held_by_a_lost_session_relaunches_it() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        pause(&runtime, &execution).await;
        let t = task.clone();
        runtime.host.executor.write(move |db| {
            db.execute("INSERT INTO delegation_provisioning VALUES ('lost',?,NULL,'lost-attempt','orchestrator','ready','ambiguous',NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,'0','0')", [&t])?;
            db.execute("INSERT INTO capacity_reservations VALUES ('lost-r','fixture',?,NULL,'lost-attempt','coordinator','ambiguous','0','0','termination_unconfirmed')", [&t])?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        runtime.recover().await.unwrap();
        assert_eq!(
            sql(&runtime, "SELECT state||':'||blocker FROM runtime_tasks").await,
            "PAUSED:native_session_outcome_unknown"
        );
        let error = runtime
            .control("project".into(), message(&execution, "paused-msg"))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "task_paused");
        // The attempt was settled by that recovery (its host is gone and no engine works
        // in the project), while the task stays held; a later recovery keeps it settled.
        runtime.recover().await.unwrap();
        assert_eq!(
            sql(&runtime, "SELECT state FROM capacity_reservations").await,
            "released"
        );
        resume(&runtime, &execution, "continue-held").await.unwrap();
        coordinator_relaunched(&runtime).await;
        assert_ne!(
            sql(&runtime, "SELECT coalesce(blocker,'') FROM runtime_tasks").await,
            "native_session_outcome_unknown"
        );
        close(root, runtime).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unfinished_admission_is_bounded_and_not_reraised_by_restarts() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        // Admitted, but its finish receipt was never written (crash mid-control).
        let e = execution.clone();
        runtime.host.executor.publication(move |store| {
            let request = json!({"projectId":"project","executionId":e,"requestId":"lost","action":"pause"});
            cortex_core::workspace::control::admit_input(store, "project", &actor(), &request, None)?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        runtime.recover().await.unwrap();
        let reasons = "SELECT count(*) FROM runtime_events WHERE type='NATIVE_RECOVERY_REQUIRED' AND json_extract(payload_json,'$.reason')='native_control_outcome_unknown'";
        assert_eq!(sql(&runtime, reasons).await, "1");
        assert_eq!(
            sql(&runtime, "SELECT json_extract(result_json,'$.outcome') FROM runtime_requests WHERE actor='native-desktop-finish' AND request_key='lost'").await,
            "unknown"
        );
        // The same request never claims it happened.
        let error = runtime
            .control("project".into(), control(&execution, "lost", "pause"))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "native_control_outcome_unknown");
        // Later restarts do not force another barrier for it.
        runtime.recover().await.unwrap();
        runtime.recover().await.unwrap();
        assert_eq!(sql(&runtime, reasons).await, "1");
        let _ = task;
        close(root, runtime).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_settling_marks_the_input_unknown_instead_of_duplicating_it() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        // A message to a task held in recovery restarts its coordinator, so finish
        // refuses (unconfirmed shutdown) after the follow-up was admitted. (A message
        // to a working task restarts nothing; see the boundary test below.)
        exec(
            &runtime,
            "UPDATE runtime_tasks SET state='RECOVERING'; INSERT INTO capacity_reservations VALUES ('r2','fixture',(SELECT task_id FROM runtime_tasks),NULL,'a2','worker','ambiguous','0','0',NULL)",
        )
        .await;
        let request = json!({"projectId":"project","executionId":execution,"requestId":"follow","action":"incoming_prompt","text":"More"});
        let error = runtime
            .control("project".into(), request.clone())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "native_shutdown_unconfirmed");
        // The same key never records a second copy and never claims success.
        let error = runtime
            .control("project".into(), request)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "native_control_outcome_unknown");
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'"
            )
            .await,
            "1"
        );
        assert_eq!(
            sql(&runtime, "SELECT state FROM runtime_tasks").await,
            "RECOVERING"
        );
        assert_eq!(
            sql(&runtime, "SELECT blocker FROM runtime_tasks").await,
            "native_control_outcome_unknown"
        );
        let _ = task;
        close(root, runtime).await;
    }

    /// An owner message to a working task with a live worker process is recorded at once:
    /// nothing is settled, cancelled or interrupted, and the task keeps its state.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_message_to_a_working_task_settles_and_interrupts_nothing() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        exec(
            &runtime,
            "UPDATE runtime_tasks SET state='EXECUTING',revision=1;
             INSERT INTO runtime_nodes VALUES ((SELECT task_id FROM runtime_tasks),1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a3',NULL);
             INSERT INTO capacity_reservations VALUES ('r3','fixture',(SELECT task_id FROM runtime_tasks),'a3','a3','worker','reserved','0','0',NULL)",
        )
        .await;
        // The task's live driver (it runs the worker): it must not be cancelled.
        let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let (done, finished) = tokio::sync::watch::channel(false);
        runtime.install_pump(task.clone(), cancel, finished).await;
        let request = json!({"projectId":"project","executionId":execution,"requestId":"steer","action":"incoming_prompt","text":"Use the other output"});
        let receipt = runtime
            .control("project".into(), request.clone())
            .await
            .unwrap();
        assert!(
            !cancelled.has_changed().unwrap() && !*cancelled.borrow_and_update(),
            "the running driver was cancelled"
        );
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_TASK_CONTINUED'"
            )
            .await,
            "0",
            "a second driver was started"
        );
        assert_eq!(receipt["state"], "EXECUTING");
        assert_eq!(
            runtime.control("project".into(), request).await.unwrap(),
            receipt
        );
        assert_eq!(
            sql(
                &runtime,
                "SELECT state||':'||coalesce(blocker,'') FROM runtime_tasks"
            )
            .await,
            "EXECUTING:"
        );
        assert_eq!(
            sql(&runtime, "SELECT status FROM runtime_nodes").await,
            "running"
        );
        assert_eq!(
            sql(&runtime, "SELECT state FROM capacity_reservations").await,
            "reserved"
        );
        assert_eq!(sql(&runtime, "SELECT count(*) FROM runtime_events WHERE type IN ('TURN_INTERRUPTED','LIVE_ASSIGNMENTS_CANCELLED','NATIVE_RECOVERY_REQUIRED')").await, "0");
        assert_eq!(sql(&runtime, "SELECT json_extract(payload_json,'$.delivery') FROM runtime_events WHERE type='USER_STEERING'").await, "boundary");
        done.send_replace(true);
        close(root, runtime).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_follow_up_uses_the_task_route_without_a_resolvable_owner_default() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: false,
        })
        .await;
        let request = json!({"projectId":"project","executionId":execution,"requestId":"follow-1","action":"incoming_prompt","text":"More"});
        // No task route and no default: refused before any admission.
        let error = runtime
            .control("project".into(), request.clone())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "native_route_authorization_unavailable");
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'"
            )
            .await,
            "0"
        );
        // The task's own route accepts it.
        let t = task.clone();
        runtime.host.executor.write(move |db| {
            lifecycle::event_tx(db, &t, "NATIVE_TASK_SETTINGS",
                json!({"settings":{"model":"task-model","effort":"","mode":"read_only","language":"auto"},"activation":"next_use"}))?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        let mut request = request;
        request["requestId"] = json!("follow-2");
        runtime.control("project".into(), request).await.unwrap();
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='USER_STEERING'"
            )
            .await,
            "1"
        );
        close(root, runtime).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_coordinator_route_is_pinned_when_the_task_starts() {
        let routes = Arc::new(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        });
        let root = std::env::temp_dir().join(format!("pin-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let shared = routes.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || Runtime::open(&path, "project", shared))
                .await
                .unwrap()
                .unwrap(),
        );
        let draft = runtime
            .command(
                "project".into(),
                json!({"protocol":{"major":1,"minor":8},"requestId":"draft","projectId":"project",
            "actorId":actor(),"operation":"tasks.create","target":"project","expectedVersion":"0",
            "ownerEpoch":null,"payload":{"description":"Pin fixture","checkout":{"kind":"main"}}}),
            )
            .await
            .unwrap();
        runtime.command("project".into(), json!({"protocol":{"major":1,"minor":8},"requestId":"start","projectId":"project",
            "actorId":actor(),"operation":"tasks.start","target":draft["target"],"expectedVersion":draft["entityVersion"],
            "ownerEpoch":null,"payload":{}})).await.unwrap();
        let mut pinned = String::new();
        for _ in 0..200 {
            pinned = sql(&runtime, "SELECT coalesce((SELECT json_extract(payload_json,'$.route_id') FROM runtime_events WHERE type='NATIVE_COORDINATOR_ROUTE'),'')").await;
            if !pinned.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(pinned, "default-0");
        // The owner changes the default; the running task keeps its route.
        routes.generation.store(1, Ordering::SeqCst);
        let task = sql(&runtime, "SELECT task_id FROM runtime_tasks").await;
        let route = runtime.coordinator_route(&task, None).await.unwrap();
        assert_eq!(route.model, "default-0");
        assert_eq!(
            runtime
                .routes
                .route(&runtime.host.root, "orchestrator")
                .unwrap()
                .model,
            "default-1"
        );
        close(root, runtime).await;
    }

    /// The rows an earlier host left after a worker session start that it refused
    /// before launching the engine (`phase`/`error` as given), followed by the owner's
    /// free-text answer to the runtime stop question and an unconfirmed follow-up:
    /// RECOVERING, blocker `native_control_outcome_unknown`, the worker's delegation
    /// and reservation `ambiguous`, its channel revoked, its node cancelled.
    async fn legacy_unlaunched_worker(runtime: &Runtime, task: &str, phase: &str, error: &str) {
        let (t, phase, error) = (task.to_owned(), phase.to_owned(), error.to_owned());
        runtime.host.executor.write(move |db| {
            let revision: i64 = db.query_row("SELECT revision FROM runtime_tasks WHERE task_id=?", [&t], |r| r.get(0))?;
            db.execute("INSERT INTO runtime_nodes VALUES (?,?,'styleguide','worker-technical-writer','Write the guide','[]','[]','execution','cancelled','legacy-assignment',NULL)",
                (&t, revision))?;
            db.execute("INSERT INTO delegation_provisioning VALUES ('legacy-assignment',?,NULL,'legacy-attempt','worker-technical-writer','starting','ambiguous',NULL,'ch_legacy',NULL,NULL,NULL,NULL,NULL,1,'0','0')",
                [&t])?;
            db.execute("INSERT INTO capacity_reservations VALUES ('legacy-reservation','fixture',?,'legacy-assignment','legacy-attempt','worker','ambiguous','0','0','termination_unconfirmed')",
                [&t])?;
            lifecycle::event_tx(db, &t, "NATIVE_SESSION_START_FAILED", json!({"channel_id":"ch_legacy","error":error,"phase":phase}))?;
            lifecycle::event_tx(db, &t, "NATIVE_DRIVER_STOPPED", json!({"reason":"native_session_start_failed"}))?;
            lifecycle::event_tx(db, &t, "LIVE_ASSIGNMENTS_CANCELLED", json!({"assignment_refs":["legacy-assignment"],"reason":"task_stopped_for_question"}))?;
            lifecycle::event_tx(db, &t, "USER_QUESTION_REQUESTED", json!({"blocker":"native_session_start_failed",
                "options":domain::STOP_OPTIONS_EN,"question":"The task stopped: native_session_start_failed. What should happen next?",
                "question_id":"pq_legacy","resume_state":"RECOVERING","revision":revision,"runtime_stop":true}))?;
            db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,answer_text,answer_source,created_at,answered_at)
                VALUES ('pq_legacy',?,'runtime:stop:pq_legacy','The task stopped: native_session_start_failed. What should happen next?',?,'answered','Try again.','text','0','0')",
                (&t, json!(domain::STOP_OPTIONS_EN).to_string()))?;
            lifecycle::event_tx(db, &t, "USER_ANSWER_RECEIVED", json!({"answer_source":"text","question_id":"pq_legacy"}))?;
            db.execute("UPDATE runtime_tasks SET state='RECOVERING',blocker='native_control_outcome_unknown' WHERE task_id=?", [&t])?;
            lifecycle::event_tx(db, &t, "TASK_STATE_CHANGED", json!({"blocker":"native_control_outcome_unknown","previous":"RECOVERING","state":"RECOVERING"}))?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
    }
    fn message(execution: &str, key: &str) -> Value {
        json!({"projectId":"project","executionId":execution,"requestId":key,"action":"prompt","text":"Try again."})
    }
    async fn coordinator_relaunched(runtime: &Runtime) {
        for _ in 0..200 {
            if sql(runtime, "SELECT count(*) FROM runtime_events WHERE type='NATIVE_SESSION_START_FAILED' AND json_extract(payload_json,'$.channel_id')!='ch_legacy'").await != "0" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the coordinator was not relaunched");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_legacy_worker_refused_before_launch_is_settled_and_the_next_message_relaunches() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        legacy_unlaunched_worker(&runtime, &task, "external_mcp", "authority_refused").await;
        // Service start: the durable start failure proves no engine was ever launched.
        runtime.recover().await.unwrap();
        assert_eq!(sql(&runtime, "SELECT status FROM delegation_provisioning WHERE idempotency_key='legacy-assignment'").await, "terminated");
        assert_eq!(sql(&runtime, "SELECT state||':'||release_observation FROM capacity_reservations WHERE reservation_id='legacy-reservation'").await, "released:never_launched");
        assert_eq!(sql(&runtime, "SELECT json_extract(payload_json,'$.evidence') FROM runtime_events WHERE type='NATIVE_PROCESS_SETTLED'").await,
            "session_start_failed_before_engine_launch");
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_PROCESS_UNCONFIRMED'"
            )
            .await,
            "0"
        );
        // The owner's next message is admitted, settles and relaunches the coordinator
        // (the fixture engine exits before initialize: no model is called).
        let receipt = runtime
            .control("project".into(), message(&execution, "after-fix"))
            .await
            .unwrap();
        assert_eq!(receipt["outcome"], "recorded");
        coordinator_relaunched(&runtime).await;
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_CONTROL_OUTCOME_UNKNOWN'"
            )
            .await,
            "0"
        );
        // Recovery is idempotent: a later restart records nothing new for the attempt.
        runtime.recover().await.unwrap();
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_PROCESS_SETTLED'"
            )
            .await,
            "1"
        );
        close(root, runtime).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_possibly_spawned_worker_stays_fail_closed_until_a_later_boot() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        // `session_new` is after the engine launch: its process may still run, and an
        // engine process does work in the project.
        legacy_unlaunched_worker(&runtime, &task, "session_new", "io_error").await;
        let mut engine = fake_engine(&root);
        runtime.recover().await.unwrap();
        assert_eq!(sql(&runtime, "SELECT status FROM delegation_provisioning WHERE idempotency_key='legacy-assignment'").await, "ambiguous");
        assert_eq!(
            sql(
                &runtime,
                "SELECT state FROM capacity_reservations WHERE reservation_id='legacy-reservation'"
            )
            .await,
            "ambiguous"
        );
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_PROCESS_SETTLED'"
            )
            .await,
            "0"
        );
        // The attempt is stamped once with this boot; restarts in the same boot do not settle it.
        runtime.recover().await.unwrap();
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_PROCESS_UNCONFIRMED'"
            )
            .await,
            "1"
        );
        let error = runtime
            .control("project".into(), message(&execution, "same-boot"))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "native_shutdown_unconfirmed");
        assert_eq!(sql(&runtime, "SELECT status FROM delegation_provisioning WHERE idempotency_key='legacy-assignment'").await, "ambiguous");
        // A recovery in a later boot: no process of that boot can still run.
        exec(&runtime, "UPDATE runtime_events SET payload_json=json_set(payload_json,'$.boot_id','an-earlier-boot') WHERE type='NATIVE_PROCESS_UNCONFIRMED'").await;
        runtime.recover().await.unwrap();
        assert_eq!(sql(&runtime, "SELECT state||':'||release_observation FROM capacity_reservations WHERE reservation_id='legacy-reservation'").await, "released:previous_boot");
        assert_eq!(sql(&runtime, "SELECT status FROM delegation_provisioning WHERE idempotency_key='legacy-assignment'").await, "terminated");
        assert_eq!(sql(&runtime, "SELECT json_extract(payload_json,'$.evidence') FROM runtime_events WHERE type='NATIVE_PROCESS_SETTLED'").await,
            "unconfirmed_attempt_from_previous_boot");
        stop_fake_engine(&mut engine);
        runtime
            .control("project".into(), message(&execution, "next-boot"))
            .await
            .unwrap();
        coordinator_relaunched(&runtime).await;
        close(root, runtime).await;
    }

    /// A process of this user whose command line names an engine adapter and whose
    /// working directory is `root` (`sh -c` keeps the marker as its own argument).
    fn fake_engine(root: &std::path::Path) -> std::process::Child {
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 60; :", "codex-acp"])
            .current_dir(root)
            .process_group(0)
            .spawn()
            .unwrap();
        // The scan reads it from the process table: wait until it shows its marker.
        for _ in 0..200 {
            if std::fs::read(format!("/proc/{}/cmdline", child.id()))
                .is_ok_and(|c| c.windows(9).any(|w| w == b"codex-acp"))
            {
                return child;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the fake engine did not start");
    }
    fn stop_fake_engine(child: &mut std::process::Child) {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        child.wait().unwrap();
    }

    /// The service was killed (SIGKILL) while a worker ran: its delegation and
    /// reservation stay open with no process record. On restart nothing of that engine
    /// works in the project, so the attempt is settled and the task held as
    /// interrupted; the owner's message then continues it, and Stop, Resume, Close and
    /// Delete all work. Earlier builds refused all of them as
    /// `native_shutdown_unconfirmed`, on every restart, until a reboot.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_worker_of_a_killed_service_is_settled_on_restart_and_the_task_continues() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        killed_worker(&runtime, &task).await;
        // Another task's writer of the same file is queued behind the worker's holding.
        let r = root.to_string_lossy().into_owned();
        let conflict = || {
            runtime
                .host
                .executor
                .write(move |db| {
                    cortex_core::scheduler::policy::write_conflict_tx(db, "other-task")
                })
                .unwrap()
        };
        runtime.host.executor.write(move |db| {
            db.execute("INSERT INTO tasks VALUES ('other-task',?,'0')", [&r])?;
            db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('other-task','EXECUTING','g','0','0')", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('other-task',(SELECT revision FROM runtime_tasks WHERE task_id='other-task'),'reset','worker-backend-dev','Reset test','[]','[\"test/counter.test.js\"]','execution','pending',NULL,NULL)", [])?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        assert_eq!(
            conflict().await.unwrap().unwrap().unwrap()["holder_task"],
            json!(task)
        );
        runtime.recover().await.unwrap();
        assert_eq!(sql(&runtime, "SELECT status FROM delegation_provisioning WHERE idempotency_key='killed-assignment'").await, "terminated");
        assert_eq!(sql(&runtime, "SELECT state||':'||release_observation FROM capacity_reservations WHERE reservation_id='killed-reservation'").await,
            "released:no_engine_process");
        assert_eq!(sql(&runtime, "SELECT json_extract(payload_json,'$.evidence') FROM runtime_events WHERE type='NATIVE_PROCESS_SETTLED'").await,
            "service_gone_and_no_engine_process_in_project");
        // Held for the owner, not driven on by itself.
        assert_eq!(
            sql(
                &runtime,
                "SELECT state||':'||blocker FROM runtime_tasks WHERE task_id!='other-task'"
            )
            .await,
            "RECOVERING:native_session_outcome_unknown"
        );
        assert_eq!(sql(&runtime, "SELECT count(*) FROM runtime_events WHERE type IN ('NATIVE_RECOVERY_RESUMED','NATIVE_PROCESS_UNCONFIRMED')").await, "0");
        // The settled worker no longer holds its files for the other task.
        assert!(conflict().await.unwrap().unwrap().is_none());
        // A later restart records nothing new for it.
        let counted = sql(&runtime, "SELECT count(*) FROM runtime_events").await;
        runtime.recover().await.unwrap();
        assert_eq!(
            sql(&runtime, "SELECT count(*) FROM runtime_events").await,
            counted
        );
        // The owner's message is admitted and relaunches the coordinator.
        let receipt = runtime
            .control("project".into(), message(&execution, "continue-after-kill"))
            .await
            .unwrap();
        assert_eq!(receipt["outcome"], "recorded");
        coordinator_relaunched(&runtime).await;
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_CONTROL_OUTCOME_UNKNOWN'"
            )
            .await,
            "0"
        );
        close(root, runtime).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_task_of_a_killed_service_can_be_stopped_resumed_closed_and_deleted() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        killed_worker(&runtime, &task).await;
        runtime.recover().await.unwrap();
        // Stop is confirmed (no unknown outcome), and so is Resume.
        pause(&runtime, &execution).await;
        assert_eq!(
            sql(&runtime, "SELECT state FROM runtime_tasks").await,
            "PAUSED"
        );
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_CONTROL_OUTCOME_UNKNOWN'"
            )
            .await,
            "0"
        );
        resume(&runtime, &execution, "resume-after-kill")
            .await
            .unwrap();
        coordinator_relaunched(&runtime).await;
        // Close as is reports no unconfirmed process, and Delete removes the task.
        let request = json!({"projectId":"project","executionId":execution,"requestId":"close-after-kill","action":"close_task","confirmed":true});
        let receipt = runtime.control("project".into(), request).await.unwrap();
        assert_eq!(receipt["outcome"], "recorded");
        assert_eq!(receipt["processesUnconfirmed"], Value::Null);
        let draft = sql(&runtime, "SELECT task_id FROM workspace_tasks").await;
        runtime
            .host
            .executor
            .publication(move |store| store.purge_workspace_task("project", &draft, false))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            sql(&runtime, "SELECT count(*) FROM capacity_reservations").await,
            "0"
        );
        close(root, runtime).await;
    }

    /// While an engine process still works in the project, the killed service's
    /// attempt stays fail-closed; once it is gone the next restart settles it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_killed_services_worker_stays_unconfirmed_while_an_engine_works_in_the_project() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        killed_worker(&runtime, &task).await;
        let mut engine = fake_engine(&root);
        runtime.recover().await.unwrap();
        assert_eq!(
            sql(
                &runtime,
                "SELECT state FROM capacity_reservations WHERE reservation_id='killed-reservation'"
            )
            .await,
            "ambiguous"
        );
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_PROCESS_UNCONFIRMED'"
            )
            .await,
            "1"
        );
        let error = runtime
            .control("project".into(), message(&execution, "while-alive"))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "native_shutdown_unconfirmed");
        stop_fake_engine(&mut engine);
        runtime.recover().await.unwrap();
        assert_eq!(sql(&runtime, "SELECT state||':'||release_observation FROM capacity_reservations WHERE reservation_id='killed-reservation'").await,
            "released:no_engine_process");
        close(root, runtime).await;
    }

    /// The rows a service killed while its worker ran leaves, as the live store had
    /// them: the task executing, the worker's node running, its delegation `ready` with
    /// an engine session, its reservation `reserved`, no process record.
    async fn killed_worker(runtime: &Runtime, task: &str) {
        let t = task.to_owned();
        runtime.host.executor.write(move |db| {
            let revision: i64 = db.query_row("SELECT revision FROM runtime_tasks WHERE task_id=?", [&t], |r| r.get(0))?;
            db.execute("INSERT INTO runtime_nodes VALUES (?,?,'formatAll','worker-backend-dev','Add formatAll','[]','[\"src/format.js\",\"test/counter.test.js\"]','execution','running','killed-assignment',NULL)",
                (&t, revision))?;
            db.execute("INSERT INTO delegation_provisioning VALUES ('killed-assignment',?,NULL,'killed-attempt','worker-backend-dev','ready','ready',NULL,'ch_killed',NULL,NULL,'es-killed',NULL,NULL,1,'0','0')",
                [&t])?;
            db.execute("INSERT INTO capacity_reservations VALUES ('killed-reservation','fixture',?,'killed-assignment','killed-attempt','worker','reserved','0','0',NULL)",
                [&t])?;
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',blocker=NULL WHERE task_id=?", [&t])?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_closed_legacy_task_becomes_deletable_after_recovery() {
        let (root, runtime, execution, task) = seeded(Routes {
            generation: AtomicUsize::new(0),
            default_available: true,
        })
        .await;
        legacy_unlaunched_worker(&runtime, &task, "external_mcp", "authority_refused").await;
        // Close commits its cancellation and is reported done, so the window closes the
        // card; the unconfirmed process stays recorded and keeps deletion refused.
        let request = json!({"projectId":"project","executionId":execution,"requestId":"close-1","action":"close_task","confirmed":true});
        let receipt = runtime
            .control("project".into(), request.clone())
            .await
            .unwrap();
        assert_eq!(receipt["outcome"], "recorded");
        assert_eq!(receipt["processesUnconfirmed"], true);
        assert_eq!(
            sql(&runtime, "SELECT state||':'||blocker FROM runtime_tasks").await,
            "CANCELLED:owner_resolved_without_certification"
        );
        assert_eq!(sql(&runtime, "SELECT json_extract(payload_json,'$.processes_unconfirmed') FROM runtime_events WHERE type='OWNER_TASK_RESOLVED'").await, "1");
        assert_eq!(
            sql(
                &runtime,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_CONTROL_OUTCOME_UNKNOWN'"
            )
            .await,
            "0"
        );
        // The same request replays its receipt.
        assert_eq!(
            runtime.control("project".into(), request).await.unwrap()["outcome"],
            "recorded"
        );
        let draft = sql(&runtime, "SELECT task_id FROM workspace_tasks").await;
        let purge = |draft: String| {
            runtime
                .host
                .executor
                .publication(move |store| store.purge_workspace_task("project", &draft, false))
                .unwrap()
        };
        assert_eq!(
            purge(draft.clone()).await.unwrap().unwrap_err().code(),
            "active_execution"
        );
        runtime.recover().await.unwrap();
        purge(draft).await.unwrap().unwrap();
        close(root, runtime).await;
    }
}

use crate::{actor, failure, native_runtime::Runtime};
use cortex_core::{
    lifecycle::{self, domain},
    ownership::OwnerEpoch,
    workspace::control,
    CoreError, CoreResult,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{io, sync::Arc};
fn err(e: CoreError) -> io::Error {
    failure(e.code())
}
impl Runtime {
    pub(crate) async fn control(
        self: &Arc<Self>,
        project: String,
        request: Value,
    ) -> io::Result<Value> {
        self.control_input(project, request, None).await
    }
    /// An incoming owner prompt whose attachments the transport already
    /// captured; they are recorded in the same admission as its text.
    pub(crate) async fn control_input(
        self: &Arc<Self>,
        project: String,
        request: Value,
        capture: Option<cortex_core::input::Capture>,
    ) -> io::Result<Value> {
        // Retain the operation if its socket/window disappears. Unknown effects are
        // reconciled from the intent, never silently retried.
        let runtime = self.clone();
        let (send, reply) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            // A verifier rerun may be running under the ordinary control lane.
            // Its exact command must remain stoppable while that handler is waiting.
            let lane = if request["action"] == "command_stop" {
                &runtime.command_controls
            } else {
                &runtime.controls
            };
            let _serial = lane.lock().await;
            let result = runtime.apply_control(project, request, capture).await;
            let _ = send.send(result);
        });
        reply
            .await
            .map_err(|_| failure("native_control_outcome_unknown"))?
    }
    async fn apply_control(
        self: &Arc<Self>,
        project: String,
        request: Value,
        capture: Option<cortex_core::input::Capture>,
    ) -> io::Result<Value> {
        if *self.stop.borrow() {
            return Err(failure("service_stopping"));
        }
        for field in ["projectId", "executionId", "requestId"] {
            workspace_protocol::validate("Identity", &request[field])
                .map_err(|_| failure("invalid_request"))?;
        }
        if request["projectId"] != project {
            return Err(failure("unauthorized"));
        }
        // Owned control tasks keep this fence through settlement and fresh admission,
        // even if the caller disappears. Stop and cancellation remain available.
        let _maintenance = if matches!(
            request["action"].as_str(),
            Some("resume" | "retry" | "prompt" | "incoming_prompt" | "rerun_check")
        ) {
            if let Some(state) = self.maintenance_state.clone() {
                Some(
                    tokio::task::spawn_blocking(move || {
                        crate::machine::maintenance::admission(&state)
                    })
                    .await
                    .map_err(|_| failure("maintenance_busy"))?
                    .map_err(|e| failure(e.code))?,
                )
            } else {
                None
            }
        } else {
            None
        };
        if matches!(
            request["action"].as_str(),
            Some(
                "worker_message" | "worker_stop" | "command_stop" | "rerun_check" | "task_settings"
            )
        ) {
            return self.owner_control(project, request).await.map_err(err);
        }
        if request.as_object().is_none_or(|m| {
            m.keys().any(|k| {
                ![
                    "projectId",
                    "executionId",
                    "requestId",
                    "action",
                    "expectedVersion",
                    "text",
                    "attachments",
                    "confirmed",
                    "path",
                ]
                .contains(&k.as_str())
            })
        }) {
            return Err(failure("invalid_request"));
        }
        let request = control::canonical_request(request, capture.as_ref()).map_err(err)?;
        let p = project.clone();
        let r = request.clone();
        if let Some(receipt) = self
            .host
            .executor
            .publication(move |s| control::replay(s, &p, &actor(), &r))
            .map_err(err)?
            .await
            .map_err(|_| failure("native_host_closed"))?
            .map_err(err)?
        {
            return Ok(receipt);
        }
        // Resume continues the retained task where Pause left it. Retry, and an owner
        // message to a task that waits, is held or was reopened, restart the coordinator.
        // A message to a working task interrupts nothing: Core records it for the
        // coordinator's next boundary (`lifecycle::steering`).
        let resume = request["action"] == "resume";
        let message = matches!(
            request["action"].as_str(),
            Some("prompt" | "incoming_prompt")
        );
        let restart = matches!(
            request["action"].as_str(),
            Some("retry" | "prompt" | "incoming_prompt")
        );
        let route = if restart {
            let task = self.control_task(&project, &request).await.map_err(err)?;
            Some(self.coordinator_route(&task, None).await.map_err(err)?)
        } else {
            None
        };
        let p = project.clone();
        let r = request.clone();
        let b = self
            .host
            .executor
            .publication(move |s| control::admit_input(s, &p, &actor(), &r, capture.as_ref()))
            .map_err(err)?
            .await
            .map_err(|_| failure("native_host_closed"))?
            .map_err(err)?;
        // From here the control is admitted. If it cannot reach its finish receipt,
        // its outcome is settled explicitly as unknown (never claimed, never forever).
        let outcome = async {
            // Lifting a write denial, like a boundary message, leaves the running work alone.
            let lift = request["action"] == "allow_write";
            let boundary = lift
                || message && {
                    let (t, key) = (
                        b.task.clone(),
                        request["requestId"].as_str().unwrap_or_default().to_owned(),
                    );
                    self.host
                        .executor
                        .read(move |db| cortex_core::lifecycle::steering::delivery(db, &t, &key))?
                        .await
                        .map_err(|_| CoreError::Refused("native_host_closed"))??
                        .as_deref()
                        == Some(cortex_core::lifecycle::steering::BOUNDARY)
                };
            // The running driver, its workers, review and verification keep going.
            if !boundary {
                self.settle_task(&b.task).await?;
            }
            // Continue on a task held for recovery (a restart left an outcome unknown
            // while it was paused) has nothing to resume: no driver would run it. Every
            // owned process is already confirmed gone (Resume refuses otherwise), so it
            // continues like an owner message, never silently doing nothing.
            let route = match route {
                _ if boundary => None,
                Some(route) => Some(route),
                None if resume && self.held_for_recovery(&b.task).await? => {
                    Some(self.coordinator_route(&b.task, None).await?)
                }
                None => None,
            };
            let restart = !boundary && (restart || route.is_some());
            let p = project.clone();
            let r = request.clone();
            let result = self
                .host
                .executor
                .publication(move |s| control::finish(s, &p, &actor(), &r, restart))?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??;
            Ok::<_, CoreError>((result, route, boundary))
        }
        .await;
        let (result, route, boundary) = match outcome {
            Ok(result) => result,
            Err(error) => {
                let p = project.clone();
                let r = request.clone();
                let reason = error.code().to_owned();
                if let Ok(settled) = self
                    .host
                    .executor
                    .publication(move |s| control::settle_unknown(s, &p, &actor(), &r, &reason))
                {
                    let _ = settled.await;
                }
                return Err(err(error));
            }
        };
        if boundary {
            // A working task has a driver that delivers the message at its next
            // boundary. Only when none runs (never replacing a live one) is one started.
            if request["action"] != "allow_write" && !self.driven(&b.task).await {
                self.resume_task(b.task, b.owner).await.map_err(err)?;
            }
        } else if let Some(route) = route {
            // An owner message reaches the relaunched coordinator in its owner-message
            // section (`lifecycle::steering`), with any earlier undelivered ones.
            let prompt = if message {
                format!("{}\n\nContinue from the retained task state.", b.goal)
            } else {
                format!(
                    "{}\n\nContinue from the retained task state. Owner input: {}",
                    b.goal,
                    request["text"].as_str().unwrap_or("Continue.")
                )
            };
            self.continue_task(b.task, b.owner, prompt, route)
                .await
                .map_err(err)?;
        } else if resume {
            self.resume_task(b.task, b.owner).await.map_err(err)?;
        }
        Ok(result)
    }
    /// RECOVERING with a recorded reason: no driver continues it on its own.
    async fn held_for_recovery(&self, task: &str) -> CoreResult<bool> {
        let task = task.to_owned();
        self.host
            .executor
            .read(move |db| {
                Ok(db
                    .query_row(
                        "SELECT state='RECOVERING' AND coalesce(blocker,'')!='' FROM runtime_tasks WHERE task_id=?",
                        [&task],
                        |r| r.get::<_, bool>(0),
                    )
                    .optional()?
                    .unwrap_or(false))
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// The runtime task bound to a desktop execution request.
    async fn control_task(&self, project: &str, request: &Value) -> CoreResult<String> {
        let (p, execution) = (
            project.to_owned(),
            request["executionId"]
                .as_str()
                .ok_or(CoreError::Refused("invalid_execution"))?
                .to_owned(),
        );
        self.host
            .executor
            .publication(move |s| {
                s.read(|db| {
                    control::binding(
                        db,
                        &s.key().root().to_string_lossy(),
                        &p,
                        &actor(),
                        &execution,
                    )
                    .map(|b| b.task)
                })
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// The coordinator route of a task: its explicit choice, else the route
    /// pinned when it started, else the owner default. An explicit or pinned
    /// route never needs a resolvable owner default.
    pub(super) async fn coordinator_route(
        &self,
        task: &str,
        fallback: Option<crate::native_sessions::SessionRoute>,
    ) -> CoreResult<crate::native_sessions::SessionRoute> {
        let settings = self.task_settings(task).await?;
        let pin = self.coordinator_pin(task).await?;
        let chosen = settings["model"].as_str().is_some_and(|m| !m.is_empty());
        if !chosen {
            if let Some(pin) = pin {
                return self.routes.pinned_route(
                    &self.host.root,
                    &pin,
                    settings["effort"].as_str().filter(|e| !e.is_empty()),
                );
            }
        }
        if settings.is_object() {
            return self.routes.task_route(&self.host.root, &settings);
        }
        match fallback {
            Some(route) => Ok(route),
            None => self.routes.route(&self.host.root, "orchestrator"),
        }
    }
    /// The coordinator route persisted when the task started.
    async fn coordinator_pin(&self, task: &str) -> CoreResult<Option<Value>> {
        let task = task.to_owned();
        self.host
            .executor
            .read(move |db| {
                let raw: Option<String> = db
                    .query_row(
                        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_COORDINATOR_ROUTE' ORDER BY id DESC LIMIT 1",
                        [&task],
                        |r| r.get(0),
                    )
                    .optional()?;
                Ok(raw.map(|s| serde_json::from_str::<Value>(&s)).transpose()?)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// Continue a resumed task without a replan: waits and questions preserved by
    /// Pause stay answerable, and interrupted work resumes through the driver's
    /// own recovery mechanisms (lost-worker retry, coordinator recovery).
    async fn resume_task(self: &Arc<Self>, task: String, owner: OwnerEpoch) -> CoreResult<()> {
        let t = task.clone();
        let state = self
            .host
            .executor
            .write(move |db| {
                let (state, _) = domain::task_tx(db, &t)?;
                let blocker: Option<String> = db.query_row(
                    "SELECT blocker FROM runtime_tasks WHERE task_id=?",
                    [&t],
                    |r| r.get(0),
                )?;
                // The runtime's own stop question has no driver; its answer relaunches.
                let stop_question: bool = db.query_row(
                    "SELECT coalesce((SELECT json_extract(payload_json,'$.runtime_stop')=1 FROM runtime_events
                        WHERE task_id=? AND type='USER_QUESTION_REQUESTED' ORDER BY id DESC LIMIT 1),0)",
                    [&t],
                    |r| r.get(0),
                )?;
                let driven = !state.terminal()
                    && state != cortex_core::TaskState::Paused
                    && blocker.as_deref().is_none_or(str::is_empty)
                    && !(state == cortex_core::TaskState::WaitingUser && stop_question);
                if driven {
                    lifecycle::event_tx(db, &t, "NATIVE_TASK_CONTINUED", json!({"state":state}))?;
                }
                Ok(driven.then_some(state))
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        let Some(state) = state else {
            return Ok(());
        };
        let (cancel, cancelled) = tokio::sync::watch::channel(false);
        let (done, finished) = tokio::sync::watch::channel(false);
        self.install_pump(task.clone(), cancel, finished).await;
        let runtime = self.clone();
        self.spawn_recovery(async move {
            let result = if state == cortex_core::TaskState::Planning {
                runtime
                    .recover_planning(&task, &owner, cancelled.clone())
                    .await
            } else {
                runtime.progress(&task, &owner, cancelled.clone()).await
            };
            if let Err(error) = result {
                // The same stop as a live driver's: a completion gap returns to the
                // coordinator (which then asks the owner) instead of hanging unasked.
                if !*cancelled.borrow() && !*runtime.stop.borrow() {
                    runtime
                        .stop_after_error(&task, &owner, error, cancelled.clone())
                        .await;
                }
            }
            done.send_replace(true);
        })
        .await;
        Ok(())
    }
    pub(crate) async fn decisions(&self, project: String, request: Value) -> io::Result<Value> {
        self.host
            .executor
            .publication(move |s| control::decisions(s, &project, &actor(), &request))
            .map_err(err)?
            .await
            .map_err(|_| failure("native_host_closed"))?
            .map_err(err)
    }
    pub(crate) async fn answer_command(
        self: &Arc<Self>,
        project: String,
        request: Value,
    ) -> io::Result<Value> {
        self.answer_command_from(project, request, None).await
    }
    pub(crate) async fn answer_command_from(
        self: &Arc<Self>,
        project: String,
        request: Value,
        session: Option<String>,
    ) -> io::Result<Value> {
        let (request, capture) = answer_files(request)?;
        let runtime = self.clone();
        let (send, reply) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _serial = runtime.controls.lock().await;
            let result = async {
                if *runtime.stop.borrow() {
                    return Err(failure("service_stopping"));
                }
                runtime
                    .host
                    .executor
                    .publication(move |s| {
                        control::answer_with_input(
                            s,
                            &project,
                            &actor(),
                            &request,
                            session.as_deref(),
                            capture.as_ref(),
                        )
                    })
                    .map_err(err)?
                    .await
                    .map_err(|_| failure("native_host_closed"))?
                    .map_err(err)
            }
            .await;
            runtime.wake.notify_waiters();
            // Python hands a committed answer to the next coordinator turn. A
            // runtime stop question has no live driver left to wake.
            let result = match result {
                Ok(receipt) => match runtime.continue_owed_tasks().await {
                    Ok(()) => Ok(receipt),
                    Err(_) => Err(failure("answer_continuation_unconfirmed")),
                },
                Err(error) => Err(error),
            };
            let _ = send.send(result);
        });
        reply
            .await
            .map_err(|_| failure("native_control_outcome_unknown"))?
    }
    /// Relaunch every coordinator owed by an answered runtime stop question,
    /// after the answer commits or after a restart. A continuation that cannot
    /// start becomes a new explicit stop or recovery reason, never a silent wait.
    pub(crate) async fn continue_owed_tasks(self: &Arc<Self>) -> CoreResult<()> {
        let tasks = self
            .host
            .executor
            .write(|db| {
                let rows = db
                    .prepare(
                        "SELECT task_id FROM runtime_tasks WHERE state='RECOVERING' ORDER BY rowid",
                    )?
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut owed = Vec::new();
                for task in rows {
                    if domain::stop_continuation_owed_tx(db, &task)? {
                        owed.push(task);
                    }
                }
                Ok(owed)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        for task in tasks {
            self.continue_owed(task).await?;
        }
        Ok(())
    }
    async fn continue_owed(self: &Arc<Self>, task: String) -> CoreResult<()> {
        // The stop already ended its driver; never run two drivers for one task.
        self.settle_task(&task).await?;
        let route = self.coordinator_route(&task, None).await;
        let refused = route.as_ref().err().map(|e| e.code().to_owned());
        let t = task.clone();
        let continued = self
            .host
            .executor
            .write(move |db| {
                if !domain::stop_continuation_owed_tx(db, &t)? {
                    return Ok(None);
                }
                if let Some(code) = refused {
                    lifecycle::event_tx(db, &t, "NATIVE_DRIVER_STOPPED", json!({"reason":code}))?;
                    domain::stop_tx(db, &t, &code)?;
                    return Ok(None);
                }
                if let Err(error) = domain::native_continue_tx(db, &t) {
                    domain::native_recovery_required_tx(db, &t, error.code())?;
                    return Ok(None);
                }
                let (owner, epoch, goal, answer) = db.query_row(
                    "SELECT o.owner_ref,o.epoch,r.goal,(SELECT q.answer_text FROM pending_questions q
                        WHERE q.task_id=r.task_id AND q.status='answered' ORDER BY q.rowid DESC LIMIT 1)
                     FROM runtime_tasks r JOIN task_owners o USING(task_id) WHERE r.task_id=?",
                    [&t],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, Option<String>>(3)?,
                        ))
                    },
                )?;
                Ok(Some((OwnerEpoch { owner, epoch }, goal, answer)))
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        if let (Some((owner, goal, answer)), Ok(route)) = (continued, route) {
            let prompt = format!(
                "{goal}\n\nContinue from the retained task state. Owner input: {}",
                answer.as_deref().unwrap_or("Continue.")
            );
            self.continue_task(task, owner, prompt, route).await?;
        }
        Ok(())
    }
    pub(crate) async fn stop_execution(
        self: &Arc<Self>,
        project: String,
        execution: String,
    ) -> io::Result<Value> {
        self.control(project.clone(),json!({"projectId":project,"executionId":execution,"requestId":format!("stop-{}",crate::identity()?),"action":"pause"})).await
    }
}
/// Files typed with an answer are checked and decoded once, here. The command that
/// Core records carries no file bytes, only `payload.capture`, the fingerprint of
/// the retained bodies, so a replay of the same answer binds to the same files.
/// `payload.captureQuestions` names, file by file, the question whose answer carried
/// it, so each answered question later shows its own files.
fn answer_files(mut request: Value) -> io::Result<(Value, Option<cortex_core::input::Capture>)> {
    let mut files = Vec::new();
    let mut owners = Vec::new();
    if let Some(answers) = request["payload"]["answers"].as_array_mut() {
        for answer in answers {
            let question = answer["questionId"].clone();
            if let Some(found) = answer.as_object_mut().and_then(|a| a.remove("attachments")) {
                match found {
                    Value::Array(items) => {
                        owners.extend(std::iter::repeat_n(question, items.len()));
                        files.extend(items);
                    }
                    Value::Null => {}
                    _ => return Err(failure("invalid_request")),
                }
            }
        }
    }
    if files.is_empty() {
        return Ok((request, None));
    }
    let capture = cortex_core::input::capture_from_wire(&Value::Array(files)).map_err(err)?;
    request["payload"]["capture"] = capture.fingerprint();
    request["payload"]["captureQuestions"] = Value::Array(owners);
    Ok((request, Some(capture)))
}
