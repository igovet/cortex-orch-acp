//! Owner messages to a working task: recorded once, never interrupting running work,
//! applied at the coordinator's next boundary and delivered to it exactly once.
//! Owner `deny-write:` rules are recorded with the message and enforced.
mod support;
use cortex_core::{
    lifecycle::{completion, domain, plan, steering},
    workspace::{control, create_draft, start_native_input_with_policy, NativeExecutionPolicy},
    CoreResult, Store,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::Project;

fn setup() -> (Project, Store) {
    let p = Project::new();
    let s = p.open();
    s.write(|db| {
        db.execute(
            "INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)",
            [p.0.to_str().unwrap()],
        )?;
        Ok(())
    })
    .unwrap();
    (p, s)
}
/// A started native task: (runtime task, execution).
fn started(s: &Store, n: usize) -> (String, String) {
    started_with(s, n, "Steering fixture")
}
fn started_with(s: &Store, n: usize, description: &str) -> (String, String) {
    started_with_policy(s, n, description, None)
}
fn started_with_policy(
    s: &Store,
    n: usize,
    description: &str,
    policy: Option<NativeExecutionPolicy>,
) -> (String, String) {
    let create = json!({"protocol":{"major":1,"minor":8},"requestId":format!("create-{n}"),"projectId":"p","actorId":"owner",
        "operation":"tasks.create","target":"p","expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":description,"checkout":{"kind":"main"}}});
    let draft = create_draft(s, "p", "owner", &create).unwrap().receipt["target"]
        .as_str()
        .unwrap()
        .to_owned();
    let start = json!({"protocol":{"major":1,"minor":8},"requestId":format!("start-{n}"),"projectId":"p","actorId":"owner",
        "operation":"tasks.start","target":draft,"expectedVersion":"1","ownerEpoch":null,"payload":{}});
    let started =
        start_native_input_with_policy(s, "p", "owner", &start, None, None, policy).unwrap();
    (
        started["task"].as_str().unwrap().to_owned(),
        started["receipt"]["target"].as_str().unwrap().to_owned(),
    )
}
/// Admit and finish one owner message as the service does; `restart` is what an
/// older caller would pass, and Core never restarts a boundary message.
fn say(s: &Store, execution: &str, key: &str, text: &str) -> CoreResult<Value> {
    let request = control::canonical_request(
        json!({"projectId":"p","executionId":execution,"requestId":key,"action":"prompt","text":text}),
        None,
    )?;
    control::admit_input(s, "p", "owner", &request, None)?;
    control::finish(s, "p", "owner", &request, true)
}
fn one(s: &Store, sql: &str) -> String {
    s.read(|db| {
        Ok(db
            .query_row(sql, [], |r| r.get::<_, rusqlite::types::Value>(0))
            .map(|v| match v {
                rusqlite::types::Value::Text(t) => t,
                rusqlite::types::Value::Integer(i) => i.to_string(),
                rusqlite::types::Value::Null => String::new(),
                other => format!("{other:?}"),
            })?)
    })
    .unwrap()
}
/// The task in `state` with one running worker node holding a live reservation.
fn working(s: &Store, task: &str, state: &str, stage: &str) {
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state=?,revision=1 WHERE task_id=?", (state, task))?;
        db.execute("INSERT INTO runtime_nodes VALUES (?,1,'n','worker-backend-dev','Work','[]','[\"src\"]',?,'running','a-n',NULL)",
            (task, stage))?;
        db.execute("INSERT INTO capacity_reservations VALUES ('r-n','fixture',?,'a-n','attempt-n','worker','reserved','0','0',NULL)",
            [task])?;
        Ok(())
    })
    .unwrap();
}
fn count(s: &Store, event: &str) -> i64 {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE type=?",
            [event],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

fn active_plan_args(s: &Store) -> Value {
    let raw = one(s, "SELECT plan_json FROM runtime_tasks");
    let stored: Value = serde_json::from_str(&raw).unwrap();
    json!({
        "expected_plan": format!("{:x}", Sha256::digest(raw.as_bytes())),
        "source_request": "owner-steer",
        "plan": plan::steerable_plan(&stored).unwrap(),
    })
}

fn begin_plan(s: &Store, task: &str, proposed: &Value) {
    s.write(|db| {
        plan::accept_tx(db, task, "coordinator", "plan", &support::with_goal(proposed))?;
        db.execute(
            "UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id=?",
            [task],
        )?;
        Ok(())
    })
    .unwrap();
}

fn node(id: &str, role: &str, deps: &[&str]) -> Value {
    json!({"id":id,"role":role,"objective":format!("Work on {id}"),
        "dependencies":deps,"scope":["src"],"model":"auto"})
}

fn assert_amend_refused(s: &Store, task: &str, args: &Value, code: &str) {
    let snapshot = || {
        (
            one(s, "SELECT plan_json FROM runtime_tasks"),
            one(s, "SELECT json_group_array(json_array(node_id,role,objective,dependencies_json,scope_json,status,assignment_ref,result_json)) FROM (SELECT * FROM runtime_nodes ORDER BY node_id)"),
            count(s, "PLAN_AMENDED"),
        )
    };
    let before = snapshot();
    assert_eq!(
        s.write(|db| plan::amend_active_tx(db, task, args))
            .unwrap_err()
            .code(),
        code
    );
    assert_eq!(snapshot(), before, "refusal must leave no partial amendment");
}

#[test]
fn active_plan_replaces_only_unclaimed_work_and_locks_verification_routing_and_scope() {
    let (_p, s) = setup();
    let (task, execution) = started(&s, 1);
    let proposed = json!({
        "tasks":[node("claimed", "backend-dev", &[]), node("drop", "backend-dev", &["claimed"]),
                 node("pending", "backend-dev", &["claimed"]), node("previouslyClaimed", "backend-dev", &["claimed"])],
        "verification":[["cargo","test"]],
    });
    begin_plan(&s, &task, &proposed);
    s.write(|db| {
        db.execute(
            "UPDATE runtime_nodes SET status='running',assignment_ref='a-claimed' WHERE task_id=? AND node_id='claimed'",
            [&task],
        )?;
        // A previously claimed node may look pending again; the retained event still fences it.
        cortex_core::lifecycle::event_tx(db, &task, "ASSIGNMENT_CREATED",
            json!({"node":"previouslyClaimed","revision":1}))?;
        Ok(())
    }).unwrap();
    say(&s, &execution, "deny", "deny-write: private").unwrap();

    let mut next = active_plan_args(&s);
    let tasks = next["plan"]["tasks"].as_array_mut().unwrap();
    tasks.remove(1);
    tasks[1]["objective"] = json!("Refined pending objective");
    tasks.push(node("added", "backend-dev", &["pending"]));
    for index in [0, 2] {
        let mut args = next.clone();
        args["plan"]["tasks"][index]["objective"] = json!("Attempt to change claimed work");
        assert_amend_refused(&s, &task, &args, "active_plan_node_already_claimed");
    }
    let mut args = next.clone();
    args["plan"]["verification"] = json!([["cargo", "check"]]);
    assert_amend_refused(&s, &task, &args, "active_plan_verification_locked");

    let mut args = next.clone();
    args["plan"]["tasks"][1]["complexity"] = json!("high");
    assert_amend_refused(&s, &task, &args, "active_plan_routing_locked");

    let mut args = next.clone();
    args["plan"]["tasks"][3]["scope"] = json!(["private/child"]);
    assert_amend_refused(&s, &task, &args, "write_denied");

    let receipt = s.write(|db| plan::amend_active_tx(db, &task, &next)).unwrap();
    assert_eq!(receipt["revision"], 1);
    assert_eq!(receipt["changed"], true);
    assert_eq!(active_plan_args(&s)["plan"], next["plan"]);
    assert_eq!(active_plan_args(&s)["expected_plan"], receipt["plan_digest"]);
    assert_eq!(
        one(&s, "SELECT group_concat(node_id || ':' || status, ',') FROM (SELECT * FROM runtime_nodes ORDER BY node_id)"),
        "added:pending,claimed:running,pending:pending,previouslyClaimed:pending"
    );
    assert_eq!(one(&s, "SELECT assignment_ref FROM runtime_nodes WHERE node_id='claimed'"), "a-claimed");
    assert_eq!(count(&s, "PLAN_AMENDED"), 1);
    assert_amend_refused(&s, &task, &next, "active_plan_changed");
}

#[test]
fn active_sprint_planning_amends_read_only_work_with_empty_verification() {
    let (_p, s) = setup();
    let (task, _) = started_with_policy(
        &s,
        1,
        "Plan the sprint",
        Some(NativeExecutionPolicy::SprintPlanning),
    );
    begin_plan(&s, &task, &json!({"tasks":[node("study", "explorer", &[])],"verification":[]}));
    let mut args = active_plan_args(&s);
    args["plan"]["tasks"][0]["objective"] = json!("Include the active sprint");
    let mut writer = args.clone();
    writer["plan"]["tasks"][0]["role"] = json!("backend-dev");
    assert_amend_refused(&s, &task, &writer, "read_only_task_writer_refused");
    let receipt = s.write(|db| plan::amend_active_tx(db, &task, &args)).unwrap();
    assert_eq!(receipt["changed"], true);
    assert_eq!(active_plan_args(&s)["plan"], args["plan"]);
}

#[test]
fn active_consultation_plan_preserves_runtime_node_and_gates_new_work() {
    let (_p, s) = setup();
    let (task, _) = started(&s, 1);
    // Record an escalation through its retained input and let plan intake insert the gate.
    s.write(|db| {
        cortex_core::lifecycle::event_tx(db, &task, "REPLAN_REQUEST_ACCEPTED",
            json!({"operation":"escalation_request","revision":0,"proposal":{"reason":"Independent advice"}}))?;
        Ok(())
    }).unwrap();
    let proposed = json!({"tasks":[node("study", "explorer", &[]), node("followup", "explorer", &["study"])],
        "verification":[["cargo","test"]]});
    begin_plan(&s, &task, &proposed);
    s.write(|db| {
        db.execute("UPDATE runtime_nodes SET status='running',assignment_ref='a-consult' WHERE task_id=? AND node_id='_consultation'", [&task])?;
        Ok(())
    }).unwrap();
    let before: Value = serde_json::from_str(&one(&s, "SELECT plan_json FROM runtime_tasks")).unwrap();
    assert_eq!(before["tasks"][0]["id"], "_consultation");
    let mut args = active_plan_args(&s);
    assert_eq!(args["plan"], support::with_goal(&proposed));
    let unchanged = s.write(|db| plan::amend_active_tx(db, &task, &args)).unwrap();
    assert_eq!(unchanged["changed"], false);
    assert_eq!(count(&s, "PLAN_AMENDED"), 0);

    let mut forged = args.clone();
    forged["plan"]["tasks"][0]["id"] = json!("_consultation");
    assert_amend_refused(&s, &task, &forged, "duplicate_or_invalid_node");
    forged = args.clone();
    forged["plan"]["tasks"][0]["role"] = json!("senior-consultant");
    assert_amend_refused(&s, &task, &forged, "runtime_reserved_role");
    forged = args.clone();
    forged["plan"]["tasks"][0]["dependencies"] = json!(["_consultation"]);
    assert_amend_refused(&s, &task, &forged, "unknown_dependency");

    args["plan"]["tasks"][0]["objective"] = json!("Study the guidance");
    args["plan"]["tasks"].as_array_mut().unwrap().push(node("new", "explorer", &[]));
    let receipt = s.write(|db| plan::amend_active_tx(db, &task, &args)).unwrap();
    assert_eq!(receipt["changed"], true);
    assert_eq!(active_plan_args(&s)["plan"], args["plan"]);
    let after: Value = serde_json::from_str(&one(&s, "SELECT plan_json FROM runtime_tasks")).unwrap();
    assert_eq!(after["tasks"][0], before["tasks"][0]);
    assert_eq!(after["tasks"][1]["dependencies"], json!(["_consultation"]));
    assert_eq!(after["tasks"][2], before["tasks"][2]);
    assert_eq!(after["tasks"][3]["dependencies"], json!(["_consultation"]));
    assert_eq!(one(&s, "SELECT status || ':' || assignment_ref FROM runtime_nodes WHERE node_id='_consultation'"), "running:a-consult");
    assert_eq!(one(&s, "SELECT dependencies_json FROM runtime_nodes WHERE node_id='new'"), r#"["_consultation"]"#);
    assert_eq!(count(&s, "PLAN_AMENDED"), 1);
}

#[test]
fn an_unobserved_active_handoff_survives_and_only_its_own_committed_reply_can_confirm() {
    use cortex_core::lifecycle::event_tx;
    let (_p, s) = setup();
    let (task, execution) = started(&s, 1);
    working(&s, &task, "EXECUTING", "execution");
    say(&s, &execution, "one", "First guidance").unwrap();
    say(&s, &execution, "two", "Second guidance").unwrap();
    let abandoned = s.write(|db| {
        let watermark = steering::active_watermark(db, &task)?.unwrap();
        let (handoff, handed) = steering::hand_active_tx(db, &task, "lost", 0, watermark)?;
        assert_eq!(handed.iter().map(|m| &m["request"]).collect::<Vec<_>>(), vec![&json!("one"), &json!("two")]);
        assert_eq!(steering::active_watermark(db, &task)?, None);
        assert_eq!(steering::confirm_tx(db, &task)?, 0);
        assert_eq!(completion::pending_input_tx(db, &task)?.len(), 2);
        Ok(handoff)
    }).unwrap();
    say(&s, &execution, "three", "Third guidance").unwrap();
    let (handoff, chunk) = s.write(|db| {
        let channel = "current";
        db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,attempt_id,owner_epoch,issued_at)
            VALUES (?,'x',?,'thread','coordinator','fixture-attempt',1,'now')", (channel, &task))?;
        db.execute("INSERT INTO acp_host_sessions(session_id,project_root,task_id,client_info,client_capabilities,mcp_servers,created_at)
            VALUES ('display','/fixture',?,'{\"name\":\"cortex-native\"}','{}','[]','now')", [&task])?;
        event_tx(db, &task, "NATIVE_ACTIVE_STEER_CHANNEL", json!({"channel_id":channel}))?;
        let through = steering::active_watermark(db, &task)?.unwrap();
        let (handoff, handed) = steering::hand_active_tx(db, &task, channel, 0, through)?;
        assert_eq!(handed.iter().map(|m| &m["request"]).collect::<Vec<_>>(), vec![&json!("three")]);
        db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,message_id,created_at)
            VALUES ('display',0,'agent_message','Visible reply','fixture-attempt:reply','now')", [])?;
        let chunk = event_tx(db, &task, "NATIVE_STEERING_REPLY_CHUNK",
            json!({"channel_id":channel,"handoff":handoff,"display_session":"display","seq":0}))?;
        Ok((handoff, chunk))
    }).unwrap();
    say(&s, &execution, "four", "After the handoff").unwrap();
    s.write(|db| {
        let channel = "current";
        for (stop, linked_handoff, receipt) in [
            ("cancelled", handoff, Some(chunk)),
            ("max_tokens", handoff, Some(chunk)),
            ("max_turn_requests", handoff, Some(chunk)),
            ("end_turn", abandoned, Some(chunk)),
            ("end_turn", handoff, None),
            ("end_turn", handoff, Some(chunk + 10_000)),
        ] {
            let reply = event_tx(db, &task, "NATIVE_TURN_SETTLED",
                json!({"channel_id":channel,"stop_reason":stop,"handoff":linked_handoff,"reply_chunk":receipt}))?;
            assert!(!steering::confirm_active_tx(db, &task, channel, reply)?);
        }
        assert_eq!(completion::pending_input_tx(db, &task)?.len(), 4);
        let reply = event_tx(db, &task, "NATIVE_TURN_SETTLED",
            json!({"channel_id":channel,"stop_reason":"end_turn","handoff":handoff,"reply_chunk":chunk}))?;
        assert!(!steering::confirm_active_tx(db, &task, "lost", reply)?);
        assert!(steering::confirm_active_tx(db, &task, channel, reply)?);
        assert!(!steering::confirm_active_tx(db, &task, channel, reply)?);
        assert_eq!(steering::pending(db, &task)?.iter().map(|m| m["request"].as_str().unwrap().to_owned()).collect::<Vec<_>>(),
            vec!["one", "two", "four"]);
        assert_eq!(completion::pending_input_tx(db, &task)?.len(), 3);
        Ok(())
    }).unwrap();
    assert_eq!(count(&s, "STEERING_DELIVERED"), 1);
    assert_eq!(count(&s, "STEERING_APPLIED"), 1);
    assert_eq!(one(&s, "SELECT state FROM runtime_tasks"), "EXECUTING");
    assert_eq!(one(&s, "SELECT status FROM runtime_nodes"), "running");
}

#[test]
fn a_message_during_work_review_or_verification_interrupts_nothing() {
    for (n, (state, stage)) in [
        ("EXECUTING", "execution"),
        ("REVIEWING", "review"),
        ("VERIFYING", "execution"),
    ]
    .into_iter()
    .enumerate()
    {
        let (_p, s) = setup();
        let (task, execution) = started(&s, n);
        working(&s, &task, state, stage);
        // Recorded although a worker process is live: nothing needs to settle first.
        let receipt = say(&s, &execution, "steer", "Use the other output").unwrap();
        assert_eq!(receipt["outcome"], "recorded");
        assert_eq!(receipt["state"], state, "{state}: the task state changed");
        assert_eq!(one(&s, "SELECT status FROM runtime_nodes"), "running");
        assert_eq!(
            one(&s, "SELECT state FROM capacity_reservations"),
            "reserved"
        );
        assert_eq!(count(&s, "TURN_INTERRUPTED"), 0, "{state}");
        assert_eq!(count(&s, "LIVE_ASSIGNMENTS_CANCELLED"), 0, "{state}");
        assert_eq!(one(&s, "SELECT json_extract(payload_json,'$.delivery') FROM runtime_events WHERE type='USER_STEERING'"), "boundary");
        // Not a boundary while the node runs: nothing is applied yet.
        assert!(!s.write(|db| steering::apply_pending_tx(db, &task)).unwrap());
        assert_eq!(s.read(|db| steering::pending(db, &task)).unwrap().len(), 1);
        // The same request replays its receipt and is never recorded twice.
        let request = control::canonical_request(
            json!({"projectId":"p","executionId":execution,"requestId":"steer","action":"prompt","text":"Use the other output"}),
            None,
        )
        .unwrap();
        assert_eq!(
            control::replay(&s, "p", "owner", &request).unwrap(),
            Some(receipt)
        );
        assert_eq!(
            say(&s, &execution, "steer", "Use the other output")
                .unwrap_err()
                .code(),
            "native_control_already_recorded"
        );
        assert_eq!(count(&s, "USER_STEERING"), 1);
    }
}

#[test]
fn pending_messages_apply_after_the_work_and_reach_the_coordinator_once_in_order_across_restarts() {
    let (p, s) = setup();
    let (task, execution) = started(&s, 0);
    working(&s, &task, "EXECUTING", "execution");
    say(&s, &execution, "first", "First guidance").unwrap();
    say(&s, &execution, "second", "Second guidance").unwrap();
    // The worker, review and verification finish; the revision reaches reconciliation.
    s.write(|db| {
        db.execute("UPDATE runtime_nodes SET status='completed'", [])?;
        db.execute("UPDATE capacity_reservations SET state='released'", [])?;
        db.execute(
            "UPDATE runtime_tasks SET state='RECONCILING',synthesis='Unconfirmed.' WHERE task_id=?",
            [&task],
        )?;
        Ok(())
    })
    .unwrap();
    // A completion never passes a message still waiting for the coordinator.
    assert_eq!(
        s.write(|db| completion::complete_tx(db, &task, &completion::EvidenceSnapshot::default()))
            .unwrap_err()
            .code(),
        "completion_pending_input"
    );
    // Service restart between record and delivery: the store alone carries them.
    drop(s);
    let s = p.open();
    let delivered = s.write(|db| steering::deliver_tx(db, &task)).unwrap();
    let texts: Vec<_> = delivered
        .iter()
        .map(|m| m["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["First guidance", "Second guidance"]);
    let prompt = steering::guidance_prompt(&delivered);
    assert!(
        prompt.contains("1. First guidance") && prompt.contains("2. Second guidance"),
        "{prompt}"
    );
    assert_eq!(
        one(
            &s,
            "SELECT state||':'||coalesce(synthesis,'') FROM runtime_tasks"
        ),
        "PLANNING:"
    );
    assert_eq!(count(&s, "STEERING_APPLIED"), 2);
    assert_eq!(
        (
            count(&s, "STEERING_HANDED"),
            count(&s, "STEERING_DELIVERED")
        ),
        (2, 0)
    );
    // The service dies after the hand-over, before the engine replied: the next
    // coordinator turn is handed both again, marked repeated, and nothing is lost.
    drop(s);
    let s = p.open();
    let again = s.write(|db| steering::deliver_tx(db, &task)).unwrap();
    assert_eq!(again.len(), 2);
    assert!(again.iter().all(|m| m["repeated"] == true));
    assert!(steering::guidance_prompt(&again).contains("1. (repeated) First guidance"));
    // That turn's prompt reply is observed: both are delivered, once each.
    confirmed_prompt(&s, &task);
    assert_eq!(s.write(|db| steering::confirm_tx(db, &task)).unwrap(), 2);
    assert_eq!(count(&s, "STEERING_DELIVERED"), 2);
    let states = s.read(|db| steering::message_states(db, &task)).unwrap();
    assert!(
        states
            .iter()
            .all(|m| m["delivery"] == "delivered" && m["deliveredAt"].is_string()),
        "{states:?}"
    );
    // Another restart: a delivered message is never delivered again.
    drop(s);
    let s = p.open();
    assert!(s
        .write(|db| steering::deliver_tx(db, &task))
        .unwrap()
        .is_empty());
    assert_eq!(count(&s, "STEERING_DELIVERED"), 2);
}
/// A coordinator prompt of `task` whose engine reply was observed now.
fn confirmed_prompt(s: &Store, task: &str) {
    s.write(|db| {
        let now = cortex_core::now();
        db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,owner_epoch,issued_at)
            VALUES ('ch_coordinator','x',?,'thread','coordinator',1,?)", (task, &now))?;
        cortex_core::lifecycle::event_tx(db, task, "NATIVE_TURN_SETTLED", json!({"channel_id":"ch_coordinator","stop_reason":"end_turn"}))?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_message_to_a_task_whose_worker_asked_the_owner_waits_for_the_boundary() {
    let (_p, s) = setup();
    let (task, execution) = started(&s, 0);
    working(&s, &task, "EXECUTING", "execution");
    // The worker's own question (an assignment wait) is open.
    s.write(|db| {
        db.execute("INSERT INTO pending_questions(id,task_id,assignment_ref,dedup_key,text,options_json,status,created_at)
            VALUES ('wq',?,'a-n','wq','Which output?','[\"A\",\"B\"]','open','0')", [&task])?;
        Ok(())
    })
    .unwrap();
    let receipt = say(&s, &execution, "steer", "Prefer the short output").unwrap();
    assert_eq!(receipt["state"], "EXECUTING");
    assert_eq!(count(&s, "TURN_INTERRUPTED"), 0);
    assert_eq!(
        one(&s, "SELECT status FROM pending_questions WHERE id='wq'"),
        "open"
    );
    assert_eq!(one(&s, "SELECT status FROM runtime_nodes"), "running");
    assert_eq!(one(&s, "SELECT json_extract(payload_json,'$.delivery') FROM runtime_events WHERE type='USER_STEERING'"), "boundary");
}

#[test]
fn allow_write_lifts_exactly_the_owners_own_denial() {
    let (_p, s) = setup();
    let (task, execution) = started(&s, 0);
    say(
        &s,
        &execution,
        "deny",
        "deny-write: secrets deny-write: docs/guide.md",
    )
    .unwrap();
    // Lifting a path never denied, or natural language, changes nothing.
    say(
        &s,
        &execution,
        "other",
        "allow-write: src please let it write secrets again",
    )
    .unwrap();
    assert_eq!(
        s.read(|db| steering::denials(db, &task)).unwrap(),
        ["docs/guide.md", "secrets"]
    );
    // A lift of a path inside a denied directory is not that denial: nothing is lifted.
    say(&s, &execution, "inside", "allow-write: secrets/key.txt").unwrap();
    assert_eq!(
        s.read(|db| steering::denials(db, &task)).unwrap(),
        ["docs/guide.md", "secrets"]
    );
    // The exact path, normalized like the denial, is lifted.
    say(&s, &execution, "lift", "ALLOW-WRITE: ./secrets/").unwrap();
    assert_eq!(
        s.read(|db| steering::denials(db, &task)).unwrap(),
        ["docs/guide.md"]
    );
    // One message never both denies and allows a path; an outside path refuses whole.
    assert_eq!(
        say(&s, &execution, "both", "deny-write: a allow-write: a")
            .unwrap_err()
            .code(),
        "write_rule_conflict"
    );
    assert_eq!(
        say(&s, &execution, "out", "allow-write: ../x")
            .unwrap_err()
            .code(),
        "gateway_path_denied"
    );
    // The desktop's Remove control lifts one denial in force, idempotently, and nothing else.
    let lift = |key: &str, path: &str| {
        let request = control::canonical_request(
            json!({"projectId":"p","executionId":execution,"requestId":key,"action":"allow_write","path":path}),
            None,
        )
        .unwrap();
        control::admit_input(&s, "p", "owner", &request, None)
            .and_then(|_| control::finish(&s, "p", "owner", &request, false))
    };
    assert_eq!(
        lift("remove-unknown", "src").unwrap_err().code(),
        "write_denial_unknown"
    );
    lift("remove", "docs/guide.md").unwrap();
    assert!(s
        .read(|db| steering::denials(db, &task))
        .unwrap()
        .is_empty());
    assert_eq!(count(&s, "OWNER_WRITE_ALLOWED"), 1);
    // A later plan touching a lifted path is accepted again.
    let plan = json!({"tasks":[{"id":"n","role":"backend-dev","objective":"Work","dependencies":[],"scope":["secrets"],"model":"auto"}],
        "verification":[["cargo","test"]]});
    s.write(|db| plan::accept_tx(db, &task, "coordinator", "after-lift", &support::with_goal(&plan)))
        .unwrap();
    // The capsule shows the current (empty) set.
    let capsule = s
        .read(|db| cortex_core::recovery_context::snapshot(db, &task))
        .unwrap();
    assert_eq!(capsule["owner_write_denials"]["paths"], json!([]));
}

/// Owner decision 2026-11-23: the project's rule files carry no default write
/// protection; the runtime approves a task's edit of them. Only an explicit owner
/// `deny-write:` protects one, and `allow-write:` lifts that again.
#[test]
fn rule_files_are_writable_unless_the_owner_denies_them() {
    let (_p, s) = setup();
    let (task, execution) = started(&s, 0);
    assert!(s.read(|db| steering::rule_file_protections(db, &task)).unwrap().is_empty());
    assert!(s.read(|db| steering::write_denials(db, &task)).unwrap().is_empty());
    let capsule = s
        .read(|db| cortex_core::recovery_context::snapshot(db, &task))
        .unwrap();
    assert_eq!(capsule["rule_file_protection"]["paths"], json!([]));
    say(&s, &execution, "deny", "deny-write: AGENTS.md").unwrap();
    assert_eq!(s.read(|db| steering::write_denials(db, &task)).unwrap(), ["AGENTS.md"]);
    say(&s, &execution, "lift", "allow-write: AGENTS.md").unwrap();
    assert!(s.read(|db| steering::write_denials(db, &task)).unwrap().is_empty());
}

#[test]
fn a_queued_message_is_taken_only_at_a_boundary() {
    let (_p, s) = setup();
    let (task, execution) = started(&s, 0);
    working(&s, &task, "EXECUTING", "execution");
    let request = control::canonical_request(
        json!({"projectId":"p","executionId":execution,"requestId":"q1","action":"prompt","text":"Queued","queued":true}),
        None,
    )
    .unwrap();
    // A question the coordinator asked the owner: a queued message is never a restart, so it
    // stays queued (nothing is kept). The service takes queued messages only at a boundary.
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id=?", [&task])?;
        db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at)
            VALUES ('cq',?,'cq','How?','[\"A\"]','open','0')", [&task])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        control::admit_input(&s, "p", "owner", &request, None)
            .err()
            .unwrap()
            .code(),
        "queued_message_not_at_boundary"
    );
    s.write(|db| {
        db.execute("UPDATE pending_questions SET status='answered'", [])?;
        db.execute("UPDATE runtime_nodes SET status='completed'", [])?;
        db.execute("UPDATE capacity_reservations SET state='released'", [])?;
        Ok(())
    })
    .unwrap();
    control::admit_input(&s, "p", "owner", &request, None).unwrap();
    control::finish(&s, "p", "owner", &request, false).unwrap();
    assert_eq!(one(&s, "SELECT json_extract(payload_json,'$.delivery') FROM runtime_events WHERE type='USER_STEERING'"), "boundary");
    assert_eq!(
        s.write(|db| steering::deliver_tx(db, &task)).unwrap().len(),
        1
    );
}

#[test]
fn a_message_during_planning_returns_an_unstarted_plan_to_the_coordinator() {
    let (_p, s) = setup();
    let (task, execution) = started(&s, 0);
    assert_eq!(one(&s, "SELECT state FROM runtime_tasks"), "PLANNING");
    // Sent while the coordinator's planning turn runs.
    say(&s, &execution, "steer", "Also cover the docs").unwrap();
    assert_eq!(one(&s, "SELECT state FROM runtime_tasks"), "PLANNING");
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLAN_READY',revision=1 WHERE task_id=?", [&task])?;
        db.execute("INSERT INTO runtime_nodes VALUES (?,1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','pending',NULL,NULL)", [&task])?;
        Ok(())
    })
    .unwrap();
    assert!(s.write(|db| steering::apply_pending_tx(db, &task)).unwrap());
    assert_eq!(one(&s, "SELECT state FROM runtime_tasks"), "PLANNING");
    assert_eq!(one(&s, "SELECT status FROM runtime_nodes"), "cancelled");
    assert_eq!(
        s.write(|db| steering::deliver_tx(db, &task)).unwrap().len(),
        1
    );
}

#[test]
fn a_message_to_a_task_waiting_for_the_owner_keeps_its_restart_delivery() {
    let (_p, s) = setup();
    let (task, execution) = started(&s, 0);
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id=?", [&task])?;
        db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at)
            VALUES ('q',?,'q','Which output?','[\"A\",\"B\"]','open','0')", [&task])?;
        Ok(())
    })
    .unwrap();
    let receipt = say(&s, &execution, "steer", "Neither").unwrap();
    assert_eq!(receipt["state"], "PLANNING");
    assert_eq!(
        count(&s, "TURN_INTERRUPTED"),
        2,
        "admission and the relaunch interrupt, as before"
    );
    assert_eq!(one(&s, "SELECT json_extract(payload_json,'$.delivery') FROM runtime_events WHERE type='USER_STEERING'"), "restart");
    assert!(s
        .read(|db| steering::pending(db, &task))
        .unwrap()
        .is_empty());
    // Applied at once, and handed to the relaunched coordinator like any message.
    let handed = s.write(|db| steering::deliver_tx(db, &task)).unwrap();
    assert_eq!(handed.len(), 1);
    assert!(steering::guidance_prompt(&handed).starts_with("Owner input"));
}

