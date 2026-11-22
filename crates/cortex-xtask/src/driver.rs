//! Host-side container build driver (`build-deb`, `new-build-dir`): snapshots the build inputs,
//! builds the pinned Ubuntu builder image and runs `packaging/container/build.sh` in it.

use crate::debian::build_stamp;
use crate::fsutil::{read_text, write_text};
use crate::json::{dumps, parse, Format, Res, Value};
use crate::package::payload_files;
use crate::proc::{output, run};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const IMAGE: &str = "cortex-workspace-builder:ubuntu26.04-rust1.99.1-node24.14.1";
const TREES: &[&str] = &[
    "apps/cortex-workspace-desktop",
    "packages/workspace-ui",
    "packages/workspace-domain",
    "packages/workspace-contract",
    "prototypes/cortex-workspace/desktop",
];
const SUFFIXES: &[&str] = &[
    "rs", "toml", "lock", "json", "ts", "tsx", "js", "mjs", "css", "html", "png", "ico", "sql",
    "tmpl", "c", "txt",
];
const SKIP: &[&str] = &["node_modules", "target", "dist", "__pycache__"];

pub struct Options {
    pub build_stamp: Option<String>,
    pub release: bool,
    pub local_release: bool,
    pub ci_dev: bool,
    pub engine: String,
    pub jobs: u32,
    pub test_workspace: bool,
    pub check_install: bool,
    pub check_installed: bool,
    pub prior_archive: Option<PathBuf>,
    pub resume: Option<String>,
    /// Builds and caches kept per channel after success (`--keep-builds`, 0 disables).
    pub keep_builds: usize,
}

impl Options {
    pub fn parse(args: &[String]) -> Res<Options> {
        let mut o = Options {
            build_stamp: None,
            release: false,
            local_release: false,
            ci_dev: false,
            engine: "docker".into(),
            jobs: std::thread::available_parallelism().map_or(2, |n| n.get().min(6) as u32),
            test_workspace: false,
            check_install: false,
            check_installed: false,
            prior_archive: None,
            resume: None,
            keep_builds: crate::builds::KEEP,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let (name, inline) = match arg.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (arg.as_str(), None),
            };
            let mut value = |what: &str| -> Res<String> {
                inline
                    .clone()
                    .or_else(|| iter.next().cloned())
                    .ok_or(format!("{what} requires a value"))
            };
            match name {
                "--build-stamp" => o.build_stamp = Some(build_stamp(&value(name)?)?.to_string()),
                "--release" => o.release = true,
                "--local-release" => o.local_release = true,
                "--ci-dev" => o.ci_dev = true,
                "--engine" => {
                    o.engine = value(name)?;
                    if o.engine != "docker" && o.engine != "podman" {
                        return Err("--engine must be docker or podman".into());
                    }
                }
                "--jobs" => {
                    o.jobs = value(name)?
                        .parse()
                        .map_err(|_| "jobs must be between 1 and 16")?;
                    if !(1..=16).contains(&o.jobs) {
                        return Err("jobs must be between 1 and 16".into());
                    }
                }
                "--keep-builds" => {
                    o.keep_builds = value(name)?
                        .parse()
                        .ok()
                        .filter(|n| *n <= 50)
                        .ok_or("keep-builds must be between 0 and 50")?;
                }
                "--test-workspace" => o.test_workspace = true,
                "--check-install" => o.check_install = true,
                "--check-installed" => o.check_installed = true,
                "--prior-archive" => o.prior_archive = Some(PathBuf::from(value(name)?)),
                "--resume" => {
                    let id = value(name)?;
                    let tail = id.strip_prefix("build-").unwrap_or("");
                    if tail.len() != 8
                        || !tail
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                    {
                        return Err("expected a local build ID".into());
                    }
                    o.resume = Some(id);
                }
                other => return Err(format!("unknown option {other}")),
            }
        }
        if [o.release, o.local_release, o.ci_dev]
            .iter()
            .filter(|b| **b)
            .count()
            > 1
        {
            return Err("--release, --local-release and --ci-dev are mutually exclusive".into());
        }
        if o.check_installed && (o.prior_archive.is_none() || o.engine != "docker") {
            return Err(
                "--check-installed requires Docker and --prior-archive from the same channel"
                    .into(),
            );
        }
        if o.prior_archive.is_some() && !o.check_installed {
            return Err("--prior-archive requires --check-installed".into());
        }
        Ok(o)
    }
}

