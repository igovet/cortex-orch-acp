mod support;
use cortex_core::{lifecycle::TaskMachine, store::publication::PublicationIntent};
use support::*;
#[test]
fn uncommitted_new_report_is_removed_by_exact_intent_reconciliation() {
    let p = Project::new();
    let s = p.open();
    let dir = p.key().directory();
    let (intent, journal) =
        PublicationIntent::prepare(&dir, "r_one", "r_one.md", "new", None).unwrap();
    intent.install(&dir).unwrap();
    s.read(|db| PublicationIntent::reconcile(&dir, &journal, db))
        .unwrap();
    assert!(!dir.join("r_one.md").exists());
}
#[test]
fn pipeline_is_newest_first_and_failed_commit_restores_exact_previous_bytes() {
    let p = Project::new();
    let s = p.open();
    let dir = p.key().directory();
    let old = b"previous";
    std::fs::write(dir.join("pipeline.md"), old).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        dir.join("pipeline.md"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(old));
    let (intent, journal) =
        PublicationIntent::prepare(&dir, "r_pipeline", "pipeline.md", "newest", Some(&digest))
            .unwrap();
    intent.install(&dir).unwrap();
    assert_eq!(
        std::fs::read(dir.join("pipeline.md")).unwrap(),
        b"newest\n\n---\n\nprevious"
    );
    s.read(|db| PublicationIntent::reconcile(&dir, &journal, db))
        .unwrap();
    assert_eq!(std::fs::read(dir.join("pipeline.md")).unwrap(), old);
}
#[test]
fn committed_report_is_kept_and_existing_immutable_report_cannot_be_replaced() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    let dir = p.key().directory();
    let (intent, journal) =
        PublicationIntent::prepare(&dir, "r_one", "r_one.md", "published", None).unwrap();
    intent.install(&dir).unwrap();
    s.write(|db|{
        db.execute("INSERT INTO reports VALUES ('r_one','t','report','r_one.md','now')",[])?;
        db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES ('r_one','title','summary','author','now',?,9)",[intent.digest()])?;
        Ok(())
    }).unwrap();
    s.read(|db| PublicationIntent::reconcile(&dir, &journal, db))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("r_one.md")).unwrap(),
        "published"
    );
    assert!(PublicationIntent::prepare(&dir, "r_one", "r_one.md", "overwrite", None).is_err());
}
#[test]
fn interrupted_before_replace_preserves_old_report_and_rejects_conflicts() {
    let p = Project::new();
    let s = p.open();
    let dir = p.key().directory();
    let (_, journal) = PublicationIntent::prepare(&dir, "r_one", "r_one.md", "new", None).unwrap();
    s.read(|db| PublicationIntent::reconcile(&dir, &journal, db))
        .unwrap();
    assert!(!dir.join("r_one.md").exists());
    assert!(PublicationIntent::prepare(&dir, "r_one", "../escape", "new", None).is_err());
}
