//! Trusted ingress snapshots. Bodies are private files, metadata is transactional.
//! A rolled-back admission can leave unreferenced private files, never a replayable input.
use crate::{
    store::{locks, publication::PublicationIntent},
    CoreError, CoreResult, Store,
};
use rusqlite::Transaction;
use serde_json::{json, Value};
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

pub const MAX_BYTES: usize = 10 * 1024 * 1024;
pub struct Block {
    pub index: usize,
    pub kind: String,
    pub mime: Option<String>,
    pub name: Option<String>,
    pub uri: Option<String>,
    pub availability: String,
    pub reason: Option<String>,
    pub body: Option<Vec<u8>>,
}
pub struct Capture {
    pub blocks: Vec<Block>,
}
impl Capture {
    pub fn fingerprint(&self) -> Value {
        json!(self
            .blocks
            .iter()
            .map(|b| json!({"index":b.index,"kind":b.kind,
        "mime":b.mime,"name":b.name,"uri":b.uri,"availability":b.availability,"reason":b.reason,
        "sha256":b.body.as_ref().map(|s|crate::digest(s))}))
            .collect::<Vec<_>>())
    }
    pub(crate) fn persist(
        &self,
        store: &Store,
        db: &Transaction<'_>,
        task: &str,
        session: &str,
        text: &str,
    ) -> CoreResult<Value> {
        let total: usize = self
            .blocks
            .iter()
            .filter_map(|b| b.body.as_ref())
            .map(Vec::len)
            .sum();
        if total > MAX_BYTES
            || self.blocks.iter().any(|b| {
                b.body.as_ref().is_some_and(|s| s.len() > MAX_BYTES)
                    || (b.availability == "stored") != b.body.is_some()
            })
        {
            return Err(CoreError::Refused("invalid_input_capture"));
        }
        // Start is revision 1; each later owner input with attachments is the
        // next source revision. Every engine prompt of the task carries all
        // retained bodies, so their sum stays within one prompt's cap.
        let revision: i64 = db.query_row(
            "SELECT coalesce(max(revision),0)+1 FROM source_revisions WHERE task_id=?",
            [task],
            |r| r.get(0),
        )?;
        let retained: i64 = db.query_row(
            "SELECT coalesce(sum(b.size_bytes),0) FROM source_blocks b JOIN source_revisions r
             ON r.report_id=b.report_id WHERE b.task_id=? AND b.availability='stored'",
            [task],
            |r| r.get(0),
        )?;
        if retained.max(0) as usize + total > MAX_BYTES {
            return Err(CoreError::Refused(
                "task_attachment_total_exceeds_10mib_cap",
            ));
        }
        let id = format!("r_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let directory = store.key().directory().join(task);
        locks::private_directory(&directory, true)?;
        let bodies = directory.join(".source_blocks");
        locks::private_directory(&bodies, true)?;
        let filename = format!("{id}.md");
        let (intent, _journal) =
            PublicationIntent::prepare(&directory, &id, &filename, text, None)?;
        intent.install(&directory)?;
        db.execute(
            "INSERT INTO reports VALUES (?,?,'source',?,?)",
            (&id, task, &filename, crate::now()),
        )?;
        db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES (?,'User input','Retained direct user input','user',?,?,?)",
            (&id,crate::now(),intent.digest(),text.len() as i64))?;
        let mut descriptors = Vec::new();
        for b in &self.blocks {
            let body_ref = b.body.as_ref().map(|_| format!("{id}_{}.bin", b.index));
            if let (Some(body), Some(name)) = (&b.body, &body_ref) {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(bodies.join(name))?;
                file.write_all(body)?;
                file.sync_all()?;
            }
            let hash = b.body.as_ref().map(|s| crate::digest(s));
            let size = b.body.as_ref().map(|s| s.len() as i64);
            db.execute(
                "INSERT INTO source_blocks VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                rusqlite::params![
                    task,
                    &id,
                    b.index as i64,
                    b.kind,
                    b.mime,
                    b.name,
                    b.uri,
                    hash,
                    size,
                    b.availability,
                    b.reason,
                    "user_direct",
                    body_ref,
                    if b.body.is_some() {
                        Some(crate::now())
                    } else {
                        None
                    },
                    crate::now()
                ],
            )?;
            descriptors.push(json!({"kind":b.kind,"mime":b.mime,"name":b.name,"uri":b.uri,"sha256":hash,
                "size_bytes":size,"availability":b.availability,"reason":b.reason,"trust_label":"user_direct",
                "source_ref":format!("{id}:{}",b.index)}));
        }
        locks::sync_directory(&bodies)?;
        locks::sync_directory(&directory)?;
        locks::sync_directory(&store.key().directory())?;
        db.execute(
            "INSERT INTO source_events VALUES (?,?,?,?,NULL,NULL)",
            (format!("acp:{session}"), &id, task, crate::now()),
        )?;
        db.execute(
            "INSERT INTO source_revisions VALUES (?,?,?,?,?,?)",
            (
                task,
                revision,
                session,
                &id,
                &id,
                json!(descriptors).to_string(),
            ),
        )?;
        Ok(json!(descriptors))
    }
}
/// Retain attachments typed with an answer to an open question as the task's
/// next source revision. Nothing is steered or interrupted: the running turn
/// continues, and the next engine prompt of each session carries the new bodies.
pub fn retain_for_answer(
    store: &Store,
    task: &str,
    session: &str,
    text: &str,
    capture: &Capture,
) -> CoreResult<Value> {
    store.write(|db| {
        if crate::lifecycle::domain::task_tx(db, task)?.0.terminal() {
            return Err(CoreError::Refused("task_not_resumable"));
        }
        let descriptors = capture.persist(store, db, task, session, text)?;
        crate::lifecycle::event_tx(
            db,
            task,
            "USER_ANSWER_ATTACHMENTS",
            json!({"attachments":descriptors}),
        )?;
        Ok(descriptors)
    })
}

