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
pub struct Lock(File);
impl Lock {
    pub fn new(state: &Path, name: &str) -> Result<Self> {
        crate::local::private_tree(state)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(state.join(name))?;
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Self(file));
            }
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(e.into());
            }
            if Instant::now() >= deadline {
                return Err(refuse("machine_busy", "Machine state is busy."));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
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
pub(super) fn available(tool: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|p| {
        fs::metadata(p.join(tool)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
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
    for tool in ["git", "node", "npm", "bwrap"] {
        let ok = available(tool);
        items.push(json!({"name":tool,"ok":ok,"detail":if ok{"Available"}else{"Not found by the desktop service"},
            "action":if ok{String::new()}else{format!("Make {tool} available to the desktop service, then run diagnostics again.")}}));
    }
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
