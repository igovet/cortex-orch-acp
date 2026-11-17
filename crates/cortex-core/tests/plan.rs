mod support;
use cortex_core::lifecycle::{plan, TaskMachine};
use serde_json::{json, Value};
use support::Project;
fn proposal() -> Value {
    json!({"tasks":[{"id":"n","role":"backend-dev","objective":"Work","dependencies":[],"scope":["./src//","src"],"model":"auto"}],"verification":[["cargo","test"]]})
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
        ".cortex/x",
        ".cortex-dev/x",
        ".git/config",
        "x\\y",
        ".cortex-dev-owner.json",
    ] {
        let mut p = base.clone();
        p["tasks"][0]["scope"] = json!([scope]);
        assert!(plan::validate(&p).is_err(), "{scope}");
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
        assert_eq!(plan::validate(&p).unwrap_err().code(), "verification_trivial_command", "{argv}");
        // One real command does not excuse another that checks nothing.
        p["verification"] = json!([["cargo", "test"], argv]);
        assert_eq!(plan::validate(&p).unwrap_err().code(), "verification_trivial_command", "{argv}");
    }
}
