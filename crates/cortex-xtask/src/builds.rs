//! Build-output hygiene for `.workspace-build` (`clean-builds`, and automatically
//! after every successful `build-deb`).
//!
//! Keeps the newest `keep` (default [`KEEP`]) `build-*` directories per channel
//! (`dev`, `release`, or `incomplete` for a build without a manifest) and the newest
//! `keep` `.cache/<channel>-<image>` compiler caches per channel. Never removed,
//! whatever their age: the build in progress (its directory is `flock`ed by
//! `build-deb` for the whole build), a directory any running container references
//! (its `container.cid`, or a bind-mount source at or below it), the build holding
//! an installed package's exact archive (`dpkg-query`), and a Dev build whose
//! `local-dev` copy may be in use by a qualification run. A removed build also takes
//! its `local-dev/` or `local-prod/` copy. Cargo's `target/` is never touched.
use crate::fsutil::read_text;
use crate::json::{parse, Res, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// Builds and caches kept per channel by default.
pub const KEEP: usize = 2;
/// Set by `scripts/build-dev-deb.sh`, which holds the `local-dev` lock itself.
pub const LOCAL_DEV_LOCK_HELD: &str = "CORTEX_BUILD_LOCAL_DEV_LOCK_HELD";
const ENGINE_DEADLINE: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Build,
    Cache,
}

/// One build directory or cache found below the output root.
#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub kind: Kind,
    /// `dev`, `release` or `incomplete` (builds without a manifest).
    pub group: String,
    /// Newer sorts greater: (build stamp, last use).
    pub recency: (String, SystemTime),
    /// Builder image digest (64 hex) of a cache, or of a build's manifest.
    pub image: Option<String>,
    /// Artifact file names of a finished build.
    pub artifacts: Vec<String>,
    pub container_id: Option<String>,
    /// `flock` held by a running `build-deb`.
    pub locked: bool,
}

/// What must survive regardless of age.
#[derive(Clone, Debug, Default)]
pub struct Protect {
    /// The build this process is running (or just finished).
    pub current: Option<PathBuf>,
    /// IDs of running containers.
    pub running_containers: BTreeSet<String>,
    /// Bind-mount sources of running containers.
    pub mounts: BTreeSet<PathBuf>,
    /// `<package>_<version>_` prefixes of installed archives.
    pub installed: Vec<String>,
    /// Whether `local-dev` copies may be removed now (the lock is held).
    pub local_dev_free: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub path: PathBuf,
    pub remove: bool,
    pub reason: &'static str,
}

fn mtime(path: &Path) -> SystemTime {
    fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

fn build_name(name: &str) -> bool {
    name.strip_prefix("build-").is_some_and(|t| {
        t.len() == 8
            && t.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    })
}

fn digest_of(image: &str) -> Option<String> {
    let d = image.strip_prefix("sha256:").unwrap_or(image);
    (d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit())).then(|| d.to_owned())
}

/// Whether another process holds the build's `flock` (a build in progress).
fn locked(path: &Path) -> bool {
    let Ok(dir) = fs::File::open(path) else {
        return false;
    };
    let busy = unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0;
    if !busy {
        unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_UN) };
    }
    busy
}

/// Hold an exclusive `flock` on a build directory for as long as the file lives.
pub fn lock_build(path: &Path) -> Res<fs::File> {
    let dir = fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    if unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!("build {} is already in progress", path.display()));
    }
    Ok(dir)
}

