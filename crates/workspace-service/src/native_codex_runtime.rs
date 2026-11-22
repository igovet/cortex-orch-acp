//! Native interpreter for private Codex launch artifacts; never a socket operation.
use cortex_engine::environment::{CODEX_CONFIG, CODEX_DISABLED_FEATURES, CODEX_MCP_SERVERS};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    ffi::{CString, OsString},
    fs,
    io::{self, Read},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

pub const MODE: &str = "--native-codex-runtime";
/// Codex config layers every Cortex-launched App Server carries (sessions, catalogue
/// observation and the plan-limits read): no subagents, plugins (whose marketplace
/// sync runs `git ls-remote` against the owner's configured sources, and whose MCP
/// servers, such as the owner's computer-use REPL or app tools, would start with
/// every thread), hooks, apps or native shell/image tools. Non-persistent `-c`
/// layers; nothing is written. A session thread's own `features` table replaces
/// these `features.*` layers, so codex-acp also carries the same features in
/// [`CODEX_CONFIG`] (see [`thread_features`]).
pub(crate) const BASE: &[&str] = &[
    "-c",
    "agents.enabled=false",
    "-c",
    "features.multi_agent=false",
    "-c",
    "features.multi_agent_v2=false",
    "-c",
    "features.plugins=false",
    "-c",
    "features.hooks=false",
    "-c",
    "features.apps=false",
    "-c",
    "features.shell_tool=false",
    "-c",
    "features.view_image=false",
    "-c",
    "tools.experimental_request_user_input.enabled=false",
];
const FAILURE: &str = "cortex codex native feature/MCP check failed";

