//! Native OpenCode engine support for Settings: trusted-binary discovery with the
//! sandboxed version probe, the owner's provider import and the jailed launch a
//! prompt-free catalog read uses.
//!
//! Python oracles: `engine/opencode_install.py` (`trusted_binary`, `probe_version`),
//! `connections/discovery.py` (`discover_opencode`), `engine/opencode_adapter_acp.py`
//! (`build_opencode_config`, `process_environment`, `owner_provider_config`) and
//! `orchestration/sandbox.py` (`verification_command`, `engine_jail`) for the
//! catalog-read profile: no project grant, a private disposable working directory.
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    ffi::{CString, OsStr, OsString},
    fs,
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// The ACP subcommand of the standalone binary.
pub(crate) const ACP: &str = "acp";
/// The one agent a Cortex OpenCode process runs as.
pub(crate) const AGENT: &str = "cortex";
const PROMPT_FILE: &str = "cortex-opencode-agent.md";
const CREDENTIAL_PREFIX: &str = "cortex-opencode-credential-";
const CATALOG_PROBE_PROMPT: &str =
    "This OpenCode process only answers a Cortex catalog read; no task is running.";
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const OWNER_FILES: [&str; 3] = ["config.json", "opencode.json", "opencode.jsonc"];
const OWNER_KEYS: [&str; 3] = ["provider", "enabled_providers", "disabled_providers"];
const OWNER_CONFIG_MAX: u64 = 1 << 20;
const OWNER_FILE_REF_MAX: u64 = 64 << 10;
const OWNER_PROVIDERS_MAX_CHARS: usize = 64 << 10;

fn unique() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!(
        "{}-{}-{nanos}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
fn executable(path: &Path) -> bool {
    CString::new(path.as_os_str().as_bytes())
        .is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::X_OK) } == 0)
}
/// Python `shutil.which(name, path=PATH)`.
pub(crate) fn which(name: &str, path: Option<&OsString>) -> Option<PathBuf> {
    let default = OsString::from("/bin:/usr/bin");
    std::env::split_paths(path.unwrap_or(&default))
        .map(|dir| dir.join(name))
        .find(|candidate| {
            let absolute = Path::new("/").join(candidate);
            absolute.exists() && executable(&absolute) && !absolute.is_dir()
        })
        .map(|p| Path::new("/").join(p))
}

/// `opencode_install.trusted_binary`: the error is the Python refusal code.
pub(crate) fn trusted_binary(binary: &Path, home: &Path) -> std::result::Result<(), &'static str> {
    const UNTRUSTED: &str = "engine_install_untrusted_location";
    const DRIFTED: &str = "engine_install_drifted";
    if !binary.is_absolute() {
        return Err(UNTRUSTED);
    }
    let resolved = binary.canonicalize().map_err(|_| DRIFTED)?;
    if resolved != binary {
        return Err(UNTRUSTED);
    }
    if [".local/share/app.codeg", ".config/app.codeg"]
        .iter()
        .any(|root| resolved.starts_with(home.join(root)))
    {
        return Err(UNTRUSTED);
    }
    let info = fs::metadata(binary).map_err(|_| DRIFTED)?;
    if !info.is_file() || !executable(binary) {
        return Err(DRIFTED);
    }
    if ![unsafe { libc::getuid() }, 0].contains(&info.uid()) || info.mode() & 0o022 != 0 {
        return Err(UNTRUSTED);
    }
    Ok(())
}

