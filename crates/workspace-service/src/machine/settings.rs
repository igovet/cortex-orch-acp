//! Private JSON and fixed-category logs, shared with the retained migration provider.
use super::{git::Git, native::*};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub fn unique() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        now_ms(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
/// Owner operations on one task that run outside the machine lock (Git effects).
/// In-process: integration and merge run only in the service, which owns its state
/// directory.
static ACTIVE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::BTreeMap<(PathBuf, String, String), &'static str>>,
> = std::sync::LazyLock::new(Default::default);
/// Refusal codes a long holder may name; anything else in a holder note is ignored.
const LONG_CODES: [&str; 3] = [
    "git_in_progress",
    "maintenance_in_progress",
    "integration_in_progress",
];
fn open_lock(state: &Path, name: &str) -> Result<File> {
    crate::local::private_tree(state)?;
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(state.join(name))?)
}
fn try_flock(file: &File) -> Result<bool> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let e = std::io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(e.into())
    }
}
/// The note a long holder of `name` leaves beside the lock. The flock stays the
/// truth: the note is read only while the lock is held by someone else, and only if
/// its writer is still alive, so a note left by a killed process is harmless.
fn note_path(state: &Path, name: &str) -> PathBuf {
    state.join(format!("{name}.holder"))
}
fn long_holder(state: &Path, name: &str) -> Option<Refused> {
    let note = read_json(&note_path(state, name), Value::Null).ok()?;
    let pid = i32::try_from(note["pid"].as_u64()?).ok()?;
    let alive = unsafe { libc::kill(pid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    let code = LONG_CODES.iter().find(|c| note["code"] == **c)?;
    (alive && pid > 0).then(|| {
        refuse(
            code,
            note["message"]
                .as_str()
                .unwrap_or("Another operation is running."),
        )
    })
}
pub struct Lock(File, Option<PathBuf>);
impl Lock {
    /// Short holders only keep this lock for a read-modify-write of saved state, so a
    /// waiter normally waits milliseconds. Behind a long holder that named itself (in
    /// this or another process, such as the maintenance worker) the waiter is
    /// refused at once with that holder's reason.
    pub fn new(state: &Path, name: &str) -> Result<Self> {
        let file = open_lock(state, name)?;
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if try_flock(&file)? {
                return Ok(Self(file, None));
            }
            if let Some(refusal) = long_holder(state, name) {
                return Err(refusal);
            }
            if Instant::now() >= deadline {
                return Err(refuse("machine_busy", "Machine state is busy."));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// For recording an effect that already happened (after Git): waits for any
    /// holder, long or short, and never gives up, so a completed effect is always
    /// recorded.
    pub fn wait(state: &Path, name: &str) -> Result<Self> {
        let file = open_lock(state, name)?;
        loop {
            if try_flock(&file)? {
                return Ok(Self(file, None));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// For a holder that runs a long external step (network, Git hooks, a package
    /// install): any other caller of this lock is refused with `code`/`message`
    /// until it is dropped, including callers in other processes.
    pub fn long(
        state: &Path,
        name: &str,
        code: &'static str,
        message: &'static str,
    ) -> Result<Self> {
        debug_assert!(LONG_CODES.contains(&code));
        let mut lock = Self::new(state, name)?;
        let note = note_path(state, name);
        write_json(
            &note,
            &json!({"pid":std::process::id(),"code":code,"message":message}),
        )?;
        lock.1 = Some(note);
        Ok(lock)
    }
    /// The lock if it is free now; `None` while another holder has it. Never waits.
    pub fn try_new(state: &Path, name: &str) -> Result<Option<Self>> {
        let file = open_lock(state, name)?;
        Ok(try_flock(&file)?.then(|| Self(file, None)))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        // The note goes before the lock, so no waiter sees a released lock's note.
        if let Some(note) = self.1.take() {
            let _ = fs::remove_file(note);
        }
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}
/// One task's owner operation running outside the machine lock. Claimed and checked
/// only under the machine lock, so check-and-claim is atomic with the saved state.
pub struct Claim((PathBuf, String, String));
pub fn claim(
    state: &Path,
    project: &str,
    task: &str,
    message: &'static str,
    _lock: &Lock,
) -> Result<Claim> {
    idle(state, project, task, _lock)?;
    let key = (state.to_owned(), project.to_owned(), task.to_owned());
    ACTIVE.lock().unwrap().insert(key.clone(), message);
    Ok(Claim(key))
}
/// Refuses at once, naming the operation, while another owner operation on this
/// task is still running its Git effects.
pub fn idle(state: &Path, project: &str, task: &str, _lock: &Lock) -> Result<()> {
    let key = (state.to_owned(), project.to_owned(), task.to_owned());
    match ACTIVE.lock().unwrap().get(&key) {
        Some(message) => Err(refuse("operation_in_progress", *message)),
        None => Ok(()),
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        ACTIVE.lock().unwrap().remove(&self.0);
    }
}
/// Record an effect that already happened: only `fields` of one task's record (set,
/// or removed when `record` lacks them) go into the saved product as it is now, so
/// every other edit made meanwhile (a title, a target, another task) is kept. Waits
/// for the machine lock without giving up: a completed effect is always recorded.
pub fn save_task(
    state: &Path,
    project: &str,
    task: &str,
    record: &Value,
    fields: &[&str],
) -> Result<()> {
    let _lock = Lock::wait(state, "machine.lock")?;
    let mut saved = product(state, project)?;
    if !saved["tasks"][task].is_object() {
        saved["tasks"][task] = json!({});
    }
    let target = saved["tasks"][task].as_object_mut().unwrap();
    for field in fields {
        match record.get(*field) {
            Some(value) => {
                target.insert((*field).to_owned(), value.clone());
            }
            None => {
                target.remove(*field);
            }
        }
    }
    write_json(&product_path(state, project)?, &saved)
}
pub fn read_json(path: &Path, default: Value) -> Result<Value> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(default),
        Err(e) => return Err(e.into()),
    };
    if !file.metadata()?.is_file() || file.metadata()?.len() > FRAME_BUDGET as u64 {
        return Err(refuse(
            "state_unreadable",
            "Saved Workspace state could not be read.",
        ));
    }
    let mut data = Vec::new();
    file.take(FRAME_BUDGET as u64 + 1).read_to_end(&mut data)?;
    serde_json::from_slice(&data).map_err(|_| {
        refuse(
            "state_unreadable",
            "Saved Workspace state could not be read.",
        )
    })
}
pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| refuse("invalid_path", "Missing parent."))?;
    crate::local::private_tree(parent)?;
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(refuse("state_unreadable", "Unsafe saved state."));
    }
    let temporary = parent.join(format!(".tmp-{}", unique()));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        let data = serde_json::to_vec(value)
            .map_err(|_| refuse("state_unreadable", "Invalid saved state."))?;
        if data.len() > FRAME_BUDGET {
            return Err(refuse(
                "product_full",
                "Saved state exceeds the display limit.",
            ));
        }
        f.write_all(&data)?;
        f.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}
fn default_root(state: &Path) -> PathBuf {
    if workspace_protocol::channel::current().is_dev() {
        return state.join("data/cortex-workspace/worktrees");
    }
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
        .join("cortex-workspace/worktrees")
}
fn expand(path: &str) -> PathBuf {
    if let Some(tail) = path.strip_prefix("~/") {
        home().join(tail)
    } else {
        PathBuf::from(path)
    }
}
fn resolved(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for p in path.components() {
        match p {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => (),
            _ => normalized.push(p),
        }
    }
    if let Ok(real) = fs::canonicalize(&normalized) {
        return Ok(real);
    }
    let mut parent = normalized.as_path();
    let mut tail = Vec::new();
    while !parent.exists() {
        tail.push(
            parent
                .file_name()
                .ok_or_else(|| refuse("unsafe_worktree_root", "Invalid folder."))?
                .to_os_string(),
        );
        parent = parent
            .parent()
            .ok_or_else(|| refuse("unsafe_worktree_root", "Invalid folder."))?;
    }
    let mut real = fs::canonicalize(parent)?;
    for p in tail.iter().rev() {
        real.push(p);
    }
    Ok(real)
}
pub fn machine(state: &Path) -> Result<Value> {
    let mut value = json!({"name":"This machine","retentionDays":30,"autoArchiveDays":7,"worktreeRoot":default_root(state)});
    let saved = read_json(&state.join("machine.json"), json!({}))?;
    if let Some(map) = saved.as_object() {
        for (k, v) in map {
            value[k] = v.clone();
        }
    }
    Ok(value)
}
pub async fn update(state: &Path, p: &Value, projects: &Value) -> Result<Value> {
    let mut patch = p["patch"]
        .as_object()
        .cloned()
        .ok_or_else(|| refuse("invalid_setting", "That setting is not supported."))?;
    if patch.keys().any(|k| {
        !["name", "retentionDays", "autoArchiveDays", "worktreeRoot"].contains(&k.as_str())
    }) {
        return Err(refuse("invalid_setting", "That setting is not supported."));
    }
    for k in ["retentionDays", "autoArchiveDays"] {
        if patch
            .get(k)
            .is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=3650).contains(&n)))
        {
            return Err(refuse(
                "invalid_setting",
                "Use a whole number of days from 1.",
            ));
        }
    }
    if patch.get("name").is_some_and(|v| {
        !v.as_str()
            .is_some_and(|s| !s.trim().is_empty() && s.trim().chars().count() <= 80)
    }) {
        return Err(refuse("invalid_setting", "Use a short name."));
    }
    if let Some(value) = patch.get("worktreeRoot") {
        let text = value
            .as_str()
            .filter(|s| (s.starts_with('/') || s.starts_with("~/")) && !s.contains('\0'))
            .ok_or_else(|| refuse("invalid_setting", "Use an absolute folder, or one under ~."))?;
        let candidate = resolved(&expand(text))?;
        let mut ancestor = candidate.as_path();
        while !ancestor.exists() {
            ancestor = ancestor.parent().unwrap();
        }
        if !ancestor.is_dir()
            || Git::new().is_repo(ancestor).await?
            || projects
                .as_array()
                .into_iter()
                .flatten()
                .any(|p| p["root"].as_str().is_some_and(|r| candidate.starts_with(r)))
        {
            return Err(refuse(
                "unsafe_worktree_root",
                "Choose a writable folder outside every project checkout.",
            ));
        }
        let c = std::ffi::CString::new(ancestor.as_os_str().as_encoded_bytes())
            .map_err(|_| refuse("invalid_setting", "Invalid folder."))?;
        if unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) } != 0 {
            return Err(refuse(
                "unsafe_worktree_root",
                "This location is not writable.",
            ));
        }
        patch.insert("worktreeRoot".into(), json!(candidate));
    }
    let _lock = Lock::new(state, "machine.lock")?;
    let mut value = read_json(&state.join("machine.json"), json!({}))?;
    for (k, v) in patch {
        value[k] = v;
    }
    write_json(&state.join("machine.json"), &value)?;
    Ok(json!({"done":true}))
}
pub fn log_folder(state: &Path) -> Result<PathBuf> {
    Ok(crate::local::private_tree(&state.join("logs"))?)
}
pub fn service_log(state: &Path, event: &str) -> Result<()> {
    if ![
        "helper_ok",
        "helper_refused",
        "helper_failed",
        "agent_started",
        "agent_stopped",
        "scheduler_failed",
    ]
    .contains(&event)
    {
        return Err(refuse("unknown_log_event", "Unknown log event."));
    }
    let _lock = Lock::new(state, "service-log.lock")?;
    let folder = log_folder(state)?;
    let current = folder.join("service.log");
    let previous = folder.join("service.log.1");
    for p in [&current, &previous] {
        if fs::symlink_metadata(p).is_ok_and(|m| !m.is_file() || m.file_type().is_symlink()) {
            return Err(refuse("unsafe_log_file", "Unsafe log file."));
        }
    }
    if fs::metadata(&current).is_ok_and(|m| m.len() >= 65536) {
        fs::rename(&current, &previous)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(current)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    writeln!(file, "{}", json!({"at":now_ms(),"event":event}))?;
    Ok(())
}
fn tree_bytes(path: &Path) -> u64 {
    let mut pending = vec![path.to_path_buf()];
    let mut total = 0;
    let mut count = 0;
    while let Some(path) = pending.pop() {
        if let Ok(rows) = fs::read_dir(path) {
            for row in rows.flatten() {
                count += 1;
                if count > 200000 {
                    return total;
                }
                if let Ok(m) = fs::symlink_metadata(row.path()) {
                    if m.is_dir() {
                        pending.push(row.path());
                    } else {
                        total += m.len();
                    }
                }
            }
        }
    }
    total
}
fn mb(bytes: u64) -> f64 {
    ((bytes as f64 / 1048576.0) * 10.0).round() / 10.0
}
/// On the owner's tool search path, the one worker commands and the verifier use.
pub(super) fn available(tool: &str) -> bool {
    crate::tool_path::locate(tool).is_some()
}
/// Replace the owner's extra tool directories. Every entry is validated before the
/// short `machine.lock` write; the shell is never read under it.
pub fn set_tool_path_extra(state: &Path, p: &Value, projects: &Value) -> Result<Value> {
    let entries = p["directories"]
        .as_array()
        .filter(|a| a.len() <= crate::tool_path::MAX_EXTRA)
        .ok_or_else(|| {
            refuse(
                "invalid_setting",
                "Use a list of at most 16 absolute folders.",
            )
        })?;
    let roots: Vec<PathBuf> = projects
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["root"].as_str().map(PathBuf::from))
        .collect();
    let mut saved: Vec<String> = Vec::new();
    for entry in entries {
        let text = entry
            .as_str()
            .filter(|s| s.len() <= 4096)
            .ok_or_else(|| refuse("invalid_setting", "Use an absolute folder."))?;
        let text = expand(text.trim());
        let text = text.to_string_lossy();
        let stored = crate::tool_path::check_extra(&text, &roots).map_err(|reason| {
            refuse(
                "invalid_tool_directory",
                match reason {
                    "relative" => "Use an absolute folder, or one under ~.",
                    "missing" | "not_a_directory" => "That folder does not exist.",
                    "world_writable" => {
                        "Anyone can write to that folder, so it cannot supply programs."
                    }
                    "private_state" => "That folder is Cortex's private state.",
                    "project_folder" => "Folders inside a project cannot supply programs.",
                    _ => "That folder cannot be used.",
                },
            )
        })?;
        if !saved.contains(&stored) {
            saved.push(stored);
        }
    }
    {
        let _lock = Lock::new(state, "machine.lock")?;
        let mut value = read_json(&state.join("machine.json"), json!({}))?;
        value["toolPathExtra"] = json!(saved);
        write_json(&state.join("machine.json"), &value)?;
    }
    crate::tool_path::set_extra_paths(&saved);
    Ok(crate::tool_path::snapshot())
}
pub fn overview(state: &Path, projects: &Value) -> Result<Value> {
    let mut value = machine(state)?;
    let mut storage = vec![json!({"label":"Workspace state","mb":mb(tree_bytes(state))})];
    let mut failures = 0;
    for p in projects.as_array().into_iter().flatten() {
        let id = p["id"]
            .as_str()
            .ok_or_else(|| refuse("invalid_project", "Unknown project."))?;
        for record in product(state, id)?["tasks"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
        {
            if record["purged"] == true && record["reclamation"]["vacuum_status"] == "failed" {
                failures += 1;
            }
        }
        let root = p["root"]
            .as_str()
            .ok_or_else(|| refuse("invalid_project", "Unknown project."))?;
        let store = super::context::key(Path::new(root))?.directory();
        if store.is_dir() {
            storage.push(json!({"label":p["name"],"mb":mb(tree_bytes(&store))}));
        }
    }
    let runs = read_json(&state.join("machine-runs.json"), json!({}))?;
    value["defaultWorktreeRoot"] = json!(default_root(state));
    value["reclamationFailures"] = json!(failures);
    value["ghAvailable"] = json!(available("gh"));
    value["version"] = json!(env!("CARGO_PKG_VERSION"));
    value["diskMb"] = json!(storage
        .iter()
        .map(|r| r["mb"].as_f64().unwrap_or(0.0))
        .sum::<f64>());
    value["storage"] = json!(storage);
    value["cleanup"] = runs["cleanup"].clone();
    value["diagnostics"] = runs["diagnostics"].clone();
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extra_tool_directories_round_trip_through_machine_settings_and_are_validated() {
        use std::os::unix::fs::PermissionsExt;
        let s = crate::tool_path::tests::Scratch::new("settings");
        let state = s.dir("state");
        write_json(&state.join("machine.json"), &json!({"name":"Desk"})).unwrap();
        let a = s.dir("tools/a");
        let b = s.dir("tools/b");
        let project = s.dir("project");
        let inside = s.dir("project/bin");
        let open = s.dir("open");
        fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).unwrap();
        let projects = json!([{"id":"p","root":project,"name":"P"}]);
        set_tool_path_extra(
            &state,
            &json!({"directories":[a, format!("{}/", b.display()), a]}),
            &projects,
        )
        .unwrap();
        let saved = read_json(&state.join("machine.json"), json!({})).unwrap();
        assert_eq!(saved["name"], "Desk", "other settings are kept");
        assert_eq!(saved["toolPathExtra"], json!([a, b]));
        assert_eq!(
            crate::tool_path::saved_extra(&state),
            [a.to_string_lossy(), b.to_string_lossy()]
        );
        for bad in [
            json!("relative/bin"),
            json!(inside),
            json!(open),
            json!(s.0.join("missing")),
        ] {
            let e =
                set_tool_path_extra(&state, &json!({"directories":[bad]}), &projects).unwrap_err();
            assert_eq!(e.code, "invalid_tool_directory", "{bad}");
        }
        assert!(set_tool_path_extra(&state, &json!({"directories":"x"}), &projects).is_err());
        assert_eq!(
            crate::tool_path::saved_extra(&state).len(),
            2,
            "a refused change saves nothing"
        );
        set_tool_path_extra(&state, &json!({"directories":[]}), &projects).unwrap();
        assert!(crate::tool_path::saved_extra(&state).is_empty());
    }
    #[test]
    fn a_long_holder_in_another_process_is_named_at_once_and_a_dead_ones_note_is_ignored() {
        let state = std::env::temp_dir().join(format!("cw-lock-note-{}", unique()));
        crate::local::private_tree(&state).unwrap();
        // Another process holds the lock: its own open file description and a note.
        let other = open_lock(&state, "maintenance.lock").unwrap();
        assert!(try_flock(&other).unwrap());
        let mut alive = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        write_json(&note_path(&state, "maintenance.lock"),
            &json!({"pid":alive.id(),"code":"maintenance_in_progress","message":"An update is being installed."})).unwrap();
        let started = Instant::now();
        let refused = Lock::new(&state, "maintenance.lock")
            .err()
            .map(|e| (e.code, e.message));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            refused,
            Some((
                "maintenance_in_progress",
                "An update is being installed.".to_owned()
            ))
        );
        // A note whose writer is gone names nothing: the waiter waits for the flock.
        alive.kill().unwrap();
        alive.wait().unwrap();
        let s = state.clone();
        let waiter = std::thread::spawn(move || {
            Lock::new(&s, "maintenance.lock")
                .map(|_| ())
                .map_err(|e| e.code)
        });
        std::thread::sleep(Duration::from_millis(100));
        drop(other);
        assert_eq!(waiter.join().unwrap(), Ok(()));
        // A long holder of this process leaves its note while held, removes it on drop.
        let held = Lock::long(
            &state,
            "git-y.lock",
            "git_in_progress",
            "A push is running.",
        )
        .unwrap();
        assert!(note_path(&state, "git-y.lock").exists());
        drop(held);
        assert!(!note_path(&state, "git-y.lock").exists());
        fs::remove_dir_all(state).unwrap();
    }
    #[test]
    fn a_recorded_effect_waits_through_a_long_holder_and_is_saved() {
        let state = std::env::temp_dir().join(format!("cw-wait-{}", unique()));
        write_json(&product_path(&state, "p").unwrap(),
            &json!({"tasks":{"a":{"title":"Owner title","status":"ready_to_merge","acceptance":{"digest":"d"}}}})).unwrap();
        let held = Lock::long(
            &state,
            "machine.lock",
            "integration_in_progress",
            "Busy fixture.",
        )
        .unwrap();
        assert_eq!(
            Lock::new(&state, "machine.lock").err().map(|e| e.code),
            Some("integration_in_progress")
        );
        let s = state.clone();
        let save =
            std::thread::spawn(move || {
                save_task(&s, "p", "a",
            &json!({"title":"stale copy","status":"done","integration":{"method":"squash"}}),
            &["status", "acceptance", "integration"]).map_err(|e| e.code)
            });
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !save.is_finished(),
            "a recorded effect never gives up behind a holder"
        );
        drop(held);
        assert_eq!(save.join().unwrap(), Ok(()));
        // Only the owned fields changed; the owner's title edit is kept.
        assert_eq!(
            product(&state, "p").unwrap()["tasks"]["a"],
            json!({"title":"Owner title","status":"done","integration":{"method":"squash"}})
        );
        fs::remove_dir_all(state).unwrap();
    }
    #[test]
    fn a_long_holder_refuses_other_callers_at_once_and_short_ones_wait() {
        let state = std::env::temp_dir().join(format!("cw-lock-{}", unique()));
        let held = Lock::long(
            &state,
            "git-x.lock",
            "git_in_progress",
            "A push is running.",
        )
        .unwrap();
        let s = state.clone();
        let started = Instant::now();
        let refused = std::thread::spawn(move || {
            Lock::new(&s, "git-x.lock")
                .err()
                .map(|e| (e.code, e.message))
        })
        .join()
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            refused,
            Some(("git_in_progress", "A push is running.".to_owned()))
        );
        drop(held);
        // A short holder is waited for, never refused.
        let short = Lock::new(&state, "git-x.lock").unwrap();
        let s = state.clone();
        let waiter =
            std::thread::spawn(move || Lock::new(&s, "git-x.lock").map(|_| ()).map_err(|e| e.code));
        std::thread::sleep(Duration::from_millis(50));
        drop(short);
        assert_eq!(waiter.join().unwrap(), Ok(()));
        fs::remove_dir_all(state).unwrap();
    }
    #[test]
    fn a_task_claim_refuses_its_task_only_and_save_task_keeps_other_edits() {
        let state = std::env::temp_dir().join(format!("cw-claim-{}", unique()));
        write_json(&product_path(&state, "p").unwrap(),
            &json!({"tasks":{"a":{"status":"ready_to_merge"},"b":{"title":"old"}},"sprints":[{"id":"s"}]})).unwrap();
        let lock = Lock::new(&state, "machine.lock").unwrap();
        let claim = claim(&state, "p", "a", "Merging a.", &lock).unwrap();
        assert_eq!(
            idle(&state, "p", "a", &lock).err().map(|e| e.code),
            Some("operation_in_progress")
        );
        assert!(idle(&state, "p", "b", &lock).is_ok());
        assert!(idle(&state, "q", "a", &lock).is_ok());
        drop(lock);
        // Another task's edit made while `a` merges is kept by the merge's save.
        let mut edited = product(&state, "p").unwrap();
        edited["tasks"]["b"]["title"] = json!("new");
        write_json(&product_path(&state, "p").unwrap(), &edited).unwrap();
        save_task(&state, "p", "a", &json!({"status":"done"}), &["status"]).unwrap();
        let saved = product(&state, "p").unwrap();
        assert_eq!(saved["tasks"]["a"], json!({"status":"done"}));
        assert_eq!(saved["tasks"]["b"]["title"], "new");
        assert_eq!(saved["sprints"], json!([{"id":"s"}]));
        drop(claim);
        let lock = Lock::new(&state, "machine.lock").unwrap();
        assert!(idle(&state, "p", "a", &lock).is_ok());
        drop(lock);
        fs::remove_dir_all(state).unwrap();
    }
    #[test]
    fn logs_only_fixed_events_rotate_and_refuse_symlinks() {
        let state = std::env::temp_dir().join(format!("cw-logs-{}", unique()));
        for _ in 0..2200 {
            service_log(&state, "helper_ok").unwrap();
        }
        assert_eq!(
            service_log(&state, "prompt-secret").unwrap_err().code,
            "unknown_log_event"
        );
        let folder = log_folder(&state).unwrap();
        assert_eq!(
            fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for path in [folder.join("service.log"), folder.join("service.log.1")] {
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            for line in fs::read_to_string(path).unwrap().lines() {
                let row: Value = serde_json::from_str(line).unwrap();
                assert_eq!(row.as_object().unwrap().len(), 2);
                assert_eq!(row["event"], "helper_ok");
            }
        }
        fs::remove_file(folder.join("service.log")).unwrap();
        std::os::unix::fs::symlink(state.join("outside"), folder.join("service.log")).unwrap();
        assert_eq!(
            service_log(&state, "helper_ok").unwrap_err().code,
            "unsafe_log_file"
        );
        assert!(!state.join("outside").exists());
        fs::remove_dir_all(state).unwrap();
    }
}
pub fn diagnostics(state: &Path, projects: &Value) -> Result<Value> {
    let mut items = Vec::new();
    let origin = |o: &str| match o {
        "shell" => "read from your shell",
        "fallback" => "a usual tool folder",
        "extra" => "a folder you added",
        "system" => "the system folders",
        _ => "the service's own PATH",
    };
    // Resolved exactly where worker commands and the verifier look programs up.
    for (tool, optional) in [
        ("git", false),
        ("node", false),
        ("npm", false),
        ("bwrap", false),
        ("docker", true),
        ("python3", true),
    ] {
        let found = crate::tool_path::locate(tool);
        let ok = found.is_some() || optional;
        let detail = match &found {
            Some((path, o)) => format!("Found at {} ({})", path.display(), origin(o)),
            None if optional => "Not found on the tool search path (optional)".into(),
            None => "Not found on the tool search path".into(),
        };
        items.push(json!({"name":tool,"ok":ok,"optional":optional,"path":found.as_ref().map(|f|&f.0),"detail":detail,
            "action":if ok{String::new()}else{format!("Install {tool}, or add the folder that holds it under Tool search path on this page, then run diagnostics again.")}}));
    }
    let path = crate::tool_path::snapshot();
    let (ok, detail) = match path["source"].as_str() {
        Some("shell") => (
            true,
            format!(
                "Read from your shell ({}), {} folders",
                path["shell"].as_str().unwrap_or("shell"),
                path["resolved"].as_array().map_or(0, Vec::len)
            ),
        ),
        Some("pending") => (true, "Still reading your shell; the usual tool folders are used meanwhile".into()),
        Some("fallback") => (
            path["reason"] == "unsupported_shell",
            format!(
                "Your shell could not be read ({}); the service's PATH and the usual tool folders are used",
                path["reason"].as_str().unwrap_or("unknown")
            ),
        ),
        _ => (true, "The service's own PATH".into()),
    };
    items.push(json!({"name":"Tool search path","ok":ok,"detail":detail,
        "action":if ok{""}else{"Check that your shell starts without waiting for input, then choose Re-read from my shell, or add folders yourself."}}));
    items.push(
        json!({"name":"Rust service","ok":true,"detail":env!("CARGO_PKG_VERSION"),"action":""}),
    );
    let writable = std::ffi::CString::new(state.as_os_str().as_encoded_bytes())
        .is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0);
    items.push(json!({"name":"Private state","ok":writable,"detail":if writable{"Writable"}else{"The app cannot save its private state"},
        "action":if writable{""}else{"Restore write permission for this app's private state folder."}}));
    for p in projects.as_array().into_iter().flatten() {
        let exists = Path::new(p["root"].as_str().unwrap_or("")).is_dir();
        items.push(json!({"name":format!("Project {}",p["name"].as_str().unwrap_or("")),"ok":exists,
            "detail":if exists{"Folder available"}else{"Project folder is missing"},
            "action":if exists{""}else{"Reconnect the drive or register the project at its current location, then run diagnostics again."}}));
    }
    let run = json!({"at":now_ms(),"status":if items.iter().all(|i|i["ok"]==true){"ok"}else{"degraded"},"checks":items.len(),"items":items});
    let _lock = Lock::new(state, "machine.lock")?;
    let path = state.join("machine-runs.json");
    let mut saved = read_json(&path, json!({}))?;
    saved["diagnostics"] = run.clone();
    write_json(&path, &saved)?;
    Ok(run)
}
