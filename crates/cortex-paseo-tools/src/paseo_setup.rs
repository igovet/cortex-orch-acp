//! The Paseo-on-Rust setup commands of `cortex-paseo-tools` (the Rust
//! replacement of the retired Python setup script). Every command prints one
//! JSON document (sorted keys, two-space indent) on success; a refusal prints
//! `error: ...` on stderr and exits 1. Nothing here runs an interpreter.
//!
//! - `bootstrap-paseo-git-install`: the Paseo plugin build step. Stages an
//!   owner-private runtime `<state>/paseo-runtime/<version>` (the non-code
//!   payload the Rust entries read from `CORTEX_ACP_RUNTIME_ROOT`, the load
//!   shim and the two built binaries) and writes the launch record.
//! - `configure-paseo-plugin-launch` / `install-paseo-plugin`: the explicit
//!   launch record / full daemon installation for a checkout build.
//! - `init-storage`, `status`, `validate-connection`, `add-connection`,
//!   `install-engine`, `add-opencode-connection`, `refresh-catalog`,
//!   `pin-route`: thin wrappers over the native settings API (`config`).
use crate::config::{self, Environment};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

type Res<T> = Result<T, String>;

pub const COMMANDS: [&str; 11] = [
    "status",
    "init-storage",
    "bootstrap-paseo-git-install",
    "configure-paseo-plugin-launch",
    "install-paseo-plugin",
    "validate-connection",
    "add-connection",
    "install-engine",
    "add-opencode-connection",
    "refresh-catalog",
    "pin-route",
];

/// Package files the Rust entries read at runtime, relative to the payload
/// root. The load shim imports `replay-spool.mjs` from its own directory.
const PAYLOAD_FILES: [&str; 4] = [
    "package-identity.json",
    "pinned-dependencies.json",
    "integrations/paseo/server/acp-load-shim.mjs",
    "integrations/paseo/server/replay-spool.mjs",
];
const BINARIES: [&str; 2] = ["cortex-acp", "cortex-paseo-tools"];
const SHIM: &str = "integrations/paseo/server/acp-load-shim.mjs";
const LAUNCH_FILE: &str = "paseo-plugin-launch.json";
const EXCLUDED_DIRS: [&str; 15] = [
    ".cache",
    ".git",
    ".next",
    ".tmp",
    ".turbo",
    "__pycache__",
    "build",
    "cache",
    "coverage",
    "dist",
    "node_modules",
    "out",
    "target",
    "temp",
    "tmp",
];
const PASEO_CLI_CANDIDATES: [&str; 2] = ["paseo", "/opt/Paseo/resources/bin/paseo"];

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}

/// Run one setup command; `None` when `command` is not one of them.
pub fn run(command: &str, argv: &[String]) -> Option<(String, i32)> {
    if !COMMANDS.contains(&command) {
        return None;
    }
    Some(match dispatch(command, argv) {
        Ok(value) => (pretty(&value), 0),
        Err(message) => {
            eprintln!("error: {message}");
            (String::new(), 1)
        }
    })
}

fn dispatch(command: &str, argv: &[String]) -> Res<Value> {
    let source = default_source_root();
    match command {
        "status" => {
            no_flags(argv)?;
            status()
        }
        "init-storage" => {
            no_flags(argv)?;
            init_storage(&source)
        }
        "bootstrap-paseo-git-install" => {
            let flags = Flags::parse(argv, &["--source-root", "--bin-dir"], &[])?;
            let source = flags.path("--source-root")?.unwrap_or(source);
            let bin_dir = match flags.path("--bin-dir")? {
                Some(dir) => dir,
                None => own_directory()?,
            };
            bootstrap(&source, &bin_dir)
        }
        "configure-paseo-plugin-launch" => {
            let flags = Flags::parse(argv, &["--rust-binary", "--runtime-root"], &["--replace"])?;
            let runtime = flags.path("--runtime-root")?.unwrap_or(source);
            let binary = select_binary(flags.path("--rust-binary")?)?;
            write_launch_config(
                &Roots::from_process()?,
                &runtime,
                &binary,
                flags.has("--replace"),
                true,
            )
        }
        "install-paseo-plugin" => {
            let flags = Flags::parse(argv, &["--rust-binary", "--source-root"], &[])?;
            let source = flags.path("--source-root")?.unwrap_or(source);
            let binary = select_binary(flags.path("--rust-binary")?)?;
            install_paseo_plugin(&source, &binary)
        }
        "validate-connection" => {
            let flags = Flags::parse(argv, &["--connection"], &[])?;
            let id = flags.required("--connection")?;
            let revision = connections_revision(&source)?;
            api(
                &source,
                json!({"surface":"connections","action":"update","operation":"validate",
                       "connection_id":id,"expected_revision":revision}),
            )
        }
        "add-connection" => add_connection(&source, argv),
        "install-engine" => {
            let flags = Flags::parse(argv, &["--engine"], &[])?;
            api(
                &source,
                json!({"surface":"connections","action":"install_engine","engine":flags.required("--engine")?}),
            )
        }
        "add-opencode-connection" => {
            let flags = Flags::parse(argv, &["--binary", "--name"], &[])?;
            let mut request = json!({"surface":"connections","action":"onboard_opencode",
                "explicit_opt_in":true,"expected_revision":connections_revision(&source)?});
            if let Some(binary) = flags.get("--binary") {
                request["binary"] = json!(binary);
            }
            if let Some(name) = flags.get("--name") {
                request["name"] = json!(name);
            }
            api(&source, request)
        }
        "refresh-catalog" => {
            let flags = Flags::parse(argv, &["--workspace"], &[])?;
            let mut request = json!({"surface":"route_catalog","action":"refresh"});
            if let Some(path) = flags.path("--workspace")? {
                request["project"] = json!(path);
            }
            api(&source, request)
        }
        "pin-route" => pin_route(&source, argv),
        _ => Err("unknown command".into()),
    }
}