/// The first `\bv?(\d+\.\d+\.\d+)\b` of `text`.
pub(crate) fn first_version(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let digits = |from: usize| {
        let mut end = from;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        end
    };
    for start in 0..bytes.len() {
        if start > 0 && word(bytes[start - 1]) {
            continue;
        }
        let mut at = start;
        if bytes[at] == b'v' {
            at += 1;
        }
        let mut parts = 0;
        let first = at;
        loop {
            let end = digits(at);
            if end == at {
                break;
            }
            parts += 1;
            at = end;
            if parts == 3 {
                break;
            }
            if bytes.get(at) == Some(&b'.') {
                at += 1;
            } else {
                break;
            }
        }
        if parts == 3 && !bytes.get(at).is_some_and(|b| word(*b)) {
            return Some(text[first..at].to_owned());
        }
    }
    None
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)
}
/// `orchestration.sandbox.verification_command(system_root=True)` for an empty root.
fn verification(bwrap: &Path, root: &Path, scratch: &Path, argv: &[&OsStr]) -> Command {
    let var_tmp = scratch.join("var-tmp");
    let _ = private_dir(&var_tmp);
    let mut c = Command::new(bwrap);
    c.args(["--die-with-parent", "--unshare-net", "--clearenv"])
        .args(["--setenv", "PATH", "/usr/local/bin:/usr/bin:/bin"]);
    for name in ["HOME", "TMPDIR", "TMP", "TEMP"] {
        c.arg("--setenv").arg(name).arg(scratch);
    }
    c.args(["--setenv", "LANG", "C.UTF-8"])
        .args(["--setenv", "PYTHONDONTWRITEBYTECODE", "1"])
        .args(["--ro-bind", "/", "/", "--proc", "/proc"])
        .args(["--dev-bind", "/dev", "/dev", "--bind", "/tmp", "/tmp"])
        .arg("--bind")
        .arg(&var_tmp)
        .arg("/var/tmp")
        .arg("--bind")
        .arg(scratch)
        .arg(scratch)
        .arg("--ro-bind")
        .arg(root)
        .arg(root)
        .arg("--chdir")
        .arg(root)
        .arg("--")
        .args(argv);
    c
}
/// `opencode_install.probe_version`: `<binary> --version` in a network-less,
/// read-only sandbox with a throwaway HOME; `None` when it cannot be established.
pub(crate) fn probe_version(binary: &Path, path: Option<&OsString>) -> Option<String> {
    let bwrap = which("bwrap", path)?;
    let directory = std::env::temp_dir().join(format!("cortex-opencode-version-{}", unique()));
    private_dir(&directory).ok()?;
    let run = || -> Option<String> {
        let (root, scratch) = (directory.join("root"), directory.join("scratch"));
        private_dir(&root).ok()?;
        private_dir(&scratch).ok()?;
        let mut command = verification(
            &bwrap,
            &root,
            &scratch,
            &[binary.as_os_str(), "--version".as_ref()],
        );
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(test)]
        if crate::native_account_fixture::guest().is_ok() {
            command.stderr(Stdio::inherit());
        }
        let mut child = command.spawn().ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut out = vec![];
            let _ = (&mut stdout).take(1 << 20).read_to_end(&mut out);
            out
        });
        let deadline = Instant::now() + PROBE_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().ok()? {
                break Some(status);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let out = reader.join().ok()?;
        if !status?.success() {
            return None;
        }
        first_version(&String::from_utf8_lossy(&out))
    };
    let version = run();
    let _ = fs::remove_dir_all(&directory);
    version
}

/// A trusted OpenCode binary that reports a version.
pub(crate) struct Found {
    pub version: String,
    pub binary: PathBuf,
}
/// `discovery.discover_opencode`: candidates in order `binary`,
/// `~/.opencode/bin/opencode`, `opencode` on PATH (`$CORTEX_OPENCODE_BINARY` is
/// not inherited by the service), each resolved to its real file; the first trusted
/// one whose version can be read.
pub(crate) fn discover(
    home: &Path,
    path: Option<&OsString>,
    binary: Option<&Path>,
) -> Option<Found> {
    let candidates = [
        binary.map(Path::to_path_buf),
        Some(home.join(".opencode/bin/opencode")),
        which("opencode", path),
    ];
    let mut seen = vec![];
    for candidate in candidates.into_iter().flatten() {
        let Ok(real) = candidate.canonicalize() else {
            continue;
        };
        if seen.contains(&real) {
            continue;
        }
        seen.push(real.clone());
        if trusted_binary(&real, home).is_err() {
            continue;
        }
        if let Some(version) = probe_version(&real, path) {
            return Some(Found {
                version,
                binary: real,
            });
        }
    }
    None
}
/// `discovery.is_qualified("opencode", version)`.
pub(crate) fn qualified(version: &str) -> bool {
    crate::config_read_model::adapter_version("opencode", version)["qualified"] == true
}
/// `install_resolution._resolve_opencode_install` for the exact `(binary, "acp")`
/// launch: trust, the sandboxed version probe and a qualified version.
pub(crate) fn verified_install(
    command: &[String],
    home: &Path,
    path: Option<&OsString>,
) -> std::result::Result<PathBuf, &'static str> {
    if command.len() != 2 || command[1] != ACP {
        return Err("invalid_field");
    }
    let binary = PathBuf::from(&command[0]);
    trusted_binary(&binary, home)?;
    let version = probe_version(&binary, path).ok_or("engine_install_drifted")?;
    if !qualified(&version) {
        return Err("opencode_version_unqualified");
    }
    Ok(binary)
}

