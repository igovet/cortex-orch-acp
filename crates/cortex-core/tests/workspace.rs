mod support;
use cortex_core::{workspace::create_draft, Store};
use serde_json::{json, Value};
use support::{count, Project};

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
fn command() -> Value {
    json!({"protocol":{"major":1,"minor":8},"requestId":"create","projectId":"p","actorId":"owner",
        "operation":"tasks.create","target":"p","expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":"Новая задача\n😀","checkout":{"kind":"main"}}})
}
#[test]
fn native_draft_commits_options_receipt_and_outbox_without_a_runtime_task() {
    let (_p, s) = setup();
    let c = command();
    let first = create_draft(&s, "p", "owner", &c).unwrap();
    assert!(first.fresh);
    assert_eq!(first.receipt["outcome"], "recorded");
    workspace_protocol::validate("Receipt", &first.receipt).unwrap();
    assert_eq!(count(&s, "workspace_tasks"), 1);
    assert_eq!(count(&s, "workspace_requests"), 1);
    assert_eq!(count(&s, "workspace_outbox"), 2);
    assert_eq!(count(&s, "runtime_tasks"), 0);
    let replay = create_draft(&s, "p", "owner", &c).unwrap();
    assert!(!replay.fresh);
    assert_eq!(replay.receipt, first.receipt);
    assert_eq!(count(&s, "workspace_outbox"), 2);
    s.read(|db| {
        let options: String =
            db.query_row("SELECT options_json FROM workspace_task_options", [], |r| {
                r.get(0)
            })?;
        assert_eq!(
            serde_json::from_str::<Value>(&options)?,
            json!({"checkout":{"kind":"main"}})
        );
        Ok(())
    })
    .unwrap();
    let mut changed = c.clone();
    changed["payload"]["description"] = json!("different");
    assert_eq!(
        create_draft(&s, "p", "owner", &changed).unwrap().receipt["outcome"],
        "payload_conflict"
    );
    assert_eq!(count(&s, "workspace_tasks"), 1);
}
#[test]
fn unauthorized_or_unsupported_writes_never_fall_through_or_change_data() {
    let (_p, s) = setup();
    assert!(create_draft(&s, "p", "forged", &command()).is_err());
    let mut c = command();
    c["payload"]["description"] = json!(" \n ");
    assert!(create_draft(&s, "p", "owner", &c).is_err());
    // A file that breaks a wire limit or rule is refused before anything is stored.
    for (n, bad) in [
        json!([{"name":"../a.txt","mimeType":"text/plain","data":"YQ=="}]),
        json!([{"name":"a\\b.txt","mimeType":"text/plain","data":"YQ=="}]),
        json!([{"name":"a.txt","mimeType":"text","data":"YQ=="}]),
        json!([{"name":"a.txt","mimeType":"text/plain","data":"YQ"}]),
        json!([{"name":"a.txt","mimeType":"text/plain","data":"YR=="}]),
        json!((0..9).map(|i| json!({"name":format!("{i}.txt"),"mimeType":"text/plain","data":"YQ=="})).collect::<Vec<_>>()),
        json!([{"name":"big.bin","mimeType":"application/octet-stream","data":"A".repeat(5_592_408)}]),
    ]
    .into_iter()
    .enumerate()
    {
        c = command();
        c["requestId"] = json!(format!("bad-{n}"));
        c["payload"]["attachments"] = bad;
        let refused = create_draft(&s, "p", "owner", &c).err().map(|e| e.code());
        // The count rule is also in the wire schema, which refuses first.
        assert!(matches!(refused, Some("invalid_attachments" | "invalid_workspace_request")), "case {n}: {refused:?}");
    }
    assert_eq!(count(&s, "workspace_tasks"), 0);
    assert_eq!(count(&s, "workspace_requests"), 0);
    assert_eq!(count(&s, "workspace_outbox"), 0);
}
#[test]
fn request_failure_rolls_back_draft_options_and_public_events() {
    let (_p, s) = setup();
    s.write(|db| {
        db.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON workspace_requests BEGIN SELECT RAISE(ABORT,'injected'); END;")?;
        Ok(())
    }).unwrap();
    assert!(create_draft(&s, "p", "owner", &command()).is_err());
    for table in [
        "workspace_tasks",
        "workspace_task_options",
        "workspace_requests",
        "workspace_outbox",
    ] {
        assert_eq!(count(&s, table), 0);
    }
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT version FROM workspace_projects", [], |r| r
                .get::<_, i64>(0))?,
            1
        );
        Ok(())
    })
    .unwrap();
}
#[test]
fn concurrent_duplicate_requests_have_one_draft_and_identical_receipts() {
    let (_p, s) = setup();
    let s = std::sync::Arc::new(s);
    let joins: Vec<_> = (0..8)
        .map(|_| {
            let s = s.clone();
            std::thread::spawn(move || create_draft(&s, "p", "owner", &command()).unwrap().receipt)
        })
        .collect();
    let receipts: Vec<_> = joins.into_iter().map(|j| j.join().unwrap()).collect();
    assert!(receipts.iter().all(|r| r == &receipts[0]));
    assert_eq!(count(&s, "workspace_tasks"), 1);
    assert_eq!(count(&s, "workspace_requests"), 1);
}
#[test]
fn receipts_are_digest_compatible_with_the_frozen_adapter_digest() {
    let (_p, s) = setup();
    let c = command();
    create_draft(&s, "p", "owner", &c).unwrap();
    // Frozen once from the retired adapter's canonical JSON digest
    // (sorted keys, compact separators, non-ASCII preserved) of `command()`.
    let frozen = "cb643fa70f8c2e0e5b1c3dbc5e5554043b159176aa54b2995d68d09099bff7b0";
    s.read(|db| {
        let digest: String =
            db.query_row("SELECT digest FROM workspace_requests", [], |r| r.get(0))?;
        assert_eq!(digest, frozen);
        Ok(())
    })
    .unwrap();
    // A pre-existing adapter receipt replays unchanged rather than creating a draft.
    let prior = json!({"requestId":"prior","outcome":"recorded","target":"old-task","entityVersion":"1","recordedAt":"now"});
    s.write(|db| {
        let mut old = c.clone();
        old["requestId"] = json!("prior");
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&old)?));
        db.execute(
            "INSERT INTO workspace_requests VALUES ('p','owner','prior','tasks.create',?,?)",
            (digest, prior.to_string()),
        )?;
        Ok(())
    })
    .unwrap();
    let mut old = c;
    old["requestId"] = json!("prior");
    assert_eq!(create_draft(&s, "p", "owner", &old).unwrap().receipt, prior);
    assert_eq!(count(&s, "workspace_tasks"), 1);
}
#[test]
fn invalid_target_is_receipted_without_creating_a_task() {
    let (_p, s) = setup();
    let mut c = command();
    c["target"] = json!("another-project");
    assert_eq!(
        create_draft(&s, "p", "owner", &c).unwrap().receipt["outcome"],
        "invalid_request"
    );
    assert_eq!(
        create_draft(&s, "p", "owner", &c).unwrap().receipt["target"],
        "another-project"
    );
    assert_eq!(count(&s, "workspace_tasks"), 0);
    assert_eq!(count(&s, "workspace_requests"), 1);
}

