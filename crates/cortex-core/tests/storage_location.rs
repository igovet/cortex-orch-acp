//! One project store per logical project and channel: `.cortex/cortex.sqlite3`
//! for the default/release channel and `.cortex-dev/cortex.sqlite3` for
//! Workspace Dev, never a `.codex/cortex` store, and never any cross-channel
//! read, import or migration.
mod support;
use cortex_core::{OpenMode, ProjectKey, Store, StoreChannel};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use support::Project;

/// Every `cortex.sqlite3` database below `root`, relative and sorted.
fn databases(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                walk(&path, root, out);
            } else if path.file_name().is_some_and(|n| n == "cortex.sqlite3") {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = vec![];
    walk(root, root, &mut out);
    out.sort();
    out
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn each_channel_has_exactly_one_store_directory_and_database() {
    for (channel, directory) in [
        (StoreChannel::Release, ".cortex"),
        (StoreChannel::Dev, ".cortex-dev"),
    ] {
        let p = Project::new();
        let key = ProjectKey::new(&p.0, channel).unwrap();
        assert_eq!(key.directory(), p.0.join(directory));
        drop(Store::open(key, OpenMode::Initialize).unwrap());
        // Reopening, including through a second key, never adds another database.
        drop(
            Store::open(
                ProjectKey::new(&p.0, channel).unwrap(),
                OpenMode::Initialize,
            )
            .unwrap(),
        );
        assert_eq!(
            databases(&p.0),
            vec![PathBuf::from(directory).join("cortex.sqlite3")]
        );
        assert!(!p.0.join(".codex").exists(), "no .codex/cortex store");
    }
}

#[test]
fn a_linked_worktree_shares_the_one_store_of_its_logical_project() {
    let p = Project::new();
    git(&p.0, &["init", "-q", "-b", "main"]);
    git(&p.0, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let linked = p.0.join("linked-worktree");
    git(&p.0, &["worktree", "add", "-q", linked.to_str().unwrap()]);
    let linked = linked.canonicalize().unwrap();
    for channel in [StoreChannel::Release, StoreChannel::Dev] {
        let main = ProjectKey::new(&p.0, channel).unwrap();
        let other = ProjectKey::new(&linked, channel).unwrap();
        assert_eq!(other.root(), main.root());
        assert_eq!(other.database(), main.database());
        assert!(std::sync::Arc::ptr_eq(
            &other.commit_signal(),
            &main.commit_signal()
        ));
        drop(Store::open(main, OpenMode::Initialize).unwrap());
        drop(Store::open(other, OpenMode::LiveReadOnly).unwrap());
    }
    assert_eq!(
        databases(&p.0),
        vec![
            PathBuf::from(".cortex").join("cortex.sqlite3"),
            PathBuf::from(".cortex-dev").join("cortex.sqlite3"),
        ]
    );
}

#[test]
fn the_dev_store_never_reads_imports_or_migrates_release_or_legacy_storage() {
    let p = Project::new();
    let release = p.0.join(".cortex");
    let legacy = p.0.join(".codex/cortex");
    for dir in [&release, &legacy] {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("cortex.sqlite3"), b"canary: must not be opened").unwrap();
    }
    let before: Vec<_> = [&release, &legacy]
        .iter()
        .map(|d| {
            let f = d.join("cortex.sqlite3");
            (
                fs::read(&f).unwrap(),
                fs::metadata(&f).unwrap().modified().unwrap(),
            )
        })
        .collect();
    drop(
        Store::open(
            ProjectKey::new(&p.0, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap(),
    );
    let after: Vec<_> = [&release, &legacy]
        .iter()
        .map(|d| {
            let f = d.join("cortex.sqlite3");
            (
                fs::read(&f).unwrap(),
                fs::metadata(&f).unwrap().modified().unwrap(),
            )
        })
        .collect();
    assert_eq!(before, after);
    assert_eq!(fs::read_dir(&release).unwrap().count(), 1);
    assert_eq!(fs::read_dir(&legacy).unwrap().count(), 1);
    assert!(p.0.join(".cortex-dev/cortex.sqlite3").is_file());
    // The release channel refuses the canary rather than reading or replacing it.
    assert!(
        Store::open(
            ProjectKey::new(&p.0, StoreChannel::Release).unwrap(),
            OpenMode::LiveReadOnly
        )
        .is_err()
    );
    assert_eq!(
        fs::read(release.join("cortex.sqlite3")).unwrap(),
        b"canary: must not be opened"
    );
}

/// Live defect: `.cortex-dev/` showed as untracked in the project's `git status`. Each
/// channel's store keeps itself out of Git with its own `*` ignore file, never by editing
/// the project's tracked `.gitignore` or `.git/info/exclude`; an existing file stays.
#[test]
fn each_channel_store_stays_out_of_git_status_without_touching_project_ignore_files() {
    for (channel, directory) in [
        (StoreChannel::Release, ".cortex"),
        (StoreChannel::Dev, ".cortex-dev"),
    ] {
        let p = Project::new();
        git(&p.0, &["init", "-q"]);
        let exclude = fs::read(p.0.join(".git/info/exclude")).ok();
        drop(
            Store::open(
                ProjectKey::new(&p.0, channel).unwrap(),
                OpenMode::Initialize,
            )
            .unwrap(),
        );
        let ignore = p.0.join(directory).join(".gitignore");
        assert_eq!(fs::read(&ignore).unwrap(), b"*\n");
        let status = Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=all"])
            .current_dir(&p.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(status.status.success());
        assert_eq!(String::from_utf8(status.stdout).unwrap(), "", "{directory}");
        assert!(!p.0.join(".gitignore").exists());
        assert_eq!(fs::read(p.0.join(".git/info/exclude")).ok(), exclude);
        // A reopen never rewrites it, whatever it holds.
        fs::write(&ignore, b"owner\n").unwrap();
        drop(
            Store::open(
                ProjectKey::new(&p.0, channel).unwrap(),
                OpenMode::Initialize,
            )
            .unwrap(),
        );
        assert_eq!(fs::read(&ignore).unwrap(), b"owner\n");
        // Read-only opens never create it.
        fs::remove_file(&ignore).unwrap();
        drop(
            Store::open(
                ProjectKey::new(&p.0, channel).unwrap(),
                OpenMode::RuntimeValidate,
            )
            .unwrap(),
        );
        assert!(!ignore.exists());
    }
}

#[test]
fn injected_channel_roots_keep_home_databases_and_project_anchors_separate() {
    use cortex_core::store::{StoreRoot, locks, schema};
    use std::os::unix::fs::PermissionsExt;

    let p = Project::new();
    let state = Project::new();
    let release = StoreRoot::new(&state.0.join("release"), StoreChannel::Release).unwrap();
    let dev = StoreRoot::new(&state.0.join("dev"), StoreChannel::Dev).unwrap();
    for root in [&release, &dev] {
        let key = root.project_key(&p.0).unwrap();
        let expected = key.home_directory().unwrap();
        if key.channel() == StoreChannel::Dev {
            // An interrupted old initializer can leave only its private zero-byte file.
            locks::private_directory(&key.anchor_directory(), true).unwrap();
            locks::create_private_database(&key.database()).unwrap();
            locks::open_private(&key.anchor_directory().join(".access.lock"), true, false).unwrap();
        }
        assert!(Store::open(key.clone(), OpenMode::InspectImmutable).is_err());
        assert!(!root.path().exists());
        let opened = Store::open(key.clone(), OpenMode::Initialize).unwrap();
        assert_eq!(opened.key().directory(), expected);
        assert_eq!(opened.key(), &key);
        assert_eq!(
            expected.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            opened
                .key()
                .database()
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(opened);
        let legacy_key = ProjectKey::new(&p.0, key.channel()).unwrap();
        let old_db = rusqlite::Connection::open_with_flags(
            legacy_key.database(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            schema::validate_header(&old_db, false).unwrap_err().code(),
            "unsupported_storage"
        );
        assert_eq!(databases(root.path()).len(), 1);
        assert_eq!(
            databases(&key.anchor_directory()),
            vec![PathBuf::from("cortex.sqlite3")]
        );
        assert_eq!(
            Store::open(key, OpenMode::InspectImmutable)
                .unwrap()
                .key()
                .directory(),
            expected
        );
    }
}

#[test]
fn creating_a_home_store_preserves_pre_database_channel_routing() {
    use cortex_core::store::{StoreRoot, locks};
    use std::{io::Write, os::unix::fs::OpenOptionsExt};

    let project = Project::new();
    let state = Project::new();
    let dev = StoreRoot::new(&state.0, StoreChannel::Dev).unwrap();
    let key = dev.project_key(&project.0).unwrap();
    let source = key.anchor_directory();
    locks::private_directory(&source, true).unwrap();
    let routing = source.join("routing.json");
    let write_routing = |path: &Path, bytes: &[u8]| {
        fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    };
    write_routing(&routing, b"{\"model_policy_version\":2}\n");
    let authoritative = key.home_directory().unwrap().join("routing.json");
    assert!(Store::open(key.clone(), OpenMode::LiveReadOnly).is_err());
    assert!(!authoritative.exists());
    drop(Store::open(key.clone(), OpenMode::Initialize).unwrap());
    let retained = source.join(".migration-backups/relocated-pre-database-routing/routing.json");
    assert!(!routing.exists());
    assert_eq!(
        fs::read(&authoritative).unwrap(),
        fs::read(&retained).unwrap()
    );
    write_routing(&authoritative, b"new home bytes");
    drop(Store::open(key, OpenMode::Initialize).unwrap());
    assert_eq!(fs::read(authoritative).unwrap(), b"new home bytes");
}

#[test]
fn reports_progress_only_for_an_actual_fenced_relocation() {
    use cortex_core::store::StoreRoot;
    use std::sync::{Arc, Mutex};

    let project = Project::new();
    let state = Project::new();
    let fresh = Project::new();
    let root = StoreRoot::new(&state.0, StoreChannel::Dev).unwrap();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let watch = |project: &Path| {
        let observations = observations.clone();
        root.project_key(project).unwrap().observe_relocation(move |active| {
            observations.lock().unwrap().push(active);
        })
    };
    drop(Store::open(watch(&fresh.0), OpenMode::Initialize).unwrap());
    let checkout = ProjectKey::new(&project.0, StoreChannel::Dev).unwrap();
    drop(Store::open(checkout.clone(), OpenMode::Initialize).unwrap());
    drop(Store::open(watch(&project.0), OpenMode::LiveReadOnly).unwrap());
    assert!(observations.lock().unwrap().is_empty());
    let pending = checkout.directory().join(".publication_pending.json");
    fs::write(&pending, b"{}").unwrap();
    assert!(Store::open(watch(&project.0), OpenMode::Initialize).is_err());
    assert_eq!(*observations.lock().unwrap(), [true, false]);
    fs::remove_file(pending).unwrap();
    drop(Store::open(watch(&project.0), OpenMode::Initialize).unwrap());
    drop(Store::open(watch(&project.0), OpenMode::Initialize).unwrap());
    assert_eq!(*observations.lock().unwrap(), [true, false, true, false]);
}

#[test]
fn absent_and_copied_anchors_respect_the_original_project_identity() {
    use cortex_core::store::StoreRoot;

    let p = Project::new();
    let state = Project::new();
    let root = StoreRoot::new(&state.0, StoreChannel::Release).unwrap();
    let key = root.project_key(&p.0).unwrap();
    drop(Store::open(key.clone(), OpenMode::Initialize).unwrap());

    let different = StoreRoot::new(&state.0.join("other"), StoreChannel::Release).unwrap();
    let wrong = different.project_key(&p.0).unwrap();
    assert_eq!(
        Store::open(wrong, OpenMode::Initialize)
            .err()
            .unwrap()
            .code(),
        "store_root_mismatch"
    );
    assert!(!different.path().exists());

    fs::remove_dir_all(key.anchor_directory()).unwrap();
    assert_eq!(
        Store::open(key.clone(), OpenMode::LiveReadOnly)
            .err()
            .unwrap()
            .code(),
        "project_store_anchor_missing"
    );
    assert!(!key.anchor_directory().exists());
    drop(Store::open(key.clone(), OpenMode::Initialize).unwrap());
    let replacement = p.0.join("replacement");
    fs::create_dir(&replacement).unwrap();
    let original = state.0.join("original-project");
    fs::rename(&p.0, &original).unwrap();
    fs::rename(original.join("replacement"), &p.0).unwrap();
    fs::rename(original.join(".cortex"), key.anchor_directory()).unwrap();
    assert_eq!(
        Store::open(key, OpenMode::Initialize).err().unwrap().code(),
        "project_storage_mismatch"
    );
}

#[test]
fn linked_worktree_and_symlink_share_the_bound_home_store() {
    use cortex_core::store::StoreRoot;
    use std::os::unix::fs::symlink;

    let p = Project::new();
    let state = Project::new();
    git(&p.0, &["init", "-q", "-b", "main"]);
    git(&p.0, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let linked = state.0.join("linked");
    git(&p.0, &["worktree", "add", "-q", linked.to_str().unwrap()]);
    let alias = state.0.join("alias");
    symlink(&linked, &alias).unwrap();
    let state_alias = state.0.join("state-alias");
    let canonical_state = state.0.join("real-state");
    fs::create_dir(&canonical_state).unwrap();
    symlink(&canonical_state, &state_alias).unwrap();

    let root = StoreRoot::new(&state_alias, StoreChannel::Dev).unwrap();
    let key = root.project_key(&alias).unwrap();
    assert_eq!(key, root.project_key(&p.0).unwrap());
    assert!(key.home_directory().unwrap().starts_with(canonical_state));
    let store = Store::open(key, OpenMode::Initialize).unwrap();
    let second = Store::open(root.project_key(&linked).unwrap(), OpenMode::LiveReadOnly).unwrap();
    assert_eq!(store.key().directory(), second.key().directory());
    assert!(!linked.join(".cortex-dev").exists());
}

#[test]
fn relocated_store_with_pending_wal_is_inspected_without_mutation() {
    use cortex_core::store::StoreRoot;
    use sha2::{Digest, Sha256};
    use std::{collections::BTreeMap, os::unix::fs::MetadataExt};

    // Snapshot outside the process that opened the writer, after that
    // process has exited without closing SQLite. This preserves committed
    // WAL evidence and never opens a plain database FD beside a live one.
    let p = Project::new();
    let state = Project::new();
    let local = ProjectKey::new(&p.0, StoreChannel::Dev).unwrap();
    drop(Store::open(local.clone(), OpenMode::Initialize).unwrap());
    let root = StoreRoot::new(&state.0, StoreChannel::Dev).unwrap();
    let key = root.project_key(&p.0).unwrap();
    let relocated = Store::open(key.clone(), OpenMode::Initialize).unwrap();
    let home = relocated.key().directory();
    assert_ne!(home, local.directory());
    drop(relocated);
    assert!(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "leave_pending_wal_child"])
            .env(
                "CORTEX_TEST_PENDING_WAL_DATABASE",
                home.join("cortex.sqlite3")
            )
            .status()
            .unwrap()
            .success()
    );
    assert!(home.join("cortex.sqlite3-wal").metadata().unwrap().len() > 0);

    let snapshot = |directory: &Path| {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let meta = entry.metadata().unwrap();
                let digest = meta
                    .is_file()
                    .then(|| Sha256::digest(fs::read(entry.path()).unwrap()).to_vec());
                (
                    entry.file_name(),
                    (
                        meta.dev(),
                        meta.ino(),
                        meta.len(),
                        meta.mode(),
                        meta.modified().unwrap(),
                        digest,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let anchor = local.directory();
    let before = (snapshot(&home), snapshot(&anchor));
    assert_eq!(
        Store::open(key.clone(), OpenMode::InspectImmutable)
            .err()
            .unwrap()
            .code(),
        "storage_inspection_requires_checkpoint"
    );
    assert_eq!((snapshot(&home), snapshot(&anchor)), before);

    // Simulate the original checkout being unavailable. Inspection must
    // neither create a project at the canonical path nor touch its home store.
    let detached = state.0.join("detached-project");
    fs::rename(&p.0, &detached).unwrap();
    let result = Store::open(key, OpenMode::InspectImmutable)
        .err()
        .unwrap()
        .code();
    assert!(!p.0.exists());
    fs::rename(&detached, &p.0).unwrap();
    assert_eq!(result, "project_store_anchor_missing");
    assert_eq!((snapshot(&home), snapshot(&anchor)), before);
}

#[test]
fn leave_pending_wal_child() {
    let Some(path) = std::env::var_os("CORTEX_TEST_PENDING_WAL_DATABASE") else {
        return;
    };
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .unwrap();
    db.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
         CREATE TABLE pending_wal_fixture(value TEXT);
         INSERT INTO pending_wal_fixture VALUES ('committed only to WAL');",
    )
    .unwrap();
    std::process::exit(0);
}