// ---------------------------------------------------------------- arguments

struct Flags {
    values: BTreeMap<String, String>,
    switches: Vec<String>,
}
impl Flags {
    fn parse(argv: &[String], valued: &[&str], switches: &[&str]) -> Res<Self> {
        let mut flags = Self {
            values: BTreeMap::new(),
            switches: vec![],
        };
        let mut it = argv.iter();
        while let Some(arg) = it.next() {
            if switches.contains(&arg.as_str()) {
                flags.switches.push(arg.clone());
            } else if valued.contains(&arg.as_str()) {
                let Some(value) = it.next() else {
                    return Err(format!("{arg} needs a value"));
                };
                if flags.values.insert(arg.clone(), value.clone()).is_some() {
                    return Err(format!("{arg} was given twice"));
                }
            } else {
                return Err(format!("unrecognized argument {arg:?}"));
            }
        }
        Ok(flags)
    }
    fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }
    fn required(&self, name: &str) -> Res<String> {
        self.get(name)
            .map(str::to_owned)
            .ok_or_else(|| format!("{name} is required"))
    }
    fn path(&self, name: &str) -> Res<Option<PathBuf>> {
        match self.get(name) {
            None => Ok(None),
            Some(p) if p.starts_with('/') && !p.contains('\0') => Ok(Some(PathBuf::from(p))),
            Some(_) => Err(format!("{name} must be an absolute path")),
        }
    }
}
fn no_flags(argv: &[String]) -> Res<()> {
    Flags::parse(argv, &[], &[]).map(|_| ())
}

// ------------------------------------------------------------------- roots

/// This checkout (the compiled-in manifest location), the default payload
/// root exactly as `cortex-acp` itself resolves it.
fn default_source_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}
fn own_directory() -> Res<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.canonicalize().ok())
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .ok_or_else(|| "cannot locate this executable's directory".to_string())
}
fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
/// The cortex-acp an explicit step records: the named binary, else the one
/// beside this tool. The tools binary must sit beside it.
fn select_binary(named: Option<PathBuf>) -> Res<PathBuf> {
    let binary = match named {
        Some(path) => path,
        None => own_directory()?.join("cortex-acp"),
    };
    let tools = binary.with_file_name("cortex-paseo-tools");
    let missing: Vec<String> = [&binary, &tools]
        .into_iter()
        .filter(|p| !is_executable(p))
        .map(|p| p.display().to_string())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "the Rust Paseo entry is not built: {}. Build it with `cargo build --release --locked -p cortex-acp -p cortex-paseo-tools` or pass --rust-binary PATH.",
            missing.join(", ")
        ));
    }
    Ok(binary)
}

/// Python `Path.resolve()` (non-strict): canonicalize the longest existing
/// prefix and append the rest lexically.
fn resolve_lenient(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => resolve_lenient(parent).join(name),
        _ => path.to_path_buf(),
    }
}

struct Roots {
    home: PathBuf,
    raw_config_home: Option<String>,
    config_base: PathBuf,
    state_base: PathBuf,
}
impl Roots {
    fn from_process() -> Res<Self> {
        let home = std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .ok_or("HOME must be set to an absolute directory")?;
        let symlink = fs::symlink_metadata(&home).map(|m| m.file_type().is_symlink());
        if !home.is_absolute() || symlink.unwrap_or(true) || !home.is_dir() {
            return Err(format!(
                "HOME must be an existing, non-symlink directory: {}",
                home.display()
            ));
        }
        let absolute = |name: &str| -> Res<Option<String>> {
            match std::env::var(name).ok().filter(|v| !v.is_empty()) {
                Some(v) if !v.starts_with('/') => {
                    Err(format!("{name} must be an absolute path: {v}"))
                }
                other => Ok(other),
            }
        };
        let raw_config_home = absolute("XDG_CONFIG_HOME")?;
        let raw_state_home = absolute("XDG_STATE_HOME")?;
        let config_base = resolve_lenient(
            &raw_config_home
                .as_deref()
                .map_or_else(|| home.join(".config"), PathBuf::from),
        );
        let state_base = resolve_lenient(
            &raw_state_home
                .as_deref()
                .map_or_else(|| home.join(".local/state"), PathBuf::from),
        );
        Ok(Self {
            home,
            raw_config_home,
            config_base,
            state_base,
        })
    }
}