fn native_task(s: &Store, task: &str) -> cortex_core::ownership::OwnerEpoch {
    let owner = cortex_core::lifecycle::TaskMachine::new(s)
        .create(task, "Goal", "runtime-owner")
        .unwrap();
    s.write(|db| {
        cortex_core::lifecycle::event_tx(
            db,
            task,
            "TASK_CREATED",
            json!({"execution_host":"rust"}),
        )?;
        Ok(())
    })
    .unwrap();
    owner
}
#[test]
fn native_binding_is_idempotent_projects_state_and_rejects_wrong_identities() {
    use cortex_core::{projections::ProjectionReader, workspace::bind_native_execution, OpenMode};
    use workspace_protocol::{
        core::{CoreQuery, Target},
        read::QueryReply,
    };
    let (p, s) = setup();
    let draft = create_draft(&s, "p", "owner", &command()).unwrap().receipt["target"]
        .as_str()
        .unwrap()
        .to_owned();
    let owner = native_task(&s, "runtime");
    assert!(bind_native_execution(&s, "p", "other", &draft, "runtime", &owner).is_err());
    let execution = bind_native_execution(&s, "p", "owner", &draft, "runtime", &owner).unwrap();
    assert_eq!(
        bind_native_execution(&s, "p", "owner", &draft, "runtime", &owner).unwrap(),
        execution
    );
    assert_eq!(count(&s, "workspace_executions"), 1);
    assert_eq!(count(&s, "acp_host_sessions"), 1);
    let r = ProjectionReader::new(
        cortex_core::Store::open(p.key(), OpenMode::LiveReadOnly).unwrap(),
        "p".into(),
        "owner".into(),
    )
    .unwrap();
    let QueryReply::Task(t) = r
        .query(CoreQuery::TaskGet(Target { id: draft.clone() }))
        .unwrap()
        .data
    else {
        panic!()
    };
    assert_eq!(t.execution_id.as_deref(), Some(execution.as_str()));
    assert_ne!(t.execution_state, "unknown");
    let mut stale = owner.clone();
    stale.epoch += 1;
    assert!(bind_native_execution(&s, "p", "owner", &draft, "runtime", &stale).is_err());
    let mut second = command();
    second["requestId"] = json!("second");
    let other = create_draft(&s, "p", "owner", &second).unwrap().receipt["target"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(bind_native_execution(&s, "p", "owner", &other, "runtime", &owner).is_err());
    let legacy = cortex_core::lifecycle::TaskMachine::new(&s)
        .create("legacy", "Legacy", "runtime-owner")
        .unwrap();
    assert_eq!(
        bind_native_execution(&s, "p", "owner", &other, "legacy", &legacy)
            .unwrap_err()
            .code(),
        "native_binding_evidence_invalid"
    );
}
/// Live D3 regression: every draft saved with a chosen model was refused with
/// `native_start_options_unavailable`. Saved settings now start, as in Python
/// `_dispatch_workspace_start`, and reach the coordinator as initial settings.
#[test]
fn saved_model_settings_start_and_record_initial_settings_and_access_mode() {
    use cortex_core::workspace::{draft_settings, start_native};
    let (_p, s) = setup();
    let start = |target: &str, request: &str| {
        json!({"protocol":{"major":1,"minor":8},"requestId":request,"projectId":"p","actorId":"owner",
            "operation":"tasks.start","target":target,"expectedVersion":"1","ownerEpoch":null,"payload":{}})
    };
    for (n, model) in ["xr_b067806096f03b639ac4fcf2", "gpt-6.1-sol"]
        .into_iter()
        .enumerate()
    {
        let settings = json!({"model":model,"effort":"high","mode":"auto","language":"ru"});
        let mut c = command();
        c["requestId"] = json!(format!("create-{n}"));
        c["payload"]["settings"] = settings.clone();
        let draft = create_draft(&s, "p", "owner", &c).unwrap().receipt["target"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            draft_settings(&s, "p", "owner", &draft).unwrap(),
            Some(settings.clone())
        );
        let started = start_native(&s, "p", "owner", &start(&draft, &format!("start-{n}")))
            .unwrap_or_else(|e| panic!("saved settings refused: {}", e.code()));
        assert_eq!(started["receipt"]["outcome"], "recorded");
        let task = started["task"].as_str().unwrap().to_owned();
        s.read(|db| {
            let recorded: String = db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_TASK_SETTINGS'",
                [&task], |r| r.get(0))?;
            assert_eq!(
                serde_json::from_str::<Value>(&recorded)?,
                json!({"settings":settings,"activation":"initial"})
            );
            assert_eq!(cortex_core::access::current(db, &task)?, "auto");
            let (origin, session): (String, String) = db.query_row(
                "SELECT json_extract(payload_json,'$.origin'),json_extract(payload_json,'$.session_id') FROM runtime_events
                 WHERE task_id=? AND type='NATIVE_ACCESS_SWITCHED'", [&task], |r| Ok((r.get(0)?, r.get(1)?)))?;
            assert_eq!(origin, "acp_pre_task_selection");
            let bound: String = db.query_row("SELECT e.acp_session_id FROM workspace_executions e WHERE e.runtime_task_id=?",
                [&task], |r| r.get(0))?;
            assert_eq!(session, bound);
            Ok(())
        })
        .unwrap();
    }
    // A malformed saved choice is refused with its own reason before any effect.
    let mut c = command();
    c["requestId"] = json!("create-bad");
    let draft = create_draft(&s, "p", "owner", &c).unwrap().receipt["target"]
        .as_str()
        .unwrap()
        .to_owned();
    s.write(|db| {
        db.execute("UPDATE workspace_task_options SET options_json=? WHERE task_id=?",
            (json!({"checkout":{"kind":"main"},"settings":{"model":"gpt-6.1-sol","effort":"high","mode":"root","language":"ru"}}).to_string(), &draft))?;
        Ok(())
    })
    .unwrap();
    let tasks = count(&s, "runtime_tasks");
    assert_eq!(
        draft_settings(&s, "p", "owner", &draft).unwrap_err().code(),
        "native_task_settings_invalid"
    );
    assert_eq!(
        start_native(&s, "p", "owner", &start(&draft, "start-bad"))
            .unwrap_err()
            .code(),
        "native_task_settings_invalid"
    );
    assert_eq!(count(&s, "runtime_tasks"), tasks);
    assert_eq!(draft_settings(&s, "p", "owner", "missing").unwrap(), None);
}
fn start_command(target: &str, request: &str) -> Value {
    json!({"protocol":{"major":1,"minor":8},"requestId":request,"projectId":"p","actorId":"owner",
        "operation":"tasks.start","target":target,"expectedVersion":"1","ownerEpoch":null,"payload":{}})
}
/// Live D3 remainder: a draft saved with a separate worktree is refused with its
/// own reason (the native host runs sessions only in the project folder), before
/// any runtime task, binding or event exists.
#[test]
fn worktree_draft_is_refused_with_its_own_reason_before_any_effect() {
    use cortex_core::workspace::start_native;
    let (_p, s) = setup();
    let mut c = command();
    c["payload"]["checkout"] = json!({"kind":"worktree"});
    let draft = create_draft(&s, "p", "owner", &c).unwrap().receipt["target"]
        .as_str()
        .unwrap()
        .to_owned();
    let outbox = count(&s, "workspace_outbox");
    assert_eq!(
        start_native(&s, "p", "owner", &start_command(&draft, "start-worktree"))
            .unwrap_err()
            .code(),
        "native_worktree_unavailable"
    );
    for table in ["runtime_tasks", "workspace_executions", "runtime_events"] {
        assert_eq!(count(&s, table), 0, "{table}");
    }
    assert_eq!(count(&s, "workspace_outbox"), outbox);
}
fn git(root: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "core.hooksPath=/dev/null", "-c", "user.name=Test",
               "-c", "user.email=test@example.invalid", "-C"])
        .arg(root)
        .args(args)
        .output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().into()
}

