//! Startup approved-adapter following, ported from Python
//! `configuration.management.follow_approved_adapters` (the oracle).
//!
//! An existing connection advances to a newer, exactly registered adapter that
//! Cortex itself installed. It never downgrades, installs, launches an adapter,
//! changes account policy or invents a missing version. The exact pre-change
//! registry bytes are retained privately before one atomic replacement under the
//! shared `connections.lock`; a concurrent registry change refuses the commit.
//! A failure leaves the registry as it was and is never retried.
//!
//! Discovery prefers a verified channel-local bundled install (Python
//! `discover_bundled` with `install_resolution.resolve_verified_install`), else
//! Cortex's owned installs, exactly like Python `discover_engine`.
use crate::registry::Registry;
use cortex_core::{store::locks, CoreError, CoreResult};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use workspace_service::config_read_model::{adapter_manifest, adapter_version, route_fingerprint};

const MAX_REGISTRY: u64 = 4 * 1024 * 1024;
pub(crate) const PACKAGES: [(&str, &str); 2] = [
    ("codex-acp", "@agentclientprotocol/codex-acp"),
    ("claude-agent-acp", "@agentclientprotocol/claude-agent-acp"),
];

/// Trusted host inputs only (process environment), never ACP/model fields.
pub struct AdapterHost {
    /// `$HOME`, for the install trust check's untrusted-location roots.
    pub home: PathBuf,
    /// `$XDG_DATA_HOME/cortex/engines`, else `$HOME/.local/share/cortex/engines`.
    pub owned: PathBuf,
    /// `CORTEX_NODE_BINARY`, else `node` on `PATH`; resolved like Python.
    pub node: Option<PathBuf>,
    /// The channel-local bundled engines root of this runtime payload, if any.
    pub bundle: Option<PathBuf>,
}

impl AdapterHost {
    pub fn from_process(home: &Path, runtime_root: &Path) -> Self {
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let node = std::env::var_os("CORTEX_NODE_BINARY")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                let path = std::env::var_os("PATH").unwrap_or_else(|| "/bin:/usr/bin".into());
                std::env::split_paths(&path)
                    .map(|dir| dir.join("node"))
                    .find(|candidate| executable(candidate))
            })
            .map(|found| resolve(&found));
        Self {
            home: home.to_path_buf(),
            owned: data.join("cortex/engines"),
            node,
            bundle: bundled_root(runtime_root),
        }
    }
}

/// One committed change, reported on stderr only after the commit.
#[derive(Debug, PartialEq, Eq)]
pub struct Followed {
    pub engine: String,
    pub old_version: String,
    pub new_version: String,
}