fn environment(runtime_root: &Path) -> Environment {
    let mut environment = Environment::from_process();
    environment.runtime_root = Some(runtime_root.display().to_string());
    environment
}

/// The channel's owner config and state roots (`<config>/cortex`, `<state>/cortex`).
fn owner_roots(runtime_root: &Path) -> Res<(PathBuf, PathBuf)> {
    config::owner_roots(&environment(runtime_root))
        .ok_or_else(|| "HOME and the XDG roots must be absolute paths".to_string())
}

// --------------------------------------------------------- private storage

/// One user-owned, non-symlinked 0700 directory; created when `create`.
fn private_directory(path: &Path, create: bool) -> Res<PathBuf> {
    let info = match fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Err(format!(
                    "required directory does not exist: {}",
                    path.display()
                ));
            }
            // A concurrent creator may win; its folder gets the checks below.
            match DirBuilder::new().mode(0o700).create(path) {
                Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                    return Err(format!(
                        "cannot create private directory {}: {e}",
                        path.display()
                    ))
                }
                _ => {}
            }
            fs::symlink_metadata(path).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(format!("cannot inspect directory {}: {e}", path.display())),
    };
    if info.file_type().is_symlink() || !info.is_dir() {
        return Err(format!(
            "refusing symlinked or non-directory private path: {}",
            path.display()
        ));
    }
    if info.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "private directory is not owned by the current user: {}",
            path.display()
        ));
    }
    if info.mode() & 0o7777 != 0o700 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot secure private directory {}: {e}", path.display()))?;
    }
    let info = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if info.mode() & 0o7777 != 0o700 || info.file_type().is_symlink() {
        return Err(format!(
            "private directory is not owner-only: {}",
            path.display()
        ));
    }
    Ok(path.to_path_buf())
}
/// `private_directory` for a path whose missing ancestors are created 0700.
fn private_directory_all(path: &Path) -> Res<PathBuf> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
    }
    private_directory(path, true)
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

/// Exclusive same-directory temp file, fsync, then rename over `path`.
fn atomic_write(path: &Path, body: &[u8], mode: u32) -> Res<()> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", unique_suffix()));
    let write = || -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(body)?;
        file.sync_all()?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))?;
        fs::rename(&temporary, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&temporary);
        format!("cannot securely write {}: {e}", path.display())
    })
}

// ------------------------------------------------------------ launch record

fn desired_record(roots: &Roots, runtime: &Path, binary: &Path) -> Value {
    json!({
        "argv": [binary.display().to_string()],
        "env": {
            "HOME": roots.home.display().to_string(),
            "XDG_CONFIG_HOME": roots.config_base.display().to_string(),
            "XDG_STATE_HOME": roots.state_base.display().to_string(),
            "CORTEX_ACP_RUNTIME_ROOT": runtime.display().to_string(),
        },
        "shim": runtime.join(SHIM).display().to_string(),
    })
}
fn string_map(value: &Value) -> Option<BTreeMap<String, String>> {
    value
        .as_object()?
        .iter()
        .map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
        .collect()
}
/// The storage part of a record env: everything but the runtime root.
fn storage_env(desired: &Value) -> BTreeMap<String, String> {
    let mut env = string_map(&desired["env"]).unwrap_or_default();
    env.remove("CORTEX_ACP_RUNTIME_ROOT");
    env
}
fn record_shape(value: &Value) -> Option<(&Vec<Value>, &Value, &str)> {
    let object = value.as_object()?;
    if object.len() != 3 {
        return None;
    }
    Some((
        object.get("argv")?.as_array()?,
        object.get("env")?,
        object.get("shim")?.as_str()?,
    ))
}
/// A prior Cortex Python-entry record for these storage roots. It is
/// upgraded to the Rust form, never refused: `["python3","-B",<runtime>/src/cortex_acp.py]`.
fn is_managed_python_record(value: &Value, desired: &Value) -> bool {
    let Some((argv, env, shim)) = record_shape(value) else {
        return false;
    };
    if argv.len() != 3 || argv[0] != "python3" || argv[1] != "-B" {
        return false;
    }
    let Some(entry) = argv[2].as_str().map(Path::new) else {
        return false;
    };
    if string_map(env).as_ref() != Some(&storage_env(desired)) {
        return false;
    }
    let shim = Path::new(shim);
    let Some(runtime) = entry.parent().and_then(Path::parent) else {
        return false;
    };
    entry.is_absolute()
        && shim.is_absolute()
        && entry == runtime.join("src/cortex_acp.py")
        && shim == runtime.join(SHIM)
}
/// A prior Cortex Rust record for the same storage roots (any runtime root).
fn is_managed_rust_record(value: &Value, desired: &Value) -> bool {
    let Some((argv, env, shim)) = record_shape(value) else {
        return false;
    };
    let [Some(binary)] = [argv.first().and_then(Value::as_str)] else {
        return false;
    };
    if argv.len() != 1 {
        return false;
    }
    let Some(mut env) = string_map(env) else {
        return false;
    };
    let Some(runtime) = env.remove("CORTEX_ACP_RUNTIME_ROOT") else {
        return false;
    };
    if env != storage_env(desired) {
        return false;
    }
    let (binary, runtime, shim) = (Path::new(binary), Path::new(&runtime), Path::new(shim));
    binary.is_absolute()
        && runtime.is_absolute()
        && shim.is_absolute()
        && shim == runtime.join(SHIM)
}