/// Linux passes the artifact path as one argument to this native interpreter.
/// No shell or PATH lookup is used; child environments retain Python locale semantics.
pub fn artifact(service: &Path, binary: &Path) -> io::Result<String> {
    let service = service.to_str().ok_or_else(invalid)?;
    let binary = binary.to_str().ok_or_else(invalid)?;
    let header = format!("#!{service} {MODE}\n");
    // Linux binfmt_script's 256-byte buffer must contain the complete header.
    if !service.starts_with('/')
        || service.bytes().any(|b| b <= 32 || b >= 127)
        || header.len() > 255
        || !binary.starts_with('/')
        || binary.contains('\0')
    {
        return Err(invalid());
    }
    Ok(format!("{header}{}\n", serde_json::to_string(binary)?))
}
fn invalid() -> io::Error {
    io::Error::other("unqualified native Codex launch artifact")
}
fn binary(path: &Path) -> io::Result<OsString> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let m = file.metadata()?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != unsafe { libc::getuid() }
        || m.mode() & 0o777 != 0o700
        || m.len() > 16 * 1024
    {
        return Err(invalid());
    }
    let mut bytes = String::new();
    (&mut file).take(16 * 1024 + 1).read_to_string(&mut bytes)?;
    if bytes.len() > 16 * 1024 {
        return Err(invalid());
    }
    let (_, payload) = bytes.split_once('\n').ok_or_else(invalid)?;
    let binary: String = serde_json::from_str(payload).map_err(|_| invalid())?;
    if artifact(&std::env::current_exe()?, Path::new(&binary))? != bytes {
        return Err(invalid());
    }
    Ok(binary.into())
}
// CPython's default C-locale coercion is an observable launcher environment
// effect. Probe libc locale availability without changing process-global locale.
fn coerced_locale() -> Option<&'static str> {
    if std::env::var_os("LC_ALL").is_some_and(|v| !v.is_empty())
        || std::env::var_os("PYTHONCOERCECLOCALE").is_some_and(|v| v == "0")
    {
        return None;
    }
    fn available(name: &str) -> bool {
        let Ok(name) = CString::new(name) else {
            return false;
        };
        unsafe {
            let locale = libc::newlocale(libc::LC_CTYPE_MASK, name.as_ptr(), std::ptr::null_mut());
            if locale.is_null() {
                return false;
            }
            libc::freelocale(locale);
            true
        }
    }
    let requested = ["LC_CTYPE", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "C".into());
    if requested != "C" && requested != "POSIX" && available(&requested) {
        return None;
    }
    ["C.UTF-8", "C.utf8", "UTF-8"]
        .into_iter()
        .find(|name| available(name))
}
async fn checked(binary: &OsString, args: &[String]) -> io::Result<String> {
    // Python subprocess.run inherits the launcher environment/cwd and stdin,
    // captures both streams, restores SIGPIPE/SIGXFSZ, and kills/reaps on timeout.
    let mut c = Command::new(binary);
    c.env_remove(CODEX_MCP_SERVERS);
    if let Some(locale) = coerced_locale() {
        c.env("LC_CTYPE", locale);
    }
    c.args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    unsafe {
        c.pre_exec(|| {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            libc::signal(libc::SIGXFSZ, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut child = c.spawn()?;
    let mut stdout = child.stdout.take().ok_or_else(invalid)?;
    let mut stderr = child.stderr.take().ok_or_else(invalid)?;
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (_, _, status) = tokio::try_join!(
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err),
            child.wait()
        )?;
        if !status.success() {
            return Err(invalid());
        }
        String::from_utf8(out).map_err(|_| invalid())
    })
    .await;
    match result {
        Ok(Ok(out)) => Ok(out),
        other => {
            // Never retry a check or fall back to the Python artifact.
            let _ = child.kill().await;
            let _ = child.wait().await;
            other.map_err(|_| invalid())?
        }
    }
}
fn whitespace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}
fn disabled_features(text: &str) -> bool {
    // Python str.splitlines / str.split, including its four extra separators.
    let mut observed = std::collections::BTreeMap::new();
    for line in text.split([
        '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}',
        '\u{2029}',
    ]) {
        let parts: Vec<_> = line.split(whitespace).filter(|s| !s.is_empty()).collect();
        if parts.len() >= 3 {
            observed.insert(parts[0], *parts.last().unwrap());
        }
    }
    [
        "multi_agent",
        "multi_agent_v2",
        "plugins",
        "shell_tool",
        "view_image",
    ]
    .iter()
    .all(|name| observed.get(name) == Some(&"false"))
}
async fn servers(binary: &OsString, args: &[String]) -> io::Result<Vec<(String, bool)>> {
    let mut args = args.to_vec();
    args.extend(["mcp", "list", "--json"].map(str::to_owned));
    let text = checked(binary, &args).await?;
    let data: Value = serde_json::from_str(&text).map_err(|_| invalid())?;
    let mut names = BTreeSet::new();
    data.as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            let enabled = item
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or_else(invalid)?;
            if !server_name(name) || !names.insert(name.to_owned()) {
                return Err(invalid());
            }
            Ok((name.to_owned(), enabled))
        })
        .collect()
}
/// A name that is one bare TOML key and one Codex MCP server name.
fn server_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
/// One of the session's secret-free MCP relays, as the service handed it over in
/// [`CODEX_MCP_SERVERS`] (the ACP stdio entry shape: name, command, args, env).
struct Relay {
    name: String,
    layer: String,
}
/// The relays this App Server defines itself. Absent: none (catalogue observation,
/// or a launch without MCP). Anything malformed refuses the launch.
fn relays() -> io::Result<Vec<Relay>> {
    let Some(raw) = std::env::var_os(CODEX_MCP_SERVERS) else {
        return Ok(vec![]);
    };
    let raw = raw.into_string().map_err(|_| invalid())?;
    if raw.len() > 64 * 1024 {
        return Err(invalid());
    }
    let data: Value = serde_json::from_str(&raw).map_err(|_| invalid())?;
    let text = |v: &Value| {
        v.as_str()
            .filter(|s| !s.contains('\0'))
            .map(str::to_owned)
            .ok_or_else(invalid)
    };
    let mut names = BTreeSet::new();
    data.as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(|item| {
            let name = text(&item["name"])?;
            let command = text(&item["command"])?;
            if !server_name(&name) || !names.insert(name.clone()) || !command.starts_with('/') {
                return Err(invalid());
            }
            let args = item["args"]
                .as_array()
                .ok_or_else(invalid)?
                .iter()
                .map(|a| Ok(toml_string(&text(a)?)))
                .collect::<io::Result<Vec<_>>>()?;
            let env = item["env"]
                .as_array()
                .ok_or_else(invalid)?
                .iter()
                .map(|e| {
                    Ok(format!(
                        "{}={}",
                        toml_string(&text(&e["name"])?),
                        toml_string(&text(&e["value"])?)
                    ))
                })
                .collect::<io::Result<Vec<_>>>()?;
            // One inline table, so no field of it is left over from another layer of this name.
            let layer = format!(
                "mcp_servers.{name}={{command={},args=[{}],env={{{}}}}}",
                toml_string(&command),
                args.join(","),
                env.join(",")
            );
            Ok(Relay { name, layer })
        })
        .collect()
}
/// Every server the owner configured for Codex is turned off and, for a session,
/// exactly its own relays are defined, all as non-persistent `-c` layers checked
/// through `codex mcp list --json` before App Server starts.
///
/// Codex lets a thread's request config replace this whole `mcp_servers` layer
/// (a later `mcp_servers` table replaces earlier `mcp_servers.<name>` keys of the
/// same layer, then the owner's `config.toml` merges back in underneath). So the
/// relays never travel as ACP servers, which codex-acp would send as that table.
/// A relay that shares a name with an owner's server refuses the launch: the
/// owner's entry would merge its own fields (URL, environment, tool filters) into it.
async fn verify(binary: &OsString) -> io::Result<Vec<String>> {
    let base: Vec<String> = BASE.iter().map(|s| (*s).to_owned()).collect();
    let mut features = base.clone();
    features.extend(["features", "list"].map(str::to_owned));
    if !disabled_features(&checked(binary, &features).await?) {
        return Err(invalid());
    }
    let relays = relays()?;
    thread_features(!relays.is_empty())?;
    let relay = |name: &str| relays.iter().any(|r| r.name == name);
    let discovered = servers(binary, &base).await?;
    if discovered.iter().any(|(name, _)| relay(name)) {
        return Err(invalid());
    }
    let mut args = base;
    for (name, _) in discovered {
        args.extend(["-c".to_owned(), format!("mcp_servers.{name}.enabled=false")]);
    }
    for r in &relays {
        args.extend(["-c".to_owned(), r.layer.clone()]);
    }
    let listed = servers(binary, &args).await?;
    if listed
        .iter()
        .any(|(name, enabled)| *enabled && !relay(name))
        || relays.iter().any(|r| {
            !listed
                .iter()
                .any(|(name, enabled)| *enabled && *name == r.name)
        })
    {
        return Err(invalid());
    }
    args.extend(["-c".to_owned(), trust_override(&std::env::current_dir()?)?]);
    args.push("app-server".to_owned());
    Ok(args)
}
/// codex-acp (2.6.0) sends a `features` table in every session thread's config, and
/// App Server lets that table replace the whole `features` layer of its own `-c`
/// flags: with only the [`BASE`] layers, the owner's plugins (and their MCP servers)
/// are back on for the thread. codex-acp spreads its session config, read from
/// [`CODEX_CONFIG`] (inherited by this launcher), into that table. A session launch
/// (one that hands over relays) therefore refuses unless that config is exactly
/// [`CODEX_DISABLED_FEATURES`], each `false`; any other launch may omit it, but a
/// present one must have that exact shape.
fn thread_features(session: bool) -> io::Result<()> {
    let raw = match std::env::var_os(CODEX_CONFIG) {
        None if !session => return Ok(()),
        None => return Err(invalid()),
        Some(raw) => raw.into_string().map_err(|_| invalid())?,
    };
    if raw.len() > 8192 {
        return Err(invalid());
    }
    let data: Value = serde_json::from_str(&raw).map_err(|_| invalid())?;
    let object = data.as_object().ok_or_else(invalid)?;
    let features = object
        .get("features")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    let instruction_file = object.get("model_instructions_file");
    if instruction_file.is_some_and(|v| {
        !v.as_str().is_some_and(|s| {
            let path = Path::new(s);
            path.is_absolute()
                && path
                    .file_name()
                    .is_some_and(|n| n == "model-instructions.md")
        })
    }) || object.len() != 1 + usize::from(instruction_file.is_some())
        || features.len() != CODEX_DISABLED_FEATURES.len()
        || !CODEX_DISABLED_FEATURES
            .iter()
            .all(|name| features.get(*name) == Some(&Value::Bool(false)))
    {
        return Err(invalid());
    }
    Ok(())
}
/// A TOML basic string. TOML forbids U+0000..U+001F (tab aside) and U+007F raw.
fn toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c < ' ' || c == '\u{7f}' => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
/// A thread that starts without a request trust override (codex-acp's
/// ephemeral title thread) in an untrusted git project under a writable
/// sandbox makes App Server persist `[projects."<cwd>"] trust_level =
/// "trusted"` into `$CODEX_HOME/config.toml`, the owner's own file for a
/// native-account route. Python's engine jail mounts that file read-only, so
/// the write fails there; Rust runs no engine jail. This launcher therefore
/// grants its App Server process, as a non-persistent `-c` layer, exactly the
/// trust codex-acp already sends with every session thread for the same root
/// (App Server's cwd). Codex keeps quote characters literally in a `-c` key
/// path and splits at its first `=`, so only the inline-table form names an
/// arbitrary path.
pub fn trust_override(cwd: &Path) -> io::Result<String> {
    let cwd = cwd
        .to_str()
        .filter(|s| s.starts_with('/'))
        .ok_or_else(invalid)?;
    let key = toml_string(cwd);
    Ok(format!("projects={{{key}={{trust_level=\"trusted\"}}}}"))
}
/// The artifact names the writing process's own executable as interpreter, so
/// every binary that can write it (`workspace-service`, `cortex-acp`,
/// `cortex-paseo-tools`) calls this first in `main`, before any async runtime.
/// Exits the process in runtime mode; otherwise returns without effects.
pub fn dispatch() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new(MODE)) {
        std::process::exit(run(args.collect()));
    }
}
/// Called before constructing the service runtime. The final peer replaces us,
/// preserving PID, stdio, cwd, environment (less the relay hand-over variable),
/// status and ordinary signal delivery.
pub fn run(args: Vec<OsString>) -> i32 {
    if args.len() != 2 || args[1] != "app-server" {
        return 64;
    }
    let result = (|| {
        let binary = binary(Path::new(&args[0]))?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let argv = rt.block_on(verify(&binary))?;
        drop(rt);
        Ok::<_, io::Error>((binary, argv))
    })();
    let Ok((binary, argv)) = result else {
        eprintln!("{FAILURE}");
        return 65;
    };
    let mut command = std::process::Command::new(binary);
    // The relays are now App Server config layers; no model-run command inherits them.
    command.args(argv).env_remove(CODEX_MCP_SERVERS);
    if let Some(locale) = coerced_locale() {
        command.env("LC_CTYPE", locale);
    }
    // Python os.execv (unlike subprocess.run) preserves Python's ignored signals.
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            Ok(())
        });
    }
    let _ = command.exec();
    1
}
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod tool_inventory;

