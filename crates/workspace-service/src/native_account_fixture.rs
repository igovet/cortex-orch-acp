//! Manual, offline guest fixture. No launch choice in this module is compiled into the service.
use cortex_engine::{
    engine::{Engine, PermissionDecision, PermissionPolicy},
    FutureResult,
};
use serde_json::{json, Value};
use std::{
    ffi::{CString, OsStr, OsString},
    fs::{self, OpenOptions},
    io,
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

mod codex_refresh;

const HOME: &str = "/cortex-fixture/home";
const MARKER: &str = "/cortex-fixture/guest";
const ACP_TIMEOUT: Duration = Duration::from_secs(12);
const OPEN_CODE: &str = "/usr/bin/opencode";

pub(crate) fn guest() -> io::Result<()> {
    let meta = fs::symlink_metadata(MARKER)?;
    if meta.is_file()
        && meta.uid() == 0
        && meta.mode() & 0o7777 == 0o444
        && unsafe { libc::geteuid() } == 1000
        && fs::read(MARKER)? == b"offline cortex native account fixture\n"
        && std::env::var_os("HOME").as_deref() == Some(OsStr::new(HOME))
        && Path::new(HOME).canonicalize()? == Path::new(HOME)
        && fs::read_dir("/sys/class/net")?.count() == 1
        && Path::new("/sys/class/net/lo").exists()
    {
        Ok(())
    } else {
        Err(io::Error::other("requires the disposable offline guest"))
    }
}

fn profile(role: &str, home: &Path) -> io::Result<Option<String>> {
    if home != Path::new(HOME) {
        return Ok(None);
    }
    guest()?;
    let engine = std::env::var("CORTEX_FIXTURE_ENGINE").unwrap_or_default();
    if !matches!(engine.as_str(), "opencode" | "codex" | "claude")
        || !matches!(role, "engine" | "catalogue" | "usage")
        || (engine == "opencode" && role == "usage")
    {
        return Err(io::Error::other("invalid fixture route"));
    }
    Ok(Some(format!("cortex-fixture-{engine}-{role}")))
}

pub(crate) fn prefix(role: &str, home: &Path) -> io::Result<Vec<OsString>> {
    Ok(match profile(role, home)? {
        Some(profile) => ["/usr/bin/aa-exec", "-p", &profile, "--"]
            .map(OsString::from)
            .into(),
        None => vec![],
    })
}

// The std Command (and the Tokio wrapper) already exists. Retain its exact argv,
// environment, session/process-group setup, and stdio. Only its exec label changes.
pub(crate) fn on_exec(command: &mut Command, role: &str, home: &Path) -> io::Result<()> {
    let Some(profile) = profile(role, home)? else {
        return Ok(());
    };
    let change = CString::new(format!("exec {profile}")).unwrap();
    unsafe {
        command.pre_exec(move || {
            let fd = libc::open(
                c"/proc/self/attr/apparmor/exec".as_ptr(),
                libc::O_WRONLY | libc::O_CLOEXEC,
            );
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let bytes = change.as_bytes();
            let result = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
            let error = (result < 0).then(io::Error::last_os_error);
            libc::close(fd);
            if let Some(error) = error {
                return Err(error);
            }
            if result as usize != bytes.len() {
                return Err(io::Error::from(io::ErrorKind::WriteZero));
            }
            Ok(())
        });
    }
    Ok(())
}

#[test]
fn ordinary_home_does_not_enable_candidate() {
    let home = Path::new("/not-the-fixture-home");
    for role in ["engine", "catalogue", "usage"] {
        assert!(prefix(role, home).unwrap().is_empty());
    }
}

struct NoPrompts;
impl PermissionPolicy for NoPrompts {
    fn authority(&self) -> FutureResult<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn decide<'a>(
        &'a self,
        _: &'a str,
        _: &'a Value,
        _: u64,
        _: &'a Value,
    ) -> FutureResult<'a, PermissionDecision> {
        Box::pin(async { Ok(PermissionDecision::Cancelled) })
    }
}

fn home() -> PathBuf {
    guest().expect("use scripts/test-native-account-service.sh, never real HOME");
    PathBuf::from(HOME)
}

fn database(home: &Path) -> PathBuf {
    home.join(".local/share/opencode/opencode.db")
}