// --- UTC stamps -----------------------------------------------------------------------------

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

fn stamp_to_micros(stamp: &str) -> i64 {
    let n = |r: std::ops::Range<usize>| stamp[r].parse::<i64>().unwrap();
    let days = days_from_civil(n(0..4), n(4..6), n(6..8));
    (((days * 24 + n(8..10)) * 60 + n(10..12)) * 60 + n(12..14)) * 1_000_000 + n(14..20)
}

fn micros_to_stamp(micros: i64) -> String {
    let (secs, frac) = (micros.div_euclid(1_000_000), micros.rem_euclid(1_000_000));
    let (days, rest) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02}{frac:06}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn now_stamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    micros_to_stamp(now.as_micros() as i64)
}

/// Reserve increasing inputs, including cache resumes; explicit inputs can replay.
fn select_stamp(output_root: &Path, output: &Path, requested: Option<&str>) -> Res<String> {
    let saved = output.join("build-stamp.txt");
    if fs::symlink_metadata(&saved)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("build stamp must not be a symlink".into());
    }
    if saved.exists() {
        let text = read_text(&saved)?;
        let prior = build_stamp(text.trim())?.to_string();
        if requested == Some(prior.as_str()) {
            return Ok(prior);
        }
    }
    let state = output_root.join("last-build-stamp.txt");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&state)
        .map_err(|e| format!("cannot open {}: {e}", state.display()))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err("cannot lock the build stamp state".into());
    }
    use std::io::{Read, Seek, Write};
    let mut previous = String::new();
    file.read_to_string(&mut previous)
        .map_err(|e| e.to_string())?;
    let previous = previous.trim().to_string();
    if !previous.is_empty() {
        build_stamp(&previous)?;
    }
    let mut stamp = requested.map(str::to_string).unwrap_or_else(now_stamp);
    if !previous.is_empty() && stamp <= previous {
        if requested.is_some() {
            return Err("new build stamp must exceed the previous build".into());
        }
        stamp = micros_to_stamp(stamp_to_micros(&previous) + 1);
    }
    file.set_len(0).map_err(|e| e.to_string())?;
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|e| e.to_string())?;
    file.write_all(format!("{stamp}\n").as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    write_text(&saved, &format!("{stamp}\n"))?;
    Ok(stamp)
}

// --- source snapshot -----------------------------------------------------------------------

fn linked(root: &Path, path: &Path) -> bool {
    let mut current = Some(path);
    while let Some(p) = current {
        if p == root || !p.starts_with(root) {
            break;
        }
        if fs::symlink_metadata(p)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return true;
        }
        current = p.parent();
    }
    false
}

