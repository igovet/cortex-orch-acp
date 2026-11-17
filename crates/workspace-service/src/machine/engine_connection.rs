//! Native verified login/status/reference registration and private API-key creation.
use super::native::{refuse, Refused, Result};
use crate::config_read_model::{adapter_manifest, adapter_version};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[path = "engine_add_key.rs"]
mod add_key;
#[path = "engine_registration.rs"]
mod registration;
#[cfg(test)]
#[path = "engine_connection_tests.rs"]
mod tests;

/// Every request shape is native. Actions other than login_command, recheck and
/// add_key are refused before any effect, like the retained `_action`.
pub(super) fn supported(_params: &Value) -> bool {
    true
}
fn unavailable() -> Refused {
    refuse(
        "engine_unavailable",
        "Install the qualified bundled adapter first.",
    )
}
fn not_ready() -> Refused {
    refuse("engine_unavailable", "The engine install or native account is not ready. Check the adapter and login, then recheck.")
}
fn unchecked() -> Refused {
    refuse(
        "engine_unavailable",
        "The engine could not be checked. No login or key was copied.",
    )
}
// The retained helper exits without JSON for InstallUnresolvable. The socket
// translates this private marker to that same RPC error, not an action refusal.
fn unresolved() -> Refused {
    refuse("machine_tool_failed", "InstallUnresolvable")
}
fn regular(path: &Path, executable: bool) -> bool {
    fs::metadata(path).is_ok_and(|m| {
        m.is_file()
            && (!executable
                || unsafe {
                    libc::access(
                        std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
                            .unwrap()
                            .as_ptr(),
                        libc::X_OK,
                    )
                } == 0)
    })
}
fn qualified(engine: &str, version: &str) -> bool {
    adapter_version(engine, version)["qualified"] == true
}
fn version_key(s: &str) -> Vec<(usize, String)> {
    if !crate::config_read_model::matches(
        r"^[0-9]+\.[0-9]+\.[0-9]+(?:\+sdk\.[0-9]+\.[0-9]+\.[0-9]+)?$",
        s,
    ) {
        return vec![];
    }
    s.replace("+sdk.", ".")
        .split('.')
        .map(|part| {
            let part = part.trim_start_matches('0');
            (part.len(), part.to_owned())
        })
        .collect()
}
fn json_file(path: &Path) -> Option<Value> {
    // Never block the native inspection lane on a special file.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mut bytes = vec![];
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn directories(root: &Path, engine: &str) -> Result<Vec<PathBuf>> {
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(vec![]);
    };
    let mut paths = vec![];
    for (index, entry) in entries.enumerate() {
        if index >= 512 {
            return Err(unchecked());
        }
        let path = entry.map_err(|_| unchecked())?.path();
        if path.is_dir()
            && !path
                .file_name()
                .unwrap_or_default()
                .as_encoded_bytes()
                .starts_with(b".")
        {
            paths.push(path);
        }
    }
    paths.sort();
    // Keep Python's first-candidate tie behavior, including directory labels.
    paths.sort_by_key(|p| {
        let name = p.file_name().unwrap_or_default().to_str().unwrap_or("");
        std::cmp::Reverse((qualified(engine, name), version_key(name)))
    });
    Ok(paths)
}
struct Adapter {
    version: String,
    entry: PathBuf,
    node: PathBuf,
}
fn select(entries: Vec<PathBuf>, engine: &str, node: &Path) -> Option<Adapter> {
    let mut best = None;
    for root in entries {
        let package = root.join("node_modules/@agentclientprotocol").join(engine);
        let Some((version, target)) = json_file(&package.join("package.json"))
            .as_ref()
            .and_then(adapter_manifest)
        else {
            continue;
        };
        let (Ok(base), Ok(entry)) = (package.canonicalize(), package.join(target).canonicalize())
        else {
            continue;
        };
        if !entry.starts_with(base) || !regular(&entry, false) {
            continue;
        }
        let key = (
            qualified(engine, &version),
            version_key(&version),
            version_key(root.file_name().unwrap_or_default().to_str().unwrap_or("")),
        );
        if best.as_ref().is_none_or(|(old, _)| key > *old) {
            best = Some((
                key,
                Adapter {
                    version,
                    entry,
                    node: node.to_owned(),
                },
            ));
        }
    }
    best.map(|(_, a)| a)
}
fn dependency(package: &Path, name: &str, probe: &str) -> Option<PathBuf> {
    std::iter::once(package.join("node_modules"))
        .chain(
            package
                .ancestors()
                .skip(1)
                .filter(|p| p.file_name().is_some_and(|n| n == "node_modules"))
                .map(Path::to_path_buf),
        )
        .map(|root| root.join(name))
        .find(|p| regular(&p.join(probe), false))
}
fn package_above(entry: &Path) -> Result<PathBuf> {
    entry
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take(6)
        .map(|p| p.join("package.json"))
        .find(|p| regular(p, false))
        .ok_or_else(unresolved)
}
fn trusted(path: &Path, home: &Path) -> bool {
    path.is_absolute()
        && path.canonicalize().is_ok_and(|p| p == path)
        && ![".local/share/app.codeg", ".config/app.codeg"]
            .iter()
            .any(|root| path.starts_with(home.join(root)))
}
fn digest(path: &Path) -> Result<String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| not_ready())?;
    let before = file.metadata().map_err(|_| not_ready())?;
    let identity = |m: &fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    const MAX: u64 = 512 * 1024 * 1024;
    if !before.is_file() || before.len() > MAX {
        return Err(not_ready());
    }
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf).map_err(|_| not_ready())?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > MAX {
            return Err(not_ready());
        }
        hash.update(&buf[..n]);
    }
    if bytes != before.len()
        || identity(&before) != identity(&file.metadata().map_err(|_| not_ready())?)
    {
        return Err(not_ready());
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn verify(adapter: &Adapter, engine: &str, home: &Path) -> Result<PathBuf> {
    let manifest = package_above(&adapter.entry)?;
    let package = manifest.parent().unwrap();
    let document = json_file(&manifest)
        .filter(Value::is_object)
        .ok_or_else(unresolved)?;
    if !document["version"].as_str().is_some_and(|s| !s.is_empty()) {
        return Err(not_ready());
    }
    let manifest_hash = digest(&manifest)?;
    let entry_hash = digest(&adapter.entry)?;
    let (program, binary_hash) = if engine == "codex-acp" {
        let dep = dependency(package, "@openai/codex", "package.json").ok_or_else(unresolved)?;
        if !json_file(&dep.join("package.json"))
            .is_some_and(|v| v["version"].as_str().is_some_and(|s| !s.is_empty()))
        {
            return Err(unresolved());
        }
        let (arch, triple) = match std::env::consts::ARCH {
            "x86_64" => ("x64", "x86_64-unknown-linux-musl"),
            "aarch64" => ("arm64", "aarch64-unknown-linux-musl"),
            _ => return Err(unresolved()),
        };
        let probe = format!("vendor/{triple}/bin/codex");
        let dep = dependency(package, &format!("@openai/codex-linux-{arch}"), &probe)
            .ok_or_else(unresolved)?;
        let program = dep.join(probe);
        let hash = digest(&program)?;
        (Some(program), Some(hash))
    } else {
        (None, None)
    };
    if ![
        adapter.node.as_path(),
        adapter.entry.as_path(),
        manifest.as_path(),
    ]
    .iter()
    .all(|p| trusted(p, home))
        || !regular(&adapter.node, true)
        || program
            .as_ref()
            .is_some_and(|p| !trusted(p, home) || !regular(p, true))
    {
        return Err(not_ready());
    }
    if digest(&adapter.entry)? != entry_hash
        || digest(&manifest)? != manifest_hash
        || json_file(&manifest).is_none_or(|v| v["version"] != document["version"])
        || program
            .as_ref()
            .zip(binary_hash)
            .is_some_and(|(p, hash)| digest(p).ok().as_ref() != Some(&hash))
    {
        return Err(not_ready());
    }
    // The nearest manifest may be below the discovered package. Verify its own bin.
    if let Some(bins) = document["bin"].as_object() {
        let targets: Vec<_> = bins.values().filter_map(Value::as_str).collect();
        if !targets.is_empty()
            && !targets.iter().any(|s| {
                package
                    .join(s)
                    .canonicalize()
                    .is_ok_and(|p| p == adapter.entry)
            })
        {
            return Err(not_ready());
        }
    }
    if let Some(program) = program {
        return Ok(program);
    }
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => "",
    };
    Ok(dependency(
        package,
        &format!("@anthropic-ai/claude-agent-sdk-linux-{arch}"),
        "package.json",
    )
    .map(|p| p.join("claude"))
    .unwrap_or_else(|| PathBuf::from("/nonexistent")))
}
fn node(path: Option<std::ffi::OsString>) -> Option<PathBuf> {
    std::env::split_paths(&path.unwrap_or_default())
        .map(|p| p.join("node"))
        .find(|p| regular(p, true))
        .and_then(|p| p.canonicalize().ok())
}
pub(super) fn action(config: &crate::Config, params: &Value) -> Result<Value> {
    super::native::object_params(params)?;
    if !matches!(
        params["action"].as_str(),
        Some("login_command" | "recheck" | "add_key")
    ) {
        return Err(refuse(
            "unsupported_action",
            "Unsupported connection action.",
        ));
    }
    if params["action"] == "add_key" {
        add_key::validate(params)?;
        let command = command(config, params)?;
        add_key::validate_revision(params)?;
        let folder = super::project_config::owner_folder(config).map_err(|_| {
            refuse(
                "connection_failed",
                "The connection could not be saved. Refresh before retrying; no key is shown.",
            )
        })?;
        return add_key::add(&folder, params, &command["adapter"]);
    }
    command(config, params)
}
pub(super) async fn recheck(config: &crate::Config, params: &Value) -> Result<Value> {
    let command = command(config, params)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(unchecked)?;
    status(
        &command,
        params["engine"].as_str().unwrap_or(""),
        &home,
        std::time::Duration::from_secs(8),
        |child| crate::launch::runtime_environment(child, config).map_err(|_| unchecked()),
    )
    .await?;
    // All admission and account checks precede registry effects; no Python replay.
    let folder = super::project_config::owner_folder(config).map_err(|_| not_ready())?;
    registration::register(
        &folder,
        &home,
        params["engine"].as_str().unwrap(),
        &command["adapter"],
    )
}
async fn status(
    command: &Value,
    engine: &str,
    home: &Path,
    wait: std::time::Duration,
    environment: impl FnOnce(&mut tokio::process::Command) -> Result<()>,
) -> Result<()> {
    use std::process::Stdio;
    let mut process =
        tokio::process::Command::new(command["program"].as_str().ok_or_else(unchecked)?);
    environment(&mut process)?;
    let mut child = process
        .args(if engine == "codex-acp" {
            ["login", "status"]
        } else {
            ["auth", "status"]
        })
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| unchecked())?;
    let result = match tokio::time::timeout(wait, child.wait()).await {
        Ok(result) => result.map_err(|_| unchecked())?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(unchecked());
        }
    };
    if !result.success() {
        return Err(refuse("native_login_required", "The native CLI reports no active login. Finish signing in in the terminal, then recheck."));
    }
    Ok(())
}
/// The verified native CLI (Codex or Claude Code) of the qualified adapter that
/// sessions use, as for `login status`/`auth status`. `None` when no qualified,
/// verified adapter carries one.
pub(super) fn native_cli(config: &crate::Config, engine: &str) -> Option<PathBuf> {
    let found = command(config, &json!({"engine":engine,"action":"recheck"})).ok()?;
    found["program"].as_str().map(PathBuf::from)
}
fn command(config: &crate::Config, params: &Value) -> Result<Value> {
    let engine = params["engine"].as_str().unwrap_or("");
    if !["codex-acp", "claude-agent-acp"].contains(&engine) {
        return Err(refuse("unsupported_engine", "Choose Codex or Claude."));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .and_then(|p| p.canonicalize().ok())
        .ok_or_else(not_ready)?;
    // Match the retained child environment: release drops XDG_DATA_HOME and
    // CORTEX_NODE_BINARY; dev uses only its channel-local data root.
    let data = if workspace_protocol::channel::current().is_dev() {
        config.state_dir.join("data")
    } else {
        home.join(".local/share")
    };
    let bundle = config.launch.bundled_engines();
    lookup(
        params["action"] != "add_key",
        engine,
        &home,
        &data,
        bundle.as_deref(),
        std::env::var_os("PATH"),
    )
}
#[cfg(test)]
fn login(
    engine: &str,
    home: &Path,
    data: &Path,
    bundle: Option<&Path>,
    path: Option<std::ffi::OsString>,
) -> Result<Value> {
    lookup(true, engine, home, data, bundle, path)
}
fn lookup(
    native_cli: bool,
    engine: &str,
    home: &Path,
    data: &Path,
    bundle: Option<&Path>,
    path: Option<std::ffi::OsString>,
) -> Result<Value> {
    let bundled = bundle.and_then(|root| {
        let node = root.join("node/bin/node");
        if !regular(&node, true) {
            return None;
        }
        let adapter = select(vec![root.to_owned()], engine, &node.canonicalize().ok()?)?;
        (qualified(engine, &adapter.version) && verify(&adapter, engine, home).is_ok())
            .then_some(adapter)
    });
    let adapter = match bundled {
        Some(adapter) => adapter,
        None => {
            let node = node(path).ok_or_else(unavailable)?;
            select(
                directories(&data.join("cortex/engines").join(engine), engine)?,
                engine,
                &node,
            )
            .ok_or_else(unavailable)?
        }
    };
    if !qualified(engine, &adapter.version) {
        return Err(unavailable());
    }
    let program = verify(&adapter, engine, home)?;
    if !native_cli {
        return Ok(json!({"adapter":[adapter.node, adapter.entry]}));
    }
    if !regular(&program, true) {
        return Err(refuse(
            "native_cli_missing",
            "This adapter does not contain the native login executable.",
        ));
    }
    Ok(
        json!({"program":program.canonicalize().map_err(|_| unchecked())?,
        "args":if engine == "codex-acp" {vec!["login"]} else {vec!["auth", "login"]},
        "adapter":[adapter.node, adapter.entry]}),
    )
}
