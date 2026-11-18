//! Owner attachment bodies: one private, content-addressed file per body under
//! `<state>/attachments/<sha256>`. Saved Workspace state, commands and replies carry
//! only a reference `{name, mimeType, size, sha256}`; the bytes cross the socket once,
//! in `product.attachment` `upload`, and are read back only by `product.attachment`
//! `read` or at engine delivery.
use super::{native::*, settings};
use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};
/// Most bytes one attachment set (and so one body) may hold.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_FILES: usize = 8;
/// An uploaded body no saved record references yet (a draft not sent) is kept this long.
pub const UNSENT_GRACE: Duration = Duration::from_secs(24 * 3600);
static LAST_SWEEP: AtomicU64 = AtomicU64::new(0);

pub fn directory(state: &Path) -> PathBuf {
    state.join("attachments")
}
fn digest_ok(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn unavailable() -> Refused {
    refuse(
        "attachment_unavailable",
        "This attachment is no longer saved here. Choose the file again.",
    )
}
fn inline_refused() -> Refused {
    refuse(
        "inline_attachment_refused",
        "Attachments are sent as saved references. Attach the file again.",
    )
}
fn invalid() -> Refused {
    refuse("invalid_attachments","Choose valid files: at most 8 files totaling 4 MiB, with readable contents and simple filenames.")
}
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 255
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}
fn plain_mime(mime: &str) -> bool {
    mime.len() <= 100
        && mime.split_once('/').is_some_and(|(a, b)| {
            [a, b].iter().all(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b".+_-".contains(&c))
            })
        })
}
/// A reference: exactly `name`, `mimeType`, `size` and a lowercase hex `sha256`.
pub fn is_reference(item: &Value) -> bool {
    item.as_object().is_some_and(|m| {
        m.len() == 4
            && m.get("name")
                .and_then(Value::as_str)
                .is_some_and(plain_name)
            && m.get("mimeType")
                .and_then(Value::as_str)
                .is_some_and(plain_mime)
            && m.get("size")
                .and_then(Value::as_u64)
                .is_some_and(|s| s as usize <= MAX_BYTES)
            && m.get("sha256")
                .and_then(Value::as_str)
                .is_some_and(digest_ok)
    })
}
fn reference(name: &str, mime: &str, bytes: &[u8], sha: &str) -> Value {
    json!({"name":name,"mimeType":mime,"size":bytes.len(),"sha256":sha})
}
fn body_path(state: &Path, sha: &str) -> Result<PathBuf> {
    if !digest_ok(sha) {
        return Err(unavailable());
    }
    Ok(directory(state).join(sha))
}
/// Store one body. Idempotent: an existing body of the same digest is kept (and its
/// time renewed, so a fresh upload of it is not swept as unsent). Written to a private
/// temporary file, synced, renamed, then the directory is synced.
pub fn put(state: &Path, bytes: &[u8]) -> Result<String> {
    let sha = format!("{:x}", Sha256::digest(bytes));
    let folder = crate::local::private_tree(&directory(state))?;
    let path = folder.join(&sha);
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if meta.is_file() && meta.len() == bytes.len() as u64 {
            if let Ok(file) = OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
            {
                let _ = file.set_modified(SystemTime::now());
            }
            return Ok(sha);
        }
        // A wrong-sized or non-file entry under a digest name is never trusted.
        if meta.is_dir() {
            return Err(refuse("state_unreadable", "Unsafe saved attachment."));
        }
        fs::remove_file(&path)?;
    }
    let temporary = folder.join(format!(".tmp-{}", settings::unique()));
    let result = (|| -> Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&temporary, &path)?;
        File::open(&folder)?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    result.map(|()| sha)
}
/// One body by digest, bounded, confined to the attachment directory, and verified.
pub fn read(state: &Path, sha: &str) -> Result<Vec<u8>> {
    let path = body_path(state, sha)?;
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(unavailable()),
        Err(_) => return Err(unavailable()),
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_BYTES as u64 {
        return Err(unavailable());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES || format!("{:x}", Sha256::digest(&bytes)) != sha {
        return Err(unavailable());
    }
    Ok(bytes)
}
/// The one call whose request carries bytes: validates inline files exactly as before
/// and answers their references.
pub fn upload(state: &Path, files: &Value) -> Result<Value> {
    super::product::attachments(files)?;
    let mut out = Vec::new();
    for item in files.as_array().into_iter().flatten() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(item["data"].as_str().unwrap_or(""))
            .map_err(|_| invalid())?;
        let sha = put(state, &bytes)?;
        out.push(reference(
            item["name"].as_str().unwrap_or(""),
            item["mimeType"].as_str().unwrap_or(""),
            &bytes,
            &sha,
        ));
    }
    maybe_sweep(state);
    Ok(json!(out))
}
/// `product.attachment`: `upload` takes `files`; `read` takes one `sha256` and answers
/// its bytes as base64. No path ever comes from the client.
pub fn action(state: &Path, p: &Value) -> Result<Value> {
    match p["action"].as_str() {
        Some("upload") => upload(state, &p["files"]),
        Some("read") => {
            let sha = string(p, "sha256")?;
            let bytes = read(state, sha)?;
            Ok(json!({"sha256":sha,"size":bytes.len(),
                "data":base64::engine::general_purpose::STANDARD.encode(bytes)}))
        }
        _ => Err(refuse(
            "unsupported_action",
            "This action is not available.",
        )),
    }
}
/// A command's attachment list: references only, each to a saved body of that size,
/// at most 8 totaling 4 MiB. Inline bytes are refused.
pub fn checked_references(state: &Path, items: &Value) -> Result<()> {
    let items = items.as_array().ok_or_else(invalid)?;
    if items.len() > MAX_FILES {
        return Err(invalid());
    }
    let mut total = 0u64;
    for item in items {
        if item.get("data").is_some() {
            return Err(inline_refused());
        }
        if !is_reference(item) {
            return Err(invalid());
        }
        let size = item["size"].as_u64().unwrap_or(0);
        let path = body_path(state, item["sha256"].as_str().unwrap_or(""))?;
        match fs::symlink_metadata(&path) {
            Ok(m) if m.is_file() && m.len() == size => {}
            _ => return Err(unavailable()),
        }
        total += size;
    }
    if total > MAX_BYTES as u64 {
        return Err(invalid());
    }
    Ok(())
}
/// The inline form of a reference list, for Core and engine delivery inside this
/// process only. Each body is read and verified at this moment.
pub fn resolve(state: &Path, items: &Value) -> Result<Value> {
    checked_references(state, items)?;
    let mut out = Vec::new();
    for item in items.as_array().into_iter().flatten() {
        let bytes = read(state, item["sha256"].as_str().unwrap_or(""))?;
        if bytes.len() as u64 != item["size"].as_u64().unwrap_or(u64::MAX) {
            return Err(unavailable());
        }
        out.push(json!({"name":item["name"],"mimeType":item["mimeType"],
            "data":base64::engine::general_purpose::STANDARD.encode(bytes)}));
    }
    Ok(json!(out))
}
/// Every `attachments` list in a command resolved to inline bodies for Core.
pub fn resolve_all(state: &Path, value: &mut Value) -> Result<()> {
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if key == "attachments" && v.is_array() {
                    *v = resolve(state, v)?;
                } else {
                    resolve_all(state, v)?;
                }
            }
        }
        Value::Array(items) => {
            for v in items {
                resolve_all(state, v)?;
            }
        }
        _ => {}
    }
    Ok(())
}
/// For one engine prompt built from saved records (sprint planning): every reference
/// whose body is still here is read now and inlined in this in-memory copy; one that
/// is gone stays a reference, which the prompt names without bytes.
pub fn inline_for_delivery(state: &Path, value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if key == "attachments" {
                    for item in v.as_array_mut().into_iter().flatten() {
                        if !is_reference(item) {
                            continue;
                        }
                        if let Ok(bytes) = read(state, item["sha256"].as_str().unwrap_or("")) {
                            *item = json!({"name":item["name"],"mimeType":item["mimeType"],
                                "data":base64::engine::general_purpose::STANDARD.encode(bytes)});
                        }
                    }
                } else {
                    inline_for_delivery(state, v);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| inline_for_delivery(state, v)),
        _ => {}
    }
}
/// Saved inline bodies under any `attachments` list become references. A body that
/// does not decode is left exactly as it is, so nothing is ever lost.
pub fn externalize(state: &Path, value: &mut Value) -> Result<bool> {
    let mut changed = false;
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if key == "attachments" {
                    for item in v.as_array_mut().into_iter().flatten() {
                        let (Some(name), Some(mime), Some(data)) = (
                            item["name"].as_str(),
                            item["mimeType"].as_str(),
                            item.get("data").and_then(Value::as_str),
                        ) else {
                            continue;
                        };
                        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data)
                        else {
                            continue;
                        };
                        let sha = put(state, &bytes)?;
                        *item = reference(name, mime, &bytes, &sha);
                        changed = true;
                    }
                } else {
                    changed |= externalize(state, v)?;
                }
            }
        }
        Value::Array(items) => {
            for v in items {
                changed |= externalize(state, v)?;
            }
        }
        _ => {}
    }
    Ok(changed)
}
/// Digests referenced under any `attachments` list.
pub fn references(value: &Value, out: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                if key == "attachments" {
                    for item in v.as_array().into_iter().flatten() {
                        if let Some(sha) = item["sha256"].as_str() {
                            out.insert(sha.to_owned());
                        }
                    }
                } else {
                    references(v, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| references(v, out)),
        _ => {}
    }
}
/// Whether any `attachments` list still holds an inline body.
fn inline(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, v)| {
            if key == "attachments" {
                v.as_array().is_some_and(|items| {
                    items
                        .iter()
                        .any(|i| i.get("data").is_some_and(Value::is_string))
                })
            } else {
                inline(v)
            }
        }),
        Value::Array(items) => items.iter().any(inline),
        _ => false,
    }
}
pub fn held(value: &Value) -> HashSet<String> {
    let mut out = HashSet::new();
    references(value, &mut out);
    out
}
/// A loaded product file that still holds inline bodies: the bodies are written first,
/// then (only if the machine lock is free now) the file is re-read under the lock,
/// converted and replaced atomically. Without the lock the caller still gets the
/// references; its own next save, or the next load, writes them.
/// A body that cannot be written now leaves the loaded value inline, as it was saved.
pub fn migrate(state: &Path, project: &str, saved: &mut Value) -> Result<()> {
    if !inline(saved) {
        return Ok(());
    }
    let mut converted = saved.clone();
    if !matches!(externalize(state, &mut converted), Ok(true)) {
        return Ok(());
    }
    *saved = converted;
    if let Ok(Some(_lock)) = settings::Lock::try_new(state, "machine.lock") {
        let path = product_path(state, project)?;
        // Re-read under the lock: a save made meanwhile is converted, never overwritten.
        if let Ok(mut fresh) = settings::read_json(&path, json!({})) {
            if matches!(externalize(state, &mut fresh), Ok(true)) {
                let _ = settings::write_json(&path, &fresh);
            }
        }
    }
    Ok(())
}
/// The files the owner attached to answers, as references the product file keeps under
/// `answerFiles.<task>.attachments`, so they live exactly as long as the task's own
/// messages: through archive, until the task (or its sprint) is deleted or purged.
/// The source is Core's own record of each answered question's stored files
/// (`USER_ANSWER_ATTACHMENTS`, an exact digest per file), so an answer recorded before
/// this existed is covered too, without attributing anything by guess. Only a file whose
/// body is still saved here, at its recorded size, is referenced. Answers true when the
/// product value changed.
pub fn answer_references(
    store: &cortex_core::Store,
    state: &Path,
    project: &str,
    product: &mut Value,
) -> Result<bool> {
    let rows = store
        .read(|db| {
            let mut query = db.prepare(
                "SELECT x.task_id,e.payload_json FROM workspace_tasks t
                 JOIN workspace_executions x ON x.task_id=t.task_id
                 JOIN runtime_events e ON e.task_id=x.runtime_task_id
                 WHERE t.project_id=? AND e.type='USER_ANSWER_ATTACHMENTS' ORDER BY e.id",
            )?;
            let rows = query
                .query_map([project], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .map_err(super::context::core)?;
    if rows.is_empty() {
        return Ok(false);
    }
    if product.get("answerFiles").is_none_or(Value::is_null) {
        product["answerFiles"] = json!({});
    }
    if !product["answerFiles"].is_object() {
        return Err(refuse(
            "state_unreadable",
            "Saved answer files are unreadable.",
        ));
    }
    let mut changed = false;
    for (task, payload) in rows {
        if product["tasks"][&task]["purged"] == true {
            continue;
        }
        let Ok(payload) = serde_json::from_str::<Value>(&payload) else {
            continue;
        };
        for d in payload["attachments"].as_array().into_iter().flatten() {
            if d["availability"] != "stored" {
                continue;
            }
            let item = json!({"name":d["name"],"mimeType":d["mime"],"size":d["size_bytes"],"sha256":d["sha256"]});
            if !is_reference(&item) {
                continue;
            }
            let saved = &mut product["answerFiles"][&task];
            if !saved["attachments"].is_array() {
                *saved = json!({"attachments":[]});
            }
            if saved["attachments"]
                .as_array()
                .is_some_and(|a| a.contains(&item))
            {
                continue;
            }
            let Ok(path) = body_path(state, item["sha256"].as_str().unwrap_or("")) else {
                continue;
            };
            if !fs::symlink_metadata(&path)
                .is_ok_and(|m| m.is_file() && Some(m.len()) == item["size"].as_u64())
            {
                continue;
            }
            if let Some(list) = saved["attachments"].as_array_mut() {
                list.push(item);
                changed = true;
            }
        }
    }
    // A task with nothing kept leaves no empty entry behind.
    if let Some(map) = product["answerFiles"].as_object_mut() {
        map.retain(|_, v| v["attachments"].as_array().is_some_and(|a| !a.is_empty()));
    }
    Ok(changed)
}
/// After an answer commits: its saved files become referenced at once, under the
/// machine lock, so no sweep can take them in between.
pub fn retain_answer_files(root: &Path, state: &Path, project: &str) -> Result<()> {
    let Some(store) = super::context::open(root, cortex_core::OpenMode::LiveReadOnly)? else {
        return Ok(());
    };
    let _lock = settings::Lock::new(state, "machine.lock")?;
    let path = product_path(state, project)?;
    let mut saved = settings::read_json(&path, json!({}))?;
    if answer_references(&store, state, project, &mut saved)? {
        settings::write_json(&path, &saved)?;
    }
    Ok(())
}
/// The answer files of one task are released with it (purge, sprint deletion).
pub fn forget_answers(product: &mut Value, task: &str) {
    if let Some(map) = product
        .get_mut("answerFiles")
        .and_then(Value::as_object_mut)
    {
        map.remove(task);
    }
}
/// Delete every body no saved product file references, except one renewed within
/// `grace` (an upload not sent yet). The caller holds `machine.lock`. Any unreadable
/// product file stops the sweep before anything is deleted.
pub fn sweep_locked(state: &Path, grace: Duration) -> Result<usize> {
    let folder = directory(state);
    let Ok(entries) = fs::read_dir(&folder) else {
        return Ok(0);
    };
    let mut kept = HashSet::new();
    if let Ok(products) = fs::read_dir(state.join("product")) {
        for entry in products.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                references(&settings::read_json(&path, json!({}))?, &mut kept);
            }
        }
    }
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let stored = digest_ok(&name);
        if (stored && kept.contains(&name)) || !(stored || name.starts_with(".tmp-")) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let recent = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_none_or(|age| age < grace);
        if meta.is_file() && !recent && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}
/// After a save that dropped references: sweep now, under the held lock.
pub fn release(state: &Path, before: &HashSet<String>, after: &Value) {
    let now = held(after);
    if before.iter().any(|sha| !now.contains(sha)) {
        let _ = sweep_locked(state, UNSENT_GRACE);
    }
}
/// At most once an hour per process, unsent uploads past their grace are removed.
pub fn maybe_sweep(state: &Path) {
    let now = now_ms();
    let last = LAST_SWEEP.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 3_600_000
        || LAST_SWEEP
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    if let Ok(Some(_lock)) = settings::Lock::try_new(state, "machine.lock") {
        let _ = sweep_locked(state, UNSENT_GRACE);
    }
}
#[cfg(test)]
#[path = "attachment_files_tests.rs"]
mod tests;
