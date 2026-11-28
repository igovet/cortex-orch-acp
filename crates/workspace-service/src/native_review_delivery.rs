//! Inline delivery with a review coverage refusal. A reviewer refused
//! `review_file_coverage_required` or `review_predecessor_reports_required` gets the
//! current pages of what is missing in the refusal itself, recorded as a read on its
//! own channel exactly as the matching `project_read` / `read_report` call would
//! record it, so the next submission passes without another round of reads.
//! The delivery adds no authority: only a changed file of the review that the
//! reviewer may read (the refusal item names it as a missing read, never as outside
//! its read scope) and that is not a protected path, and only a report the review
//! packet itself requires. A bounded budget applies; whatever it does not reach
//! stays named in `missing` and the next refusal continues where this one stopped.
use cortex_core::{
    channels::{Credential, Identity},
    CoreResult, Store,
};
use serde_json::{json, Value};
use std::path::Path;

/// Content bytes delivered inline by one refusal.
const INLINE_BYTES: usize = 64_000;
/// Files (or reports) delivered by one refusal.
const MAX_ITEMS: usize = 12;
const READ_REASON: &str = "current complete read is missing or stale";

/// The inline delivery for a refusal `code` naming `missing`, or `None` when the
/// code carries none or nothing could be delivered.
pub(super) fn inline(
    store: &Store,
    c: &Credential,
    i: &Identity,
    root: &Path,
    scoped: bool,
    code: &str,
    missing: &[String],
) -> Option<Value> {
    if i.stage.as_deref() != Some("review") {
        return None;
    }
    let delivered = match code {
        "review_file_coverage_required" | "review_project_revision_changed" =>
            files(store, c, i, root, scoped, missing),
        "review_predecessor_reports_required" => reports(store, c, i, missing),
        _ => return None,
    };
    if delivered.is_empty() {
        return None;
    }
    let note = if code == "review_project_revision_changed" {
        "These pages are from the rebound checkout and count as read. Assess the new bytes, finish anything still missing, and submit your current verdict with a fresh request key."
    } else {
        "These pages were read for you on this channel and count as read. Submit the same review again; anything not delivered stays listed in missing."
    };
    Some(json!({"delivered_as_read":true,"items":delivered,"note":note}))
}

