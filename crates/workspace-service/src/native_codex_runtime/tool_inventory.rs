//! Offline Codex CLI prompt renderer, not an ACP/App Server session.
use super::{trust_override, BASE};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    ffi::CStr,
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

const SHELL_TOOLS: [&str; 2] = ["exec_command", "write_stdin"];

fn array(value: &Value) -> Result<&[Value], &'static str> {
    value
        .as_array()
        .filter(|a| !a.is_empty())
        .map(Vec::as_slice)
        .ok_or("expected nonempty array")
}

fn name(value: &Value) -> Result<&str, &'static str> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .ok_or("invalid declaration name")
}

fn declarations(bytes: &[u8]) -> Result<BTreeSet<String>, &'static str> {
    let root: Value = serde_json::from_slice(bytes).map_err(|_| "invalid prompt JSON")?;
    let [inventory, messages @ ..] = array(&root)? else {
        return Err("missing prompt records");
    };
    if inventory["type"] != "additional_tools"
        || inventory["role"] != "developer"
        || messages.is_empty()
        || !messages
            .iter()
            .all(|m| m["type"] == "message" && m["content"].is_array())
    {
        return Err("unsupported prompt record layout");
    }
    let mut namespaces = BTreeSet::new();
    let mut tools = BTreeSet::new();
    let mut nested = BTreeSet::new();
    for group in array(&inventory["tools"])? {
        let ns = name(&group["name"])?;
        if group["type"] != "namespace" || !namespaces.insert(ns) {
            return Err("unsupported or duplicated namespace");
        }
        for tool in array(&group["tools"])? {
            let tool_name = name(&tool["name"])?;
            let description = tool["description"].as_str().ok_or("missing description")?;
            if !tools.insert(format!("{ns}.{tool_name}")) {
                return Err("duplicated tool");
            }
            if ns == "functions" && tool_name == "exec" && tool["type"] == "custom" {
                // Code Mode advertises nested declarations in the exec description.
                // References in usage examples are not declarations.
                for section in description.split("\n### ").skip(1) {
                    let (heading, body) = section.split_once('\n').ok_or("incomplete heading")?;
                    let label = heading
                        .strip_prefix('`')
                        .and_then(|s| s.strip_suffix('`'))
                        .ok_or("invalid declaration heading")?;
                    let signature = format!("declare const tools: {{ {label}(");
                    let mut blocks = body.split("```");
                    if label.is_empty()
                        || blocks.next().is_none()
                        || !blocks.next().is_some_and(|code| {
                            code.lines().any(|line| line.starts_with(&signature))
                        })
                        || blocks.next().is_none()
                        || blocks.next().is_some()
                        || !nested.insert(format!("functions.exec.{label}"))
                    {
                        return Err("incomplete or duplicated nested declaration");
                    }
                }
            } else if tool["type"] != "function" || !tool["parameters"].is_object() {
                return Err("unsupported tool definition");
            }
        }
    }
    for required in ["functions.exec", "functions.wait", "clock.sleep"] {
        if !tools.contains(required) {
            return Err("missing baseline tool");
        }
    }
    if !nested.contains("functions.exec.apply_patch") {
        return Err("missing baseline nested declaration");
    }
    tools.extend(nested);
    Ok(tools)
}

#[test]
fn codex_inventory_rejects_incomplete_evidence() {
    for invalid in [
        "",
        "[]",
        r#"[{"type":"additional_tools","role":"developer","tools":[]},{"type":"message","content":[]}]"#,
        r#"[{"type":"additional_tools","role":"developer","tools":[{"type":"namespace","name":"functions","tools":[]}]}]"#,
        r#"[{"type":"additional_tools","role":"developer","tools":["#,
    ] {
        assert!(declarations(invalid.as_bytes()).is_err());
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        // mkdtemp creates the parent atomically with mode 0700.
        let mut template = *b"/tmp/cortex-codex-inventory-XXXXXX\0";
        let path = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
        assert!(!path.is_null(), "mkdtemp: {}", io::Error::last_os_error());
        Self(PathBuf::from(
            unsafe { CStr::from_ptr(path) }.to_str().unwrap(),
        ))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove private Codex scratch");
    }
}

