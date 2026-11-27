//! Native desktop owner adapter. Bindings, CAS, intent and receipts are authoritative.
#[cfg(test)]
mod coordinator_card_tests {
    use super::*;
    #[test]
    fn activity_does_not_stale_a_pending_coordinator_card() {
        let root = std::env::temp_dir().join(format!("coordinator-card-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let key = crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap();
        let store = Store::open(key, crate::OpenMode::Initialize).unwrap();
        let machine = lifecycle::TaskMachine::new(&store);
        let owner = machine.create("t", "g", "o").unwrap();
        machine
            .transition(
                "t",
                &owner,
                "plan",
                TaskState::Planning,
                None,
                &lifecycle::FoundationGuards,
            )
            .unwrap();
        store
            .write(|db| {
                let q = lifecycle::skill_proposals::request(
                    db,
                    &root,
                    "t",
                    "c",
                    "ask",
                    &json!({"recommended":2,"question":"Continue?","options":["Yes","No"]}),
                )?;
                let mut b = Binding {
                    task: "t".into(),
                    draft: "d".into(),
                    session: "s".into(),
                    owner: owner.clone(),
                    version: 1,
                    execution_version: 1,
                    goal: "g".into(),
                };
                let first = decisions_tx(db, &b)?[0].clone();
                assert!(first.get("askedBy").is_none(), "{first}");
                lifecycle::event_tx(
                    db,
                    "t",
                    "NATIVE_ACTIVITY_CHANGED",
                    json!({"status":"completed"}),
                )?;
                b.execution_version += 100;
                let second = decisions_tx(db, &b)?[0].clone();
                assert_eq!(first["version"], second["version"]);
                assert_eq!(second["status"], "pending");
                waits::answer_tx(
                    db,
                    "t",
                    q["question_id"].as_str().unwrap(),
                    "Yes",
                    "text",
                    None,
                )?;
                assert_eq!(decisions_tx(db, &b)?[0]["status"], "answered");
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn coordinator_card_option_details_survive_reopen_alongside_legacy_labels() {
        let root = std::env::temp_dir().join(format!("option-details-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let key = crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap();
        let store = Store::open(key.clone(), crate::OpenMode::Initialize).unwrap();
        let machine = lifecycle::TaskMachine::new(&store);
        let owner = machine.create("t", "g", "o").unwrap();
        machine
            .transition("t", &owner, "plan", TaskState::Planning, None, &lifecycle::FoundationGuards)
            .unwrap();
        let receipt = store.write(|db| {
            lifecycle::skill_proposals::request(
                db, &root, "t", "c", "ask",
                &json!({"questions":[
                    {"recommended":1,"question":"Keep it?", "options":[{"label":"Yes","detail":"Keeps `src/a.rs:42`"}, "No"]},
                    {"recommended":2,"question":"Try again?", "options":["Retry", "Stop"]},
                ]}),
            )
        }).unwrap();
        drop(store);
        let store = Store::open(key, crate::OpenMode::Initialize).unwrap();
        store.write(|db| {
            let b = Binding {
                task: "t".into(), draft: "d".into(), session: "s".into(), owner,
                version: 1, execution_version: 1, goal: "g".into(),
            };
            let cards = decisions_tx(db, &b)?;
            assert_eq!(cards[0]["kind"], "question_batch");
            assert_eq!(cards[0]["options"][0]["label"], "Yes");
            assert_eq!(cards[0]["options"][0]["detail"], "Keeps `src/a.rs:42`");
            let option = workspace_protocol::DecisionOption::try_from(cards[0]["options"][0].clone())
                .expect("ordinary detail must survive the existing Workspace wire decoder");
            assert_eq!(option.as_value()["detail"], "Keeps `src/a.rs:42`");
            assert_eq!(option.as_value()["recommended"], true);
            assert!(cards[0]["options"][1].get("recommended").is_none());
            assert!(cards[1]["options"][0].get("recommended").is_none());
            assert_eq!(cards[1]["options"][1]["recommended"], true);
            assert!(cards[0]["options"][1].get("detail").is_none());
            assert_eq!(cards[1]["options"][0]["label"], "Retry");
            assert!(cards[1]["options"][0].get("detail").is_none());
            let id = receipt["question_id"].as_str().unwrap();
            waits::answer_tx(db, "t", id, "Yes", "text", None)?;
            let answered = &decisions_tx(db, &b)?[0];
            assert_eq!(answered["answerText"], "Yes");
            assert_eq!(answered["options"], cards[0]["options"]);
            assert!(crate::routing::evidence(db, "t", &root)?.inputs.iter()
                .any(|i| i.text == "Yes" && i.selected_option));
            assert_eq!(
                waits::answered_question_tx(db, "t", "Keep it?", &["Yes".into(), "No".into()])?,
                Some((id.into(), "Yes".into()))
            );
            Ok(())
        }).unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stop_option_of_a_coordinator_question_is_an_answer_but_the_runtime_stop_question_finishes() {
        let root = std::env::temp_dir().join(format!("stop-answer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let key = crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap();
        let store = Store::open(key, crate::OpenMode::Initialize).unwrap();
        let owner = lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        lifecycle::TaskMachine::new(&store)
            .transition(
                "t",
                &owner,
                "plan",
                TaskState::Planning,
                None,
                &lifecycle::FoundationGuards,
            )
            .unwrap();
        store
            .write(|db| {
                // Use distinct decisions so the repeat-question guard does not reuse an answer.
                for (answer, question) in [
                    ("Stop", "Stop the web daemon before deployment?"),
                    ("stop", "Should the data import continue overnight?"),
                    ("finish now", "When should we wrap up the audit?"),
                    ("Стоп", "Продолжить очистку кеша?"),
                ] {
                    let q = lifecycle::skill_proposals::request(
                        db,
                        &root,
                        "t",
                        "c",
                        &format!("ask-{answer}"),
                        &json!({"question":question,"recommended":2,"options":["Stop","Keep"]}),
                    )?;
                    assert_eq!(q["state"], "WAITING_USER", "question for {answer} did not open: {q}");
                    assert!(waits::answer_tx(db, "t", q["question_id"].as_str().unwrap(), answer, "text", None)?);
                    assert_ne!(domain::task_tx(db, "t")?.0, TaskState::Cancelled, "{answer} cancelled the task");
                    let forced: i64 = db.query_row(
                        "SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='USER_FORCE_FINISHED'",
                        [],
                        |r| r.get(0),
                    )?;
                    assert_eq!(forced, 0);
                }
                // A runtime stop question an earlier version recorded keeps its finish keywords.
                domain::state_tx(db, "t", TaskState::Executing, None)?;
                domain::legacy_stop_question_tx(db, "t", "native_session_start_failed")?;
                let id: String = db.query_row(
                    "SELECT id FROM pending_questions WHERE task_id='t' AND status='open' AND dedup_key LIKE 'runtime:stop:%'",
                    [],
                    |r| r.get(0),
                )?;
                assert!(waits::answer_tx(db, "t", &id, "Stop", "text", None)?);
                assert_eq!(domain::task_tx(db, "t")?.0, TaskState::Cancelled);
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn the_runtime_stop_question_speaks_the_task_language_and_names_the_reason() {
        let root = std::env::temp_dir().join(format!("stop-ru-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let key = crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap();
        let store = Store::open(key, crate::OpenMode::Initialize).unwrap();
        for (task, settings, goal, ru) in [
            ("ru", Some("ru"), "Write the guide", true),
            ("en", Some("en"), "Напиши руководство", false),
            (
                "auto",
                Some("auto"),
                "Изучи аватарку и опиши стиль канала",
                true,
            ),
            ("none", None, "Describe the channel style", false),
        ] {
            lifecycle::TaskMachine::new(&store)
                .create(task, "g", "o")
                .unwrap();
            store
                .write(|db| {
                    lifecycle::event_tx(db, task, "NATIVE_START_ADMITTED", json!({"goal":goal}))?;
                    if let Some(language) = settings {
                        lifecycle::event_tx(db, task, "NATIVE_TASK_SETTINGS", json!({"settings":{"language":language}}))?;
                    }
                    domain::state_tx(db, task, TaskState::Executing, None)?;
                    // The runtime itself never asks: the stop is a continuation.
                    assert_eq!(domain::stop_tx(db, task, "native_session_start_failed")?, TaskState::Recovering);
                    assert_eq!(db.query_row("SELECT count(*) FROM pending_questions WHERE task_id=?", [task], |r| r.get::<_, i64>(0))?, 0);
                    // A stored question of an earlier version still reads in the task language.
                    domain::state_tx(db, task, TaskState::Executing, None)?;
                    domain::legacy_stop_question_tx(db, task, "native_session_start_failed")?;
                    let (text, options): (String, String) = db.query_row(
                        "SELECT text,options_json FROM pending_questions WHERE task_id=? AND status='open'",
                        [task],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    assert!(!text.contains("native_session_start_failed"), "{text}");
                    if ru {
                        assert!(text.starts_with("Задача остановлена: не удалось запустить сессию воркера.\n\nЧто произошло: "), "{text}");
                        assert!(text.contains("Что можно сделать:") && text.ends_with("Что делать дальше?"), "{text}");
                        assert_eq!(options, json!(domain::STOP_OPTIONS_RU).to_string());
                    } else {
                        assert!(text.starts_with("The task stopped: a worker session could not start.\n\nWhat happened: "), "{text}");
                        assert!(text.contains("What you can do:") && text.ends_with("What should happen next?"), "{text}");
                        assert_eq!(options, json!(domain::STOP_OPTIONS_EN).to_string());
                    }
                    Ok(())
                })
                .unwrap();
        }
        // The Russian finish option, exactly as offered, finishes the task.
        store
            .write(|db| {
                let id: String = db.query_row(
                    "SELECT id FROM pending_questions WHERE task_id='ru' AND status='open'",
                    [],
                    |r| r.get(0),
                )?;
                assert!(waits::answer_tx(
                    db,
                    "ru",
                    &id,
                    domain::STOP_OPTIONS_RU[1],
                    "text",
                    None
                )?);
                assert_eq!(domain::task_tx(db, "ru")?.0, TaskState::Cancelled);
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn activity_does_not_stale_a_pending_worker_card_but_revocation_still_fences_it() {
        let root = std::env::temp_dir().join(format!("worker-card-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(
            crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap(),
            crate::OpenMode::Initialize,
        )
        .unwrap();
        let owner = lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        store.write(|db|{
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'work','worker-backend-dev','Work','[]','[\"src\"]','execution','running','assignment',NULL)",[])?;
            let c=crate::channels::issue_tx(db,"t",&owner,crate::channels::Principal::Worker{assignment:"assignment"})?;
            crate::channels::bind_session_tx(db,&c,"engine")?;
            let wait = waits::request_tx_bound(db,"t",&c.channel_id,"question",&[waits::QuestionForm{
                question:"Continue?".into(),options:vec![json!({"label":"Yes","detail":"Keep the current route"}),"No".into()]}])?;
            assert_eq!(wait["questions"][0]["options"], json!(["Yes", "No"]));
            let mut b=Binding{task:"t".into(),draft:"d".into(),session:"s".into(),owner:owner.clone(),version:1,execution_version:1,goal:"g".into()};
            let first=decisions_tx(db,&b)?[0].clone();
            // The worker's question is the owner's to answer in the task's Conversation, named by the role that asked.
            assert_eq!(first["askedBy"],"backend-dev");
            assert_eq!(first["options"][0]["label"],"Yes");
            assert_eq!(first["options"][0]["detail"],"Keep the current route");
            assert_eq!(first["status"],"pending");
            lifecycle::event_tx(db,"t","NATIVE_TURN_SETTLED",json!({"status":"completed"}))?;
            b.execution_version+=100;
            let second=decisions_tx(db,&b)?[0].clone();
            assert_eq!(first["version"],second["version"]);
            assert_eq!(second["status"],"pending");
            crate::channels::revoke_tx(db,&c.channel_id)?;
            assert_eq!(decisions_tx(db,&b)?[0]["status"],"superseded");
            Ok(())
        }).unwrap();
        // The refused desktop answer commits the authority-loss cancellation separately,
        // so the question is not left pending but unanswerable.
        let open = |store: &Store| {
            store
                .read(|db| {
                    Ok(db.query_row(
                        "SELECT count(*) FROM pending_questions WHERE status='open'",
                        [],
                        |r| r.get::<_, i64>(0),
                    )?)
                })
                .unwrap()
        };
        assert_eq!(open(&store), 1);
        assert_eq!(
            store
                .write(|db| waits::cancel_unauthorized_tx(db, "t"))
                .unwrap(),
            1
        );
        assert_eq!(open(&store), 0);
        assert_eq!(
            store
                .write(|db| waits::cancel_unauthorized_tx(db, "t"))
                .unwrap(),
            0
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    /// The runtime's stop question carries each answer's consequence, the recommended
    /// answer and what the desktop can open; the question it replaced names its successor.
    #[test]
    fn a_runtime_stop_card_carries_consequences_targets_and_its_replacement() {
        let root = std::env::temp_dir().join(format!("stop-card-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(
            crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap(),
            crate::OpenMode::Initialize,
        )
        .unwrap();
        let owner = lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        store.write(|db|{
            db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1 WHERE task_id='t'",[])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'_review_1','worker-code-reviewer','Review','[]','[\"**\"]','review','completed','rv',?)",
                [json!({"verdict":"BLOCK","summary":"Not traced.","report_ref":"r_0123456789ab"}).to_string()])?;
            domain::legacy_stop_question_tx(db,"t","native_session_start_failed")?;
            domain::legacy_stop_question_tx(db,"t","review_blocked")?;
            let b=Binding{task:"t".into(),draft:"d".into(),session:"s".into(),owner:owner.clone(),version:1,execution_version:1,goal:"g".into()};
            let cards=decisions_tx(db,&b)?;
            assert_eq!(cards.len(),2);
            assert_eq!(cards[0]["status"],"superseded");
            assert_eq!(cards[0]["replacedBy"],cards[1]["decisionId"]);
            let card=&cards[1];
            assert_eq!(card["status"],"pending");
            assert_eq!(card["runtimeStop"],true);
            assert_eq!(card["options"][0]["action"],"continue");
            assert_eq!(card["options"][0]["recommended"],true);
            assert_eq!(card["options"][1]["action"],"finish");
            assert!(card["options"][1]["detail"].as_str().unwrap().contains("findings stay unaddressed"));
            assert_eq!(card["targets"],json!([{"kind":"report","reportId":"r_0123456789ab","role":"code-reviewer"}]));
            assert_eq!(card["headline"],"The task stopped: the independent review blocked the result.");
            // Its options act by themselves: an option answer takes no added words.
            assert_eq!(card["optionsOnly"],true);
            Ok(())
        }).unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    /// An answered question reads the files that came with its own answer. An older
    /// record that does not name its question is shown only when unambiguous.
    #[test]
    fn an_answered_question_reads_its_own_files_and_an_older_record_only_when_unambiguous() {
        let root = std::env::temp_dir().join(format!("answer-files-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(
            crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap(),
            crate::OpenMode::Initialize,
        )
        .unwrap();
        let owner = lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        lifecycle::TaskMachine::new(&store)
            .transition(
                "t",
                &owner,
                "plan",
                TaskState::Planning,
                None,
                &lifecycle::FoundationGuards,
            )
            .unwrap();
        store
            .write(|db| {
                let sha = "a".repeat(64);
                let file = |name: &str, index: usize, uri: Value| {
                    json!({"kind":"image","mime":"image/png","name":name,"uri":uri,"sha256":sha,"size_bytes":3,
                        "availability":"stored","reason":null,"trust_label":"user_direct","source_ref":format!("r_0:{index}")})
                };
                let reference = |name: &str| json!([{"name":name,"mimeType":"image/png","size":3,"sha256":sha}]);
                let ask = |key: &str| -> CoreResult<String> {
                    Ok(lifecycle::skill_proposals::request(db, &root, "t", "c", key,
                        &json!({"recommended":2,"question":key,"options":["Yes","No"]}))?["question_id"].as_str().unwrap().to_owned())
                };
                // Named: the file belongs to the question its answer named.
                let named = ask("named")?;
                assert!(waits::answer_tx(db, "t", &named, "Yes", "text", None)?);
                lifecycle::event_tx(db, "t", "USER_ANSWER_ATTACHMENTS",
                    json!({"attachments":[file("a.png", 0, Value::Null)],"question_ids":[named]}))?;
                // Older desktop record right after one lone question's answer: shown there.
                let older = ask("older")?;
                assert!(waits::answer_tx(db, "t", &older, "Yes", "text", None)?);
                lifecycle::event_tx(db, "t", "USER_ANSWER_ATTACHMENTS",
                    json!({"attachments":[file("b.png", 0, Value::Null)]}))?;
                // Older record kept before its answer (a typed prompt's resource): never guessed.
                let typed = ask("typed")?;
                lifecycle::event_tx(db, "t", "USER_ANSWER_ATTACHMENTS",
                    json!({"attachments":[file("c.png", 1, json!("file:///c.png"))]}))?;
                assert!(waits::answer_tx(db, "t", &typed, "No", "text", None)?);
                let b = Binding{task:"t".into(),draft:"d".into(),session:"s".into(),owner:owner.clone(),version:1,execution_version:1,goal:"g".into()};
                let cards = decisions_tx(db, &b)?;
                let of = |q: &str| cards.iter().find(|c| c["questionId"] == q).unwrap()["answerAttachments"].clone();
                assert_eq!(of(&named), reference("a.png"));
                assert_eq!(of(&older), reference("b.png"));
                assert!(of(&typed).is_null(), "{cards:?}");
                // An ordinary question's options take added words; nothing marks it options-only.
                assert!(cards.iter().all(|c| c.get("optionsOnly").is_none()));
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
use crate::{
    error::refuse,
    lifecycle::{self, domain, waits},
    ownership::OwnerEpoch,
    projections, CoreError, CoreResult, Store, TaskState,
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

#[derive(Clone)]
pub struct Binding {
    pub task: String,
    pub draft: String,
    pub session: String,
    pub owner: OwnerEpoch,
    pub version: i64,
    pub execution_version: i64,
    pub goal: String,
}
pub fn binding(
    db: &Connection,
    root: &str,
    project: &str,
    actor: &str,
    execution: &str,
) -> CoreResult<Binding> {
    let binding = binding_with(db, root, project, actor, execution, true)?;
    let task = projections::task(db, project, root, &binding.draft)?
        .ok_or(CoreError::Refused("native_execution_binding_invalid"))?;
    if task.execution_id.as_deref() != Some(execution)
        || task
            .warnings
            .iter()
            .any(|w| w == projections::CONTRADICTORY_EVIDENCE)
    {
        return refuse("native_execution_binding_invalid");
    }
    Ok(binding)
}
/// Read-only binding: the same project/root/owner/session identity, without
/// requiring the Rust host to own the task. Completed tasks the Python host ran
/// are never adopted, but their questions must still be readable (never answerable:
/// a terminal task offers no answer operation).
fn read_binding(
    db: &Connection,
    root: &str,
    project: &str,
    actor: &str,
    execution: &str,
) -> CoreResult<Binding> {
    binding_with(db, root, project, actor, execution, false)
}
fn binding_with(
    db: &Connection,
    root: &str,
    project: &str,
    actor: &str,
    execution: &str,
    native: bool,
) -> CoreResult<Binding> {
    let row = db.query_row(
        "SELECT e.runtime_task_id,t.task_id,e.acp_session_id,o.owner_ref,o.epoch,t.version,e.version,r.goal
         FROM workspace_executions e JOIN workspace_tasks t USING(task_id)
         JOIN workspace_projects p USING(project_id) JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
         JOIN tasks rt ON rt.id=r.task_id JOIN task_owners o ON o.task_id=r.task_id
         JOIN acp_host_sessions s ON s.session_id=e.acp_session_id
         WHERE p.project_id=? AND p.project_root=? AND p.activated_by=? AND rt.project_root=p.project_root
         AND e.execution_id=? AND e.owner_epoch=o.epoch AND s.task_id=r.task_id AND s.project_root=p.project_root
         AND (?5=0 OR EXISTS(SELECT 1 FROM runtime_events v WHERE v.task_id=r.task_id AND v.type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED')
         AND json_extract(v.payload_json,'$.execution_host')='rust'))",
        (project,root,actor,execution,native), |r| Ok(Binding {
            task:r.get(0)?,draft:r.get(1)?,session:r.get(2)?,
            owner:OwnerEpoch {owner:r.get(3)?,epoch:r.get(4)?},version:r.get(5)?,
            execution_version:r.get(6)?,goal:r.get(7)?,
        })).optional()?;
    row.ok_or(CoreError::Refused("native_execution_binding_invalid"))
}
/// An owner disposition for the stable Workspace row, never an admission
/// against a possibly unrelated runtime task or session.
pub fn close_unconfirmed(
    store: &Store,
    project: &str,
    actor: &str,
    task: &str,
    execution: &str,
    version: workspace_protocol::core::Version,
) -> CoreResult<()> {
    let root = store.key().root().to_string_lossy();
    store.write(|db| {
        let owns_project: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspace_projects
             WHERE project_id=? AND project_root=? AND activated_by=?)",
            (project, root.as_ref(), actor),
            |r| r.get(0),
        )?;
        if !owns_project {
            return refuse("unauthorized");
        }
        let projection = projections::task(db, project, &root, task)?
            .ok_or(CoreError::Refused("invalid_task"))?;
        if projection.execution_id.as_deref() != Some(execution) {
            return refuse("stale_version");
        }
        let warned = |warning| projection.warnings.iter().any(|w| w == warning);
        if warned(projections::CLOSED_UNCONFIRMED) {
            return Ok(());
        }
        if projection.version != version {
            return refuse("stale_version");
        }
        if !warned(projections::CONTRADICTORY_EVIDENCE) {
            return refuse("records_not_conflicting");
        }
        let receipt = json!({"taskId": task, "executionId": execution, "closedAt": crate::now()});
        let identity = json!({"taskId":task,"executionId":execution,"expectedVersion":version});
        db.execute(
            "INSERT INTO workspace_requests VALUES (?,?,?,?,?,?)",
            (
                project,
                actor,
                projections::unconfirmed_close_key(db, task, execution)?,
                projections::CLOSE_UNCONFIRMED,
                crate::digest(identity.to_string().as_bytes()),
                receipt.to_string(),
            ),
        )?;
        let next = version
            .get()
            .checked_add(1)
            .ok_or(CoreError::Refused("invalid_version"))?;
        db.execute(
            "UPDATE workspace_tasks SET version=? WHERE project_id=? AND task_id=?",
            (next, project, task),
        )?;
        crate::store::append_public_tx(db, project, task, "task", next)?;
        Ok(())
    })
}

fn key(request: &Value) -> CoreResult<&str> {
    let key = request["requestId"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_request"))?;
    workspace_protocol::validate("Identity", &json!(key))
        .map_err(|_| CoreError::Refused("invalid_request"))?;
    Ok(key)
}
fn prior(db: &Connection, b: &Binding, request: &Value) -> CoreResult<Option<Value>> {
    let row: Option<(String,String)> = db.query_row(
        "SELECT digest,result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=? AND operation='native_desktop_control'",
        (&b.task,&b.owner.owner,key(request)?), |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((digest, _)) = row else {
        return Ok(None);
    };
    // request_tx uses the canonical JSON digest.
    if digest
        != crate::digest(&serde_json::to_vec(
            &json!({"operation":"native_desktop_control","payload":request}),
        )?)
    {
        return refuse("payload_conflict");
    }
    let finished: Option<String> = db.query_row(
        "SELECT result_json FROM runtime_requests WHERE task_id=? AND actor='native-desktop-finish' AND request_key=?",
        (&b.task,key(request)?), |r| r.get(0)).optional()?;
    let encoded = finished.ok_or(CoreError::Refused("native_control_outcome_unknown"))?;
    let receipt: Value = serde_json::from_str(&encoded)?;
    // An explicitly settled unknown outcome never claims the control happened.
    if receipt["outcome"] == "unknown" {
        return refuse("native_control_outcome_unknown");
    }
    Ok(Some(receipt))
}
/// Bound an admitted control that never reached its finish receipt. The finish
/// slot is closed with an explicit `outcome: unknown` receipt (revision and time
/// recorded), the task is held in a retryable recovery state and the request
/// keeps replaying as `native_control_outcome_unknown`. Nothing is claimed to
/// have happened, and later restarts no longer re-raise this admission.
pub fn settle_unknown(
    store: &Store,
    project: &str,
    actor: &str,
    request: &Value,
    reason: &str,
) -> CoreResult<()> {
    store.write(|db| {
        let b = binding(
            db,
            &store.key().root().to_string_lossy(),
            project,
            actor,
            request["executionId"]
                .as_str()
                .ok_or(CoreError::Refused("invalid_execution"))?,
        )?;
        let key = key(request)?;
        let admitted: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=? AND operation='native_desktop_control')",
            (&b.task, &b.owner.owner, key),
            |r| r.get(0),
        )?;
        let finished: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor='native-desktop-finish' AND request_key=?)",
            (&b.task, key),
            |r| r.get(0),
        )?;
        if !admitted || finished {
            return Ok(());
        }
        let revision = domain::task_tx(db, &b.task)?.1;
        lifecycle::request_tx(
            db,
            &b.task,
            "native-desktop-finish",
            key,
            "native_desktop_finished",
            request,
            || {
                lifecycle::event_tx(
                    db,
                    &b.task,
                    "NATIVE_CONTROL_OUTCOME_UNKNOWN",
                    json!({"request":key,"action":request["action"],"reason":reason,"revision":revision}),
                )?;
                // Pause/stop are visible in the state itself; other actions leave the
                // task unobservable until the owner retries.
                if !matches!(
                    request["action"].as_str(),
                    Some("task_settings" | "worker_message" | "rerun_check" | "pause" | "stop")
                ) && !lifecycle::automatic::held(db, &b.task)?
                    // No-stop policy: the unknown control outcome is a continuation that
                    // records it; only a paused or waiting task keeps it as its barrier.
                    && !lifecycle::automatic::continue_unsettled_tx(
                        db,
                        &b.task,
                        "native_control_outcome_unknown",
                    )?
                {
                    domain::native_recovery_required_tx(
                        db,
                        &b.task,
                        "native_control_outcome_unknown",
                    )?;
                }
                Ok(json!({"outcome":"unknown","native":true,"reason":reason,
                    "revision":revision,"settledAt":crate::now()}))
            },
        )?;
        Ok(())
    })
}
pub fn replay(
    store: &Store,
    project: &str,
    actor: &str,
    request: &Value,
) -> CoreResult<Option<Value>> {
    store.read(|db| {
        let b = binding(
            db,
            &store.key().root().to_string_lossy(),
            project,
            actor,
            request["executionId"]
                .as_str()
                .ok_or(CoreError::Refused("invalid_execution"))?,
        )?;
        prior(db, &b, request)
    })
}
pub fn admit(store: &Store, project: &str, actor: &str, request: &Value) -> CoreResult<Binding> {
    admit_input(store, project, actor, request, None)
}
/// Build the trusted request identity once, before replay, admission and finish.
/// Client-provided fingerprints are not accepted.
pub fn canonical_request(
    mut request: Value,
    capture: Option<&crate::input::Capture>,
) -> CoreResult<Value> {
    if !request.is_object() || request.get("capture").is_some() {
        return refuse("invalid_request");
    }
    if let Some(c) = capture.filter(|c| !c.blocks.is_empty()) {
        if !matches!(
            request["action"].as_str(),
            Some("prompt" | "incoming_prompt" | "rework")
        ) {
            return refuse("native_attachments_unavailable");
        }
        request["capture"] = c.fingerprint();
    }
    Ok(request)
}
const OWNER_REWORK: &str = "workspace_owner_rework";

/// Saved owner disposition, read under the Workspace machine lock. It is
/// trusted adapter evidence, never supplied in the public request.
pub struct ReworkDisposition<'a> {
    pub archived: bool,
    pub closed: bool,
    pub integrated: bool,
    pub target: Option<&'a str>,
    pub history: Value,
    pub worktree_root: &'a std::path::Path,
}

pub fn owner_rework_event(db: &Connection, task: &str) -> CoreResult<Option<i64>> {
    Ok(db.query_row(
        "SELECT max(id) FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED'
         AND json_extract(payload_json,'$.origin')='owner_rework'",
        [task], |r| r.get(0),
    )?)
}

pub fn owner_resolution_current(db: &Connection, task: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1
         AND type IN ('OWNER_TASK_RESOLVED','USER_FORCE_FINISHED')
         AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0))",
        [task], |r| r.get(0),
    )?)
}

pub fn rework_allowed(
    db: &Connection, task: &str, closed: bool, archived: bool,
) -> CoreResult<()> {
    if archived {
        return refuse("task_archived");
    }
    match db.query_row("SELECT state FROM runtime_tasks WHERE task_id=?", [task], |r| r.get::<_, String>(0))?.as_str() {
        "CANCELLED" | "FAILED" => {}
        "COMPLETED" if closed || owner_resolution_current(db, task)? => {}
        "PAUSED" if closed => {}
        _ => return refuse("task_not_closed"),
    }
    lifecycle::automatic::require_settled_execution(db, task)
}

pub fn rework_binding(
    db: &Connection, root: &str, project: &str, actor: &str, request: &Value,
) -> CoreResult<Binding> {
    if request["projectId"] != project || request["action"] != "rework" {
        return refuse("invalid_request");
    }
    let execution = request["executionId"].as_str().ok_or(CoreError::Refused("invalid_execution"))?;
    let b = rework_binding_for_execution(db, root, project, actor, execution)?;
    if request["taskId"] != b.draft {
        return refuse("invalid_task");
    }
    if request["ownerEpoch"] != b.owner.epoch.to_string() {
        return refuse("stale_epoch");
    }
    Ok(b)
}

/// A close without confirmation grants no runtime authority. It is eligible
/// only once the exact underlying execution can be independently reconciled.
pub fn rework_binding_for_execution(
    db: &Connection, root: &str, project: &str, actor: &str, execution: &str,
) -> CoreResult<Binding> {
    let b = binding_with(db, root, project, actor, execution, true)?;
    let raw = projections::task_with_disposition(db, project, root, &b.draft, false)?
        .ok_or(CoreError::Refused("native_execution_binding_invalid"))?;
    if raw.execution_id.as_deref() != Some(execution)
        || raw.warnings.iter().any(|w| w == projections::CONTRADICTORY_EVIDENCE)
    {
        return refuse("native_execution_binding_invalid");
    }
    Ok(b)
}

pub fn unconfirmed_close_current(
    db: &Connection, project: &str, root: &str, task: &str, execution: &str,
) -> CoreResult<bool> {
    Ok(projections::unconfirmed_close_at(db, project, root, task, execution)?.is_some())
}

pub fn replay_rework(db: &Connection, b: &Binding, request: &Value) -> CoreResult<Option<Value>> {
    let prior: Option<(String, String)> = db.query_row(
        "SELECT digest,result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
        (&b.task, &b.owner.owner, key(request)?), |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    prior.map(|(digest,result)| {
        let expected = crate::digest(&serde_json::to_vec(&json!({"operation":OWNER_REWORK,"payload":request}))?);
        if digest != expected { return refuse("payload_conflict"); }
        Ok(serde_json::from_str(&result)?)
    }).transpose()
}

/// Reopen, bind the working base, keep the owner's new input, and record the
/// planning state and receipt together. The host starts the ordinary driver
/// only after this commits. A repeated request has no Git or message effects.
pub fn rework(
    store: &Store,
    project: &str,
    actor: &str,
    request: &Value,
    capture: Option<&crate::input::Capture>,
    disposition: ReworkDisposition<'_>,
) -> CoreResult<Value> {
    let capture = capture.filter(|c| !c.blocks.is_empty());
    if request.get("capture") != capture.map(|c| c.fingerprint()).as_ref() {
        return refuse("invalid_input_capture");
    }
    let root = store.key().root().to_string_lossy();
    let mut replacement = None;
    let mut task = None;
    let result = store.write(|db| {
        let b = rework_binding(db, &root, project, actor, request)?;
        task = Some(b.task.clone());
        lifecycle::request_tx(db, &b.task, &b.owner.owner, key(request)?, OWNER_REWORK, request, || {
            if disposition.archived {
                return refuse("task_archived");
            }
            if request["expectedVersion"] != b.version.to_string() {
                return refuse("stale_version");
            }
            let closed = disposition.closed
                || unconfirmed_close_current(db, project, &root, &b.draft, request["executionId"].as_str().unwrap())?;
            rework_allowed(db, &b.task, closed, false)?;
            checked_owner_text(request)?;
            let generation = format!("{}:rework:{}", b.task, key(request)?);
            let base = super::worktrees::rework_checkout(
                db, store.key(), project, &b, &generation, disposition.worktree_root,
                disposition.target, disposition.integrated, &mut replacement,
            )?;
            let mut change = json!({"request":key(request)?,"baseRevision":base,"closed":disposition.history,
                "targetBranch":disposition.target,"resetChangesBase":disposition.integrated});
            if let Some((tree, witness)) = &replacement {
                change["worktree"] = serde_json::to_value(tree)?;
                change["worktreeWitness"] = serde_json::to_value(witness)?;
            }
            let event = domain::owner_rework_tx(db, &b.task, change)?;
            owner_message_tx(store, db, &b, request, capture, TaskState::Recovering, false)?;
            domain::settle_recovery_tx(db, &b.task, TaskState::Planning)?;
            Ok(json!({"outcome":"recorded","done":true,"state":"PLANNING",
                "reworkEvent":event.to_string(),"baseRevision":base}))
        })
    });
    if result.is_err() {
        if let Some(task) = task {
            super::worktrees::abandon_unbound_rework(store, &task, replacement);
        }
    }
    result
}

/// An owned process that has not been confirmed gone.
fn unsettled_tx(db: &rusqlite::Transaction<'_>, task: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND status!='terminated')
         OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')",
        (task, task),
        |r| r.get(0),
    )?)
}
pub(super) fn checked_owner_text(request: &Value) -> CoreResult<(&str, Vec<String>, Vec<String>)> {
    let text=request["text"].as_str().filter(|s|!s.trim().is_empty()&&s.chars().count()<=32_000).ok_or(CoreError::Refused("invalid_message"))?;
    if request.get("attachments").is_some_and(|a|a.as_array().is_none_or(|a|!a.is_empty())) {return refuse("native_attachments_unavailable");}
    // Parsed before anything is kept: a denial outside the project refuses whole.
    let denied=lifecycle::steering::parse_denials(text)?;
    let lifted=lifecycle::steering::parse_lifts(text)?;
    // One message never both denies and allows the same path.
    if lifted.iter().any(|p|denied.contains(p)) {return refuse("write_rule_conflict");}
    Ok((text, denied, lifted))
}

/// Use the same source, steering and transcript path for an owner message on rework.
pub(super) fn owner_message_tx(
    store: &Store,
    db: &Transaction<'_>,
    b: &Binding,
    request: &Value,
    capture: Option<&crate::input::Capture>,
    state: TaskState,
    boundary: bool,
) -> CoreResult<()> {
    let (text, denied, lifted) = checked_owner_text(request)?;
    let sessions=db.prepare("SELECT session_id FROM acp_host_sessions WHERE task_id=?
        AND json_extract(client_info,'$.name')='cortex-rust-acp'")?
        .query_map([&b.task],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    if sessions.len()>1 {return refuse("incoming_session_binding_conflict");}
    let attachments=match capture {
        Some(c)=>Some(c.persist(store,db,&b.task,sessions.first().unwrap_or(&b.session),text)?),
        None=>None,
    };
    let mut steering=json!({"text":text,"request":key(request)?,"deny_write":denied,
        "delivery":if boundary {lifecycle::steering::BOUNDARY} else {lifecycle::steering::RESTART}});
    if !lifted.is_empty() {steering["allow_write"]=json!(lifted);}
    if let Some(a)=&attachments {steering["attachments"]=a.clone();}
    lifecycle::event_tx(db,&b.task,"USER_STEERING",steering)?;
    // A restart applies the message now: the relaunched coordinator gets it.
    if !boundary {lifecycle::event_tx(db,&b.task,"STEERING_APPLIED",json!({"request":key(request)?,"state":state}))?;}
    let attachments=attachments.map(|a|a.to_string());
    let at=crate::now();
    for session in std::iter::once(&b.session).chain(sessions.iter()) {
        let seq:i64=db.query_row("SELECT coalesce(max(seq),-1)+1 FROM (
            SELECT seq FROM host_transcript WHERE session_id=? UNION ALL SELECT seq FROM host_updates WHERE session_id=?)",
            (session,session),|r|r.get(0))?;
        db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,attachments_json,created_at) VALUES (?,?,'user_message',?,?,?)",
            (session,seq,text,&attachments,&at))?;
    }
    Ok(())
}

/// Admit a canonical request from `canonical_request`. Captured attachments
/// become the next source revision in the same transaction as steering and
/// transcript rows. Replay and finish must receive this same request.
pub fn admit_input(
    store: &Store,
    project: &str,
    actor: &str,
    request: &Value,
    capture: Option<&crate::input::Capture>,
) -> CoreResult<Binding> {
    let capture = capture.filter(|c| !c.blocks.is_empty());
    if capture.is_some()
        && !matches!(
            request["action"].as_str(),
            Some("prompt" | "incoming_prompt")
        )
    {
        return refuse("native_attachments_unavailable");
    }
    // Fail closed if a caller did not bind the exact capture before replay.
    if request.get("capture") != capture.map(|c| c.fingerprint()).as_ref() {
        return refuse("invalid_input_capture");
    }
    store.write(|db| {
        let b = binding(db,&store.key().root().to_string_lossy(),project,actor,
            request["executionId"].as_str().ok_or(CoreError::Refused("invalid_execution"))?)?;
        if prior(db,&b,request)?.is_some() { return refuse("native_control_already_recorded"); }
        let action = request["action"].as_str().ok_or(CoreError::Refused("invalid_action"))?;
        let mut state = domain::task_tx(db,&b.task)?.0;
        // An owner message to a finished result (Request changes) reopens it for a
        // fresh plan, as Python's steering does. Cancelled tasks stay ended.
        if matches!(action,"prompt"|"incoming_prompt") && domain::reopen_tx(db,&b.task)? {
            state = TaskState::Recovering;
        }
        // A finished (not cancelled) task keeps its denials for a reopen; the owner may lift one.
        if state.terminal() && !matches!(action,"close_task"|"mark_resolved") && !(action=="allow_write" && state!=TaskState::Cancelled) { return refuse("task_not_resumable"); }
        lifecycle::request_tx(db,&b.task,&b.owner.owner,key(request)?,"native_desktop_control",request,|| {
            match action {
                "pause" | "stop" => {
                    domain::owner_pause_tx(db,&b.task,&b.owner,&format!("desktop:{}",key(request)?),false,b.version)?;
                    db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND revoked_at IS NULL",(crate::now(),&b.task))?;
                },
                "resume" => {
                    // Refuse before any commit while an owned process is unconfirmed, so a
                    // refusal never leaves a resumed task without a driver.
                    if unsettled_tx(db,&b.task)? {return refuse("native_shutdown_unconfirmed");}
                    let version = request["expectedVersion"].as_str().and_then(|s| s.parse().ok()).ok_or(CoreError::Refused("stale_version"))?;
                    domain::owner_pause_tx(db,&b.task,&b.owner,&format!("desktop:{}",key(request)?),true,version)?;
                },
                "retry" => {
                    // A node waiting out an infrastructure retry delay retries at once on
                    // the owner's Retry: the delay only spares model calls, never the owner.
                    if state==TaskState::Executing {
                        let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64());
                        let waiting:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events e JOIN runtime_tasks t USING(task_id)
                            WHERE e.task_id=?1 AND e.type='ASSIGNMENT_RETRIED' AND json_extract(e.payload_json,'$.revision')=t.revision
                            AND json_extract(e.payload_json,'$.retry_not_before')>?2
                            AND e.id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='OWNER_RETRY_REQUESTED'),0))",
                            (&b.task, now), |r| r.get(0))?;
                        if !waiting { return refuse("retry_not_expected"); }
                        lifecycle::event_tx(db,&b.task,"OWNER_RETRY_REQUESTED",json!({"request":key(request)?}))?;
                    } else {
                    if !matches!(state,TaskState::Recovering|TaskState::WaitingUser|TaskState::Blocked) { return refuse("retry_not_expected"); }
                    // Retry is the owner's explicit acknowledgement of effects whose outcome
                    // stayed unknown (e.g. a command interrupted by a restart). Record them as
                    // settled-unknown, never as success, so recovery stops re-raising them on
                    // every service restart.
                    let acknowledged=db.execute("UPDATE runtime_requests SET result_json=json_set(CASE WHEN json_valid(result_json) THEN result_json ELSE '{}' END,
                        '$.settled',1,'$.outcome_unknown',1,'$.settled_by','owner_retry','$.settled_at',?)
                        WHERE task_id=? AND operation IN ('outside_command','outside_write','command_run','project_write','project_edit','project_mkdir','project_delete','project_skill_link','draft_write','external_call')
                        AND json_extract(result_json,'$.complete') IS NOT 1 AND json_extract(result_json,'$.status') IS NOT 'complete' AND json_extract(result_json,'$.settled') IS NOT 1",
                        (crate::now(),&b.task))?;
                    if acknowledged>0 {
                        lifecycle::event_tx(db,&b.task,"OWNER_ACKNOWLEDGED_UNKNOWN_EFFECTS",json!({"count":acknowledged,"request":key(request)?}))?;
                    }
                    domain::interrupt_tx(db,&b.task)?;
                    }
                },
                "prompt" | "incoming_prompt" => {
                    checked_owner_text(request)?;
                    let boundary=lifecycle::steering::boundary_tx(db,&b.task,state)?;
                    if request["queued"]==true && !boundary {return refuse("queued_message_not_at_boundary");}
                    if state==TaskState::Paused {
                        if action!="incoming_prompt" {return refuse("task_paused");}
                        // A new owner message explicitly resumes the ACP chat. Resume
                        // and source capture commit together, before any engine starts.
                        // Same pre-check as Resume: a refusal commits nothing.
                        if unsettled_tx(db,&b.task)? {return refuse("native_shutdown_unconfirmed");}
                        domain::owner_pause_tx(db,&b.task,&b.owner,&format!("incoming-resume:{}",key(request)?),true,b.version)?;
                    }
                    owner_message_tx(store, db, &b, request, capture, state, boundary)?;
                    if !boundary {domain::interrupt_tx(db,&b.task)?;}
                },
                "close_task" | "mark_resolved" => {
                    if request["confirmed"]!=true {return refuse("confirmation_required");}
                    if request.get("expectedVersion").is_some_and(|v|*v!=b.version.to_string()) {return refuse("stale_version");}
                    if state!=TaskState::Completed { domain::cancel_task_tx(db,&b.task,"owner_resolved_without_certification")?; }
                },
                // The owner removes one of their own write denials; running work is untouched.
                "allow_write" => {
                    let path=request["path"].as_str().ok_or(CoreError::Refused("write_denial_unknown"))?;
                    lifecycle::steering::lift_tx(db,&b.task,path,key(request)?)?;
                },
                _ => return refuse("native_operation_unavailable"),
            }
            Ok(json!({"admitted":true}))
        })?;
        Ok(b)
    })
}
pub fn finish(
    store: &Store,
    project: &str,
    actor: &str,
    request: &Value,
    restart: bool,
) -> CoreResult<Value> {
    store.write(|db| {
        let b=binding(db,&store.key().root().to_string_lossy(),project,actor,request["executionId"].as_str().unwrap())?;
        lifecycle::request_tx(db,&b.task,"native-desktop-finish",key(request)?,"native_desktop_finished",request,|| {
            let unsettled: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND status!='terminated')
                 OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')",
                (&b.task,&b.task), |r|r.get(0))?;
            let closing=matches!(request["action"].as_str(),Some("close_task"|"mark_resolved"));
            // A close already committed its cancellation at admission (or the task had
            // finished): the task is closed whatever an earlier attempt's process did, so
            // the close is reported done. The unconfirmed process stays recorded and keeps
            // deletion refused until it is settled; nothing claims it exited.
            let state=domain::task_tx(db,&b.task)?.0;
            // An owner message recorded for the coordinator's next boundary leaves the
            // running work (and its live processes) alone: nothing restarts.
            // Lifting a write denial likewise leaves running work alone.
            let boundary=request["action"]=="allow_write" || matches!(request["action"].as_str(),Some("prompt"|"incoming_prompt"))
                && lifecycle::steering::delivery(db,&b.task,key(request)?)?.as_deref()==Some(lifecycle::steering::BOUNDARY);
            if unsettled && !(closing && state.terminal()) && !boundary {return refuse("native_shutdown_unconfirmed");}
            if restart && !boundary {domain::native_continue_tx(db,&b.task)?;}
            if closing {
                let mut resolved=json!({"certified":state==TaskState::Completed});
                if unsettled {resolved["processes_unconfirmed"]=json!(true);}
                lifecycle::event_tx(db,&b.task,"OWNER_TASK_RESOLVED",resolved)?;
            }
            let version=domain::pause_version_tx(db,&b.task)?;
            let mut receipt=json!({"outcome":"recorded","native":true,"state":domain::task_tx(db,&b.task)?.0,"entityVersion":version.to_string()});
            if closing && unsettled {receipt["processesUnconfirmed"]=json!(true);}
            Ok(receipt)
        })
    })
}
/// A native engine permission request waiting for the owner (see
/// `permission_decisions`): Allow once / Deny, with who asks, what, where and why.
#[allow(clippy::too_many_arguments)]
fn permission_item(
    db: &Transaction<'_>,
    b: &Binding,
    requirement: i64,
    state: TaskState,
    id: &str,
    text: &str,
    status: &str,
    created: &str,
    answered: Option<String>,
    answer: Option<String>,
    assignment: Option<String>,
) -> CoreResult<Value> {
    use crate::permission_decisions as pd;
    let (details, reason) = pd::details_tx(db, &b.task, id)?;
    let version: i64 = db
        .query_row(
            "SELECT id FROM runtime_events WHERE task_id=? AND type='NATIVE_PERMISSION_DECISION_OPENED'
             AND json_extract(payload_json,'$.question_id')=? ORDER BY id DESC LIMIT 1",
            (&b.task, id),
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(b.execution_version);
    let shown = match status {
        "answered" => "answered",
        "open" => "pending",
        _ => "superseded",
    };
    let option = |label: &str| json!({"optionId":format!("wo_{}", &crate::digest(format!("{id}:{label}").as_bytes())[..40]),"label":label});
    let asked_by: Option<String> = match &assignment {
        Some(key) => db
            .query_row(
                "SELECT role_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=? AND role_id<>'orchestrator'",
                (&b.task, key),
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|role| role.strip_prefix("worker-").unwrap_or(&role).to_owned()),
        None => None,
    }
    .or_else(|| details["role"].as_str().map(|r| r.strip_prefix("worker-").unwrap_or(r).to_owned()));
    let mut value = json!({"decisionId":format!("wd_{id}"),"questionId":id,"kind":"permission","batchId":null,
        "executionId":Value::Null,"acpSessionId":b.session,"scope":assignment.unwrap_or("task".into()),"prompt":text,
        "options":[option(pd::ALLOW),option(pd::DENY)],"version":version.to_string(),"ownerEpoch":b.owner.epoch.to_string(),
        "requirementRevision":requirement.to_string(),"status":shown,
        "allowedOperations":if shown=="pending"&&state!=TaskState::Paused{vec!["decisions.answer"]}else{vec![]},
        "createdAt":created,"answeredAt":answered,"answerText":answer,
        "permission":{"operation":details["operation"],"paths":details["paths"],"reason":details["reason"],
            "reasonText":details["reason_text"],"command":details["command"],"cwd":details["cwd"],"why":details["why"],"access":details["access"],"rule":details["rule"]}});
    if let Some(role) = asked_by {
        value["askedBy"] = json!(role);
    }
    if shown == "superseded" {
        let ru = domain::task_language_tx(db, &b.task)? == "ru";
        value["headline"] = json!(pd::withdrawn_sentence(reason.as_deref(), status, ru));
    }
    Ok(value)
}
/// The files attached to each answered question, as `{name, mimeType, size, sha256}`
/// references, by question id. An answer names its question per file (`question_ids`).
/// An older record without names is attributed only when that is unambiguous: its files
/// came from the desktop (named, numbered from 0, no URI) and the answer recorded in the
/// same moment just before them is one question's. Such an attribution is marked `true`
/// so a batch member never receives it.
fn answer_attachments_tx(
    db: &Transaction<'_>,
    task: &str,
) -> CoreResult<std::collections::HashMap<String, (Vec<Value>, bool)>> {
    let rows = db
        .prepare(
            "SELECT id,payload_json,created_at FROM runtime_events
             WHERE task_id=? AND type='USER_ANSWER_ATTACHMENTS' ORDER BY id",
        )?
        .query_map([task], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let reference = |d: &Value| -> Option<Value> {
        (d["availability"] == "stored").then_some(())?;
        Some(
            json!({"name":d["name"].as_str()?,"mimeType":d["mime"].as_str()?,
            "size":d["size_bytes"].as_u64()?,"sha256":d["sha256"].as_str()?}),
        )
    };
    let mut out: std::collections::HashMap<String, (Vec<Value>, bool)> = Default::default();
    for (event, payload, at) in rows {
        let payload: Value = serde_json::from_str(&payload)?;
        let Some(files) = payload["attachments"].as_array() else {
            continue;
        };
        let (owners, legacy): (Vec<Option<String>>, bool) = match payload["question_ids"].as_array()
        {
            Some(ids) if ids.len() == files.len() => (
                ids.iter().map(|q| q.as_str().map(str::to_owned)).collect(),
                false,
            ),
            Some(_) => continue,
            None => {
                let desktop = files.iter().enumerate().all(|(i, f)| {
                    f["uri"].is_null()
                        && f["name"].is_string()
                        && f["source_ref"]
                            .as_str()
                            .and_then(|s| s.rsplit_once(':'))
                            .is_some_and(|(_, n)| n == i.to_string())
                });
                let answered: Option<(String, String)> = db
                    .query_row(
                        "SELECT json_extract(payload_json,'$.question_id'),created_at FROM runtime_events
                         WHERE task_id=? AND type='NATIVE_QUESTION_ANSWERED' AND id<? ORDER BY id DESC LIMIT 1",
                        (task, event),
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                let same_moment = |a: &str| {
                    let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
                    matches!((parse(a), parse(&at)), (Some(a), Some(b))
                        if (b - a).num_milliseconds() >= 0 && (b - a).num_milliseconds() <= 2000)
                };
                match answered {
                    Some((q, a)) if desktop && same_moment(&a) => {
                        (vec![Some(q); files.len()], true)
                    }
                    _ => continue,
                }
            }
        };
        for (file, owner) in files.iter().zip(owners) {
            if let (Some(q), Some(r)) = (owner, reference(file)) {
                let entry = out.entry(q).or_insert_with(|| (Vec::new(), legacy));
                entry.1 |= legacy;
                entry.0.push(r);
            }
        }
    }
    Ok(out)
}
fn decisions_tx(db: &Transaction<'_>, b: &Binding) -> CoreResult<Vec<Value>> {
    let waits = waits::projection_tx(db, &b.task)?;
    let rows=db.prepare("SELECT id,text,options_json,status,created_at,answered_at,answer_text,assignment_ref,dedup_key FROM pending_questions WHERE task_id=? ORDER BY rowid LIMIT 1001")?
        .query_map([&b.task],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?,
            r.get::<_,String>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<String>>(7)?,r.get::<_,String>(8)?)))?
        .collect::<Result<Vec<_>,_>>()?;
    if rows.len() > 1000 {
        return refuse("native_decision_history_limit");
    }
    let requirement: i64 = db.query_row(
        "SELECT coalesce((SELECT requirement_revision FROM task_orchestration WHERE task_id=?),0)",
        [&b.task],
        |r| r.get(0),
    )?;
    let state = domain::task_tx(db, &b.task)?.0;
    // A paused task keeps the coordinator question it was waiting on, shown but not
    // answerable until Resume; the question itself is not superseded by Pause.
    let paused_saved = if state == TaskState::Paused {
        domain::pause_saved_tx(db, &b.task)?
    } else {
        None
    };
    let attached = answer_attachments_tx(db, &b.task)?;
    rows.into_iter().map(|(id,text,options,status,created,answered,answer,assignment,dedup)| {
        if dedup.starts_with(crate::permission_decisions::DEDUP_PREFIX) {
            return permission_item(db,b,requirement,state,&id,&text,&status,&created,answered,answer,assignment);
        }
        let w=waits.iter().rev().find(|w|w["question_id"]==id||w["questions"].as_array().is_some_and(|qs|qs.iter().any(|q|q["question_id"]==id)));
        let resumable=match w {Some(w)=>waits::valid_tx(db,w)?,None=>false};
        let interrupted=w.is_some_and(|w| w["reason"]=="native_session_lost"
            && matches!(w["status"].as_str(),Some("open"|"ready"))
            && w["continuation_required"]==true) && !resumable;
        let paused_question=paused_saved.as_ref().is_some_and(|saved|saved["resumeState"]=="WAITING_USER"
            &&saved["questionIds"].as_array().is_some_and(|ids|ids.contains(&json!(id))));
        let node_question = dedup.starts_with("obstacle:");
        let valid=if node_question {
            lifecycle::obstacles::node_question_is_current(db,&b.task,&id)?
        } else {
            match w {Some(w)=>w["status"]=="open"&&(interrupted||resumable),None=>assignment.is_none()&&(state==TaskState::WaitingUser||paused_question)}
        };
        let status=match status.as_str() {"answered"=>"answered","open" if valid=>"pending",_=>"superseded"};
        let stored: Vec<Value> = serde_json::from_str(options.as_deref().unwrap_or("[]"))?;
        let options = stored.iter().enumerate().map(|(i, option)| {
            let label = waits::option_label(option).ok_or(CoreError::Refused("invalid_question_form"))?;
            let mut wire = json!({"optionId":format!("wo_{}", &crate::digest(format!("{id}:{i}:{label}").as_bytes())[..40]),"label":label});
            if let Some(detail) = option["detail"].as_str() {
                wire["detail"] = json!(detail);
            }
            if option["recommended"] == true {
                wire["recommended"] = json!(true);
            }
            Ok(wire)
        }).collect::<CoreResult<Vec<_>>>()?;
        let saved_batch: Option<String> = if w.is_none() {
            db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'
                AND EXISTS(SELECT 1 FROM json_each(json_extract(payload_json,'$.questions'))
                    WHERE json_extract(value,'$.question_id')=?) ORDER BY id DESC LIMIT 1",
                (&b.task,&id),|r|r.get(0)).optional()?
        } else { None };
        let saved_batch: Value = saved_batch.map(|s|serde_json::from_str(&s)).transpose()?.unwrap_or(Value::Null);
        let batch=w.and_then(|w|w.get("batch_id")).cloned().unwrap_or_else(||saved_batch["batch_id"].clone());
        // Each card binds its own question/wait, never unrelated turn or
        // activity events arriving between display and the answer transaction.
        let question_version: Option<i64> = if let Some(w)=w {
            w["event_id"].as_i64()
        } else if assignment.is_none() || node_question {
            db.query_row("SELECT id FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'
                AND (json_extract(payload_json,'$.question_id')=? OR EXISTS(
                    SELECT 1 FROM json_each(json_extract(payload_json,'$.questions'))
                    WHERE json_extract(value,'$.question_id')=?)) ORDER BY id DESC LIMIT 1",
                (&b.task,&id,&id),|r|r.get(0)).optional()?
        } else { None };
        let version=question_version.unwrap_or(b.execution_version).to_string();
        // A worker's question is answered in the task's Conversation like the coordinator's, so it names the role that asked:
        // its assignment is the worker's provisioning key. A coordinator question (no assignment) names none.
        let asked_by: Option<String> = match &assignment {
            Some(key) => db.query_row("SELECT role_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=? AND role_id<>'orchestrator'",
                (&b.task,key),|r|r.get::<_,String>(0)).optional()?.map(|role| role.strip_prefix("worker-").unwrap_or(&role).to_owned()),
            None => None,
        };
        let mut value=json!({"decisionId":format!("wd_{id}"),"questionId":id,"kind":if batch.is_null(){"question"}else{"question_batch"},
            "batchId":batch,"executionId":Value::Null,"acpSessionId":b.session,"scope":assignment.unwrap_or("task".into()),"prompt":text,
            "options":options,"version":version,"ownerEpoch":b.owner.epoch.to_string(),"requirementRevision":requirement.to_string(),
            "status":status,"allowedOperations":if status=="pending"&&state!=TaskState::Paused&&!interrupted{vec!["decisions.answer"]}else{vec![]},
            "createdAt":created,"answeredAt":answered,"answerText":answer});
        if let Some(role)=asked_by {value["askedBy"]=json!(role);}
        // A batch member's optional short label, kept with the question it names.
        let topic=w.unwrap_or(&saved_batch)["questions"].as_array().and_then(|q|q.iter()
            .find(|q|q["question_id"]==json!(id))).and_then(|q|q["topic"].as_str()).map(str::to_owned);
        if let Some(topic)=topic {value["topic"]=json!(topic);}
        if !batch.is_null() {value["questionIds"]=json!(w.unwrap_or(&saved_batch)["questions"].as_array().unwrap().iter().map(|q|q["question_id"].clone()).collect::<Vec<_>>());}
        // The runtime's own stop question carries what it explained beside its complete
        // text: each answer's consequence, the recommended answer and what the desktop
        // can open (the blocking task, the review report, the denied path).
        if w.is_none() {
            let stop: Option<String> = db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'
                AND json_extract(payload_json,'$.question_id')=? AND json_extract(payload_json,'$.runtime_stop')=1 ORDER BY id DESC LIMIT 1",
                (&b.task,&id),|r|r.get(0)).optional()?;
            if let Some(stop) = stop.map(|s| serde_json::from_str::<Value>(&s)).transpose()? {
                value["runtimeStop"]=json!(true);
                if let (Some(options),Some(details))=(value["options"].as_array_mut(),stop["option_details"].as_array()) {
                    for (o,d) in options.iter_mut().zip(details) {
                        if o["label"]==d["label"] {o["detail"]=d["detail"].clone(); o["action"]=d["action"].clone();}
                    }
                }
                if let Some(o)=stop["recommended"].as_u64().and_then(|i|value["options"].get_mut(i as usize)) {o["recommended"]=json!(true);}
                if stop["targets"].is_array() {value["targets"]=stop["targets"].clone();}
                if stop["headline"].is_string() {value["headline"]=stop["headline"].clone();}
            }
            // The runtime's stop question and a skill proposal act on the chosen option
            // itself, so an option answer to them takes no added words.
            let proposal: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'
                AND json_extract(payload_json,'$.question_id')=? AND json_extract(payload_json,'$.skill_proposal') IS NOT NULL)",
                (&b.task,&id),|r|r.get(0))?;
            if proposal || value["runtimeStop"]==true {value["optionsOnly"]=json!(true);}
        }
        // The files the owner attached to this answer. One recorded before answers named
        // their files is shown only for a lone question, never guessed within a batch.
        if status=="answered" {
            if let Some((files,legacy))=attached.get(&id) {
                if !*legacy || batch.is_null() {value["answerAttachments"]=json!(files);}
            }
        }
        // A question the runtime replaced with a later one names it.
        if status=="superseded" {
            let by: Option<String> = db.query_row("SELECT json_extract(payload_json,'$.question_id') FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'
                AND EXISTS(SELECT 1 FROM json_each(json_extract(payload_json,'$.supersedes')) WHERE value=?) ORDER BY id LIMIT 1",
                (&b.task,&id),|r|r.get(0)).optional()?;
            if let Some(by)=by {value["replacedBy"]=json!(format!("wd_{by}"));}
        }
        Ok(value)
    }).collect()
}
pub fn decisions(store: &Store, project: &str, actor: &str, request: &Value) -> CoreResult<Value> {
    store.write(|db|{
        let target=request["target"].as_str().ok_or(CoreError::Refused("invalid_target"))?;
        let execution=if request["operation"]=="decisions.get" {
            db.query_row("SELECT e.execution_id FROM pending_questions q JOIN workspace_executions e ON e.runtime_task_id=q.task_id WHERE q.id=?",
                [target.strip_prefix("wd_").ok_or(CoreError::Refused("not_found"))?],|r|r.get::<_,String>(0)).optional()?.ok_or(CoreError::Refused("not_found"))?
        }else{target.to_owned()};
        let b=read_binding(db,&store.key().root().to_string_lossy(),project,actor,&execution)?;
        let native=binding(db,&store.key().root().to_string_lossy(),project,actor,&execution).is_ok();
        let mut items=decisions_tx(db,&b)?;
        // A task the Rust host does not own is shown, never answered here.
        if !native { for item in &mut items { item["allowedOperations"]=json!([]); } }
        for item in &mut items {item["executionId"]=json!(execution);}
        if request["operation"]=="decisions.get" {return items.into_iter().find(|i|i["decisionId"]==target).ok_or(CoreError::Refused("not_found"));}
        let limit=request["limit"].as_u64().unwrap_or(100) as usize;
        let token=crate::digest(format!("{project}:{execution}:{}:{limit}",b.execution_version).as_bytes());
        let after=if let Some(c)=request["cursor"].as_str() {c.strip_prefix(&format!("{token}.")).and_then(|s|s.parse::<usize>().ok()).ok_or(CoreError::Refused("invalid_cursor"))?}else{0};
        if after>items.len(){return refuse("invalid_cursor");}
        let next=(after+limit).min(items.len());
        let watermark:i64=db.query_row("SELECT coalesce((SELECT high_watermark FROM workspace_outbox_state WHERE project_id=?),0)",[project],|r|r.get(0))?;
        let result=json!({"items":items[after..next],"nextCursor":if next<items.len(){Some(format!("{token}.{next}"))}else{None},"highWatermark":watermark.to_string()});
        workspace_protocol::validate("DecisionPage",&result).map_err(|_|CoreError::Refused("native_decision_page_unavailable"))?;
        Ok(result)
    })
}
pub fn answer(store: &Store, project: &str, actor: &str, command: &Value) -> CoreResult<Value> {
    answer_from_session(store, project, actor, command, None)
}
/// The answering transport supplies its real session separately from client payload.
pub fn answer_from_session(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    session: Option<&str>,
) -> CoreResult<Value> {
    answer_with_input(store, project, actor, command, session, None)
}
/// An answer whose attached files were captured by the transport. They become the
/// task's next source revision in the answer's own transaction, so the answered turn
/// carries them and a replay of the same answer retains nothing twice. The command
/// must carry `payload.capture`, the capture's fingerprint.
pub fn answer_with_input(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    session: Option<&str>,
    capture: Option<&crate::input::Capture>,
) -> CoreResult<Value> {
    let capture = capture.filter(|c| !c.blocks.is_empty());
    if command["payload"].get("capture") != capture.map(|c| c.fingerprint()).as_ref() {
        return refuse("invalid_input_capture");
    }
    let outcome = answer_in_transaction(store, project, actor, command, session, capture);
    // A superseded refusal rolls back answer_tx's authority-loss cancellation.
    // Commit it separately so the question cannot stay pending but unanswerable.
    if matches!(&outcome, Err(CoreError::Refused("superseded"))) {
        let _ = store.write(|db| {
            let execution = command["payload"]["executionId"]
                .as_str()
                .ok_or(CoreError::Refused("invalid_execution"))?;
            let b = binding(
                db,
                &store.key().root().to_string_lossy(),
                project,
                actor,
                execution,
            )?;
            waits::cancel_unauthorized_tx(db, &b.task)
        });
    }
    outcome
}
fn answer_in_transaction(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    session: Option<&str>,
    capture: Option<&crate::input::Capture>,
) -> CoreResult<Value> {
    store.write(|db| {
        let p = &command["payload"];
        let execution = p["executionId"]
            .as_str()
            .ok_or(CoreError::Refused("invalid_execution"))?;
        let b = binding(
            db,
            &store.key().root().to_string_lossy(),
            project,
            actor,
            execution,
        )?;
        if let Some(session) = session {
            let bound: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM acp_host_sessions WHERE session_id=? AND task_id=?)",
                (session, &b.task),
                |r| r.get(0),
            )?;
            if !bound {
                return refuse("question_answer_session_mismatch");
            }
        }
        lifecycle::request_tx(
            db,
            &b.task,
            actor,
            key(command)?,
            "native_desktop_answer",
            command,
            || {
                if command["ownerEpoch"] != b.owner.epoch.to_string() {
                    return refuse("stale_epoch");
                }
                if p["acpSessionId"] != b.session {
                    return refuse("invalid_request");
                }
                if domain::task_tx(db, &b.task)?.0 == TaskState::Paused {
                    return refuse("task_paused");
                }
                let decisions = decisions_tx(db, &b)?;
                let members: Vec<_> = decisions
                    .iter()
                    .filter(|d| {
                        if p["batchId"].is_null() {
                            d["batchId"].is_null() && d["decisionId"] == command["target"]
                        } else {
                            d["batchId"] == p["batchId"] && command["target"] == p["batchId"]
                        }
                    })
                    .collect();
                let answers = p["answers"]
                    .as_array()
                    .ok_or(CoreError::Refused("invalid_request"))?;
                if members.is_empty() || members.len() != answers.len() {
                    return refuse("invalid_request");
                }
                let mut converted = Vec::new();
                for d in members {
                    let matching: Vec<_> = answers
                        .iter()
                        .filter(|a| {
                            a["decisionId"] == d["decisionId"] && a["questionId"] == d["questionId"]
                        })
                        .collect();
                    if matching.len() != 1
                        || d["status"] != "pending"
                        || !d["allowedOperations"]
                            .as_array()
                            .is_some_and(|a| a.contains(&json!("decisions.answer")))
                    {
                        return refuse("superseded");
                    }
                    let a = matching[0];
                    if a.get("attachments")
                        .is_some_and(|a| a.as_array().is_none_or(|a| !a.is_empty()))
                    {
                        return refuse("native_attachments_unavailable");
                    }
                    if a["expectedVersion"] != d["version"]
                        || command["expectedVersion"] != d["version"]
                    {
                        return refuse("stale_version");
                    }
                    let typed = a["text"].as_str().unwrap_or("");
                    // A chosen option's text is its label, optionally followed by a blank
                    // line and the owner's own words; the answer recorded is exactly that.
                    let (text, comment) = if a["optionId"].is_null() {
                        (typed.to_owned(), false)
                    } else {
                        let label = d["options"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|o| o["optionId"] == a["optionId"])
                            .and_then(|o| o["label"].as_str())
                            .ok_or(CoreError::Refused("invalid_request"))?;
                        if typed.is_empty() || typed == label {
                            (label.to_owned(), false)
                        } else {
                            let words = typed
                                .strip_prefix(label)
                                .and_then(|rest| rest.strip_prefix("\n\n"))
                                .map(str::trim)
                                .filter(|words| !words.is_empty())
                                .ok_or(CoreError::Refused("invalid_request"))?;
                            (format!("{label}\n\n{words}"), true)
                        }
                    };
                    if text.trim().is_empty() {
                        return refuse("invalid_request");
                    }
                    // A permission decision takes one of its two options, never free text.
                    let permission = d["kind"] == "permission";
                    if permission && (a["optionId"].is_null() || comment) {
                        return refuse("invalid_request");
                    }
                    // The runtime's stop question and a skill proposal act on the chosen
                    // option itself, so their option takes no added words.
                    if comment && d["optionsOnly"] == true {
                        return refuse("option_comment_unavailable");
                    }
                    converted.push((
                        d["questionId"].as_str().unwrap().to_owned(),
                        text,
                        permission,
                    ));
                }
                let answered_text = converted
                    .iter()
                    .map(|(_, t, _)| t.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                // Which answered question each captured file came with, when the
                // transport said so: one question id per file, each answered here.
                let owners = match (p.get("captureQuestions"), capture) {
                    (None, _) => None,
                    (Some(Value::Array(ids)), Some(c))
                        if ids.len() == c.blocks.len()
                            && ids
                                .iter()
                                .all(|q| converted.iter().any(|(id, _, _)| q == id)) =>
                    {
                        Some(ids.clone())
                    }
                    _ => return refuse("invalid_input_capture"),
                };
                for (q, text, permission) in converted {
                    if permission {
                        if !crate::permission_decisions::answer_tx(
                            db,
                            &b.task,
                            &q,
                            text == crate::permission_decisions::ALLOW,
                        )? {
                            return refuse("superseded");
                        }
                        continue;
                    }
                    if !waits::answer_tx(
                        db,
                        &b.task,
                        &q,
                        &text,
                        "text",
                        Some(session.unwrap_or(&b.session)),
                    )? {
                        return refuse("superseded");
                    }
                }
                if let Some(capture) = capture {
                    let descriptors = capture.persist(
                        store,
                        db,
                        &b.task,
                        session.unwrap_or(&b.session),
                        &answered_text,
                    )?;
                    let mut payload = json!({"attachments":descriptors});
                    if let Some(owners) = owners {
                        payload["question_ids"] = Value::Array(owners);
                    }
                    lifecycle::event_tx(db, &b.task, "USER_ANSWER_ATTACHMENTS", payload)?;
                }
                Ok(super::receipt(
                    command,
                    "recorded",
                    command["target"].as_str().unwrap(),
                    b.execution_version,
                ))
            },
        )
    })
}