/// Persist the Rust argv, the user storage roots and the runtime root.
/// Paseo daemon plugins do not inherit the shell environment that ran
/// setup; only non-secret absolute path settings are kept.
fn write_launch_config(
    roots: &Roots,
    runtime: &Path,
    binary: &Path,
    replace: bool,
    replace_managed: bool,
) -> Res<Value> {
    if !binary.is_absolute() || !is_executable(binary) {
        return Err("Rust binary must be an existing executable absolute path".into());
    }
    let runtime = resolve_lenient(runtime);
    let desired = desired_record(roots, &runtime, binary);
    // The record lives in the Cortex config root of the environment it
    // describes, the root the provider reads first in that environment.
    let conventional = roots.home.join(".config");
    let config_parent =
        if roots.raw_config_home.is_some() && roots.config_base != resolve_lenient(&conventional) {
            private_directory(&roots.config_base, true)?
        } else {
            private_directory(&conventional, true)?
        };
    let root = private_directory(&config_parent.join("cortex"), true)?;
    let path = root.join(LAUNCH_FILE);
    let summary = |created: bool| {
        json!({"path":path.display().to_string(),"created":created,
               "argv":desired["argv"],"env":desired["env"]})
    };
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            atomic_write(&path, pretty(&desired).as_bytes(), 0o600)?;
            return Ok(summary(true));
        }
        Err(e) => return Err(format!("cannot inspect {}: {e}", path.display())),
        Ok(info) => {
            if info.file_type().is_symlink() {
                return Err(format!(
                    "refusing symlinked Paseo launch configuration: {}",
                    path.display()
                ));
            }
            if !info.is_file() || info.mode() & 0o7777 != 0o600 {
                return Err(format!(
                    "Paseo launch configuration must be a regular 0600 file: {}",
                    path.display()
                ));
            }
        }
    }
    let current: Value = fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        .map_err(|e| format!("invalid existing Paseo launch configuration: {e}"))?;
    let managed = is_managed_python_record(&current, &desired)
        || (replace_managed && is_managed_rust_record(&current, &desired));
    if !replace && current != desired && !managed {
        return Err(format!(
            "refusing to overwrite unrelated launch configuration: {}",
            path.display()
        ));
    }
    if current != desired {
        atomic_write(&path, pretty(&desired).as_bytes(), 0o600)?;
    }
    Ok(summary(false))
}

// ---------------------------------------------------------- storage / status

/// Ensure the owner-private roots and install the absent default preset.
fn init_storage(runtime: &Path) -> Res<Value> {
    let (config_root, state_root) = owner_roots(runtime)?;
    private_directory_all(&config_root)?;
    private_directory_all(&state_root)?;
    let routing = api(
        runtime,
        json!({"surface":"default_preset","action":"apply_default_preset"}),
    )?;
    Ok(json!({
        "config_root": config_root.display().to_string(),
        "state_root": state_root.display().to_string(),
        "routing": routing,
    }))
}

/// One settings-API request: its result, or the refusal as text.
fn api(runtime: &Path, mut request: Value) -> Res<Value> {
    request["api_version"] = json!(1);
    let (stdout, _code) = config::execute(request.to_string().as_bytes(), &environment(runtime));
    let answer: Value = serde_json::from_str(&stdout)
        .map_err(|_| "the native configuration API did not answer".to_string())?;
    if answer["ok"] == true {
        return Ok(answer["result"].clone());
    }
    let error = &answer["error"];
    Err(format!(
        "{}: {}",
        error["code"].as_str().unwrap_or("configuration_error"),
        error["message"]
            .as_str()
            .unwrap_or("The request was refused.")
    ))
}
fn connections_revision(runtime: &Path) -> Res<String> {
    api(runtime, json!({"surface":"connections","action":"read"}))?["revision"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "the connections read returned no revision".into())
}

