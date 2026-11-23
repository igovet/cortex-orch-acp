use super::{tests::*, *};
use std::os::unix::fs::symlink;

#[test]
fn denial_expansion_matches_the_oracle_path_patterns() {
    for (path, pattern, expected) in [
        ("src/a.rs", "**/*.rs", true),
        ("src/a.rs", "src", true),
        ("src", "src/**", true),
        ("a/x/y", "a/*", true),
        ("routes/[locale]/a", "routes/[locale]", true),
        ("a/z", "a/[xy]", false),
        ("a/x", "a/[xy]", true),
        ("a/z", "a/[!xy]", true),
        ("literal[", "literal[", true),
        ("abc", "z*", false),
        ("a", "**/*", true),
    ] {
        assert_eq!(policy::matches(path, pattern), expected, "{path} {pattern}");
    }
    let f = Fixture::new();
    let root = f.root();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/no.txt"), "no").unwrap();
    let found = policy::expand(&root, &[root.join("**/*.txt")]).unwrap();
    assert_eq!(found, vec![root.join("src/no.txt")]);
}

#[test]
fn absent_relocated_control_paths_require_the_protected_route() {
    let f = Fixture::new();
    let root = f.root();
    let home = f.0.join("home");
    let mut env = BTreeMap::new();
    env.insert("HOME".into(), home.into_os_string());
    let config = f.0.join("relocated-config");
    env.insert("XDG_CONFIG_HOME".into(), config.clone().into_os_string());
    let state = f.0.join("relocated-state");
    env.insert("XDG_STATE_HOME".into(), state.clone().into_os_string());
    for p in [
        config.join("cortex/new"),
        state.join("cortex/new"),
        f.0.join("home/.gitconfig"),
    ] {
        assert!(!p.exists());
        assert!(policy::protected_target_environment(&root, &p, &env));
    }
    assert!(!policy::protected_target_environment(
        &root,
        &f.0.join("ordinary"),
        &env
    ));
    env.insert(
        "CODEX_HOME".into(),
        f.0.join("missing-codex").into_os_string(),
    );
    assert!(policy::controls(&root, &env).is_err());
}

#[test]
fn pin_rejects_source_alias_and_retains_original_mount_inode() {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    let f = Fixture::new();
    let root = f.root();
    let alias = root.join("alias");
    symlink(f.0.join("outside"), &alias).unwrap();
    assert!(policy::pin(&alias).is_err());
    let pin = policy::pin(&root.join("source")).unwrap();
    fs::rename(root.join("source"), root.join("old")).unwrap();
    fs::write(root.join("source"), "replacement").unwrap();
    let mut held = String::new();
    fs::File::open(format!("/proc/self/fd/{}", pin.as_raw_fd()))
        .unwrap()
        .read_to_string(&mut held)
        .unwrap();
    assert_eq!(held, "unchanged");
    assert_eq!(
        fs::read_to_string(root.join("source")).unwrap(),
        "replacement"
    );
}

#[test]
fn scan_masks_external_hardlinks_without_copying_or_opening_their_contents() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new();
    let root = f.root();
    let huge = f.0.join("sparse");
    fs::File::create(&huge)
        .unwrap()
        .set_len(2 * 1024 * 1024 * 1024 + 1)
        .unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    fs::hard_link(&huge, root.join("nested/external")).unwrap();
    fs::hard_link(root.join("source"), root.join("nested/internal")).unwrap();
    fs::write(root.join("nested/opencode.db-shm"), "synthetic companion").unwrap();
    let anchor = policy::pin(&root).unwrap();
    let scan = policy::scan(&root, &anchor, &[], &[], &[]).unwrap();
    assert_eq!(
        policy::external_link_paths(&scan, &Default::default()),
        vec![root.join("nested/external")]
    );
    assert!(scan.private.contains(&root.join("nested/opencode.db-shm")));
    assert_eq!(
        fs::metadata(&huge).unwrap().ino(),
        fs::metadata(root.join("nested/external")).unwrap().ino()
    );
    assert_eq!(
        fs::metadata(&huge).unwrap().len(),
        2 * 1024 * 1024 * 1024 + 1
    );
}

#[test]
fn scan_masks_recognizable_project_credentials_but_keeps_samples() {
    let f = Fixture::new();
    let root = f.root();
    for file in [".env", ".env.local", ".env.example", ".env.staging.sample", ".netrc",
        ".claude.json", "cortex.sqlite3-wal"]
    {
        fs::write(root.join(file), "synthetic").unwrap();
    }
    let checkout = policy::pin(&root).unwrap();
    let scan = policy::scan(&root, &checkout, &[], &[], &[]).unwrap();
    for file in [".env", ".env.local", ".netrc", ".claude.json", "cortex.sqlite3-wal"] {
        assert!(scan.private.contains(&root.join(file)), "{file}");
    }
    for file in [".env.example", ".env.staging.sample"] {
        assert!(!scan.private.contains(&root.join(file)), "{file}");
    }
}

