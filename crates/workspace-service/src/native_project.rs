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
        // The project's own agent/rule files (the path itself or below it), unless
        // the owner named or lifted them. An ancestor directory such as `docs` stays writable.
        if cortex_core::lifecycle::steering::rule_file_protections(db, &i.task)?
            .iter()
            .any(|p| path == p || path.starts_with(&format!("{p}/")))
        {
            return Err(CoreError::Refused("rule_file_protected"));
        }
    }
    Ok(())
}
/// Current working checkout, not the logical root used by the shared store.
pub(super) fn task_root(db: &rusqlite::Connection, task: &str) -> CoreResult<std::path::PathBuf> {
    let path: String = db.query_row(
        "SELECT coalesce((SELECT workspace_root FROM task_workspaces WHERE task_id=t.id),
                         project_root) FROM tasks t WHERE id=?",
        [task],
        |r| r.get(0),
    )?;
    Ok(path.into())
}

/// Effective paths for a current writer. Command callers must authenticate the
/// ready channel in this same transaction and separately confine the OS view.
pub(super) fn effective_write_scope(
    db: &rusqlite::Transaction<'_>,
    i: &Identity,
) -> CoreResult<Vec<String>> {
    writable(db, i, None)?;
    let scope: String = db
        .query_row(
            "SELECT n.scope_json FROM runtime_nodes n JOIN runtime_tasks t
          ON n.task_id=t.task_id AND n.revision=t.revision
         WHERE n.task_id=? AND n.assignment_ref=? AND n.status='running'
           AND n.result_json IS NULL AND n.stage='execution'",
            (&i.task, &i.assignment),
            |r| r.get(0),
        )
        .optional()?
        .ok_or(CoreError::Refused("stale_assignment"))?;
    policy::effective_scope_tx(db, &i.task, i.assignment.as_deref(), &scope)
}

/// One-way containment, never the scheduler's symmetric conflict approximation.
/// Literal paths include descendants; '*' and '?' match within a component;
/// a whole '**' component matches zero or more components. Brackets are literal.
pub(super) fn scope_covers(scope: &str, path: &str) -> bool {
    if scope == "." {
        return true;
    }
    if scope.starts_with('/') || scope.split('/').any(|p| matches!(p, "" | "." | "..")) {
        return false;
    }
    if !scope.contains(['*', '?']) {
        return Path::new(path).starts_with(scope);
    }
    let parts: Vec<_> = path.split('/').collect();
    let mut reachable = vec![false; parts.len() + 1];
    reachable[0] = true;
    for pattern in scope.split('/') {
        if pattern == "**" {
            for n in 0..parts.len() {
                reachable[n + 1] |= reachable[n];
            }
        } else {
            for n in (0..parts.len()).rev() {
                reachable[n + 1] = reachable[n] && segment_matches(pattern, parts[n]);
            }
            reachable[0] = false;
        }
    }
    reachable[parts.len()]
}

fn segment_matches(pattern: &str, segment: &str) -> bool {
    let chars: Vec<_> = segment.chars().collect();
    let mut reachable = vec![false; chars.len() + 1];
    reachable[0] = true;
    for ch in pattern.chars() {
        if ch == '*' {
            for n in 0..chars.len() {
                reachable[n + 1] |= reachable[n];
            }
        } else {
            for n in (0..chars.len()).rev() {
                reachable[n + 1] = reachable[n] && (ch == '?' || ch == chars[n]);
            }
            reachable[0] = false;
        }
    }
    reachable[chars.len()]
}

/// Check the target against the writer's current policy. This does not
/// replace descriptor-relative, no-follow traversal for the effect.
pub(super) fn writable_target(
    db: &rusqlite::Transaction<'_>,
    i: &Identity,
    root: &Path,
    path: &str,
    scoped: bool,
) -> CoreResult<()> {
    writable(db, i, Some(path))?;
    if crate::native_engine_policy::project_private(&root.join(path)) {
        return Err(CoreError::Refused("private_or_invalid_path"));
    }
    if scoped {
        if task_root(db, &i.task)? != root {
            return Err(CoreError::Refused("project_context_conflict"));
        }
        if !effective_write_scope(db, i)?
            .iter()
            .any(|s| scope_covers(s, path))
        {
            return Err(CoreError::Refused("write_outside_assignment_scope"));
        }
    }
    Ok(())
}

