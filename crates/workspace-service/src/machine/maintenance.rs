//! Owner-only, durable maintenance jobs. No model calls or retained subprocess output.
use super::{
    context,
    native::*,
    settings::{self, Lock},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::VerifyingKey;
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

#[path = "../../../../packaging/update-signing/wire.rs"]
mod update_wire;

pub const MODE: &str = "--native-maintenance-worker";
const RELEASES: &str = "https://api.github.com/repos/igovet/cortex-orch-acp/releases/latest";
const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;
fn terminal(v: &Value) -> bool {
    matches!(
        v["phase"].as_str(),
        Some("done" | "failed" | "cancelled" | "ready" | "unknown")
    )
}
fn folder(state: &Path, id: &str) -> Result<PathBuf> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(refuse("invalid_job", "Unknown operation."));
    }
    Ok(state.join("maintenance").join(id))
}
fn status(path: &Path) -> Result<Value> {
    let v = settings::read_json(&path.join("status.json"), Value::Null)?;
    if !v.is_object() {
        return Err(refuse(
            "unknown_job",
            "This operation is no longer available.",
        ));
    }
    if !terminal(&v) && now_ms() as f64 / 1000.0 - v["updated"].as_f64().unwrap_or(0.0) > 660.0 {
        return save(
            path,
            json!({"phase":"unknown","message":"The operation stopped reporting. Check the folder or installed version before retrying."}),
        );
    }
    Ok(v)
}
fn save(path: &Path, patch: Value) -> Result<Value> {
    let mut v = settings::read_json(&path.join("status.json"), json!({}))?;
    if !v.is_object() {
        return Err(refuse("state_unreadable", "Invalid maintenance status."));
    }
    for (k, x) in patch.as_object().unwrap() {
        v[k] = x.clone();
    }
    v["updated"] = json!(now_ms() as f64 / 1000.0);
    settings::write_json(&path.join("status.json"), &v)?;
    Ok(v)
}
async fn output(program: &str, args: &[&str]) -> Result<Option<String>> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env_clear()
        // dpkg-deb extracts control metadata into temporary files even for --field.
        .env("TMPDIR", std::env::temp_dir())
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let mut out = child.stdout.take().unwrap().take(1048577);
    let mut bytes = Vec::new();
    let done = tokio::time::timeout(Duration::from_secs(10), async {
        let (read, status) = tokio::join!(out.read_to_end(&mut bytes), child.wait());
        read?;
        let status = status?;
        std::io::Result::Ok(status)
    })
    .await
    .map_err(|_| refuse("operation_timeout", "The inspection deadline expired."))??;
    if bytes.len() > 1048576 {
        return Err(refuse(
            "release_too_large",
            "Response exceeds its size limit.",
        ));
    }
    Ok(done
        .success()
        .then(|| String::from_utf8_lossy(&bytes).trim().to_owned()))
}
async fn version(package: &str) -> Result<Option<String>> {
    output("/usr/bin/dpkg-query", &["-W", "-f=${Version}", package]).await
}
async fn newer(candidate: &str, current: &str) -> Result<bool> {
    Ok(output(
        "/usr/bin/dpkg",
        &["--compare-versions", candidate, "gt", current],
    )
    .await?
    .is_some())
}
fn digest(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !f.metadata()?.is_file() {
        return Err(refuse("wrong_package", "Archive is not a regular file."));
    }
    let mut hash = Sha256::new();
    let mut buf = [0; 262144];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn release_url(url: &str, archive: bool) -> Result<()> {
    if url.starts_with("https://github.com/igovet/cortex-orch-acp/releases/download/")
        || !archive && url.starts_with("https://api.github.com/")
    {
        Ok(())
    } else {
        Err(refuse("release_url_refused", "Unsupported release URL."))
    }
}
fn curl(url: &str, limit: u64) -> Command {
    let mut c = Command::new("/usr/bin/curl");
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args([
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-redirs",
            "5",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            "600",
            "--max-filesize",
            &limit.to_string(),
            "--user-agent",
            "Cortex-Workspace/1.0.6",
            "--",
            url,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    c
}
async fn fetch(url: &str, limit: u64) -> Result<Vec<u8>> {
    release_url(url, false)?;
    let mut cmd = curl(url, limit);
    let mut child = cmd.stdout(Stdio::piped()).spawn()?;
    let mut out = child.stdout.take().unwrap().take(limit + 1);
    let mut bytes = Vec::new();
    let status = tokio::time::timeout(Duration::from_secs(25), async {
        let (a, b) = tokio::join!(out.read_to_end(&mut bytes), child.wait());
        a?;
        std::io::Result::Ok(b?)
    })
    .await
    .map_err(|_| refuse("update_check_failed", "Release check timed out."))??;
    if bytes.len() as u64 > limit {
        return Err(refuse(
            "release_too_large",
            "Release response exceeds its size limit.",
        ));
    }
    if !status.success() {
        return Err(refuse(
            "update_check_failed",
            "Could not fetch the release. Nothing was installed.",
        ));
    }
    Ok(bytes)
}
fn signature_error() -> Refused {
    refuse(
        "update_verification_failed",
        "Update verification failed. Nothing installed. A trusted signature for the exact release checksum is required.",
    )
}

fn trusted_update_key() -> Result<VerifyingKey> {
    update_wire::public_key(update_wire::TRUSTED_KEY).ok_or_else(signature_error)
}

fn verified_checksum<'a>(key: &VerifyingKey, data: &'a Value) -> Result<&'a str> {
    let version = string(data, "version").map_err(|_| signature_error())?;
    let arch = string(data, "arch").map_err(|_| signature_error())?;
    let name = format!("cortex-workspace_{version}_{arch}.deb");
    let checksum = data["checksum"]
        .as_str()
        .filter(|s| s.len() <= update_wire::CHECKSUM_LIMIT)
        .ok_or_else(signature_error)?;
    let signature = data["signature"]
        .as_str()
        .filter(|s| s.len() == update_wire::SIGNATURE_LEN.div_ceil(3) * 4)
        .and_then(|s| STANDARD.decode(s).ok())
        .ok_or_else(signature_error)?;
    update_wire::authenticate(key, checksum.as_bytes(), &signature, &name)
        .ok_or_else(signature_error)
}

fn candidate(
    release: &Value,
    current: &str,
    arch: &str,
    checksum: &[u8],
    signature: &[u8],
    key: &VerifyingKey,
) -> Result<Value> {
    let tag = release["tag_name"].as_str().unwrap_or("");
    let version = tag.strip_prefix('v').unwrap_or(tag);
    if !regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[+~.-][A-Za-z0-9.+~-]+)?$")
        .unwrap()
        .is_match(version)
    {
        return Err(refuse(
            "invalid_release",
            "The release has no valid package version.",
        ));
    }
    let name = format!("cortex-workspace_{version}_{arch}.deb");
    let url = asset(release, &name)?;
    release_url(&url, true)?;
    let sha =
        update_wire::authenticate(key, checksum, signature, &name).ok_or_else(signature_error)?;
    let checksum = std::str::from_utf8(checksum).map_err(|_| signature_error())?;
    Ok(
        json!({"phase":"available","current":current,"version":version,"arch":arch,"name":name,"url":url,"sha256":sha,
        "checksum":checksum,"signature":STANDARD.encode(signature),
        "notes":release["body"].as_str().unwrap_or("").chars().take(8000).collect::<String>(),
        "message":"Release checksum signature verified."}),
    )
}
fn asset(release: &Value, name: &str) -> Result<String> {
    let urls = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["name"] == name)
        .filter_map(|a| a["browser_download_url"].as_str())
        .collect::<Vec<_>>();
    if urls.len() != 1 {
        return Err(refuse(
            "release_incomplete",
            "The release is missing a required asset or the asset is ambiguous.",
        ));
    }
    Ok(urls[0].to_owned())
}
fn signed_assets(release: &Value, name: &str) -> Result<(String, String)> {
    let checksum = asset(release, &format!("{name}.sha256")).map_err(|_| signature_error())?;
    let signature = asset(release, &format!("{name}.sha256.sig")).map_err(|_| signature_error())?;
    Ok((checksum, signature))
}
async fn check(state: &Path) -> Result<Value> {
    let current = version("cortex-workspace").await?.ok_or_else(|| {
        refuse(
            "not_installed",
            "Install the release package before checking for updates.",
        )
    })?;
    let release: Value = serde_json::from_slice(&fetch(RELEASES, 1048576).await?)
        .map_err(|_| refuse("invalid_release", "Invalid release response."))?;
    let tag = release["tag_name"].as_str().unwrap_or("");
    let v = tag.strip_prefix('v').unwrap_or(tag);
    if !regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[+~.-][A-Za-z0-9.+~-]+)?$")
        .unwrap()
        .is_match(v)
    {
        return Err(refuse("invalid_release", "Invalid release version."));
    }
    if release["draft"] == true || release["prerelease"] == true || !newer(v, &current).await? {
        return Ok(
            json!({"phase":"current","current":current,"message":"No newer stable release. Downgrades are refused."}),
        );
    }
    let key = trusted_update_key()?;
    let arch = output("/usr/bin/dpkg", &["--print-architecture"])
        .await?
        .ok_or_else(|| refuse("invalid_release", "Unknown machine architecture."))?;
    let (checksum_url, signature_url) =
        signed_assets(&release, &format!("cortex-workspace_{v}_{arch}.deb"))?;
    let (checksum, signature) = tokio::try_join!(
        fetch(&checksum_url, update_wire::CHECKSUM_LIMIT as u64),
        fetch(&signature_url, update_wire::SIGNATURE_LEN as u64)
    )
    .map_err(|_| signature_error())?;
    let value = candidate(&release, &current, &arch, &checksum, &signature, &key)?;
    let _lock = Lock::new(state, "machine.lock")?;
    settings::write_json(&state.join("maintenance/candidate.json"), &value)?;
    Ok(value)
}
pub fn ensure_quiet(projects: &Value) -> Result<()> {
    for project in projects
        .as_array()
        .ok_or_else(|| refuse("activity_unknown", "Unknown project activity."))?
    {
        let root = Path::new(string(project, "root")?);
        if let Some(store) = context::open(root, cortex_core::OpenMode::LiveReadOnly)? {
            let quiet=store.read(|db|Ok(!db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE state NOT IN ('COMPLETED','FAILED','CANCELLED','PAUSED'))
                 OR EXISTS(SELECT 1 FROM workspace_starts s LEFT JOIN workspace_executions e USING(execution_id) WHERE s.state='unknown' OR e.runtime_task_id IS NULL)",
                [],|r|r.get::<_,bool>(0))?)).map_err(context::core)?;
            if !quiet {
                return Err(refuse(
                    "work_running",
                    "Stop or resolve unconfirmed Cortex work before installing an update.",
                ));
            }
        }
    }
    Ok(())
}
fn expand(s: &str) -> Result<PathBuf> {
    if let Some(tail) = s.strip_prefix("~/") {
        Ok(PathBuf::from(
            std::env::var_os("HOME")
                .ok_or_else(|| refuse("invalid_destination", "Home is unavailable."))?,
        )
        .join(tail))
    } else {
        Ok(PathBuf::from(s))
    }
}
fn protected(path: &Path, state: &Path) -> bool {
    path.components().any(|p| {
        matches!(
            p.as_os_str().to_str(),
            Some(".cortex" | ".cortex-dev" | ".ssh" | ".codex" | ".claude" | ".config")
        )
    }) || path.starts_with(state)
        || [
            "XDG_CONFIG_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
        ]
        .iter()
        .filter_map(std::env::var_os)
        .any(|p| path.starts_with(Path::new(&p)))
}
pub(crate) fn admission(state: &Path) -> Result<Lock> {
    let guard = Lock::new(state, "maintenance.lock")?;
    let base = state.join("maintenance");
    if base.try_exists()? {
        for (n, entry) in fs::read_dir(base)?.enumerate() {
            if n >= 10000 {
                return Err(refuse(
                    "maintenance_busy",
                    "Maintenance activity could not be bounded.",
                ));
            }
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                return Err(refuse(
                    "maintenance_busy",
                    "Maintenance activity is unconfirmed.",
                ));
            }
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let v = settings::read_json(&entry.path().join("status.json"), Value::Null)?;
            if matches!(v["kind"].as_str(), Some("install" | "rollback"))
                && !matches!(v["phase"].as_str(), Some("done" | "failed" | "cancelled"))
                && v["acknowledged"] != true
            {
                return Err(refuse("maintenance_busy","A package update is installing or unconfirmed. Finish or check it before starting work."));
            }
        }
    }
    Ok(guard)
}
async fn start(state: &Path, p: &Value, projects: &Value, spawn: bool) -> Result<Value> {
    let id = string(p, "id")?;
    let path = folder(state, id)?;
    let kind = string(p, "kind")?;
    if !["clone", "download", "install", "rollback"].contains(&kind) {
        return Err(refuse("invalid_job", "Unsupported operation."));
    }
    let _admission = Lock::new(state, "maintenance.lock")?;
    let _lock = Lock::new(state, "machine.lock")?;
    if path.join("status.json").try_exists()? {
        return status(&path);
    }
    let base = state.join("maintenance");
    crate::local::private_tree(&base)?;
    for entry in fs::read_dir(&base)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let v = settings::read_json(&entry.path().join("status.json"), Value::Null)?;
            let unconfirmed_install = matches!(v["kind"].as_str(), Some("install" | "rollback"))
                && !matches!(v["phase"].as_str(), Some("done" | "failed" | "cancelled"))
                && v["acknowledged"] != true;
            if !v.is_null() && (!terminal(&v) || unconfirmed_install) {
                return Err(refuse(
                    "maintenance_busy",
                    "Another operation is still running. Check its status first.",
                ));
            }
        }
    }
    let mut data = if kind == "clone" {
        let url = string(p, "url")?;
        let url = if url.starts_with('/') || url.starts_with("~/") {
            let source = expand(url)?.canonicalize()?;
            if protected(&source, state)
                || output(
                    "/usr/bin/git",
                    &[
                        "-c",
                        "core.hooksPath=/dev/null",
                        "-c",
                        "core.fsmonitor=false",
                        "-C",
                        source.to_str().ok_or_else(|| {
                            refuse("invalid_repository", "Invalid repository path.")
                        })?,
                        "rev-parse",
                        "--is-bare-repository",
                    ],
                )
                .await?
                .as_deref()
                    != Some("true")
            {
                return Err(refuse(
                    "invalid_repository",
                    "Choose a local bare Git repository outside private settings and runtime data.",
                ));
            }
            source.to_string_lossy().into_owned()
        } else {
            if !regex::Regex::new(r"^(?:https://github\.com/|git@github\.com:)[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?$").unwrap().is_match(url) {
                return Err(refuse("invalid_repository","Enter a credential-free GitHub URL or an absolute local bare repository path."));
            }
            url.to_owned()
        };
        let destination = expand(string(p, "destination")?)?;
        if !destination.is_absolute() || destination.as_os_str().as_encoded_bytes().contains(&0) {
            return Err(refuse(
                "invalid_destination",
                "Choose a full destination folder path.",
            ));
        }
        let parent = destination
            .parent()
            .ok_or_else(|| refuse("invalid_destination", "Choose a new folder."))?
            .canonicalize()?;
        if protected(&parent, state) {
            return Err(refuse(
                "invalid_destination",
                "Choose a destination outside private settings and runtime data.",
            ));
        }
        if destination.symlink_metadata().is_ok() {
            return Err(refuse(
                "destination_exists",
                "Existing folders are never overwritten.",
            ));
        }
        json!({"url":url,"destination":destination})
    } else {
        if workspace_protocol::channel::current().is_dev() {
            return Err(refuse(
                "dev_updates",
                "Dev updates come from local builds, not GitHub releases.",
            ));
        }
        if kind == "download" {
            let data = settings::read_json(&base.join("candidate.json"), Value::Null)?;
            let current = version("cortex-workspace").await?.unwrap_or_default();
            if data.is_null() || !newer(data["version"].as_str().unwrap_or(""), &current).await? {
                return Err(refuse("stale_release", "Check for a newer release again."));
            }
            verified_checksum(&trusted_update_key()?, &data)?;
            data
        } else {
            if p["confirmed"] != true {
                return Err(refuse(
                    "consent_required",
                    "Confirm the system package change first.",
                ));
            }
            ensure_quiet(projects)?;
            let source = folder(state, string(p, "source")?)?;
            let mut data = status(&source)?;
            if data["phase"] != "ready" {
                return Err(refuse(
                    "not_ready",
                    "Download and verify the release first.",
                ));
            }
            if kind == "install" {
                verified_checksum(&trusted_update_key()?, &data)?;
            }
            let archive = source.join(if kind == "rollback" {
                "previous.deb"
            } else {
                "update.deb"
            });
            if !archive.is_file() {
                return Err(refuse(
                    "rollback_unavailable",
                    "No previous package archive is retained. No rollback was attempted.",
                ));
            }
            if !Path::new("/usr/bin/pkexec").is_file() || !Path::new("/usr/bin/apt-get").is_file() {
                return Err(refuse(
                    "privilege_prompt_unavailable",
                    "Use the system package manager; pkexec is unavailable.",
                ));
            }
            data["archive"] = json!(archive);
            data
        }
    };
    data["kind"] = json!(kind);
    data["projects"] = projects.clone();
    settings::write_json(&path.join("request.json"), &data)?;
    let value = save(
        &path,
        json!({"id":id,"kind":kind,"phase":"starting","message":"Starting…"}),
    )?;
    if spawn {
        let exe = std::env::current_exe()?;
        let mut c = if ["install", "rollback"].contains(&kind) {
            let mut c = Command::new("/usr/bin/systemd-run");
            c.args([
                "--user",
                "--collect",
                "--quiet",
                "--service-type=exec",
                &format!("--unit=cortex-workspace-update-{id}"),
            ]);
            for key in [
                "HOME",
                "DISPLAY",
                "WAYLAND_DISPLAY",
                "DBUS_SESSION_BUS_ADDRESS",
                "XDG_RUNTIME_DIR",
            ] {
                if let Ok(v) = std::env::var(key) {
                    c.arg(format!("--setenv={key}={v}"));
                }
            }
            c.arg("--").arg(&exe);
            c
        } else {
            Command::new(&exe)
        };
        c.arg(MODE)
            .arg(state)
            .arg(id)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if !["install", "rollback"].contains(&kind) {
            c.process_group(0);
        }
        match c.spawn() {
            Ok(mut child) if ["install", "rollback"].contains(&kind) => {
                match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                    Ok(Ok(s)) if s.success() => {}
                    _ => {
                        return save(
                            &path,
                            json!({"phase":"unknown","message":"The installer launch could not be confirmed. Check the system package manager before retrying."}),
                        )
                    }
                }
            }
            Ok(mut child) => {
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
            }
            Err(_) => {
                return save(
                    &path,
                    json!({"phase":"unknown","message":"The operation launch could not be confirmed. Recheck before retrying."}),
                )
            }
        }
    }
    Ok(value)
}
async fn process(path: &Path, mut cmd: Command, installer: bool) -> Result<()> {
    if !installer && path.join("cancel").exists() {
        return Err(refuse(
            "cancelled",
            "Cancelled before dispatch; no command was run.",
        ));
    }
    let mut env = Vec::new();
    for key in [
        "HOME",
        "PATH",
        "LANG",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if let Some(v) = std::env::var_os(key) {
            env.push((key, v));
        }
    }
    cmd.env_clear()
        .envs(env)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env(
            "GIT_SSH_COMMAND",
            "/usr/bin/ssh -oBatchMode=yes -oStrictHostKeyChecking=yes",
        )
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = cmd.spawn()?;
    let until = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        if let Some(exit) = child.try_wait()? {
            return if exit.success() {
                Ok(())
            } else {
                Err(refuse("operation_failed","The operation failed. Check network, Git access or the system package manager. No automatic retry or rollback."))
            };
        }
        if !installer && (path.join("cancel").exists() || tokio::time::Instant::now() > until) {
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGTERM);
                }
            }
            if tokio::time::timeout(Duration::from_secs(3), child.wait())
                .await
                .is_err()
            {
                if let Some(pid) = child.id() {
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                }
                let _ = child.wait().await;
            }
            return Err(refuse(
                "cancelled",
                "Cancelled. Any partial clone folder is kept for inspection.",
            ));
        }
        // Failure to save progress must not detach or interrupt a system installer.
        let _ = save(path, json!({}));
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
async fn deb_fields(archive: &Path) -> Result<String> {
    output(
        "/usr/bin/dpkg-deb",
        &[
            "-f",
            archive
                .to_str()
                .ok_or_else(|| refuse("wrong_package", "Invalid archive path."))?,
            "Package",
            "Version",
            "Architecture",
        ],
    )
    .await?
    .ok_or_else(|| refuse("wrong_package", "Archive identity could not be read."))
}
async fn download(path: &Path, data: &Value) -> Result<Value> {
    let url = string(data, "url")?;
    release_url(url, true)?;
    download_with(
        path,
        data,
        curl(url, MAX_DOWNLOAD),
        true,
        &trusted_update_key()?,
    )
    .await
}
// Trusted internal fixture seam; neither command nor rollback lookup can be selected by JSON.
async fn download_with(
    path: &Path,
    data: &Value,
    mut cmd: Command,
    retain_rollback: bool,
    key: &VerifyingKey,
) -> Result<Value> {
    if path.join("cancel").exists() {
        return Err(refuse(
            "cancelled",
            "Download cancelled before dispatch. Nothing installed.",
        ));
    }
    let expected_digest = verified_checksum(key, data)?;
    let archive = path.join("update.deb");
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&archive)?;
    cmd.stdout(Stdio::from(file));
    let mut child = cmd.spawn()?;
    let until = tokio::time::Instant::now() + Duration::from_secs(605);
    loop {
        if let Some(exit) = child.try_wait()? {
            if !exit.success() {
                return Err(refuse(
                    "operation_failed",
                    "Download failed. Nothing installed.",
                ));
            }
            break;
        }
        if path.join("cancel").exists() || tokio::time::Instant::now() > until {
            let _ = child.kill().await;
            return Err(refuse(
                "cancelled",
                "Download cancelled. Nothing installed.",
            ));
        }
        let bytes = archive.metadata()?.len();
        if bytes > MAX_DOWNLOAD {
            let _ = child.kill().await;
            return Err(refuse(
                "release_too_large",
                "The archive exceeds its size limit.",
            ));
        }
        let _ = save(
            path,
            json!({"phase":"downloading","message":"Downloading release","bytes":bytes}),
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if digest(&archive)? != expected_digest {
        fs::remove_file(&archive)?;
        return Err(refuse(
            "bad_checksum",
            "Checksum mismatch. Nothing installed. Check again before retrying.",
        ));
    }
    let arch = output("/usr/bin/dpkg", &["--print-architecture"])
        .await?
        .unwrap_or_default();
    let version = string(data, "version")?;
    if data["arch"].as_str() != Some(arch.as_str())
        || deb_fields(&archive).await?
            != format!("Package: cortex-workspace\nVersion: {version}\nArchitecture: {arch}")
    {
        return Err(refuse(
            "wrong_package",
            "The archive identity does not match the selected release.",
        ));
    }
    let current = string(data, "current")?;
    let old = PathBuf::from("/var/cache/apt/archives")
        .join(format!("cortex-workspace_{current}_{arch}.deb"));
    let rollback = retain_rollback
        && old.is_file()
        && deb_fields(&old).await?
            == format!("Package: cortex-workspace\nVersion: {current}\nArchitecture: {arch}");
    let previous = if rollback {
        fs::copy(&old, path.join("previous.deb"))?;
        Some(digest(&path.join("previous.deb"))?)
    } else {
        None
    };
    save(
        path,
        json!({"phase":"ready","message":"Signature and checksum verified. Ready for your installation consent.","version":version,"current":current,
        "rollback":rollback,"sha256":expected_digest,"previousSha256":previous,
        "arch":arch,"checksum":data["checksum"],"signature":data["signature"]}),
    )
}
async fn install_with(
    state: &Path,
    path: &Path,
    data: &Value,
    rollback: bool,
    key: Option<&VerifyingKey>,
    run_installer: impl AsyncFnOnce(Command) -> Result<()>,
) -> Result<Value> {
    // A legacy or changed approval cannot reach even the privilege prompt.
    let expected_digest = if rollback {
        data["previousSha256"].as_str().ok_or_else(|| {
            refuse(
                "rollback_integrity",
                "The retained previous package changed. Rollback refused.",
            )
        })?
    } else {
        verified_checksum(key.ok_or_else(signature_error)?, data)?
    };
    let archive = Path::new(string(data, "archive")?);
    let installed = version("cortex-workspace").await?.unwrap_or_default();
    if rollback {
        if installed != data["version"] {
            return Err(refuse(
                "rollback_integrity",
                "The retained previous package changed. Rollback refused.",
            ));
        }
    } else if installed != data["current"]
        || !newer(string(data, "version")?, string(data, "current")?).await?
    {
        return Err(refuse(
            "stale_release",
            "The installed version or archive changed. Check again.",
        ));
    }
    // The privilege prompt and package manager can take minutes: anything waiting
    // for admission is told at once instead of stalling.
    let _admission = Lock::long(
        state,
        "maintenance.lock",
        "maintenance_in_progress",
        "A Cortex Workspace update is being installed. Try again when it finishes.",
    )?;
    ensure_quiet(&data["projects"])?;
    if digest(archive)? != expected_digest {
        return Err(if rollback {
            refuse(
                "rollback_integrity",
                "The retained previous package changed. Rollback refused.",
            )
        } else {
            refuse(
                "stale_release",
                "The installed version or archive changed. Check again.",
            )
        });
    }
    save(
        path,
        json!({"phase":"installing","message":"Approve the system privilege prompt. Installing…"}),
    )?;
    let mut cmd = Command::new("/usr/bin/pkexec");
    cmd.args(["/usr/bin/apt-get", "install", "--no-remove", "-y"]);
    if rollback {
        cmd.arg("--allow-downgrades");
    }
    cmd.arg(archive);
    run_installer(cmd).await?;
    let expected = if rollback {
        &data["current"]
    } else {
        &data["version"]
    };
    if version("cortex-workspace").await?.as_deref() != expected.as_str() {
        return Err(refuse("install_unconfirmed","The package manager ended but the installed version is unconfirmed. Check before retrying."));
    }
    save(
        path,
        json!({"phase":"done","message":"Installed. Restart Cortex Workspace.","version":expected}),
    )
}
pub async fn worker(state: &Path, id: &str) -> Result<()> {
    let path = folder(state, id)?;
    // A crashed or concurrently invoked worker never replays the external effect.
    let _claim = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.join("worker.claim"))
        .map_err(|_| {
            refuse(
                "unknown_outcome",
                "This job already launched. Inspect its retained status; it will not be replayed.",
            )
        })?;
    let data = settings::read_json(&path.join("request.json"), json!({}))?;
    let kind = string(&data, "kind")?;
    let work=async {
        match kind {
            "clone"=>{
                let destination=Path::new(string(&data,"destination")?);
                // Reservation is atomic; Git may not reuse a preexisting directory.
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new().mode(0o700).create(destination)?;
                save(&path,json!({"phase":"cloning","message":"Cloning repository…","destination":destination}))?;
                let mut cmd=Command::new("/usr/bin/git");
                cmd.args(["-c","core.hooksPath=/dev/null","-c","init.templateDir=","clone","--no-hardlinks","--progress","--",string(&data,"url")?,string(&data,"destination")?]);
                process(&path,cmd,false).await?;
                save(&path,json!({"phase":"done","message":"Clone complete.","destination":destination}))
            }
            "download"=>download(&path,&data).await,
            "install"|"rollback"=>{
                let rollback = kind == "rollback";
                let key = if rollback { None } else { Some(trusted_update_key()?) };
                install_with(state, &path, &data, rollback, key.as_ref(),
                    async |cmd| process(&path, cmd, true).await).await
            }
            _=>Err(refuse("invalid_job","Unsupported maintenance job.")),
        }
    }.await;
    if let Err(e) = work {
        save(
            &path,
            json!({"phase":if e.code=="cancelled" {"cancelled"} else if ["install","rollback"].contains(&kind) && ["machine_tool_failed","install_unconfirmed"].contains(&e.code) {"unknown"} else {"failed"},"code":e.code,"message":e.message}),
        )?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::os::unix::fs::{symlink, PermissionsExt};
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("cw-maintenance-{}", settings::unique()));
            fs::create_dir(&p).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn fixture_key() -> SigningKey {
        let mut seed = [0; 32];
        fs::File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut seed)
            .unwrap();
        SigningKey::from_bytes(&seed)
    }
    fn sign_fixture(data: &mut Value, signing: &SigningKey, digest: &str) {
        let version = data["version"].as_str().unwrap();
        let arch = data["arch"].as_str().unwrap();
        let checksum = format!("{digest}  cortex-workspace_{version}_{arch}.deb\n");
        data["signature"] = json!(STANDARD.encode(signing.sign(checksum.as_bytes()).to_bytes()));
        data["checksum"] = json!(checksum);
    }
    #[tokio::test]
    async fn real_archive_bytes_must_match_checksum_and_exact_debian_identity() {
        let t = Temp::new();
        let home = t.0.join("home");
        fs::create_dir(&home).unwrap();
        let root = t.0.join("project");
        fs::create_dir(&root).unwrap();
        let _store = cortex_core::Store::open(
            context::key(&root).unwrap(),
            cortex_core::OpenMode::Initialize,
        )
        .unwrap();
        let package = t.0.join("package");
        fs::create_dir_all(package.join("DEBIAN")).unwrap();
        let arch = output("/usr/bin/dpkg", &["--print-architecture"])
            .await
            .unwrap()
            .unwrap();
        fs::write(package.join("DEBIAN/control"),format!("Package: cortex-workspace\nVersion: 1.0.7\nArchitecture: {arch}\nMaintainer: Fixture <fixture@example.invalid>\nDescription: disposable test bytes\n")).unwrap();
        let source = t.0.join("source.deb");
        assert!(Command::new("/usr/bin/dpkg-deb")
            .arg("--build")
            .arg(&package)
            .arg(&source)
            .env_clear()
            .env("TMPDIR", &t.0)
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap()
            .success());
        let signing = fixture_key();
        let key = signing.verifying_key();
        let mut data = json!({"version":"1.0.7","current":"1.0.6","arch":arch});
        sign_fixture(&mut data, &signing, &digest(&source).unwrap());
        let transfer = || {
            let mut c = Command::new("/usr/bin/cat");
            c.arg(&source)
                .env_clear()
                .env("HOME", &home)
                .kill_on_drop(true);
            c
        };
        let path = t.0.join("good");
        crate::local::private_tree(&path).unwrap();
        let good = download_with(&path, &data, transfer(), false, &key)
            .await
            .unwrap();
        assert_eq!(good["phase"], "ready");
        assert_eq!(good["rollback"], false);
        let path = t.0.join("corrupt");
        crate::local::private_tree(&path).unwrap();
        let mut bad = data.clone();
        sign_fixture(&mut bad, &signing, &"0".repeat(64));
        assert_eq!(
            download_with(&path, &bad, transfer(), false, &key)
                .await
                .unwrap_err()
                .code,
            "bad_checksum"
        );
        assert!(!path.join("update.deb").exists());
        let path = t.0.join("wrong");
        crate::local::private_tree(&path).unwrap();
        let mut wrong = data.clone();
        wrong["version"] = json!("9.9.9");
        sign_fixture(&mut wrong, &signing, &digest(&source).unwrap());
        assert_eq!(
            download_with(&path, &wrong, transfer(), false, &key)
                .await
                .unwrap_err()
                .code,
            "wrong_package"
        );
        let path = t.0.join("cancelled");
        crate::local::private_tree(&path).unwrap();
        fs::write(path.join("cancel"), "").unwrap();
        assert_eq!(
            download_with(&path, &data, transfer(), false, &key)
                .await
                .unwrap_err()
                .code,
            "cancelled"
        );
        assert!(!path.join("update.deb").exists());
    }
    #[tokio::test]
    async fn clone_job_retains_identity_and_never_replays_with_temp_home_and_store() {
        let t = Temp::new();
        let state = t.0.join("state");
        let home = t.0.join("home");
        fs::create_dir(&home).unwrap();
        let bare = t.0.join("source.git");
        let result = Command::new("/usr/bin/git")
            .args(["init", "--bare"])
            .arg(&bare)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .await
            .unwrap();
        assert!(result.status.success());
        let root = t.0.join("project");
        fs::create_dir(&root).unwrap();
        let key = context::key(&root).unwrap();
        let _store = cortex_core::Store::open(key, cortex_core::OpenMode::Initialize).unwrap();
        let projects = json!([{"root":root,"id":"fixture"}]);
        let id = "1".repeat(32);
        let destination = t.0.join("clone");
        let p = json!({"id":id,"kind":"clone","url":bare,"destination":destination});
        let a = start(&state, &p, &projects, false).await.unwrap();
        assert_eq!(a["phase"], "starting");
        worker(&state, &id).await.unwrap();
        assert_eq!(
            status(&folder(&state, &id).unwrap()).unwrap()["phase"],
            "done"
        );
        assert!(destination.join(".git").is_dir());
        assert!(admission(&state).is_ok());
        let install = folder(&state, &"a".repeat(32)).unwrap();
        save(&install, json!({"kind":"install","phase":"unknown"})).unwrap();
        assert_eq!(admission(&state).err().unwrap().code, "maintenance_busy");
        let next =
            json!({"id":"e".repeat(32),"kind":"clone","url":bare,"destination":t.0.join("next")});
        assert_eq!(
            start(&state, &next, &projects, false)
                .await
                .unwrap_err()
                .code,
            "maintenance_busy"
        );
        save(&install, json!({"acknowledged":true})).unwrap();
        assert!(admission(&state).is_ok());
        assert_eq!(
            start(&state, &p, &projects, false).await.unwrap()["phase"],
            "done"
        );
        assert_eq!(
            worker(&state, &id).await.unwrap_err().code,
            "unknown_outcome"
        );
        let bad = json!({"id":"2".repeat(32),"kind":"clone","url":bare,"destination":destination});
        assert_eq!(
            start(&state, &bad, &projects, false)
                .await
                .unwrap_err()
                .code,
            "destination_exists"
        );
        let target = t.0.join("not-bare");
        fs::create_dir(&target).unwrap();
        let bad = json!({"id":"3".repeat(32),"kind":"clone","url":target,"destination":t.0.join("unused")});
        assert_eq!(
            start(&state, &bad, &projects, false)
                .await
                .unwrap_err()
                .code,
            "invalid_repository"
        );
        let cancelled = json!({"id":"4".repeat(32),"kind":"clone","url":bare,"destination":t.0.join("cancelled")});
        start(&state, &cancelled, &projects, false).await.unwrap();
        let path = folder(&state, &"4".repeat(32)).unwrap();
        fs::write(path.join("cancel"), "").unwrap();
        // Use a real cancellable process, without network, model or installer calls.
        let mut sleep = Command::new("/usr/bin/sleep");
        sleep.arg("5");
        assert_eq!(
            process(&path, sleep, false).await.unwrap_err().code,
            "cancelled"
        );
        save(&path, json!({"phase":"cancelled"})).unwrap();
        symlink(t.0.join("outside"), path.join("status-link")).unwrap();
        assert!(digest(&path.join("status-link")).is_err());
    }
    #[test]
    fn signed_release_requires_exact_assets_and_identity() {
        let name = "cortex-workspace_1.0.7_amd64.deb";
        let url = "https://github.com/igovet/cortex-orch-acp/releases/download/v1.0.7/";
        let mut release = json!({"tag_name":"v1.0.7","body":"notes","assets":[
            {"name":name,"browser_download_url":format!("{url}{name}")},
            {"name":format!("{name}.sha256"),"browser_download_url":format!("{url}{name}.sha256")},
        ]});
        assert_eq!(
            signed_assets(&release, name).unwrap_err().code,
            "update_verification_failed",
        );
        release["assets"].as_array_mut().unwrap().push(json!({
            "name":format!("{name}.sha256.sig"),"browser_download_url":format!("{url}{name}.sha256.sig")
        }));
        assert!(signed_assets(&release, name).is_ok());
        let signing = fixture_key();
        let key = signing.verifying_key();
        let checksum = format!("{}  {name}\n", "a".repeat(64));
        let signature = signing.sign(checksum.as_bytes()).to_bytes();
        let build = |release, arch, checksum: &str| {
            candidate(
                release,
                "1.0.6",
                arch,
                checksum.as_bytes(),
                &signature,
                &key,
            )
        };
        let good = build(&release, "amd64", &checksum).unwrap();
        assert_eq!(good["version"], "1.0.7");
        assert_eq!(verified_checksum(&key, &good).unwrap(), "a".repeat(64));
        assert_eq!(
            build(&release, "arm64", &checksum).unwrap_err().code,
            "release_incomplete"
        );
        // Even an edit with the same parsed digest and archive name breaks the signature.
        assert_eq!(
            build(&release, "amd64", checksum.trim_end())
                .unwrap_err()
                .code,
            "update_verification_failed",
        );
        let mut foreign = release.clone();
        foreign["assets"][0]["browser_download_url"] = json!("https://example.invalid/archive");
        assert_eq!(
            build(&foreign, "amd64", &checksum).unwrap_err().code,
            "release_url_refused"
        );
        let mut duplicated = release.clone();
        duplicated["assets"]
            .as_array_mut()
            .unwrap()
            .push(release["assets"][0].clone());
        assert_eq!(
            build(&duplicated, "amd64", &checksum).unwrap_err().code,
            "release_incomplete"
        );
    }

    #[tokio::test]
    async fn invalid_signed_metadata_never_invokes_installer() {
        use std::cell::Cell;
        let t = Temp::new();
        let signing = fixture_key();
        let trusted = signing.verifying_key();
        let mut good = json!({"version":"1.0.7","arch":"amd64"});
        sign_fixture(&mut good, &signing, &"a".repeat(64));
        let mut wrong_key = good.clone();
        sign_fixture(&mut wrong_key, &fixture_key(), &"a".repeat(64));
        let mut tampered = good.clone();
        tampered["checksum"] = json!(good["checksum"].as_str().unwrap().trim_end());
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove("signature");
        let mut malformed = good.clone();
        malformed["signature"] = json!("nonsense");
        let calls = Cell::new(0);
        for (candidate, key) in [
            (&wrong_key, Some(&trusted)),
            (&tampered, Some(&trusted)),
            (&missing, Some(&trusted)),
            (&malformed, Some(&trusted)),
            (&good, None),
        ] {
            let error = install_with(&t.0, &t.0, candidate, false, key, async |_| {
                calls.set(calls.get() + 1);
                Ok(())
            })
            .await
            .unwrap_err();
            assert_eq!(error.code, "update_verification_failed");
            assert!(error.message.contains("Nothing installed"));
            assert_eq!(calls.get(), 0);
        }
        assert!(!t.0.join("status.json").exists());
    }
}