/// Every `build-*` directory and `.cache/<channel>-<image>` below `output_root`.
pub fn inventory(output_root: &Path) -> Res<Vec<Item>> {
    let mut items = Vec::new();
    let mut image_use: BTreeMap<String, SystemTime> = BTreeMap::new();
    let entries = match fs::read_dir(output_root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(items),
        Err(e) => return Err(format!("{}: {e}", output_root.display())),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if !build_name(&name) || !real_dir(&path) {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        let manifest = fs::symlink_metadata(&manifest_path)
            .is_ok_and(|m| m.is_file())
            .then(|| read_text(&manifest_path).ok().and_then(|t| parse(&t).ok()))
            .flatten();
        let stamp_file = read_text(&path.join("build-stamp.txt"))
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();
        let (group, stamp, image, artifacts) = match &manifest {
            Some(m) => {
                let channel = m.get("channel").and_then(Value::as_str).unwrap_or("");
                let group = if ["dev", "release"].contains(&channel) {
                    channel.to_owned()
                } else {
                    "incomplete".to_owned()
                };
                let stamp = m
                    .get("build_stamp")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or(stamp_file);
                let image = m.get("image").and_then(Value::as_str).and_then(digest_of);
                let artifacts = m
                    .get("artifacts")
                    .and_then(Value::as_arr)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(|a| a.get("file").and_then(Value::as_str).map(str::to_owned))
                    .collect();
                (group, stamp, image, artifacts)
            }
            None => ("incomplete".to_owned(), stamp_file, None, Vec::new()),
        };
        let used = mtime(&manifest_path).max(mtime(&path));
        if let Some(image) = &image {
            let slot = image_use.entry(image.clone()).or_insert(used);
            *slot = (*slot).max(used);
        }
        let container_id = read_text(&path.join("container.cid"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()));
        items.push(Item {
            locked: locked(&path),
            path,
            kind: Kind::Build,
            group,
            recency: (stamp, used),
            image,
            artifacts,
            container_id,
        });
    }
    let cache_root = output_root.join(".cache");
    if real_dir(&cache_root) {
        for entry in fs::read_dir(&cache_root)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let Some((channel, image)) = name.split_once('-') else {
                continue;
            };
            let Some(image) = digest_of(image) else {
                continue;
            };
            if !["dev", "release"].contains(&channel) || !real_dir(&path) {
                continue;
            }
            // A cache was last used when it changed or when a build using its image finished.
            let used = mtime(&path).max(
                image_use
                    .get(&image)
                    .copied()
                    .unwrap_or(SystemTime::UNIX_EPOCH),
            );
            items.push(Item {
                path,
                kind: Kind::Cache,
                group: channel.to_owned(),
                recency: (String::new(), used),
                image: Some(image),
                artifacts: Vec::new(),
                container_id: None,
                locked: false,
            });
        }
    }
    Ok(items)
}

fn referenced(item: &Item, protect: &Protect) -> bool {
    item.container_id
        .as_ref()
        .is_some_and(|id| protect.running_containers.contains(id))
        || protect
            .mounts
            .iter()
            .any(|m| m.starts_with(&item.path) || item.path.starts_with(m) && m != Path::new("/"))
}

/// Decide each item: newest `keep` per (kind, group) are kept; older ones are
/// removed unless protected. Pure: reads nothing but its inputs.
pub fn select(items: &[Item], keep: usize, protect: &Protect, output_root: &Path) -> Vec<Decision> {
    let current_image = protect.current.as_ref().and_then(|c| {
        items
            .iter()
            .find(|i| &i.path == c)
            .and_then(|i| i.image.clone())
    });
    let mut groups: BTreeMap<(bool, &str), Vec<&Item>> = BTreeMap::new();
    for item in items {
        groups
            .entry((item.kind == Kind::Cache, item.group.as_str()))
            .or_default()
            .push(item);
    }
    let mut out = Vec::new();
    for (_, mut members) in groups {
        members.sort_by(|a, b| b.recency.cmp(&a.recency));
        for (rank, item) in members.into_iter().enumerate() {
            let installed = item
                .artifacts
                .iter()
                .any(|a| protect.installed.iter().any(|p| a.starts_with(p.as_str())));
            let copy_in_use = item.kind == Kind::Build
                && !protect.local_dev_free
                && item
                    .path
                    .file_name()
                    .is_some_and(|n| output_root.join("local-dev").join(n).exists());
            let reason = if protect.current.as_ref() == Some(&item.path) {
                "current build"
            } else if item.locked {
                "build in progress"
            } else if referenced(item, protect) {
                "referenced by a running container"
            } else if installed {
                "holds the installed package"
            } else if item.kind == Kind::Cache
                && item.image.is_some()
                && item.image == current_image
            {
                "cache of the current build"
            } else if rank < keep {
                "newest"
            } else if copy_in_use {
                "local-dev copy may be in use (lock busy)"
            } else {
                out.push(Decision {
                    path: item.path.clone(),
                    remove: true,
                    reason: "older than the newest kept",
                });
                continue;
            };
            out.push(Decision {
                path: item.path.clone(),
                remove: false,
                reason,
            });
        }
    }
    out
}

