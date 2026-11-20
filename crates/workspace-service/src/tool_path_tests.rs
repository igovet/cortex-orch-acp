use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

/// A private directory with a fixture home and tool directories; removed on drop.
pub(crate) struct Scratch(pub(crate) PathBuf);
impl Scratch {
    pub(crate) fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("tool-path-{tag}-{}", nonce()));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir(p.join("home")).unwrap();
        Self(p)
    }
    pub(crate) fn dir(&self, relative: &str) -> PathBuf {
        let d = self.0.join(relative);
        fs::create_dir_all(&d).unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
        d
    }
    /// A shell named `name` (its name selects the flags) running `body` under /bin/sh.
    pub(crate) fn script_shell(&self, name: &str, body: &str) -> PathBuf {
        let p = self.dir("shells").join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A real POSIX shell reached through a link named `sh`: it reads `$HOME/.profile`
/// as a login shell does, so the fixture's startup file sets the PATH.
pub(crate) fn login_shell(s: &Scratch, profile: &str) -> PathBuf {
    let link = s.dir("real-shell").join("sh");
    symlink("/bin/sh", &link).unwrap();
    fs::write(s.0.join("home/.profile"), profile).unwrap();
    link
}

#[test]
fn markers_pick_the_value_out_of_noisy_startup_output() {
    let (b, e) = ("<B1>", "<E1>");
    let noisy = b"Welcome!\n<B1>banner without end\nmotd \x1b[1mbold\x1b[0m\n\n<B1>/a/bin:/usr/bin<E1>\ntrailing goodbye\n";
    assert_eq!(parse(noisy, b, e).as_deref(), Some("/a/bin:/usr/bin"));
    assert_eq!(parse(b"no markers at all", b, e), None);
    assert_eq!(
        parse(b"<B1>/a/bin", b, e),
        None,
        "an unterminated value is not accepted"
    );
    assert_eq!(
        parse(b"<B1>/a\n/b<E1>", b, e),
        None,
        "a value never spans lines"
    );
    assert_eq!(parse(b"<B1><E1>", b, e), None);
}

#[test]
fn the_flags_run_startup_files_for_each_supported_shell_and_split_the_markers() {
    for name in ["bash", "zsh", "sh", "dash", "ksh", "mksh"] {
        let a = invocation(Path::new(&format!("/bin/{name}")), "BEGINMARK", "ENDMARK").unwrap();
        assert_eq!(&a[..3], ["-l", "-i", "-c"], "{name}");
        assert!(a[3].contains("\"$PATH\""));
        assert!(!a[3].contains("BEGINMARK") && !a[3].contains("ENDMARK"));
    }
    let fish = invocation(Path::new("/usr/bin/fish"), "BEGINMARK", "ENDMARK").unwrap();
    assert_eq!(&fish[..3], ["--login", "--interactive", "--command"]);
    assert!(fish[3].contains("(string join : $PATH)"));
    assert_eq!(
        invocation(Path::new("/bin/tcsh"), "B", "E").unwrap_err(),
        "unsupported_shell"
    );
}

#[test]
fn a_real_login_shell_reports_the_path_its_startup_file_sets() {
    let s = Scratch::new("capture");
    let tools = s.dir("home/.nvm/versions/node/v24.1.0/bin");
    let shell = login_shell(
        &s,
        &format!(
            "echo 'Welcome to the fixture'\nprintf '__CORTEX_TOOL_PATH_BEGIN_fake__\\n'\nexport PATH={}:$PATH:relative/bin\necho done-profile\n",
            tools.display()
        ),
    );
    let home = s.0.join("home");
    let path = capture(&shell, &home, OsStr::new("/usr/bin:/bin"), CAPTURE_TIMEOUT).unwrap();
    assert!(path.starts_with(&format!("{}:", tools.display())), "{path}");
    let d = detect(
        &shell,
        &home,
        OsStr::new("/usr/bin:/bin"),
        &[],
        CAPTURE_TIMEOUT,
    );
    assert_eq!(d.source, Source::Shell);
    assert_eq!(d.reason, None);
    assert_eq!(d.shell.as_deref(), Some("sh"));
    assert_eq!(d.directories[0], tools);
    assert!(d.directories.contains(&PathBuf::from("/usr/bin")));
    assert!(d.rejected.contains(&Rejected {
        entry: "relative/bin".into(),
        reason: "relative"
    }));
}

#[test]
fn a_hung_shell_is_killed_with_its_process_group_and_the_fallback_records_why() {
    let s = Scratch::new("timeout");
    let pid_file = s.0.join("background.pid");
    let shell = s.script_shell(
        "zsh",
        &format!("sleep 30 &\necho $! > {}\nsleep 30", pid_file.display()),
    );
    let home = s.0.join("home");
    let local = s.dir("home/.local/bin");
    let started = Instant::now();
    let d = detect(
        &shell,
        &home,
        OsStr::new("/usr/bin:/bin"),
        &[],
        Duration::from_millis(500),
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(d.source, Source::Fallback);
    assert_eq!(d.reason, Some("timeout"));
    assert_eq!(d.shell.as_deref(), Some("zsh"));
    assert!(d.directories.contains(&local), "{:?}", d.directories);
    // The background child the startup file left running is gone too.
    let pid: libc::pid_t = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_ne!(
        unsafe { libc::kill(pid, 0) },
        0,
        "background child survived"
    );
}

#[test]
fn a_failed_or_unsupported_shell_falls_back_with_the_reason() {
    let s = Scratch::new("failed");
    let home = s.0.join("home");
    let failing = s.script_shell("bash", "echo 'rc error' >&2\nexit 1");
    let d = detect(
        &failing,
        &home,
        OsStr::new("/usr/bin:/bin"),
        &[],
        CAPTURE_TIMEOUT,
    );
    assert_eq!((d.source, d.reason), (Source::Fallback, Some("no_marker")));
    let other = s.script_shell("tcsh", "exit 0");
    let d = detect(
        &other,
        &home,
        OsStr::new("/usr/bin:/bin"),
        &[],
        CAPTURE_TIMEOUT,
    );
    assert_eq!(
        (d.source, d.reason),
        (Source::Fallback, Some("unsupported_shell"))
    );
    let missing = s.0.join("shells/fish");
    let d = detect(
        &missing,
        &home,
        OsStr::new("/usr/bin:/bin"),
        &[],
        CAPTURE_TIMEOUT,
    );
    assert_eq!(
        (d.source, d.reason),
        (Source::Fallback, Some("spawn_failed"))
    );
    assert!(d.directories.contains(&PathBuf::from("/usr/bin")));
}

#[test]
fn fallback_adds_existing_well_known_directories_and_nvm_default() {
    let s = Scratch::new("fallback");
    let home = s.0.join("home");
    let cargo = s.dir("home/.cargo/bin");
    let old = s.dir("home/.nvm/versions/node/v22.3.0/bin");
    let newer = s.dir("home/.nvm/versions/node/v24.14.1/bin");
    let older24 = s.dir("home/.nvm/versions/node/v24.2.0/bin");
    s.dir("home/.nvm/alias");
    fs::write(home.join(".nvm/alias/default"), "24\n").unwrap();
    let d = fallback(&home, OsStr::new("/usr/bin"), &[], "timeout");
    assert_eq!(d.directories[0], PathBuf::from("/usr/bin"));
    assert!(d.directories.contains(&cargo));
    assert!(d.directories.contains(&newer));
    assert!(!d.directories.contains(&old) && !d.directories.contains(&older24));
    assert!(
        !d.directories.iter().any(|p| p.ends_with(".volta/bin")),
        "only existing ones"
    );
    // A named alias is followed once; `node` means the newest installed.
    fs::write(home.join(".nvm/alias/default"), "lts/jod\n").unwrap();
    s.dir("home/.nvm/alias/lts");
    fs::write(home.join(".nvm/alias/lts/jod"), "v22.3.0\n").unwrap();
    assert_eq!(nvm_default(&home), Some(old));
    fs::write(home.join(".nvm/alias/default"), "node").unwrap();
    assert_eq!(nvm_default(&home), Some(newer));
}

#[test]
fn validation_rejects_unsafe_entries_dedupes_in_order_and_caps() {
    let s = Scratch::new("validate");
    let good = s.dir("good/bin");
    let alias = s.0.join("alias-bin");
    symlink(&good, &alias).unwrap();
    let open = s.dir("open");
    fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).unwrap();
    let sticky = s.dir("sticky");
    fs::set_permissions(&sticky, fs::Permissions::from_mode(0o1777)).unwrap();
    let private = s.dir("state/cortex-workspace");
    let store = s.dir("project/.cortex/bin");
    let file = s.0.join("a-file");
    fs::write(&file, "").unwrap();
    let text = |p: &Path| p.to_string_lossy().into_owned();
    let (kept, rejected) = normalise(
        [
            text(&good),
            "relative/bin".into(),
            "".into(),
            text(&s.0.join("missing")),
            text(&open),
            text(&sticky),
            text(&private.join("tools")),
            text(&private),
            text(&store),
            text(&file),
            text(&alias),
            text(&good),
            format!("{}/../good/bin", s.0.display()),
        ],
        &[private.clone()],
    );
    assert_eq!(kept, vec![good.clone(), sticky]);
    let reasons: Vec<_> = rejected.iter().map(|r| r.reason).collect();
    assert_eq!(
        reasons,
        [
            "relative",
            "relative",
            "missing",
            "world_writable",
            "missing",
            "private_state",
            "private_state",
            "not_a_directory",
            "invalid"
        ]
    );
    // A project folder and private state cannot be added by the owner either.
    let project = s.dir("project");
    assert_eq!(
        check_extra(&text(&s.dir("project/tools")), &[project.clone()]),
        Err("project_folder")
    );
    assert_eq!(
        check_extra(&format!("{}/", good.display()), &[project]),
        Ok(text(&good))
    );
    assert_eq!(check_extra("/", &[]), Err("invalid"));
    // At most MAX_ENTRIES survive; the rest are reported as over the limit.
    let many: Vec<String> = (0..MAX_ENTRIES + 6)
        .map(|i| text(&s.dir(&format!("many/{i}"))))
        .collect();
    let (kept, rejected) = normalise(many, &[]);
    assert_eq!(kept.len(), MAX_ENTRIES);
    assert_eq!(rejected.len(), 6);
    assert!(rejected.iter().all(|r| r.reason == "limit"));
}

#[test]
fn saved_extra_directories_are_read_bounded_and_without_links() {
    let s = Scratch::new("saved");
    let state = s.dir("state");
    fs::write(
        state.join("machine.json"),
        serde_json::to_vec(&json!({"name":"x","toolPathExtra":["/opt/a", 7, "/opt/b"]})).unwrap(),
    )
    .unwrap();
    assert_eq!(saved_extra(&state), ["/opt/a", "/opt/b"]);
    let linked = s.dir("linked");
    symlink(state.join("machine.json"), linked.join("machine.json")).unwrap();
    assert!(saved_extra(&linked).is_empty());
    assert!(saved_extra(&s.0.join("absent")).is_empty());
}

/// The observed failure: a shell path without `~/.cargo/bin` or the nvm Node left
/// `cargo`/`npm` unfound (exit 127, `execvp npm`). The usual per-user tool folders
/// that exist now fill gaps after the shell's own entries and the owner's extra
/// folders, before the system folders; an entry the shell already has keeps its place.
#[test]
fn usual_tool_folders_fill_gaps_after_the_shell_path_and_for_unstarted_processes() {
    let s = Scratch::new("usual");
    let home = s.0.join("home");
    let cargo = s.dir("home/.cargo/bin");
    let local = s.dir("home/.local/bin");
    let node = s.dir("home/.nvm/versions/node/v24.1.0/bin");
    s.dir("home/.nvm/alias");
    fs::write(home.join(".nvm/alias/default"), "24\n").unwrap();
    let extra = s.dir("extra");
    let state = State {
        home: home.clone(),
        private: vec![],
        detected: Detected {
            directories: vec![local.clone(), PathBuf::from("/usr/bin")],
            source: Source::Shell,
            shell: Some("bash".into()),
            reason: None,
            read_at: 0,
            rejected: vec![],
        },
        extra: vec![extra.clone()],
    };
    let resolved = resolved_in(&state);
    let order: Vec<_> = resolved.iter().map(|(p, o)| (p.clone(), *o)).collect();
    assert_eq!(order[0], (local.clone(), "shell"));
    assert_eq!(order[1], (PathBuf::from("/usr/bin"), "shell"));
    assert_eq!(order[2], (extra.clone(), "extra"));
    assert_eq!(order[3], (cargo.clone(), "fallback"));
    assert_eq!(order[4], (node.clone(), "fallback"));
    assert!(order[5..].iter().all(|(_, o)| *o == "system"), "{order:?}");
    assert_eq!(
        order.iter().filter(|(p, _)| *p == local).count(),
        1,
        "an entry the shell has is not repeated"
    );
    // An incoming ACP agent never started the shell read: its client's PATH, then
    // the same usual folders, then the system folders.
    let path = inherited_with_usual(&home, OsStr::new("/usr/bin"), &[]).unwrap();
    let entries: Vec<_> = std::env::split_paths(&path).collect();
    assert_eq!(entries[0], PathBuf::from("/usr/bin"));
    assert!(entries.contains(&cargo) && entries.contains(&node) && entries.contains(&local));
    assert!(entries.contains(&PathBuf::from("/usr/local/bin")));
    // fnm's default version is found in its data directory.
    let fnm = s.dir("home/.fnm/aliases/default/bin");
    if std::env::var_os("FNM_DIR").is_none()
        && !std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("fnm/aliases/default/bin")
            .is_dir()
    {
        assert_eq!(fnm_default(&home), Some(fnm.clone()));
        assert!(usual(&home).contains(&fnm));
    }
}