fn worktree_draft(s: &Store) -> String {
    let mut c = command();
    c["payload"]["description"] = json!("Worktree // run");
    c["payload"]["checkout"] = json!({"kind":"worktree"});
    create_draft(s, "p", "owner", &c).unwrap().receipt["target"].as_str().unwrap().into()
}

#[test]
fn worktree_start_materializes_only_committed_head_and_replays_exactly() {
    use cortex_core::workspace::{start_native_worktree, worktrees::resolve};
    use std::fs;
    let (p, s) = setup();
    let machine = Project::new();
    git(&p.0, &["init"]);
    fs::write(p.0.join("file"), "committed").unwrap();
    fs::write(p.0.join(".gitattributes"), "file filter=external\n").unwrap();
    git(&p.0, &["add", "file", ".gitattributes"]);
    git(&p.0, &["commit", "-m", "base"]);
    let base = git(&p.0, &["rev-parse", "HEAD"]);
    fs::write(p.0.join("file"), "staged").unwrap();
    git(&p.0, &["add", "file"]);
    fs::write(p.0.join("file"), "unstaged").unwrap();
    fs::write(p.0.join("untracked"), "local only").unwrap();

    // A required and broken smudge demonstrates that checkout doesn't run filters.
    git(&p.0, &["config", "filter.external.smudge", "false"]);
    git(&p.0, &["config", "filter.external.required", "true"]);
    let draft = worktree_draft(&s);
    let cmd = start_command(&draft, "worktree");
    let first = start_native_worktree(&s, "p", "owner", &cmd, &machine.0).unwrap();
    let task = first["task"].as_str().unwrap();
    let owner = cortex_core::ownership::OwnerEpoch {
        owner: "owner".into(),
        epoch: first["owner"]["epoch"].as_i64().unwrap(),
    };
    let checkout = resolve(&s, task, &owner).unwrap();
    let w = checkout.worktree.as_ref().unwrap();
    assert_eq!(checkout.logical.root(), p.0);
    assert!(w.path.starts_with(&machine.0));
    assert!(w.branch.starts_with("cw/worktree-run-"));
    assert_eq!(w.branch.rsplit('-').next().unwrap().len(), 12);
    assert_eq!(w.base_sha, base);
    assert_eq!(fs::read_to_string(w.path.join("file")).unwrap(), "committed");
    assert!(!w.path.join("untracked").exists());
    assert_eq!(fs::read_to_string(p.0.join("file")).unwrap(), "unstaged");
    assert_eq!(git(&p.0, &["show", ":file"]), "staged");
    let outbox = count(&s, "workspace_outbox");
    let replay = start_native_worktree(&s, "p", "owner", &cmd, &p.0).unwrap();
    assert_eq!(replay["fresh"], false);
    assert_eq!(first["receipt"], replay["receipt"]);
    assert_eq!(count(&s, "workspace_outbox"), outbox);
    assert_eq!(count(&s, "task_workspaces"), 1);
    let mut changed = cmd;
    changed["expectedVersion"] = json!("2");
    assert_eq!(start_native_worktree(&s, "p", "owner", &changed, &machine.0)
        .unwrap_err().code(), "payload_conflict");
    assert_eq!(resolve(&s, task, &cortex_core::ownership::OwnerEpoch {
        owner: "owner".into(), epoch: owner.epoch + 1
    }).unwrap_err().code(), "stale_owner");
    git(&w.path, &["update-ref", "--no-deref", "HEAD", &base]);
    assert_eq!(resolve(&s, task, &owner).unwrap_err().code(), "worktree_unavailable");
}

