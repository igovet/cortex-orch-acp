mod support;
use cortex_core::{
    lifecycle::{skill_proposals as skills, waits, FoundationGuards, TaskMachine},
    store::documents::{Publication, Publisher},
    TaskState,
};
use serde_json::{json, Value};
use support::*;
fn evidence(s: &cortex_core::Store) -> Value {
    let api = Publisher::new(s);
    let mut cases = Vec::new();
    for key in ["one", "two", "three", "four", "five"] {
        let d = api.create_draft("t", "o", "general", key).unwrap();
        api.write_draft(
            "t",
            "o",
            &d.draft_id,
            &format!(
                "Cortex draft ID: `{}`\n\n# Case\n\nReal fixture case.",
                d.draft_id
            ),
        )
        .unwrap();
        let r = api
            .publish(
                "t",
                "o",
                &d.draft_id,
                key,
                Publication {
                    title: "Case",
                    summary: "Case",
                    author: "o",
                    source_revision: 0,
                    artifacts: json!([]),
                },
            )
            .unwrap();
        cases.push(json!({"report_id":r.report_id,"case":key}));
    }
    json!(cases)
}
fn setup() -> (Project, cortex_core::Store, Value) {
    let p = Project::new();
    let s = p.open();
    let m = TaskMachine::new(&s);
    let o = m.create("t", "goal", "o").unwrap();
    m.transition(
        "t",
        &o,
        "planning",
        TaskState::Planning,
        None,
        &FoundationGuards,
    )
    .unwrap();
    let e = evidence(&s);
    (
        p,
        s,
        json!({"request_key":"q","question":"Create?","options":["Create the skill","Not now"],
        "skill_proposal":{"name":"case-skill","action":"create","purpose":"Repeated cases","evidence":e.as_array().unwrap()[..2]}}),
    )
}
#[test]
fn coordinator_batch_waits_for_every_answer() {
    let (p, s, _) = setup();
    let a = json!({"request_key":"batch","questions":[
        {"question":"Output?","options":["Long","Short"],"topic":"Output"},
        {"question":"Language?","options":["English","Russian"],"topic":"Language"}]});
    let q = s
        .write(|db| skills::request(db, &p.0, "t", "c", "batch", &a))
        .unwrap();
    let ids: Vec<String> = s
        .read(|db| {
            Ok(db
                .prepare("SELECT id FROM pending_questions ORDER BY rowid")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(q["question_id"], ids[0]);
    s.write(|db| waits::answer_tx(db, "t", &ids[0], "Long", "text", None))
        .unwrap();
    assert_eq!(
        s.write(|db| skills::continuation(db, "t", "c", false))
            .unwrap()
            .as_deref(),
        Some("open")
    );
    s.write(|db| waits::answer_tx(db, "t", &ids[1], "Russian", "text", None))
        .unwrap();
    let prompt = s
        .write(|db| skills::continuation(db, "t", "c", true))
        .unwrap()
        .unwrap();
    assert!(prompt.contains("Output?: Long") && prompt.contains("Language?: Russian"));
}
#[test]
fn max_evidence_cross_task_duplicates_and_symlinks() {
    let (p, s, mut a) = setup();
    let all: Vec<Value> = s
        .read(|db| {
            Ok(db
                .prepare("SELECT id FROM reports ORDER BY rowid")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|id| json!({"report_id":id,"case":"case"}))
                .collect())
        })
        .unwrap();
    let mut too_many = all.clone();
    too_many.push(all[0].clone());
    a["skill_proposal"]["evidence"] = json!(too_many);
    assert_eq!(
        s.write(|db| skills::request(db, &p.0, "t", "c", "bad", &a))
            .unwrap_err()
            .code(),
        "skill_proposal_evidence"
    );
    a["skill_proposal"]["evidence"] = json!(all);
    std::fs::create_dir(p.0.join("outside")).unwrap();
    std::os::unix::fs::symlink("outside", p.0.join(".agents")).unwrap();
    assert_eq!(
        s.write(|db| skills::request(db, &p.0, "t", "c", "bad", &a))
            .unwrap_err()
            .code(),
        "skill_proposal_exists"
    );
    std::fs::remove_file(p.0.join(".agents")).unwrap();
    let q = s
        .write(|db| skills::request(db, &p.0, "t", "c", "good", &a))
        .unwrap();
    let m = TaskMachine::new(&s);
    let owner = m.create("other", "g", "o").unwrap();
    m.transition(
        "other",
        &owner,
        "plan",
        TaskState::Planning,
        None,
        &FoundationGuards,
    )
    .unwrap();
    assert_eq!(
        s.write(|db| skills::request(db, &p.0, "other", "d", "pending", &a))
            .unwrap_err()
            .code(),
        "skill_proposal_pending"
    );
    s.write(|db| {
        waits::answer_tx(
            db,
            "t",
            q["question_id"].as_str().unwrap(),
            "2",
            "text",
            None,
        )
    })
    .unwrap();
    assert_eq!(
        s.write(|db| skills::request(db, &p.0, "other", "d", "answered", &a))
            .unwrap_err()
            .code(),
        "skill_proposal_already_asked"
    );
}
#[test]
fn exact_action_matched_labels_and_unsafe_legacy_answers() {
    for (action, pairs) in [
        (
            "create",
            vec![
                ["Create the skill", "Not now"],
                ["Создать навык", "Не сейчас"],
                ["Approve creating the skill", "Decline creating the skill"],
                ["Одобрить создание навыка", "Отклонить создание навыка"],
            ],
        ),
        (
            "update",
            vec![
                ["Update the skill", "Not now"],
                ["Обновить навык", "Не сейчас"],
                ["Approve updating the skill", "Decline updating the skill"],
                ["Одобрить обновление навыка", "Отклонить обновление навыка"],
            ],
        ),
    ] {
        for pair in pairs {
            let options = json!(pair);
            assert!(skills::safe_options(&options, action));
            for answer in ["1", pair[0]] {
                assert_eq!(skills::decide(&options, answer, action), "approved");
            }
            for answer in ["2", pair[1]] {
                assert_eq!(skills::decide(&options, answer, action), "declined");
            }
            for answer in ["yes", "yes, but", "3", "", "1."] {
                assert_eq!(skills::decide(&options, answer, action), "unclear");
            }
            assert!(!skills::safe_options(&json!([pair[1], pair[0]]), action));
            assert!(!skills::safe_options(
                &options,
                if action == "create" {
                    "update"
                } else {
                    "create"
                }
            ));
        }
    }
    assert_eq!(
        skills::decide(&json!(["Arbitrary approval", "Not now"]), "1", "create"),
        "unclear"
    );
    assert_eq!(
        skills::decide(
            &json!([" CREATE  THE SKILL ", " Not  NOW "]),
            " create the  skill ",
            "create"
        ),
        "approved"
    );
}
#[test]
fn proposal_evidence_validation_rollback_and_filesystem_state() {
    let (p, s, mut a) = setup();
    let cases = a["skill_proposal"]["evidence"].clone();
    for bad in [
        json!([]),
        json!([cases[0]]),
        json!([cases[0], cases[0]]),
        json!([cases[0],{"report_id":"r_000000000000","case":"unknown"}]),
    ] {
        a["skill_proposal"]["evidence"] = bad;
        assert!(s
            .write(|db| skills::request(db, &p.0, "t", "c", "bad", &a))
            .is_err());
        assert_eq!(count(&s, "pending_questions"), 0);
    }
    a["skill_proposal"]["evidence"] = cases.clone();
    // A published pipeline is not an ordinary report.
    s.write(|db| {
        db.execute(
            "UPDATE reports SET kind='pipeline' WHERE id=?",
            [cases[1]["report_id"].as_str()],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.write(|db| skills::request(db, &p.0, "t", "c", "bad", &a))
            .unwrap_err()
            .code(),
        "skill_proposal_evidence_unknown"
    );
    s.write(|db| {
        db.execute(
            "UPDATE reports SET kind='report' WHERE id=?",
            [cases[1]["report_id"].as_str()],
        )?;
        Ok(())
    })
    .unwrap();
    a["options"] = json!(["Not now", "Create the skill"]);
    assert_eq!(
        s.write(|db| skills::request(db, &p.0, "t", "c", "bad", &a))
            .unwrap_err()
            .code(),
        "skill_proposal_options"
    );
    a["options"] = json!(["Create the skill", "Not now"]);
    a["skill_proposal"]["name"] = json!("bad--name");
    assert!(s
        .write(|db| skills::request(db, &p.0, "t", "c", "bad", &a))
        .is_err());
    a["skill_proposal"]["name"] = json!("case-skill");
    std::fs::create_dir_all(p.0.join(".agents/skills/case-skill")).unwrap();
    assert_eq!(
        s.write(|db| skills::request(db, &p.0, "t", "c", "bad", &a))
            .unwrap_err()
            .code(),
        "skill_proposal_exists"
    );
    a["skill_proposal"]["action"] = json!("update");
    a["options"] = json!(["Обновить навык", "Не сейчас"]);
    assert_eq!(
        s.write(|db| skills::request(db, &p.0, "t", "c", "bad", &a))
            .unwrap_err()
            .code(),
        "skill_proposal_missing"
    );
    std::fs::write(p.0.join(".agents/skills/case-skill/SKILL.md"), "unchanged").unwrap();
    s.write(|db| skills::request(db, &p.0, "t", "c", "good", &a))
        .unwrap();
    assert_eq!(count(&s, "pending_questions"), 1);
}
#[test]
fn durable_decisions_retries_and_single_continuation_leave_project_unchanged() {
    for answer in ["1", "2", "yes, but"] {
        let (p, s, a) = setup();
        let r = s
            .write(|db| skills::request(db, &p.0, "t", "c", "q", &a))
            .unwrap();
        assert_eq!(
            s.write(|db| skills::request(db, &p.0, "t", "c", "q", &a))
                .unwrap(),
            r
        );
        let id = r["question_id"].as_str().unwrap();
        assert_eq!(
            s.write(|db| skills::continuation(db, "t", "c", false))
                .unwrap()
                .as_deref(),
            Some("open")
        );
        s.write(|db| {
            assert!(waits::answer_tx(db, "t", id, answer, "text", None)?);
            Ok(())
        })
        .unwrap();
        drop(s);
        let s = p.open();
        assert!(!s
            .write(|db| waits::answer_tx(db, "t", id, answer, "text", None))
            .unwrap());
        assert_eq!(
            s.read(|db| Ok(db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='SKILL_PROPOSAL_DECIDED'",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .unwrap(),
            1
        );
        let recovered = s.read(|db| skills::planning_instruction(db, "t")).unwrap();
        assert_eq!(
            recovered.contains("/skill-management Create"),
            answer == "1"
        );
        let text = s
            .write(|db| skills::continuation(db, "t", "c", true))
            .unwrap()
            .unwrap();
        assert_eq!(text.contains("/skill-management Create"), answer == "1");
        assert_eq!(
            text.contains("Do not create, update or link"),
            answer != "1"
        );
        assert!(s
            .write(|db| skills::continuation(db, "t", "c", true))
            .unwrap()
            .is_none());
        assert!(!p.0.join(".agents").exists());
        assert_eq!(
            s.write(|db| skills::request(db, &p.0, "t", "c", "other", &a))
                .unwrap_err()
                .code(),
            "skill_proposal_limit"
        );
    }
}
