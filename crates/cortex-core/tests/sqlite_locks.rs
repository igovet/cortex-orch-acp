mod support;
use cortex_core::{OpenMode, Store};
use std::{
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::Path,
};
use support::*;

/// POSIX record locks this process holds on `path`, read from `/proc/locks`
/// without opening `path` (closing any descriptor of it would drop them).
fn held_locks(path: &Path) -> usize {
    let ino = std::fs::metadata(path).unwrap().ino().to_string();
    let pid = std::process::id().to_string();
    std::fs::read_to_string("/proc/locks")
        .unwrap()
        .lines()
        .filter(|line| {
            let f: Vec<_> = line.split_whitespace().collect();
            f.get(1) == Some(&"POSIX")
                && f.get(4) == Some(&pid.as_str())
                && f.get(5).and_then(|d| d.rsplit(':').next()) == Some(ino.as_str())
        })
        .count()
}

fn tasks(store: &Store) -> i64 {
    store
        .read(|db| Ok(db.query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))?))
        .unwrap()
}

/// Closing any descriptor of a file releases every POSIX lock the process
/// holds on it, including SQLite's. Another process closing its connection
/// would then see itself as the last one and delete the live WAL and SHM.
#[test]
fn opening_an_existing_store_keeps_the_locks_of_live_connections() {
    let p = Project::new();
    let live = p.open();
    tasks(&live);
    let database = p.key().database();
    assert!(held_locks(&database) > 0, "an idle WAL connection holds its shared lock");
    for mode in [OpenMode::Initialize, OpenMode::RuntimeValidate] {
        drop(Store::open(p.key(), mode).unwrap());
        assert!(held_locks(&database) > 0, "{mode:?} open released live locks");
    }
    assert_eq!(tasks(&live), 0);
}

#[test]
fn initialize_creates_a_private_database_and_refuses_a_symlink() {
    use std::os::unix::fs::PermissionsExt;
    let p = Project::new();
    drop(p.open());
    let database = p.key().database();
    assert_eq!(
        std::fs::metadata(&database).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let q = Project::new();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(q.key().directory())
        .unwrap();
    std::os::unix::fs::symlink(&database, q.key().database()).unwrap();
    assert!(Store::open(q.key(), OpenMode::Initialize).is_err());
    assert!(std::fs::symlink_metadata(q.key().database())
        .unwrap()
        .file_type()
        .is_symlink());
}
