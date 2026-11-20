use super::{tests::*, *};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
fn git(root: &Path, argv: &[&str]) {
    let out = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(argv)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "fixture git: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn available(f: &Fixture) -> bool {
    if capability() {
        true
    } else {
        assert!(f.prepare(Profile::Development, &[f.root()], &[]).is_err());
        eprintln!("advanced namespace fixtures unavailable: preparation refused; positive coverage requires container");
        false
    }
}
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
fn guarded_home_state_readonly_caches_and_sensitive_relocations() {
    let f = Fixture::new();
    let root = f.root();
    let home = f.0.join("home");
    fs::remove_dir_all(home.join(".cache")).unwrap();
    for n in [
        ".sdk/bin",
        ".config/autostart",
        ".local/bin",
        ".local/share/tool",
        ".local/share/keyrings",
        ".local/state/cortex",
        ".ssh",
        ".cargo/registry",
    ] {
        fs::create_dir_all(home.join(n)).unwrap();
    }
    fs::create_dir(f.0.join("relocated")).unwrap();
    let mut env = BTreeMap::new();
    env.insert("HOME".into(), home.clone().into_os_string());
    env.insert(
        "UV_CACHE_DIR".into(),
        f.0.join("relocated").into_os_string(),
    );
    env.insert("PIP_CACHE_DIR".into(), home.join(".ssh").into_os_string());
    env.insert("npm_config_cache".into(), home.clone().into_os_string());
    let result = Sandbox::with_environment(
        &root,
        Profile::Guarded,
        &[root.clone()],
        &[],
        false,
        env.clone(),
    );
    if !available(&f) {
        assert!(result.is_err());
        return;
    }
    let s = result.unwrap();
    run(
        &s,
        r#"
        touch "$HOME/.cache/new" "$HOME/.npm/new" "$HOME/.sdk/bin/new"
        touch "$HOME/.local/share/tool/new" "$HOME/.local/state/new"
        ! touch "$HOME/.ssh/no"; ! touch "$HOME/.config/autostart/no"
        ! touch "$HOME/.local/bin/no"; ! touch "$HOME/.local/share/keyrings/no"
        ! touch "$HOME/.local/state/cortex/no"
    "#,
        &[],
    );
    drop(s);
    let s = Sandbox::with_environment(&root, Profile::ReadOnly, &[], &[], false, env).unwrap();
    run(
        &s,
        r#"
        touch "$HOME/.cache/new" "$HOME/.cargo/registry/new" "$UV_CACHE_DIR/new"
        ! touch "$HOME/.sdk/bin/no"; ! touch "$HOME/.cargo/bin/no"
        ! touch "$HOME/.local/share/tool/no"; ! touch "$PIP_CACHE_DIR/no"
        ! touch "$npm_config_cache/no"
    "#,
        &[],
    );
    assert!(f.0.join("relocated/new").is_file());
}
#[test]
fn git_data_guards_linked_worktrees_submodules_and_absent_entries() {
    let f = Fixture::new();
    let root = f.root();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "source"]);
    git(&root, &["commit", "-qm", "initial"]);
    let linked = f.0.join("linked");
    git(
        &root,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    fs::create_dir(root.join("husky")).unwrap();
    fs::write(root.join("husky/hook"), "fixture").unwrap();
    git(&root, &["config", "core.hooksPath", "husky"]);
    let module = root.join(".git/modules/sub");
    fs::create_dir_all(&module).unwrap();
    git(&module, &["init", "--bare", "-q"]);
    let sibling = root.join(".git/worktrees/other");
    fs::create_dir_all(&sibling).unwrap();
    fs::write(sibling.join("config"), "unchanged").unwrap();
    if !available(&f) {
        return;
    }
    for profile in [Profile::Guarded, Profile::Development] {
        let mut s = f.prepare(profile, &[root.clone()], &[]).unwrap();
        run(
            &s,
            r#"
            echo change >> source
            git -c user.name=Fixture -c user.email=fixture@example.invalid add source
            git -c user.name=Fixture -c user.email=fixture@example.invalid commit -qm updated
            ! git config planted.value wrong
            ! touch .git/hooks/injected; ! touch husky/injected
            ! touch .git/worktrees/other/config
            echo planted > .git/config.worktree
        "#,
            &[],
        );
        if profile == Profile::Development {
            run(
                &s,
                r#"
                test "$(git config core.hooksPath)" = /dev/null
                test "$(git config core.fsmonitor)" = false
                ! touch .git/info/attributes
                touch .git/modules/sub/objects/data
                ! touch .git/modules/new
                ! touch .git/modules/sub/config
            "#,
                &[],
            );
        } else {
            run(
                &s,
                "touch .git/info/attributes; ! touch .git/modules/sub/objects/no",
                &[],
            );
        }
        assert!(s.finish().contains(&"config.worktree".to_owned()));
        assert!(!root.join(".git/config.worktree").exists());
    }
    // Reopen only the selected worktree beneath the common read-only container.
    let mut env = BTreeMap::new();
    env.insert("HOME".into(), f.0.join("home").into_os_string());
    let mut s = Sandbox::with_environment(
        &linked,
        Profile::Development,
        &[linked.clone()],
        &[],
        false,
        env,
    )
    .unwrap();
    run(&s,"echo linked >> source; git add source; ! touch .git; ! touch \"$1/config\"; ! touch \"$2/config\"",
        &[&root.join(".git"),&sibling]);
    s.finish();
    assert_eq!(
        fs::read_to_string(sibling.join("config")).unwrap(),
        "unchanged"
    );
}
#[test]
fn narrow_grants_do_not_reopen_git_and_nested_metadata_stays_readonly() {
    let f = Fixture::new();
    let root = f.root();
    git(&root, &["init", "-q"]);
    fs::create_dir_all(root.join("nested/.git")).unwrap();
    fs::write(root.join("nested/.git/config"), "private").unwrap();
    if !available(&f) {
        return;
    }
    let s = f
        .prepare(
            Profile::Development,
            &[root.join("source"), root.join("missing")],
            &[],
        )
        .unwrap();
    run(
        &s,
        "echo changed > source; ! touch .git/index; ! touch nested/.git/config",
        &[],
    );
    assert_eq!(
        fs::read_to_string(root.join("nested/.git/config")).unwrap(),
        "private"
    );
}
#[test]
fn readonly_overlay_is_discarded_but_ignored_output_persists() {
    let f = Fixture::new();
    let root = f.root();
    fs::create_dir(root.join("cache")).unwrap();
    if !available(&f) {
        return;
    }
    let s = f
        .prepare(Profile::ReadOnly, &[root.join("cache")], &[])
        .unwrap();
    if std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_some() {
        assert!(
            s.overlay,
            "documented container must exercise the real disk-backed overlay"
        );
    }
    if s.overlay {
        run(&s,"echo discarded > source; echo new > generated; touch cache/persistent; ! touch .cortex-dev/new; test \"$PYTEST_ADDOPTS\" != ''",&[]);
    } else {
        run(
            &s,
            "! touch source; ! touch generated; touch cache/persistent",
            &[],
        );
    }
    let scratch = s.scratch.clone();
    drop(s);
    assert_eq!(
        fs::read_to_string(root.join("source")).unwrap(),
        "unchanged"
    );
    assert!(!root.join("generated").exists());
    assert!(root.join("cache/persistent").exists());
    assert!(!scratch.exists());
}
#[test]
fn hardlink_external_materialization_internal_aliases_and_denials() {
    let f = Fixture::new();
    let root = f.root();
    fs::write(f.0.join("shared"), "outside").unwrap();
    fs::hard_link(f.0.join("shared"), root.join("external")).unwrap();
    fs::hard_link(root.join("source"), root.join("internal")).unwrap();
    fs::hard_link(f.0.join("shared"), root.join("denied")).unwrap();
    if !available(&f) {
        return;
    }
    let s = f
        .prepare(Profile::Guarded, &[root.clone()], &[root.join("denied")])
        .unwrap();
    run(
        &s,
        "! sh -c 'echo no > external'; echo internal > source; test \"$(cat internal)\" = internal",
        &[],
    );
    drop(s);
    let s = f
        .prepare(
            Profile::Development,
            &[root.clone()],
            &[root.join("denied")],
        )
        .unwrap();
    run(
        &s,
        "echo detached > external; ! sh -c 'echo no > denied'; echo internal2 > internal",
        &[],
    );
    assert_eq!(fs::read_to_string(f.0.join("shared")).unwrap(), "outside");
    assert_eq!(
        fs::read_to_string(root.join("external")).unwrap(),
        "detached\n"
    );
    assert_eq!(
        fs::metadata(root.join("source")).unwrap().ino(),
        fs::metadata(root.join("internal")).unwrap().ino()
    );
    assert_ne!(
        fs::metadata(root.join("external")).unwrap().ino(),
        fs::metadata(f.0.join("shared")).unwrap().ino()
    );
}
#[test]
fn materialization_rechecks_authority_source_and_parent_without_alias_writes() {
    let f = Fixture::new();
    let root = f.root();
    fs::write(f.0.join("shared"), "outside").unwrap();
    fs::hard_link(f.0.join("shared"), root.join("external")).unwrap();
    let mut env = BTreeMap::new();
    env.insert("HOME".into(), f.0.join("home").into_os_string());
    // This callback is the production pre-mutation authority fence.
    let r = Sandbox::with_environment_authorized(
        &root,
        Profile::Development,
        &[root.clone()],
        &[],
        false,
        env.clone(),
        &mut || Err(io::Error::other("authority_changed")),
    );
    assert!(r.is_err());
    assert_eq!(fs::metadata(root.join("external")).unwrap().nlink(), 2);
    let r = Sandbox::with_environment_authorized(
        &root,
        Profile::Development,
        &[root.clone()],
        &[],
        false,
        env,
        &mut || {
            fs::remove_file(root.join("external"))?;
            symlink(f.0.join("shared"), root.join("external"))
        },
    );
    assert!(r.is_err());
    assert_eq!(fs::read_to_string(f.0.join("shared")).unwrap(), "outside");
}
#[test]
fn pin_rejects_source_alias_and_retains_original_mount_inode() {
    let f = Fixture::new();
    let root = f.root();
    let alias = root.join("alias");
    symlink(f.0.join("outside"), &alias).unwrap();
    assert!(policy::pin(&alias).is_err());
    if !available(&f) {
        return;
    }
    let s = f
        .prepare(Profile::Guarded, &[root.join("source")], &[])
        .unwrap();
    fs::rename(root.join("source"), root.join("old")).unwrap();
    fs::write(root.join("source"), "replacement").unwrap();
    run(
        &s,
        "test \"$(cat source)\" = unchanged; echo pinned > source",
        &[],
    );
    assert_eq!(
        fs::read_to_string(root.join("source")).unwrap(),
        "replacement"
    );
    assert_eq!(fs::read_to_string(root.join("old")).unwrap(), "pinned\n");
}
#[test]
fn glob_denials_override_project_grants_without_disabling_command() {
    let f = Fixture::new();
    let root = f.root();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/deny.txt"), "unchanged").unwrap();
    fs::write(root.join("src/ok.rs"), "ok").unwrap();
    if !available(&f) {
        return;
    }
    let s = f
        .prepare(
            Profile::Development,
            &[root.clone()],
            &[root.join("**/*.txt")],
        )
        .unwrap();
    run(
        &s,
        "! sh -c 'echo no > src/deny.txt'; echo yes > src/ok.rs; echo yes > new",
        &[],
    );
    assert_eq!(
        fs::read_to_string(root.join("src/deny.txt")).unwrap(),
        "unchanged"
    );
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
fn narrow_file_materialization_and_private_alias_targets() {
    let f = Fixture::new();
    let root = f.root();
    fs::write(f.0.join("shared"), "outside").unwrap();
    fs::hard_link(f.0.join("shared"), root.join("external")).unwrap();
    fs::set_permissions(root.join("external"), fs::Permissions::from_mode(0o751)).unwrap();
    fs::create_dir(root.join("protected-data")).unwrap();
    fs::hard_link(f.0.join("shared"), root.join("protected-data/alias")).unwrap();
    symlink("protected-data", root.join("nested-private")).unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    symlink("../protected-data", root.join("nested/.cortex")).unwrap();
    if !available(&f) {
        return;
    }
    let s = f
        .prepare(
            Profile::Development,
            &[root.join("external"), root.join("protected-data")],
            &[],
        )
        .unwrap();
    run(
        &s,
        "echo detached > external; ! sh -c 'echo wrong > protected-data/alias'",
        &[],
    );
    assert_eq!(fs::read_to_string(f.0.join("shared")).unwrap(), "outside");
    assert_eq!(
        fs::metadata(root.join("external")).unwrap().mode() & 0o777,
        0o751
    );
}
#[test]
fn hardlink_byte_limit_and_parent_replacement_are_prelaunch_refusals() {
    let f = Fixture::new();
    let root = f.root();
    let huge = fs::File::create(f.0.join("sparse")).unwrap();
    huge.set_len(2 * 1024 * 1024 * 1024 + 1).unwrap();
    fs::hard_link(f.0.join("sparse"), root.join("huge")).unwrap();
    let mut called = false;
    let r = Sandbox::with_environment_authorized(
        &root,
        Profile::Development,
        &[root.clone()],
        &[],
        false,
        BTreeMap::new(),
        &mut || {
            called = true;
            Ok(())
        },
    );
    assert!(r
        .err()
        .unwrap()
        .to_string()
        .contains("command_hardlink_materialization_limit"));
    assert!(!called);
    fs::remove_file(root.join("huge")).unwrap();
    fs::create_dir(root.join("dir")).unwrap();
    fs::write(f.0.join("shared"), "outside").unwrap();
    fs::hard_link(f.0.join("shared"), root.join("dir/link")).unwrap();
    fs::create_dir(f.0.join("replacement")).unwrap();
    fs::write(f.0.join("replacement/link"), "other").unwrap();
    let r = Sandbox::with_environment_authorized(
        &root,
        Profile::Development,
        &[root.clone()],
        &[],
        false,
        BTreeMap::new(),
        &mut || {
            fs::rename(root.join("dir"), root.join("moved"))?;
            symlink(f.0.join("replacement"), root.join("dir"))
        },
    );
    assert!(r.is_err());
    assert_eq!(
        fs::read_to_string(f.0.join("replacement/link")).unwrap(),
        "other"
    );
    assert_eq!(fs::read_to_string(f.0.join("shared")).unwrap(), "outside");
}
#[test]
fn hardlink_heavy_guarded_directories_are_demoted_as_a_whole() {
    let f = Fixture::new();
    let root = f.root();
    fs::create_dir(root.join("deps")).unwrap();
    fs::write(f.0.join("shared"), "outside").unwrap();
    for n in 0..257 {
        fs::hard_link(f.0.join("shared"), root.join(format!("deps/{n}"))).unwrap();
    }
    if !available(&f) {
        return;
    }
    let s = f.prepare(Profile::Guarded, &[root.clone()], &[]).unwrap();
    run(
        &s,
        "! touch deps/new; ! sh -c 'echo no > deps/0'; touch allowed",
        &[],
    );
    assert!(!root.join("deps/new").exists());
    assert!(root.join("allowed").exists());
}

#[test]
fn bulky_ignored_external_hardlinks_stay_readonly_without_copying() {
    let f = Fixture::new();
    let root = f.root();
    // Run this filesystem-heavy positive fixture only where the required namespace
    // backend exists; the direct host still checks fail-closed preparation.
    if !available(&f) {
        return;
    }
    git(&root, &["init", "-q"]);
    fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
    let deps = root.join("node_modules");
    fs::create_dir(&deps).unwrap();
    for n in 0..2 {
        fs::write(f.0.join(format!("shared-{n}")), "outside").unwrap();
    }
    for n in 0..100_001 {
        fs::hard_link(
            f.0.join(format!("shared-{}", n % 2)),
            deps.join(n.to_string()),
        )
        .unwrap();
    }
    let mut called = false;
    let s = Sandbox::with_environment_authorized(
        &root,
        Profile::Development,
        &[root.clone()],
        &[],
        false,
        BTreeMap::new(),
        &mut || {
            called = true;
            Ok(())
        },
    )
    .unwrap();
    assert!(!called, "bulky ignored output must not be materialized");
    run(
        &s,
        "! touch node_modules/new; ! sh -c 'echo no > node_modules/0'; touch allowed",
        &[],
    );
    assert_eq!(
        fs::metadata(deps.join("0")).unwrap().ino(),
        fs::metadata(f.0.join("shared-0")).unwrap().ino()
    );
}
#[test]
fn mount_source_descriptors_do_not_leak_to_the_payload() {
    let f = Fixture::new();
    if !available(&f) {
        return;
    }
    for profile in [Profile::ReadOnly, Profile::Guarded, Profile::Development] {
        let writes = if profile == Profile::ReadOnly {
            vec![]
        } else {
            vec![f.root()]
        };
        let s = f.prepare(profile, &writes, &[]).unwrap();
        run(&s, r#"test "$(ls /proc/self/fd | wc -l)" -lt 8"#, &[]);
    }
}

#[test]
fn missing_or_invalid_home_uses_scratch_without_unprotecting_xdg_state() {
    let f = Fixture::new();
    let root = f.root();
    let config = f.0.join("config");
    fs::create_dir_all(config.join("cortex")).unwrap();
    fs::create_dir(f.0.join("relocated")).unwrap();
    let mut env = BTreeMap::new();
    env.insert("HOME".into(), "relative-home".into());
    env.insert("XDG_CONFIG_HOME".into(), config.clone().into_os_string());
    env.insert(
        "UV_CACHE_DIR".into(),
        f.0.join("relocated").into_os_string(),
    );
    env.insert(
        "PIP_CACHE_DIR".into(),
        config.join("cortex").into_os_string(),
    );
    assert!(policy::protected_target_environment(
        &root,
        &config.join("cortex/new"),
        &env
    ));
    let r = Sandbox::with_environment(&root, Profile::ReadOnly, &[], &[], false, env);
    if !available(&f) {
        assert!(r.is_err());
        return;
    }
    let s = r.unwrap();
    run(
        &s,
        r#"test "$HOME" = "$TMPDIR"; test "$XDG_CACHE_HOME" = "$TMPDIR/.cache"
        touch "$UV_CACHE_DIR/new"; ! touch "$PIP_CACHE_DIR/no""#,
        &[],
    );
    assert!(f.0.join("relocated/new").exists());
    assert!(!config.join("cortex/no").exists());
}
/// One parallel scan replaces the two former walks: it finds private entries at any
/// depth (without descending them), every writable external hard link, and counts
/// exactly what a sequential walk sees; the prepared view then hides those entries.
#[test]
fn single_scan_finds_deep_private_entries_and_external_links() {
    let f = Fixture::new();
    let root = f.root();
    for i in 0..40 {
        let dir = root.join(format!("tree/{i}/deeper/{}", i % 7));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("file"), "x").unwrap();
    }
    fs::create_dir_all(root.join("tree/3/deeper/.cortex")).unwrap();
    fs::write(root.join("tree/3/deeper/.cortex/secret"), "private").unwrap();
    fs::create_dir_all(root.join("tree/9/.git/objects")).unwrap();
    fs::write(root.join("tree/9/.git/config"), "[core]").unwrap();
    fs::write(root.join("tree/12/.cortex-workspace-dev.json"), "{}").unwrap();
    fs::write(f.0.join("shared"), "outside").unwrap();
    fs::hard_link(f.0.join("shared"), root.join("tree/20/deeper/6/external")).unwrap();
    fs::hard_link(root.join("source"), root.join("tree/5/internal")).unwrap();
    let scan = policy::scan(&root, &[root.clone()], &[]).unwrap();
    // Read-only binds first, store directories (hidden by a read-only tmpfs) last.
    assert_eq!(
        scan.private,
        [
            root.join("tree/12/.cortex-workspace-dev.json"),
            root.join("tree/9/.git"),
            root.join(".cortex-dev"),
            root.join("tree/3/deeper/.cortex"),
        ]
    );
    let mut sequential = 0;
    let mut pending = vec![root.clone()];
    while let Some(dir) = pending.pop() {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            sequential += 1;
            if e.file_type().unwrap().is_dir() && !policy::private(&e.file_name()) {
                pending.push(e.path());
            }
        }
    }
    assert_eq!(scan.entries, sequential);
    let overlays = policy::links(
        &root,
        Profile::Guarded,
        &[root.clone()],
        &[],
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(overlays, vec![root.join("tree/20/deeper/6/external")]);
    if !available(&f) {
        return;
    }
    let s = f.prepare(Profile::Guarded, &[root.clone()], &[]).unwrap();
    run(
        &s,
        r#"
        ! cat tree/3/deeper/.cortex/secret
        ! touch tree/9/.git/config
        test -z "$(cat tree/12/.cortex-workspace-dev.json)"
        ! sh -c 'echo no > tree/20/deeper/6/external'
        echo yes > tree/5/internal
    "#,
        &[],
    );
    assert_eq!(fs::read_to_string(f.0.join("shared")).unwrap(), "outside");
    assert_eq!(fs::read_to_string(root.join("source")).unwrap(), "yes\n");
}