// Native Linux x86-64 only. No inherited sockets (all stdio is null or a pipe).
// Reject other syscall ABIs, new sockets, connect, and io_uring before exec.
// This needs no network namespace or privileged setup.
fn block_network() -> io::Result<()> {
    let op = |code, jt, jf, k| libc::sock_filter { code, jt, jf, k };
    let mut filter = [
        op(0x20, 0, 0, 4),           // seccomp_data.arch
        op(0x15, 1, 0, 0xc000_003e), // AUDIT_ARCH_X86_64
        op(0x06, 0, 0, libc::SECCOMP_RET_KILL_PROCESS),
        op(0x20, 0, 0, 0),           // seccomp_data.nr
        op(0x35, 3, 0, 0x4000_0000), // reject x32 syscall numbers
        op(0x15, 2, 0, libc::SYS_socket as u32),
        op(0x15, 1, 0, libc::SYS_connect as u32),
        op(0x15, 0, 1, libc::SYS_io_uring_setup as u32),
        op(0x06, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        op(0x06, 0, 0, libc::SECCOMP_RET_ALLOW),
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    // SAFETY: the BPF program lives across the synchronous prctl call.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
            || libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

async fn offline_cli(binary: &Path, args: &[&str]) -> Vec<u8> {
    let scratch = Scratch::new();
    let mut cmd = Command::new(binary);
    cmd.env_clear().env("PATH", "/usr/bin:/bin").args(args);
    for (key, leaf) in [
        ("HOME", "home"),
        ("CODEX_HOME", "codex"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_RUNTIME_DIR", "run"),
        ("TMPDIR", "tmp"),
    ] {
        let dir = scratch.0.join(leaf);
        fs::create_dir(&dir).unwrap();
        cmd.env(key, dir);
    }
    let project = scratch.0.join("project");
    fs::create_dir(&project).unwrap();
    cmd.current_dir(&project);
    if args != ["--version"] {
        cmd.args([
            "-c",
            &trust_override(&project).unwrap(),
            "debug",
            "prompt-input",
        ]);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // SAFETY: the callback makes only async-signal-safe system calls before exec.
    unsafe { cmd.pre_exec(block_network) };
    let mut child = cmd
        .spawn()
        .expect("start Codex with mandatory seccomp filter");
    let mut stdout = child.stdout.take().unwrap().take(1024 * 1024 + 1);
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(15), async {
        stdout.read_to_end(&mut bytes).await.unwrap();
        assert!(
            bytes.len() <= 1024 * 1024,
            "Codex prompt output exceeded limit"
        );
        assert!(child.wait().await.unwrap().success(), "Codex CLI failed");
    })
    .await
    .expect("Codex CLI timed out");
    bytes
}

#[tokio::test]
#[ignore = "requires a trusted Codex CLI 0.166.0 binary and Linux x86-64 seccomp"]
async fn codex_launcher_offline_tool_inventory() {
    let binary = PathBuf::from(
        std::env::var_os("CORTEX_TEST_CODEX_CLI").expect("set CORTEX_TEST_CODEX_CLI"),
    );
    assert!(binary.is_absolute(), "use an absolute Codex binary path");
    let version = offline_cli(&binary, &["--version"]).await;
    assert_eq!(
        String::from_utf8_lossy(&version).trim(),
        "codex-cli 0.166.0",
        "unsupported Codex CLI version"
    );

    let baseline = declarations(&offline_cli(&binary, BASE).await).expect("launcher inventory");
    for name in SHELL_TOOLS {
        assert!(
            !baseline
                .iter()
                .any(|id| id.rsplit('.').next() == Some(name)),
            "{name} is visible with launcher policy"
        );
    }

    // Append a single last-wins override only for this second, fresh scratch run.
    let control_args = [BASE, &["-c", "features.shell_tool=true"]].concat();
    let control = declarations(&offline_cli(&binary, &control_args).await)
        .expect("positive-control inventory");
    let mut expected = baseline;
    expected.extend(SHELL_TOOLS.map(|name| format!("functions.exec.{name}")));
    assert_eq!(
        control, expected,
        "unexpected positive-control inventory delta"
    );
    eprintln!(
        "Codex CLI 0.166.0: both shell declarations absent under launcher policy and present in the isolated control"
    );
}