fn status() -> Res<Value> {
    let source = default_source_root();
    let (config_root, state_root) = owner_roots(&source)?;
    let mut out = Map::new();
    out.insert(
        "storage".into(),
        json!({"config_root":config_root.display().to_string(),
               "state_root":state_root.display().to_string()}),
    );
    for (key, request) in [
        (
            "connections",
            json!({"surface":"connections","action":"read"}),
        ),
        (
            "discovered_engines",
            json!({"surface":"connections","action":"discover_connections"}),
        ),
        (
            "user_defaults",
            json!({"surface":"user_defaults","action":"read"}),
        ),
    ] {
        match api(&source, request) {
            Ok(value) => {
                out.insert(key.into(), value);
            }
            Err(error) => {
                out.insert(format!("{key}_error"), json!(error));
            }
        }
    }
    Ok(Value::Object(out))
}

fn add_connection(source: &Path, argv: &[String]) -> Res<Value> {
    if argv.iter().any(|a| {
        matches!(
            a.as_str(),
            "--api-key-file" | "--api-key-stdin" | "--install-record"
        )
    }) {
        return Err("API-key connections and install records are not offered by the Rust setup; \
            install the pinned adapter with `install-engine` and register its native account with `add-connection --engine ENGINE`."
            .into());
    }
    let flags = Flags::parse(argv, &["--engine"], &[])?;
    api(
        source,
        json!({"surface":"connections","action":"onboard_connection",
               "engine":flags.required("--engine")?,"explicit_opt_in":true,
               "expected_revision":connections_revision(source)?}),
    )
}

/// Pin one catalog route as the user-default route (native catalog only: a
/// route exists once a real engine session observed it, see `refresh-catalog`).
fn pin_route(source: &Path, argv: &[String]) -> Res<Value> {
    let flags = Flags::parse(argv, &["--connection", "--model"], &[])?;
    let (connection, model) = (flags.required("--connection")?, flags.required("--model")?);
    let catalog = api(source, json!({"surface":"route_catalog","action":"read"}))?;
    let route = catalog["routes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["connection_id"] == connection && r["model"] == model)
        .ok_or_else(|| {
            format!("no catalog route for connection {connection:?} and model {model:?}; run refresh-catalog first")
        })?;
    let route_id = route["route_id"].clone();
    let read = api(source, json!({"surface":"user_defaults","action":"read"}))?;
    let mut defaults = read["defaults"].as_object().cloned().unwrap_or_default();
    defaults.remove("coordinator");
    defaults.insert("route_id".into(), route_id.clone());
    defaults.insert("route_connection_id".into(), json!(connection));
    defaults.insert("route_model".into(), json!(model));
    defaults.insert("parameters".into(), json!({}));
    let mut request = json!({"surface":"user_defaults","action":"update",
        "expected_revision":read["revision"],"defaults":defaults});
    if let Some(ceiling) = read.get("ceiling").filter(|c| c.is_object()) {
        request["ceiling"] = ceiling.clone();
    }
    api(source, request)?;
    Ok(json!({"route_id":route_id,"connection_id":connection,"model":model}))
}

// ------------------------------------------------------------------ bootstrap

fn read_version(source: &Path) -> Res<String> {
    let identity = fs::read(source.join("package-identity.json"))
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|e| e.to_string()))
        .map_err(|e| format!("invalid package identity for Paseo bootstrap: {e}"))?;
    match identity["version"].as_str() {
        Some(v) if !v.is_empty() && !v.contains(['/', '\\']) && !v.contains("..") => Ok(v.into()),
        _ => Err("package identity has an unsafe version for Paseo bootstrap.".into()),
    }
}

fn read_source(path: &Path) -> Res<Vec<u8>> {
    let info = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !info.is_file() || info.file_type().is_symlink() {
        return Err(format!(
            "payload file is not a regular file: {}",
            path.display()
        ));
    }
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}
/// Copy `source` to `target` below `runtime` through an atomic rename; an
/// identical existing file is left alone.
fn place(runtime: &Path, source: &Path, target: &Path, mode: u32) -> Res<()> {
    let body = read_source(source)?;
    if let Some(parent) = target.parent() {
        if !resolve_lenient(parent).starts_with(resolve_lenient(runtime)) {
            return Err(format!(
                "payload path escapes the runtime: {}",
                target.display()
            ));
        }
        if !parent.exists() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
    }
    if let Ok(existing) = fs::symlink_metadata(target) {
        if existing.is_file()
            && !existing.file_type().is_symlink()
            && fs::read(target).is_ok_and(|b| b == body)
            && existing.mode() & 0o7777 == mode
        {
            return Ok(());
        }
    }
    atomic_write(target, &body, mode)
}
fn stage(source: &Path, bin_dir: &Path, runtime: &Path) -> Res<()> {
    for relative in PAYLOAD_FILES {
        place(
            runtime,
            &source.join(relative),
            &runtime.join(relative),
            0o600,
        )?;
    }
    for binary in BINARIES {
        place(
            runtime,
            &bin_dir.join(binary),
            &runtime.join("bin").join(binary),
            0o700,
        )?;
    }
    Ok(())
}