// --- the process configuration and the owner's providers -------------------------

/// `build_opencode_config`.
fn config(prompt: &str) -> Value {
    json!({
        "autoupdate": false, "share": "disabled", "default_agent": AGENT,
        "plugin": ["-opencode.agent", "-opencode.plan"],
        "agent": {
            AGENT: {"mode": "primary", "prompt": format!("{{file:{prompt}}}")},
            "build": {"disable": true}, "plan": {"disable": true}, "title": {"disable": true},
        },
        "permission": {"*": "ask", "task": "deny", "question": "deny", "skill": "deny"},
    })
}
/// `process_environment`: the owner's provider definitions join Cortex's configuration.
fn process_environment(prompt: &str, owner: &Value) -> BTreeMap<String, String> {
    let mut config = config(prompt);
    if let Some(owner) = owner.as_object().filter(|o| !o.is_empty()) {
        if let Some(providers) = owner.get("provider").and_then(Value::as_object) {
            if !providers.is_empty() {
                config["provider"] = Value::Object(providers.clone());
            }
        }
        for key in ["enabled_providers", "disabled_providers"] {
            if let Some(list) = owner.get(key) {
                config[key] = list.clone();
            }
        }
    }
    BTreeMap::from([
        (
            "OPENCODE_CONFIG_CONTENT".to_owned(),
            serde_json::to_string(&config).unwrap(),
        ),
        ("OPENCODE_DISABLE_PROJECT_CONFIG".to_owned(), "1".to_owned()),
        ("OPENCODE_DISABLE_AUTOUPDATE".to_owned(), "1".to_owned()),
    ])
}

