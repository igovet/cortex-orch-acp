//! Owner-run maintenance tools: `uninstall` and `clone-dev-settings`.
//!
//! `uninstall` is reversible-by-default: with no `--remove-*` flag it only prints
//! exactly what it would remove. Real deletion needs a specific `--remove-*` flag
//! AND `--i-confirm-removal`. It deletes only the channel's owner config and state
//! roots (`<config>/cortex`, `<state>/cortex`) and, only with `--project PATH`, that
//! one project's own store directory; each root is re-verified as an owner-private,
//! non-symlinked directory immediately before it is removed. It never touches an
//! engine's own install or account directories, shell or editor configuration, or
//! SSH/Git credentials.
//!
//! `clone-dev-settings` copies only non-secret settings files into the Dev channel
//! config directory. It never reads or copies account or state files, drops every
//! secret-looking key and every executable-argument, environment, URL or history
//! field, keeps only native-account connections (references retained as metadata,
//! authorization required again), disables MCP servers for review, and refuses
//! symlinks and any overlap between source, target and the production paths.
use crate::client_recipes::render;
use crate::config::{owner_roots, Environment};
use crate::py::{self, Py};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

// ----------------------------------------------------------------- uninstall

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// `"absent"`, `"owner_private"` or `"unsafe"`; never deletes anything.
fn directory_state(path: &Path) -> &'static str {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "absent",
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() && meta.uid() == uid() => "owner_private",
        _ => "unsafe",
    }
}

fn tree_bytes(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => tree_bytes(&entry.path()),
            Ok(kind) if kind.is_symlink() => 0,
            _ => entry.metadata().map_or(0, |m| m.len()),
        })
        .sum()
}

fn project_store(project: &Path) -> Result<PathBuf, String> {
    let canonical = project.canonicalize().map_err(|_| "project must be an existing directory".to_string())?;
    if !project.is_absolute() || canonical != project || !canonical.is_dir() {
        return Err("project must be an absolute, canonical directory".into());
    }
    Ok(canonical.join(cortex_acp::startup::project_store_name()))
}

fn plan(project: Option<&Path>) -> Result<Vec<Value>, String> {
    let mut entries: Vec<(&str, Result<PathBuf, String>)> = Vec::new();
    match owner_roots(&Environment::from_process()) {
        Some((config, state)) => {
            entries.push(("user_config", Ok(config)));
            entries.push(("user_state", Ok(state)));
        }
        None => {
            entries.push(("user_config_error", Err("storage_root_unavailable".into())));
            entries.push(("user_state_error", Err("storage_root_unavailable".into())));
        }
    }
    if let Some(project) = project {
        entries.push(("project_cortex_store", Ok(project_store(project)?)));
    }
    Ok(entries
        .into_iter()
        .map(|(scope, path)| match path {
            Err(code) => json!({"scope": scope, "path": null, "status": code, "bytes": 0}),
            Ok(path) => {
                let status = directory_state(&path);
                let bytes = if status == "owner_private" { tree_bytes(&path) } else { 0 };
                json!({"scope": scope, "path": path, "status": status, "bytes": bytes})
            }
        })
        .collect())
}

fn remove_verified(path: &Path) -> Result<Option<String>, String> {
    match directory_state(path) {
        "absent" => Ok(None),
        "unsafe" => Err(format!("refusing to remove {}: not an owner-private real directory.", path.display())),
        _ => fs::remove_dir_all(path).map(|()| Some(path.display().to_string())).map_err(|e| format!("cannot remove {}: {e}", path.display())),
    }
}

