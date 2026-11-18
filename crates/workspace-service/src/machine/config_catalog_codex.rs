//! Catalog-only Codex package resolution, private launch artifacts and offline efforts.
use super::*;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

fn data() -> &'static Value {
    static DATA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(include_str!("config_catalog_codex.json")).unwrap())
}
#[cfg(test)]
thread_local! {
    /// A synthetic bundle cannot have the real upstream digest; tests name it here.
    static EXACT_HASH: std::cell::RefCell<Option<(String, String)>> = const { std::cell::RefCell::new(None) };
}
/// Python `_CODEX_2_3_SHA256`: the only bundle digest a version's patch accepts.
fn exact_hash(version: &str) -> Option<String> {
    #[cfg(test)]
    if let Some((v, hash)) = EXACT_HASH.with(|h| h.borrow().clone()) {
        return (v == version).then_some(hash);
    }
    data()["exact_hashes"][version].as_str().map(str::to_owned)
}
pub(super) struct Install {
    files: install::ClaudeInstall,
    manifest: PathBuf,
    entry: PathBuf,
    pub(super) binary: PathBuf,
    version: String,
    source: String,
    opener: Option<Vec<u8>>,
    launcher: String,
}
fn dependency(root: &Path, name: &str, probe: &str) -> Option<PathBuf> {
    let nested = root.join("node_modules").join(name);
    if nested.join(probe).is_file() {
        return Some(nested);
    }
    root.ancestors()
        .skip(1)
        .filter(|p| p.file_name().is_some_and(|n| n == "node_modules"))
        .map(|p| p.join(name))
        .find(|p| p.join(probe).is_file())
}
fn trusted_location(env: &RouteEnvironment, path: &Path) -> Option<()> {
    let home = env.home.canonicalize().ok()?;
    if !path.is_absolute()
        || path.canonicalize().ok()? != path
        || [".local/share/app.codeg", ".config/app.codeg"]
            .iter()
            .any(|p| path.starts_with(home.join(p)))
    {
        return None;
    }
    Some(())
}
fn trusted(env: &RouteEnvironment, path: &Path) -> Option<String> {
    trusted_location(env, path)?;
    install::digest(path)
}
/// The `@openai/codex` manifest and the vendored binary codex-acp itself
/// resolves from its package, when that binary is executable.
fn bundled(package: &Path) -> Option<(PathBuf, PathBuf)> {
    let codex = dependency(package, "@openai/codex", "package.json")?.join("package.json");
    if serde_json::from_slice::<Value>(&install::plain(&codex)?).ok()?["version"]
        .as_str()?
        .is_empty()
    {
        return None;
    }
    let (name, triple) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => ("@openai/codex-linux-x64", "x86_64-unknown-linux-musl"),
        _ => return None,
    };
    let probe = format!("vendor/{triple}/bin/codex");
    let binary = dependency(package, name, &probe)?.join(probe);
    if fs::metadata(&binary).ok()?.permissions().mode() & 0o111 == 0 {
        return None;
    }
    Some((codex, binary))
}
/// Retain the producer's previously qualified ASCII-path boundary. Standalone
/// launcher escaping tests do not qualify additional full-refresh install shapes.
fn qualified_path(binary: &Path) -> Option<()> {
    binary
        .to_str()?
        .bytes()
        .all(|b| (32..=126).contains(&b))
        .then_some(())
}
fn state(env: &RouteEnvironment, r: &Value) -> PathBuf {
    engine_home(env, r)
}
fn safe_leaf(path: &Path, mode: u32) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| {
        m.is_file()
            && m.nlink() == 1
            && m.uid() == unsafe { libc::getuid() }
            && m.permissions().mode() & 0o777 == mode
    })
}
fn safe_state(env: &RouteEnvironment, r: &Value) -> bool {
    let root = state(env, r);
    for p in [
        env.state.join("cortex/connections"),
        root.parent().unwrap().to_owned(),
        root.clone(),
        root.join("manual-review-adapter"),
        root.join("manual-review-adapter/dist"),
    ] {
        if fs::symlink_metadata(&p).is_ok_and(|m| {
            !m.is_dir()
                || m.uid() != unsafe { libc::getuid() }
                || m.permissions().mode() & 0o022 != 0
        }) || p.exists() && p.canonicalize().ok().as_deref() != Some(p.as_path())
        {
            return false;
        }
    }
    for (p, mode) in [
        ("codex-acp-runtime", 0o700),
        ("manual-review-adapter/package.json", 0o600),
        ("manual-review-adapter/dist/index.js", 0o600),
        ("manual-review-adapter/dist/xdg-open", 0o700),
    ] {
        let p = root.join(p);
        if fs::symlink_metadata(&p).is_ok() && !safe_leaf(&p, mode) {
            return false;
        }
    }
    for p in [
        root.join("manual-review-adapter"),
        root.join("manual-review-adapter/dist"),
    ] {
        if p.exists() && private_directory(&p, false).is_err() {
            return false;
        }
    }
    let link = root.join("manual-review-adapter/node_modules");
    !fs::symlink_metadata(&link).is_ok_and(|m| !m.file_type().is_symlink())
}
pub(super) fn resolve(env: &RouteEnvironment, r: &Value) -> Option<Install> {
    if r["engine"] != "codex-acp" || !safe_state(env, r) {
        return None;
    }
    // The shared regular-file/bin/canonical verifier is stricter than Python's
    // legacy bin shapes. Unqualified candidates stay on Python before effects.
    let mut shared = r.clone();
    shared["engine"] = json!("claude-agent-acp");
    let mut files = install::claude(env, &shared)?;
    let argv = r["launch_command"].as_array()?;
    let manifest = install::candidate(argv)?;
    let package = manifest.parent()?;
    let entry = PathBuf::from(argv[1].as_str()?);
    let version = serde_json::from_slice::<Value>(&install::plain(&manifest)?).ok()?["version"]
        .as_str()?
        .to_owned();
    let (codex, binary) = bundled(package)?;
    for path in [&codex, &binary] {
        files.0.push((path.clone(), trusted(env, path)?));
    }
    qualified_path(&binary)?;
    let source = String::from_utf8(install::plain(&entry)?).ok()?;
    // A version with an exact upstream digest (2.3.0) is patched only from that
    // bundle: `patched` re-checks the digest and every call site, as Python does.
    let opener_path = entry.parent()?.join("xdg-open");
    let opener = if opener_path.exists() {
        let bytes = install::plain(&opener_path)?;
        files
            .0
            .push((opener_path.clone(), trusted(env, &opener_path)?));
        Some(bytes)
    } else {
        None
    };
    Some(Install {
        files,
        manifest,
        entry,
        binary: binary.clone(),
        version,
        source,
        opener,
        launcher: crate::native_codex_runtime::artifact(&std::env::current_exe().ok()?, &binary)
            .ok()?,
    })
}
/// The native Codex launcher a task session (coordinator, worker, review,
/// verification or planning) pins as `CODEX_PATH`, as Python's
/// `EngineProcess` pins its runtime wrapper for every verified codex-acp
/// launch. Its App Server then gets the launcher's process-level guards and
/// non-persistent trust for its own working folder
/// (`native_codex_runtime::trust_override`), so codex-acp's ephemeral title
/// thread never persists `[projects."<cwd>"]` into the owner's
/// `config.toml`. The binary is the one codex-acp itself resolves, located as
/// `resolve` does but without re-hashing the install on every launch. The
/// artifact is named by its content, so a concurrent catalogue refresh,
/// another entry binary or a legacy Python wrapper never replaces the bytes a
/// session is about to execute.
pub(super) fn session_launcher(
    env: &RouteEnvironment,
    r: &Value,
) -> std::result::Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    let fail = || "codex_launcher_unavailable".to_owned();
    if r["engine"] != "codex-acp" {
        return Err(fail());
    }
    repair_state_modes(env, r);
    if !safe_state(env, r) {
        return Err(fail());
    }
    let argv = r["launch_command"].as_array().ok_or_else(fail)?;
    let manifest = install::candidate(argv).ok_or_else(fail)?;
    let (_, binary) = bundled(manifest.parent().ok_or_else(fail)?).ok_or_else(fail)?;
    trusted_location(env, &binary).ok_or_else(fail)?;
    qualified_path(&binary).ok_or_else(fail)?;
    let launcher = crate::native_codex_runtime::artifact(
        &std::env::current_exe().map_err(|_| fail())?,
        &binary,
    )
    .map_err(|_| fail())?;
    let root = state_root(env, r)?;
    let digest = format!("{:x}", Sha256::digest(launcher.as_bytes()));
    let path = root.join(format!("codex-acp-runtime-{}", &digest[..16]));
    write_file(&path, launcher.as_bytes(), 0o700)?;
    Ok(path)
}
/// The private connection state directory, created like Python's
/// `connection_state_dir` when absent.
fn state_root(env: &RouteEnvironment, r: &Value) -> std::result::Result<PathBuf, String> {
    let root = state(env, r);
    for p in [
        env.state.join("cortex/connections"),
        root.parent().unwrap().to_owned(),
        root.clone(),
    ] {
        if !p.exists() {
            // Explicit private mode: the process umask (002 on common desktops)
            // must never leave a group-writable directory that `safe_state`
            // then refuses on the very next launch.
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&p)
                .map_err(|_| "engine_unreachable:PortableError".to_owned())?;
        }
    }
    Ok(root)
}
/// Directories this module created under an earlier umask-dependent mode
/// (group-writable, e.g. 0775) are repaired to 0700 when they are real
/// directories owned by this user. Symlinks, foreign owners and every other
/// shape are left untouched so `safe_state` still refuses them.
fn repair_state_modes(env: &RouteEnvironment, r: &Value) {
    let root = state(env, r);
    for p in [
        env.state.join("cortex/connections"),
        root.parent().unwrap().to_owned(),
        root,
    ] {
        if let Ok(m) = fs::symlink_metadata(&p) {
            if m.is_dir()
                && m.uid() == unsafe { libc::getuid() }
                && m.permissions().mode() & 0o022 != 0
            {
                let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o700));
            }
        }
    }
}
/// Python `install_resolution._build_codex_install` raises `InstallUnresolvable`
/// for these shapes before any hash, trust or verifier check. `catalog.refresh`
/// then launches the recorded command with no install: no private adapter,
/// no `CODEX_PATH`/`NO_BROWSER` and no offline effort enrichment, exactly like
/// a direct launch. Admit only the shapes whose Python outcome is certain:
/// no resolvable `@openai/codex` manifest, or (Linux x86_64) a parseable
/// manifest with no resolvable vendored binary. Both manifests must be
/// bounded regular non-symlink files that parse as JSON, so Python's parser
/// cannot raise outside its caught errors. Every other unresolved candidate
/// keeps Python (see `retained`).
pub(super) fn unverified(r: &Value) -> bool {
    let Some(argv) = r["launch_command"].as_array() else {
        return false;
    };
    let Some(manifest) = install::candidate(argv) else {
        return false;
    };
    let parses = |path: &Path| {
        install::plain(path).is_some_and(|b| serde_json::from_slice::<Value>(&b).is_ok())
    };
    let Some(entry) = argv[1].as_str() else {
        return false;
    };
    // Lexically normal absolute entry: dependency lookup is purely lexical in
    // both implementations, so no `.`/`..`/`//` normalization may differ.
    let entry = Path::new(entry);
    if r["engine"] != "codex-acp"
        || !entry.is_absolute()
        || entry.components().collect::<PathBuf>().as_os_str() != entry.as_os_str()
        || !parses(&manifest)
    {
        return false;
    }
    let Some(package) = manifest.parent() else {
        return false;
    };
    let Some(codex) = dependency(package, "@openai/codex", "package.json") else {
        return true;
    };
    if !parses(&codex.join("package.json"))
        || (std::env::consts::OS, std::env::consts::ARCH) != ("linux", "x86_64")
    {
        return false;
    }
    dependency(
        package,
        "@openai/codex-linux-x64",
        "vendor/x86_64-unknown-linux-musl/bin/codex",
    )
    .is_none()
}
/// Why an unresolved Codex package candidate keeps the retained Python
/// producer; `None` when the native producer owns it (verified or unverified)
/// or when the launch is not a Codex package candidate at all.
pub(super) fn retained(env: &RouteEnvironment, r: &Value) -> Option<&'static str> {
    install::candidate(r["launch_command"].as_array()?)?;
    if r["engine"] != "codex-acp" || unverified(r) || resolve(env, r).is_some() {
        return None;
    }
    // Python's verifier may still succeed or fail (trust roots, symlinked
    // components, executability, drift, legacy bin shapes, non-ASCII or
    // non-Linux-x86_64 bundles, unsafe private state): no certain outcome.
    Some("codex_install_outcome_unqualified")
}
impl Install {
    pub(super) fn unchanged(&self) -> bool {
        self.files.unchanged()
    }
    fn patched(&self) -> Option<String> {
        let package = self.manifest.parent()?;
        if self.entry != package.join("dist/index.js")
            || package.file_name()? != "codex-acp"
            || package.parent()?.file_name()? != "@agentclientprotocol"
            || package.parent()?.parent()?.file_name()? != "node_modules"
        {
            return None;
        }
        let rules = data()["rules"][&self.version].as_array()?;
        if let Some(hash) = exact_hash(&self.version) {
            if install::digest(&self.entry) != Some(hash) {
                return None;
            }
        }
        if rules
            .iter()
            .any(|rule| self.source.matches(rule[0].as_str().unwrap()).count() != 1)
        {
            return None;
        }
        let mut source = self.source.clone();
        for rule in rules {
            source = source.replacen(rule[0].as_str()?, rule[1].as_str()?, 1);
        }
        Some(source)
    }
    pub(super) fn prepare(
        &self,
        env: &RouteEnvironment,
        r: &Value,
    ) -> std::result::Result<(PathBuf, PathBuf), String> {
        let fail = || "engine_unreachable:PortableError".to_owned();
        let source = self.patched().ok_or_else(fail)?;
        if !self.unchanged() {
            return Err(fail());
        }
        repair_state_modes(env, r);
        if !safe_state(env, r) {
            return Err(fail());
        }
        let root = state_root(env, r)?;
        let package = root.join("manual-review-adapter");
        let dist = package.join("dist");
        for p in [&package, &dist] {
            private_directory(p, true).map_err(|_| fail())?;
        }
        let entry = dist.join("index.js");
        write_file(&entry, source.as_bytes(), 0o600)?;
        write_file(
            &package.join("package.json"),
            &install::plain(&self.manifest).ok_or_else(fail)?,
            0o600,
        )?;
        let modules = self
            .manifest
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let link = package.join("node_modules");
        if fs::symlink_metadata(&link).is_ok()
            && link.canonicalize().ok().as_deref() != Some(modules)
        {
            fs::remove_file(&link).map_err(|_| fail())?;
        }
        if fs::symlink_metadata(&link).is_err() {
            std::os::unix::fs::symlink(modules, &link).map_err(|_| fail())?;
        }
        if link.canonicalize().ok().as_deref() != Some(modules) {
            return Err(fail());
        }
        if let Some(bytes) = &self.opener {
            write_file(&dist.join("xdg-open"), bytes, 0o700)?;
        }
        // Atomically migrate safe legacy Python artifacts without executing them.
        // This request never replays through Python after artifact preparation.
        let path = root.join("codex-acp-runtime");
        write_file(&path, self.launcher.as_bytes(), 0o700)?;
        if !self.unchanged() {
            return Err(fail());
        }
        Ok((entry, path))
    }
}
fn write_file(path: &Path, bytes: &[u8], mode: u32) -> std::result::Result<(), String> {
    let fail = || "machine_outcome_unknown".to_owned();
    if fs::symlink_metadata(path).is_ok() {
        if !safe_leaf(path, mode) {
            return Err(fail());
        }
        if install::plain(path).as_deref() == Some(bytes) {
            return Ok(());
        }
    }
    let tmp = path.with_extension(format!("{}.tmp", random_id().map_err(|_| fail())?));
    let result = (|| -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::File::open(path.parent().unwrap())?.sync_all()?;
        if !safe_leaf(path, mode) || install::plain(path).as_deref() != Some(bytes) {
            return Err(std::io::Error::other("private artifact drift"));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|_| fail())
}
pub(super) fn parse_ceiling(raw: &[u8]) -> BTreeMap<String, Vec<String>> {
    let Ok(v) = serde_json::from_slice::<Value>(raw) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for entry in v["models"].as_array().into_iter().flatten() {
        if entry["visibility"] != "list" {
            continue;
        }
        let Some(slug) = entry["slug"].as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        let levels: Vec<String> = entry["supported_reasoning_levels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                v["effort"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
            .collect();
        if !levels.is_empty() {
            out.insert(slug.to_owned(), levels);
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        scratch: Scratch,
        env: RouteEnvironment,
        record: Value,
    }
    impl Fixture {
        fn new() -> Self {
            let scratch = Scratch::new().unwrap();
            let root = scratch.0.to_str().unwrap();
            let corpus: Value = serde_json::from_str(include_str!(
                "../../tests/fixtures/config-catalog-producer-python.json"
            ))
            .unwrap();
            let case = corpus["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == "codex-pinned-hoisted")
                .unwrap();
            for (path, item) in case["initial"].as_object().unwrap() {
                let path = scratch.0.join(path);
                if item["directory"] == true {
                    fs::create_dir_all(&path).unwrap();
                } else {
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(
                        &path,
                        item["content"].as_str().unwrap().replace("@ROOT@", root),
                    )
                    .unwrap();
                }
                fs::set_permissions(
                    path,
                    fs::Permissions::from_mode(item["mode"].as_u64().unwrap() as u32),
                )
                .unwrap();
            }
            fs::create_dir_all(scratch.0.join("bin")).unwrap();
            fs::write(scratch.0.join("bin/peer"), "#!/bin/sh\n").unwrap();
            fs::set_permissions(
                scratch.0.join("bin/peer"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            let registry: Value = serde_json::from_slice(
                &fs::read(scratch.0.join("config/cortex/connections.json")).unwrap(),
            )
            .unwrap();
            let record = registry["connections"]["cn_000000000001"].clone();
            let env = RouteEnvironment {
                home: scratch.0.join("home"),
                config: scratch.0.join("config"),
                state: scratch.0.join("state"),
                routing_override: None,
            };
            Self {
                scratch,
                env,
                record,
            }
        }
    }
    #[test]
    fn codex_pins_dependencies_artifacts_and_unsafe_state() {
        let f = Fixture::new();
        let pin = resolve(&f.env, &f.record).unwrap();
        assert!(pin.unchanged());
        fs::create_dir_all(f.env.state.join("cortex")).unwrap();
        let (entry, wrapper) = pin.prepare(&f.env, &f.record).unwrap();
        assert!(entry.is_file() && wrapper.is_file());
        let native = fs::read(&wrapper).unwrap();
        assert_eq!(native, pin.launcher.as_bytes());
        assert!(!native.starts_with(b"#!/usr/bin/env python3"));
        let inode = fs::metadata(&wrapper).unwrap().ino();
        pin.prepare(&f.env, &f.record).unwrap();
        assert_eq!(
            fs::metadata(&wrapper).unwrap().ino(),
            inode,
            "identical artifact reused"
        );
        let legacy = data()["wrapper"]
            .as_str()
            .unwrap()
            .replace("'@BINARY@'", &format!("'{}'", pin.binary.display()));
        fs::write(&wrapper, &legacy).unwrap();
        let mut running_old = fs::File::open(&wrapper).unwrap();
        let old_inode = running_old.metadata().unwrap().ino();
        pin.prepare(&f.env, &f.record).unwrap();
        assert_ne!(fs::metadata(&wrapper).unwrap().ino(), old_inode);
        let mut retained = String::new();
        std::io::Read::read_to_string(&mut running_old, &mut retained).unwrap();
        assert_eq!(
            retained, legacy,
            "already-open legacy artifact remains intact"
        );
        assert_eq!(fs::read(&wrapper).unwrap(), native);
        assert_eq!(
            fs::metadata(&wrapper).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(fs::read_dir(wrapper.parent().unwrap()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
        assert!(
            resolve(&f.env, &f.record).is_some(),
            "existing launcher state stays native"
        );
        fs::write(&pin.binary, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(!pin.unchanged());
        assert!(pin.prepare(&f.env, &f.record).is_err());
        fs::remove_file(&wrapper).unwrap();
        std::os::unix::fs::symlink(f.scratch.0.join("missing"), &wrapper).unwrap();
        assert!(
            resolve(&f.env, &f.record).is_none(),
            "unsafe wrapper selects retained provider before writes"
        );
    }
    #[test]
    fn session_launcher_survives_group_writable_state_left_by_a_permissive_umask() {
        let f = Fixture::new();
        fs::create_dir_all(f.env.state.join("cortex")).unwrap();
        let first = session_launcher(&f.env, &f.record).expect("first launch");
        let connections = f.env.state.join("cortex/connections");
        let home = state(&f.env, &f.record);
        for p in [&connections, home.parent().unwrap(), &home] {
            assert_eq!(
                fs::metadata(p).unwrap().permissions().mode() & 0o022,
                0,
                "created private"
            );
            // What a umask-002 desktop session used to leave behind.
            fs::set_permissions(p, fs::Permissions::from_mode(0o775)).unwrap();
        }
        let second = session_launcher(&f.env, &f.record).expect("launch after 0775 state");
        assert_eq!(first, second);
        for p in [&connections, home.parent().unwrap(), &home] {
            assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o700);
        }
        // A symlinked state directory is still refused, never repaired.
        fs::remove_file(&second).unwrap();
        fs::remove_dir(&home.join("manual-review-adapter")).ok();
        fs::remove_dir(&home).unwrap();
        std::os::unix::fs::symlink(f.scratch.0.join("missing"), &home).unwrap();
        assert!(session_launcher(&f.env, &f.record).is_err());
    }
    #[test]
    fn unresolvable_candidates_are_direct_and_others_retained() {
        let f = Fixture::new();
        assert!(!unverified(&f.record), "verified bundle");
        assert_eq!(retained(&f.env, &f.record), None);
        let pin = resolve(&f.env, &f.record).unwrap();
        let package = pin.manifest.parent().unwrap().to_owned();
        let codex = dependency(&package, "@openai/codex", "package.json")
            .unwrap()
            .join("package.json");
        fs::remove_file(&pin.binary).unwrap();
        assert!(unverified(&f.record), "no vendored binary");
        assert_eq!(retained(&f.env, &f.record), None);
        fs::write(&codex, "{").unwrap();
        assert!(!unverified(&f.record), "Python parser outcome not certain");
        assert_eq!(
            retained(&f.env, &f.record),
            Some("codex_install_outcome_unqualified")
        );
        fs::remove_file(&codex).unwrap();
        assert!(unverified(&f.record), "no @openai/codex manifest");
        let real = package.join("real-package.json");
        fs::rename(&pin.manifest, &real).unwrap();
        std::os::unix::fs::symlink(&real, &pin.manifest).unwrap();
        assert!(!unverified(&f.record), "symlinked adapter manifest");
        assert!(retained(&f.env, &f.record).is_some());
        fs::remove_file(&pin.manifest).unwrap();
        fs::rename(&real, &pin.manifest).unwrap();
        let mut r = f.record.clone();
        r["launch_command"][1] = json!(pin.entry.to_str().unwrap().replace("/dist/", "/dist/./"));
        assert!(!unverified(&r), "lexically non-normal entry");
        assert!(retained(&f.env, &r).is_some());
        let mut r = f.record.clone();
        r["engine"] = json!("claude-agent-acp");
        assert!(!unverified(&r));
        assert_eq!(retained(&f.env, &r), None);
        let mut r = f.record.clone();
        r["launch_command"] = json!([r["launch_command"][0]]);
        assert!(
            !unverified(&r) && retained(&f.env, &r).is_none(),
            "direct launch"
        );
    }
    #[test]
    fn exact_upstream_bundles_are_patched_natively_and_drift_refused() {
        for version in ["2.3.0", "2.6.0"] {
            exact_bundle_is_patched_natively_and_drift_refused(version);
        }
    }
    #[test]
    fn exact_bundle_digests_and_rules_are_registered_per_version() {
        let hashes = &data()["exact_hashes"];
        for version in ["2.3.0", "2.6.0"] {
            let hash = hashes[version].as_str().unwrap();
            assert!(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()));
            assert_eq!(data()["rules"][version].as_array().unwrap().len(), 14);
        }
        assert_ne!(hashes["2.3.0"], hashes["2.6.0"]);
        assert_eq!(
            hashes["2.6.0"],
            "733b7521c2a437a34ed016d45a6f46bebff2bac123a172467574ee827db98b99"
        );
        // A version without an exact digest is not refused by digest, so only
        // the registered ones may carry one: 2.4.0 and 2.5.0 are unregistered.
        assert!(hashes["2.4.0"].is_null() && data()["rules"]["2.4.0"].is_null());
        // 2.6.0 keeps 2.3.0's rule semantics but reads the permission profile
        // and effective-config call sites of its own bundle.
        let (old, new) = (&data()["rules"]["2.3.0"], &data()["rules"]["2.6.0"]);
        for index in [0, 2, 6, 7, 8, 9, 10, 11, 12, 13] {
            assert_eq!(old[index], new[index], "rule {index} is shared");
        }
        for index in [1, 3, 4, 5] {
            assert_ne!(
                old[index][0], new[index][0],
                "rule {index} reads the 2.6.0 site"
            );
        }
        let turn = new[5][1].as_str().unwrap();
        assert!(
            turn.contains("approvalPolicy: \"untrusted\"") && turn.contains("dangerFullAccess")
        );
        assert!(!turn.contains("permissions:"));
    }
    fn exact_bundle_is_patched_natively_and_drift_refused(version: &str) {
        let f = Fixture::new();
        let pin = resolve(&f.env, &f.record).unwrap();
        let rules = data()["rules"][version].as_array().unwrap();
        let site = |r: &Value| r[0].as_str().unwrap().to_owned();
        let source: String = rules
            .iter()
            .map(|r| format!("{}\n// fixture call site\n", site(r)))
            .collect();
        assert!(rules.iter().all(|r| source.matches(&site(r)).count() == 1));
        fs::write(&pin.entry, &source).unwrap();
        fs::write(
            &pin.manifest,
            format!(
                r#"{{"name": "@agentclientprotocol/codex-acp", "version": "{version}", "bin": {{"codex-acp": "dist/index.js"}}}}"#
            ),
        )
        .unwrap();
        // This bundle stands in for the upstream one whose digest Python pins.
        let digest = install::digest(&pin.entry).unwrap();
        EXACT_HASH.with(|h| *h.borrow_mut() = Some((version.into(), digest)));
        assert_eq!(
            retained(&f.env, &f.record),
            None,
            "exact bundle stays native"
        );
        let exact = resolve(&f.env, &f.record).expect("exact bundle resolves natively");
        fs::create_dir_all(f.env.state.join("cortex")).unwrap();
        let (entry, wrapper) = exact.prepare(&f.env, &f.record).unwrap();
        let patched = fs::read_to_string(&entry).unwrap();
        assert!(wrapper.is_file() && patched != source);
        assert!(rules
            .iter()
            .all(|r| patched.contains(r[1].as_str().unwrap())));
        // Same version and call sites under another digest: refused before launch.
        EXACT_HASH.with(|h| *h.borrow_mut() = Some((version.into(), "0".repeat(64))));
        let drifted = resolve(&f.env, &f.record).unwrap();
        assert!(drifted.prepare(&f.env, &f.record).is_err());
        EXACT_HASH.with(|h| *h.borrow_mut() = None);
    }
    #[test]
    fn codex_manifest_and_dependency_drift_is_detected() {
        let f = Fixture::new();
        let pin = resolve(&f.env, &f.record).unwrap();
        let manifest = dependency(
            pin.manifest.parent().unwrap(),
            "@openai/codex",
            "package.json",
        )
        .unwrap()
        .join("package.json");
        fs::write(&manifest, "{\"version\":\"changed\"}").unwrap();
        assert!(!pin.unchanged());
    }
    #[test]
    fn codex_debug_timeout_reaps_and_output_bound_refuses() {
        let f = Fixture::new();
        let binary = resolve(&f.env, &f.record).unwrap().binary;
        fs::write(&binary, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
        let pin = resolve(&f.env, &f.record).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let start = std::time::Instant::now();
        assert!(runtime
            .block_on(ceiling(&pin, Duration::from_millis(25)))
            .unwrap()
            .is_empty());
        assert!(start.elapsed() < Duration::from_secs(3));
        let script = format!(
            "#!/bin/sh\ni=0; while [ \"$i\" -lt 4100 ]; do printf '%s' '{}'; i=$((i+1)); done\n",
            "x".repeat(1024)
        );
        fs::write(&binary, script).unwrap();
        let pin = resolve(&f.env, &f.record).unwrap();
        assert_eq!(
            runtime
                .block_on(ceiling(&pin, Duration::from_secs(3)))
                .unwrap_err(),
            "machine_outcome_unknown"
        );
    }
}
pub(super) async fn ceiling(
    install: &Install,
    budget: Duration,
) -> std::result::Result<BTreeMap<String, Vec<String>>, String> {
    if !install.unchanged() {
        return Ok(BTreeMap::new());
    }
    // Python intentionally uses ambient environment/cwd for this offline command,
    // not the ACP credential environment. Never persist either output stream.
    let mut c = Command::new(&install.binary);
    c.args(["debug", "models"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    unsafe {
        c.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let Ok(mut child) = c.spawn() else {
        return Ok(BTreeMap::new());
    };
    let pid = child.id();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    async fn bounded(stream: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        stream
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(std::io::Error::other("unqualified offline output"));
        }
        Ok(bytes)
    }
    let result = tokio::time::timeout(budget, async {
        tokio::try_join!(bounded(stdout), bounded(stderr), child.wait())
    })
    .await;
    match result {
        Ok(Ok((out, _, status))) => Ok(if status.success() {
            parse_ceiling(&out)
        } else {
            BTreeMap::new()
        }),
        other => {
            if let Some(pid) = pid {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .map_err(|_| "machine_outcome_unknown".to_owned())?
                .map_err(|_| "machine_outcome_unknown".to_owned())?;
            if other.is_ok() {
                Err("machine_outcome_unknown".into())
            } else {
                Ok(BTreeMap::new())
            }
        }
    }
}