/// JSONC to JSON, leaving strings intact (comments, then trailing commas).
fn strip_jsonc(text: &str) -> std::result::Result<String, ()> {
    fn scan(
        source: &[char],
        mut handle: impl FnMut(&[char], usize, &mut String) -> std::result::Result<usize, ()>,
    ) -> std::result::Result<String, ()> {
        let (mut out, mut i) = (String::new(), 0);
        while i < source.len() {
            if source[i] == '"' {
                let mut j = i + 1;
                while j < source.len() && source[j] != '"' {
                    j += if source[j] == '\\' { 2 } else { 1 };
                }
                let end = (j + 1).min(source.len());
                out.extend(&source[i..end]);
                i = j + 1;
            } else {
                i = handle(source, i, &mut out)?;
            }
        }
        Ok(out)
    }
    let chars: Vec<char> = text.chars().collect();
    let without_comments = scan(&chars, |s, i, out| {
        if s[i] == '/' && s.get(i + 1) == Some(&'/') {
            return Ok(s[i..]
                .iter()
                .position(|c| *c == '\n')
                .map_or(s.len(), |p| i + p));
        }
        if s[i] == '/' && s.get(i + 1) == Some(&'*') {
            let mut j = i + 2;
            while j + 1 < s.len() {
                if s[j] == '*' && s[j + 1] == '/' {
                    return Ok(j + 2);
                }
                j += 1;
            }
            return Err(());
        }
        out.push(s[i]);
        Ok(i + 1)
    })?;
    let chars: Vec<char> = without_comments.chars().collect();
    scan(&chars, |s, i, out| {
        if s[i] == ',' {
            let mut j = i + 1;
            while j < s.len() && " \t\r\n".contains(s[j]) {
                j += 1;
            }
            if j < s.len() && (s[j] == '}' || s[j] == ']') {
                return Ok(i + 1);
            }
        }
        out.push(s[i]);
        Ok(i + 1)
    })
}
fn merge(base: &mut Map<String, Value>, extra: Map<String, Value>) {
    for (key, value) in extra {
        match (base.get_mut(&key), value) {
            (Some(Value::Object(existing)), Value::Object(new)) => merge(existing, new),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}
fn read_bounded(path: &Path, limit: u64) -> Option<Vec<u8>> {
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut data = vec![];
    fs::File::open(path)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() as u64 <= limit).then_some(data)
}
fn read_owner_configuration(directory: &Path) -> (Map<String, Value>, Vec<String>) {
    let (mut merged, mut problems) = (Map::new(), vec![]);
    for name in OWNER_FILES {
        let path = directory.join(name);
        if !path.exists() {
            continue;
        }
        let parsed = read_bounded(&path, OWNER_CONFIG_MAX)
            .and_then(|data| String::from_utf8(data).ok())
            .and_then(|text| strip_jsonc(&text).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let Some(Value::Object(parsed)) = parsed else {
            problems.push(format!("{name}:unreadable"));
            continue;
        };
        let picked: Map<String, Value> = parsed
            .into_iter()
            .filter(|(k, _)| OWNER_KEYS.contains(&k.as_str()))
            .collect();
        merge(&mut merged, picked);
    }
    (merged, problems)
}
/// The matches of `\{(env|file):([^}]*)\}` in `text` as `(start, end, is_env, target)`.
fn substitutions(text: &str) -> Vec<(usize, usize, bool, &str)> {
    let mut out = vec![];
    let mut at = 0;
    while let Some(open) = text[at..].find('{').map(|p| p + at) {
        let rest = &text[open + 1..];
        let kind = if rest.starts_with("env:") {
            Some((true, 4))
        } else if rest.starts_with("file:") {
            Some((false, 5))
        } else {
            None
        };
        if let Some((is_env, skip)) = kind {
            if let Some(close) = rest[skip..].find('}') {
                let end = open + 1 + skip + close + 1;
                out.push((open, end, is_env, &rest[skip..skip + close]));
                at = end;
                continue;
            }
        }
        at = open + 1;
    }
    out
}
fn has_substitution(text: &str) -> bool {
    !substitutions(text).is_empty()
}
fn credential_key(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        "apikey",
        "api_key",
        "api-key",
        "token",
        "secret",
        "password",
        "authorization",
        "cookie",
    ]
    .iter()
    .any(|k| lower.contains(k))
}
fn provider_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(c))
}
fn allowed_npm(value: &str) -> bool {
    value.strip_prefix("@ai-sdk/").is_some_and(|rest| {
        let b = rest.as_bytes();
        !b.is_empty()
            && b.len() <= 64
            && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
            && b[1..]
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
    })
}
struct Skip(&'static str);
const CREDENTIAL_MARK: &str = "\u{0}cortex-credential";
fn substitute(
    text: &str,
    home: &Path,
    config_dir: &Path,
    environ: &BTreeMap<String, String>,
) -> std::result::Result<String, Skip> {
    let mut out = String::new();
    let mut last = 0;
    for (start, end, is_env, target) in substitutions(text) {
        out.push_str(&text[last..start]);
        last = end;
        let target = target.trim();
        if is_env {
            out.push_str(environ.get(target).map_or("", String::as_str));
            continue;
        }
        let path = if let Some(rest) = target.strip_prefix("~/") {
            home.join(rest)
        } else if target.starts_with('/') {
            PathBuf::from(target)
        } else {
            config_dir.join(target)
        };
        let data = read_bounded(&path, OWNER_FILE_REF_MAX)
            .and_then(|d| String::from_utf8(d).ok())
            .ok_or(Skip("file_reference_unreadable"))?;
        out.push_str(data.trim());
    }
    out.push_str(&text[last..]);
    Ok(out)
}
/// `_sanitize_provider`: `None` is Python's `_OMIT`.
fn walk(
    node: &Value,
    secret: bool,
    ctx: &(&Path, &Path, &BTreeMap<String, String>),
    credentials: &mut Vec<Vec<u8>>,
) -> std::result::Result<Option<Value>, Skip> {
    match node {
        Value::Object(map) => {
            let mut out = Map::new();
            for (name, value) in map {
                if has_substitution(name) {
                    return Err(Skip("unsupported_substitution"));
                }
                if name == "npm" && !value.as_str().is_some_and(allowed_npm) {
                    return Err(Skip("package_not_allowed"));
                }
                let flagged = secret || name == "headers" || credential_key(name);
                if let Some(resolved) = walk(value, flagged, ctx, credentials)? {
                    out.insert(name.clone(), resolved);
                }
            }
            Ok(Some(Value::Object(out)))
        }
        Value::Array(items) => {
            let mut out = vec![];
            for item in items {
                if let Some(resolved) = walk(item, secret, ctx, credentials)? {
                    out.push(resolved);
                }
            }
            Ok(Some(Value::Array(out)))
        }
        Value::String(text) => {
            let value = substitute(text, ctx.0, ctx.1, ctx.2)?;
            if secret || has_substitution(text) {
                if value.is_empty() {
                    return Ok(None);
                }
                credentials.push(value.into_bytes());
                return Ok(Some(json!({CREDENTIAL_MARK: credentials.len() - 1})));
            }
            if has_substitution(&value) {
                return Err(Skip("unsupported_substitution"));
            }
            Ok(Some(Value::String(value)))
        }
        other => Ok(Some(other.clone())),
    }
}
fn refer(node: Value, paths: &[String]) -> Value {
    match node {
        Value::Object(map) => {
            if let Some(index) = map.get(CREDENTIAL_MARK).and_then(Value::as_u64) {
                return Value::String(format!("{{file:{}}}", paths[index as usize]));
            }
            Value::Object(map.into_iter().map(|(k, v)| (k, refer(v, paths))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(|v| refer(v, paths)).collect()),
        other => other,
    }
}
/// What `owner_provider_config` carries into a Cortex process.
#[derive(Default)]
pub(crate) struct OwnerProviders {
    pub config: Value,
    pub files: Vec<(String, Vec<u8>)>,
    pub imported: Vec<String>,
    pub skipped: Vec<(String, String)>,
}
/// `owner_provider_config(home, credential_dir, environ)`.
pub(crate) fn owner_providers(
    home: &Path,
    credential_dir: &str,
    environ: &BTreeMap<String, String>,
) -> OwnerProviders {
    let config_dir = home.join(".config/opencode");
    let (owner, problems) = read_owner_configuration(&config_dir);
    let mut skipped: Vec<(String, String)> =
        problems.into_iter().map(|p| ("*".into(), p)).collect();
    let (mut files, mut imported) = (vec![], vec![]);
    let mut providers = Map::new();
    let mut definitions = owner.get("provider").cloned();
    if definitions.as_ref().is_some_and(|d| !d.is_object()) {
        skipped.push(("*".into(), "invalid_provider_section".into()));
        definitions = None;
    }
    let mut entries: Vec<(String, Value)> = definitions
        .and_then(|d| d.as_object().cloned())
        .map(|m| m.into_iter().collect())
        .unwrap_or_default();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let ctx = (home, config_dir.as_path(), environ);
    for (id, definition) in entries {
        if !provider_id(&id) {
            skipped.push(("?".into(), "invalid_provider_id".into()));
            continue;
        }
        if !definition.is_object() {
            skipped.push((id, "invalid_definition".into()));
            continue;
        }
        let mut credentials = vec![];
        match walk(&definition, false, &ctx, &mut credentials) {
            Err(Skip(reason)) => skipped.push((id, reason.into())),
            Ok(sanitized) => {
                let paths: Vec<String> = (0..credentials.len())
                    .map(|i| format!("{credential_dir}/{CREDENTIAL_PREFIX}{}", files.len() + i))
                    .collect();
                providers.insert(id.clone(), refer(sanitized.unwrap_or(Value::Null), &paths));
                files.extend(paths.into_iter().zip(credentials));
                imported.push(id);
            }
        }
    }
    let mut config = Map::new();
    if !providers.is_empty() {
        config.insert("provider".into(), Value::Object(providers));
    }
    for key in ["enabled_providers", "disabled_providers"] {
        let Some(value) = owner.get(key) else {
            continue;
        };
        if value
            .as_array()
            .is_some_and(|a| a.iter().all(|i| i.as_str().is_some_and(provider_id)))
        {
            config.insert(key.into(), value.clone());
        } else {
            skipped.push(("*".into(), format!("invalid_{key}")));
        }
    }
    let config = Value::Object(config);
    if serde_json::to_string(&config).unwrap().len() > OWNER_PROVIDERS_MAX_CHARS {
        skipped.push(("*".into(), "configuration_too_large".into()));
        return OwnerProviders {
            config: json!({}),
            skipped,
            ..Default::default()
        };
    }
    OwnerProviders {
        config,
        files,
        imported,
        skipped,
    }
}

// --- the catalog-read launch ------------------------------------------------------

/// OpenCode needs an unprivileged (non-setuid) bubblewrap: its per-sandbox user
/// namespace keeps another jail from reading the control-server credential.
pub(crate) fn isolating_bwrap(path: Option<&OsString>) -> Option<PathBuf> {
    let bwrap = which("bwrap", path)?;
    (fs::metadata(&bwrap).ok()?.mode() & 0o4000 == 0).then_some(bwrap)
}
/// `sandbox.tool_state_dirs`: the HOME directories every sandboxed process may write.
fn tool_state_dirs(home: &Path) -> Vec<PathBuf> {
    for name in [".cache", ".npm"] {
        let path = home.join(name);
        if !path.exists() && path.symlink_metadata().is_err() {
            let _ = private_dir(&path);
        }
    }
    const SENSITIVE: [&str; 17] = [
        ".ssh",
        ".gnupg",
        ".aws",
        ".azure",
        ".kube",
        ".docker",
        ".password-store",
        ".pki",
        ".mozilla",
        ".thunderbird",
        ".config",
        ".local",
        ".claude",
        ".codex",
        ".cortex",
        ".git",
        ".paseo",
    ];
    let mut found = vec![];
    let Ok(entries) = fs::read_dir(home) else {
        return found;
    };
    let is_dir = |p: &Path| p.symlink_metadata().is_ok_and(|m| m.is_dir());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let hidden_sensitive =
            SENSITIVE.contains(&name.as_str()) || name == ".hermes" || name == ".cortex-dev";
        if (name.starts_with('.') || name == "go") && !hidden_sensitive && is_dir(&entry.path()) {
            found.push(entry.path());
        }
    }
    let share = home.join(".local/share");
    if is_dir(&share) {
        if let Ok(entries) = fs::read_dir(&share) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !["keyrings", "applications", "autostart", "systemd"].contains(&name.as_str())
                    && is_dir(&entry.path())
                {
                    found.push(entry.path());
                }
            }
        }
    }
    let state = home.join(".local/state");
    if is_dir(&state) {
        found.push(state);
    }
    found.sort();
    found
}
fn memfd(content: &[u8]) -> std::io::Result<fs::File> {
    use std::os::fd::FromRawFd;
    let name = CString::new("cortex-engine-data").unwrap();
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut file = unsafe { fs::File::from_raw_fd(fd) };
    file.write_all(content)?;
    use std::io::Seek;
    file.rewind()?;
    Ok(file)
}
/// The jailed OpenCode launch of one catalog read plus what must outlive its spawn.
pub(crate) struct Launch {
    pub command: Command,
    pub imported: Vec<String>,
    pub skipped: Vec<(String, String)>,
    _data: Vec<fs::File>,
}
/// `EngineProcess(..., jail={"scopes": [], "readonly": []})` for OpenCode with no
/// project grant: `workspace` is a private, read-only, empty directory and `jail_tmp`
/// the process's scratch. `base` carries the allowlisted caller environment
/// (`PATH`, `LANG`, ...). `data` is the engine's own data directory
/// (`$XDG_DATA_HOME/opencode`).
pub(crate) fn launch(
    bwrap: &Path,
    binary: &Path,
    home: &Path,
    data: &Path,
    workspace: &Path,
    jail_tmp: &Path,
    base: &BTreeMap<String, String>,
) -> std::io::Result<Launch> {
    private_dir(&jail_tmp.join("xdg-state"))?;
    let prompt = jail_tmp.join(PROMPT_FILE);
    let environ: BTreeMap<String, String> = std::env::vars().collect();
    let owner = owner_providers(home, jail_tmp.to_str().unwrap_or("/"), &environ);
    let mut env = base.clone();
    env.insert("HOME".into(), home.to_string_lossy().into_owned());
    env.insert(
        "XDG_DATA_HOME".into(),
        data.parent().unwrap_or(data).to_string_lossy().into_owned(),
    );
    env.extend(process_environment(
        prompt.to_str().unwrap_or("/"),
        &owner.config,
    ));
    env.insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
    env.insert(
        "XDG_STATE_HOME".into(),
        jail_tmp.join("xdg-state").to_string_lossy().into_owned(),
    );
    env.insert("TMPDIR".into(), jail_tmp.to_string_lossy().into_owned());
    let mut c = Command::new(bwrap);
    c.args([
        "--die-with-parent",
        "--unshare-user",
        "--unshare-pid",
        "--unshare-ipc",
        "--clearenv",
    ]);
    for (name, value) in &env {
        c.arg("--setenv").arg(name).arg(value);
    }
    let var_tmp = jail_tmp.join("var-tmp");
    private_dir(&var_tmp)?;
    c.args(["--ro-bind", "/", "/", "--proc", "/proc"])
        .args(["--dev-bind", "/dev", "/dev", "--bind", "/tmp", "/tmp"])
        .arg("--bind")
        .arg(&var_tmp)
        .arg("/var/tmp")
        .arg("--bind")
        .arg(jail_tmp)
        .arg(jail_tmp);
    for name in ["/etc/passwd", "/etc/group"] {
        if Path::new(name).is_file() {
            c.args(["--ro-bind", name, name]);
        }
    }
    // HOME is read-only; the tool directories (and OpenCode's data directory) are not.
    c.arg("--ro-bind").arg(home).arg(home);
    for directory in tool_state_dirs(home) {
        c.arg("--bind").arg(&directory).arg(&directory);
    }
    c.arg("--ro-bind").arg(workspace).arg(workspace);
    if !data.exists() {
        fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(data)?;
    }
    let real = data.canonicalize()?;
    if !real.is_dir() {
        return Err(std::io::Error::other("engine_state_dir_invalid"));
    }
    c.arg("--bind").arg(&real).arg(&real);
    // `bin` holds helper executables OpenCode later runs outside any jail: read-only.
    let bin = real.join("bin");
    if !bin.exists() && bin.symlink_metadata().is_err() {
        private_dir(&bin)?;
    }
    if let Ok(target) = bin.canonicalize() {
        if target.starts_with(&real) {
            c.arg("--ro-bind").arg(&target).arg(&target);
        }
    }
    let plugins = real.join("plugins");
    if plugins.is_dir() && !plugins.is_symlink() {
        c.arg("--tmpfs")
            .arg(&plugins)
            .arg("--remount-ro")
            .arg(&plugins);
    }
    // The owner's own OpenCode configuration is hidden behind a private tmpfs.
    let masked = home.join(".config/opencode");
    let masked = if masked.is_symlink() {
        masked.canonicalize().ok()
    } else {
        Some(masked)
    };
    if let Some(masked) = masked.filter(|m| m.is_dir()) {
        c.arg("--tmpfs").arg(masked);
    }
    let mut files = vec![];
    let mut data_files = vec![(
        prompt.to_string_lossy().into_owned(),
        CATALOG_PROBE_PROMPT.as_bytes().to_vec(),
    )];
    data_files.extend(owner.files.iter().cloned());
    let mut descriptors = vec![];
    for (destination, content) in &data_files {
        let file = memfd(content)?;
        use std::os::fd::AsRawFd;
        descriptors.push(file.as_raw_fd());
        c.args(["--perms", "0400", "--ro-bind-data"])
            .arg(file.as_raw_fd().to_string())
            .arg(destination);
        files.push(file);
    }
    c.arg("--chdir")
        .arg(workspace)
        .arg("--");
    // Only the explicitly selected synthetic guest gets extra setup.
    #[cfg(test)]
    c.args(crate::native_account_fixture::prefix("catalogue", home)?);
    c.arg(binary).arg(ACP);
    c.env_clear().env("PATH", "/usr/bin:/bin");
    // The descriptors were created close-on-exec: only this child inherits them.
    unsafe {
        c.pre_exec(move || {
            for fd in &descriptors {
                if libc::fcntl(*fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    Ok(Launch {
        command: c,
        imported: owner.imported,
        skipped: owner.skipped,
        _data: files,
    })
}
/// The allowlisted caller environment of `connections.environment.build_process_env`.
pub(crate) fn base_environment() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in [
        "PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TMPDIR", "USER", "LOGNAME",
    ] {
        if let Ok(mut value) = std::env::var(key) {
            if key == "PATH" {
                value = value
                    .split(':')
                    .filter(|p| p.starts_with('/'))
                    .collect::<Vec<_>>()
                    .join(":");
            }
            env.insert(key.to_owned(), value);
        }
    }
    env
}

#[cfg(test)]
#[path = "config_opencode_tests.rs"]
mod tests;
