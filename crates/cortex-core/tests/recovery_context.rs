mod support;
use cortex_core::{
    channels::{self, Principal},
    lifecycle::{self, TaskMachine},
    ownership,
    recovery_context::*,
    CoreError,
};
use serde_json::json;
use support::*;
#[test]
fn capsule_rebuild_preserves_constraints_answers_and_exact_fences() {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s)
        .create("t", "Original α goal", "owner")
        .unwrap();
    s.write(|db|{db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'",[])?;
        db.execute("INSERT INTO task_events(task_id,event_type,dedup_key,payload_json,origin,created_at) VALUES ('t','user_message','constraint',?,'owner','now')",
            [json!({"text":"Never lose constraint β"}).to_string()])?;
        db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,answer_text,answer_source,created_at)
            VALUES ('q','t','q','Which route?','[\"A\",\"B\"]','answered','A, without paid calls','text','now')",[])?;
        lifecycle::event_tx(db,"t","USER_ANSWER_RECEIVED",json!({"question_id":"q"}))?;
        Ok(())}).unwrap();
    let c = s
        .write(|db| {
            let c = channels::issue_tx(db, "t", &o, Principal::Coordinator)?;
            channels::bind_session_tx(db, &c, "wire")?;
            generation_tx(db, &c, "session_open", None)?;
            Ok(c)
        })
        .unwrap();
    let (first, body) = s.write(|db| delivery_tx(db, &c)).unwrap();
    assert!(body.contains("Original α goal"));
    assert!(body.contains("Never lose constraint β"));
    assert!(body.contains("A, without paid calls"));
    assert!(!body.contains(c.secret()));
    let id = s
        .write(|db| generation_tx(db, &c, "observed_compaction", Some("compact-1")))
        .unwrap();
    assert_eq!(
        s.write(|db| generation_tx(db, &c, "observed_compaction", Some("compact-1")))
            .unwrap(),
        id
    );
    s.write(|db| {
        lifecycle::event_tx(db, "t", "USER_STEERING", json!({"text":"New constraint γ"}))?;
        Ok(())
    })
    .unwrap();
    let (second, body) = s.write(|db| delivery_tx(db, &c)).unwrap();
    assert_ne!(first, second);
    assert!(body.contains("New constraint γ"));
    assert!(body.contains(&id));
    let next = s
        .write(|db| {
            channels::revoke_tx(db, &c.channel_id)?;
            db.execute(
                "UPDATE delegation_provisioning SET status='terminated' WHERE channel_id=?",
                [&c.channel_id],
            )?;
            let c = channels::issue_tx(db, "t", &o, Principal::Coordinator)?;
            channels::bind_session_tx(db, &c, "wire")?;
            generation_tx(db, &c, "session_replacement", None)?;
            Ok(c)
        })
        .unwrap();
    let (_, body) = s.write(|db| delivery_tx(db, &next)).unwrap();
    assert!(body.contains("New constraint γ"));
    assert!(s.write(|db| delivery_tx(db, &c)).is_err());
    s.write(|db| {
        ownership::acquire_tx(db, "t", "replacement", Some(o.epoch))?;
        Ok(())
    })
    .unwrap();
    assert!(s.write(|db| delivery_tx(db, &next)).is_err());
}
#[test]
fn compaction_burst_is_bounded_and_old_events_age_out() {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    let c = s
        .write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'",
                [],
            )?;
            let c = channels::issue_tx(db, "t", &o, Principal::Coordinator)?;
            channels::bind_session_tx(db, &c, "wire")?;
            generation_tx(db, &c, "session_open", None)?;
            Ok(c)
        })
        .unwrap();
    for n in 0..5 {
        s.write(|db| generation_tx(db, &c, "observed_compaction", Some(&format!("c{n}"))))
            .unwrap();
    }
    assert_eq!(
        s.write(|db| generation_tx(db, &c, "observed_compaction", Some("burst")))
            .unwrap_err()
            .code(),
        "compaction_loop_requires_user_input"
    );
    s.write(|db| {
        db.execute(
            "UPDATE context_generations SET created_at='2000-01-01T00:00:00Z'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    s.write(|db| generation_tx(db, &c, "observed_compaction", Some("later")))
        .unwrap();
}
// An oversized constraint is never dropped and never repeated whole: the capsule
// carries a bounded preview with size and sha256, and read_source returns it in full.
#[test]
fn oversized_constraint_is_bounded_with_a_hash_and_a_read_path() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute(
            "INSERT INTO task_events(task_id,event_type,dedup_key,payload_json,origin,created_at)
        VALUES ('t','user_message','large',?,'owner','now')",
            [json!({"text":"a".repeat(MAX_CHARACTERS)}).to_string()],
        )?;
        Ok(())
    })
    .unwrap();
    let v = s.read(|db| snapshot(db, "t")).unwrap();
    assert!(v.to_string().chars().count() < MAX_CHARACTERS);
    let row = &v["constraints"][0];
    assert_eq!(row["payload"]["text"]["clipped"], true);
    assert_eq!(row["payload"]["text"]["size_chars"], MAX_CHARACTERS);
    assert_eq!(row["payload"]["text"]["sha256"].as_str().unwrap().len(), 64);
}
// A 32k goal repeated by 32k follow-ups and a maximum plan stays under the ceiling; the
// goal appears once, repeats are sha256 references, and omitted constraints are indexed.
#[test]
fn goal_followups_and_max_plan_stay_under_the_ceiling_without_repeating_the_goal() {
    let p = Project::new();
    let s = p.open();
    let goal = "g".repeat(32_000);
    TaskMachine::new(&s).create("t", &goal, "owner").unwrap();
    let plan = json!({"tasks":(0..64).map(|i|json!({"id":format!("n{i:03}"),"objective":"o".repeat(8000)})).collect::<Vec<_>>(),
        "verification":(0..16).map(|_|(0..64).map(|_|"v".repeat(4096)).collect::<Vec<_>>()).collect::<Vec<_>>()});
    seed_nodes(&s, 64, &"o".repeat(8000), 64, &"r".repeat(20_000));
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id='t'", [plan.to_string()])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('acp-1',?,'t','{\"name\":\"cortex-rust-acp\"}','{}','[]','now',NULL)",
            [p.0.to_str().unwrap()])?;
        // The goal repeated as a follow-up, then distinct 32k follow-ups and many small ones.
        let mut texts: Vec<String> = (0..6).map(|i| format!("{i}{}", "f".repeat(31_999))).collect();
        texts.extend((0..200).map(|i| format!("small steering {i}")));
        texts.push(goal.clone());
        for (n, text) in texts.iter().enumerate() {
            db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,attachments_json,created_at) VALUES ('acp-1',?,'user_message',?,NULL,?)",
                (n as i64, text, format!("2026-01-01T00:{:02}:{:02}.000001+00:00", n / 60, n % 60)))?;
            db.execute("INSERT INTO task_events(task_id,event_type,dedup_key,payload_json,origin,created_at) VALUES ('t','user_message',?,?,'owner','now')",
                (format!("k{n}"), json!({"text":text}).to_string()))?;
        }
        Ok(())
    })
    .unwrap();
    let v = s.read(|db| snapshot(db, "t")).unwrap();
    let text = v.to_string();
    assert!(text.chars().count() < MAX_CHARACTERS, "{}", text.chars().count());
    assert_eq!(text.matches(&goal).count(), 1, "the goal is carried once");
    assert!(text.contains("\"same_as\":\"original_goal\""));
    assert!(!text.contains(&"f".repeat(TEXT_PROBE)), "follow-ups are bounded");
    // Omitted constraints are listed and readable by id; none is silently lost.
    let notice = v["clipping"].as_array().unwrap().iter().find(|n| n["section"] == "constraints").unwrap();
    let omitted = notice["omitted"].as_i64().unwrap() as usize;
    assert!(omitted > 0);
    assert!(notice["how_to_read"].as_str().unwrap().contains("<event_id>:constraint"));
    let ids = notice["omitted_event_ids"].as_array().unwrap().len();
    assert_eq!(ids, omitted);
    assert_eq!(notice["index_complete"], true);
}
const TEXT_PROBE: usize = 2_000;