/// Attachment limits of the Workspace wire (`x-limits.attachmentFiles`/`attachmentBytes`).
const WIRE_FILES: usize = 8;
const WIRE_BYTES: usize = 4 * 1024 * 1024;
fn invalid_attachments() -> CoreError {
    CoreError::Refused("invalid_attachments")
}
/// Strict padded standard base64, as the desktop encodes it.
fn decode_base64(data: &str) -> Option<Vec<u8>> {
    let data = data.as_bytes();
    if data.len() % 4 != 0 {
        return None;
    }
    let sextet = |b: u8| -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a') as u32 + 26),
            b'0'..=b'9' => Some((b - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    for (i, chunk) in data.chunks_exact(4).enumerate() {
        let last = (i + 1) * 4 == data.len();
        let (a, b) = (sextet(chunk[0])?, sextet(chunk[1])?);
        if chunk[2] == b'=' {
            // Canonical padding only: unused bits must be zero.
            if !last || chunk[3] != b'=' || b & 15 != 0 {
                return None;
            }
            out.push(((a << 2) | (b >> 4)) as u8);
        } else {
            let c = sextet(chunk[2])?;
            if chunk[3] == b'=' {
                if !last || c & 3 != 0 {
                    return None;
                }
                out.extend([((a << 2) | (b >> 4)) as u8, (((b & 15) << 4) | (c >> 2)) as u8]);
            } else {
                let d = sextet(chunk[3])?;
                out.extend([
                    ((a << 2) | (b >> 4)) as u8,
                    (((b & 15) << 4) | (c >> 2)) as u8,
                    (((c & 3) << 6) | d) as u8,
                ]);
            }
        }
    }
    Some(out)
}
/// Turn the owner's wire `Attachments` (name, MIME type, base64 data) into a
/// retained-input capture. Every file is checked here, whatever transport carried
/// it: at most 8 files, 4 MiB decoded in total, a simple file name (no path
/// separator or control character), a well-formed MIME type and strict base64.
/// A failure retains nothing; the caller refuses the whole message.
pub fn capture_from_wire(items: &Value) -> CoreResult<Capture> {
    workspace_protocol::validate("Attachments", items).map_err(|_| invalid_attachments())?;
    let items = items.as_array().ok_or_else(invalid_attachments)?;
    if items.len() > WIRE_FILES {
        return Err(invalid_attachments());
    }
    let mut total = 0usize;
    let mut blocks = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let name = item["name"].as_str().ok_or_else(invalid_attachments)?;
        let mime = item["mimeType"].as_str().ok_or_else(invalid_attachments)?;
        let data = item["data"].as_str().ok_or_else(invalid_attachments)?;
        if name.trim().is_empty()
            || name == "."
            || name == ".."
            || name.chars().any(|c| c == '/' || c == '\\' || c.is_control())
        {
            return Err(invalid_attachments());
        }
        let token = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".+_-".contains(&c))
        };
        match mime.split_once('/') {
            Some((a, b)) if token(a) && token(b) => {}
            _ => return Err(invalid_attachments()),
        }
        let body = decode_base64(data).ok_or_else(invalid_attachments)?;
        total += body.len();
        if total > WIRE_BYTES {
            return Err(invalid_attachments());
        }
        blocks.push(Block {
            index,
            kind: if mime.to_ascii_lowercase().starts_with("image/") {
                "image"
            } else {
                "resource"
            }
            .into(),
            mime: Some(mime.to_ascii_lowercase()),
            name: Some(name.into()),
            uri: None,
            availability: "stored".into(),
            reason: None,
            body: Some(body),
        });
    }
    Ok(Capture { blocks })
}