fn git_metadata_fixture(root: &Path, outside: &Path) {
    assert!(Command::new("/usr/bin/git")
        .args(["init", "-q", "--template="])
        .current_dir(root)
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap()
        .success());
    let git = root.join(".git");
    fs::write(git.join(".env.local"), "private sentinel").unwrap();
    fs::hard_link(git.join(".env.local"), root.join("checkout-alias")).unwrap();
    fs::hard_link(outside, git.join("outside-alias")).unwrap();
    fs::hard_link(root.join("source"), git.join("visible-alias")).unwrap();
    fs::write(git.join("protected-target"), "symlink target").unwrap();
    symlink("protected-target", git.join(".netrc")).unwrap();
    fs::hard_link(git.join("protected-target"), root.join("target-alias")).unwrap();
    fs::create_dir_all(git.join("worktrees/sibling")).unwrap();
    fs::write(git.join("worktrees/sibling/marker"), "sibling sentinel").unwrap();
    fs::hard_link(git.join("worktrees/sibling/marker"), git.join("sibling-alias")).unwrap();
    let fifo = std::ffi::CString::new(git.join("pipe").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
}

#[test]
fn git_metadata_scan_accounts_for_private_and_visible_aliases() {
    let f = Fixture::new();
    let root = f.root();
    git_metadata_fixture(&root, &f.0.join("outside"));
    let git = root.join(".git");
    let checkout = policy::pin(&root).unwrap();
    let git_pin = std::sync::Arc::new(policy::pin(&git).unwrap());
    let scan = policy::scan(
        &root, &checkout, &[], &[(git.clone(), git_pin)], &[git.join("worktrees")],
    ).unwrap();
    assert!(scan.private.contains(&git.join(".env.local")));
    assert!(scan.private.contains(&git.join("pipe")));
    assert_eq!(scan.private_targets, vec![git.join("protected-target")]);
    let masks = scan.private_targets.iter().cloned().collect();
    assert_eq!(
        policy::external_link_paths(&scan, &masks),
        vec![
            git.join("outside-alias"), git.join("protected-target"),
            git.join("sibling-alias"), root.join("checkout-alias"), root.join("target-alias"),
        ]
    );
}

#[test]
fn git_metadata_masks_apply_in_ordinary_and_verifier_views() {
    let f = Fixture::new();
    let root = f.root();
    git_metadata_fixture(&root, &f.0.join("outside"));
    let available = capability();
    for profile in [Profile::Development, Profile::Guarded, Profile::ReadOnly] {
        let result = f.prepare(profile, &[], &[]);
        if !available {
            assert!(result.is_err(), "expected setup refusal");
            eprintln!("{profile:?}: no namespace-positive Git metadata evidence");
            continue;
        }
        let sandbox = result.unwrap();
        run(&sandbox, r#"
            /usr/bin/git status --short >/dev/null
            test "$(cat .git/visible-alias)" = unchanged
            test ! -s .git/.env.local
            test ! -s .git/outside-alias
            test ! -s checkout-alias
            test ! -s .git/.netrc
            test ! -s target-alias
            test ! -p .git/pipe
            test ! -e .git/worktrees/sibling/marker
            test ! -s .git/sibling-alias
        "#, &[]);
    }
}

#[test]
fn linked_git_metadata_keeps_current_registration_and_masks_siblings() {
    let f = Fixture::new();
    let root = f.root();
    git_metadata_fixture(&root, &f.0.join("outside"));
    let git = |args: &[&str]| {
        let out = Command::new("/usr/bin/git")
            .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid"])
            .args(args)
            .current_dir(&root)
            .env_clear()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success(), "fixture Git failed: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["add", "--", "source"]);
    git(&["commit", "-qm", "synthetic"]);
    let linked = f.0.join("linked");
    git(&["worktree", "add", "--detach", linked.to_str().unwrap(), "HEAD"]);

    let common = root.join(".git");
    let dir = common.join("worktrees/linked");
    fs::write(dir.join(".env.local"), "worktree private sentinel").unwrap();
    fs::hard_link(dir.join(".env.local"), linked.join("worktree-alias")).unwrap();
    fs::write(linked.join("source"), "modified\n").unwrap();

    let checkout = policy::pin(&linked).unwrap();
    let mut inputs = policy::git_inputs(&linked, &checkout).unwrap();
    let paths: Vec<_> = inputs.iter().map(|(path, _)| path.as_path()).collect();
    assert_eq!(paths, vec![dir.as_path(), common.as_path()]);
    inputs.sort_by_key(|(path, _)| path.components().count());
    let scan = policy::scan(
        &linked,
        &checkout,
        &[],
        &inputs,
        &[common.join("worktrees")],
    )
    .unwrap();
    assert!(scan.private.contains(&dir.join(".env.local")));
    assert!(scan.private.contains(&common.join(".env.local")));
    let masked = policy::external_link_paths(&scan, &Default::default());
    assert!(masked.contains(&linked.join("worktree-alias").as_path()));
    assert!(masked.contains(&common.join("sibling-alias").as_path()));

    if !capability() {
        eprintln!("linked Git observation needs namespace-positive execution");
        return;
    }
    let environment = BTreeMap::from([("HOME".into(), f.0.join("home").into_os_string())]);
    let mut sand = Sandbox::with_environment(
        &linked,
        Profile::ReadOnly,
        &[],
        &[],
        true,
        environment,
    )
    .unwrap();
    run(
        &sand,
        r#"
        test -f "$1/HEAD"
        test ! -s "$1/.env.local"
        test ! -s "$2/.env.local"
        test ! -e "$2/worktrees/sibling/marker"
        test ! -s worktree-alias
        "#,
        &[&dir, &common],
    );
    sand.restrict_git_observation(&linked).unwrap();
    run(
        &sand,
        r#"
        /usr/bin/git status --short | grep -Fx ' M source'
        /usr/bin/git diff --no-ext-diff --no-textconv HEAD -- | grep -Fx '+modified'
        "#,
        &[],
    );
}

#[test]
fn mount_source_descriptors_do_not_leak_to_the_payload() {
    let f = Fixture::new();
    if !capability() {
        assert!(f.prepare(Profile::ReadOnly, &[], &[]).is_err());
        eprintln!("descriptor isolation needs namespace-positive execution");
        return;
    }
    let s = f.prepare(Profile::ReadOnly, &[], &[]).unwrap();
    run(&s, r#"test "$(ls /proc/self/fd | wc -l)" -lt 8"#, &[]);
}