pub fn uninstall(argv: &[String]) -> (String, i32) {
    let (mut project, mut user, mut store, mut confirm) = (None::<PathBuf>, false, false, false);
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--project" => match it.next() {
                Some(value) => project = Some(PathBuf::from(value)),
                None => {
                    eprintln!("error: --project needs a value");
                    return (String::new(), 2);
                }
            },
            "--remove-user-scope" => user = true,
            "--remove-project-store" => store = true,
            "--i-confirm-removal" => confirm = true,
            other => {
                eprintln!("error: unrecognized argument {other:?}");
                return (String::new(), 2);
            }
        }
    }
    let pretty = |value: &Value| format!("{}\n", serde_json::to_string_pretty(value).unwrap_or_default());
    if !user && !store {
        return match plan(project.as_deref()) {
            Ok(report) => (pretty(&json!({"dry_run": true, "would_remove": report})), 0),
            Err(error) => {
                eprintln!("error: {error}");
                (String::new(), 2)
            }
        };
    }
    if !confirm {
        eprintln!("error: --remove-user-scope/--remove-project-store requires --i-confirm-removal.");
        return (String::new(), 2);
    }
    if store && project.is_none() {
        eprintln!("error: --remove-project-store requires --project PATH.");
        return (String::new(), 2);
    }
    let mut removed = serde_json::Map::new();
    let outcome = (|| -> Result<(), String> {
        if user {
            let (config, state) = owner_roots(&Environment::from_process()).ok_or("the owner storage roots are unavailable")?;
            let mut gone = Vec::new();
            for root in [config, state] {
                gone.extend(remove_verified(&root)?);
            }
            removed.insert("user_scope".into(), json!(gone));
        }
        if store {
            let directory = project_store(project.as_deref().unwrap_or(Path::new("")))?;
            removed.insert("project_store".into(), json!(remove_verified(&directory)?));
        }
        Ok(())
    })();
    match outcome {
        Ok(()) => (pretty(&json!({"dry_run": false, "removed": removed})), 0),
        Err(error) => {
            eprintln!("error: {error}");
            (String::new(), 1)
        }
    }
}

// ------------------------------------------------------ clone-dev-settings

const FILES: [&str; 5] = ["connections.json", "routing.json", "routing-profiles.json", "mcp-servers.json", "defaults.json"];
/// Executable arguments, environment and URLs can contain secrets without a secret-looking key.
const UNSAFE: [&str; 9] = ["env", "environment", "args", "launch_command", "endpoint", "url", "override_history", "pending", "history"];

fn no_links(path: &Path) -> Result<(), String> {
    for part in path.ancestors() {
        if part.is_symlink() {
            return Err("symbolic link refused".into());
        }
    }
    Ok(())
}

fn sanitize(value: &Py, skipped: &mut BTreeSet<String>, prefix: &str, sensitive: &Regex) -> Py {
    match value {
        Py::Dict(entries) => {
            let mut result = Py::dict();
            for (key, item) in entries {
                if sensitive.is_match(key) || UNSAFE.contains(&key.as_str()) {
                    skipped.insert(format!("{prefix}.{key}"));
                } else {
                    result.set(key, sanitize(item, skipped, &format!("{prefix}.{key}"), sensitive));
                }
            }
            result
        }
        Py::List(items) => Py::List(items.iter().map(|item| sanitize(item, skipped, prefix, sensitive)).collect()),
        other => other.clone(),
    }
}

