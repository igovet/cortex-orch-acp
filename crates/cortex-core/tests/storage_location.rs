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
        drop(Store::open(ProjectKey::new(&p.0, channel).unwrap(), OpenMode::Initialize).unwrap());
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
            (fs::read(&f).unwrap(), fs::metadata(&f).unwrap().modified().unwrap())
        })
        .collect();
    drop(Store::open(ProjectKey::new(&p.0, StoreChannel::Dev).unwrap(), OpenMode::Initialize).unwrap());
    let after: Vec<_> = [&release, &legacy]
        .iter()
        .map(|d| {
            let f = d.join("cortex.sqlite3");
            (fs::read(&f).unwrap(), fs::metadata(&f).unwrap().modified().unwrap())
        })
        .collect();
    assert_eq!(before, after);
    assert_eq!(fs::read_dir(&release).unwrap().count(), 1);
    assert_eq!(fs::read_dir(&legacy).unwrap().count(), 1);
    assert!(p.0.join(".cortex-dev/cortex.sqlite3").is_file());
    // The release channel refuses the canary rather than reading or replacing it.
    assert!(Store::open(
        ProjectKey::new(&p.0, StoreChannel::Release).unwrap(),
        OpenMode::LiveReadOnly
    )
    .is_err());
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
        drop(Store::open(ProjectKey::new(&p.0, channel).unwrap(), OpenMode::Initialize).unwrap());
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
        drop(Store::open(ProjectKey::new(&p.0, channel).unwrap(), OpenMode::Initialize).unwrap());
        assert_eq!(fs::read(&ignore).unwrap(), b"owner\n");
        // Read-only opens never create it.
        fs::remove_file(&ignore).unwrap();
        drop(Store::open(ProjectKey::new(&p.0, channel).unwrap(), OpenMode::RuntimeValidate).unwrap());
        assert!(!ignore.exists());
    }
}