/// Allocated bytes below `path`, on its own file system, without following links.
pub fn size(path: &Path) -> u64 {
    let Ok(root) = fs::symlink_metadata(path) else {
        return 0;
    };
    let device = root.dev();
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(m) = fs::symlink_metadata(&p) else {
            continue;
        };
        total += m.blocks() * 512;
        if m.is_dir() && m.dev() == device {
            if let Ok(entries) = fs::read_dir(&p) {
                stack.extend(entries.flatten().map(|e| e.path()));
            }
        }
    }
    total
}

pub fn human(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    if bytes as f64 >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// A command's stdout within the deadline; `Ok(None)` when the program is absent.
fn bounded(command: &mut Command) -> Res<Option<String>> {
    let label = format!("{:?}", command.get_program());
    let mut child = match command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot start {label}: {e}")),
    };
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if start.elapsed() > ENGINE_DEADLINE => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{label} timed out"));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let text = reader.join().unwrap_or_default();
    Ok(Some(if status.success() {
        text
    } else {
        String::new()
    }))
}

/// Running containers and their bind-mount sources, for each engine present. An
/// engine that is present but cannot be queried in time refuses the cleanup.
fn containers(protect: &mut Protect) -> Res<()> {
    for engine in ["docker", "podman"] {
        let Some(ids) = bounded(Command::new(engine).args(["ps", "-q", "--no-trunc"]))? else {
            continue;
        };
        let ids: Vec<&str> = ids.split_whitespace().collect();
        if ids.is_empty() {
            continue;
        }
        let inspected = bounded(
            Command::new(engine)
                .args([
                    "inspect",
                    "--format",
                    "{{.Id}}{{range .Mounts}}\t{{.Source}}{{end}}",
                ])
                .args(&ids),
        )?
        .unwrap_or_default();
        if inspected.trim().is_empty() {
            return Err(format!(
                "{engine} lists running containers but cannot inspect them; nothing removed"
            ));
        }
        for line in inspected.lines() {
            let mut fields = line.split('\t');
            if let Some(id) = fields.next() {
                protect.running_containers.insert(id.trim().to_owned());
            }
            protect
                .mounts
                .extend(fields.filter(|s| s.starts_with('/')).map(PathBuf::from));
        }
    }
    Ok(())
}

/// `<package>_<version>_` of each installed Workspace package (epoch dropped).
fn installed() -> Vec<String> {
    let text = bounded(Command::new("dpkg-query").args([
        "-W",
        "-f=${Package}\t${Version}\t${db:Status-Abbrev}\n",
        "cortex-workspace",
        "cortex-workspace-dev",
    ]))
    .ok()
    .flatten()
    .unwrap_or_default();
    installed_prefixes(&text)
}

pub fn installed_prefixes(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let (package, version, status) = (f.next()?, f.next()?, f.next().unwrap_or("ii"));
            if version.is_empty() || !status.starts_with("ii") {
                return None;
            }
            let version = version.split_once(':').map_or(version, |(_, v)| v);
            Some(format!("{package}_{version}_"))
        })
        .collect()
}

fn local_dev_free(output_root: &Path) -> bool {
    if std::env::var_os(LOCAL_DEV_LOCK_HELD).is_some() {
        return true;
    }
    let Ok(dir) = fs::File::open(output_root.join("local-dev")) else {
        return true;
    };
    // Held while this process cleans; released when `dir` closes at exit.
    let free = unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if free {
        std::mem::forget(dir);
    }
    free
}

