//! Owned fixtures in a private temporary state directory only.
use super::*;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

fn temp(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "cw-attachments-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&root)
        .unwrap();
    root.canonicalize().unwrap()
}
fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn inline(name: &str, bytes: &[u8]) -> Value {
    json!({"name":name,"mimeType":"image/png","data":b64(bytes)})
}
fn age(state: &Path, digest: &str) {
    let old = SystemTime::now() - Duration::from_secs(86_400 + 60);
    File::options()
        .write(true)
        .open(directory(state).join(digest))
        .unwrap()
        .set_modified(old)
        .unwrap();
}
fn saved_text(state: &Path) -> String {
    fs::read_to_string(product_path(state, "p").unwrap()).unwrap()
}

#[test]
fn an_inline_product_file_is_migrated_once_and_keeps_every_body() {
    let state = temp("migrate");
    let shot = vec![7u8; 300_000];
    let note = b"plan notes".to_vec();
    let product = json!({
        "messages":[{"id":"m1","taskId":"t","text":"see","delivery":"queued","attachments":[inline("shot.png",&shot)]}],
        "comments":{"t":[{"id":"c1","text":"here","attachments":[inline("shot.png",&shot)]}]},
        "sprints":[{"id":"sp","attachments":[inline("brief.png",&note)],
            "conversation":[{"kind":"answer","text":"a","attachments":[inline("n.png",&note)]}]}],
        "tasks":{"t":{"title":"keep","attachments":[]}}});
    settings::write_json(&product_path(&state, "p").unwrap(), &product).unwrap();
    let before = saved_text(&state).len();

    let loaded = super::super::native::product(&state, "p").unwrap();
    let text = saved_text(&state);
    assert!(
        !text.contains("\"data\""),
        "the saved file holds references only"
    );
    assert!(text.len() * 50 < before, "{} -> {}", before, text.len());
    assert_eq!(
        loaded["messages"][0]["attachments"][0],
        json!({"name":"shot.png","mimeType":"image/png","size":shot.len(),"sha256":sha(&shot)})
    );
    assert_eq!(loaded["tasks"]["t"]["title"], "keep");
    // One body per content: the message and the comment share it.
    let mut bodies: Vec<_> = fs::read_dir(directory(&state))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().into_string().unwrap())
        .collect();
    bodies.sort();
    let mut want = vec![sha(&shot), sha(&note)];
    want.sort();
    assert_eq!(bodies, want);
    assert_eq!(read(&state, &sha(&shot)).unwrap(), shot);
    let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(directory(&state)), 0o700);
    assert_eq!(mode(directory(&state).join(sha(&shot))), 0o600);
    // Idempotent: a second load changes nothing.
    let again = super::super::native::product(&state, "p").unwrap();
    assert_eq!(again, loaded);
    assert_eq!(saved_text(&state), text);
}

#[test]
fn a_crash_after_the_bodies_before_the_file_is_finished_on_the_next_load() {
    let state = temp("crash");
    let bytes = b"screenshot".to_vec();
    // The bodies were written, the product file was not replaced yet.
    put(&state, &bytes).unwrap();
    settings::write_json(
        &product_path(&state, "p").unwrap(),
        &json!({"messages":[{"id":"m","attachments":[inline("a.png",&bytes)]}]}),
    )
    .unwrap();
    let loaded = super::super::native::product(&state, "p").unwrap();
    assert_eq!(
        loaded["messages"][0]["attachments"][0]["sha256"],
        sha(&bytes)
    );
    assert!(!saved_text(&state).contains("\"data\""));
    // No temporary file is left behind in either directory.
    for dir in [directory(&state), state.join("product")] {
        assert!(fs::read_dir(dir)
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().starts_with(".tmp-")));
    }
}

#[test]
fn a_held_lock_defers_the_rewrite_but_the_reader_still_gets_references() {
    let state = temp("locked");
    let bytes = b"held".to_vec();
    settings::write_json(
        &product_path(&state, "p").unwrap(),
        &json!({"messages":[{"id":"m","attachments":[inline("a.png",&bytes)]}]}),
    )
    .unwrap();
    let lock = settings::Lock::new(&state, "machine.lock").unwrap();
    let loaded = super::super::native::product(&state, "p").unwrap();
    assert!(loaded["messages"][0]["attachments"][0]["data"].is_null());
    assert!(
        saved_text(&state).contains("\"data\""),
        "never written without the lock"
    );
    drop(lock);
    super::super::native::product(&state, "p").unwrap();
    assert!(!saved_text(&state).contains("\"data\""));
}

