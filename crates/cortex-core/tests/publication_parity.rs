mod support;
use cortex_core::{
    lifecycle::TaskMachine,
    store::{
        documents::{Publication, Publisher},
        publication::PublicationIntent,
    },
    OpenMode, Store,
};
use support::*;
fn setup() -> (Project, Store) {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    TaskMachine::new(&s)
        .create("other", "goal", "owner")
        .unwrap();
    (p, s)
}
fn meta() -> Publication<'static> {
    Publication {
        title: "Work",
        summary: "Evidence",
        author: "worker",
        source_revision: 0,
        artifacts: serde_json::json!([]),
    }
}
fn publish(s: &Store, body: &str, pipeline: bool) -> String {
    let api = Publisher::new(s);
    let key = uuid::Uuid::new_v4().to_string();
    let d = if pipeline {
        api.create_pipeline_draft("t", "owner", &key)
    } else {
        api.create_draft("t", "owner", "general", &key)
    }
    .unwrap();
    api.write_draft(
        "t",
        "owner",
        &d.draft_id,
        &format!("Cortex draft ID: `{}`\n\n{body}", d.draft_id),
    )
    .unwrap();
    api.publish("t", "owner", &d.draft_id, &key, meta())
        .unwrap()
        .report_id
}
#[test] // Python tests/test_markdown_store.py::test_roundtrip_verbatim_and_no_semantic_parser
fn roundtrip_verbatim_and_no_semantic_parser() {
    let (_p, s) = setup();
    for body in [
        "No headings",
        "- [x] complete\nnot tested",
        "{\"status\":\"done\"}",
        "# Свободный отчёт\n\n😀\r\n\tend\n",
    ] {
        let id = publish(&s, body, false);
        assert!(Publisher::new(&s)
            .read_report("t", &id, None, 4000)
            .unwrap()
            .markdown
            .ends_with(body));
    }
}
#[test] // Python test_pipeline_newest_first_cursor_and_snapshot
fn pipeline_newest_first_cursor_and_snapshot() {
    let (p, s) = setup();
    let api = Publisher::new(&s);
    let rid = publish(&s, &"old ".repeat(3000), true);
    let old = api.read_report("t", &rid, None, 100).unwrap();
    publish(&s, "ordinary report", false);
    let list = api.list("t", None, 1).unwrap();
    assert_eq!(publish(&s, "Fresh edition", true), rid);
    assert!(api
        .read_report("t", &rid, None, 4000)
        .unwrap()
        .markdown
        .contains("Fresh edition\n\n---\n\n"));
    assert_eq!(
        api.read_report("t", &rid, old.next_cursor.as_deref(), 100)
            .unwrap_err()
            .code(),
        "cursor_stale"
    );
    let following = api.list("t", list.next_cursor.as_deref(), 100).unwrap();
    assert_eq!(
        following
            .reports
            .iter()
            .filter(|v| v.report_id == rid)
            .count(),
        1
    );
    assert_eq!(count(&s, "reports"), 2);
    assert!(p.key().directory().join("t/pipeline.md").is_file());
}
#[test] // Python test_big_unicode_read_restart_and_stable_catalogue
fn big_unicode_read_restart_and_stable_catalogue() {
    let (p, s) = setup();
    let body = "😀З\n".repeat(50000);
    let rid = publish(&s, &body, false);
    publish(&s, "second", false);
    let page = Publisher::new(&s).list("t", None, 1).unwrap();
    publish(&s, "later", false);
    drop(s);
    let s = Store::open(p.key(), OpenMode::RuntimeValidate).unwrap();
    let api = Publisher::new(&s);
    assert_eq!(
        api.list("t", page.next_cursor.as_deref(), 100)
            .unwrap()
            .reports
            .len(),
        1
    );
    let mut cursor = None;
    let mut parts = String::new();
    loop {
        let page = api.read_report("t", &rid, cursor.as_deref(), 3999).unwrap();
        assert!(page.markdown.chars().count() <= 3999);
        parts.push_str(&page.markdown);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(parts.ends_with(&body));
    assert!(parts.starts_with("Cortex draft ID:"));
}
#[test] // Python test_isolation_and_foreign_continuation
fn isolation_and_foreign_continuation() {
    let (_p, s) = setup();
    let id = publish(&s, "one", false);
    publish(&s, "two", false);
    let api = Publisher::new(&s);
    assert_eq!(
        api.read_report("other", &id, None, 1).unwrap_err().code(),
        "not_found"
    );
    let list = api.list("t", None, 1).unwrap();
    let c = list.next_cursor.unwrap();
    assert_eq!(
        api.list("other", Some(&c), 1).unwrap_err().code(),
        "invalid_cursor"
    );
    let c = api
        .read_report("t", &id, None, 1)
        .unwrap()
        .next_cursor
        .unwrap();
    assert_eq!(
        api.read_report("t", &id, Some(&c.replace("cr.", "cd.")), 1)
            .unwrap_err()
            .code(),
        "invalid_cursor"
    );
    for n in [0, 101] {
        assert!(api.list("t", None, n).is_err());
    }
    assert!(api.read_report("t", &id, None, 4001).is_err());
}
#[test] // Python test_pipeline_draft_rejects_unfilled_server_placeholder
fn pipeline_draft_rejects_unfilled_server_placeholder() {
    let (_p, s) = setup();
    let api = Publisher::new(&s);
    assert_eq!(
        api.create_draft("t", "worker", "pipeline", "x")
            .unwrap_err()
            .code(),
        "runtime_owned_pipeline"
    );
    let d = api.create_pipeline_draft("t", "owner", "d").unwrap();
    assert!(d.markdown.contains("{{CURRENT_WORK_GRAPH}}"));
    assert_eq!(
        api.publish("t", "owner", &d.draft_id, "p", meta())
            .unwrap_err()
            .code(),
        "draft_guidance_remaining"
    );
    assert!(d.draft_path.exists());
    assert_eq!(count(&s, "reports"), 0);
}
#[test] // Python test_own_drafts_bindings_revision_and_artifact_metadata (storage_v11.py)
fn own_drafts_bindings_revision_and_artifact_metadata() {
    let (_p, s) = setup();
    let api = Publisher::new(&s);
    let mine = api
        .create_draft("t", "owner", "implementation", "same")
        .unwrap();
    let other = api.create_draft("t", "worker", "general", "same").unwrap();
    assert_eq!(
        api.own_drafts("t", "owner", "", 100).unwrap(),
        vec![mine.draft_id.clone()]
    );
    assert_ne!(mine.draft_id, other.draft_id);
    assert_eq!(
        api.write_draft("t", "worker", &mine.draft_id, "stolen")
            .unwrap_err()
            .code(),
        "draft_not_owned"
    );
    assert_eq!(
        api.read_draft("other", "owner", &mine.draft_id, None, 1)
            .unwrap_err()
            .code(),
        "draft_not_owned"
    );
    api.write_draft(
        "t",
        "owner",
        &mine.draft_id,
        &format!("Cortex draft ID: `{}`\n\ncomplete", mine.draft_id),
    )
    .unwrap();
    let mut m = meta();
    m.artifacts = serde_json::json!([{"reference":"src/app.rs","version":"commit:abc"}]);
    let r = api
        .publish("t", "owner", &mine.draft_id, "publish", m)
        .unwrap();
    assert_eq!(
        api.list("t", None, 100).unwrap().reports[0].artifacts,
        r.artifacts
    );
    assert!(!mine.draft_path.exists());
    assert_eq!(api.own_drafts("t", "owner", "", 100).unwrap().len(), 0);
}
#[test] // Python test_replay_all_writes_and_conflicts
fn replay_all_writes_and_conflicts() {
    let (_p, s) = setup();
    let api = Publisher::new(&s);
    let d = api.create_draft("t", "owner", "general", "key").unwrap();
    assert_eq!(api.create_draft("t", "owner", "general", "key").unwrap(), d);
    assert_eq!(
        api.create_draft("t", "owner", "planning", "key")
            .unwrap_err()
            .code(),
        "delivery_conflict"
    );
    api.write_draft(
        "t",
        "owner",
        &d.draft_id,
        &format!("Cortex draft ID: `{}`\n\nwork", d.draft_id),
    )
    .unwrap();
    let first = api
        .publish("t", "owner", &d.draft_id, "key", meta())
        .unwrap();
    assert_eq!(
        api.publish("t", "owner", &d.draft_id, "key", meta())
            .unwrap(),
        first
    );
    let mut changed = meta();
    changed.summary = "changed";
    assert_eq!(
        api.publish("t", "owner", &d.draft_id, "key", changed)
            .unwrap_err()
            .code(),
        "delivery_conflict"
    );
    assert_eq!(count(&s, "reports"), 1);
    assert_eq!(count(&s, "editions"), 1);
}
#[test] // Python test_bodies_only_in_real_files_and_report_tamper
fn bodies_only_in_real_files_and_report_tamper() {
    let (p, s) = setup();
    let id = publish(&s, "private body marker", false);
    let api = Publisher::new(&s);
    let file = p.key().directory().join("t").join(format!("{id}.md"));
    std::fs::write(&file, "external edit").unwrap();
    assert_eq!(
        api.read_report("t", &id, None, 10).unwrap_err().code(),
        "file_conflict"
    );
    assert!(api.list("t", None, 10).is_ok()); // Catalogue does not scan immutable bodies.
                                              // Editions contain only digests/metadata.
    assert!(s
        .write(|db| {
            db.execute("UPDATE editions SET summary='changed'", [])?;
            Ok(())
        })
        .is_err());
}
#[test] // Python test_atomic_rollback_and_immutable_reports
fn atomic_rollback_and_immutable_reports() {
    let (p, s) = setup();
    let api = Publisher::new(&s);
    let id = publish(&s, "old", true);
    let old = std::fs::read(p.key().directory().join("t/pipeline.md")).unwrap();
    let d = api.create_pipeline_draft("t", "owner", "second").unwrap();
    api.write_draft(
        "t",
        "owner",
        &d.draft_id,
        &format!("Cortex draft ID: `{}`\n\nnew", d.draft_id),
    )
    .unwrap();
    s.write(|db|{db.execute_batch("CREATE TRIGGER reject_edition BEFORE INSERT ON editions BEGIN SELECT RAISE(ABORT,'injected'); END;")?;Ok(())}).unwrap();
    assert!(api
        .publish("t", "owner", &d.draft_id, "second", meta())
        .is_err());
    assert_eq!(
        std::fs::read(p.key().directory().join("t/pipeline.md")).unwrap(),
        old
    );
    assert_eq!(count(&s, "editions"), 1);
    assert!(d.draft_path.exists());
    assert_eq!(
        api.read_report("t", &id, None, 4000)
            .unwrap()
            .markdown
            .as_bytes(),
        old
    );
}
#[test] // Python test_post_commit_file_failure_recovers_on_restart
fn post_commit_file_failure_recovers_on_restart() {
    let (p, s) = setup();
    let api = Publisher::new(&s);
    let d = api.create_draft("t", "owner", "general", "d").unwrap();
    api.write_draft(
        "t",
        "owner",
        &d.draft_id,
        &format!("Cortex draft ID: `{}`\n\ncomplete", d.draft_id),
    )
    .unwrap();
    s.write(|db|{db.execute_batch("CREATE TRIGGER reject_cleanup BEFORE DELETE ON pending_source_deletions BEGIN SELECT RAISE(ABORT,'injected'); END;")?;Ok(())}).unwrap();
    assert!(api.publish("t", "owner", &d.draft_id, "p", meta()).is_err());
    assert_eq!(count(&s, "reports"), 1);
    assert_eq!(count(&s, "pending_source_deletions"), 1);
    s.write(|db| {
        db.execute_batch("DROP TRIGGER reject_cleanup;")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = p.open();
    let api = Publisher::new(&s);
    api.reconcile("t").unwrap();
    let r = api.publish("t", "owner", &d.draft_id, "p", meta()).unwrap();
    assert_eq!(count(&s, "reports"), 1);
    assert_eq!(count(&s, "pending_source_deletions"), 0);
    assert!(api
        .read_report("t", &r.report_id, None, 4000)
        .unwrap()
        .markdown
        .ends_with("complete"));
}
#[test] // Python test_concurrent_delivery_once_and_distinct_writes
fn concurrent_delivery_once_and_distinct_writes() {
    let (p, s) = setup();
    let api = Publisher::new(&s);
    let d = api.create_draft("t", "owner", "general", "d").unwrap();
    api.write_draft(
        "t",
        "owner",
        &d.draft_id,
        &format!("Cortex draft ID: `{}`\n\ncomplete", d.draft_id),
    )
    .unwrap();
    let ids: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    let store = Store::open(p.key(), OpenMode::RuntimeValidate).unwrap();
                    Publisher::new(&store)
                        .publish("t", "owner", &d.draft_id, "same", meta())
                        .unwrap()
                        .report_id
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(ids.iter().all(|id| id == &ids[0]));
    assert_eq!(count(&s, "reports"), 1);
}
#[test] // Python store recovery tests: interrupted replacement, exact committed digest
fn domain_reconciles_interrupted_intent_before_next_operation() {
    let (p, s) = setup();
    let id = publish(&s, "old", true);
    let dir = p.key().directory().join("t");
    let old = std::fs::read(&dir.join("pipeline.md")).unwrap();
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(&old));
    let (intent, _) =
        PublicationIntent::prepare(&dir, &id, "pipeline.md", "uncommitted", Some(&digest)).unwrap();
    intent.install(&dir).unwrap();
    Publisher::new(&s).reconcile("t").unwrap();
    assert_eq!(std::fs::read(dir.join("pipeline.md")).unwrap(), old);
}
#[test] // Python read_draft cursor binding/staleness and atomic editor replacement regressions
fn draft_cursor_stale_and_atomic_editor_replacement() {
    let (_p, s) = setup();
    let api = Publisher::new(&s);
    let d = api.create_draft("t", "owner", "general", "d").unwrap();
    let c = api
        .read_draft("t", "owner", &d.draft_id, None, 1)
        .unwrap()
        .next_cursor
        .unwrap();
    let temp = d.draft_path.with_extension("new");
    std::fs::copy(&d.draft_path, &temp).unwrap();
    std::fs::rename(&temp, &d.draft_path).unwrap();
    api.write_draft(
        "t",
        "owner",
        &d.draft_id,
        &format!("Cortex draft ID: `{}`\n\nreplaced", d.draft_id),
    )
    .unwrap();
    assert_eq!(
        api.read_draft("t", "owner", &d.draft_id, Some(&c), 1)
            .unwrap_err()
            .code(),
        "draft_cursor_stale"
    );
    api.publish("t", "owner", &d.draft_id, "p", meta()).unwrap();
}
#[test] // Python non-mutating inspection contract extended to publication reads
fn inspection_reads_never_reconcile_or_create_files() {
    let (p, s) = setup();
    let id = publish(&s, "body", false);
    drop(s);
    let names: Vec<_> = std::fs::read_dir(p.key().directory())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let s = Store::open(p.key(), OpenMode::InspectImmutable).unwrap();
    let api = Publisher::new(&s);
    assert!(api.read_report("t", &id, None, 4000).is_ok());
    assert_eq!(
        api.create_draft("t", "owner", "general", "d")
            .unwrap_err()
            .code(),
        "inspection_read_only"
    );
    let after: Vec<_> = std::fs::read_dir(p.key().directory())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, after);
}
#[test] // Python test_invalid_inputs plus write_report metadata bounds in contracts.py:107-112
fn publication_rejects_invalid_metadata_future_revision_and_bad_markers() {
    let (_p, s) = setup();
    let api = Publisher::new(&s);
    let d = api.create_draft("t", "owner", "general", "d").unwrap();
    assert!(api.create_draft("t", "owner", "general", "").is_err());
    assert_eq!(
        api.write_draft("t", "owner", &d.draft_id, "missing marker")
            .unwrap_err()
            .code(),
        "draft_marker_missing"
    );
    api.write_draft(
        "t",
        "owner",
        &d.draft_id,
        &format!("Cortex draft ID: `{}`\n\ncomplete", d.draft_id),
    )
    .unwrap();
    let mut future = meta();
    future.source_revision = 1;
    assert_eq!(
        api.publish("t", "owner", &d.draft_id, "future", future)
            .unwrap_err()
            .code(),
        "invalid_source_revision"
    );
    let over = "😀".repeat(321);
    let mut long = meta();
    long.summary = &over;
    assert_eq!(
        api.publish("t", "owner", &d.draft_id, "long", long)
            .unwrap_err()
            .code(),
        "invalid_publication"
    );
    let mut bad = meta();
    bad.artifacts = serde_json::json!([{"reference":"path","version":"x","unexpected":"field"}]);
    assert_eq!(
        api.publish("t", "owner", &d.draft_id, "bad", bad)
            .unwrap_err()
            .code(),
        "invalid_publication"
    );
    assert_eq!(count(&s, "reports"), 0);
    assert!(d.draft_path.exists());
}
#[test] // Python immutable-read and duplicate write-report draft_conflict protections
fn invalid_utf8_report_and_modified_replayed_draft_fail_closed() {
    let (p, s) = setup();
    let api = Publisher::new(&s);
    let d = api.create_draft("t", "owner", "general", "d").unwrap();
    api.write_draft(
        "t",
        "owner",
        &d.draft_id,
        &format!("Cortex draft ID: `{}`\n\ncomplete", d.draft_id),
    )
    .unwrap();
    let r = api.publish("t", "owner", &d.draft_id, "p", meta()).unwrap();
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&d.draft_path)
        .unwrap();
    f.write_all(b"changed").unwrap();
    drop(f);
    assert_eq!(
        api.publish("t", "owner", &d.draft_id, "p", meta())
            .unwrap_err()
            .code(),
        "draft_conflict"
    );
    std::fs::write(
        p.key()
            .directory()
            .join("t")
            .join(format!("{}.md", r.report_id)),
        [0xff],
    )
    .unwrap();
    assert_eq!(
        api.read_report("t", &r.report_id, None, 1)
            .unwrap_err()
            .code(),
        "file_conflict"
    );
}
