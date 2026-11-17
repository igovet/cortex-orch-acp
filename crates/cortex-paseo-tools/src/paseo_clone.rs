//! `paseo-clone`: create a read-consistent persistent dev clone of the production
//! Paseo home. The production home is only read: it is never written, locked or
//! modified, and the clone target is fixed to `~/.paseo-dev`.
//!
//! Runtime state (pid, lock and socket files) is not copied, nor is the per-home
//! daemon identity (`server-id`, `daemon-keypair.json`): the dev home keeps its own,
//! so it never adopts the production server id or relay keypair. A SQLite database is
//! copied as a consistent snapshot (`VACUUM INTO` over a read-only connection) rather
//! than as raw bytes, and its `-wal`/`-shm`/`-journal` sidecars are not copied, since
//! the snapshot already contains their committed content and a raw sidecar would not
//! match the snapshot file. Refresh only while the dev daemon is stopped.
use std::fs;
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

const RUNTIME_NAMES: [&str; 4] = ["daemon.pid", "daemon.lock", "daemon.sock", "paseo.sock"];
const RUNTIME_SUFFIXES: [&str; 3] = ["pid", "lock", "sock"];
const IDENTITY_NAMES: [&str; 2] = ["server-id", "daemon-keypair.json"];
const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

type Res<T> = Result<T, String>;

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/nonexistent"))
}

fn resolve(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut tail = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            return tail.iter().rev().fold(real, |acc: PathBuf, part| acc.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn skip(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    RUNTIME_NAMES.contains(&name)
        || path.extension().and_then(|e| e.to_str()).is_some_and(|e| RUNTIME_SUFFIXES.contains(&e))
        || fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

fn is_sqlite(path: &Path) -> bool {
    let mut header = [0u8; 16];
    fs::File::open(path).and_then(|mut f| f.read_exact(&mut header)).is_ok() && &header == b"SQLite format 3\0"
}

fn snapshot_sqlite(source: &Path, target: &Path) -> Res<()> {
    let connection = rusqlite::Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("cannot open {} read-only: {e}", source.display()))?;
    let destination = target.to_str().ok_or("non-UTF-8 clone path")?;
    connection.execute("VACUUM INTO ?1", [destination]).map_err(|e| format!("cannot snapshot {}: {e}", source.display()))?;
    Ok(())
}

fn copy_entry(from: &Path, to: &Path) -> Res<()> {
    let meta = fs::symlink_metadata(from).map_err(|e| format!("{}: {e}", from.display()))?;
    if meta.file_type().is_symlink() {
        let link = fs::read_link(from).map_err(|e| e.to_string())?;
        return std::os::unix::fs::symlink(link, to).map_err(|e| format!("{}: {e}", to.display()));
    }
    fs::copy(from, to).map_err(|e| format!("copy {}: {e}", from.display()))?;
    if let (Ok(modified), Ok(file)) = (meta.modified(), fs::File::open(to)) {
        let _ = file.set_modified(modified);
    }
    Ok(())
}

fn walk(source: &Path, destination: &Path, at_root: bool) -> Res<()> {
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    let mut entries: Vec<_> = fs::read_dir(source).map_err(|e| format!("{}: {e}", source.display()))?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    let names: Vec<String> = entries.iter().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    for entry in &entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if skip(&path) {
            continue;
        }
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            walk(&path, &destination.join(&name), false)?;
            continue;
        }
        if at_root && IDENTITY_NAMES.contains(&name.as_str()) {
            continue;
        }
        // A sidecar of a SQLite file that is itself snapshotted would not match the snapshot.
        let sidecar_of_database = SIDECARS.iter().any(|suffix| {
            name.strip_suffix(suffix).is_some_and(|base| names.iter().any(|n| n == base) && is_sqlite(&source.join(base)))
        });
        if sidecar_of_database {
            continue;
        }
        let target = destination.join(&name);
        if meta.is_file() && is_sqlite(&path) {
            snapshot_sqlite(&path, &target)?;
        } else {
            copy_entry(&path, &target)?;
        }
    }
    let _ = fs::set_permissions(destination, fs::Permissions::from_mode(fs::metadata(source).map(|m| m.permissions().mode()).unwrap_or(0o700)));
    Ok(())
}