#[test]
fn saves_take_references_only_and_replies_carry_no_bytes() {
    let state = temp("save");
    let bytes = b"\x89PNG fixture".to_vec();
    let refs = action(
        &state,
        &json!({"action":"upload","files":[inline("s.png",&bytes)]}),
    )
    .unwrap();
    assert_eq!(
        refs,
        json!([{"name":"s.png","mimeType":"image/png","size":bytes.len(),"sha256":sha(&bytes)}])
    );
    let message = |id: &str, attachments: Value| {
        json!({"action":"add","message":{"id":id,"taskId":"t","workerId":null,
        "text":"look","mode":"queue","delivery":"queued","attachments":attachments}})
    };
    // Inline bytes are refused on every save; only the upload takes them.
    let refused = super::super::product::conversation(
        &state,
        "p",
        &message("m0", json!([inline("s.png", &bytes)])),
        false,
    )
    .unwrap_err();
    assert_eq!(refused.code, "inline_attachment_refused");
    let reply =
        super::super::product::conversation(&state, "p", &message("m1", refs.clone()), false)
            .unwrap();
    assert!(!reply.to_string().contains("\"data\""));
    assert!(!saved_text(&state).contains("\"data\""));
    // A reference to a body that is not here, or with the wrong size, is refused.
    let mut wrong = refs.clone();
    wrong[0]["size"] = json!(bytes.len() + 1);
    assert_eq!(
        super::super::product::conversation(&state, "p", &message("m2", wrong), false)
            .unwrap_err()
            .code,
        "attachment_unavailable"
    );
    let missing = json!([{"name":"x.png","mimeType":"image/png","size":1,"sha256":"0".repeat(64)}]);
    assert_eq!(
        super::super::product::conversation(&state, "p", &message("m3", missing), false)
            .unwrap_err()
            .code,
        "attachment_unavailable"
    );
    // Sprints and comments follow the same rule.
    let sprint = super::super::product::sprint(
        &state,
        "p",
        &json!({"action":"create","description":"d","attachments":refs}),
        None,
    )
    .unwrap();
    assert!(sprint["sprintId"].is_string());
    let refused = super::super::product::sprint(
        &state,
        "p",
        &json!({"action":"note","sprintId":sprint["sprintId"],"text":"n",
        "attachments":[inline("s.png",&bytes)]}),
        None,
    )
    .unwrap_err();
    assert_eq!(refused.code, "inline_attachment_refused");
    // The resolved form exists only inside the service, for Core and the engine.
    let mut command = json!({"payload":{"attachments":refs,"answers":[{"attachments":[]}]}});
    resolve_all(&state, &mut command).unwrap();
    assert_eq!(command["payload"]["attachments"][0]["data"], b64(&bytes));
    let mut inline_command =
        json!({"payload":{"answers":[{"attachments":[inline("s.png",&bytes)]}]}});
    assert_eq!(
        resolve_all(&state, &mut inline_command).unwrap_err().code,
        "inline_attachment_refused"
    );
}