#[test]
fn worktree_start_refuses_no_git_unborn_and_existing_path_without_main_fallback() {
    use cortex_core::workspace::start_native_worktree;
    use std::fs;
    let (p, s) = setup();
    let machine = Project::new();
    let draft = worktree_draft(&s);
    let start = |request: &str| start_native_worktree(&s, "p", "owner",
        &start_command(&draft, request), &machine.0);
    assert_eq!(start("not-git").unwrap_err().code(), "worktree_git_required");
    git(&p.0, &["init"]);
    assert_eq!(start("unborn").unwrap_err().code(), "worktree_head_unborn");
    git(&p.0, &["commit", "--allow-empty", "-m", "base"]);
    assert_eq!(start("not-git").unwrap_err().code(), "worktree_git_required");

    use sha2::{Digest, Sha256};
    let hex = |v: &[u8]| format!("{:x}", Sha256::digest(v));
    let repo = hex(p.0.join(".git").as_os_str().as_encoded_bytes());
    let suffix = hex(draft.as_bytes());
    let collision = machine.0.join(&repo[..16]).join(format!("worktree-run-{}", &suffix[..12]));
    fs::create_dir_all(&collision).unwrap();
    fs::write(collision.join("keep"), "untouched").unwrap();
    assert_eq!(start("collision").unwrap_err().code(), "worktree_collision");
    assert_eq!(fs::read_to_string(collision.join("keep")).unwrap(), "untouched");
    assert_eq!(count(&s, "runtime_tasks"), 0);
    assert_eq!(count(&s, "task_workspaces"), 0);
}