fn label(pid: u32, role: &str, home: &Path) {
    let label = fs::read_to_string(format!("/proc/{pid}/attr/apparmor/current")).unwrap();
    let expected = profile(role, home).unwrap().unwrap();
    assert_eq!(label.trim(), format!("{expected} (enforce)"));
}

fn environment(home: &Path) -> crate::native_routes::RouteEnvironment {
    crate::native_routes::RouteEnvironment {
        config: home.join(".config"),
        state: home.join(".local/state"),
        home: home.to_owned(),
        routing_override: None,
    }
}

fn opencode_record(home: &Path) -> Value {
    json!({
        "id": "cn_000000000001",
        "engine": "opencode",
        "launch_command": [OPEN_CODE, "acp"],
        "auth_mode": "native_account",
        "credential_ref": {
            "kind": "engine_native_dir", "owned": false, "path": database(home).parent().unwrap(),
        },
    })
}

async fn opencode(home: &Path, require_session: bool) -> Engine {
    let cwd = home.join("checkout");
    let mut launch = crate::native_routes::fixture_session_launch(
        environment(home),
        &cwd,
        opencode_record(home),
    )
    .unwrap();
    let mut prefix = prefix("engine", home).unwrap().into_iter();
    let executable = std::mem::replace(&mut launch.executable, prefix.next().unwrap().into());
    launch.args = prefix
        .map(|arg| arg.into_string().unwrap())
        .chain([executable.into_os_string().into_string().unwrap()])
        .chain(launch.args)
        .collect();
    let engine = Engine::launch(launch, Arc::new(NoPrompts)).await.unwrap();
    let initialized = engine.initialize(ACP_TIMEOUT).await.unwrap_or_else(|e| {
        panic!(
            "OpenCode initialize: {e:?}; {:?}",
            engine.diagnostics().snapshot()
        )
    });
    let version = std::env::var("CORTEX_FIXTURE_OPENCODE_VERSION").unwrap();
    assert!(matches!(version.as_str(), "2.2.0" | "2.2.6"));
    assert_eq!(initialized["agentInfo"]["version"], version);
    label(engine.pid(), "engine", home);
    if require_session {
        engine
            .start_session(&cwd, Vec::new(), ACP_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "OpenCode session/new: {e:?}; {:?}",
                    engine.diagnostics().snapshot()
                )
            });
    }
    engine
}

#[tokio::test]
#[ignore = "requires a NIC-less synthetic QEMU guest and the pinned binary"]
async fn guest_opencode_engine() {
    opencode_lifecycle(true).await;
    println!("CORTEX_OPENCODE_ENGINE_BOOTSTRAP_WAL_REOPEN_PASS");
}

#[tokio::test]
#[ignore = "requires a NIC-less synthetic QEMU guest and the pinned binary"]
async fn guest_opencode_bootstrap() {
    opencode_lifecycle(false).await;
    println!("CORTEX_OPENCODE_BOOTSTRAP_ONLY_PASS");
}