fn quoted_array(section: &str, key: &str) -> Vec<String> {
    let Some(start) = section.find(&format!("\n{key} = [")) else {
        return Vec::new();
    };
    let body = &section[start..];
    let body = &body[body.find('[').unwrap() + 1..];
    let body = &body[..body.find(']').unwrap_or(body.len())];
    body.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// Resolve Cargo workspace members without invoking Cargo (literal paths and `dir/*`).
fn workspace_members(root: &Path) -> Res<Vec<String>> {
    let text = format!("\n{}", read_text(&root.join("Cargo.toml"))?);
    let start = text.find("\n[workspace]").ok_or("no [workspace] table")?;
    let rest = &text[start + 1..];
    let section = &rest[..rest[1..].find("\n[").map(|i| i + 1).unwrap_or(rest.len())];
    let section = format!("\n{}", section.trim_start_matches("[workspace]"));
    let excluded: Vec<String> = quoted_array(&section, "exclude");
    let mut members = std::collections::BTreeSet::new();
    for pattern in quoted_array(&section, "members") {
        let path = Path::new(&pattern);
        if path.is_absolute()
            || path.components().any(|c| {
                let s = c.as_os_str().to_string_lossy();
                s == ".." || SKIP.contains(&s.as_ref()) || s.starts_with('.')
            })
        {
            return Err(format!("unsafe workspace member: {pattern}"));
        }
        let matches: Vec<String> = if let Some(dir) = pattern.strip_suffix("/*") {
            let mut found: Vec<String> = fs::read_dir(root.join(dir))
                .map_err(|_| format!("missing workspace member: {pattern}"))?
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| format!("{dir}/{}", e.file_name().to_string_lossy()))
                .collect();
            found.sort();
            found
        } else if pattern.contains(['*', '?', '[']) {
            return Err(format!("unsupported workspace member glob: {pattern}"));
        } else if root.join(&pattern).exists() {
            vec![pattern.clone()]
        } else {
            return Err(format!("missing workspace member: {pattern}"));
        };
        for member in matches {
            if excluded.contains(&member) {
                continue;
            }
            let manifest = root.join(&member).join("Cargo.toml");
            if linked(root, &manifest) || !manifest.is_file() {
                return Err(format!("missing or linked workspace manifest: {member}"));
            }
            members.insert(member);
        }
    }
    Ok(members.into_iter().collect())
}

fn copy_source_tree(root: &Path, source: &Path, destination: &Path) -> Res<()> {
    let mut stack = vec![source.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .flatten()
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if !SKIP.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(path);
                }
            } else if meta.is_file()
                && !name.starts_with('.')
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| SUFFIXES.contains(&e))
                    .unwrap_or(false)
            {
                let target = destination.join(path.strip_prefix(root).unwrap());
                fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
                fs::copy(&path, &target).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// Copy only build sources; never mount the checkout, home, Git or local config.
pub fn snapshot(root: &Path, destination: &Path) -> Res<()> {
    let members = workspace_members(root)?;
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    let mut names: Vec<String> = [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "packaging/voice/package.json",
        "packaging/voice/package-lock.json",
        "packaging/voice/voice.cjs",
        "packaging/voice/model.json",
        "packaging/engines/package.json",
        "packaging/engines/package-lock.json",
        "packaging/engines/node-checksums.json",
        "packaging/debian/postinst",
        "packaging/debian/prerm",
        "package-identity.json",
        "packaging/systemd/user/cortex-workspace.service",
        "packaging/systemd/user/cortex-workspace-dev.service",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    names.extend(
        [32, 48, 64, 128, 256, 512]
            .iter()
            .map(|s| format!("packaging/icons/cortex-{s}.png")),
    );
    for name in names {
        let source = root.join(&name);
        if source.is_file() && !linked(root, &source) {
            let target = destination.join(&name);
            fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(&source, &target).map_err(|e| e.to_string())?;
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in TREES.iter().map(|s| s.to_string()).chain(members) {
        if !seen.insert(name.clone()) {
            continue;
        }
        let source = root.join(&name);
        if linked(root, &source) || !source.is_dir() {
            continue;
        }
        copy_source_tree(root, &source, destination)?;
    }
    // The runtime payload set the installer ships, including extensionless assets.
    if root.join("runtime-payload.json").is_file() {
        for (relative, path) in payload_files(root)? {
            if linked(root, &path) {
                return Err("linked runtime payload".into());
            }
            let target = destination.join(relative);
            fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(&path, &target).map_err(|e| e.to_string())?;
        }
    }
    readable(destination)
}

/// The build container reads the copy as another user: a source file the owner keeps
/// private (0600) is copied readable (0644, or 0755 when executable) instead of failing
/// the build. Only the copy changes, never the checkout.
fn readable(dir: &Path) -> Res<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            readable(&path)?;
        } else if meta.is_file() {
            let mode = if meta.permissions().mode() & 0o100 != 0 {
                0o755
            } else {
                0o644
            };
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// --- orchestration -------------------------------------------------------------------------

fn require_release(root: &Path, channel: &str) -> Res<()> {
    let branch = match channel {
        "release" => "main",
        _ => "dev",
    };
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    if env("GITHUB_ACTIONS") != "true"
        || env("GITHUB_REF") != format!("refs/heads/{branch}")
        || env("GITHUB_EVENT_NAME") != "workflow_dispatch"
    {
        return Err(format!(
            "{channel} requires a GitHub Actions workflow_dispatch on {branch}"
        ));
    }
    let current = output(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["branch", "--show-current"]),
    )?;
    if current.trim() != branch {
        return Err(format!("{channel} requires current branch {branch}"));
    }
    if !output(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["status", "--porcelain"]),
    )?
    .trim()
    .is_empty()
    {
        return Err(format!("{channel} requires a clean tree"));
    }
    Ok(())
}

fn toolchain_channel(root: &Path) -> Res<String> {
    let text = read_text(&root.join("rust-toolchain.toml"))?;
    let channel = text
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("channel")
                .and_then(|r| r.trim().strip_prefix('='))
                .map(|v| v.trim().trim_matches('"').to_string())
        })
        .ok_or("rust-toolchain.toml has no channel")?;
    let parts: Vec<&str> = channel.split('.').collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("an exact Rust toolchain is required".into());
    }
    Ok(channel)
}