/// Install the stable owner-private runtime for a staged Git plugin and
/// point the launch record at its Rust binary.
fn bootstrap(source: &Path, bin_dir: &Path) -> Res<Value> {
    for binary in BINARIES {
        if !is_executable(&bin_dir.join(binary)) {
            return Err(format!(
                "{} is not built in {}; build it with `cargo build --release --locked -p cortex-acp -p cortex-paseo-tools` first.",
                binary,
                bin_dir.display()
            ));
        }
    }
    let version = read_version(source)?;
    init_storage(source)?;
    let (_, state_root) = owner_roots(source)?;
    let parent = private_directory(&state_root.join("paseo-runtime"), true)?;
    let runtime = parent.join(&version);
    if runtime.exists() {
        private_directory(&runtime, false)?;
        // The same version pins the same non-binary payload; restore any
        // missing file, and always take the freshly built binaries.
        stage(source, bin_dir, &runtime)?;
    } else {
        let staging = parent.join(format!(".paseo-runtime-{}", unique_suffix()));
        DirBuilder::new()
            .mode(0o700)
            .create(&staging)
            .map_err(|e| format!("cannot stage owner-private Paseo runtime: {e}"))?;
        let moved = stage(source, bin_dir, &staging)
            .and_then(|()| fs::rename(&staging, &runtime).map_err(|e| e.to_string()));
        if let Err(e) = moved {
            let _ = fs::remove_dir_all(&staging);
            return Err(format!("cannot stage owner-private Paseo runtime: {e}"));
        }
    }
    let launch = write_launch_config(
        &Roots::from_process()?,
        &runtime,
        &runtime.join("bin/cortex-acp"),
        false,
        true,
    )?;
    Ok(json!({"runtime_root":runtime.display().to_string(),"launch_config":launch}))
}

// ------------------------------------------------------------ plugin install