async fn opencode_lifecycle(require_session: bool) {
    let home = home();
    let db = database(&home);
    assert!(
        !db.exists(),
        "bootstrap requires a genuinely absent database"
    );
    let first = opencode(&home, require_session).await;
    let meta = fs::metadata(&db).expect("native OpenCode must bootstrap its database");
    let reader =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .unwrap();
    let journal: String = reader
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
    reader
        .execute_batch(
            "CREATE TABLE fixture_evidence (n INTEGER); INSERT INTO fixture_evidence VALUES(1)",
        )
        .unwrap();
    let wal = db.with_extension("db-wal");
    let shm = db.with_extension("db-shm");
    let identities = [&wal, &shm].map(|p| fs::metadata(p).unwrap().ino());
    let second = opencode(&home, require_session).await;
    assert_eq!(fs::metadata(&db).unwrap().ino(), meta.ino());
    assert_eq!(
        [&wal, &shm].map(|p| fs::metadata(p).unwrap().ino()),
        identities
    );
    assert_eq!(
        unsafe { libc::kill(-(first.pid() as i32), libc::SIGKILL) },
        0
    );
    tokio::time::timeout(ACP_TIMEOUT, first.exited())
        .await
        .unwrap();
    drop(first);
    second.shutdown().await.unwrap();
    assert_eq!(
        reader
            .query_row("SELECT n FROM fixture_evidence", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    reader.close().unwrap();
    // Engine::shutdown reaps the ACP leader and signals its process group.
    // Descendants can still be closing SQLite's descriptors after that reply.
    tokio::time::timeout(Duration::from_secs(3), async {
        while wal.exists() || shm.exists() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("last close did not remove WAL companions after process-group shutdown");
    let reopened = opencode(&home, require_session).await;
    assert_eq!(fs::metadata(&db).unwrap().ino(), meta.ino());
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a NIC-less synthetic QEMU guest and the pinned binary"]
async fn guest_opencode_catalogue() {
    let home = home();
    let db = database(&home);
    assert!(!db.exists());
    crate::machine::config_action::catalog_refresh::fixture_catalogue(
        &environment(&home),
        &opencode_record(&home),
    )
    .await
    .expect("native service catalogue observation did not complete");
    assert!(db.is_file(), "catalogue must use the shared native DB path");
    println!("CORTEX_OPENCODE_CATALOGUE_PASS");
}

#[test]
#[ignore = "requires a NIC-less synthetic QEMU guest with the pinned CLI"]
fn guest_usage() {
    let home = home();
    let cwd = home.join("checkout");
    let response = match std::env::var("CORTEX_FIXTURE_ENGINE").as_deref() {
        Ok("codex") => {
            let data = crate::machine::usage_refresh::codex(
                Path::new("/usr/bin/codex"),
                &home,
                &home.join(".codex"),
                &cwd,
                None,
            )
            .expect("Codex app server must reply to account/rateLimits/read");
            assert_eq!(data["id"], 2);
            // The synthetic account is not a subscription. A correlated error is
            // transport evidence, not a qualified account or usable limits.
            if !data["result"].is_object() {
                assert!(data["error"].is_object());
                println!("CORTEX_CODEX_USAGE_OFFLINE_ERROR");
                return;
            }
            "codex"
        }
        Ok("claude") => {
            let bytes = crate::machine::usage_refresh::claude(
                Path::new("/usr/bin/claude"),
                &home,
                &home.join(".claude"),
                &cwd,
                None,
            )
            .expect("Claude /usage did not complete on the synthetic account");
            assert!(crate::config_read_model::usage_refresh::parse_claude(&bytes, 0.0).is_some());
            "claude"
        }
        _ => panic!("no OpenCode usage helper exists"),
    };
    println!("CORTEX_USAGE_RESULT {response}");
}

#[test]
#[ignore = "requires a NIC-less synthetic QEMU guest"]
fn guest_descendants() {
    let home = home();
    let role = std::env::var("CORTEX_FIXTURE_ROLE").unwrap();
    let neighbor = database(&home).with_file_name("neighbor");
    let allowed = home.join("checkout/input");
    assert_eq!(fs::read(&neighbor).unwrap(), b"synthetic neighbor\n");
    assert!(OpenOptions::new().write(true).open(&neighbor).is_ok());
    let output = Command::new("/usr/bin/aa-exec")
        .args(prefix(&role, &home).unwrap().into_iter().skip(1))
        .args(["/usr/bin/sh", "-ec"])
        .arg(concat!(
            "test \"$(/usr/bin/cat \"$1\")\" = 'usable project input'; ",
            "if /usr/bin/cat \"$2\" >/dev/null 2>&1; then exit 3; fi; ",
            "if (echo changed >\"$2\") 2>/dev/null; then exit 4; fi; ",
            "if /usr/bin/stat \"$2\" >/dev/null 2>&1; then echo metadata-visible; ",
            "else echo metadata-denied; fi"
        ))
        .arg("fixture")
        .arg(allowed)
        .arg(&neighbor)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "descendant probe: {:?}",
        output.status
    );
    assert_eq!(fs::read(&neighbor).unwrap(), b"synthetic neighbor\n");
    let observation = std::str::from_utf8(&output.stdout).unwrap().trim();
    assert!(matches!(
        observation,
        "metadata-visible" | "metadata-denied"
    ));
    println!("CORTEX_DESCENDANT_CONTENT_AND_WRITE_DENIED {role} {observation}");
}
