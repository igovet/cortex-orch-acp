use super::*;
use rusqlite::OptionalExtension;

pub(super) async fn run(replan: bool, automatic: bool) -> CoreResult<()> {
    let fixture = Fixture::new();
    let scenario = if automatic { "typed-deliverables-auto" } else { "typed-deliverables" };
    std::fs::write(fixture.0.join("project/scenario.txt"), scenario)?;
    if automatic {
        let root = fixture.0.join("project");
        for args in [
            &["symbolic-ref", "HEAD", "refs/heads/main"][..],
            &["config", "user.name", "Fixture"],
            &["config", "user.email", "fixture@example.invalid"],
            &["add", "work.txt", "scenario.txt"],
            &["commit", "-qm", "Fixture baseline"],
        ] {
            assert!(std::process::Command::new("/usr/bin/git")
                .current_dir(&root).args(args).status()?.success());
        }
    }
    let service = Service::bind_native(fixture.config(), Arc::new(Routes))
        .await
        .unwrap();
    let project = service
        .register(&fixture.0.join("project"), "Typed sprint")
        .await
        .unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    service.activate(&project).await.unwrap();
    let id = machine(&service, "product.sprint", &project, json!({
        "action":"create","description":"Typed dependency fixture","runMode":if automatic {"automatic"} else {"manual"},"checkout":{"kind":"main"},
        "targetBranch":if automatic {json!("main")} else {Value::Null}
    })).await["sprintId"].clone();
    let consumer = |key: &str, version: &str| {
        json!({
            "key":key,"title":key,"description":"Consume the preceding JSON object.","dependsOn":["a"],
            "consumes":[{"from":"a","name":"sample","version":version,"kind":"json"}]
        })
    };
    let producer = json!({
        "key":"a","title":"A","description":"Produce a JSON object with an integer answer.","dependsOn":[],
        "produces":[{"name":"sample","version":"1.0","kind":"json"}]
    });
    let sprint = |items| {
        let plan = json!({"version":1,"items":items});
        json!({"action":"patch","sprintId":id,"patch":{"planning":"ready","plan":plan,"history":[plan]}})
    };
    let invalid = service
        .machine(
            serde_json::from_value(json!({"op":"product.sprint","projectId":project,
        "params":sprint(json!([producer,consumer("b", "2.0")]))}))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid["code"], "deliverable_not_declared", "{invalid}");
    let mut items = vec![producer, consumer("b", "1.0")];
    if !replan && !automatic {
        items.push(consumer("c", "1.0"));
    }
    machine(&service, "product.sprint", &project, sprint(json!(items))).await;
    let task_ids = machine(
        &service,
        "product.sprint",
        &project,
        json!({
            "action":"materialize","sprintId":id,"planVersion":1
        }),
    )
    .await["taskIds"]
        .clone();
    let (a, b) = (task_ids[0].as_str().unwrap(), task_ids[1].as_str().unwrap());
    if automatic {
        let server = tokio::spawn(service.clone().serve());
        let result = automatic_start(&service, &fixture.store(), &project, &id, a, b).await;
        service.stop().await.unwrap();
        server.await.unwrap().unwrap();
        return result;
    }
    let start =
        |task: &str, key: &str| command(&project, "tasks.start", json!(task), key, json!({}));
    let first = service
        .workspace(start(b, "b-too-early"))
        .await
        .unwrap_err();
    assert_eq!(first.to_string(), "deliverable_producer_unsettled");
    assert_eq!(
        service.workspace(start(a, "a-start")).await.unwrap()["outcome"],
        "recorded"
    );

    let store = fixture.store();
    let runtime = |card: &str| {
        store.read(|db| Ok(db.query_row(
        "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e USING(execution_id)
         WHERE w.project_id=? AND w.task_id=?", (&project,card), |r| r.get::<_,String>(0),
    )?))
    };
    let a_runtime = runtime(a)?;
    let requirement = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            let snapshot = store.read(|db| {
                let state: (String, Option<String>) = db.query_row("SELECT state,blocker FROM runtime_tasks WHERE task_id=?",
                    [&a_runtime], |r| Ok((r.get(0)?,r.get(1)?)))?;
                let ready = cortex_core::deliverables::ready(&store,db,&project,b)?;
                Ok((state,ready))
            }).unwrap();
            match snapshot {
                ((_, _), Ok(inputs)) if !inputs.is_empty() => break inputs[0].clone(),
                ((state, reason), Err(issue)) if matches!(state.as_str(), "COMPLETED"|"BLOCKED"|"WAITING_USER"|"FAILED") => {
                    let events = store.read(|db| Ok(db.prepare("SELECT type,payload_json FROM runtime_events WHERE task_id=? ORDER BY id DESC LIMIT 18")?
                        .query_map([&a_runtime], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?)).unwrap();
                    panic!("A in {state} {reason:?}: {issue:?} {events:?}");
                }
                _ => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
    }).await.expect("typed producer did not settle");
    let report = requirement["report_id"].as_str().unwrap();
    let path = store
        .key()
        .directory()
        .join(&a_runtime)
        .join(format!("{report}.md"));
    let bytes = std::fs::read(&path)?;
    std::fs::write(&path, "tampered")?;
    assert_eq!(
        service
            .workspace(start(b, "b-invalid-output"))
            .await
            .unwrap_err()
            .to_string(),
        "deliverable_invalid_evidence"
    );
    std::fs::write(&path, &bytes)?;
    std::fs::write(
        fixture.0.join("project/scenario.txt"),
        "typed-deliverables-consumer",
    )?;
    std::fs::write(fixture.0.join("project/.git/typed-producer-id"), a)?;
    assert_eq!(
        service.workspace(start(b, "b-start")).await.unwrap()["outcome"],
        "recorded"
    );
    let b_runtime = runtime(b)?;
    let evidence = store.read(|db| {
        cortex_core::deliverables::read_for_consumer(&store, db, &b_runtime, a, "sample", "1.0")
    })?;
    assert_eq!(evidence["deliverable"]["data"], json!({"answer":42}));
    assert_eq!(evidence["binding"]["report_id"], report);
    assert_eq!(
        store
            .read(|db| cortex_core::deliverables::read_for_consumer(
                &store, db, &a_runtime, a, "sample", "1.0"
            ))
            .unwrap_err()
            .code(),
        "deliverable_not_required"
    );
    assert_eq!(
        store
            .read(|db| cortex_core::deliverables::read_for_consumer(
                &store, db, &b_runtime, a, "sample", "2.0"
            ))
            .unwrap_err()
            .code(),
        "deliverable_not_required"
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while !fixture.0.join("project/.git/typed-output-read").exists() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the authenticated consumer did not receive its typed input");
    if replan {
        renewal(
            &service,
            &store,
            &fixture.0.join("project"),
            &project,
            &a_runtime,
            &b_runtime,
            a,
            b,
            report,
        )
        .await?;
        service.stop().await.unwrap();
        return Ok(());
    }
    service.stop().await.unwrap();
    // Core's real reopen transition invalidates the old immutable output. The same
    // report remains in the report store but cannot admit the next consumer.
    store.write(|db| {
        assert!(cortex_core::lifecycle::domain::reopen_tx(db, &a_runtime)?);
        Ok(())
    })?;
    assert_eq!(
        store
            .read(|db| cortex_core::deliverables::ready(
                &store,
                db,
                &project,
                task_ids[2].as_str().unwrap()
            ))?
            .unwrap_err()
            .code,
        "deliverable_producer_unsettled"
    );
    assert!(path.is_file());
    Ok(())
}

async fn automatic_start(
    service: &Service,
    store: &Store,
    project: &str,
    sprint: &Value,
    producer: &str,
    consumer: &str,
) -> CoreResult<()> {
    machine(service, "product.sprint", project, json!({
        "action":"patch","sprintId":sprint,"patch":{"queueState":"running"}
    })).await;
    let admitted = tokio::time::timeout(Duration::from_secs(70), async {
        loop {
            let binding = store.read(|db| {
                let producer: Option<String> = db.query_row(
                    "SELECT runtime_task_id FROM workspace_executions WHERE task_id=?",
                    [producer], |row| row.get(0),
                ).optional()?;
                let consumer: Option<String> = db.query_row(
                    "SELECT e.payload_json FROM workspace_executions w JOIN runtime_events e ON e.task_id=w.runtime_task_id
                     WHERE w.task_id=? AND e.type='SPRINT_INPUTS_BOUND' ORDER BY e.id DESC LIMIT 1",
                    [consumer], |row| row.get(0),
                ).optional()?;
                Ok(producer.zip(consumer))
            }).unwrap();
            if let Some(binding) = binding {
                break binding;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await;
    let (runtime, binding) = match admitted {
        Ok(binding) => binding,
        Err(_) => {
            let graph = machine(service, "product.sprint", project, json!({"action":"live_graph","sprintId":sprint})).await;
            let events = store.read(|db| {
                let mut query = db.prepare("SELECT e.type, coalesce(json_extract(e.payload_json,'$.state'),
                    json_extract(e.payload_json,'$.reason'),json_extract(e.payload_json,'$.revision'),'')
                    FROM runtime_events e JOIN workspace_executions w ON w.runtime_task_id=e.task_id
                    WHERE w.task_id=?1 OR w.task_id=?2 ORDER BY e.id DESC LIMIT 35")?;
                let events = query.query_map((producer, consumer), |row| Ok((
                    row.get::<_, String>(0)?, row.get::<_, rusqlite::types::Value>(1)?
                )))?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(events)
            })?;
            let board = machine(service, "product.board", project, json!({})).await;
            let task = &board["tasks"][producer];
            panic!("queue did not start dependent: order={} status={} git={} acceptance={} events={events:?}",
                graph["order"], task["status"], task["merge"], task["acceptance"]);
        }
    };
    let binding: Value = serde_json::from_str(&binding)?;
    let repaired_revision = binding["inputs"][0]["revision"].as_i64().unwrap();
    assert!(repaired_revision > 1, "{binding}");
    assert_eq!(binding["inputs"][0]["runtime_id"], runtime);
    store.read(|db| {
        let (missing, verified): (i64, i64) = db.query_row(
            "SELECT (SELECT id FROM runtime_events WHERE task_id=?1 AND type='SPRINT_OUTPUT_REQUIRED'
                AND json_extract(payload_json,'$.revision')=1),
             (SELECT id FROM runtime_events WHERE task_id=?1 AND type='VERIFICATION_PASSED'
                AND json_extract(payload_json,'$.revision')=?2)",
            (&runtime, repaired_revision), |row| Ok((row.get(0)?,row.get(1)?)),
        )?;
        assert!(verified > missing);
        assert_eq!(binding["inputs"][0]["verification_event"], verified);
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
async fn renewal(
    service: &Service,
    store: &Store,
    root: &Path,
    project: &str,
    a_runtime: &str,
    b_runtime: &str,
    a: &str,
    b: &str,
    original: &str,
) -> CoreResult<()> {
    async fn wait<T>(mut observe: impl FnMut() -> CoreResult<Option<T>>, context: &str) -> T {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(value) = observe().unwrap() {
                    return value;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{context}"))
    }
    let runtime = |task| {
        store.read(|db| {
            Ok(db.query_row(
                "SELECT t.state,t.revision,e.execution_id FROM runtime_tasks t
         JOIN workspace_executions e ON e.runtime_task_id=t.task_id WHERE t.task_id=?",
                [task],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?)
        })
    };
    let (_, start, b_execution) = wait(
        || {
            let state = runtime(b_runtime)?;
            Ok((state.0 == "COMPLETED").then_some(state))
        },
        "consumer first plan did not settle",
    )
    .await;
    let (_, _, a_execution) = runtime(a_runtime)?;
    std::fs::write(root.join("typed-original-report.txt"), original)?;
    std::fs::write(root.join("scenario.txt"), "typed-deliverables-renew")?;
    service
        .prompt_execution(
            project,
            &a_execution,
            "Publish a new JSON object for the input.",
            Some("renew-a"),
            json!([]),
        )
        .await
        .unwrap();
    let mut ticks = 0;
    let renewed = wait(
        || {
            store.read(|db| {
                ticks += 1;
                let ready = cortex_core::deliverables::ready(store, db, project, b)?;
                let (state, revision, blocker): (String, i64, Option<String>) = db.query_row(
                    "SELECT state,revision,blocker FROM runtime_tasks WHERE task_id=?", [a_runtime],
                    |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
                )?;
                let failed = revision > 1 && matches!(state.as_str(), "COMPLETED" | "FAILED" | "BLOCKED" | "WAITING_USER")
                    && ready.is_err();
                if failed || ticks % 250 == 0 {
                    let events = db.prepare("SELECT type, coalesce(json_extract(payload_json,'$.reason'),json_extract(payload_json,'$.code'),'')
                         FROM runtime_events WHERE task_id=? AND type IN ('NATIVE_DRIVER_STOPPED','PLAN_ACCEPTED',
                            'WORKER_COMPLETED','WORKER_FAILED','TASK_STATE_CHANGED','NATIVE_ENGINE_FAILURE',
                            'NATIVE_TURN_SETTLED','NATIVE_RESULT_REFUSED','VERIFICATION_PASSED','VERIFICATION_FAILED',
                            'SPRINT_REVISION_REQUIRED') ORDER BY id DESC LIMIT 30")?
                        .query_map([a_runtime], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    eprintln!("typed producer renewal {state} #{revision} {blocker:?}: {events:?}");
                }
                if failed {
                    panic!("renewed producer {state} {blocker:?} {ready:?}");
                }
                Ok(ready
                    .ok()
                    .and_then(|rows| rows.into_iter().next())
                    .filter(|input| input["report_id"] != original))
            })
        },
        "producer did not republish an independently verified input",
    )
    .await;
    store.read(|db| {
        let missing: String = db.query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='SPRINT_OUTPUT_REQUIRED'",
            [a_runtime], |row| row.get(0),
        )?;
        let missing = serde_json::from_str::<Value>(&missing)?;
        assert_eq!(missing["issues"], json!([
            {"code":"deliverable_missing","output":{"kind":"json","name":"sample","version":"1.0"}}]));
        let omitted_revision = missing["revision"].as_i64().unwrap();
        assert!(omitted_revision > 1);
        assert!(renewed["revision"].as_i64().unwrap() > omitted_revision, "{renewed}");
        let certified: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'
                AND json_extract(payload_json,'$.revision')=?)",
            (a_runtime, omitted_revision), |row| row.get(0),
        )?;
        assert!(!certified);
        Ok(())
    })?;
    assert_eq!(
        store
            .read(|db| cortex_core::deliverables::read_for_consumer(
                store, db, b_runtime, a, "sample", "1.0"
            ))
            .unwrap_err()
            .code(),
        "deliverable_binding_changed"
    );

    // A new plan on a different channel cannot borrow the old read from the
    // previous coordinator. Observe the exact refused driver step.
    std::fs::write(
        root.join("scenario.txt"),
        "typed-deliverables-consumer-no-read",
    )?;
    service
        .prompt_execution(
            project,
            &b_execution,
            "Use the newly published input.",
            Some("renew-b"),
            json!([]),
        )
        .await
        .unwrap();
    wait(
        || {
            store.read(|db| {
                Ok(db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM runtime_events
         WHERE task_id=? AND type='NATIVE_DRIVER_STOPPED'
           AND json_extract(payload_json,'$.reason')='deliverable_prospective_read_required')",
                        [b_runtime],
                        |r| r.get::<_, bool>(0),
                    )?
                    .then_some(()))
            })
        },
        "a coordinator that skipped the new read was not fenced",
    )
    .await;
    std::fs::remove_file(root.join(".git/typed-output-read"))?;
    std::fs::write(
        root.join("scenario.txt"),
        "typed-deliverables-consumer-renew",
    )?;
    let bound = wait(|| store.read(|db| {
        let latest: String = db.query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='SPRINT_INPUTS_BOUND' ORDER BY id DESC LIMIT 1",
            [b_runtime], |row| row.get(0),
        )?;
        let value: Value = serde_json::from_str(&latest)?;
        Ok((value["revision"].as_i64().is_some_and(|r| r > start)
            && value["inputs"][0]["report_id"] == renewed["report_id"]).then_some(value))
    }), "consumer never rebound the producer's repaired artifact").await;
    let evidence = store.read(|db| {
        cortex_core::deliverables::read_for_consumer(store, db, b_runtime, a, "sample", "1.0")
    })?;
    assert_eq!(evidence["binding_status"], "bound");
    assert_eq!(evidence["deliverable"]["data"], json!({"answer":43}));
    assert_eq!(
        evidence["binding"]["report_id"],
        bound["inputs"][0]["report_id"]
    );
    Ok(())
}