pub async fn action(state: &Path, p: &Value, projects: &Value) -> Result<Value> {
    match p["action"].as_str().unwrap_or("") {
        // The owner's tool search path: shown, re-read from the shell (outside every
        // lock, bounded by its own timeout) and extended with owner-added folders.
        "tool_path.read" => Ok(crate::tool_path::snapshot()),
        "tool_path.reread" => {
            if !crate::tool_path::started() {
                return Err(refuse(
                    "tool_path_unavailable",
                    "The tool search path is read by the installed service only.",
                ));
            }
            tokio::task::spawn_blocking(crate::tool_path::reread)
                .await
                .map_err(|_| refuse("tool_path_unavailable", "The shell could not be read."))
        }
        "tool_path.set_extra" => settings::set_tool_path_extra(state, p, projects),
        "update.status" => {
            let dev = workspace_protocol::channel::current().is_dev();
            Ok(
                json!({"phase":if dev {"dev"} else {"idle"},"current":version(if dev {"cortex-workspace-dev"} else {"cortex-workspace"}).await?,
                "privilegePrompt":Path::new("/usr/bin/pkexec").is_file(),"message":if dev {"Dev updates come from local builds."} else {"Check GitHub for a newer stable release."}}),
            )
        }
        "update.check" => {
            if workspace_protocol::channel::current().is_dev() {
                return Err(refuse("dev_updates", "Dev updates come from local builds."));
            }
            check(state).await
        }
        "start" => start(state, p, projects, true).await,
        "read" => status(&folder(state, string(p, "id")?)?),
        "acknowledge" => {
            let path = folder(state, string(p, "id")?)?;
            let v = status(&path)?;
            if p["confirmed"] != true
                || v["phase"] != "unknown"
                || !matches!(v["kind"].as_str(), Some("install" | "rollback"))
            {
                return Err(refuse(
                    "confirmation_required",
                    "Check the system package manager and explicitly confirm before allowing work.",
                ));
            }
            save(
                &path,
                json!({"acknowledged":true,"message":"Installation remains unconfirmed. The owner checked the package manager and allowed work; this is not an installation success."}),
            )
        }
        "cancel" => {
            let path = folder(state, string(p, "id")?)?;
            let v = status(&path)?;
            if matches!(v["kind"].as_str(), Some("install" | "rollback")) {
                return Err(refuse(
                    "install_in_progress",
                    "Do not interrupt the system package manager. Wait for its result.",
                ));
            }
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path.join("cancel"))?;
            Ok(v)
        }
        _ => Err(refuse("invalid_action", "Unsupported maintenance action.")),
    }
}