#[test]
fn the_on_demand_read_is_bounded_confined_and_verified() {
    let state = temp("read");
    let bytes = b"body".to_vec();
    let digest = put(&state, &bytes).unwrap();
    let reply = action(&state, &json!({"action":"read","sha256":digest})).unwrap();
    assert_eq!(reply["data"], b64(&bytes));
    for bad in [
        "../product/p.json",
        "ABCDEF",
        &digest.to_uppercase(),
        &format!("{digest}/x"),
        "",
    ] {
        assert_eq!(
            action(&state, &json!({"action":"read","sha256":bad}))
                .unwrap_err()
                .code,
            "attachment_unavailable",
            "{bad}"
        );
    }
    assert!(action(&state, &json!({"action":"read","path":"/etc/passwd"})).is_err());
    // A body whose content no longer matches its digest is never served.
    fs::write(directory(&state).join(&digest), b"tampered").unwrap();
    assert_eq!(
        read(&state, &digest).unwrap_err().code,
        "attachment_unavailable"
    );
    // A symlink under a digest name is not followed.
    let other = put(&state, b"other").unwrap();
    fs::remove_file(directory(&state).join(&other)).unwrap();
    std::os::unix::fs::symlink(state.join("machine.lock"), directory(&state).join(&other)).unwrap();
    assert_eq!(
        read(&state, &other).unwrap_err().code,
        "attachment_unavailable"
    );
    // An oversized file is refused without being read whole.
    let big = "f".repeat(64);
    fs::write(directory(&state).join(&big), vec![0u8; MAX_BYTES + 1]).unwrap();
    assert_eq!(
        read(&state, &big).unwrap_err().code,
        "attachment_unavailable"
    );
    // Uploads keep the existing limits.
    let too_many: Vec<_> = (0..9).map(|i| inline(&format!("{i}.png"), b"x")).collect();
    assert!(upload(&state, &json!(too_many)).is_err());
}

#[test]
fn removing_the_last_reference_deletes_the_body_and_keeps_every_other() {
    let state = temp("sweep");
    let shared = b"shared".to_vec();
    let alone = b"alone".to_vec();
    let fresh = b"unsent".to_vec();
    let refs = upload(
        &state,
        &json!([inline("a.png", &shared), inline("b.png", &alone)]),
    )
    .unwrap();
    upload(&state, &json!([inline("c.png", &fresh)])).unwrap();
    for digest in [sha(&shared), sha(&alone)] {
        age(&state, &digest);
    }
    let add = |id: &str, a: Value| {
        json!({"action":"add","message":{"id":id,"taskId":"t","workerId":null,
        "text":"x","mode":"queue","delivery":"queued","attachments":a}})
    };
    super::super::product::conversation(&state, "p", &add("m1", refs.clone()), false).unwrap();
    super::super::product::conversation(&state, "p", &add("m2", json!([refs[0]])), false).unwrap();
    // Another project references the shared body too.
    settings::write_json(
        &product_path(&state, "q").unwrap(),
        &json!({"messages":[{"id":"o","attachments":[refs[0]]}]}),
    )
    .unwrap();
    super::super::product::conversation(
        &state,
        "p",
        &json!({"action":"remove","messageId":"m1"}),
        false,
    )
    .unwrap();
    let exists = |b: &[u8]| directory(&state).join(sha(b)).exists();
    assert!(!exists(&alone), "its last reference is gone");
    assert!(exists(&shared), "still referenced");
    assert!(exists(&fresh), "an unsent upload is kept within its grace");
    age(&state, &sha(&fresh));
    let _lock = settings::Lock::new(&state, "machine.lock").unwrap();
    assert_eq!(sweep_locked(&state, SystemTime::now()).unwrap(), 1);
    assert!(!exists(&fresh) && exists(&shared));
}

#[test]
fn an_answer_file_lives_as_long_as_its_task_record() {
    let state = temp("answers");
    let answered = b"answer shot".to_vec();
    let stray = b"never sent".to_vec();
    let refs = upload(&state, &json!([inline("shot.png", &answered)])).unwrap();
    upload(&state, &json!([inline("stray.png", &stray)])).unwrap();
    age(&state, &sha(&answered));
    age(&state, &sha(&stray));
    let mut product = json!({"tasks":{"t":{"archive":{"at":1,"previousStatus":"done"}}},
        "messages":[],"comments":{},"sprints":[],"answerFiles":{"t":{"attachments":refs}}});
    settings::write_json(&product_path(&state, "p").unwrap(), &product).unwrap();
    let exists = |b: &[u8]| directory(&state).join(sha(b)).exists();
    {
        let _lock = settings::Lock::new(&state, "machine.lock").unwrap();
        // Referenced (an archived task keeps its answers): kept; nothing references the stray.
        assert_eq!(sweep_locked(&state, SystemTime::now()).unwrap(), 1);
    }
    assert!(exists(&answered) && !exists(&stray));
    assert_eq!(read(&state, &sha(&answered)).unwrap(), answered);
    // Deleting the task releases its answers with it.
    forget_answers(&mut product, "t");
    assert_eq!(product["answerFiles"], json!({}));
    settings::write_json(&product_path(&state, "p").unwrap(), &product).unwrap();
    let _lock = settings::Lock::new(&state, "machine.lock").unwrap();
    assert_eq!(sweep_locked(&state, SystemTime::now()).unwrap(), 1);
    assert!(!exists(&answered));
}

