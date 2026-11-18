mod support;
use cortex_core::{
    channels::{self, Principal},
    lifecycle::{self, TaskMachine},
    ownership,
    recovery_context::*,
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
    assert!(
        text.chars().count() < MAX_CHARACTERS,
        "{}",
        text.chars().count()
    );
    assert_eq!(text.matches(&goal).count(), 1, "the goal is carried once");
    assert!(text.contains("\"same_as\":\"original_goal\""));
    assert!(
        !text.contains(&"f".repeat(TEXT_PROBE)),
        "follow-ups are bounded"
    );
    // Omitted constraints are listed and readable by id; none is silently lost.
    let notice = v["clipping"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["section"] == "constraints")
        .unwrap();
    let omitted = notice["omitted"].as_i64().unwrap() as usize;
    assert!(omitted > 0);
    assert!(notice["how_to_read"]
        .as_str()
        .unwrap()
        .contains("<event_id>:constraint"));
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
/// Clipping shortens prose, never identifiers: at every representation tier each
/// node's report id, verdict and report-structure marker stay exact, even when
/// alphabetically earlier result fields (checks, findings) are long.
#[test]
fn clipping_keeps_every_report_ref_exact() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "Goal", "owner").unwrap();
    seed_nodes(&s, 64, &"o".repeat(8000), 64, "x");
    s.write(|db| {
        let ids: Vec<String> = db
            .prepare("SELECT node_id FROM runtime_nodes WHERE task_id='t'")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for (n, id) in ids.iter().enumerate() {
            let result = json!({"checks":[{"name":"c".repeat(3000),"outcome":"not_run","blocker":"b".repeat(3000)}],
                "findings":["f".repeat(3000)],"report_ref":format!("r_{n:012x}"),"verdict":"APPROVE",
                "report_structure":{"status":"REPORT_STRUCTURE_INCOMPLETE","missing":["## Changes"]},"summary":"s".repeat(3000)});
            db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id='t' AND node_id=?", (result.to_string(), id))?;
        }
        Ok(())
    })
    .unwrap();
    let v = s.read(|db| snapshot(db, "t")).unwrap();
    assert!(v.to_string().chars().count() < MAX_CHARACTERS);
    let nodes = v["current_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 64);
    assert!(v["clipping"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["section"] == "current_nodes" && n["reduced_detail_level"].as_i64() > Some(0)));
    for n in 0..64 {
        let node = nodes
            .iter()
            .find(|x| x["report_ref"] == format!("r_{n:012x}"))
            .unwrap_or_else(|| panic!("r_{n:012x} lost"));
        assert_eq!(node["verdict"], "APPROVE");
        assert_eq!(node["report_structure"], "REPORT_STRUCTURE_INCOMPLETE");
    }
}
/// Owner messages to a working task are the capsule's constraints, with their paths.
#[test]
fn owner_steering_messages_are_constraints() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "Goal", "owner").unwrap();
    s.write(|db| {
        lifecycle::event_tx(db, "t", "USER_STEERING",
            json!({"text":"Keep the API stable. deny-write: src/api.rs","deny_write":["src/api.rs"],"delivery":"boundary","request":"m1"}))?;
        Ok(())
    })
    .unwrap();
    let v = s.read(|db| snapshot(db, "t")).unwrap();
    let constraints = v["constraints"].as_array().unwrap();
    assert_eq!(constraints.len(), 1);
    assert_eq!(constraints[0]["type"], "owner_steering");
    assert_eq!(
        constraints[0]["payload"]["text"],
        "Keep the API stable. deny-write: src/api.rs"
    );
    assert_eq!(
        constraints[0]["payload"]["deny_write"],
        json!(["src/api.rs"])
    );
    assert_eq!(v["owner_write_denials"]["paths"], json!(["src/api.rs"]));
}
/// The brief lists every predecessor report id with role, title, size and pages, for
/// a dependent node and for the consultation (its packet's reports), and the owner's
/// constraints and acceptance commands.
#[test]
fn brief_lists_predecessor_reports_constraints_and_acceptance() {
    use cortex_core::assignment_brief::{extras, predecessor_reports};
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "Goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1,plan_json=? WHERE task_id='t'",
            [json!({"tasks":[],"verification":[["cargo","test"]]}).to_string()])?;
        for (id, bytes) in [("r_aaaaaaaaaaaa", 9001), ("r_bbbbbbbbbbbb", 10)] {
            db.execute("INSERT INTO reports(id,task_id,kind,filename,created_at) VALUES (?,'t','report',?,'0')", (id, format!("{id}.md")))?;
            db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES (?,'Explorer findings','S','x','0','d',?)", (id, bytes))?;
        }
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'scan','worker-explorer','Scan','[]','[\"src\"]','execution','completed','a1',?)",
            [json!({"report_ref":"r_aaaaaaaaaaaa"}).to_string()])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'build','worker-backend-dev','Build','[\"scan\"]','[\"src\"]','execution','running','a2',NULL)", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'_consultation','worker-senior-consultant','Consult','[]','[\".\"]','execution','running','a3',NULL)", [])?;
        lifecycle::event_tx(db, "t", "CONSULTATION_REQUIRED", json!({"revision":1,"node":"_consultation",
            "dependency_reports":[{"report_id":"r_bbbbbbbbbbbb","availability":"recorded"}]}))?;
        lifecycle::event_tx(db, "t", "USER_STEERING", json!({"text":"Keep it small.","delivery":"boundary","request":"m"}))?;
        Ok(())
    })
    .unwrap();
    let reports = s.read(|db| predecessor_reports(db, "t", "build")).unwrap();
    assert_eq!(
        reports,
        vec![
            json!({"report_id":"r_aaaaaaaaaaaa","node":"scan","role":"explorer",
        "title":"Explorer findings","size_bytes":9001,"pages_at_most":3})
        ]
    );
    let consult = s
        .read(|db| predecessor_reports(db, "t", "_consultation"))
        .unwrap();
    assert_eq!(consult[0]["report_id"], "r_bbbbbbbbbbbb");
    assert_eq!(consult[0]["pages_at_most"], 1);
    let brief = s
        .read(|db| extras(db, "t", "build", "worker-backend-dev", "execution", "ru"))
        .unwrap();
    assert_eq!(
        brief["acceptance"]["verification_commands"],
        json!([["cargo", "test"]])
    );
    assert_eq!(
        brief["constraints"]["owner_messages"][0]["text"],
        "Keep it small."
    );
    assert_eq!(brief["report"]["template"], "implementation");
    assert_eq!(brief["user_question_language"], "ru");
    assert_eq!(
        brief["predecessor_reports"][0]["report_id"],
        "r_aaaaaaaaaaaa"
    );
    let review = s
        .read(|db| extras(db, "t", "build", "worker-code-reviewer", "review", "en"))
        .unwrap();
    assert!(
        review.get("predecessor_reports").is_none(),
        "a review reads its review_packet"
    );
    assert_eq!(review["report"]["sections"][0]["heading"], "## Verdict");
}