/// Authorize every missing parent before the first mkdir. The returned names
/// are the only prefixes the effect may create.
pub(super) fn missing_parents(
    root: &Path,
    path: &str,
    mut check: impl FnMut(&str) -> CoreResult<()>,
) -> CoreResult<Vec<String>> {
    let mut missing = Vec::new();
    for (end, _) in path.match_indices('/') {
        let prefix = &path[..end];
        match root.join(prefix).symlink_metadata() {
            Ok(m) if m.is_dir() => {}
            Ok(m) if m.file_type().is_symlink() => {
                return Err(CoreError::Refused("symlink_refused"))
            }
            Ok(_) => return Err(CoreError::Refused("gateway_path_exists")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                check(prefix)?;
                missing.push(prefix.to_owned());
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(missing)
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
// A bind mount can alias another project or store without a symlink.
pub(super) fn openat(folder: &File, name: &str, flags: i32) -> CoreResult<File> {
    let name = std::ffi::CString::new(name).map_err(|_| CoreError::Refused("invalid_path"))?;
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    // RESOLVE_NO_SYMLINKS rejects every component, including the leaf. Adding
    // O_NOFOLLOW here can turn a symlink to a directory into ENOTDIR.
    // openat2 rejects O_NONBLOCK with O_PATH; an O_PATH descriptor never opens
    // a special file for I/O. Ordinary I/O opens remain nonblocking.
    let nonblock = if flags & libc::O_PATH == 0 {
        libc::O_NONBLOCK
    } else {
        0
    };
    how.flags = (flags | libc::O_CLOEXEC | nonblock) as u64;
    if flags & libc::O_CREAT != 0 {
        how.mode = 0o644;
    }
    how.resolve = libc::RESOLVE_NO_XDEV | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_BENEATH;
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            folder.as_raw_fd(),
            name.as_ptr(),
            &how,
            size_of::<libc::open_how>(),
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(libc::ELOOP) => CoreError::Refused("symlink_refused"),
            Some(libc::EXDEV) => CoreError::Refused("project_mount_alias_refused"),
            Some(libc::ENOSYS | libc::EINVAL) => {
                CoreError::Refused("project_traversal_unavailable")
            }
            _ => error.into(),
        });
    }
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}
fn parent(root: &Path, path: &str) -> CoreResult<(File, String)> {
    parent_for_effect(root, path, &[])
}

fn parent_for_effect(root: &Path, path: &str, missing: &[String]) -> CoreResult<(File, String)> {
    safe(path)?;
    if path == "." {
        return Err(CoreError::Refused("regular_file_required"));
    }
    let (prefix, name) = path.rsplit_once('/').unwrap_or(("", path));
    let mut folder = open_project_root(root)?;
    let mut end = 0;
    for part in prefix.split('/').filter(|p| !p.is_empty()) {
        end += part.len();
        let opened = openat(&folder, part, libc::O_RDONLY | libc::O_DIRECTORY);
        folder = match opened {
            Err(CoreError::Io(e))
                if e.kind() == std::io::ErrorKind::NotFound
                    && missing.iter().any(|p| p == &prefix[..end]) =>
            {
                let c = std::ffi::CString::new(part).unwrap();
                let rc = unsafe { libc::mkdirat(folder.as_raw_fd(), c.as_ptr(), 0o755) };
                if rc < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                openat(&folder, part, libc::O_RDONLY | libc::O_DIRECTORY)?
            }
            other => other?,
        };
        end += 1;
    }
    Ok((folder, name.to_owned()))
}

pub(super) fn open_project_root(root: &Path) -> CoreResult<File> {
    use std::os::unix::fs::OpenOptionsExt;
    if root.canonicalize()? != root {
        return Err(CoreError::Refused("project_context_conflict"));
    }
    let folder = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)?;
    // Test the confinement primitive on this very descriptor before any
    // mutation, including a mkdir whose name does not exist yet.
    openat(&folder, ".", libc::O_RDONLY | libc::O_DIRECTORY)
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
/// SHA-256 of a project file's current bytes, read the same way `project_read`
/// reads them (descriptor traversal, no symlinks, a regular unlinked file only).
/// `None` when there are no such bytes to read: missing, a symlink, a directory,
/// a hard-linked or unreadable entry.
pub(super) fn current_sha256(root: &Path, path: &str) -> Option<String> {
    let (folder, name) = parent(root, path).ok()?;
    let (b, _) = bytes(&folder, &name).ok()??;
    Some(digest(&b))
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
            Some("Read the current target with project_read on this channel. For whole-file replacement, finish all pages (continue a truncated line with next_offset and next_column) and use full_content_sha256; a partial sha256 is identity only. Use project_edit for partial changes."),
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
fn read(
    store: &Store,
    i: &Identity,
    c: &Credential,
    args: &Value,
    batched: bool,
) -> CoreResult<Value> {
    read_at(store, i, c, args, batched, store.key().root())
}

fn read_at(
    store: &Store,
    i: &Identity,
    c: &Credential,
    args: &Value,
    batched: bool,
    root: &Path,
) -> CoreResult<Value> {
    let path = text(args, "path")?;
    safe(path)?;
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
            let (folder, name) = parent(root, path)?;
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
    let (folder, name) = parent(root, path)?;
    let (b, _) = bytes(&folder, &name)?.ok_or(CoreError::Refused("file_missing"))?;
    let hash = digest(&b);
    let mut meta = json!({"path":path,"sha256":hash,"size_bytes":b.len()});
    if let Some(mime) = mime(&b) {
        if b.len() > 10 * 1024 * 1024 {
            return Err(CoreError::Refused("file_too_large"));
        }
        meta["mime"] = json!(mime);
        // A direct read delivers the image and is hash-bound coverage of its bytes.
        // The batch refuses image replies (`batch_multimodal_unsupported`): the image
        // never reaches the model there, so a batched read grants no coverage, for a
        // write or for review (an unread changed image is a review limit instead).
        if !batched {
            save_coverage(
                store,
                i,
                c,
                path,
                &json!({"sha256":hash,"lines":[],"complete":true,"content":"image"}),
            )?;
            meta["read_coverage_complete"] = json!(true);
        }
        let mut r = crate::native_sources::reply(meta);
        r["content"].as_array_mut().unwrap().push(json!({"type":"image","mimeType":mime,"data":base64::engine::general_purpose::STANDARD.encode(b)}));
        return Ok(r);
    }
    // Bytes with no text to show (not UTF-8, or containing NUL) are returned by
    // identity only. That read is hash-bound coverage for a write precondition,
    // single or batched; a review treats such a changed file as having no
    // reviewable text (an `unverified` limit), never as reviewed.
    if std::str::from_utf8(&b).map_or(true, |t| t.contains('\0')) {
        meta["binary"] = json!(true);
        save_coverage(
            store,
            i,
            c,
            path,
            &json!({"sha256":hash,"lines":[],"complete":true,"content":"binary"}),
        )?;
        meta["read_coverage_complete"] = json!(true);
        return Ok(crate::native_sources::reply(meta));
    }
    let text = std::str::from_utf8(&b).expect("checked above");
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    // `column` continues the line at `offset` from that 1-based character, so a
    // line longer than one segment is returned whole across consecutive pages.
    let column = args["column"].as_u64().unwrap_or(1) as usize;
    if column > 1
        && lines
            .get(start - 1)
            .is_none_or(|line| column > line.chars().count())
    {
        return Err(CoreError::Refused("invalid_arguments"));
    }
    let mut content = String::new();
    let mut truncated = Vec::new();
    let mut covered = coverage(store, i, c, path, &hash)?;
    let mut seen = covered["lines"].as_array().cloned().unwrap_or_default();
    // Characters of an over-long line already returned contiguously from its
    // start, by line number; the line joins `lines` once every one was returned.
    let mut segments = covered["partial_lines"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let prior_complete = covered["complete"] == true;
    let budget = 8000;
    let mut end = start - 1;
    let mut next_column = Value::Null;
    for (idx, line) in lines.iter().enumerate().skip(start - 1).take(limit) {
        let skip = if idx + 1 == start { column - 1 } else { 0 };
        let total = line.chars().count();
        // At most 2000 characters per segment; a final line break rides along.
        let body = total - usize::from(line.ends_with('\n'));
        let mut take = body.saturating_sub(skip).min(2000);
        if skip + take >= body {
            take = total - skip;
        }
        let mut segment = line.chars().skip(skip).take(take).collect::<String>();
        let number = if args["line_numbers"] == false {
            String::new()
        } else {
            format!("{}\t", idx + 1)
        };
        if serde_json::to_vec(&format!("{content}{number}{segment}"))?.len() > budget {
            if !content.is_empty() {
                break;
            }
            while segment.chars().count() > 1
                && serde_json::to_vec(&format!("{number}{segment}"))?.len() > budget
            {
                segment.pop();
            }
        }
        let reached = skip + segment.chars().count();
        content.push_str(&number);
        content.push_str(&segment);
        end = idx + 1;
        let key = (idx + 1).to_string();
        let prefix = segments.get(&key).and_then(Value::as_u64).unwrap_or(0) as usize;
        let prefix = if skip <= prefix {
            prefix.max(reached)
        } else {
            prefix
        };
        if prefix >= total {
            segments.remove(&key);
            if !seen.contains(&json!(idx + 1)) {
                seen.push(json!(idx + 1));
            }
        } else if prefix > 0 {
            segments.insert(key, json!(prefix));
        }
        if reached < total {
            // The rest of this line is the next page: continue it from `next_column`.
            truncated.push(idx + 1);
            next_column = json!(reached + 1);
            break;
        }
    }
    covered["lines"] = json!(seen);
    if segments.is_empty() {
        covered.as_object_mut().unwrap().remove("partial_lines");
    } else {
        covered["partial_lines"] = Value::Object(segments);
    }
    covered["complete"] = json!(prior_complete || seen.len() == lines.len());
    meta["content"] = json!(content);
    meta["total_lines"] = json!(lines.len());
    meta["start_line"] = json!(start);
    meta["start_column"] = json!(column);
    meta["end_line"] = json!(end);
    meta["complete"] =
        json!(start == 1 && column == 1 && end == lines.len() && truncated.is_empty());
    meta["partial"] = json!(meta["complete"] != true);
    meta["next_offset"] = if !next_column.is_null() {
        json!(end)
    } else if end < lines.len() {
        json!(end + 1)
    } else {
        Value::Null
    };
    meta["next_column"] = next_column;
    meta["truncated_lines"] = json!(truncated);
    meta["read_coverage_complete"] = covered["complete"].clone();
    meta["full_content_sha256"] = if covered["complete"] == true {
        json!(hash)
    } else {
        Value::Null
    };
    // Use the same structured result and byte bound as batch delivery. A
    // retained output lookup does not record which project lines were read, so a
    // page the batch would defer records nothing and claims only the coverage
    // already recorded before it.
    if !batched || serde_json::to_vec(&meta)?.len() <= crate::native_batch::ITEM_BYTES {
        save_coverage(store, i, c, path, &covered)?;
    } else {
        meta["read_coverage_complete"] = json!(prior_complete);
        meta["full_content_sha256"] = if prior_complete {
            json!(hash)
        } else {
            Value::Null
        };
        meta["coverage_recorded"] = json!(false);
    }
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
    let workspace = intent["workspace"].as_str().map(Path::new);
    if let Some(workspace) = workspace {
        let current = store.read(|db| task_root(db, task))?;
        if workspace != current {
            return Ok(None);
        }
    }
    let observed = match parent(workspace.unwrap_or_else(|| store.key().root()), path) {
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
mod effective_scope_tests {
    use super::*;
    use cortex_core::{lifecycle::TaskMachine, OpenMode, ProjectKey, StoreChannel};

    #[test]
    fn scope_matching_never_uses_a_symmetric_overlap_as_a_grant() {
        for (scope, path, allowed) in [
            (".", "src/lib.rs", true),
            ("**/*", "src", true),
            ("src", "src/lib.rs", true),
            ("src/lib.rs", "src", false),
            ("src", "src2/lib.rs", false),
            ("src/**", "src", true),
            ("src/**", "src/deep/lib.rs", true),
            ("src/**/test?.rs", "src/deep/test1.rs", true),
            ("src/*.rs", "src/deep/lib.rs", false),
            ("src/*.rs", "src/lib.rs", true),
            ("routes/[slug]", "routes/[slug]/page.rs", true),
            ("routes/[slug]", "routes/s/page.rs", false),
            ("../src", "src/lib.rs", false),
        ] {
            assert_eq!(scope_covers(scope, path), allowed, "{scope:?} {path:?}");
        }
    }

    #[test]
    fn grants_require_a_live_current_assignment_and_do_not_override_denials() {
        let root = std::env::temp_dir().join(format!(
            "project-authority-{}",
            crate::native_effects::identity().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let owner = TaskMachine::new(&store)
            .create("t", "Scoped work", "owner")
            .unwrap();
        let c = store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src/**\"]','execution','running','a',NULL)", [])?;
            let c = channels::issue_tx(db, "t", &owner, channels::Principal::Worker { assignment: "a" })?;
            channels::bind_session_tx(db, &c, "engine")?;
            Ok(c)
        }).unwrap();
        let i = store
            .read(|db| channels::authenticate(db, &c.channel_id, c.secret(), true))
            .unwrap();
        let check_with_scope = |path: &str, scoped: bool| {
            store
                .write(|db| {
                    authorize(db, &i, &c, None)?;
                    writable_target(db, &i, &root, path, scoped)
                })
                .err()
                .map(|e| e.code())
        };
        let check = |path: &str| check_with_scope(path, true);
        assert_eq!(check_with_scope("other/a.rs", false), None);
        assert_eq!(check("src/a.rs"), None);
        assert_eq!(check("other/a.rs"), Some("write_outside_assignment_scope"));
        store
            .write(|db| {
                policy::request_access_tx(db, "t", &c.channel_id, "expand", &["other".into()])?;
                Ok(())
            })
            .unwrap();
        assert_eq!(check("other/a.rs"), None);
        assert_eq!(
            check(".cortex-dev/cortex.sqlite3"),
            Some("private_or_invalid_path")
        );
        assert_eq!(check("AGENTS.md"), Some("rule_file_protected"));
        store.write(|db| {
            lifecycle::event_tx(db, "t", "USER_STEERING",
                json!({"text":"deny-write: other","deny_write":["other"],"request":"owner-denial"}))?;
            Ok(())
        }).unwrap();
        assert_eq!(check("other/a.rs"), Some("write_denied"));
        assert_eq!(check_with_scope("other/a.rs", false), Some("write_denied"));
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET revision=2", [])?;
                Ok(())
            })
            .unwrap();
        assert_eq!(check("src/a.rs"), Some("channel_authority_refused"));
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
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
mod long_line_tests {
    use super::*;
    use cortex_core::{
        lifecycle::{completion, TaskMachine},
        OpenMode, ProjectKey, StoreChannel,
    };
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    /// A running review-stage reviewer channel on a fresh project store.
    fn reviewer() -> (Fixture, Store, Identity, Credential) {
        let root = std::env::temp_dir().join(format!(
            "long-lines-{}",
            crate::native_effects::identity().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let f = Fixture(root);
        let store = Store::open(
            ProjectKey::new(&f.0, StoreChannel::Dev).unwrap(),
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
        (f, store, i, c)
    }
    fn page(store: &Store, i: &Identity, c: &Credential, args: Value) -> Value {
        read(store, i, c, &args, false).unwrap()["structuredContent"].take()
    }
    /// Follows next_offset/next_column from `(offset, column)` to the end of the
    /// file: the raw text returned, the page count and the last page.
    fn follow(
        store: &Store,
        i: &Identity,
        c: &Credential,
        path: &str,
        (mut offset, mut column): (Value, Value),
    ) -> (String, usize, Value) {
        let (mut text, mut pages) = (String::new(), 0);
        loop {
            let r = page(
                store,
                i,
                c,
                json!({"path":path,"offset":offset,"column":column,"line_numbers":false}),
            );
            pages += 1;
            text.push_str(r["content"].as_str().unwrap());
            if r["next_offset"].is_null() {
                return (text, pages, r);
            }
            if r["next_column"].is_null() {
                assert_eq!(r["truncated_lines"], json!([]));
                column = json!(1);
            } else {
                // A cut line ends its page and is continued, never skipped.
                assert_eq!(r["truncated_lines"], json!([r["end_line"]]));
                assert_eq!(r["next_offset"], r["end_line"]);
                column = r["next_column"].clone();
            }
            offset = r["next_offset"].clone();
        }
    }
    /// Following next_offset and next_column returns every character of a file
    /// whose lines exceed one segment (by length, and by the inline byte budget of
    /// escaped control characters), and only that contiguous read is coverage.
    #[test]
    fn long_lines_are_read_whole_by_segments_and_only_then_count_as_coverage() {
        let (f, store, i, c) = reviewer();
        let long = format!(
            "{{\n\"ledger\":\"{}\"\n{}\n}}",
            "é0123456789".repeat(450),
            "\u{1}".repeat(2500)
        );
        fs::write(f.0.join("ledger.json"), &long).unwrap();
        fs::write(f.0.join("skipped.json"), &long).unwrap();
        let hash = digest(long.as_bytes());
        let first = page(&store, &i, &c, json!({"path":"ledger.json"}));
        assert_eq!(first["read_coverage_complete"], false);
        assert!(first["full_content_sha256"].is_null());
        let (text, pages, last) = follow(&store, &i, &c, "ledger.json", (json!(1), json!(1)));
        assert_eq!(text, long, "every byte is returned exactly once");
        assert!(pages > 4, "{pages}");
        assert_eq!(last["read_coverage_complete"], true);
        assert_eq!(last["full_content_sha256"], json!(hash));
        // The reviewer's advertised project_read schema accepts the continuation.
        let tools = cortex_engine::schema::catalogue(
            "worker",
            Some("worker-code-reviewer"),
            Some("review"),
        )
        .unwrap();
        let schema = &tools.iter().find(|t| t["name"] == "project_read").unwrap()["inputSchema"];
        let args = json!({"path":"ledger.json","offset":2,"column":2001});
        cortex_engine::schema::validate(schema, &args).unwrap();
        assert!(cortex_engine::schema::validate(schema, &json!({"path":"a","column":0})).is_err());
        // A numbered continuation keeps the line's own number.
        let numbered = page(
            &store,
            &i,
            &c,
            json!({"path":"ledger.json","offset":2,"column":2001}),
        );
        assert!(numbered["content"].as_str().unwrap().starts_with("2\t"));
        assert_eq!(numbered["start_column"], 2001);
        // A column past the end of its line is refused.
        let past = read(
            &store,
            &i,
            &c,
            &json!({"path":"ledger.json","offset":1,"column":3}),
            false,
        );
        assert_eq!(past.unwrap_err().code(), "invalid_arguments");
        // The other copy: the first page, then every page after a gap in line 2.
        let cut = page(&store, &i, &c, json!({"path":"skipped.json"}));
        assert_eq!(
            (cut["truncated_lines"].clone(), cut["next_column"].clone()),
            (json!([2]), json!(2001))
        );
        let (_, _, rest) = follow(&store, &i, &c, "skipped.json", (json!(2), json!(3000)));
        assert_eq!(rest["read_coverage_complete"], false);
        assert!(rest["full_content_sha256"].is_null());
        store
            .write(|db| {
                let file = |path: &str| json!({"path":path,"change":"written","current_sha256":hash});
                lifecycle::event_tx(db, "t", "REVIEW_PACKET", json!({"assignment_ref":"ar","node":"_review_1",
                    "packet":{"changed_files":{"files":[file("ledger.json"),file("skipped.json")],"total":2,"complete":true},
                    "read_scope_covers_change":true,"outside_read_scope":[]}}))?;
                Ok(())
            })
            .unwrap();
        let missing = store
            .read(|db| {
                completion::review_coverage_missing_tx(db, "t", "ar", &json!({"verdict":"APPROVE"}))
            })
            .unwrap();
        assert_eq!(
            missing,
            vec![
                json!({"path":"skipped.json","reason":"current complete read is missing or stale"})
            ]
        );
    }
    /// Owner store 2026-11: a packet named `review_lines` [242,254] for a 253-line
    /// file (a final newline counted as a phantom line 254), and no read could cover
    /// it. Ranges now end at the last line project_read reports.
    #[test]
    fn review_lines_end_at_the_last_line_project_read_reports() {
        let (f, store, i, c) = reviewer();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&f.0)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}");
            String::from_utf8(out.stdout).unwrap()
        };
        let lines: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        fs::write(f.0.join("a.txt"), &lines).unwrap();
        git(&["init", "-q"]);
        git(&["add", "a.txt"]);
        git(&["commit", "-qm", "base"]);
        let base = git(&["rev-parse", "HEAD"]).trim().to_owned();
        fs::write(
            f.0.join("a.txt"),
            lines.replace("line 40\n", "line forty\n"),
        )
        .unwrap();
        let ranges = review_line_ranges(&f.0, &base, "a.txt").unwrap();
        assert_eq!(ranges, vec![[37, 40]]);
        let last = page(
            &store,
            &i,
            &c,
            json!({"path":"a.txt","offset":37,"limit":4}),
        );
        assert_eq!(last["total_lines"], 40);
        assert_eq!(last["end_line"], 40);
        assert!(last["next_offset"].is_null());
    }
    /// Owner stores 2026-11: a changed woff2 font could never be covered (a binary
    /// read recorded nothing), and batched pages over the inline bound were not
    /// recorded although the read claimed coverage, so reviews looped on
    /// `review_file_coverage_required`.
    #[test]
    fn binary_image_and_batched_reads_record_only_what_they_returned() {
        let (f, store, i, c) = reviewer();
        let font = b"wOF2\x00\x01\xff\xfe".repeat(2000);
        let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0u8; 64]].concat();
        let encoded = format!("{}\n", "QUJDRA==".repeat(12)).repeat(400);
        let text: String = (1..=300)
            .map(|n| format!("{n:>4} {}\n", "x".repeat(60)))
            .collect();
        for (name, body) in [
            ("font.woff2", font.as_slice()),
            ("logo.png", png.as_slice()),
            ("blob.txt", encoded.as_bytes()),
            ("code.rs", text.as_bytes()),
        ] {
            fs::write(f.0.join(name), body).unwrap();
        }
        let state = |path: &str| read_state(&store, &i, &c, path).unwrap();
        // A binary read is hash-bound coverage, single or batched.
        let reply = read(&store, &i, &c, &json!({"path":"font.woff2"}), true).unwrap();
        assert_eq!(reply["structuredContent"]["binary"], true);
        assert_eq!(reply["structuredContent"]["read_coverage_complete"], true);
        assert_eq!(state("font.woff2").unwrap()["content"], "binary");
        assert_eq!(state("font.woff2").unwrap()["sha256"], digest(&font));
        // The batch refuses an image reply, so a batched image read grants nothing;
        // a direct read, which shows the image, is hash-bound coverage.
        read(&store, &i, &c, &json!({"path":"logo.png"}), true).unwrap();
        assert!(state("logo.png").is_none());
        let direct = read(&store, &i, &c, &json!({"path":"logo.png"}), false).unwrap();
        assert!(direct["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["type"] == "image"));
        assert_eq!(state("logo.png").unwrap()["content"], "image");
        // A batched page past the inline item bound (which the batch defers) records
        // nothing and claims only the coverage recorded before it.
        let r = read(&store, &i, &c, &json!({"path":"code.rs","limit":300}), true).unwrap();
        let v = &r["structuredContent"];
        assert!(serde_json::to_vec(v).unwrap().len() > crate::native_batch::ITEM_BYTES);
        assert_eq!(v["coverage_recorded"], false);
        assert_eq!(v["read_coverage_complete"], false);
        assert!(v["full_content_sha256"].is_null());
        assert!(state("code.rs").is_none());
        // Pages within the bound are recorded as usual.
        let mut offset = json!(1);
        loop {
            let r = read(
                &store,
                &i,
                &c,
                &json!({"path":"code.rs","offset":offset,"limit":40}),
                true,
            )
            .unwrap();
            let v = &r["structuredContent"];
            assert!(v["coverage_recorded"].is_null());
            if v["next_offset"].is_null() {
                assert_eq!(v["read_coverage_complete"], true);
                break;
            }
            offset = v["next_offset"].clone();
        }
        assert_eq!(state("code.rs").unwrap()["complete"], true);
        // Once complete, a later deferred page still claims that recorded coverage.
        let r = read(&store, &i, &c, &json!({"path":"code.rs","limit":300}), true).unwrap();
        assert_eq!(r["structuredContent"]["read_coverage_complete"], true);
        // The packet marks content without reviewable text; review needs no text read
        // of it, and an approval records it as an unverified limit instead.
        let file = |path: &str, bytes: &[u8]| json!({"path":path,"change":"added","current_sha256":digest(bytes)});
        let mut packet = json!({"changed_files":{"files":[file("font.woff2",&font),file("logo.png",&png),
            file("blob.txt",encoded.as_bytes()),file("code.rs",text.as_bytes()),
            {"path":"stale.bin","change":"added","current_sha256":"0".repeat(64)}],"total":5,"complete":true},
            "read_scope_covers_change":true,"outside_read_scope":[]});
        fs::write(f.0.join("stale.bin"), [0u8, 1, 2]).unwrap();
        add_review_lines(&f.0, None, &mut packet);
        let kinds: Vec<Value> = packet["changed_files"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["review_content"].clone())
            .collect();
        // Content is classified only when its bytes are the packet's current bytes.
        assert_eq!(
            kinds,
            vec![
                json!("binary"),
                json!("image"),
                json!("opaque"),
                Value::Null,
                Value::Null
            ]
        );
        store
            .write(|db| {
                lifecycle::event_tx(
                    db,
                    "t",
                    "REVIEW_PACKET",
                    json!({"assignment_ref":"ar","node":"_review_1","packet":packet}),
                )?;
                Ok(())
            })
            .unwrap();
        let missing = store
            .read(|db| {
                completion::review_coverage_missing_tx(db, "t", "ar", &json!({"verdict":"APPROVE"}))
            })
            .unwrap();
        assert_eq!(
            missing,
            vec![json!({"path":"stale.bin","reason":"current complete read is missing or stale"})]
        );
        let limits = store
            .read(|db| completion::review_change_set_limits_tx(db, "t", "ar"))
            .unwrap();
        assert_eq!(limits.len(), 1, "{limits:?}");
        assert_eq!(limits[0]["outcome"], "unverified");
        // The shown image is reviewed; the font and the encoded blob are not text.
        assert_eq!(limits[0]["paths"], json!(["font.woff2", "blob.txt"]));
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
    /// A writing worker's file write to a project rule file is refused with its own
    /// code unless the owner lifted it; an ancestor directory and other files stay writable.
    #[test]
    fn rule_files_refuse_writes_until_the_owner_lifts_them() {
        let f = Fixture::new();
        let store = f.open();
        TaskMachine::new(&store)
            .create("t", "Implement the parser", "owner")
            .unwrap();
        let i = Identity {
            task: "t".into(),
            attempt: "a".into(),
            principal: "worker".into(),
            epoch: 1,
            role: "worker".into(),
            profile: Some("worker-backend-dev".into()),
            stage: Some("execution".into()),
            assignment: Some("as".into()),
            session: None,
        };
        let check = |path: &'static str| {
            let i = i.clone();
            store
                .write(move |db| writable(db, &i, Some(path)))
                .err()
                .map(|e| e.code())
        };
        for path in [
            "AGENTS.md",
            ".claude/settings.json",
            "docs/project/verification.md",
        ] {
            assert_eq!(check(path), Some("rule_file_protected"), "{path}");
        }
        for path in ["docs", "src/lib.rs", "docs/guide.md", "crates/x/AGENTS.md"] {
            assert_eq!(check(path), None, "{path}");
        }
        store
            .write(|db| {
                lifecycle::event_tx(db, "t", "USER_STEERING",
                    json!({"text":"allow-write: AGENTS.md","allow_write":["AGENTS.md"],"request":"m"}))?;
                Ok(())
            })
            .unwrap();
        assert_eq!(check("AGENTS.md"), None);
        assert_eq!(check("CLAUDE.md"), Some("rule_file_protected"));
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
#[allow(clippy::too_many_arguments)]
pub(super) fn execute(
    store: &Store,
    i: &Identity,
    c: &Credential,
    name: &str,
    args: &Value,
    batched: bool,
    root: &Path,
    scoped: bool,
) -> CoreResult<Value> {
    if name == "project_read" {
        if scoped {
            return read_at(store, i, c, args, batched, root);
        }
        return read(store, i, c, args, batched);
    }
    let mut failed_field: Option<String> = None;
    let result = execute_inner(store, i, c, name, args, &mut failed_field, root, scoped);
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
#[allow(clippy::too_many_arguments)]
fn execute_inner(
    store: &Store,
    i: &Identity,
    c: &Credential,
    name: &str,
    args: &Value,
    failed_field: &mut Option<String>,
    root: &Path,
    scoped: bool,
) -> CoreResult<Value> {
    if name == "command_status" {
        return crate::native_command_handles::status(
            store,
            &i.task,
            &c.channel_id,
            text(args, "tool_call_ref")?,
        );
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
    let root_path = if scoped { root } else { store.key().root() };
    store.write(|db| {
        authorize(db, i, c, None)?;
        writable_target(db, i, root_path, path, scoped)
    })?;
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
    let old = match parent(root_path, path) {
        Ok((folder, file)) => bytes(&folder, &file)?,
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            if name != "project_write" {
                return Err(CoreError::Refused("file_missing"));
            }
            None
        }
        Err(e) => return Err(e),
    };
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
        authorize(db, i, c, None)?;
        writable_target(db, i, root_path, path, scoped)?;
        missing_parents(root_path, path, |parent| {
            writable_target(db, i, root_path, parent, scoped)
        })?;
        db.execute(
            "INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",
            (
                &i.task,
                &key,
                &c.channel_id,
                name,
                &hash,
                json!({"complete":false,"path":path,"workspace":scoped.then_some(root_path),"before_sha256":old_hash,"after_sha256":new_hash,"reply":v}).to_string(),
            ),
        )?;
        Ok(())
    })?;
    store.write(|db| {
        authorize(db, i, c, None)?;
        writable_target(db, i, root_path, path, scoped)?;
        let missing = missing_parents(root_path, path, |parent| {
            writable_target(db, i, root_path, parent, scoped)
        })?;
        let (folder, file) = parent_for_effect(root_path, path, &missing)?;
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
            if bytes(&folder, &file)?
                .as_ref()
                .map(|(b, _)| digest(b))
                .unwrap_or("absent".into())
                != old_hash
            {
                return Err(CoreError::Refused("gateway_stale_read"));
            }
            // The transaction fences state changes, but expiry is wall-clock.
            // Recheck immediately after fsync, before publishing the replacement.
            authorize(db, i, c, None)?;
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
        effect
    })?;
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

/// Lines of a modified, Git-tracked file a reviewer must read: every line of the
/// current file that differs from `base` (the commit checked out when the task's
/// execution started), with three lines of context, as inclusive 1-based
/// `[start, end]` ranges. That difference includes all of the task's changes, also
/// any committed since, and any uncommitted change that predates the task, so it
/// never covers less than the task changed. `None` (the whole file stays required)
/// for a file absent from `base`, a Git failure, or ranges not smaller than the file.
pub(crate) fn review_line_ranges(root: &Path, base: &str, path: &str) -> Option<Vec<[u64; 2]>> {
    if base.len() != 40 || !base.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    const CONTEXT: u64 = 3;
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(["-c", "core.quotePath=false"])
            .args(args)
            .current_dir(root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
    };
    let literal = format!(":(literal){path}");
    run(&["cat-file", "-e", &format!("{base}:{path}")])?;
    let out = run(&[
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--unified=0",
        base,
        "--",
        &literal,
    ])?;
    let text = String::from_utf8(out.stdout).ok()?;
    // Count lines exactly as project_read does (`split_inclusive`): a final line
    // break ends the last line and starts no further one, so a range never names a
    // line past `total_lines` that no read could cover.
    let total = fs::read(root.join(path))
        .ok()?
        .split_inclusive(|b| *b == b'\n')
        .count() as u64;
    let mut ranges: Vec<[u64; 2]> = Vec::new();
    for line in text.lines().filter(|l| l.starts_with("@@ ")) {
        // @@ -a[,b] +c[,d] @@
        let new = line.split(' ').nth(2)?.strip_prefix('+')?;
        let (start, count) = match new.split_once(',') {
            Some((s, n)) => (s.parse::<u64>().ok()?, n.parse::<u64>().ok()?),
            None => (new.parse::<u64>().ok()?, 1),
        };
        // A pure deletion (count 0) sits after line `start`: read around it.
        let (first, last) = if count == 0 {
            (start, start + 1)
        } else {
            (start, start + count - 1)
        };
        let range = [
            first.saturating_sub(CONTEXT).max(1),
            (last + CONTEXT).min(total.max(1)),
        ];
        match ranges.last_mut() {
            Some(prev) if range[0] <= prev[1] + 1 => prev[1] = prev[1].max(range[1]),
            _ => ranges.push(range),
        }
    }
    let required: u64 = ranges.iter().map(|[a, b]| b + 1 - a).sum();
    (!ranges.is_empty() && required < total).then_some(ranges)
}
/// Bytes of a changed file a reviewer cannot review as text: `image`, `binary` (not
/// UTF-8, or containing NUL) or `opaque` (encoded or minified text: at least 8 KiB
/// with lines averaging over 1,000 bytes, or base64-like lines without spaces).
/// `None` for ordinary text, which a review must read.
pub(crate) fn review_text(b: &[u8]) -> Option<&'static str> {
    if mime(b).is_some() {
        return Some("image");
    }
    let Ok(text) = std::str::from_utf8(b) else {
        return Some("binary");
    };
    if text.contains('\0') {
        return Some("binary");
    }
    // An SVG is a picture: its markup is drawing data, not reviewable prose or code.
    let head = text.trim_start_matches('\u{feff}').trim_start();
    if (head.starts_with("<svg") || head.starts_with("<?xml") || head.starts_with("<!--"))
        && text[..text.len().min(4096)].contains("<svg")
    {
        return Some("image");
    }
    if b.len() < 8192 {
        return None;
    }
    let lines = text.split_inclusive('\n').count().max(1);
    if b.len() / lines > 1000 {
        return Some("opaque");
    }
    // Base64/base64url text: every non-empty line uses only that alphabet, and
    // nearly all of them are long (wrapped encoded data, not words or code).
    let body: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let alphabet = |l: &&str| {
        l.trim_end_matches('\r')
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"+/=-_".contains(&c))
    };
    let long = body.iter().filter(|l| l.len() >= 40).count();
    (body.iter().all(alphabet) && long * 10 >= body.len() * 9).then_some("opaque")
}
/// Text a review is not required to read completely: more than about eight
/// `project_read` pages (8,000 bytes each) of required bytes. Large generated
/// data (a 66 KiB page snapshot, twenty pages) had turned every review of a task
/// into coverage refusals (owner store 2026-11-21); such a file is a disclosed
/// limit (`review_content: "bulk"`) unless the review reads it completely.
const BULK_BYTES: usize = 64 * 1024;
/// Bytes of the lines in `ranges` (1-based, inclusive), as `project_read` counts lines.
fn required_bytes(bytes: &[u8], ranges: &[[u64; 2]]) -> usize {
    let lines: Vec<&[u8]> = bytes.split_inclusive(|b| *b == b'\n').collect();
    ranges
        .iter()
        .map(|[start, end]| {
            let first = (*start as usize).saturating_sub(1).min(lines.len());
            let last = (*end as usize).min(lines.len()).max(first);
            lines[first..last].iter().map(|l| l.len()).sum::<usize>()
        })
        .sum()
}
/// Adds `review_lines` to each modified, tracked file of a review packet whose
/// changed lines are fewer than the whole file (`review_line_ranges`), and drops a
/// stale one. Called whenever the packet names the files' current bytes.
/// `base` is the `head` of the packet's `change_boundary.baseline_event`; without
/// one (no commit yet, or it is unknown) every file stays a complete read.
///
/// It also marks a changed file whose current bytes (exactly those of its
/// `current_sha256`) have no reviewable text with `review_content` (`review_text`):
/// such a file is never a missing review read; an approval records it as an
/// `unverified` limit (`completion::review_change_set_limits_tx`) unless the review
/// read it completely (an image directly, so that it was shown). Text whose required
/// bytes (the `review_lines` ranges, or the whole file) exceed `BULK_BYTES` is
/// marked `bulk` the same way.
pub(crate) fn add_review_lines(root: &Path, base: Option<&str>, packet: &mut Value) {
    let Some(files) = packet["changed_files"]["files"].as_array_mut() else {
        return;
    };
    let mut any = false;
    let mut bulk = false;
    for file in files {
        if let Some(entry) = file.as_object_mut() {
            entry.remove("review_lines");
            entry.remove("review_content");
        }
        let Some(hash) = file["current_sha256"].as_str().map(str::to_owned) else {
            continue;
        };
        let Some(path) = file["path"].as_str().map(str::to_owned) else {
            continue;
        };
        let current = parent(root, &path)
            .ok()
            .and_then(|(folder, name)| bytes(&folder, &name).ok().flatten())
            .map(|(b, _)| b)
            .filter(|b| digest(b) == hash);
        let Some(current) = current else {
            continue;
        };
        if let Some(kind) = review_text(&current) {
            file["review_content"] = json!(kind);
            continue;
        }
        let ranges = (file["change"] == "modified")
            .then(|| base.and_then(|base| review_line_ranges(root, base, &path)))
            .flatten();
        // What the review would have to read: the changed ranges, or the whole file.
        let required = match &ranges {
            Some(ranges) => required_bytes(&current, ranges),
            None => current.len(),
        };
        if required > BULK_BYTES {
            file["review_content"] = json!("bulk");
            bulk = true;
            continue;
        }
        if let Some(ranges) = ranges {
            file["review_lines"] = json!(ranges);
            any = true;
        }
    }
    if any {
        packet["changed_files"]["review_lines_guidance"] = json!("A file with review_lines needs only those line ranges of its current bytes read completely with project_read (offset and limit), not the whole file: they are every line that differs from the commit the task started on, with context. Read more of it when you need context. A file without review_lines must be read completely.");
    }
    if let Some(changed) = packet["changed_files"].as_object_mut() {
        changed.remove("review_content_guidance");
        if bulk {
            changed.insert("review_content_guidance".into(), json!("A file with review_content bulk is generated data or longer than about eight project_read pages: it is not required reading. Read what you need of it; unless you read it completely, an approval discloses it as not reviewed in full."));
        }
    }
}
/// The commit checked out when the task's execution started: the `head` of the
/// execution baseline a review packet's change boundary names.
pub(crate) fn review_base(
    db: &rusqlite::Connection,
    task: &str,
    packet: &Value,
) -> CoreResult<Option<String>> {
    use rusqlite::OptionalExtension;
    let Some(event) = packet["change_boundary"]["baseline_event"].as_i64() else {
        return Ok(None);
    };
    Ok(db
        .query_row(
            "SELECT json_extract(payload_json,'$.head') FROM runtime_events
             WHERE id=? AND task_id=? AND type='EXECUTION_BASELINE'",
            (event, task),
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

#[cfg(test)]
mod svg_content_tests {
    use super::review_text;

    /// Owner store 2026-11-21: a review was refused five times for 32 small
    /// generated SVG character parts. An SVG is a picture, never required text.
    #[test]
    fn an_svg_is_an_image_and_ordinary_markup_is_text() {
        for svg in [
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"2\"/></svg>",
            "<?xml version=\"1.0\"?>\n<svg viewBox=\"0 0 4 4\"></svg>\n",
            "\u{feff}<!-- generated -->\n<svg></svg>",
        ] {
            assert_eq!(review_text(svg.as_bytes()), Some("image"), "{svg}");
        }
        for text in [
            "<html><body><svg></svg></body></html>",
            "fn main() {}\n",
            "<?xml version=\"1.0\"?><project/>",
        ] {
            assert_eq!(review_text(text.as_bytes()), None, "{text}");
        }
    }
}

#[cfg(test)]
mod bulk_content_tests {
    use super::{add_review_lines, digest, required_bytes, BULK_BYTES};
    use serde_json::json;

    /// AI-Frontier 2026-11-21: ~66 KiB generated Playwright page snapshots (about
    /// twenty pages) were required reads in every review. Text past about eight pages
    /// is `bulk`, a disclosed limit; ordinary text stays a required read.
    #[test]
    fn text_past_about_eight_pages_is_bulk_and_ordinary_text_is_required() {
        let root = std::env::temp_dir().join(format!("bulk-content-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let big: String = (0..2000)
            .map(|i| format!("- generic [ref=e{i}]: Page snapshot line\n"))
            .collect();
        assert!(big.len() > BULK_BYTES);
        let small = "fn main() {}\n";
        std::fs::write(root.join("page.yml"), &big).unwrap();
        std::fs::write(root.join("main.rs"), small).unwrap();
        let mut packet = json!({"changed_files":{"files":[
            {"path":"page.yml","change":"added","current_sha256":digest(big.as_bytes())},
            {"path":"main.rs","change":"added","current_sha256":digest(small.as_bytes())},
        ]}});
        add_review_lines(&root, None, &mut packet);
        let files = &packet["changed_files"]["files"];
        assert_eq!(files[0]["review_content"], "bulk");
        assert!(files[1]["review_content"].is_null());
        assert!(packet["changed_files"]["review_content_guidance"]
            .as_str()
            .unwrap()
            .contains("not required reading"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn required_bytes_count_only_the_ranges() {
        let text = b"a\nbb\nccc\ndddd\n";
        assert_eq!(required_bytes(text, &[[2, 3]]), 7);
        assert_eq!(required_bytes(text, &[[1, 1], [4, 9]]), 7);
    }
}