#[test]
fn attachment_sweep_respects_configured_age_and_fails_closed_on_directory_errors() {
    let state = temp("scheduled-sweep");
    let sha = put(&state, b"stray").unwrap();
    age(&state, &sha);
    let target = directory(&state).join(&sha);
    let symlink = directory(&state).join("a".repeat(64));
    std::os::unix::fs::symlink(&target, &symlink).unwrap();
    let lock = settings::Lock::new(&state, "machine.lock").unwrap();
    settings::write_json(&state.join("machine.json"), &json!({"unreferencedAttachmentRetentionDays":2})).unwrap();
    let now = SystemTime::now();
    assert_eq!(sweep_locked(&state, now).unwrap(), 0);
    let product = state.join("product");
    std::fs::write(&product, b"not a directory").unwrap();
    let later = now + Duration::from_secs(86_400);
    assert!(sweep_locked(&state, later).is_err());
    assert!(target.is_file());
    std::fs::remove_file(&product).unwrap();
    assert_eq!(sweep_locked(&state, later).unwrap(), 1);
    assert!(!target.exists());
    assert!(symlink.symlink_metadata().unwrap().file_type().is_symlink());
    drop(lock);
}

fn answer_sweep_store(state: &Path) -> (crate::Project, cortex_core::Store, String) {
    let root = state.join("project");
    fs::create_dir(&root).unwrap();
    crate::project_guard::reserve(&root).unwrap();
    let key = crate::native_runtime::key(&root).unwrap();
    let store = cortex_core::Store::open(key, cortex_core::OpenMode::Initialize).unwrap();
    let answer = put(state, b"still retained").unwrap();
    age(state, &answer);
    store.write(|db| {
        db.execute("INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)", [root.to_str()])?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,execution_id,created_at) VALUES ('task','p','Fixture','execution','now')", [])?;
        db.execute("INSERT INTO workspace_executions VALUES ('execution','task','runtime','session',1,1,'now')", [])?;
        db.execute("INSERT INTO tasks VALUES ('runtime',?,'now')", [root.to_str()])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('runtime','WAITING_USER','Fixture','now','now')", [])?;
        db.execute(
            "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES ('runtime','USER_ANSWER_ATTACHMENTS','owner',?,'now')",
            [json!({"attachments":[{"sha256":answer,"availability":"stored"}]}).to_string()],
        )?;
        Ok(())
    }).unwrap();
    let project = crate::Project {
        project_id: "p".into(),
        display_name: "Fixture".into(),
        canonical_root: root,
        activation: Some(crate::Activation {
            actor_id: "owner".into(),
            recorded_at: 0,
        }),
    };
    assert!(!state.join("product").exists(), "the answer exists only in Core");
    (project, store, answer)
}

#[test]
fn attachment_sweep_keeps_a_committed_answer_before_the_product_cache_is_saved() {
    let state = temp("answer-references");
    let (project, store, answer) = answer_sweep_store(&state);
    crate::local::save_registry(&state.join("projects.json"), &[project]).unwrap();
    let orphan = put(&state, b"unreferenced").unwrap();
    age(&state, &orphan);
    let _lock = settings::Lock::new(&state, "machine.lock").unwrap();
    let access = cortex_core::store::locks::AccessLock::acquire(
        &store.key().directory().join(".access.lock"), false, true, Duration::ZERO,
    ).unwrap();
    assert!(sweep_locked(&state, SystemTime::now()).is_err());
    assert!(directory(&state).join(&orphan).exists());
    drop(access);
    assert_eq!(sweep_locked(&state, SystemTime::now()).unwrap(), 1);
    assert!(directory(&state).join(&answer).is_file());
    assert!(!directory(&state).join(&orphan).exists());
}