fn files(
    store: &Store,
    c: &Credential,
    i: &Identity,
    root: &Path,
    scoped: bool,
    missing: &[String],
) -> Vec<Value> {
    let binding = if scoped && root != store.key().root() {
        match crate::native_project::checkout_binding(store, i) {
            Ok(Some(b)) => Some(b),
            _ => return Vec::new(),
        }
    } else {
        None
    };
    let mut out = Vec::new();
    let mut spent = 0usize;
    let mut items = 0usize;
    for item in missing {
        if spent >= INLINE_BYTES || items >= MAX_ITEMS {
            break;
        }
        let Some(path) = item
            .strip_suffix(&format!(" ({READ_REASON})"))
            .filter(|p| !p.is_empty())
        else {
            continue;
        };
        if crate::native_project::safe(path).is_err()
            || crate::native_project::protected(root, path).is_some()
        {
            continue;
        }
        let Some(sha) = crate::native_project::current_sha256(root, path) else {
            continue;
        };
        items += 1;
        let _ = read_file(store, c, i, root, scoped, binding.as_ref(), path, &sha, &mut spent, &mut out);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn read_file(
    store: &Store,
    c: &Credential,
    i: &Identity,
    root: &Path,
    scoped: bool,
    binding: Option<&Value>,
    path: &str,
    sha: &str,
    spent: &mut usize,
    out: &mut Vec<Value>,
) -> CoreResult<()> {
    // Continue after the lines this channel already read of these exact bytes.
    let seen: Vec<u64> = crate::native_project::read_state(store, i, c, path, binding)?
        .filter(|v| v["sha256"] == sha)
        .and_then(|v| v["lines"].as_array().cloned())
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    let mut offset = (1u64..).find(|n| !seen.contains(n)).unwrap_or(1);
    let mut column = 1u64;
    loop {
        let reply = crate::native_project::execute(
            store,
            i,
            c,
            "project_read",
            &json!({"path":path,"offset":offset,"column":column}),
            false,
            root,
            scoped,
        )?;
        let page = &reply["structuredContent"];
        // An image or bytes without text are recorded by identity; nothing to show.
        let has_image = reply["content"]
            .as_array()
            .is_some_and(|b| b.iter().any(|b| b["type"] == "image"));
        if page["binary"] == true || has_image {
            out.push(json!({"path":path,"sha256":sha,"delivered":"identity_only",
                "reason":"no reviewable text (binary or image); recorded as read by its exact bytes"}));
            return Ok(());
        }
        let content = page["content"].as_str().unwrap_or_default();
        *spent += content.len();
        let next = page["next_offset"].as_u64();
        let mut entry = json!({"path":path,"sha256":page["sha256"],"start_line":page["start_line"],
            "end_line":page["end_line"],"total_lines":page["total_lines"],"content":content});
        if next.is_some() {
            entry["next_offset"] = page["next_offset"].clone();
            entry["next_column"] = page["next_column"].clone();
        }
        out.push(entry);
        let Some(next) = next else {
            return Ok(());
        };
        if *spent >= INLINE_BYTES {
            return Ok(());
        }
        offset = next;
        column = page["next_column"].as_u64().unwrap_or(1);
    }
}

fn reports(store: &Store, c: &Credential, i: &Identity, missing: &[String]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut spent = 0usize;
    for id in missing.iter().take(MAX_ITEMS) {
        if spent >= INLINE_BYTES {
            break;
        }
        let mut args = json!({"report_id":id});
        let mut pages = 0;
        loop {
            let Ok(page) = crate::native_documents::execute(store, c, i, "read_report", &args)
            else {
                break;
            };
            // A deferred oversized reply carries no body; leave the report listed.
            let Some(markdown) = page["markdown"].as_str() else {
                break;
            };
            spent += markdown.len();
            let next = page["next_cursor"].clone();
            out.push(json!({"report_id":id,"markdown":markdown,"next_cursor":next,
                "sha256":page["sha256"],"total_characters":page["total_characters"]}));
            let Some(cursor) = next.as_str() else {
                break;
            };
            // A report is delivered whole (a partial one cannot resume), within a page cap.
            pages += 1;
            if pages >= 25 {
                break;
            }
            args = json!({"cursor":cursor});
        }
    }
    out
}

/// The refusal reply with its inline delivery attached (and the message naming it).
pub(super) fn attach(mut reply: Value, delivery: Value) -> Value {
    reply["structuredContent"]["inline_reads"] = delivery;
    let text = reply["structuredContent"].to_string();
    reply["content"] = json!([{"type":"text","text":text}]);
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::{
        channels,
        lifecycle::TaskMachine,
        store::documents::{Publication, Publisher},
        OpenMode, ProjectKey, StoreChannel,
    };
    use std::os::unix::fs::PermissionsExt;

    fn reviewer() -> (std::path::PathBuf, Store, Identity, Credential) {
        let root = std::env::temp_dir().join(format!(
            "review-delivery-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let owner = TaskMachine::new(&store)
            .create("t", "review", "owner")
            .unwrap();
        let c = store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'_review_1','worker-code-reviewer','Review','[]','[\"**\"]','review','running','ar',NULL)", [])?;
                let c = channels::issue_tx(db, "t", &owner, channels::Principal::Worker { assignment: "ar" })?;
                channels::bind_session_tx(db, &c, "engine")?;
                Ok(c)
            })
            .unwrap();
        let i = store
            .read(|db| channels::authenticate(db, &c.channel_id, c.secret(), true))
            .unwrap();
        (root, store, i, c)
    }

    fn missing(path: &str) -> String {
        format!("{path} ({READ_REASON})")
    }

    /// Owner stores 2026-11 (365 `review_file_coverage_required` refusals): a refusal
    /// now delivers the current pages of the missing changed files, recorded as a
    /// read on the reviewer's channel, so the same review passes the next time. A file
    /// outside the read scope or a protected path is never delivered, and a file
    /// larger than one delivery is continued where the earlier one stopped.
    #[test]
    fn a_coverage_refusal_delivers_the_missing_files_as_a_recorded_read() {
        let (root, store, i, c) = reviewer();
        let small: String = (1..=300).map(|n| format!("{n:>4} {}\n", "x".repeat(60))).collect();
        let big: String = (1..=2500).map(|n| format!("{n:>4} {}\n", "y".repeat(60))).collect();
        std::fs::write(root.join("small.rs"), &small).unwrap();
        std::fs::write(root.join("big.rs"), &big).unwrap();
        std::fs::write(root.join("secret.rs"), "secret\n").unwrap();
        std::fs::write(root.join(".env"), "TOKEN=1\n").unwrap();
        let complete = |path: &str| {
            crate::native_project::read_state(&store, &i, &c, path, None)
                .unwrap()
                .is_some_and(|v| v["complete"] == true)
        };
        let items = vec![
            missing("small.rs"),
            missing(".env"),
            "secret.rs (outside reviewer read scope)".to_owned(),
            missing("big.rs"),
        ];
        let first = inline(&store, &c, &i, &root, false, "review_project_revision_changed", &items)
            .expect("delivery");
        assert_eq!(first["delivered_as_read"], true);
        assert!(first["note"].as_str().unwrap().contains("fresh request key"));
        let paths: Vec<&str> = first["items"].as_array().unwrap().iter()
            .map(|p| p["path"].as_str().unwrap()).collect();
        assert!(paths.contains(&"small.rs") && paths.contains(&"big.rs"));
        assert!(!paths.contains(&".env") && !paths.contains(&"secret.rs"));
        assert!(complete("small.rs"), "delivered file counts as read");
        assert!(!complete("big.rs"), "the budget stops a very large file");
        assert!(crate::native_project::read_state(&store, &i, &c, ".env", None).unwrap().is_none());
        assert!(crate::native_project::read_state(&store, &i, &c, "secret.rs", None).unwrap().is_none());
        // The next refusal continues after the lines already delivered.
        let second = inline(&store, &c, &i, &root, false, "review_file_coverage_required", &[missing("big.rs")])
            .expect("continued delivery");
        assert!(second["items"][0]["start_line"].as_u64().unwrap() > 1);
        let mut guard = 0;
        while !complete("big.rs") && guard < 10 {
            inline(&store, &c, &i, &root, false, "review_file_coverage_required", &[missing("big.rs")]);
            guard += 1;
        }
        assert!(complete("big.rs"));
        // Other codes and non-review channels carry no delivery.
        assert!(inline(&store, &c, &i, &root, false, "open_failed_check", &items).is_none());
        let mut worker = i.clone();
        worker.stage = Some("execution".into());
        assert!(inline(&store, &c, &worker, &root, false, "review_file_coverage_required", &items).is_none());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// `review_predecessor_reports_required` (17 owner refusals): the required report
    /// is delivered through its last page as the same channel-bound receipt a
    /// `read_report` call leaves, so the unread-report check is satisfied.
    #[test]
    fn a_predecessor_refusal_delivers_the_report_through_its_final_page() {
        let (root, store, i, c) = reviewer();
        let publisher = Publisher::new(&store);
        let d = publisher.create_draft("t", "worker", "general", "k").unwrap();
        publisher
            .write_draft("t", "worker", &d.draft_id,
                &format!("Cortex draft ID: `{}`\n\n# Round\n\n{}\n", d.draft_id, "x".repeat(9000)))
            .unwrap();
        let id = publisher
            .publish("t", "worker", &d.draft_id, "k", Publication {
                title: "Round", summary: "Round report", author: "worker",
                source_revision: 0, artifacts: json!([]),
            })
            .unwrap()
            .report_id;
        let delivery = inline(&store, &c, &i, &root, false, "review_predecessor_reports_required", &[id.clone()])
            .expect("report delivery");
        let pages = delivery["items"].as_array().unwrap();
        assert!(pages.len() >= 3 && pages.last().unwrap()["next_cursor"].is_null());
        let read: i64 = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='REPORT_PAGE_READ'
                     AND json_extract(payload_json,'$.report_id')=? AND json_type(payload_json,'$.next_cursor')='null'",
                    [&id], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(read, 1);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