/// Remove the decided items (and a removed build's local copies); print and return
/// the bytes freed. Removal errors are reported and the rest continues.
pub fn apply(
    output_root: &Path,
    decisions: &[Decision],
    dry_run: bool,
    log: fn(&str),
) -> (u64, Vec<String>) {
    let mut freed = 0;
    let mut errors = Vec::new();
    for d in decisions.iter().filter(|d| d.remove) {
        let mut paths = vec![d.path.clone()];
        if let Some(name) = d
            .path
            .file_name()
            .filter(|n| build_name(&n.to_string_lossy()))
        {
            for copy in ["local-dev", "local-prod"] {
                let p = output_root.join(copy).join(name);
                if real_dir(&p) {
                    paths.push(p);
                }
            }
        }
        for path in paths {
            let bytes = size(&path);
            let shown = path
                .strip_prefix(output_root)
                .unwrap_or(&path)
                .display()
                .to_string();
            if dry_run {
                log(&format!("would remove {shown} ({})", human(bytes)));
                freed += bytes;
                continue;
            }
            match fs::remove_dir_all(&path) {
                Ok(()) => {
                    log(&format!("removed {shown} ({})", human(bytes)));
                    freed += bytes;
                }
                Err(e) => errors.push(format!("{shown}: {e}")),
            }
        }
    }
    (freed, errors)
}

/// Inventory, protect, select and remove. `current` is the build in progress.
pub fn clean(
    output_root: &Path,
    keep: usize,
    current: Option<&Path>,
    dry_run: bool,
    log: fn(&str),
) -> Res<u64> {
    if fs::symlink_metadata(output_root).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("build output must not be a symlink".into());
    }
    let mut protect = Protect {
        current: current.map(Path::to_path_buf),
        installed: installed(),
        local_dev_free: local_dev_free(output_root),
        ..Protect::default()
    };
    containers(&mut protect)?;
    let items = inventory(output_root)?;
    let decisions = select(&items, keep, &protect, output_root);
    for d in decisions
        .iter()
        .filter(|d| !d.remove && d.reason != "newest")
    {
        log(&format!(
            "kept {} ({})",
            d.path
                .strip_prefix(output_root)
                .unwrap_or(&d.path)
                .display(),
            d.reason
        ));
    }
    let (freed, errors) = apply(output_root, &decisions, dry_run, log);
    log(&format!(
        "build hygiene: {} {} (kept the newest {keep} builds and caches per channel; target/ untouched)",
        if dry_run { "would free" } else { "freed" },
        human(freed)
    ));
    if !errors.is_empty() {
        return Err(format!("could not remove: {}", errors.join("; ")));
    }
    Ok(freed)
}

/// Print to stdout (`clean-builds`).
pub fn to_stdout(line: &str) {
    println!("{line}");
}
/// After `build-deb`: stdout is its JSON, so hygiene reports on stderr.
pub fn clean_quiet(output_root: &Path, keep: usize, current: &Path) -> Res<u64> {
    clean(output_root, keep, Some(current), false, |l| {
        eprintln!("{l}")
    })
}

pub struct Options {
    pub keep: usize,
    pub dry_run: bool,
}