fn resolve(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut tail = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            return tail.iter().rev().fold(real, |acc: PathBuf, part: &std::ffi::OsString| acc.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

struct CloneResult {
    copied: Vec<String>,
    skipped: BTreeSet<String>,
}

fn clone_settings(source: &Path, target: &Path, overwrite: bool, production: &[PathBuf]) -> Result<CloneResult, String> {
    let sensitive = Regex::new(r"(?i)secret|token|password|credential|authorization|api.?key|cookie|headers?").map_err(|e| e.to_string())?;
    if [source, target].iter().any(|p| p.components().any(|c| matches!(c.as_os_str().to_str(), Some(".paseo" | ".codex" | ".claude")))) {
        return Err("account and Paseo paths are not settings sources or targets".into());
    }
    no_links(source)?;
    no_links(target)?;
    let (source, target) = (resolve(source), resolve(target));
    let mut protected = vec![source.clone()];
    protected.extend(production.iter().map(|p| resolve(p)));
    if protected.iter().any(|p| target.starts_with(p) || p.starts_with(&target)) {
        return Err("target overlaps production paths".into());
    }
    if !source.is_dir() {
        return Err("production config directory missing".into());
    }
    if target.exists() && !overwrite {
        return Err("existing dev settings require --overwrite".into());
    }
    let mut skipped = BTreeSet::new();
    let mut documents: Vec<(String, String)> = Vec::new();
    // Read and sanitize everything before creating the target. Never walk secrets or state.
    for item in fs::read_dir(&source).map_err(|_| "filesystem error".to_string())?.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        if !FILES.contains(&name.as_str()) {
            skipped.insert(name);
        }
    }
    for name in FILES {
        let path = source.join(name);
        no_links(&path)?;
        if !path.exists() {
            continue;
        }
        let meta = fs::metadata(&path).map_err(|_| "filesystem error".to_string())?;
        if !meta.is_file() || meta.nlink() != 1 || meta.len() > 4 * 1024 * 1024 {
            return Err("unsafe settings file".into());
        }
        let text = fs::read_to_string(&path).map_err(|_| "invalid settings JSON".to_string())?;
        let raw = py::loads(&text).ok_or("invalid settings JSON")?;
        if !raw.is_dict() {
            return Err("settings must be an object".into());
        }
        let mut cleaned = sanitize(&raw, &mut skipped, name, &sensitive);
        if name == "connections.json" {
            let mut records = Py::dict();
            let connections = match raw.get_opt("connections") {
                None => Vec::new(),
                Some(Py::Dict(entries)) => entries.clone(),
                Some(_) => return Err("settings must be an object".into()),
            };
            for (identity, record) in &connections {
                let label = format!("{name}.connections.{identity}");
                if !record.is_dict() || !py::eq_str(record.get("auth_mode"), "native_account") {
                    skipped.insert(label);
                    continue;
                }
                let mut safe = sanitize(record, &mut skipped, &label, &sensitive);
                // References are metadata only. Native account files are never opened or copied.
                let reference = record.get("credential_ref");
                let reference_ok = match reference {
                    Py::Dict(entries) => {
                        py::eq_str(reference.get("kind"), "engine_native_dir")
                            && matches!(reference.get("owned"), Py::Bool(false))
                            && entries.iter().all(|(k, _)| matches!(k.as_str(), "kind" | "path" | "owned"))
                    }
                    _ => false,
                };
                if !reference_ok {
                    skipped.insert(label);
                    continue;
                }
                safe.set("credential_ref", reference.clone());
                for (key, value) in [
                    ("launch_command", Py::Null),
                    ("endpoint", Py::Null),
                    ("key_kind", Py::Null),
                    ("state", Py::str("authorization_required")),
                    ("approved_fingerprint", Py::str("dev-review-required")),
                    ("qualified", Py::Bool(false)),
                ] {
                    safe.set(key, value);
                }
                records.set(identity, safe);
            }
            cleaned.set("connections", records);
            cleaned.set("override_history", Py::dict());
            cleaned.set("auto_connect_disabled", Py::List(["codex-acp", "claude-agent-acp", "opencode"].map(Py::str).to_vec()));
        }
        if name == "mcp-servers.json" {
            if let Some(Py::Dict(servers)) = cleaned.get_opt("servers").cloned() {
                let mut updated = Py::dict();
                for (key, server) in servers {
                    if !server.is_dict() {
                        return Err("invalid MCP settings".into());
                    }
                    let mut server = server;
                    // Retain exact tool grants but require explicit review before launch.
                    server.set("args", Py::List(Vec::new()));
                    server.set("env", Py::dict());
                    server.set("enabled", Py::Bool(false));
                    updated.set(&key, server);
                }
                cleaned.set("servers", updated);
            } else if cleaned.get_opt("servers").is_some() {
                return Err("invalid MCP settings".into());
            }
        }
        documents.push((name.to_string(), format!("{}\n", render(&cleaned))));
    }
    if target.exists() {
        if !target.is_dir() {
            return Err("dev target is not a directory".into());
        }
        for (name, _) in &documents {
            let path = target.join(name);
            no_links(&path)?;
            if path.exists() && !fs::metadata(&path).is_ok_and(|m| m.is_file() && m.nlink() == 1) {
                return Err("unsafe dev target file".into());
            }
        }
    }
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&target).map_err(|_| "filesystem error".to_string())?;
    for (name, body) in &documents {
        let token = fs::read_to_string("/proc/sys/kernel/random/uuid").map_err(|_| "filesystem error".to_string())?;
        let temporary = target.join(format!(".clone-{}", token.trim().replace('-', "")));
        let written = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .and_then(|mut file| file.write_all(body.as_bytes()).and_then(|()| file.sync_all()))
            .and_then(|()| fs::rename(&temporary, target.join(name)));
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
            return Err("filesystem error".into());
        }
    }
    let mut copied: Vec<String> = documents.into_iter().map(|(name, _)| name).collect();
    copied.sort();
    Ok(CloneResult { copied, skipped })
}