fn refused(code: &'static str) -> CoreError {
    CoreError::Refused(code)
}
pub(crate) fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
/// Python `Path.resolve()` (non-strict): canonicalize the longest existing
/// prefix, then append the remaining components lexically.
pub(crate) fn resolve(path: &Path) -> PathBuf {
    let absolute = std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf());
    let parts: Vec<Component> = absolute.components().collect();
    for split in (1..=parts.len()).rev() {
        let prefix: PathBuf = parts[..split].iter().collect();
        if let Ok(mut real) = prefix.canonicalize() {
            for part in &parts[split..] {
                match part {
                    Component::ParentDir => {
                        real.pop();
                    }
                    Component::CurDir => {}
                    other => real.push(other),
                }
            }
            return real;
        }
    }
    absolute
}
/// Python `bundled_root`: only a packaged `cortex-workspace[-dev]/runtime` payload.
fn bundled_root(runtime_root: &Path) -> Option<PathBuf> {
    let runtime = resolve(runtime_root);
    let package = runtime.parent()?;
    let named = |p: &Path, names: &[&str]| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| names.contains(&n))
    };
    let root = package.join("engines");
    (named(&runtime, &["runtime"])
        && named(package, &["cortex-workspace", "cortex-workspace-dev"])
        && root.is_dir())
    .then_some(root)
}
/// Python `discovery.version_key`, compared without integer overflow.
pub(crate) fn version_key(text: &str) -> Vec<(usize, String)> {
    let pattern =
        regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:\+sdk\.[0-9]+\.[0-9]+\.[0-9]+)?$").unwrap();
    if !pattern.is_match(text) {
        return vec![];
    }
    text.replace("+sdk.", ".")
        .split('.')
        .map(|part| {
            let digits = part.trim_start_matches('0');
            (digits.len(), digits.to_owned())
        })
        .collect()
}
/// Exact instruction-delivery registration equals the launch version policy
/// for both npm adapters (Python `has_exact_registration`/`is_qualified`).
pub(crate) fn qualified(engine: &str, version: &str) -> bool {
    version != "*" && adapter_version(engine, version)["qualified"] == true
}
pub(crate) fn declared_bin(manifest: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(manifest).ok()?;
    adapter_manifest(&serde_json::from_str::<Value>(&text).ok()?)
}
pub(crate) fn subdirectories(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return vec![];
    };
    let mut result: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && !p
                    .file_name()
                    .is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
        })
        .collect();
    result.sort();
    result
}
pub(crate) fn file_name(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("")
}
type Key = (bool, Vec<(usize, String)>, Vec<(usize, String)>);
/// Python `discover_engine`: a verified bundled install first, else Cortex's
/// owned installs (the declared package bin only, a qualified version before a
/// newer unqualified one).
pub(crate) fn discover(
    engine: &str,
    package: &str,
    host: &AdapterHost,
) -> Option<(String, Vec<String>)> {
    if let Some(found) = discover_bundled(engine, package, host) {
        return Some(found);
    }
    let node = host.node.as_ref()?.to_str()?.to_owned();
    let mut entries = subdirectories(&host.owned.join(engine));
    let order = |p: &PathBuf| (qualified(engine, file_name(p)), version_key(file_name(p)));
    entries.sort_by_key(|p| std::cmp::Reverse(order(p)));
    discover_in(&entries, engine, package, &node)
}
fn accessible(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::X_OK) } == 0)
}
/// Python `discover_bundled`: only this payload's channel-local bundle, with a
/// bundled node, a qualified version and the full install trust check.
pub(crate) fn discover_bundled(
    engine: &str,
    package: &str,
    host: &AdapterHost,
) -> Option<(String, Vec<String>)> {
    let root = host.bundle.as_ref()?;
    let node = root.join("node/bin/node");
    if !node.is_file() || !accessible(&node) {
        return None;
    }
    let node = resolve(&node).to_str()?.to_owned();
    let found = discover_in(std::slice::from_ref(root), engine, package, &node)?;
    if !qualified(engine, &found.0) {
        return None;
    }
    verified_install(engine, &found.1, &host.home).map(|()| found)
}
fn json_object(path: &Path) -> Option<Value> {
    let value: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    value.is_object().then_some(value)
}
fn readable(path: &Path) -> Option<()> {
    std::io::copy(&mut fs::File::open(path).ok()?, &mut std::io::sink()).ok()?;
    Some(())
}
/// Python `install_resolution._resolve_node_package_dir`: nested first, then
/// each enclosing `node_modules`, identified by a file it must contain.
fn node_package(package: &Path, name: &str, probe: &str) -> Option<PathBuf> {
    let nested = package.join("node_modules").join(name);
    if nested.join(probe).is_file() {
        return Some(nested);
    }
    package
        .ancestors()
        .skip(1)
        .filter(|p| file_name(p) == "node_modules")
        .map(|p| p.join(name))
        .find(|candidate| candidate.join(probe).is_file())
}
/// Python `resolve_verified_install` for the npm engines: build the install from
/// the launch command and its own files, then the codex/claude `verify_install`
/// location, canonical-path, executable, version and declared-bin checks.
/// The freshly computed digests of the builder trivially equal the verifier's,
/// so only their readability is observable here.
fn verified_install(engine: &str, launch: &[String], home: &Path) -> Option<()> {
    let (node, entry) = (Path::new(&launch[0]), Path::new(&launch[1]));
    let mut directory = entry.parent()?.to_path_buf();
    let mut manifest = None;
    for _ in 0..6 {
        if directory.join("package.json").is_file() {
            manifest = Some(directory.join("package.json"));
            break;
        }
        match directory.parent() {
            Some(parent) if parent != directory => directory = parent.to_path_buf(),
            _ => break,
        }
    }
    let manifest = manifest?;
    let data = json_object(&manifest)?;
    let package = manifest.parent()?;
    let mut paths = vec![node.to_path_buf(), entry.to_path_buf(), manifest.clone()];
    if engine == "codex-acp" {
        let codex = node_package(package, "@openai/codex", "package.json")?;
        let version = json_object(&codex.join("package.json"))?;
        version["version"].as_str().filter(|v| !v.is_empty())?;
        let triple = match std::env::consts::ARCH {
            "x86_64" => "x86_64-unknown-linux-musl",
            "aarch64" => "aarch64-unknown-linux-musl",
            _ => return None,
        };
        let platform = if triple.starts_with("x86_64") {
            "@openai/codex-linux-x64"
        } else {
            "@openai/codex-linux-arm64"
        };
        let probe = format!("vendor/{triple}/bin/codex");
        paths.push(node_package(package, platform, &probe)?.join(probe));
    }
    for path in &paths[1..] {
        readable(path)?;
    }
    // The recorded version is `str(version)`; only a non-empty string matches.
    data["version"].as_str().filter(|v| !v.is_empty())?;
    if !home.is_absolute() {
        return None;
    }
    let home = resolve(home);
    let untrusted = [
        home.join(".local/share/app.codeg"),
        home.join(".config/app.codeg"),
    ];
    for path in &paths {
        if !path.is_absolute()
            || resolve(path) != *path
            || untrusted.iter().any(|root| path.starts_with(root))
        {
            return None;
        }
    }
    for path in [
        Some(node),
        (engine == "codex-acp").then(|| paths[3].as_path()),
    ]
    .into_iter()
    .flatten()
    {
        if !path.is_file() || !accessible(path) {
            return None;
        }
    }
    let reparsed = json_object(&manifest)?;
    if reparsed["version"] != data["version"] {
        return None;
    }
    let targets: Vec<PathBuf> = reparsed["bin"]
        .as_object()
        .into_iter()
        .flat_map(|bin| bin.values())
        .filter_map(Value::as_str)
        .map(|target| resolve(&package.join(target)))
        .collect();
    (targets.is_empty() || targets.iter().any(|t| t == entry)).then_some(())
}
/// Python `_discover_in`: among real declared bins, the greatest
/// (qualified, package version, directory label) key; the first of equals.
pub(crate) fn discover_in(
    entries: &[PathBuf],
    engine: &str,
    package: &str,
    node: &str,
) -> Option<(String, Vec<String>)> {
    let mut best: Option<(Key, (String, Vec<String>))> = None;
    for entry in entries {
        let manifest = entry
            .join("node_modules")
            .join(package)
            .join("package.json");
        if !manifest.is_file() {
            continue;
        }
        let Some((version, bin)) = declared_bin(&manifest) else {
            continue;
        };
        let directory = manifest.parent().unwrap();
        let bin_path = resolve(&directory.join(bin));
        if !bin_path.starts_with(resolve(directory)) || !bin_path.is_file() {
            continue;
        }
        let Some(bin_text) = bin_path.to_str() else {
            continue;
        };
        let key = (
            qualified(engine, &version),
            version_key(&version),
            version_key(file_name(entry)),
        );
        // Python max() keeps the first of equal keys.
        if best.as_ref().is_none_or(|(current, _)| key > *current) {
            best = Some((key, (version, vec![node.to_owned(), bin_text.to_owned()])));
        }
    }
    best.map(|(_, found)| found)
}
/// The recorded package identity, read from disk; an old adapter never runs.
fn recorded_version(entry: &str) -> Option<String> {
    let entry = Path::new(entry);
    let real = resolve(entry);
    entry.ancestors().skip(1).find_map(|parent| {
        let (version, bin) = declared_bin(&parent.join("package.json"))?;
        (resolve(&parent.join(bin)) == real).then_some(version)
    })
}
pub(crate) fn read_registry(path: &Path) -> CoreResult<Option<Vec<u8>>> {
    match path.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(_) => {}
    }
    let mut bytes = Vec::new();
    locks::open_private(path, false, true)?
        .take(MAX_REGISTRY + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REGISTRY {
        return Err(refused("native_registry_too_large"));
    }
    Ok(Some(bytes))
}
/// Python's `json.dumps(sort_keys=True, ensure_ascii=False, separators=(",",":"))`.
/// Floats are refused rather than risk a different spelling of owner data.
pub(crate) fn python_json(value: &Value, out: &mut String) -> CoreResult<()> {
    match value {
        Value::Object(map) => {
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key)?);
                out.push(':');
                python_json(item, out)?;
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                python_json(item, out)?;
            }
            out.push(']');
        }
        Value::Number(n) if n.is_f64() => return Err(refused("native_registry_float_unsupported")),
        other => out.push_str(&serde_json::to_string(other)?),
    }
    Ok(())
}
pub(crate) fn atomic_private(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if path.symlink_metadata().is_ok() {
        locks::regular(path)?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temporary = path.with_file_name(format!(
        ".{name}.{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> CoreResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        locks::sync_directory(path.parent().unwrap())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
/// Commit only if the registry still holds exactly the planned-from bytes.
/// Like Python `registry.transaction`, the owner root is repaired to 0700 and
/// the lock wait is unbounded (a blocking `flock`).
fn commit(folder: &Path, home: &Path, original: &[u8], updated: &[u8]) -> CoreResult<()> {
    crate::startup::owner_root(folder, home)?;
    let _lock =
        locks::AccessLock::acquire(&folder.join("connections.lock"), true, true, Duration::MAX)?;
    let path = folder.join("connections.json");
    if read_registry(&path)?.as_deref() != Some(original) {
        return Err(refused("connections_revision_conflict"));
    }
    atomic_private(&folder.join("connections.adapter-backup.json"), original)?;
    atomic_private(&path, updated)
}

/// Advance existing connections; `CORTEX_ENGINE_FOLLOW=off` disables it.
/// `folder` is the channel's `cortex` configuration directory.
pub fn follow_approved_adapters(folder: &Path, host: &AdapterHost) -> CoreResult<Vec<Followed>> {
    let Some(original) = read_registry(&folder.join("connections.json"))? else {
        return Ok(vec![]);
    };
    if original.is_empty() {
        return Ok(vec![]);
    }
    // Python `_parse_registry_bytes`; iteration keeps Python's dict order.
    let mut registry = Registry::parse(Some(&original))?;
    let mut events = vec![];
    for id in registry.ids() {
        let record = registry.get(&id).unwrap();
        let engine = record["engine"].as_str().unwrap().to_owned();
        let Some((_, package)) = PACKAGES.iter().find(|(e, _)| *e == engine) else {
            continue;
        };
        let Some(command) = record["launch_command"]
            .as_array()
            .filter(|a| !a.is_empty())
        else {
            continue;
        };
        let command: Vec<String> = command
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let Some((version, launch)) = discover(&engine, package, host) else {
            continue;
        };
        if !qualified(&engine, &version) {
            continue;
        }
        let bundled = host
            .bundle
            .as_ref()
            .is_some_and(|root| Path::new(&launch[1]).starts_with(resolve(root)));
        if !bundled && !Path::new(&launch[1]).starts_with(resolve(&host.owned)) {
            continue;
        }
        if command.len() != 2 {
            continue;
        }
        let Some(old) = recorded_version(&command[1]).filter(|v| !version_key(v).is_empty()) else {
            continue;
        };
        // Never downgrade. An equal version moves only onto a different bundled
        // launch; an equal owned version is never rewritten.
        let (new_key, old_key) = (version_key(&version), version_key(&old));
        if new_key < old_key || (new_key == old_key && (!bundled || launch == command)) {
            continue;
        }
        let fingerprint = route_fingerprint(
            &engine,
            record["provider"].as_str().unwrap(),
            record["endpoint"].as_str(),
            Some(&launch),
        );
        let record = registry.get_mut(&id).unwrap();
        record["launch_command"] = json!(launch);
        record["approved_fingerprint"] = json!(fingerprint);
        events.push(Followed {
            engine,
            old_version: old,
            new_version: version,
        });
    }
    if events.is_empty() {
        return Ok(events);
    }
    let updated = registry.to_json()?;
    commit(folder, &host.home, &original, updated.as_bytes())?;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn private(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn write_private(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn versions_order_like_python() {
        assert!(version_key("2.10.0") > version_key("2.9.9"));
        assert!(version_key("0.87.0+sdk.0.3.293") > version_key("0.87.0"));
        assert!(version_key("latest").is_empty());
        assert!(qualified("codex-acp", "2.3.0") && !qualified("codex-acp", "2.4.0"));
        assert!(qualified("codex-acp", "2.6.0") && !qualified("codex-acp", "2.5.0"));
        assert!(qualified("codex-acp", "2.6.0") && !qualified("codex-acp", "2.6.1"));
        assert!(qualified("claude-agent-acp", "0.97.0"));
        assert!(qualified("claude-agent-acp", "0.107.0"));
        assert!(!qualified("claude-agent-acp", "0.107.1") && !qualified("claude-agent-acp", "0.107.2"));
        assert!(!qualified("claude-agent-acp", "*"));
    }

    #[test]
    fn python_json_is_sorted_compact_and_refuses_floats() {
        let mut out = String::new();
        python_json(&json!({"b":[1,"é\n"],"a":null,"c":true}), &mut out).unwrap();
        assert_eq!(out, "{\"a\":null,\"b\":[1,\"é\\n\"],\"c\":true}");
        assert!(python_json(&json!({"x":1.5}), &mut String::new()).is_err());
    }

    #[test]
    fn concurrent_change_refuses_without_backup_or_rewrite() {
        let root = std::env::temp_dir().join(format!("cortex-follow-{}", std::process::id()));
        let folder = root.join("cortex");
        private(&folder);
        write_private(&folder.join("connections.json"), b"changed");
        let error = commit(&folder, &root, b"planned", b"replacement").unwrap_err();
        assert!(matches!(
            error,
            CoreError::Refused("connections_revision_conflict")
        ));
        assert_eq!(
            fs::read(folder.join("connections.json")).unwrap(),
            b"changed"
        );
        assert!(!folder.join("connections.adapter-backup.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsafe_backup_refuses_before_registry_replacement() {
        let root = std::env::temp_dir().join(format!("cortex-follow-link-{}", std::process::id()));
        let folder = root.join("cortex");
        private(&folder);
        write_private(&folder.join("connections.json"), b"planned");
        std::os::unix::fs::symlink(
            root.join("elsewhere"),
            folder.join("connections.adapter-backup.json"),
        )
        .unwrap();
        assert!(commit(&folder, &root, b"planned", b"replacement").is_err());
        assert_eq!(
            fs::read(folder.join("connections.json")).unwrap(),
            b"planned"
        );
        assert!(!root.join("elsewhere").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packaged_bundle_is_detected_and_checkout_is_not() {
        let root =
            std::env::temp_dir().join(format!("cortex-follow-bundle-{}", std::process::id()));
        let runtime = root.join("cortex-workspace/runtime");
        fs::create_dir_all(&runtime).unwrap();
        assert_eq!(bundled_root(&runtime), None);
        fs::create_dir_all(root.join("cortex-workspace/engines")).unwrap();
        assert_eq!(
            bundled_root(&runtime),
            Some(resolve(&root.join("cortex-workspace/engines")))
        );
        assert_eq!(bundled_root(&root), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundled_discovery_requires_the_verified_install_shape() {
        let root = resolve(
            &std::env::temp_dir().join(format!("cortex-bundle-verify-{}", std::process::id())),
        );
        let engines = root.join("cortex-workspace/engines");
        let node = engines.join("node/bin/node");
        fs::create_dir_all(node.parent().unwrap()).unwrap();
        fs::write(&node, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
        let package = engines.join("node_modules/@agentclientprotocol/codex-acp");
        fs::create_dir_all(package.join("dist")).unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"version":"2.3.0","bin":{"codex-acp":"dist/index.js"}}"#,
        )
        .unwrap();
        fs::write(package.join("dist/index.js"), "// fixture").unwrap();
        let codex = engines.join("node_modules/@openai/codex");
        fs::create_dir_all(&codex).unwrap();
        fs::write(codex.join("package.json"), r#"{"version":"0.164.0"}"#).unwrap();
        let host = AdapterHost {
            home: root.join("home"),
            owned: root.join("owned"),
            node: None,
            bundle: Some(engines.clone()),
        };
        // Without the vendored platform binary the bundle is not trusted.
        assert_eq!(discover_bundled("codex-acp", PACKAGES[0].1, &host), None);
        if std::env::consts::ARCH == "x86_64" {
            let binary = engines.join(
                "node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex",
            );
            fs::create_dir_all(binary.parent().unwrap()).unwrap();
            fs::write(&binary, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            let (version, launch) = discover_bundled("codex-acp", PACKAGES[0].1, &host).unwrap();
            assert_eq!(version, "2.3.0");
            assert_eq!(launch[0], node.to_str().unwrap());
            // A bundle inside an app-private untrusted home root is refused.
            let untrusted = AdapterHost {
                home: root.join("home"),
                bundle: None,
                ..host
            };
            let moved = root.join("home/.config/app.codeg");
            fs::create_dir_all(&moved).unwrap();
            fs::rename(
                root.join("cortex-workspace"),
                moved.join("cortex-workspace"),
            )
            .unwrap();
            let untrusted = AdapterHost {
                bundle: Some(moved.join("cortex-workspace/engines")),
                ..untrusted
            };
            assert_eq!(
                discover_bundled("codex-acp", PACKAGES[0].1, &untrusted),
                None
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
