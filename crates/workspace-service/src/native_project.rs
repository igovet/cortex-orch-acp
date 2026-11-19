//! Channel-local read coverage and no-follow project file effects.
use base64::Engine;
use cortex_core::{
    channels::{self, Credential, Identity},
    lifecycle::{self, waits},
    scheduler::policy,
    CoreError, CoreResult, Store,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::Path,
};
pub(super) fn digest(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
pub(super) fn supports(n: &str) -> bool {
    matches!(
        n,
        "command_status"
            | "project_read"
            | "project_write"
            | "project_edit"
            | "access_request"
            | "read_large_output"
    )
}
fn text<'a>(v: &'a Value, k: &str) -> CoreResult<&'a str> {
    v[k].as_str().ok_or(CoreError::Refused("invalid_arguments"))
}
pub(super) fn safe(path: &str) -> CoreResult<()> {
    if path == "." {
        return Ok(());
    }
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path.split('/').any(|p| {
            p.is_empty()
                || [
                    ".",
                    "..",
                    ".git",
                    ".cortex",
                    ".cortex-dev",
                    ".cortex-dev-owner.json",
                    ".cortex-workspace-dev.json",
                ]
                .contains(&p)
        })
    {
        return Err(CoreError::Refused("private_or_invalid_path"));
    }
    Ok(())
}
pub(super) fn writable(
    db: &rusqlite::Transaction<'_>,
    i: &Identity,
    path: Option<&str>,
) -> CoreResult<()> {
    if i.role != "worker"
        || policy::read_only(
            i.profile.as_deref().unwrap_or(""),
            i.stage.as_deref().unwrap_or(""),
        )
    {
        return Err(CoreError::Refused("writing_worker_required"));
    }
    // A read-only sprint planning task never writes the project.
    if cortex_core::lifecycle::sprint_planning::enabled(db, &i.task)? {
        return Err(CoreError::Refused("read_only_task_writer_refused"));
    }
    let a = i
        .assignment
        .as_deref()
        .ok_or(CoreError::Refused("writing_worker_required"))?;
    waits::check_mutation_tx(db, &i.task, a)?;
    if let Some(path) = path {
        safe(path)?;
        // The owner's write denials in force at this write (lifted ones excluded).
        if cortex_core::lifecycle::steering::denials(db, &i.task)?
            .iter()
            .any(|p| policy::overlaps(path, p))
        {
            return Err(CoreError::Refused("write_denied"));
        }
    }
    Ok(())
}
fn authorize(
    db: &rusqlite::Transaction<'_>,
    i: &Identity,
    c: &Credential,
    path: Option<&str>,
) -> CoreResult<()> {
    let current = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
    if &current != i {
        return Err(CoreError::Refused("channel_identity_changed"));
    }
    writable(db, i, path)
}
fn openat(folder: &File, name: &str, flags: i32) -> CoreResult<File> {
    let name = std::ffi::CString::new(name).map_err(|_| CoreError::Refused("invalid_path"))?;
    let fd = unsafe {
        libc::openat(
            folder.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o644,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn parent(root: &Path, path: &str, create: bool) -> CoreResult<(File, String)> {
    safe(path)?;
    if path == "." {
        return Err(CoreError::Refused("regular_file_required"));
    }
    let mut parts = path.split('/').collect::<Vec<_>>();
    let name = parts.pop().unwrap().to_owned();
    let mut folder = File::open(root)?;
    for part in parts {
        if create {
            let c = std::ffi::CString::new(part).unwrap();
            let rc = unsafe { libc::mkdirat(folder.as_raw_fd(), c.as_ptr(), 0o755) };
            if rc < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        folder = openat(&folder, part, libc::O_RDONLY | libc::O_DIRECTORY)?;
    }
    Ok((folder, name))
}
fn bytes(folder: &File, name: &str) -> CoreResult<Option<(Vec<u8>, u32)>> {
    let file = match openat(folder, name, libc::O_RDONLY) {
        Ok(f) => f,
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let m = file.metadata()?;
    if !m.is_file() || m.nlink() != 1 {
        return Err(CoreError::Refused("regular_unlinked_file_required"));
    }
    let mut b = Vec::new();
    // Text has no attachment-size cap; line delivery remains bounded.
    file.take(u64::MAX).read_to_end(&mut b)?;
    Ok(Some((b, m.mode() & 0o777)))
}
fn coverage_key(path: &str) -> String {
    format!("file-read:{}", digest(path.as_bytes()))
}
fn coverage(
    store: &Store,
    i: &Identity,
    c: &Credential,
    path: &str,
    hash: &str,
) -> CoreResult<Value> {
    Ok(read_state(store, i, c, path)?
        .filter(|v| v["sha256"] == hash)
        .unwrap_or(json!({"sha256":hash,"lines":[],"complete":false})))
}
fn read_state(
    store: &Store,
    i: &Identity,
    c: &Credential,
    path: &str,
) -> CoreResult<Option<Value>> {
    let old: Option<String> = store.read(|db| Ok(db.query_row(
        "SELECT result_json FROM runtime_requests WHERE task_id=? AND request_key=? AND actor=? AND operation='native_file_read'",
        (&i.task, coverage_key(path), &c.channel_id), |r| r.get(0)).optional()?))?;
    old.map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}
fn require_read(state: Option<&Value>, hash: &str, complete: bool) -> CoreResult<()> {
    if state.is_none() || (complete && state.unwrap()["complete"] != true) {
        return Err(CoreError::Refused(if complete {
            "gateway_complete_read_required"
        } else {
            "gateway_read_required"
        }));
    }
    if state.unwrap()["sha256"] != hash {
        return Err(CoreError::Refused("gateway_stale_read"));
    }
    Ok(())
}
fn file_error(code: &str) -> Value {
    let correction = match code {
        "gateway_read_required" | "gateway_complete_read_required" =>
            Some("Read the current target with project_read on this channel. For whole-file replacement, finish all non-truncated pages and use full_content_sha256; a partial sha256 is identity only. Use project_edit for partial changes."),
        "gateway_stale_read" =>
            Some("Read the current target on this channel and rebuild the patch or replacement; never blindly retry a stale patch."),
        "gateway_write_conflict" =>
            Some("expected_sha256 must match the current bytes, or absent for creation. Read the current target and rebuild the proposal."),
        _ => None,
    };
    if let Some(correction) = correction {
        let correction = format!(
            "{correction} Nothing was changed. This is a correctable precondition, not an access refusal. No owner question or extra approval is needed under existing authority. Keyless failures may be re-proposed after correction; an explicit failed key needs a fresh key.");
        let expected = match code {
            "gateway_complete_read_required" => "a current complete read",
            "gateway_read_required" => "a prior text read",
            "gateway_stale_read" => "the content last read by this channel",
            _ => "the current SHA-256, or absent for creation",
        };
        let detail = json!({"error":code,"field":if code == "gateway_write_conflict" { "expected_sha256" } else { "path" },
            "expected":expected,"write_state":"rejected","message":correction,"correction":correction});
        json!({"content":[{"type":"text","text":detail.to_string()}],"isError":true,"structuredContent":detail})
    } else {
        json!({"content":[{"type":"text","text":code}],"isError":true})
    }
}
fn save_coverage(
    store: &Store,
    i: &Identity,
    c: &Credential,
    path: &str,
    v: &Value,
) -> CoreResult<()> {
    let mut v = v.clone();
    v["observed"] = json!(true);
    store.write(|db|{let current=channels::authenticate(db,&c.channel_id,c.secret(),true)?;if &current!=i{return Err(CoreError::Refused("channel_identity_changed"));}db.execute("INSERT INTO runtime_requests VALUES (?,?,?,'native_file_read',?,?) ON CONFLICT(task_id,actor,request_key) DO UPDATE SET digest=excluded.digest,result_json=excluded.result_json WHERE operation='native_file_read'",
        (&i.task,coverage_key(path),&c.channel_id,v["sha256"].as_str().unwrap_or("absent"),v.to_string()))?;Ok(())})
}
fn mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[255, 216, 255]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}
fn read(store: &Store, i: &Identity, c: &Credential, args: &Value) -> CoreResult<Value> {
    let path = text(args, "path")?;
    safe(path)?;
    let root = store.key().root();
    let at = root.join(path);
    // Descriptor traversal is used for files; never read through a directory alias.
    let mut current = root.to_path_buf();
    if path != "." {
        for part in path.split('/') {
            current.push(part);
            match current.symlink_metadata() {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(CoreError::Refused("symlink_refused"))
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let v = json!({"exists":false,"path":path,"sha256":"absent","parent_exists":at.parent().is_some_and(Path::is_dir)});
                    save_coverage(
                        store,
                        i,
                        c,
                        path,
                        &json!({"sha256":"absent","complete":true}),
                    )?;
                    return Ok(crate::native_sources::reply(v));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    let start = args["offset"].as_u64().unwrap_or(1) as usize;
    let limit = args["limit"].as_u64().unwrap_or(2000) as usize;
    if at.is_dir() {
        let held = if path == "." {
            File::open(root)?
        } else {
            let (folder, name) = parent(root, path, false)?;
            openat(&folder, &name, libc::O_RDONLY | libc::O_DIRECTORY)?
        };
        let mut entries = fs::read_dir(format!("/proc/self/fd/{}", held.as_raw_fd()))?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        entries.retain(|s| safe(s).is_ok());
        entries.sort();
        let total = entries.len();
        let mut selected = Vec::new();
        for entry in entries.into_iter().skip(start - 1).take(limit) {
            selected.push(entry);
            if serde_json::to_vec(&selected)?.len() > 8000 {
                selected.pop();
                break;
            }
        }
        return Ok(crate::native_sources::reply(
            json!({"path":path,"entries":selected,"total_entries":total,"next_offset":if start-1+selected.len()<total{json!(start+selected.len())}else{Value::Null}}),
        ));
    }
    let (folder, name) = parent(root, path, false)?;
    let (b, _) = bytes(&folder, &name)?.ok_or(CoreError::Refused("file_missing"))?;
    let hash = digest(&b);
    let mut meta = json!({"path":path,"sha256":hash,"size_bytes":b.len()});
    if let Some(mime) = mime(&b) {
        if b.len() > 10 * 1024 * 1024 {
            return Err(CoreError::Refused("file_too_large"));
        }
        meta["mime"] = json!(mime);
        save_coverage(
            store,
            i,
            c,
            path,
            &json!({"sha256":hash,"lines":[],"complete":true}),
        )?;
        let mut r = crate::native_sources::reply(meta);
        r["content"].as_array_mut().unwrap().push(json!({"type":"image","mimeType":mime,"data":base64::engine::general_purpose::STANDARD.encode(b)}));
        return Ok(r);
    }
    let Ok(text) = std::str::from_utf8(&b) else {
        meta["binary"] = json!(true);
        return Ok(crate::native_sources::reply(meta));
    };
    if text.contains('\0') {
        meta["binary"] = json!(true);
        save_coverage(
            store,
            i,
            c,
            path,
            &json!({"sha256":hash,"lines":[],"complete":true}),
        )?;
        return Ok(crate::native_sources::reply(meta));
    }
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let mut content = String::new();
    let mut truncated = Vec::new();
    let mut covered = coverage(store, i, c, path, &hash)?;
    let mut seen = covered["lines"].as_array().cloned().unwrap_or_default();
    let prior_complete = covered["complete"] == true;
    let mut end = start - 1;
    for (idx, line) in lines.iter().enumerate().skip(start - 1).take(limit) {
        let mut cut = line.chars().count() > 2000;
        let rendered = line.chars().take(2000).collect::<String>();
        let mut rendered = if args["line_numbers"] == false {
            rendered
        } else {
            format!("{}\t{}", idx + 1, rendered)
        };
        if serde_json::to_vec(&format!("{content}{rendered}"))?.len() > 8000 {
            if !content.is_empty() {
                break;
            }
            while serde_json::to_vec(&rendered)?.len() > 8000 {
                rendered.pop();
            }
            cut = true;
        }
        content.push_str(&rendered);
        end = idx + 1;
        if cut {
            truncated.push(idx + 1);
        } else if !seen.contains(&json!(idx + 1)) {
            seen.push(json!(idx + 1));
        }
    }
    covered["lines"] = json!(seen);
    covered["complete"] = json!(prior_complete || seen.len() == lines.len());
    save_coverage(store, i, c, path, &covered)?;
    meta["content"] = json!(content);
    meta["total_lines"] = json!(lines.len());
    meta["start_line"] = json!(start);
    meta["end_line"] = json!(end);
    meta["complete"] = json!(start == 1 && end == lines.len() && truncated.is_empty());
    meta["partial"] = json!(meta["complete"] != true);
    meta["next_offset"] = if end < lines.len() {
        json!(end + 1)
    } else {
        Value::Null
    };
    meta["truncated_lines"] = json!(truncated);
    meta["read_coverage_complete"] = covered["complete"].clone();
    meta["full_content_sha256"] = if covered["complete"] == true {
        json!(hash)
    } else {
        Value::Null
    };
    Ok(crate::native_sources::reply(meta))
}
/// Settle only a hash-bound intent. A legacy intent, changed/aliased path or
/// missing observation remains unknown. No effect is replayed, including when
/// the old bytes remain: that is a terminal unchanged acknowledgement.
fn reconcile_file(
    store: &Store,
    task: &str,
    actor: &str,
    key: &str,
    intent: &Value,
) -> CoreResult<Option<Value>> {
    let Some(path) = intent["path"].as_str() else {
        return Ok(None);
    };
    let (Some(before), Some(after)) = (
        intent["before_sha256"].as_str(),
        intent["after_sha256"].as_str(),
    ) else {
        return Ok(None);
    };
    if after.len() != 64 || intent["reply"]["structuredContent"]["sha256"] != after {
        return Ok(None);
    }
    let observed = match parent(store.key().root(), path, false) {
        Ok((folder, name)) => match bytes(&folder, &name) {
            Ok(b) => b.map(|(b, _)| digest(&b)).unwrap_or("absent".into()),
            Err(_) => return Ok(None),
        },
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => "absent".into(),
        Err(_) => return Ok(None),
    };
    let mut settled = intent.clone();
    if observed == after {
        settled["reconciliation"] = json!("after_hash_matches");
    } else if observed == before {
        settled["reply"] = json!({"isError":true,"content":[{"type":"text","text":"file_effect_not_applied"}],
            "structuredContent":{"path":path,"outcome":"unchanged","sha256":observed,"replayed":false}});
        settled["reconciliation"] = json!("before_hash_matches");
    } else {
        return Ok(None);
    }
    settled["complete"] = json!(true);
    settled["replayed"] = json!(false);
    store.write(|db| {
        let count = db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=? AND result_json=?",
            (settled.to_string(), task, actor, key, intent.to_string()))?;
        if count != 1 { return Err(CoreError::Refused("file_intent_changed")); }
        lifecycle::event_tx(db, task, "NATIVE_PROJECT_RECONCILED",
            json!({"channel_id":actor,"request_key":key,"path":path,"sha256":observed,
                "reconciliation":settled["reconciliation"],"replayed":false}))?;
        Ok(())
    })?;
    Ok(Some(settled))
}
#[cfg(test)]
mod precondition_tests {
    use super::*;

    #[test]
    fn read_preconditions_match_the_oracle_priority_and_do_not_grant_authority() {
        let partial = json!({"sha256":"old","complete":false});
        let complete = json!({"sha256":"old","complete":true});
        for (state, hash, whole, expected) in [
            (None, "new", false, "gateway_read_required"),
            (None, "new", true, "gateway_complete_read_required"),
            (
                Some(&partial),
                "old",
                true,
                "gateway_complete_read_required",
            ),
            (
                Some(&partial),
                "new",
                true,
                "gateway_complete_read_required",
            ),
            (Some(&partial), "new", false, "gateway_stale_read"),
            (Some(&complete), "new", true, "gateway_stale_read"),
        ] {
            let error = require_read(state, hash, whole).unwrap_err();
            assert_eq!(error.code(), expected);
            let reply = file_error(error.code());
            assert_eq!(reply["structuredContent"]["write_state"], "rejected");
            assert!(reply["structuredContent"]["correction"]
                .as_str()
                .unwrap()
                .contains("not an access refusal"));
        }
        require_read(Some(&partial), "old", false).unwrap();
        require_read(Some(&complete), "old", true).unwrap();
        // Actual authority decisions must never acquire read-recovery guidance.
        for code in [
            "write_denied",
            "private_or_invalid_path",
            "writing_worker_required",
            "file_outcome_unknown",
        ] {
            assert!(file_error(code).get("structuredContent").is_none());
        }
    }
}
#[cfg(test)]
mod recovery_tests {
    use super::*;
    use cortex_core::{lifecycle::TaskMachine, OpenMode, ProjectKey, StoreChannel};
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "file-recovery-{}",
                crate::native_effects::identity().unwrap()
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            Self(root)
        }
        fn open(&self) -> Store {
            Store::open(
                ProjectKey::new(&self.0, StoreChannel::Dev).unwrap(),
                OpenMode::Initialize,
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn restart_reconciles_after_before_and_diverged_without_replaying() {
        let f = Fixture::new();
        let store = f.open();
        TaskMachine::new(&store)
            .create("t", "recovery", "owner")
            .unwrap();
        for (key, disk) in [("after", "new"), ("before", "old"), ("diverged", "other")] {
            fs::write(f.0.join(key), disk).unwrap();
            let intent = json!({"complete":false,"path":key,"before_sha256":digest(b"old"),"after_sha256":digest(b"new"),
                "reply":crate::native_sources::reply(json!({"sha256":digest(b"new"),"outcome":"written"}))});
            store.write(|db| { db.execute("INSERT INTO runtime_requests VALUES ('t',?,'old-channel','project_edit','bound',?)",
                (key,intent.to_string()))?; Ok(()) }).unwrap();
        }
        // The durable store is really closed and reopened; no old credentials
        // or in-memory read coverage are needed by runtime reconciliation.
        drop(store);
        let store = f.open();
        recover_files(&store).unwrap();
        recover_files(&store).unwrap();
        let rows: Vec<(String, Value)> = store
            .read(|db| {
                Ok(db
                    .prepare(
                        "SELECT request_key,result_json FROM runtime_requests ORDER BY request_key",
                    )?
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            serde_json::from_str::<Value>(&r.get::<_, String>(1)?).unwrap(),
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        assert_eq!(rows[0].1["complete"], true);
        assert_eq!(
            rows[0].1["reply"]["structuredContent"]["outcome"],
            "written"
        );
        assert_eq!(rows[1].1["complete"], true);
        assert_eq!(
            rows[1].1["reply"]["structuredContent"]["outcome"],
            "unchanged"
        );
        assert_eq!(rows[2].1["complete"], false);
        for (key, disk) in [("after", "new"), ("before", "old"), ("diverged", "other")] {
            assert_eq!(fs::read_to_string(f.0.join(key)).unwrap(), disk);
        }
        let count: i64 = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='NATIVE_PROJECT_RECONCILED'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(count, 2);
    }
    #[test]
    fn legacy_alias_and_missing_observations_stay_unknown() {
        let f = Fixture::new();
        let store = f.open();
        TaskMachine::new(&store)
            .create("t", "recovery", "owner")
            .unwrap();
        fs::write(f.0.join("target"), "new").unwrap();
        std::os::unix::fs::symlink("target", f.0.join("alias")).unwrap();
        for (key, intent) in [
            ("legacy", json!({"complete":false})),
            (
                "alias",
                json!({"complete":false,"path":"alias","before_sha256":digest(b"old"),"after_sha256":digest(b"new"),"reply":{"structuredContent":{"sha256":digest(b"new")}}}),
            ),
            (
                "absent",
                json!({"complete":false,"path":"not-created/child","before_sha256":"absent","after_sha256":digest(b"new"),"reply":{"structuredContent":{"sha256":digest(b"new")}}}),
            ),
        ] {
            store.write(|db|{db.execute("INSERT INTO runtime_requests VALUES ('t',?,'old-channel','project_write','bound',?)",(key,intent.to_string()))?;Ok(())}).unwrap();
        }
        recover_files(&store).unwrap();
        let count:i64=store.read(|db|Ok(db.query_row("SELECT count(*) FROM runtime_requests WHERE json_extract(result_json,'$.complete')=1",[],|r|r.get(0))?)).unwrap();
        assert_eq!(count, 1); // absent is observed unchanged, not created
        assert!(!f.0.join("not-created").exists());
        assert_eq!(fs::read_to_string(f.0.join("target")).unwrap(), "new");
    }
}
pub(super) fn recover_files(store: &Store) -> CoreResult<()> {
    let rows = store.read(|db| {
        Ok(db.prepare("SELECT task_id,actor,request_key,result_json FROM runtime_requests
            WHERE operation IN ('project_write','project_edit') AND json_extract(result_json,'$.complete') IS NOT 1")?
            .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?
            .collect::<Result<Vec<_>,_>>()?)
    })?;
    for (task, actor, key, raw) in rows {
        reconcile_file(store, &task, &actor, &key, &serde_json::from_str(&raw)?)?;
    }
    Ok(())
}
pub(super) fn execute(
    store: &Store,
    i: &Identity,
    c: &Credential,
    name: &str,
    args: &Value,
) -> CoreResult<Value> {
    let mut failed_field: Option<String> = None;
    let result = execute_inner(store, i, c, name, args, &mut failed_field);
    let error_reply = |code| match &failed_field {
        Some(field) => crate::native_tool_errors::reply_at(code, field),
        None => file_error(code),
    };
    if let (Err(e), Some(key)) = (&result, args["request_key"].as_str()) {
        if matches!(name, "project_write" | "project_edit") {
            let hash = digest(&serde_json::to_vec(&json!([name, args]))?);
            let reply = error_reply(e.code());
            store.write(|db| {
                db.execute(
                    "INSERT OR IGNORE INTO runtime_requests VALUES (?,?,?,?,?,?)",
                    (
                        &i.task,
                        key,
                        &c.channel_id,
                        name,
                        hash,
                        json!({"complete":true,"reply":reply}).to_string(),
                    ),
                )?;
                Ok(())
            })?;
        }
    }
    match result {
        Err(e) if failed_field.is_some() => Ok(error_reply(e.code())),
        Err(e)
            if matches!(
                e.code(),
                "gateway_read_required"
                    | "gateway_complete_read_required"
                    | "gateway_stale_read"
                    | "gateway_write_conflict"
            ) =>
        {
            Ok(file_error(e.code()))
        }
        other => other,
    }
}
fn execute_inner(
    store: &Store,
    i: &Identity,
    c: &Credential,
    name: &str,
    args: &Value,
    failed_field: &mut Option<String>,
) -> CoreResult<Value> {
    if name == "command_status" {
        return crate::native_command_handles::status(
            store,
            &i.task,
            &c.channel_id,
            text(args, "tool_call_ref")?,
        );
    }
    if name == "project_read" {
        return read(store, i, c, args);
    }
    if name == "read_large_output" {
        return crate::native_commands::read_output(store, i, c, args);
    }
    if name == "access_request" {
        let paths: Vec<String> = serde_json::from_value(args["paths"].clone())?;
        let value = store.write(|db| {
            lifecycle::request_tx(
                db,
                &i.task,
                &format!("native-access-input:{}", c.channel_id),
                text(args, "request_key")?,
                "access_request",
                args,
                || {
                    authorize(db, i, c, None)?;
                    policy::request_access_tx(
                        db,
                        &i.task,
                        &c.channel_id,
                        text(args, "request_key")?,
                        &paths,
                    )
                },
            )
        })?;
        return Ok(crate::native_sources::reply(value));
    }
    let path = text(args, "path")?;
    safe(path)?;
    store.write(|db| authorize(db, i, c, Some(path)))?;
    let hash = digest(&serde_json::to_vec(&json!([name, args]))?);
    let key = args["request_key"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("file:{hash}"));
    let saved:Option<(String,String,String)>=store.read(|db|Ok(db.query_row("SELECT operation,digest,result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
        (&i.task,&c.channel_id,&key),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?))?;
    if let Some((op, h, v)) = saved {
        if op != name || h != hash {
            return Err(CoreError::Refused("request_key_conflict"));
        }
        let v: Value = serde_json::from_str(&v)?;
        let v = if v["complete"] == true {
            v
        } else {
            reconcile_file(store, &i.task, &c.channel_id, &key, &v)?
                .ok_or(CoreError::Refused("file_outcome_unknown"))?
        };
        return Ok(v["reply"].clone());
    }
    let (folder, file) = match parent(store.key().root(), path, false) {
        Ok(p) => p,
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            if name != "project_write" {
                return Err(CoreError::Refused("file_missing"));
            }
            if text(args, "expected_sha256")? != "absent" {
                return Err(CoreError::Refused("gateway_write_conflict"));
            }
            parent(store.key().root(), path, true)?
        }
        Err(e) => return Err(e),
    };
    let old = bytes(&folder, &file)?;
    let old_hash = old
        .as_ref()
        .map(|(b, _)| digest(b))
        .unwrap_or("absent".into());
    let covered = coverage(store, i, c, path, &old_hash)?;
    if name == "project_write" {
        if text(args, "expected_sha256")? != old_hash {
            return Err(CoreError::Refused("gateway_write_conflict"));
        }
        if old.is_some() {
            require_read(read_state(store, i, c, path)?.as_ref(), &old_hash, true)?;
        }
    } else {
        if old.is_none() {
            return Err(CoreError::Refused("file_missing"));
        }
        require_read(read_state(store, i, c, path)?.as_ref(), &old_hash, false)?;
    }
    let previous = old
        .as_ref()
        .map(|(b, _)| std::str::from_utf8(b).map(str::to_owned))
        .transpose()
        .map_err(|_| CoreError::Refused("binary_file"))?
        .unwrap_or_default();
    let mut next = if name == "project_write" {
        text(args, "content")?.to_owned()
    } else {
        let mut next = previous.clone();
        for (index, edit) in args["edits"]
            .as_array()
            .ok_or(CoreError::Refused("invalid_edits"))?
            .iter()
            .enumerate()
        {
            let old = text(edit, "old_string")?;
            let new = text(edit, "new_string")?;
            if old.is_empty() {
                return Err(CoreError::Refused("invalid_edits"));
            }
            let count = next
                .char_indices()
                .filter(|(offset, _)| next[*offset..].starts_with(old))
                .count();
            if old == new {
                *failed_field = Some(format!("edits[{index}].new_string"));
                return Err(CoreError::Refused("gateway_edit_no_change"));
            }
            if count == 0 {
                *failed_field = Some(format!("edits[{index}].old_string"));
                return Err(CoreError::Refused("gateway_edit_not_found"));
            }
            if count > 1 && edit["replace_all"] != true {
                *failed_field = Some(format!("edits[{index}].old_string"));
                return Err(CoreError::Refused("gateway_edit_not_unique"));
            }
            next = if edit["replace_all"] == true {
                next.replace(old, new)
            } else {
                next.replacen(old, new, 1)
            };
        }
        next
    };
    if next.contains('\0') {
        return Err(CoreError::Refused("binary_file"));
    }
    if path == ".gitignore" {
        let rule = format!(
            "/{}/",
            store
                .key()
                .directory()
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
        if !next.lines().any(|l| l == rule) {
            if !next.ends_with('\n') {
                next.push('\n');
            }
            next.push_str(&format!("{rule}\n"));
        }
    }
    let new_hash = digest(next.as_bytes());
    let (added, removed) = line_delta(&previous, &next);
    let complete = path != ".gitignore"
        && (old.is_none() || covered["complete"] == true || name == "project_write");
    let diff = format!(
        "--- {path}\n+++ {path}\n@@ -1,{} +1,{} @@\n{}{}",
        previous.lines().count(),
        next.lines().count(),
        previous
            .lines()
            .map(|l| format!("-{l}\n"))
            .collect::<String>(),
        next.lines().map(|l| format!("+{l}\n")).collect::<String>()
    );
    let bounded = diff.chars().take(4000).collect::<String>();
    let v = crate::native_sources::reply(
        json!({"diff":bounded,"diff_truncated":bounded.len()<diff.len(),"path":path,"outcome":"written","sha256":new_hash,"full_content_sha256":if complete{json!(new_hash)}else{Value::Null},
        "previous_size_bytes":previous.len(),"size_bytes":next.len(),"byte_count":next.len(),"edits_applied":if name=="project_edit"{args["edits"].as_array().map(Vec::len)}else{None},"previous_lines":previous.lines().count(),"new_lines":next.lines().count(),
        "drastic_shrink":next.len()*2<previous.len(),"read_coverage_complete":complete}),
    );
    // Bind both hashes and the acknowledgement before replacement. Recovery
    // observes bytes only; it never re-executes the edit or grants read coverage.
    store.write(|db| {
        authorize(db, i, c, Some(path))?;
        db.execute(
            "INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",
            (
                &i.task,
                &key,
                &c.channel_id,
                name,
                &hash,
                json!({"complete":false,"path":path,"before_sha256":old_hash,"after_sha256":new_hash,"reply":v}).to_string(),
            ),
        )?;
        Ok(())
    })?;
    let temp = format!(".cortex-edit-{}", &crate::native_effects::identity()?[..16]);
    let mut f = openat(
        &folder,
        &temp,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
    )?;
    let effect = (|| -> CoreResult<()> {
        f.write_all(next.as_bytes())?;
        f.set_permissions(fs::Permissions::from_mode(
            old.as_ref().map_or(0o644, |(_, m)| *m),
        ))?;
        f.sync_all()?;
        store.write(|db| authorize(db, i, c, Some(path)))?;
        if bytes(&folder, &file)?
            .as_ref()
            .map(|(b, _)| digest(b))
            .unwrap_or("absent".into())
            != old_hash
        {
            return Err(CoreError::Refused("gateway_stale_read"));
        }
        let src = std::ffi::CString::new(temp.as_str()).unwrap();
        let dest = std::ffi::CString::new(file).unwrap();
        let rc = unsafe {
            if old.is_none() {
                libc::renameat2(
                    folder.as_raw_fd(),
                    src.as_ptr(),
                    folder.as_raw_fd(),
                    dest.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            } else {
                libc::renameat(
                    folder.as_raw_fd(),
                    src.as_ptr(),
                    folder.as_raw_fd(),
                    dest.as_ptr(),
                )
            }
        };
        if rc < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        folder.sync_all()?;
        Ok(())
    })();
    unsafe {
        libc::unlinkat(
            folder.as_raw_fd(),
            std::ffi::CString::new(temp).unwrap().as_ptr(),
            0,
        )
    };
    effect?;
    save_coverage(
        store,
        i,
        c,
        path,
        &json!({"sha256":new_hash,"complete":complete,"lines":[]}),
    )?;
    store.write(|db|{db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?",
        (json!({"complete":true,"reply":v}).to_string(),&i.task,&c.channel_id,&key))?;
        lifecycle::event_tx(db,&i.task,"NATIVE_PROJECT_WRITTEN",json!({"channel_id":c.channel_id,"path":path,"sha256":new_hash,"add":added,"del":removed}))?;Ok(())})?;
    Ok(v)
}
/// Lines added and removed by one write: the common leading and trailing lines
/// are unchanged, the lines between them are replaced. Exact for one contiguous
/// edit and an upper bound for several separated ones.
fn line_delta(previous: &str, next: &str) -> (usize, usize) {
    let (a, b): (Vec<&str>, Vec<&str>) = (previous.lines().collect(), next.lines().collect());
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    (b.len() - head - tail, a.len() - head - tail)
}
#[cfg(test)]
mod line_delta_tests {
    use super::line_delta;
    #[test]
    fn counts_appended_replaced_and_removed_lines() {
        assert_eq!(line_delta("", "one\n"), (1, 0));
        assert_eq!(line_delta("one\n", "one\nThank you!\n"), (1, 0));
        assert_eq!(line_delta("a\nb\nc\n", "a\nX\nc\n"), (1, 1));
        assert_eq!(line_delta("a\nb\n", "a\n"), (0, 1));
        assert_eq!(line_delta("same\n", "same\n"), (0, 0));
    }
}