pub fn clone_dev_settings(argv: &[String]) -> (String, i32) {
    let (mut source, mut target, mut overwrite) = (None::<PathBuf>, None::<PathBuf>, false);
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--overwrite" => overwrite = true,
            "--source" | "--target" => {
                let Some(value) = it.next() else {
                    eprintln!("clone refused: {arg} needs a value");
                    return (String::new(), 2);
                };
                if arg == "--source" {
                    source = Some(PathBuf::from(value));
                } else {
                    target = Some(PathBuf::from(value));
                }
            }
            other => {
                eprintln!("clone refused: unrecognized argument {other:?}");
                return (String::new(), 2);
            }
        }
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let xdg = |name: &str, fallback: &str| std::env::var_os(name).map(PathBuf::from).unwrap_or_else(|| home.join(fallback));
    let (config, state) = (xdg("XDG_CONFIG_HOME", ".config"), xdg("XDG_STATE_HOME", ".local/state"));
    if !config.is_absolute() || !state.is_absolute() {
        eprintln!("clone refused: absolute XDG paths required");
        return (String::new(), 2);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let namespace = fs::read_to_string(root.join("crates/workspace-protocol/channels.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["dev"]["config_namespace"].as_str().map(String::from));
    let Some(namespace) = namespace else {
        eprintln!("clone refused: the channel table is unreadable");
        return (String::new(), 1);
    };
    let source = source.unwrap_or_else(|| config.join("cortex"));
    let target = target.unwrap_or_else(|| config.join(namespace).join("cortex"));
    let production = [config.join("cortex"), state.join("cortex"), home.join(".codex"), home.join(".claude")];
    match clone_settings(&source, &target, overwrite, &production) {
        Ok(result) => {
            let mut report = Py::dict();
            report.set("copied", Py::List(result.copied.into_iter().map(Py::Str).collect()));
            report.set("skipped", Py::List(result.skipped.into_iter().map(Py::Str).collect()));
            report.set(
                "notice",
                Py::str("Native account references retained; connections need authorization; MCP servers disabled for review."),
            );
            (format!("{}\n", render(&report)), 0)
        }
        Err(error) => {
            eprintln!("clone refused: {error}");
            (String::new(), 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("owner-tools-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn dev_clone_keeps_only_safe_native_settings() {
        let base = scratch("clone");
        let source = base.join("config/cortex");
        fs::create_dir_all(source.join("secrets")).unwrap();
        fs::write(source.join("secrets/key"), "k").unwrap();
        fs::write(
            source.join("connections.json"),
            r#"{"schema_version": 1, "connections": {
              "cn_000000000001": {"id": "cn_000000000001", "name": "Native", "engine": "codex-acp", "auth_mode": "native_account",
                "launch_command": ["/x"], "endpoint": "u", "key_kind": null, "approved_fingerprint": "f", "state": "configured", "qualified": true,
                "credential_ref": {"kind": "engine_native_dir", "path": "/home/u/.codex", "owned": false}},
              "cn_000000000002": {"id": "cn_000000000002", "auth_mode": "api_key", "credential_ref": {"kind": "secret_file", "path": "/s"}}},
              "override_history": {"x": 1}}"#,
        )
        .unwrap();
        fs::write(source.join("mcp-servers.json"), r#"{"servers": {"a": {"command": "c", "args": ["--token=1"], "env": {"K": "v"}, "enabled": true}}}"#).unwrap();
        fs::write(source.join("routing.json"), r#"{"x-api-key": "1", "models": {"m": 1}}"#).unwrap();
        let target = base.join("config/dev/cortex");
        let result = clone_settings(&source, &target, false, &[]).unwrap();
        assert_eq!(result.copied, ["connections.json", "mcp-servers.json", "routing.json"]);
        assert!(result.skipped.contains("secrets") && result.skipped.contains("connections.json.connections.cn_000000000002"));
        let connections = py::loads(&fs::read_to_string(target.join("connections.json")).unwrap()).unwrap();
        let record = connections.get("connections").get("cn_000000000001");
        assert!(py::eq_str(record.get("state"), "authorization_required") && record.get("launch_command").is_null());
        assert!(connections.get("connections").get_opt("cn_000000000002").is_none());
        let servers = fs::read_to_string(target.join("mcp-servers.json")).unwrap();
        assert!(servers.contains("\"enabled\": false") && !servers.contains("--token") && !servers.contains("\"v\""));
        assert!(!fs::read_to_string(target.join("routing.json")).unwrap().contains("api-key"));
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o700);
        // Existing dev settings need --overwrite; an overlapping target is refused.
        assert!(clone_settings(&source, &target, false, &[]).is_err());
        assert!(clone_settings(&source, &source.join("inner"), true, &[]).is_err());
        assert!(clone_settings(&source, &base.join(".codex/x"), true, &[]).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn uninstall_is_a_dry_run_unless_confirmed_and_scoped() {
        let (text, code) = uninstall(&[]);
        assert_eq!(code, 0);
        assert!(text.contains("\"dry_run\": true"));
        assert_eq!(uninstall(&["--remove-user-scope".into()]).1, 2);
        assert_eq!(uninstall(&["--remove-project-store".into(), "--i-confirm-removal".into()]).1, 2);
    }

    #[test]
    fn project_store_removal_checks_the_directory_before_deleting() {
        let base = scratch("uninstall").canonicalize().unwrap();
        let store = base.join(cortex_acp::startup::project_store_name());
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("cortex.sqlite3"), "x").unwrap();
        fs::write(base.join("keep.txt"), "k").unwrap();
        let project = base.display().to_string();
        let args = |extra: &[&str]| -> Vec<String> { ["--project", &project].iter().chain(extra).map(|s| s.to_string()).collect() };
        let (planned, code) = uninstall(&args(&[]));
        assert!(code == 0 && planned.contains("project_cortex_store") && store.exists());
        let (removed, code) = uninstall(&args(&["--remove-project-store", "--i-confirm-removal"]));
        assert_eq!(code, 0, "{removed}");
        assert!(!store.exists() && base.join("keep.txt").exists());
        // A symlinked store is never followed or removed.
        std::os::unix::fs::symlink(&base, &store).unwrap();
        assert_eq!(uninstall(&args(&["--remove-project-store", "--i-confirm-removal"])).1, 1);
        assert!(base.join("keep.txt").exists());
        fs::remove_file(&store).unwrap();
        fs::remove_dir_all(base).unwrap();
    }
}