#[test]
fn a_witnessed_worktree_survives_failed_admission_and_retries_without_new_git_effects() {
    use cortex_core::workspace::start_native_worktree;
    let (p, s) = setup();
    let machine = Project::new();
    git(&p.0, &["init"]);
    git(&p.0, &["commit", "--allow-empty", "-m", "base"]);
    let draft = worktree_draft(&s);
    let cmd = start_command(&draft, "one-start");
    s.write(|db| {
        db.execute_batch("CREATE TRIGGER fixture_fail_admit BEFORE INSERT ON workspace_executions
            BEGIN SELECT RAISE(ABORT, 'fixture injection'); END;")?;
        Ok(())
    }).unwrap();
    assert!(start_native_worktree(&s, "p", "owner", &cmd, &machine.0).is_err());
    assert_eq!(count(&s, "runtime_tasks"), 0);
    assert_eq!(count(&s, "task_workspaces"), 0);
    let witness = git(&p.0, &["worktree", "list", "--porcelain"]);
    assert_eq!(witness.lines().filter(|l| l.starts_with("worktree ")).count(), 2);
    assert_eq!(start_native_worktree(&s, "p", "owner", &start_command(&draft, "second-start"), &machine.0)
        .unwrap_err().code(), "worktree_uncertain");
    s.write(|db| {
        db.execute_batch("DROP TRIGGER fixture_fail_admit;")?;
        Ok(())
    }).unwrap();
    let result = start_native_worktree(&s, "p", "owner", &cmd, &p.0).unwrap();
    assert_eq!(result["fresh"], true);
    assert_eq!(git(&p.0, &["worktree", "list", "--porcelain"]), witness);
    assert_eq!(count(&s, "runtime_tasks"), 1);
    assert_eq!(count(&s, "task_workspaces"), 1);
}

/// Live N3: Request changes on a finished native task failed with
/// `task_not_resumable`. As in Python `policy.apply_steering`, an owner message
/// reopens a COMPLETED/FAILED task for a fresh plan; cancelled or owner-closed
/// tasks stay ended.
#[test]
fn owner_message_reopens_a_finished_task_but_not_a_cancelled_or_closed_one() {
    use cortex_core::workspace::{control, start_native};
    let (_p, s) = setup();
    let mut executions = Vec::new();
    for n in 0..3 {
        let mut c = command();
        c["requestId"] = json!(format!("create-{n}"));
        let draft = create_draft(&s, "p", "owner", &c).unwrap().receipt["target"]
            .as_str()
            .unwrap()
            .to_owned();
        let started = start_native(
            &s,
            "p",
            "owner",
            &start_command(&draft, &format!("start-{n}")),
        )
        .unwrap();
        executions.push((
            started["task"].as_str().unwrap().to_owned(),
            started["receipt"]["target"].as_str().unwrap().to_owned(),
        ));
    }
    let finish = |task: &str, state: &str| {
        s.write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET state=?,synthesis='Done.',blocker=? WHERE task_id=?",
                (state, (state == "CANCELLED").then_some("owner"), task),
            )?;
            Ok(())
        })
        .unwrap()
    };
    let prompt = |execution: &str, key: &str| {
        let request = control::canonical_request(
            json!({"projectId":"p","executionId":execution,"requestId":key,"action":"prompt",
                "text":"Please add a docstring"}),
            None,
        )
        .unwrap();
        control::admit_input(&s, "p", "owner", &request, None)
            .map(|_| control::finish(&s, "p", "owner", &request, true).unwrap())
    };
    let (completed, execution) = &executions[0];
    finish(completed, "COMPLETED");
    let receipt = prompt(execution, "changes").unwrap_or_else(|e| panic!("{}", e.code()));
    assert_eq!(receipt["state"], "PLANNING");
    s.read(|db| {
        let (synthesis, blocker): (Option<String>, Option<String>) = db.query_row(
            "SELECT synthesis,blocker FROM runtime_tasks WHERE task_id=?",
            [completed],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!((synthesis, blocker), (None, None));
        let reopened: String = db.query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED'",
            [completed],
            |r| r.get(0),
        )?;
        assert_eq!(
            serde_json::from_str::<Value>(&reopened)?,
            json!({"previous":"COMPLETED","revision":0})
        );
        let steering: i64 = db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='USER_STEERING'",
            [completed],
            |r| r.get(0),
        )?;
        assert_eq!(steering, 1);
        Ok(())
    })
    .unwrap();
    let (cancelled, execution) = &executions[1];
    finish(cancelled, "CANCELLED");
    assert_eq!(
        prompt(execution, "cancelled").unwrap_err().code(),
        "task_not_resumable"
    );
    // A completed task the owner already closed is not reopened by a late message.
    let (closed, execution) = &executions[2];
    finish(closed, "COMPLETED");
    s.write(|db| {
        cortex_core::lifecycle::event_tx(
            db,
            closed,
            "OWNER_TASK_RESOLVED",
            json!({"certified":true}),
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        prompt(execution, "closed").unwrap_err().code(),
        "task_not_resumable"
    );
    s.read(|db| {
        for task in [cancelled, closed] {
            let reopened: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE task_id=? AND type IN ('TASK_REOPENED','USER_STEERING')",
                [task],
                |r| r.get(0),
            )?;
            assert_eq!(reopened, 0);
        }
        Ok(())
    })
    .unwrap();
}
#[test]
fn explicit_owner_rework_is_versioned_and_keeps_the_closed_history() {
    use cortex_core::workspace::{control, start_native};
    let (p, s) = setup();
    for (index, state) in ["COMPLETED", "CANCELLED", "FAILED"].into_iter().enumerate() {
        let mut create = command();
        create["requestId"] = json!(format!("rework-draft-{index}"));
        let draft = create_draft(&s, "p", "owner", &create).unwrap().receipt["target"]
            .as_str().unwrap().to_owned();
        let start = start_native(&s, "p", "owner",
            &start_command(&draft, &format!("rework-start-{index}"))).unwrap();
        let task = start["task"].as_str().unwrap();
        let execution = start["receipt"]["target"].as_str().unwrap();
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state=?,revision=4,synthesis='Earlier result',
                blocker=? WHERE task_id=?", (state, if state == "COMPLETED" {None} else {Some("earlier blocker")},task))?;
            if state == "CANCELLED" {
                db.execute("INSERT INTO task_stopped VALUES (?,?)", (task, "now"))?;
            }
            cortex_core::lifecycle::event_tx(db, task, "OWNER_TASK_RESOLVED",
                json!({"certified": state == "COMPLETED"}))?;
            Ok(())
        }).unwrap();
        let b = s.read(|db| control::binding(db, p.0.to_str().unwrap(), "p", "owner", execution)).unwrap();
        let request = json!({"action":"rework","projectId":"p","taskId":draft,"executionId":execution,
            "requestId":"rework","text":"Please make a fresh plan.","expectedVersion":b.version.to_string(),
            "ownerEpoch":b.owner.epoch.to_string()});
        let run = |request: &Value, archived| control::rework(&s, "p", "owner", request, None,
            control::ReworkDisposition {
                archived, closed: false, integrated: false, target: None,
                history: json!({"acceptance":{"version":"previous"}}), worktree_root: &p.0,
            });
        let late = json!({"action":"prompt","executionId":execution,"requestId":"late","text":"Ordinary message"});
        assert_eq!(control::admit(&s, "p", "owner", &late).err().unwrap().code(), "task_not_resumable");
        assert_eq!(run(&request, true).unwrap_err().code(), "task_archived");
        let mut stale = request.clone();
        stale["expectedVersion"] = json!("0");
        assert_eq!(run(&stale, false).unwrap_err().code(), "stale_version");
        stale = request.clone();
        stale["ownerEpoch"] = json!("0");
        assert_eq!(run(&stale, false).unwrap_err().code(), "stale_epoch");
        let mut empty = request.clone();
        empty["text"] = json!("  ");
        assert_eq!(run(&empty, false).unwrap_err().code(), "invalid_message");
        let result = run(&request, false).unwrap();
        assert_eq!(result["state"], "PLANNING");
        assert_eq!(run(&request, false).unwrap(), result);
        assert_eq!(run(&empty, false).unwrap_err().code(), "payload_conflict");
        s.read(|db| {
            let (state_now, revision, synthesis, blocker): (String, i64, Option<String>,Option<String>) =
                db.query_row("SELECT state,revision,synthesis,blocker FROM runtime_tasks WHERE task_id=?",
                    [task], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            assert_eq!((state_now.as_str(), revision, synthesis, blocker), ("PLANNING",4,None,None));
            let raw: String = db.query_row("SELECT payload_json FROM runtime_events
                WHERE task_id=? AND type='TASK_REOPENED'",[task],|r|r.get(0))?;
            let event: Value = serde_json::from_str(&raw)?;
            assert_eq!(event["previous"], state);
            assert_eq!(event["origin"], "owner_rework");
            assert_eq!(event["closed"]["acceptance"]["version"], "previous");
            assert_eq!(event["closed"]["synthesis"], "Earlier result");
            assert!(!cortex_core::lifecycle::reconciliation::direct_answer_allowed(db, task)?);
            let count = |kind: &str| db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type=?",
                (task,kind),|r|r.get::<_,i64>(0));
            assert_eq!(count("OWNER_TASK_RESOLVED")?,1);
            assert_eq!(count("TASK_REOPENED")?,1);
            assert_eq!(count("USER_STEERING")?,1);
            assert!(!control::owner_resolution_current(db, task)?);
            let saved: String = db.query_row("SELECT text FROM host_transcript WHERE session_id=? AND kind='user_message'
                ORDER BY seq DESC LIMIT 1",[&b.session],|r|r.get(0))?;
            assert_eq!(saved, request["text"]);
            Ok(())
        }).unwrap();
    }
}

