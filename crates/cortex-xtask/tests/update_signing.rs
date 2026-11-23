#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("cw-signing-{}-{now}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn command(root: &Path, key: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cortex-xtask"));
    cmd.arg("--root")
        .arg(root)
        .env("CORTEX_UPDATE_SIGNING_KEY", key);
    cmd
}

fn no_secret(output: &Output, secret: &[u8]) {
    let hex: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    for stream in [&output.stdout, &output.stderr] {
        for bytes in [secret, hex.as_bytes()] {
            assert!(
                !stream.windows(bytes.len()).any(|window| window == bytes),
                "A command printed private signing material."
            );
        }
    }
}

#[test]
fn sign_and_verify_real_deb_without_logging_the_private_key() {
    let tmp = Temp::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let key = tmp.0.join("signing/release.key");
    let public = tmp.0.join("release.pub");
    let init = command(&root, &key)
        .args(["update-key", "init"])
        .output()
        .unwrap();
    assert!(init.status.success());
    let secret = fs::read(&key).unwrap();
    assert_eq!(secret.len(), 32);
    no_secret(&init, &secret);
    assert!(init.stderr.is_empty());
    assert_eq!(init.stdout.len(), 65);
    assert_eq!(
        fs::metadata(key.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::write(&public, init.stdout).unwrap();
    let duplicate = command(&root, &key)
        .args(["update-key", "init"])
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    no_secret(&duplicate, &secret);
    assert!(
        fs::read(&key).unwrap() == secret,
        "An existing private key changed."
    );

    let package = tmp.0.join("package");
    fs::create_dir_all(package.join("DEBIAN")).unwrap();
    fs::write(package.join("DEBIAN/control"), "Package: cortex-workspace\nVersion: 1.0.7\nArchitecture: all\nMaintainer: Fixture <fixture@example.invalid>\nDescription: disposable update signing fixture\n").unwrap();
    let deb = tmp.0.join("cortex-workspace_1.0.7_all.deb");
    let build = Command::new("/usr/bin/dpkg-deb")
        .arg("--build")
        .arg(package)
        .arg(&deb)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "Could not build a disposable Debian fixture."
    );

    let cli = |verb| {
        command(&root, &key)
            .arg(verb)
            .arg(&deb)
            .arg("--public-key")
            .arg(&public)
            .output()
            .unwrap()
    };
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
    let readable = cli("update-sign");
    assert!(!readable.status.success());
    no_secret(&readable, &secret);
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    let sign = cli("update-sign");
    assert!(sign.status.success());
    no_secret(&sign, &secret);
    let verify = cli("update-verify");
    assert!(verify.status.success());
    no_secret(&verify, &secret);

    let sig = deb.with_extension("deb.sha256.sig");
    let mut bytes = fs::read(&sig).unwrap();
    assert_eq!(bytes.len(), 64);
    bytes[0] ^= 1;
    fs::write(sig, bytes).unwrap();
    let corrupt = cli("update-verify");
    assert!(!corrupt.status.success());
    assert_eq!(corrupt.stderr, b"Desktop update verification failed.\n");
    no_secret(&corrupt, &secret);

    // The explicit root simulates a checkout without writing a private key there.
    let checkout = tmp.0.join("checkout");
    fs::create_dir(&checkout).unwrap();
    let forbidden = checkout.join("private/release.key");
    let refusal = command(&checkout, &forbidden)
        .args(["update-key", "init"])
        .output()
        .unwrap();
    assert!(!refusal.status.success());
    assert!(!forbidden.parent().unwrap().exists());
    no_secret(&refusal, &secret);
}