fn container_env(output: &Path) -> Res<Vec<(String, String)>> {
    let home = output.join("client-home");
    let config = home.join("docker");
    fs::create_dir_all(&config).map_err(|e| e.to_string())?;
    // Do not inherit daemon endpoints, proxies, credential helpers or engine tokens.
    Ok(vec![
        ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
        ("HOME".into(), home.display().to_string()),
        ("DOCKER_CONFIG".into(), config.display().to_string()),
    ])
}

fn sha256_file(path: &Path) -> Res<String> {
    Ok(crate::contract::hex(&Sha256::digest(
        fs::read(path).map_err(|e| e.to_string())?,
    )))
}

fn prepare_cache(path: &Path) -> Res<()> {
    let parent = path.parent().ok_or("invalid build cache path")?;
    if fs::symlink_metadata(parent)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("build cache parent must not be a symlink".into());
    }
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    if fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("build cache must not be a symlink".into());
    }
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn new_build_dir(parent: &Path) -> Res<()> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    use std::io::Read;
    for _ in 0..100 {
        let mut bytes = [0u8; 8];
        fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|e| e.to_string())?;
        let name: String = bytes
            .iter()
            .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
            .collect();
        let path = parent.join(format!("build-{name}"));
        if fs::create_dir(&path).is_ok() {
            fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
                .map_err(|e| e.to_string())?;
            println!("build-{name}");
            return Ok(());
        }
    }
    Err("cannot create a build directory".into())
}