#[test]
fn unconfirmed_close_does_not_authorize_rework_until_the_exact_execution_is_reconciled() {
    use cortex_core::workspace::{control, start_native};
    let (p, s) = setup();
    let draft = create_draft(&s, "p", "owner", &command()).unwrap().receipt["target"]
        .as_str().unwrap().to_owned();
    let start = start_native(&s, "p", "owner", &start_command(&draft, "start")).unwrap();
    let task = start["task"].as_str().unwrap();
    let execution = start["receipt"]["target"].as_str().unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='COMPLETED' WHERE task_id=?", [task])?;
        db.execute("UPDATE workspace_starts SET state='unknown' WHERE execution_id=?", [execution])?;
        Ok(())
    }).unwrap();
    let root = p.0.to_str().unwrap();
    let project = || s.read(|db| cortex_core::projections::task_records(db, "p", root, &draft))
        .unwrap().unwrap();
    control::close_unconfirmed(&s, "p", "owner", &draft, execution, project().version).unwrap();
    let mut request = json!({"action":"rework","projectId":"p","taskId":draft,"executionId":execution,
        "requestId":"repair","text":"Please do this again.","expectedVersion":project().version,
        "ownerEpoch":start["owner"]["epoch"].as_i64().unwrap().to_string()});
    let run = |request: &Value| control::rework(&s, "p", "owner", request, None,
        control::ReworkDisposition {
            archived: false, closed: false, integrated: false, target: None,
            history: json!({}), worktree_root: &p.0,
        });
    assert_eq!(run(&request).unwrap_err().code(), "native_execution_binding_invalid");
    s.write(|db| {
        db.execute("UPDATE workspace_starts SET state='claimed' WHERE execution_id=?", [execution])?;
        Ok(())
    }).unwrap();
    assert!(project().warnings.iter().any(|w| w == "owner_closed_unconfirmed"));
    request["expectedVersion"] = json!(project().version);
    assert_eq!(run(&request).unwrap()["state"], "PLANNING");
    assert_eq!(project().phase, "planning");
    assert!(!project().warnings.iter().any(|w| w == "owner_closed_unconfirmed"));
    s.read(|db| {
        let history: i64 = db.query_row("SELECT count(*) FROM workspace_requests WHERE operation='tasks.close_unconfirmed'",
            [], |r| r.get(0))?;
        assert_eq!(history, 1);
        Ok(())
    }).unwrap();
}