impl Options {
    pub fn parse(args: &[String]) -> Res<Options> {
        let mut o = Options {
            keep: KEEP,
            dry_run: false,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--dry-run" => o.dry_run = true,
                "--keep" => {
                    o.keep = iter
                        .next()
                        .and_then(|v| v.parse().ok())
                        .filter(|n| (1..=50).contains(n))
                        .ok_or("--keep must be between 1 and 50")?;
                }
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::TempDir;
    use std::time::Duration;

    fn build(
        root: &Path,
        name: &str,
        manifest: Option<(&str, &str, &str, &str)>,
        age: u64,
    ) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("input")).unwrap();
        fs::write(dir.join("input/blob"), vec![0u8; 8192]).unwrap();
        if let Some((channel, stamp, image, artifact)) = manifest {
            fs::write(
                dir.join("manifest.json"),
                format!(r#"{{"build_stamp":"{stamp}","channel":"{channel}","image":"sha256:{image}","artifacts":[{{"file":"{artifact}"}}]}}"#),
            )
            .unwrap();
            fs::File::open(dir.join("manifest.json"))
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(age))
                .unwrap();
        }
        fs::File::open(&dir)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(age))
            .unwrap();
        dir
    }
    fn cache(root: &Path, channel: &str, image: &str, age: u64) -> PathBuf {
        let dir = root.join(".cache").join(format!("{channel}-{image}"));
        fs::create_dir_all(dir.join("cargo")).unwrap();
        fs::write(dir.join("cargo/blob"), vec![0u8; 4096]).unwrap();
        fs::File::open(&dir)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(age))
            .unwrap();
        dir
    }
    fn img(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }
    fn removed(decisions: &[Decision], root: &Path) -> BTreeSet<String> {
        decisions
            .iter()
            .filter(|d| d.remove)
            .map(|d| d.path.strip_prefix(root).unwrap().display().to_string())
            .collect()
    }

    /// The newest two per channel survive, and so does every protected directory:
    /// the current build, a locked (in-progress) one, one a running container
    /// mounts, the installed package's build and the current build's cache.
    #[test]
    fn selection_keeps_newest_in_progress_referenced_and_installed() {
        let t = TempDir::new("cortex-hygiene-").unwrap();
        let root = &t.0;
        let (a, b, c, d) = (img('a'), img('b'), img('c'), img('d'));
        build(
            root,
            "build-dev00001",
            Some((
                "dev",
                "20261101000000000001",
                &a,
                "cortex-workspace-dev_1.0.6+z1_amd64.deb",
            )),
            5000,
        );
        build(
            root,
            "build-dev00002",
            Some((
                "dev",
                "20261101000000000002",
                &a,
                "cortex-workspace-dev_1.0.6+z2_amd64.deb",
            )),
            4000,
        );
        build(
            root,
            "build-dev00003",
            Some((
                "dev",
                "20261101000000000003",
                &b,
                "cortex-workspace-dev_1.0.6+z3_amd64.deb",
            )),
            3000,
        );
        build(
            root,
            "build-dev00004",
            Some((
                "dev",
                "20261101000000000004",
                &b,
                "cortex-workspace-dev_1.0.6+z4_amd64.deb",
            )),
            2000,
        );
        let current = build(root, "build-dev00005", None, 0);
        build(
            root,
            "build-rel00001",
            Some((
                "release",
                "20261101000000000001",
                &c,
                "cortex-workspace_1.0.6+z1.local_amd64.deb",
            )),
            9000,
        );
        build(
            root,
            "build-rel00002",
            Some((
                "release",
                "20261101000000000002",
                &c,
                "cortex-workspace_1.0.6+z2.local_amd64.deb",
            )),
            8000,
        );
        build(
            root,
            "build-rel00003",
            Some((
                "release",
                "20261101000000000003",
                &d,
                "cortex-workspace_1.0.6+z3.local_amd64.deb",
            )),
            7000,
        );
        build(
            root,
            "build-rel00004",
            Some((
                "release",
                "20261101000000000004",
                &d,
                "cortex-workspace_1.0.6+z4.local_amd64.deb",
            )),
            6000,
        );
        build(root, "build-old00001", None, 90000);
        build(root, "build-old00002", None, 80000);
        let busy = build(root, "build-old00003", None, 70000);
        // Not build outputs: never considered.
        fs::create_dir_all(root.join("dev-evidence.x")).unwrap();
        fs::create_dir_all(root.join("build-TOOLONGNAME")).unwrap();
        for (i, ch) in "ef78".chars().enumerate() {
            cache(root, "dev", &img(ch), 10_000 - i as u64 * 1000);
        }
        cache(root, "dev", &b, 50_000);
        cache(root, "release", &c, 100);
        cache(root, "release", &d, 200);
        cache(root, "release", &img('9'), 300);
        // A concurrent build holds its lock.
        let held = lock_build(&busy).unwrap();
        assert!(lock_build(&busy).is_err());
        let protect = Protect {
            current: Some(current.clone()),
            mounts: [root.join(".cache").join(format!("release-{}", img('9')))].into(),
            installed: installed_prefixes(
                "cortex-workspace\t1.0.6+z1.local\tii \ncortex-workspace-dev\t1:1.0.6+z9\tii \n",
            ),
            local_dev_free: true,
            ..Protect::default()
        };
        let items = inventory(root).unwrap();
        assert_eq!(items.len(), 12 + 8);
        let decisions = select(&items, 2, &protect, root);
        let gone = removed(&decisions, root);
        let expected: BTreeSet<String> = [
            "build-dev00001".to_owned(),
            "build-dev00002".to_owned(),
            "build-rel00002".to_owned(),
            // Protected builds still count among the newest two of their group.
            "build-old00001".to_owned(),
            "build-old00002".to_owned(),
            ".cache/dev-".to_owned() + &img('e'),
            ".cache/dev-".to_owned() + &img('f'),
            ".cache/dev-".to_owned() + &img('7'),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(gone, expected);
        let reason = |name: &str| {
            decisions
                .iter()
                .find(|d| d.path == root.join(name))
                .unwrap()
                .reason
        };
        assert_eq!(reason("build-dev00005"), "current build");
        assert_eq!(reason("build-old00003"), "build in progress");
        assert_eq!(reason("build-rel00001"), "holds the installed package");
        assert_eq!(
            reason(&format!(".cache/release-{}", img('9'))),
            "referenced by a running container"
        );
        // The cache of an image a newer build used ranks by that use, not its own mtime.
        assert_eq!(reason(&format!(".cache/dev-{b}")), "newest");
        // A running container that references a build by its cid keeps it too.
        fs::write(root.join("build-dev00001/container.cid"), img('f')).unwrap();
        let items = inventory(root).unwrap();
        let protect = Protect {
            running_containers: [img('f')].into(),
            ..protect
        };
        let decisions = select(&items, 2, &protect, root);
        assert!(!removed(&decisions, root).contains("build-dev00001"));

        // Removal frees what it reports, takes the build's local copy and leaves the rest.
        fs::create_dir_all(root.join("local-dev/build-dev00002")).unwrap();
        fs::create_dir_all(root.join("local-prod/build-rel00004")).unwrap();
        let (dry, errors) = apply(root, &decisions, true, |_| ());
        assert!(errors.is_empty() && dry > 0);
        assert!(root.join("build-dev00002").exists());
        let (freed, errors) = apply(root, &decisions, false, |_| ());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(freed, dry);
        assert!(!root.join("build-dev00002").exists());
        assert!(!root.join("local-dev/build-dev00002").exists());
        assert!(root.join("local-prod/build-rel00004").exists());
        assert!(root.join("build-old00003").exists() && current.exists());
        assert!(root.join("dev-evidence.x").exists());
        drop(held);
    }

    /// A busy `local-dev` lock keeps Dev builds whose copy a qualification may use.
    #[test]
    fn a_busy_local_dev_lock_keeps_builds_with_copies() {
        let t = TempDir::new("cortex-hygiene-").unwrap();
        let root = &t.0;
        let a = img('a');
        for n in 1..=3 {
            build(
                root,
                &format!("build-dev0000{n}"),
                Some(("dev", &format!("2026110100000000000{n}"), &a, "x.deb")),
                1000 - n,
            );
        }
        fs::create_dir_all(root.join("local-dev/build-dev00001")).unwrap();
        let items = inventory(root).unwrap();
        let protect = Protect::default();
        let decisions = select(&items, 2, &protect, root);
        assert!(removed(&decisions, root).is_empty());
        let protect = Protect {
            local_dev_free: true,
            ..Protect::default()
        };
        assert_eq!(
            removed(&select(&items, 2, &protect, root), root),
            ["build-dev00001".to_owned()].into()
        );
    }

    #[test]
    fn options_and_installed_versions_parse() {
        let a = |v: &[&str]| Options::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&[]).unwrap().keep, KEEP);
        assert!(a(&["--keep", "0"]).is_err());
        assert!(a(&["--keep", "3", "--dry-run"]).unwrap().dry_run);
        assert!(a(&["--force"]).is_err());
        assert_eq!(
            installed_prefixes(
                "cortex-workspace\t2:1.0\tii \ncortex-workspace-dev\t1.0\trc \nx\t\tii \n"
            ),
            ["cortex-workspace_1.0_"]
        );
    }
}
