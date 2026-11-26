//! The per-command supervisor's preparation report, through the real binary entry.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};

/// A preparation the sandbox refuses (a private-named link back to the project root)
/// tells the service why, with numeric stage timings and nothing else, and never
/// asks to launch.
#[test]
fn a_refused_preparation_reports_its_cause_and_stage_timings() {
    let base = std::env::temp_dir().join(format!(
        "command-supervisor-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let project = base.join("project");
    std::fs::create_dir_all(project.join("nested")).unwrap();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
    // A control path that cannot be resolved refuses preparation.
    let missing = base.join("missing-codex-home");
    let spec = json!({"root":project,"cwd":project,"argv":["/bin/sh","-c","echo ran > ran"],
        "profile":"guarded","writes":[project],"protected":[],"output":base.join("out"),
        "status":base.join("status"),"stop":base.join("stop"),"reference":"native-command:fixture",
        "timeout":5});
    let mut child = Command::new(env!("CARGO_BIN_EXE_workspace-service"))
        .arg("--native-command-supervisor")
        .env("CODEX_HOME", &missing)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{spec}").unwrap();
    // Closing the pipe means a wrongly accepted preparation fails fast instead of waiting for "go".
    drop(input);
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let status = child.wait().unwrap();
    let report: Value = serde_json::from_str(&line).unwrap();
    assert!(!status.success());
    assert_eq!(
        report["failed"], "sandbox_unavailable:invalid_control_path",
        "{report}"
    );
    let timings = report["timings"].as_object().unwrap();
    assert!(timings.contains_key("preparation_ms"), "{report}");
    assert!(timings.values().all(Value::is_u64), "{report}");
    assert!(!project.join("ran").exists());
    assert!(!base.join("status").exists());
    std::fs::remove_dir_all(&base).unwrap();
}
