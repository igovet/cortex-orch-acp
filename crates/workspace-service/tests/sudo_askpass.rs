//! The `sudo` shim and askpass helper modes of the real binary, against a fake `sudo`.
//! No real sudo runs and no real password exists.
use std::{
    ffi::OsStr,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sudo-askpass-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
fn script(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
fn shim(base: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_workspace-service"))
        .arg("--sudo-shim")
        .args(args)
        .env_clear()
        .env(
            "PATH",
            std::env::join_paths([base.join("shim"), base.join("fake")]).unwrap(),
        )
        .env("CORTEX_SUDO_SHIM_DIR", base.join("shim"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// The shim skips its own directory, runs the next `sudo` on PATH and puts `-A` first,
/// unless sudo is told not to prompt (`-n`) or already reads a password its own way.
#[test]
fn the_shim_runs_the_next_sudo_with_askpass() {
    let base = scratch("shim");
    for dir in ["shim", "fake"] {
        std::fs::create_dir(base.join(dir)).unwrap();
    }
    script(
        &base.join("shim/sudo"),
        "#!/bin/sh\necho shim-recursed\nexit 9\n",
    );
    script(&base.join("fake/sudo"), "#!/bin/sh\nprintf '%s|' \"$@\"\n");
    assert_eq!(
        shim(&base, &["apt-get", "install", "make"]),
        "-A|apt-get|install|make|"
    );
    assert_eq!(shim(&base, &["-u", "root", "id"]), "-A|-u|root|id|");
    assert_eq!(shim(&base, &["-n", "true"]), "-n|true|");
    assert_eq!(shim(&base, &["-S", "true"]), "-S|true|");
    std::fs::remove_dir_all(&base).unwrap();
}

/// Without its private route, or when the other end is not the service, the helper
/// prints nothing and fails, so sudo fails as it would without a password.
#[test]
fn the_helper_fails_closed_without_its_route() {
    let run = |env: &[(&str, &std::ffi::OsStr)]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workspace-service"));
        command
            .arg("--sudo-askpass")
            .arg("[sudo] password: ")
            .env_clear();
        for (k, v) in env {
            command.env(k, v);
        }
        command.output().unwrap()
    };
    let out = run(&[]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    // Unix socket addresses have a short length limit, independent of how
    // deeply the runner nests TMPDIR. mkdtemp creates the directory privately.
    let mut template = *b"/tmp/cortex-askpass-XXXXXX\0";
    // SAFETY: a writable, NUL-terminated template with six trailing Xs.
    assert!(!unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null());
    let base = Path::new(OsStr::from_bytes(&template[..template.len() - 1]));
    let socket = base.join("socket");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        use std::io::Write;
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"X").unwrap();
    });
    let out = run(&[
        ("CORTEX_ASKPASS_SOCKET", socket.as_os_str()),
        ("CORTEX_ASKPASS_TOKEN", std::ffi::OsStr::new("00")),
    ]);
    server.join().unwrap();
    std::fs::remove_dir_all(base).unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
}