#[test]
fn attachment_sweep_keeps_core_only_answers_after_remove_and_readd() {
    let state = temp("detached-answer-references");
    let (mut project, _store, answer) = answer_sweep_store(&state);
    // Remove keeps the first detached locator, so its activation may be stale.
    // A re-added project also has no activation until it is started again.
    project.activation = None;
    crate::local::save_registry(
        &state.join("removed-projects.json"),
        std::slice::from_ref(&project),
    )
    .unwrap();
    for readded in [false, true] {
        if readded {
            crate::local::save_registry(
                &state.join("projects.json"),
                std::slice::from_ref(&project),
            )
            .unwrap();
        }
        let orphan = put(&state, b"unreferenced").unwrap();
        age(&state, &orphan);
        let _lock = settings::Lock::new(&state, "machine.lock").unwrap();
        assert_eq!(sweep_locked(&state, SystemTime::now()).unwrap(), 1);
        assert!(directory(&state).join(&answer).is_file());
        assert!(!directory(&state).join(&orphan).exists());
    }
}

#[test]
fn attachment_sweep_stops_when_a_detached_store_is_unreadable() {
    let state = temp("unreadable-detached-store");
    let (project, store, answer) = answer_sweep_store(&state);
    crate::local::save_registry(&state.join("removed-projects.json"), &[project]).unwrap();
    let orphan = put(&state, b"unreferenced").unwrap();
    age(&state, &orphan);
    let _lock = settings::Lock::new(&state, "machine.lock").unwrap();
    let access = cortex_core::store::locks::AccessLock::acquire(
        &store.key().directory().join(".access.lock"),
        false,
        true,
        Duration::ZERO,
    )
    .unwrap();
    assert!(sweep_locked(&state, SystemTime::now()).is_err());
    assert!(directory(&state).join(&answer).is_file());
    assert!(directory(&state).join(&orphan).is_file());

    drop(access);
    store
        .write(|db| {
            db.pragma_update(None, "user_version", 99)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        sweep_locked(&state, SystemTime::now()).unwrap_err().code,
        "unsupported_storage"
    );
    assert!(directory(&state).join(&answer).is_file());
    assert!(directory(&state).join(&orphan).is_file());
}

#[test]
fn attachment_sweep_allows_a_never_activated_project_with_no_store() {
    let state = temp("new-project-without-store");
    let root = state.join("project");
    fs::create_dir(&root).unwrap();
    crate::project_guard::reserve(&root).unwrap();
    let project = crate::Project {
        project_id: "p".into(),
        display_name: "Fixture".into(),
        canonical_root: root,
        activation: None,
    };
    crate::local::save_registry(&state.join("projects.json"), &[project]).unwrap();
    let orphan = put(&state, b"unreferenced").unwrap();
    age(&state, &orphan);
    let _lock = settings::Lock::new(&state, "machine.lock").unwrap();
    assert_eq!(sweep_locked(&state, SystemTime::now()).unwrap(), 1);
    assert!(!directory(&state).join(&orphan).exists());
}

#[test]
fn an_unreadable_product_file_stops_the_sweep() {
    let state = temp("unreadable");
    let digest = put(&state, b"keep").unwrap();
    age(&state, &digest);
    fs::create_dir_all(state.join("product")).unwrap();
    fs::write(state.join("product").join("p.json"), b"{not json").unwrap();
    assert!(sweep_locked(&state, SystemTime::now()).is_err());
    assert!(directory(&state).join(digest).exists());
}

#[test]
fn delivery_reads_bodies_at_that_moment_and_names_a_missing_one() {
    let state = temp("deliver");
    let bytes = b"brief".to_vec();
    let refs = upload(&state, &json!([inline("a.png", &bytes)])).unwrap();
    let gone = json!({"name":"g.png","mimeType":"image/png","size":1,"sha256":"1".repeat(64)});
    let mut sprint = json!({"attachments":refs,"conversation":[{"attachments":[gone]}]});
    inline_for_delivery(&state, &mut sprint);
    assert_eq!(sprint["attachments"][0]["data"], b64(&bytes));
    assert!(sprint["conversation"][0]["attachments"][0]["data"].is_null());
}