pub fn build_deb(root: &Path, o: &Options) -> Res<()> {
    let channel = if o.release || o.local_release {
        "release"
    } else {
        "dev"
    };
    if o.local_release && std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
        return Err("--local-release is for local owner testing, not GitHub Actions".into());
    }
    if o.release || o.ci_dev {
        require_release(root, channel)?;
    }
    let output_root = root.join(".workspace-build");
    if fs::symlink_metadata(&output_root)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("build output must not be a symlink".into());
    }
    fs::create_dir_all(&output_root).map_err(|e| e.to_string())?;
    let out = match &o.resume {
        Some(id) => output_root.join(id),
        None => {
            let name = { new_build_dir_quiet(&output_root)? };
            output_root.join(name)
        }
    };
    if fs::symlink_metadata(&out)
        .map(|m| m.file_type().is_symlink() || !m.is_dir())
        .unwrap_or(true)
    {
        return Err("build directory must be a local non-symlink directory".into());
    }
    // Held for the whole build: build hygiene never removes a locked build.
    let _in_progress = crate::builds::lock_build(&out)?;
    let stamp = select_stamp(&output_root, &out, o.build_stamp.as_deref())?;
    for name in ["manifest.json", "container.cid"] {
        let _ = fs::remove_file(out.join(name));
    }
    for name in ["artifacts", "input"] {
        let _ = fs::remove_dir_all(out.join(name));
    }
    snapshot(root, &out.join("input"))?;
    // Only the reviewed recipe and Node checksums form the image build context.
    let context = out.join("context");
    fs::create_dir_all(&context).map_err(|e| e.to_string())?;
    for name in ["Dockerfile", "build.sh", "node-checksums.json"] {
        let dir = if name == "node-checksums.json" {
            "packaging/engines"
        } else {
            "packaging/container"
        };
        let source = root.join(dir).join(name);
        if fs::symlink_metadata(&source)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err("container recipe must not be a symlink".into());
        }
        fs::copy(&source, context.join(name)).map_err(|e| format!("{}: {e}", source.display()))?;
    }
    let rust = toolchain_channel(root)?;
    let env = container_env(&out)?;
    let engine = o.engine.as_str();
    let with_env = |c: &mut Command| {
        c.env_clear().envs(env.iter().cloned());
    };
    let image_file = out.join("builder-image.id");
    let mut build = Command::new(engine);
    build
        .args([
            "build",
            "--build-arg",
            &format!("RUST_VERSION={rust}"),
            "--tag",
            IMAGE,
        ])
        .arg("--iidfile")
        .arg(&image_file)
        .arg(&context);
    with_env(&mut build);
    run(&mut build, Duration::from_secs(6 * 3600))?;
    let image = read_text(&image_file)?.trim().to_owned();
    let image_digest = Some(image.strip_prefix("sha256:").unwrap_or(&image))
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("invalid builder image digest")?;
    let cache = if o.release || o.ci_dev {
        out.join("cache")
    } else {
        output_root
            .join(".cache")
            .join(format!("{channel}-{image_digest}"))
    };
    prepare_cache(&cache)?;
    let mut container = Command::new(engine);
    container
        .args(["run", "--rm", "--cidfile"])
        .arg(out.join("container.cid"))
        .args([
            "--user",
            &format!("{}:{}", unsafe { libc::getuid() }, unsafe {
                libc::getgid()
            }),
        ])
        .args(["--cap-drop=ALL", "--security-opt=no-new-privileges"])
        .args([
            "--mount",
            &format!(
                "type=bind,source={},target=/source,readonly",
                out.join("input").display()
            ),
        ])
        .args([
            "--mount",
            &format!("type=bind,source={},target=/build", out.display()),
        ])
        .args([
            "--mount",
            &format!("type=bind,source={},target=/build/cache", cache.display()),
        ])
        .args(["--env", &format!("CORTEX_WORKSPACE_BUILD_STAMP={stamp}")])
        .args([
            "--env",
            "HOME=/build/home",
            "--env",
            &format!("CORTEX_WORKSPACE_CHANNEL={channel}"),
        ]);
    if channel == "release" {
        let authorization = if o.local_release {
            "local-owner-test"
        } else {
            "github-main-clean"
        };
        container.args([
            "--env",
            &format!("CORTEX_WORKSPACE_RELEASE_AUTHORIZED={authorization}"),
        ]);
    }
    container.arg(&image).arg(o.jobs.to_string());
    if o.test_workspace {
        container.arg("test");
    }
    with_env(&mut container);
    run(&mut container, Duration::from_secs(12 * 3600))?;
    // The installed-package acceptance runners are QA subcommands of this same tool, not build steps.
    let tool = std::env::current_exe().map_err(|e| format!("cannot locate cortex-xtask: {e}"))?;
    if o.check_install {
        let mut c = Command::new(&tool);
        c.arg("check-install")
            .args(["--engine", engine, "--artifacts"])
            .arg(out.join("artifacts"))
            .args(["--channel", channel]);
        with_env(&mut c);
        run(&mut c, Duration::from_secs(6 * 3600))?;
    }
    let mut accepted = Value::Null;
    if o.check_installed {
        let archives: Vec<PathBuf> = fs::read_dir(out.join("artifacts"))
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("deb"))
            .collect();
        let [archive] = archives.as_slice() else {
            return Err("expected exactly one .deb artifact".into());
        };
        let evidence = out.join(format!("installed-{}", now_stamp()));
        let prior = o
            .prior_archive
            .as_ref()
            .unwrap()
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let mut c = Command::new(&tool);
        c.arg("installed-gate")
            .arg("--root")
            .arg(root)
            .args(["--channel", channel, "--archive"])
            .arg(archive)
            .arg("--prior-archive")
            .arg(prior)
            .arg("--evidence")
            .arg(&evidence);
        with_env(&mut c);
        run(&mut c, Duration::from_secs(12 * 3600))?;
        accepted = parse(&read_text(&evidence.join("result.json"))?)?;
        if accepted.get("accepted").map(Value::truthy) != Some(true)
            || accepted.get("channel").and_then(Value::as_str) != Some(channel)
            || accepted.get("archive_sha256").and_then(Value::as_str)
                != Some(sha256_file(archive)?.as_str())
        {
            return Err("installed acceptance did not bind to this archive".into());
        }
    }
    let mut artifacts = Vec::new();
    let mut files: Vec<PathBuf> = fs::read_dir(out.join("artifacts"))
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .collect();
    files.sort();
    for path in files {
        let mut item = Value::obj();
        item.set(
            "file",
            Value::str(path.file_name().unwrap().to_string_lossy()),
        );
        item.set("sha256", Value::str(sha256_file(&path)?));
        item.set(
            "bytes",
            Value::int(fs::metadata(&path).map_err(|e| e.to_string())?.len() as i64),
        );
        artifacts.push(item);
    }
    let mut manifest = Value::obj();
    manifest.set("build_stamp", Value::str(stamp));
    manifest.set("channel", Value::str(channel));
    manifest.set("image", Value::str(image));
    manifest.set("rust", Value::str(rust));
    manifest.set("artifacts", Value::Arr(artifacts));
    manifest.set("local_release", Value::Bool(o.local_release));
    manifest.set(
        "owner_handoff_eligible",
        Value::Bool(accepted != Value::Null && !o.local_release),
    );
    manifest.set("installed_acceptance", accepted);
    let pretty = Format {
        sort_keys: false,
        ensure_ascii: true,
        indent: Some(2),
        compact: false,
    };
    write_text(
        &out.join("manifest.json"),
        &(dumps(&manifest, &pretty)? + "\n"),
    )?;
    let mut printed = Value::obj();
    printed.set(
        "output",
        Value::str(out.strip_prefix(root).unwrap().display().to_string()),
    );
    for (k, v) in manifest.as_obj().unwrap() {
        printed.set(k, v.clone());
    }
    println!("{}", dumps(&printed, &pretty)?);
    // Disk hygiene after success: never fails the finished build. Status goes to
    // stderr so stdout stays the build JSON.
    if o.keep_builds > 0 {
        if let Err(e) = crate::builds::clean_quiet(&output_root, o.keep_builds, &out) {
            eprintln!("build hygiene skipped: {e}");
        }
    }
    Ok(())
}