#[cfg(test)]
mod tests {
    use super::trust_override;
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};

    /// The App Server `-c` feature layers and codex-acp's thread feature config
    /// name the same features, all off.
    #[test]
    fn base_layers_and_thread_config_disable_the_same_features() {
        let layered: Vec<&str> = super::BASE
            .iter()
            .filter_map(|a| a.strip_prefix("features.")?.strip_suffix("=false"))
            .collect();
        assert_eq!(layered, cortex_engine::environment::CODEX_DISABLED_FEATURES);
        let config: serde_json::Value =
            serde_json::from_str(&cortex_engine::environment::codex_thread_config()).unwrap();
        assert_eq!(
            config,
            serde_json::json!({"features":{"multi_agent":false,"multi_agent_v2":false,"plugins":false,
                "hooks":false,"apps":false,"shell_tool":false,"view_image":false}})
        );
    }

    #[test]
    fn trust_override_is_one_inline_table_with_an_escaped_path_key() {
        assert_eq!(
            trust_override(Path::new("/p/dots.co/with=eq/space s/юникод")).unwrap(),
            r#"projects={"/p/dots.co/with=eq/space s/юникод"={trust_level="trusted"}}"#
        );
        assert_eq!(
            trust_override(Path::new("/q\"b\\t\u{1}\u{7f}\n")).unwrap(),
            r#"projects={"/q\"b\\t\u0001\u007F\u000A"={trust_level="trusted"}}"#
        );
        assert!(trust_override(Path::new("relative")).is_err());
        assert!(trust_override(Path::new(OsStr::from_bytes(b"/\xff"))).is_err());
    }
}