/// Canonical plugin source hash: every file below `root` outside the
/// excluded directories (and `plugin-integrity.json`), in relative-path order,
/// each as len(path) | path | len(body) | body (lengths are 8 big-endian bytes).
pub fn plugin_payload_digest(root: &Path) -> std::io::Result<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_symlink() && path.is_dir() {
                continue;
            }
            if kind.is_dir() {
                if !EXCLUDED_DIRS.contains(&name.as_str()) {
                    walk(root, &path, out)?;
                }
            } else if name != "plugin-integrity.json" {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((relative, path));
            }
        }
        Ok(())
    }
    let mut files = vec![];
    walk(root, root, &mut files)?;
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut digest = Sha256::new();
    for (relative, path) in files {
        let body = fs::read(&path)?;
        digest.update((relative.len() as u64).to_be_bytes());
        digest.update(relative.as_bytes());
        digest.update((body.len() as u64).to_be_bytes());
        digest.update(&body);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn verify_plugin_source(source: &Path) -> Res<(PathBuf, Value, String)> {
    let root = source.join("integrations/paseo");
    let read = |name: &str| -> Res<Value> {
        fs::read(root.join(name))
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
            .map_err(|e| format!("invalid Paseo plugin source metadata: {e}"))
    };
    let manifest = read("paseo-plugin.json")?;
    let integrity = read("plugin-integrity.json")?;
    if manifest["id"] != "cortex-acp" {
        return Err("Paseo plugin manifest must declare id 'cortex-acp'.".into());
    }
    if !manifest["requirements"]["paseo"]
        .as_str()
        .is_some_and(|v| v.starts_with(">=0.9"))
    {
        return Err("Paseo plugin manifest must require Paseo >=0.9.0.".into());
    }
    let actual = plugin_payload_digest(&root).map_err(|e| e.to_string())?;
    if integrity["payload_sha256"] != actual.as_str() {
        return Err("Paseo plugin source hash does not match its integrity record.".into());
    }
    Ok((
        root.canonicalize().map_err(|e| e.to_string())?,
        manifest,
        actual,
    ))
}

fn resolve_paseo_cli() -> Res<String> {
    let candidates: Vec<String> = match std::env::var("CORTEX_PASEO_CLI")
        .ok()
        .filter(|v| !v.is_empty())
    {
        Some(c) => vec![c],
        None => PASEO_CLI_CANDIDATES.iter().map(|s| s.to_string()).collect(),
    };
    for candidate in candidates {
        let found = if candidate.contains('/') {
            Path::new(&candidate).is_file()
        } else {
            std::env::var_os("PATH").is_some_and(|p| {
                std::env::split_paths(&p).any(|d| is_executable(&d.join(&candidate)))
            })
        };
        if found {
            return Ok(candidate);
        }
    }
    Err("Paseo CLI was not found; set CORTEX_PASEO_CLI.".into())
}

fn invoke(cli: &str, args: &[&str], check: bool) -> Res<String> {
    let output = Command::new(cli)
        .args(args)
        .output()
        .map_err(|e| format!("Paseo command failed ({}): {e}", args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if check && !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(format!(
            "Paseo command failed ({}): {detail}",
            args.join(" ")
        ));
    }
    Ok(stdout)
}
fn json_output(text: &str, what: &str) -> Res<Value> {
    serde_json::from_str(text).map_err(|_| format!("{what} was not valid JSON."))
}

/// The exact old generic Cortex ACP provider registration, and only that.
fn is_owned_legacy_provider(entry: &Value, source: &Path) -> bool {
    if entry["extends"] != "acp" {
        return false;
    }
    let Some(command) = entry["command"].as_array().filter(|c| !c.is_empty()) else {
        return false;
    };
    let Some(parts) = command
        .iter()
        .map(|p| p.as_str().filter(|s| !s.is_empty()))
        .collect::<Option<Vec<&str>>>()
    else {
        return false;
    };
    let entrypoint = source.join("src/cortex_acp.py").display().to_string();
    if parts == ["python3", "-B", entrypoint.as_str()]
        || parts == ["/usr/bin/python3", "-B", entrypoint.as_str()]
    {
        return true;
    }
    if parts.len() != 1 {
        return false;
    }
    fs::canonicalize(parts[0])
        .ok()
        .filter(|p| p.is_file())
        .and_then(|p| fs::read_to_string(p).ok())
        .is_some_and(|body| body.contains(&entrypoint) && body.contains("cortex_acp.py"))
}

fn migrate_legacy_provider(config: &Value, config_path: &Path, source: &Path) -> Res<Value> {
    let legacy = &config["agents"]["providers"]["cortex"];
    if !is_owned_legacy_provider(legacy, source) {
        return Ok(json!({"migrated":false,"reason":"no_owned_legacy_provider"}));
    }
    let backup = config_path.with_file_name(format!(
        "{}.cortex-legacy-backup-{}",
        config_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.json"),
        unique_suffix()
    ));
    let migrate = || -> Res<()> {
        let original = fs::read(config_path).map_err(|e| e.to_string())?;
        atomic_write(&backup, &original, 0o600)?;
        let mut updated = config.clone();
        updated["agents"]["providers"]
            .as_object_mut()
            .map(|providers| providers.remove("cortex"));
        atomic_write(config_path, pretty(&updated).as_bytes(), 0o600)
    };
    migrate().map_err(|e| {
        let _ = fs::remove_file(&backup);
        format!("cannot migrate the legacy Paseo Cortex provider safely: {e}")
    })?;
    Ok(json!({"migrated":true,"removed_provider":"cortex","backup":backup.display().to_string()}))
}

fn install_paseo_plugin(source: &Path, binary: &Path) -> Res<Value> {
    let (root, manifest, payload_sha256) = verify_plugin_source(source)?;
    // A running provider process does not mean it can open an ACP session:
    // check the owner MCP registry the session will read before any mutation.
    api(source, json!({"surface":"mcp_servers","action":"read"}))
        .map_err(|e| format!("MCP configuration preflight failed: {e}"))?;
    let cli = resolve_paseo_cli()?;
    let daemon = json_output(
        &invoke(&cli, &["daemon", "status", "--json"], true)?,
        "Paseo daemon status",
    )?;
    if !matches!(
        daemon["localDaemon"].as_str(),
        Some("running" | "reachable")
    ) {
        return Err("local Paseo daemon is not reachable.".into());
    }
    let home = resolve_lenient(Path::new(daemon["home"].as_str().unwrap_or("")));
    let config_path = home.join("config.json");
    let info = fs::symlink_metadata(&config_path)
        .ok()
        .filter(|i| i.is_file() && !i.file_type().is_symlink())
        .ok_or_else(|| {
            format!(
                "refusing symlinked or non-regular Paseo daemon config: {}",
                config_path.display()
            )
        })?;
    if info.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "Paseo daemon config is not owned by the current user: {}",
            config_path.display()
        ));
    }
    let mode = info.mode() & 0o7777;
    if mode & 0o400 == 0 {
        return Err(format!(
            "Paseo daemon config is not readable by its owner: {}",
            config_path.display()
        ));
    }
    if mode & 0o077 != 0 {
        fs::set_permissions(&config_path, fs::Permissions::from_mode(mode & 0o700))
            .map_err(|e| e.to_string())?;
        let again = fs::symlink_metadata(&config_path).map_err(|e| e.to_string())?;
        if again.mode() & 0o077 != 0 {
            return Err(format!(
                "refusing overly-readable Paseo daemon config: {}",
                config_path.display()
            ));
        }
    }
    let config: Value = fs::read(&config_path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        .map_err(|e| format!("cannot read Paseo daemon config: {e}"))?;
    if !config.is_object() {
        return Err("Paseo daemon config is not a JSON object.".into());
    }
    if config["pluginsEnabled"] != true {
        let mut updated = config.clone();
        updated["pluginsEnabled"] = json!(true);
        atomic_write(&config_path, pretty(&updated).as_bytes(), 0o600)?;
        invoke(&cli, &["reload", "--json"], true)?;
    }
    // The plugin is evaluated as part of `plugin install`: persist the
    // daemon-safe launch command first so its first load can find it.
    let launch = write_launch_config(&Roots::from_process()?, source, binary, false, true)?;
    let installed = (|| -> Res<Value> {
        let id = manifest["id"].as_str().unwrap_or("cortex-acp");
        let plugins = json_output(
            &invoke(&cli, &["plugin", "ls", "--json"], true)?,
            "Paseo plugin list",
        )?;
        let existing = plugins
            .as_array()
            .and_then(|rows| rows.iter().find(|p| p["id"] == id))
            .cloned();
        match existing {
            None => {
                invoke(
                    &cli,
                    &["plugin", "install", &root.display().to_string()],
                    true,
                )?;
            }
            Some(existing) => {
                let bound = resolve_lenient(Path::new(existing["path"].as_str().unwrap_or("")));
                if bound != root {
                    return Err(
                        "Paseo plugin id 'cortex-acp' is bound to another source path.".into(),
                    );
                }
                if existing["status"] != "running" {
                    invoke(&cli, &["plugin", "enable", id], false)?;
                } else {
                    invoke(&cli, &["plugin", "reload", id], true)?;
                }
            }
        }
        let verified = json_output(
            &invoke(&cli, &["plugin", "ls", id, "--json"], true)?,
            "Paseo plugin verification",
        )?;
        let row = verified
            .as_array()
            .and_then(|r| r.first())
            .unwrap_or(&verified);
        if row["id"] != id || row["status"] != "running" {
            return Err("Cortex Paseo plugin did not reach running status.".into());
        }
        // Remove only our exact owned generic registration after the plugin
        // is verified; keep an owner-only backup.
        let migration = migrate_legacy_provider(&config, &config_path, source)?;
        if migration["migrated"] == true {
            invoke(&cli, &["reload", "--json"], true)?;
        }
        let routing = init_storage(source)?["routing"].clone();
        Ok(
            json!({"plugin_id":id,"source":root.display().to_string(),"status":"running",
            "payload_sha256":payload_sha256,"launch_config":launch,
            "legacy_provider_migration":migration,"routing":routing}),
        )
    })();
    if installed.is_err() && launch["created"] == true {
        // Only the record this attempt created, only if still ours.
        if let Some(path) = launch["path"].as_str() {
            let ours = fs::symlink_metadata(path).is_ok_and(|i| {
                i.is_file() && !i.file_type().is_symlink() && i.mode() & 0o7777 == 0o600
            });
            if ours {
                let _ = fs::remove_file(path);
            }
        }
    }
    installed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desired(runtime: &str) -> Value {
        json!({
            "argv": [format!("{runtime}/bin/cortex-acp")],
            "env": {"HOME":"/h","XDG_CONFIG_HOME":"/h/.config","XDG_STATE_HOME":"/h/.local/state",
                    "CORTEX_ACP_RUNTIME_ROOT": runtime},
            "shim": format!("{runtime}/{SHIM}"),
        })
    }

    #[test]
    fn python_records_upgrade_and_unrelated_ones_stay_protected() {
        let want = desired("/new");
        let python = json!({
            "argv": ["python3","-B","/old/src/cortex_acp.py"],
            "env": {"HOME":"/h","XDG_CONFIG_HOME":"/h/.config","XDG_STATE_HOME":"/h/.local/state"},
            "shim": format!("/old/{SHIM}"),
        });
        assert!(is_managed_python_record(&python, &want));
        let mut other_home = python.clone();
        other_home["env"]["HOME"] = json!("/someone-else");
        assert!(!is_managed_python_record(&other_home, &want));
        let mut stray = python.clone();
        stray["extra"] = json!(1);
        assert!(!is_managed_python_record(&stray, &want));
        assert!(!is_managed_python_record(&json!({"argv":["x"]}), &want));
        let rust_old = desired("/old");
        assert!(is_managed_rust_record(&rust_old, &want));
        assert!(!is_managed_python_record(&rust_old, &want));
        let mut moved_shim = rust_old.clone();
        moved_shim["shim"] = json!("/elsewhere/shim.mjs");
        assert!(!is_managed_rust_record(&moved_shim, &want));
    }

    #[test]
    fn plugin_digest_skips_integrity_record_and_dependency_dirs() {
        let dir = std::env::temp_dir().join(format!("cortex-digest-{}", unique_suffix()));
        fs::create_dir_all(dir.join("node_modules/x")).unwrap();
        fs::write(dir.join("a.ts"), "a").unwrap();
        let first = plugin_payload_digest(&dir).unwrap();
        fs::write(dir.join("plugin-integrity.json"), "{}").unwrap();
        fs::write(dir.join("node_modules/x/y.js"), "y").unwrap();
        assert_eq!(plugin_payload_digest(&dir).unwrap(), first);
        fs::write(dir.join("a.ts"), "b").unwrap();
        assert_ne!(plugin_payload_digest(&dir).unwrap(), first);
        fs::remove_dir_all(dir).unwrap();
    }
}