/// Live N2: owner steering and Request changes text must read back as user
/// turns of the task conversation, in order, through the desktop history page.
#[test]
fn owner_messages_read_back_as_user_turns_in_the_task_history() {
    use cortex_core::{
        projections::ProjectionReader,
        workspace::{control, start_native},
        OpenMode,
    };
    use workspace_protocol::{
        core::{CoreQuery, Page},
        read::QueryReply,
    };
    let (p, s) = setup();
    let draft = create_draft(&s, "p", "owner", &command()).unwrap().receipt["target"]
        .as_str()
        .unwrap()
        .to_owned();
    let started = start_native(&s, "p", "owner", &start_command(&draft, "start")).unwrap();
    let (task, execution) = (
        started["task"].as_str().unwrap().to_owned(),
        started["receipt"]["target"].as_str().unwrap().to_owned(),
    );
    let prompt = |key: &str, text: &str| {
        let request = control::canonical_request(
            json!({"projectId":"p","executionId":execution,"requestId":key,"action":"prompt","text":text}),
            None,
        )
        .unwrap();
        control::admit_input(&s, "p", "owner", &request, None)
            .unwrap_or_else(|e| panic!("{}", e.code()));
        control::finish(&s, "p", "owner", &request, true).unwrap();
    };
    prompt("steer", "Also add a docstring");
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='COMPLETED',synthesis='Done.' WHERE task_id=?",
            [&task],
        )?;
        Ok(())
    })
    .unwrap();
    prompt("changes", "Please rename the helper");
    let reader = ProjectionReader::new(
        Store::open(p.key(), OpenMode::LiveReadOnly).unwrap(),
        "p".into(),
        "owner".into(),
    )
    .unwrap();
    let reply = reader
        .query(CoreQuery::History(Page {
            target: draft.clone(),
            cursor: None,
            limit: 100,
        }))
        .unwrap_or_else(|e| panic!("{}", e.code()));
    let QueryReply::History(page) = reply.data else {
        panic!("not a history page")
    };
    let said: Vec<_> = page
        .items
        .iter()
        .filter(|r| r.kind == "user")
        .filter_map(|r| r.text.clone())
        .collect();
    assert_eq!(said, ["Also add a docstring", "Please rename the helper"]);
}
#[test]
fn native_binding_failure_rolls_back_every_identity_and_event() {
    use cortex_core::workspace::bind_native_execution;
    let (_p, s) = setup();
    let draft = create_draft(&s, "p", "owner", &command()).unwrap().receipt["target"]
        .as_str()
        .unwrap()
        .to_owned();
    let owner = native_task(&s, "runtime");
    let outbox = count(&s, "workspace_outbox");
    s.write(|db| { db.execute_batch("CREATE TRIGGER fail_binding BEFORE INSERT ON workspace_executions BEGIN SELECT RAISE(ABORT,'injected'); END;")?; Ok(()) }).unwrap();
    assert!(bind_native_execution(&s, "p", "owner", &draft, "runtime", &owner).is_err());
    assert_eq!(count(&s, "acp_host_sessions"), 0);
    assert_eq!(count(&s, "workspace_executions"), 0);
    assert_eq!(count(&s, "workspace_outbox"), outbox);
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT execution_id FROM workspace_tasks", [], |r| r
                .get::<_, Option<String>>(0))?,
            None
        );
        Ok(())
    })
    .unwrap();
}
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfixture";
fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char)
            } else {
                out.push('=')
            }
        }
    }
    out
}
/// Draft files are checked, retained once as private bodies of the started task's
/// first source revision, and never copied into the public admission event.
#[test]
fn draft_attachments_are_retained_privately_with_the_started_task() {
    use cortex_core::{input::capture_from_wire, workspace::start_native};
    use std::os::unix::fs::PermissionsExt;
    let (p, s) = setup();
    let files = json!([
        {"name":"shot.png","mimeType":"image/png","data":b64(PNG)},
        {"name":"notes.txt","mimeType":"text/plain","data":b64(b"read me")},
    ]);
    let decoded = capture_from_wire(&files).unwrap();
    assert_eq!(decoded.blocks.len(), 2);
    assert_eq!(decoded.blocks[0].kind, "image");
    assert_eq!(decoded.blocks[0].body.as_deref(), Some(PNG));
    assert_eq!(decoded.blocks[1].body.as_deref(), Some(&b"read me"[..]));
    let mut c = command();
    c["payload"]["attachments"] = files;
    let draft = create_draft(&s, "p", "owner", &c).unwrap().receipt["target"]
        .as_str()
        .unwrap()
        .to_owned();
    let started = start_native(&s, "p", "owner", &start_command(&draft, "start")).unwrap();
    let task = started["task"].as_str().unwrap().to_owned();
    s.read(|db| {
        let rows: Vec<(String, Option<String>, Option<String>, String, i64, String)> = db
            .prepare("SELECT kind,mime,name,sha256,size_bytes,body_ref FROM source_blocks WHERE task_id=? AND availability='stored' ORDER BY block_index")?
            .query_map([&task], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
            .collect::<Result<_, _>>()?;
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].0.as_str(), rows[0].1.as_deref(), rows[0].2.as_deref()), ("image", Some("image/png"), Some("shot.png")));
        assert_eq!(rows[0].3, sha256(PNG));
        assert_eq!(rows[0].4, PNG.len() as i64);
        let body = p.0.join(".cortex").join(&task).join(".source_blocks").join(&rows[0].5);
        let body = if body.exists() { body } else { p.0.join(".cortex-dev").join(&task).join(".source_blocks").join(&rows[0].5) };
        assert_eq!(std::fs::read(&body).unwrap(), PNG);
        assert_eq!(std::fs::metadata(&body).unwrap().permissions().mode() & 0o777, 0o600);
        let revisions: i64 = db.query_row("SELECT count(*) FROM source_revisions WHERE task_id=?", [&task], |r| r.get(0))?;
        assert_eq!(revisions, 1);
        let event: String = db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_START_ADMITTED'", [&task], |r| r.get(0))?;
        assert!(!event.contains("attachments") && !event.contains(&b64(PNG)), "file bytes reached an event: {event}");
        Ok(())
    })
    .unwrap();
    // Replaying the same Start retains nothing twice.
    let again = start_native(&s, "p", "owner", &start_command(&draft, "start")).unwrap();
    assert_eq!(again["fresh"], false);
    assert_eq!(count(&s, "source_revisions"), 1);
}
/// An owner message with files retains them as the next source revision in the
/// message's own admission; the replayed message does not retain them again.
#[test]
fn owner_prompt_files_are_one_source_revision_bound_to_the_request() {
    use cortex_core::{
        input::capture_from_wire,
        workspace::{control, start_native},
    };
    let (_p, s) = setup();
    let draft = create_draft(&s, "p", "owner", &command()).unwrap().receipt["target"]
        .as_str()
        .unwrap()
        .to_owned();
    let started = start_native(&s, "p", "owner", &start_command(&draft, "start")).unwrap();
    let (task, execution) = (
        started["task"].as_str().unwrap().to_owned(),
        started["receipt"]["target"].as_str().unwrap().to_owned(),
    );
    let capture =
        capture_from_wire(&json!([{"name":"shot.png","mimeType":"image/png","data":b64(PNG)}]))
            .unwrap();
    let request = control::canonical_request(
        json!({"projectId":"p","executionId":execution,"requestId":"files","action":"prompt","text":"Look at this","attachments":[]}),
        Some(&capture),
    )
    .unwrap();
    // Core binds the exact files: a request without the capture is refused, not stored bare.
    assert_eq!(
        control::admit_input(&s, "p", "owner", &request, None)
            .err()
            .unwrap()
            .code(),
        "invalid_input_capture"
    );
    control::admit_input(&s, "p", "owner", &request, Some(&capture)).unwrap();
    s.read(|db| {
        let (kind, sha): (String, String) = db.query_row(
            "SELECT kind,sha256 FROM source_blocks WHERE task_id=? AND availability='stored'", [&task], |r| Ok((r.get(0)?, r.get(1)?)))?;
        assert_eq!((kind.as_str(), sha), ("image", sha256(PNG)));
        let revisions: i64 = db.query_row("SELECT count(*) FROM source_revisions WHERE task_id=?", [&task], |r| r.get(0))?;
        assert_eq!(revisions, 1);
        let attached: String = db.query_row("SELECT attachments_json FROM host_transcript WHERE kind='user_message' ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))?;
        assert!(attached.contains("shot.png") && !attached.contains(&b64(PNG)));
        Ok(())
    })
    .unwrap();
    control::finish(&s, "p", "owner", &request, true).unwrap();
    assert!(control::admit_input(&s, "p", "owner", &request, Some(&capture)).is_err());
    assert_eq!(count(&s, "source_revisions"), 1);
    // The request's own `attachments` field still never carries bytes.
    let mut inline = json!({"projectId":"p","executionId":execution,"requestId":"inline","action":"prompt","text":"x",
        "attachments":[{"name":"a.txt","mimeType":"text/plain","data":"YQ=="}]});
    inline = control::canonical_request(inline, None).unwrap();
    assert_eq!(
        control::admit_input(&s, "p", "owner", &inline, None)
            .err()
            .unwrap()
            .code(),
        "native_attachments_unavailable"
    );
}
fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
