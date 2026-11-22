mod support;
use cortex_core::{OpenMode, Store};
use std::{
    fs::File,
    os::{fd::AsRawFd, unix::fs::DirBuilderExt},
};
use support::*;

/// Query this file directly: the global `/proc/locks` listing can miss records
/// when other locks change during a read. Unlike F_GETLK, F_OFD_GETLK reports
/// this process's POSIX locks too, without acquiring a lock.
fn has_shared_lock(database: &File) -> bool {
    let mut probe = libc::flock {
        l_type: libc::F_WRLCK as _,
        l_whence: libc::SEEK_SET as _,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };
    // SAFETY: the descriptor is live and probe is a writable flock.
    let result = unsafe { libc::fcntl(database.as_raw_fd(), libc::F_OFD_GETLK, &mut probe) };
    assert_eq!(
        result,
        0,
        "F_OFD_GETLK: {}",
        std::io::Error::last_os_error()
    );
    i32::from(probe.l_type) == libc::F_RDLCK && probe.l_pid == std::process::id() as libc::pid_t
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
    // Drop the probe descriptor after SQLite, even if an assertion panics:
    // closing it first would itself release SQLite's POSIX locks.
    let database;
    let live = p.open();
    tasks(&live);
    database = File::open(p.key().database()).unwrap();
    assert!(
        has_shared_lock(&database),
        "an idle WAL connection holds its shared lock"
    );
    for mode in [OpenMode::Initialize, OpenMode::RuntimeValidate] {
        drop(Store::open(p.key(), mode).unwrap());
        assert!(
            has_shared_lock(&database),
            "{mode:?} open released live locks"
        );
    }
    assert_eq!(tasks(&live), 0);
    drop(live);
    assert!(
        !has_shared_lock(&database),
        "the last close releases the lock"
    );
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