fn clone_home(source: &Path, target: &Path) -> Res<()> {
    let parent = target.parent().ok_or("the clone target has no parent")?;
    let token = fs::read_to_string("/proc/sys/kernel/random/uuid").map_err(|e| e.to_string())?;
    let name = target.file_name().and_then(|n| n.to_str()).ok_or("bad target name")?;
    let stage = parent.join(format!(".{name}.refresh-{}", token.trim().replace('-', "")));
    let outcome = (|| {
        walk(source, &stage, true)?;
        // Keep the dev home's own identity; never adopt production's.
        for identity in IDENTITY_NAMES {
            let own = target.join(identity);
            let theirs = source.join(identity);
            let own_is_file = fs::symlink_metadata(&own).is_ok_and(|m| m.is_file());
            if own_is_file && !(theirs.is_file() && fs::read(&own).ok() == fs::read(&theirs).ok()) {
                copy_entry(&own, &stage.join(identity))?;
            }
        }
        if target.exists() {
            let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            fs::rename(target, target.with_file_name(format!("{name}.previous-{seconds}"))).map_err(|e| e.to_string())?;
        }
        fs::rename(&stage, target).map_err(|e| e.to_string())
    })();
    if outcome.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    outcome
}

pub fn run(argv: &[String]) -> (String, i32) {
    let production = resolve(&home().join(".paseo"));
    let mut source = home().join(".paseo").display().to_string();
    let mut target = home().join(".paseo-dev").display().to_string();
    let mut check = false;
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--source" | "--target" => {
                let Some(value) = it.next() else {
                    eprintln!("cortex_paseo_clone: {arg} needs a value");
                    return (String::new(), 2);
                };
                if arg == "--source" {
                    source = value.clone();
                } else {
                    target = value.clone();
                }
            }
            other => {
                eprintln!("cortex_paseo_clone: unrecognized argument {other:?}\nusage: paseo-clone [--source ~/.paseo] [--target ~/.paseo-dev] [--check]");
                return (String::new(), 2);
            }
        }
    }
    let (source, target) = (PathBuf::from(source), PathBuf::from(target));
    if !source.is_absolute() || !target.is_absolute() {
        eprintln!("cortex_paseo_clone: paths must be absolute");
        return (String::new(), 2);
    }
    let (source, target) = (resolve(&source), resolve(&target));
    if source != production {
        eprintln!("cortex_paseo_clone: source must be the production Paseo home");
        return (String::new(), 2);
    }
    if target != resolve(&home().join(".paseo-dev")) {
        eprintln!("cortex_paseo_clone: target must be ~/.paseo-dev");
        return (String::new(), 2);
    }
    if !source.is_dir() {
        eprintln!("cortex_paseo_clone: production Paseo home is missing");
        return (String::new(), 2);
    }
    let report = serde_json::json!({"source": source, "target": target});
    if !check {
        if let Err(error) = clone_home(&source, &target) {
            eprintln!("cortex_paseo_clone: {error}");
            return (String::new(), 1);
        }
    }
    (format!("{report}\n"), 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("paseo-clone-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn clone_copies_state_not_runtime_or_identity_and_snapshots_databases() {
        let base = scratch("copy");
        let (source, target) = (base.join("prod"), base.join("dev"));
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("config.json"), "{}").unwrap();
        fs::write(source.join("paseo.pid"), "1").unwrap();
        fs::write(source.join("nested/x.lock"), "1").unwrap();
        fs::write(source.join("server-id"), "prod-id").unwrap();
        fs::write(source.join("nested/server-id"), "kept-below-root").unwrap();
        let db = Connection::open(source.join("agents.sqlite")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(a); INSERT INTO t VALUES (7);").unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("server-id"), "dev-id").unwrap();
        clone_home(&source, &target).unwrap();
        assert!(target.join("config.json").is_file());
        assert!(!target.join("paseo.pid").exists() && !target.join("nested/x.lock").exists());
        assert_eq!(fs::read_to_string(target.join("server-id")).unwrap(), "dev-id");
        assert_eq!(fs::read_to_string(target.join("nested/server-id")).unwrap(), "kept-below-root");
        assert!(!target.join("agents.sqlite-wal").exists());
        let copy = Connection::open(target.join("agents.sqlite")).unwrap();
        assert_eq!(copy.query_row("SELECT a FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 7);
        // The previous dev home is retained, and the production tree is untouched.
        assert!(fs::read_dir(&base).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("dev.previous-")));
        assert_eq!(fs::read_to_string(source.join("server-id")).unwrap(), "prod-id");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn only_the_production_home_may_be_the_source_and_the_dev_home_the_target() {
        for argv in [["--source", "/tmp"], ["--target", "/tmp"]] {
            assert_eq!(run(&argv.map(String::from)).1, 2);
        }
        assert_eq!(run(&["--source".into(), "relative".into()]).1, 2);
    }
}