fn seed_nodes(s: &cortex_core::Store, count: usize, objective: &str, scopes: usize, result: &str) {
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING',revision=1 WHERE task_id='t'", [])?;
        let scope = json!((0..scopes).map(|i| format!("{i:03}/{}", "s".repeat(500))).collect::<Vec<_>>());
        let deps = |n: usize| json!((0..n.min(63)).map(|i| format!("n{i:03}{}", "d".repeat(59))).collect::<Vec<_>>());
        for n in 0..count {
            db.execute(
                "INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status,assignment_ref,result_json)
                 VALUES ('t',1,?,'worker-general',?,?,?,'work','completed',?,?)",
                (format!("n{n:03}{}", "x".repeat(59)), objective, deps(n).to_string(), scope.to_string(),
                 format!("a{n}"), json!({"report_ref":format!("r{n}"),"summary":result}).to_string()),
            )?;
        }
        Ok(())
    })
    .unwrap();
}
#[test]
fn realistic_event_volume_stays_bounded_and_visible() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s)
        .create("t", "Long task goal", "owner")
        .unwrap();
    seed_nodes(&s, 40, &"o".repeat(1500), 6, &"r".repeat(900));
    s.write(|db| {
        for n in 0..4000 {
            lifecycle::event_tx(db, "t", "NATIVE_COMMAND_STARTED", json!({"ref":format!("c{n}"),"argv":["cargo","test"]}))?;
            if n != 3999 {
                lifecycle::event_tx(db, "t", "NATIVE_COMMAND_FINISHED", json!({"receipt":{"tool_call_ref":format!("c{n}"),"argv":["cargo"],"observation":{"x":"y"}}}))?;
            }
            lifecycle::event_tx(db, "t", "NATIVE_GUIDANCE_SETTLED", json!({"n":n}))?;
        }
        for n in 0..1500 {
            lifecycle::event_tx(db, "t", "WORKER_COMPLETED", json!({"assignment_ref":format!("a{n}"),"attempt_id":"x","result":{"report_ref":"r","summary":"s".repeat(2000)}}))?;
        }
        lifecycle::event_tx(db, "t", "USER_STEERING", json!({"text":"Newest steering ζ"}))?;
        Ok(())
    })
    .unwrap();
    let v = s.read(|db| snapshot(db, "t")).unwrap();
    let text = v.to_string();
    assert!(text.chars().count() < MAX_CHARACTERS);
    assert!(
        text.contains("Newest steering ζ"),
        "newest decision is kept"
    );
    assert_eq!(v["command_events_not_restated"], 7999);
    assert!(
        text.contains("c3999"),
        "the unresolved command stays visible"
    );
    assert_eq!(v["unresolved_commands"].as_array().unwrap().len(), 1);
    assert!(!text.contains("NATIVE_COMMAND_STARTED"));
    let notice = v["clipping"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["section"] == "retained_decisions")
        .unwrap();
    assert!(notice["omitted"].as_i64().unwrap() > 0 && notice["how_to_read"].is_string());
    assert_eq!(
        notice["total"].as_i64().unwrap(),
        notice["included"].as_i64().unwrap() + notice["omitted"].as_i64().unwrap()
    );
    assert!(
        !text.contains(&"s".repeat(2000)),
        "worker results are carried once, in current_nodes"
    );
}
#[test]
fn maximum_size_plans_fit_and_keep_own_objective_whole() {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s)
        .create("t", &"g".repeat(32_000), "owner")
        .unwrap();
    let plan = json!({"tasks":(0..64).map(|i|json!({"id":format!("n{i:03}"),"objective":"o".repeat(8000)})).collect::<Vec<_>>(),
        "verification":(0..16).map(|_|(0..64).map(|_|"v".repeat(4096)).collect::<Vec<_>>()).collect::<Vec<_>>()});
    seed_nodes(&s, 64, &"o".repeat(8000), 64, &"r".repeat(20_000));
    let c = s
        .write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET plan_json=? WHERE task_id='t'",
                [plan.to_string()],
            )?;
            db.execute(
                "UPDATE runtime_nodes SET objective=? WHERE node_id LIKE 'n007%'",
                ["own-objective-".to_string() + &"w".repeat(7900)],
            )?;
            lifecycle::event_tx(db, "t", "PLAN_ACCEPTED", json!({"revision":1,"plan":plan}))?;
            let c = channels::issue_tx(db, "t", &o, Principal::Coordinator)?;
            channels::bind_session_tx(db, &c, "wire")?;
            generation_tx(db, &c, "session_open", None)?;
            Ok(c)
        })
        .unwrap();
    let v = s.read(|db| snapshot(db, "t")).unwrap();
    assert!(
        v.to_string().chars().count() < MAX_CHARACTERS,
        "{}",
        v.to_string().chars().count()
    );
    assert_eq!(
        v["current_nodes"].as_array().unwrap().len(),
        64,
        "every node status stays listed"
    );
    assert!(v["clipping"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["section"] == "current_nodes"));
    let own = s.read(|db| snapshot_for(db, "t", Some("a7"))).unwrap();
    assert!(own.to_string().chars().count() < MAX_CHARACTERS);
    assert!(own.to_string().contains(&"w".repeat(7900)));
    let (_, body) = s.write(|db| delivery_tx(db, &c)).unwrap();
    assert!(body.contains("current_nodes"));
}
#[test]
fn follow_up_appears_once_across_internal_and_acp_sessions() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        for (id, name) in [("native_abc", "cortex-native"), ("acp-1", "cortex-rust-acp")] {
            db.execute("INSERT INTO acp_host_sessions VALUES (?,?,?,?,'{}','[]','now',NULL)",
                (id, p.0.to_str().unwrap(), "t", json!({"name":name}).to_string()))?;
            db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,attachments_json,created_at) VALUES (?,0,'user_message','Follow-up μ',NULL,'2026-01-01T00:00:00.000001+00:00')", [id])?;
        }
        // The same words sent again later are a distinct message and stay.
        db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,attachments_json,created_at) VALUES ('acp-1',1,'user_message','Follow-up μ',NULL,'2026-01-01T00:05:00.000001+00:00')", [])?;
        Ok(())
    }).unwrap();
    let v = s.read(|db| snapshot(db, "t")).unwrap();
    assert_eq!(v["retained_user_messages"].as_array().unwrap().len(), 2);
    assert_eq!(v.to_string().matches("Follow-up μ").count(), 2);
}