#[test]
fn a_deny_write_rule_is_recorded_and_enforced() {
    let (_p, s) = setup();
    let (task, execution) = started(&s, 0);
    say(
        &s,
        &execution,
        "deny",
        "Keep going. deny-write: ./secrets//key.txt and DENY-WRITE: `docs`.",
    )
    .unwrap();
    assert_eq!(
        one(&s, "SELECT json_extract(payload_json,'$.deny_write') FROM runtime_events WHERE type='USER_STEERING'"),
        r#"["docs","secrets/key.txt"]"#
    );
    // Natural language and a made-up "allow" form neither add nor lift a denial.
    say(
        &s,
        &execution,
        "plain",
        "do not modify src; you may write docs again",
    )
    .unwrap();
    assert_eq!(
        s.read(|db| steering::denials(db, &task)).unwrap(),
        ["docs", "secrets/key.txt"]
    );
    // The coordinator sees the rule in its capsule.
    let capsule = s
        .read(|db| cortex_core::recovery_context::snapshot(db, &task))
        .unwrap();
    assert_eq!(
        capsule["owner_write_denials"]["paths"],
        json!(["docs", "secrets/key.txt"])
    );
    // A later plan touching a denied path is refused; a disjoint one is accepted.
    let plan = |scope: Value| {
        json!({"tasks":[{"id":"n","role":"backend-dev","objective":"Work","dependencies":[],"scope":scope,"model":"auto"}],
        "verification":[["cargo","test"]]})
    };
    assert_eq!(
        s.write(|db| plan::accept_tx(
            db,
            &task,
            "coordinator",
            "denied",
            &support::with_goal(&plan(json!(["secrets"])))
        ))
        .unwrap_err()
        .code(),
        "write_denied"
    );
    assert_eq!(
        s.write(|db| plan::accept_tx(
            db,
            &task,
            "coordinator",
            "docs-file",
            &support::with_goal(&plan(json!(["docs/guide.md"])))
        ))
        .unwrap_err()
        .code(),
        "write_denied"
    );
    s.write(|db| plan::accept_tx(db, &task, "coordinator", "allowed", &support::with_goal(&plan(json!(["src"])))))
        .unwrap();
    // A worker's request for more scope on a denied path is refused too.
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id=?",
            [&task],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.write(|db| domain::propose_replan_tx(
            db,
            &task,
            "worker",
            "scope",
            "scope_request",
            &json!({"scope":["secrets/key.txt"],"reason":"needed"})
        ))
        .unwrap_err()
        .code(),
        "scope_conflicts_with_user_policy"
    );
    // Another task carries no denial of this one.
    let (other, _) = started(&s, 1);
    assert!(s
        .read(|db| steering::denials(db, &other))
        .unwrap()
        .is_empty());
}

#[test]
fn a_denial_outside_the_project_refuses_the_whole_message() {
    let (_p, s) = setup();
    let (_task, execution) = started(&s, 0);
    let before = (
        count(&s, "USER_STEERING"),
        one(&s, "SELECT count(*) FROM host_transcript"),
    );
    for (n, text) in [
        "deny-write: ../outside",
        "deny-write: /etc/passwd",
        "deny-write: .git/hooks",
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            say(&s, &execution, &format!("outside-{n}"), text)
                .unwrap_err()
                .code(),
            "gateway_path_denied",
            "{text}"
        );
    }
    assert_eq!(
        (
            count(&s, "USER_STEERING"),
            one(&s, "SELECT count(*) FROM host_transcript")
        ),
        before
    );
    assert_eq!(
        one(
            &s,
            "SELECT count(*) FROM runtime_requests WHERE request_key LIKE 'outside-%'"
        ),
        "0"
    );
}