fn new_build_dir_quiet(parent: &Path) -> Res<String> {
    // Same allocation as `new-build-dir`, returning the name instead of printing it.
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    use std::io::Read;
    for _ in 0..100 {
        let mut bytes = [0u8; 8];
        fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|e| e.to_string())?;
        let name: String = bytes
            .iter()
            .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
            .collect();
        let path = parent.join(format!("build-{name}"));
        if fs::create_dir(&path).is_ok() {
            fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
                .map_err(|e| e.to_string())?;
            return Ok(format!("build-{name}"));
        }
    }
    Err("cannot create a build directory".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_arithmetic_round_trips() {
        let s = "20261116235959999999";
        assert_eq!(micros_to_stamp(stamp_to_micros(s)), s);
        assert_eq!(
            micros_to_stamp(stamp_to_micros(s) + 1),
            "20261117000000000000"
        );
        assert_eq!(
            micros_to_stamp(stamp_to_micros("20240228235959999999") + 1),
            "20240229000000000000"
        );
        assert!(build_stamp(&now_stamp()).is_ok());
    }

    #[test]
    fn options_validate() {
        let a = |v: &[&str]| Options::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(a(&["--check-installed"]).is_err());
        assert!(a(&["--release", "--ci-dev"]).is_err());
        assert!(a(&["--jobs", "17"]).is_err());
        assert!(a(&["--resume", "build-ab_12345"]).is_ok());
        assert!(a(&["--resume", "build-AB"]).is_err());
    }
}
