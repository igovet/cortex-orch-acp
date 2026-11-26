mod support;
use cortex_core::lifecycle::{plan, TaskMachine};
use serde_json::{json, Value};
use sha2::Digest;
use support::Project;
fn proposal() -> Value {
    json!({"tasks":[{"id":"n","role":"backend-dev","objective":"Work","dependencies":[],"scope":["./src//","src"],"model":"auto","delivers":["work"]}],
        "verification":[["cargo","test"]],"outcomes":[{"id":"work","statement":"The work is done"}]})
}
fn planning(s: &cortex_core::Store) {
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'", [])?;
        Ok(())
    })
    .unwrap();
}
#[test]
fn every_goal_outcome_is_delivered_or_deferred_with_a_reason() {
    let base = proposal();
    let refused = |p: &Value| plan::validate(p).unwrap_err().code();
    let mut p = base.clone();
    p.as_object_mut().unwrap().remove("outcomes");
    assert_eq!(refused(&p), "plan_outcomes_required");
    p["outcomes"] = json!([]);
    assert_eq!(refused(&p), "plan_outcomes_required");
    // An outcome no task delivers and nobody deferred.
    let mut p = base.clone();
    p["outcomes"] = json!([{"id":"work","statement":"The work is done"},{"id":"push","statement":"The changes are pushed"}]);
    assert_eq!(refused(&p), "plan_outcome_uncovered");
    // Deferred without a reason, then with one.
    p["outcomes"][1]["deferred"] = json!("  ");
    assert_eq!(refused(&p), "plan_outcome_deferral_reason_required");
    p["outcomes"][1]["deferred"] = json!("Needs the prepared branch first");
    assert!(plan::validate(&p).is_ok());
    // Delivered and deferred at once, an unknown id, a duplicate outcome.
    p["tasks"][0]["delivers"] = json!(["work", "push"]);
    assert_eq!(refused(&p), "plan_outcome_deferral_conflict");
    p["tasks"][0]["delivers"] = json!(["work", "other"]);
    assert_eq!(refused(&p), "plan_outcome_unknown");
    let mut p = base.clone();
    p["outcomes"] = json!([{"id":"work","statement":"a"},{"id":"work","statement":"b"}]);
    assert_eq!(refused(&p), "invalid_plan_outcome");
}
#[test]
fn outcomes_survive_replans_and_steering_and_may_grow() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    planning(&s);
    let mut first = proposal();
    first["outcomes"] = json!([{"id":"work","statement":"The work is done"},{"id":"push","statement":"The changes are pushed to GitHub","deferred":"After the work"}]);
    s.write(|db| plan::accept_tx(db, "t", "c", "k1", &first)).unwrap();
    // Steering keeps the outcomes: the editable plan carries them and an amendment
    // that drops one is refused.
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='t'", [])?;
        Ok(())
    })
    .unwrap();
    let (raw, stored): (String, Value) = s
        .read(|db| {
            let raw: String = db.query_row("SELECT plan_json FROM runtime_tasks WHERE task_id='t'", [], |r| r.get(0))?;
            Ok((raw.clone(), serde_json::from_str(&raw)?))
        })
        .unwrap();
    let mut editable = plan::steerable_plan(&stored).unwrap();
    assert_eq!(editable["outcomes"], first["outcomes"]);
    editable["outcomes"] = json!([{"id":"work","statement":"The work is done"}]);
    let amend = json!({"expected_plan":format!("{:x}", sha2::Sha256::digest(raw.as_bytes())),"plan":editable});
    assert_eq!(
        s.write(|db| plan::amend_active_tx(db, "t", &amend)).unwrap_err().code(),
        "plan_outcome_dropped"
    );
    // A replan that drops or rewords an outcome is refused; one that keeps both
    // and adds an owner's new outcome is accepted.
    planning(&s);
    let mut narrowed = proposal();
    narrowed["tasks"][0]["id"] = json!("prep");
    assert_eq!(
        s.write(|db| plan::accept_tx(db, "t", "c", "k2", &narrowed)).unwrap_err().code(),
        "plan_outcome_dropped"
    );
    let mut reworded = first.clone();
    reworded["outcomes"][1] = json!({"id":"push","statement":"The push is prepared","deferred":"Later"});
    assert_eq!(
        s.write(|db| plan::accept_tx(db, "t", "c", "k3", &reworded)).unwrap_err().code(),
        "plan_outcome_dropped"
    );
    let mut next = first.clone();
    next["outcomes"][1] = json!({"id":"push","statement":"The changes are pushed to  GitHub"});
    next["outcomes"].as_array_mut().unwrap().push(json!({"id":"notes","statement":"Release notes are written"}));
    next["tasks"][0]["delivers"] = json!(["work", "push", "notes"]);
    s.write(|db| plan::accept_tx(db, "t", "c", "k4", &next)).unwrap();
    let kept = s.read(|db| plan::task_outcomes(db, "t")).unwrap().unwrap();
    assert_eq!(kept.len(), 3);
}
#[test]
fn plan_receipt_nodes_events_and_state_are_atomic() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let plan = proposal();
    s.write(|db|{db.execute_batch("CREATE TRIGGER fail_plan BEFORE INSERT ON runtime_requests WHEN NEW.operation='plan_submit' BEGIN SELECT RAISE(ABORT,'fixture'); END")?;Ok(())}).unwrap();
    assert!(s
        .write(|db| plan::accept_tx(db, "t", "channel", "key", &plan))
        .is_err());
    s.read(|db| {
        assert_eq!(
            db.query_row(
                "SELECT revision FROM runtime_tasks WHERE task_id='t'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM runtime_nodes", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
    s.write(|db| {
        db.execute_batch("DROP TRIGGER fail_plan")?;
        Ok(())
    })
    .unwrap();
    let first = s
        .write(|db| plan::accept_tx(db, "t", "channel", "key", &plan))
        .unwrap();
    assert_eq!(
        first,
        s.write(|db| plan::accept_tx(db, "t", "channel", "key", &plan))
            .unwrap()
    );
    let mut conflict = plan.clone();
    conflict["verification"] = json!([["false"]]);
    assert!(s
        .write(|db| plan::accept_tx(db, "t", "channel", "key", &conflict))
        .is_err());
    s.read(|db| {
        let (role, scope): (String, String) =
            db.query_row("SELECT role,scope_json FROM runtime_nodes", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        assert_eq!(role, "worker-backend-dev");
        assert_eq!(scope, "[\"src\"]");
        Ok(())
    })
    .unwrap();
}
#[test]
fn routing_previews_are_retained_not_recomputed_on_exact_replay() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING'", [])?;
        Ok(())
    })
    .unwrap();
    let plan = proposal();
    let hints = json!([{"node":"n","model":"auto","tier_now":"standard","work_kind":"implementation_backend","preview_status":"auto"}]);
    let first = s
        .write(|db| plan::accept_routed_tx(db, "t", "channel", "key", &plan, &hints))
        .unwrap();
    assert_eq!(first["model_hints"], hints);
    let replay = s
        .write(|db| plan::accept_routed_tx(db, "t", "channel", "key", &plan, &json!([])))
        .unwrap();
    assert_eq!(first, replay);
    let mut conflict = plan.clone();
    conflict["tasks"][0]["objective"] = json!("Other work");
    assert!(s
        .write(|db| plan::accept_routed_tx(db, "t", "channel", "key", &conflict, &hints))
        .is_err());
}
#[test]
fn graph_scope_roles_and_argv_reject_unsafe_or_incomplete_plans() {
    let base = proposal();
    for field in ["id", "role", "objective", "scope", "dependencies"] {
        let mut p = base.clone();
        p["tasks"][0].as_object_mut().unwrap().remove(field);
        assert!(plan::validate(&p).is_err(), "{field}");
    }
    for scope in [
        "/tmp",
        "../x",
        "x/../y",
        "x\\y",
    ] {
        let mut p = base.clone();
        p["tasks"][0]["scope"] = json!([scope]);
        assert!(plan::validate(&p).is_err(), "{scope}");
    }
    // Cortex private state, Git control files and credential files may be named
    // (owner decision 2026-11-25): a scope grants no write there; each write is
    // decided by the runtime.
    for scope in [".cortex/x", ".cortex-dev/x", ".git/qa-note", ".cortex-dev-owner.json", ".env"] {
        let mut p = base.clone();
        p["tasks"][0]["scope"] = json!([scope]);
        assert!(plan::validate(&p).is_ok(), "{scope}");
    }
    for role in [
        "code-reviewer",
        "build-verification",
        "senior-consultant",
        "not-a-role",
    ] {
        let mut p = base.clone();
        p["tasks"][0]["role"] = json!(role);
        assert!(plan::validate(&p).is_err(), "{role}");
    }
    let mut model = base.clone();
    model["tasks"][0]["model"] = json!("codex-acp/example");
    assert!(plan::validate(&model).is_err());
    model["tasks"][0]["model_reason"] = json!("Advisory recommendation only.");
    assert!(plan::validate(&model).is_ok());
    let mut p = base.clone();
    p["tasks"][0]["dependencies"] = json!(["missing"]);
    assert!(plan::validate(&p).is_err());
    p["tasks"][0]["dependencies"] = json!(["n"]);
    assert!(plan::validate(&p).is_err());
    p = base.clone();
    let duplicate = p["tasks"][0].clone();
    p["tasks"].as_array_mut().unwrap().push(duplicate);
    assert!(plan::validate(&p).is_err());
    for argv in [
        json!([]),
        json!(["true", "&&", "false"]),
        json!(["echo", "$(x)"]),
        json!(["echo", "x\0y"]),
        json!("true"),
        json!([1]),
    ] {
        assert!(plan::verification(&json!([argv])).is_err());
    }
    assert!(plan::verification(&json!([])).is_err());
    assert!(plan::verification(&json!([["env", "A=value", "true"]])).is_ok());
}

#[test]
fn a_plan_whose_verification_checks_nothing_is_refused() {
    let base = proposal();
    assert!(plan::validate(&base).is_ok());
    for argv in [
        json!(["true"]),
        json!([":"]),
        json!(["echo", "ok"]),
        json!(["sh", "-c", "exit 0"]),
        json!(["env", "A=1", "true"]),
    ] {
        let mut p = base.clone();
        p["verification"] = json!([argv]);
        assert_eq!(
            plan::validate(&p).unwrap_err().code(),
            "verification_trivial_command",
            "{argv}"
        );
        // One real command does not excuse another that checks nothing.
        p["verification"] = json!([["cargo", "test"], argv]);
        assert_eq!(
            plan::validate(&p).unwrap_err().code(),
            "verification_trivial_command",
            "{argv}"
        );
    }
}
/// A read-only assertion is a real check for a plan that creates files: a file
/// test or a quiet search exits nonzero when the result is wrong.
#[test]
fn a_plan_may_verify_with_file_tests_and_quiet_searches() {
    let mut p = proposal();
    p["verification"] = json!([
        ["test", "-f", "notes/a.txt"],
        ["grep", "-qx", "alpha", "notes/a.txt"]
    ]);
    assert!(plan::validate(&p).is_ok());
    p["verification"] = json!([["grep", "alpha", "notes/a.txt"]]);
    assert_eq!(
        plan::validate(&p).unwrap_err().code(),
        "verification_trivial_command"
    );
}
#[test]
fn a_replan_keeps_outcomes_by_id_and_sees_them_verbatim() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    planning(&s);
    let mut first = proposal();
    first["outcomes"] = json!([{"id":"work","statement":"The work is done"},
        {"id":"push","statement":"The changes are pushed to GitHub","deferred":"After the work"}]);
    s.write(|db| plan::accept_tx(db, "t", "c", "k1", &first)).unwrap();
    // The recovery capsule (and so task_get_context) lists the outcomes in force verbatim.
    let capsule = s.read(|db| cortex_core::recovery_context::snapshot(db, "t")).unwrap();
    assert_eq!(capsule["plan_summary"]["outcomes"], first["outcomes"]);
    assert!(capsule["plan_summary"]["outcomes_note"].as_str().unwrap().contains("only {\"id\""));
    // A replan that names the kept outcomes by id alone is accepted with the recorded statements.
    planning(&s);
    let mut by_id = proposal();
    by_id["outcomes"] = json!([{"id":"work"},{"id":"push","deferred":"Still after the work"}]);
    assert!(plan::validate(&by_id).is_ok());
    s.write(|db| plan::accept_tx(db, "t", "c", "k2", &by_id)).unwrap();
    let kept = s.read(|db| plan::task_outcomes(db, "t")).unwrap().unwrap();
    assert_eq!(kept[0]["statement"], "The work is done");
    assert_eq!(kept[1]["statement"], "The changes are pushed to GitHub");
    assert_eq!(kept[1]["deferred"], "Still after the work");
    // A dropped or reworded outcome is refused, and the missing ones are named verbatim.
    planning(&s);
    let mut reworded = proposal();
    reworded["outcomes"] = json!([{"id":"work"},{"id":"push","statement":"The push is prepared","deferred":"Later"}]);
    assert_eq!(
        s.write(|db| plan::accept_tx(db, "t", "c", "k3", &reworded)).unwrap_err().code(),
        "plan_outcome_dropped"
    );
    let stored = s
        .read(|db| {
            let raw: String = db.query_row("SELECT plan_json FROM runtime_tasks WHERE task_id='t'", [], |r| r.get(0))?;
            Ok(serde_json::from_str::<Value>(&raw)?)
        })
        .unwrap();
    assert_eq!(
        plan::dropped_outcomes(&stored, &reworded),
        vec![json!({"id":"push","statement":"The changes are pushed to GitHub","deferred":"Still after the work"})]
    );
    // An id-only entry that names no outcome in force is refused.
    let mut unknown = by_id.clone();
    unknown["outcomes"] = json!([{"id":"work"},{"id":"push","deferred":"Later"},{"id":"docs","deferred":"Later"}]);
    assert_eq!(
        s.write(|db| plan::accept_tx(db, "t", "c", "k4", &unknown)).unwrap_err().code(),
        "plan_outcome_statement_required"
    );
    assert_eq!(plan::unknown_references(Some(&stored), &unknown), vec![json!("docs")]);
    // A first plan has nothing to refer to: an id-only outcome is refused.
    TaskMachine::new(&s).create("u", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='u'", [])?;
        Ok(())
    })
    .unwrap();
    let mut fresh = proposal();
    fresh["outcomes"] = json!([{"id":"work"}]);
    assert_eq!(
        s.write(|db| plan::accept_tx(db, "u", "c", "k1", &fresh)).unwrap_err().code(),
        "plan_outcome_statement_required"
    );
    // A steering amendment may also keep an outcome by id alone.
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='t'", [])?;
        Ok(())
    })
    .unwrap();
    let raw: String = s
        .read(|db| Ok(db.query_row("SELECT plan_json FROM runtime_tasks WHERE task_id='t'", [], |r| r.get(0))?))
        .unwrap();
    let mut editable = plan::steerable_plan(&serde_json::from_str(&raw).unwrap()).unwrap();
    editable["outcomes"] = json!([{"id":"work"},{"id":"push","deferred":"Still after the work"}]);
    let amend = json!({"expected_plan":format!("{:x}", sha2::Sha256::digest(raw.as_bytes())),"plan":editable});
    s.write(|db| plan::amend_active_tx(db, "t", &amend)).unwrap();
    let kept = s.read(|db| plan::task_outcomes(db, "t")).unwrap().unwrap();
    assert_eq!(kept[0]["statement"], "The work is done");
}
#[test]
fn repeated_identical_outcome_refusals_are_kept_by_the_runtime_instead_of_spinning() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    planning(&s);
    let mut first = proposal();
    first["outcomes"] = json!([{"id":"work","statement":"The work is done"},
        {"id":"push","statement":"The changes are pushed to GitHub","deferred":"After the work"}]);
    s.write(|db| plan::accept_tx(db, "t", "c", "k1", &first)).unwrap();
    planning(&s);
    // The coordinator keeps dropping `push`.
    let narrowed = proposal();
    for attempt in 1..=plan::OUTCOME_REFUSAL_LIMIT {
        let key = format!("r{attempt}");
        assert_eq!(
            s.write(|db| plan::accept_tx(db, "t", "c", &key, &narrowed)).unwrap_err().code(),
            "plan_outcome_dropped"
        );
        let relieved = s
            .write(|db| plan::refused_outcomes_tx(db, "t", "c", &key, &narrowed))
            .unwrap();
        if attempt < plan::OUTCOME_REFUSAL_LIMIT {
            assert!(relieved.is_none(), "attempt {attempt}");
        } else {
            let relieved = relieved.unwrap();
            assert_eq!(relieved["state"], "PLAN_READY");
            assert_eq!(relieved["outcomes_kept_by_runtime"], json!(["push"]));
        }
    }
    // The kept outcome stays owed, verbatim, and the loop break is recorded.
    let kept = s.read(|db| plan::task_outcomes(db, "t")).unwrap().unwrap();
    let push = kept.iter().find(|o| o["id"] == "push").unwrap();
    assert_eq!(push["statement"], "The changes are pushed to GitHub");
    assert!(push["deferred"].as_str().unwrap().starts_with("Kept by the runtime"));
    assert_eq!(push["deferred_by"], "runtime");
    let capsule = s.read(|db| cortex_core::recovery_context::snapshot(db, "t")).unwrap();
    let shown = capsule["plan_summary"]["outcomes"].as_array().unwrap().iter().find(|o| o["id"] == "push").cloned().unwrap();
    assert_eq!(shown["deferred_by"], "runtime");
    // A runtime-deferred outcome still blocks completion: a final result that leaves it
    // not done or out is refused; only the owner's close as is lets it stay not done.
    use cortex_core::lifecycle::reconciliation::{final_outcomes, goal_met};
    let not_done = json!([{"id":"work","status":"done","evidence":"report r1"},{"id":"push","status":"not_done"}]);
    let without = json!([{"id":"work","status":"done","evidence":"report r1"}]);
    s.read(|db| {
        assert_eq!(final_outcomes(db, "t", &not_done, false).unwrap_err().code(), "completion_goal_not_met");
        assert_eq!(final_outcomes(db, "t", &without, false).unwrap_err().code(), "final_outcomes_required");
        assert!(final_outcomes(db, "t", &not_done, true).is_ok());
        assert!(!goal_met(db, "t", &json!({"outcomes":not_done}))?);
        assert!(goal_met(db, "t", &json!({"outcomes":not_done,"owner_closed_as_is":true}))?);
        Ok(())
    }).unwrap();
    // A later plan that still defers it keeps the runtime mark; a copied mark is ignored.
    planning(&s);
    let mut later = proposal();
    later["outcomes"] = json!([{"id":"work"},{"id":"push","deferred":"Next stage","deferred_by":"runtime"}]);
    s.write(|db| plan::accept_tx(db, "t", "c", "later", &later)).unwrap();
    let kept = s.read(|db| plan::task_outcomes(db, "t")).unwrap().unwrap();
    assert_eq!(kept.iter().find(|o| o["id"] == "push").unwrap()["deferred_by"], "runtime");
    assert!(kept.iter().find(|o| o["id"] == "work").unwrap().get("deferred_by").is_none());
    let broken: i64 = s
        .read(|db| Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='LOOP_BROKEN'
            AND json_extract(payload_json,'$.kind')='plan_outcome_refusal'", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(broken, 1);
}
#[test]
fn a_refused_outcome_plan_names_each_item_to_fix() {
    let mut p = proposal();
    p["outcomes"] = json!([{"id":"work","statement":"The work is done"},{"id":"push","statement":"Pushed"},
        {"id":"docs","statement":"Docs","deferred":" "},{"id":"both","statement":"Both","deferred":"later"}]);
    p["tasks"][0]["delivers"] = json!(["work", "both", "ghost"]);
    let d = plan::outcome_diagnosis(&p);
    assert_eq!(d["uncovered"], json!(["push"]));
    assert_eq!(d["unknown_delivers"], json!([{"task":"n","id":"ghost"}]));
    assert_eq!(d["delivered_and_deferred"], json!(["both"]));
    assert_eq!(d["deferral_reason_missing"], json!(["docs"]));
    assert_eq!(d["defined_outcome_ids"], json!(["work", "push", "docs", "both"]));
}
#[test]
fn owed_verification_is_visible_and_appended_by_the_runtime_never_refused() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    planning(&s);
    s.write(|db| plan::accept_tx(db, "t", "c", "k1", &proposal())).unwrap();
    let argv = json!(["cargo", "test", "-p", "owed"]);
    s.write(|db| {
        cortex_core::lifecycle::event_tx(db, "t", "VERIFICATION_FAILED", json!({"revision":1,
            "commands":[argv],"unresolved_commands":[argv],
            "checks":[{"argv":argv,"exit_code":101,"complete":true,"receipt_ref":"r1"}]}))?;
        db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'", [])?;
        Ok(())
    })
    .unwrap();
    // The coordinator reads the owed argv verbatim in the capsule's plan summary.
    let capsule = s.read(|db| cortex_core::recovery_context::snapshot(db, "t")).unwrap();
    assert_eq!(capsule["plan_summary"]["required_verification"], json!([{"argv":argv,"reason":"failed_check"}]));
    assert!(capsule["plan_summary"]["required_verification_note"].as_str().unwrap().contains("appends"));
    // A plan that leaves it out is accepted with the command appended and the event recorded.
    let accepted = s.write(|db| plan::accept_tx(db, "t", "c", "k2", &proposal())).unwrap();
    assert_eq!(accepted["verification_kept_by_runtime"], json!([{"argv":argv,"reason":"failed_check"}]));
    let (stored, events): (String, i64) = s
        .read(|db| Ok((
            db.query_row("SELECT json_extract(plan_json,'$.verification') FROM runtime_tasks WHERE task_id='t'", [], |r| r.get(0))?,
            db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='PLAN_VERIFICATION_KEPT'", [], |r| r.get(0))?,
        )))
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), json!([["cargo", "test"], argv]));
    assert_eq!(events, 1);
}
