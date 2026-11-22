//! Owner SSH config copies and explicitly opted-in desktop askpass; no credential capture.
use super::*;
use std::{
    collections::{BTreeSet, VecDeque},
    ffi::{CString, OsStr},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};

fn words(text: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escape = false;
    for c in text.chars() {
        if escape {
            word.push(c);
            escape = false;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escape = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None
            } else {
                word.push(c)
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c)
        } else if c == '#' {
            break;
        } else if c.is_whitespace() {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c)
        }
    }
    if quote.is_some() || escape {
        return None;
    }
    if !word.is_empty() {
        out.push(word)
    }
    Some(out)
}
fn glob(pattern: &Path) -> Vec<PathBuf> {
    let Ok(pattern) = CString::new(pattern.as_os_str().as_encoded_bytes()) else {
        return vec![];
    };
    unsafe {
        let mut result = std::mem::zeroed::<libc::glob_t>();
        let status = libc::glob(pattern.as_ptr(), 0, None, &mut result);
        let mut paths = Vec::new();
        if status == 0 && result.gl_pathc <= 100_000 {
            for i in 0..result.gl_pathc {
                paths.push(PathBuf::from(
                    std::ffi::CStr::from_ptr(*result.gl_pathv.add(i))
                        .to_string_lossy()
                        .into_owned(),
                ));
            }
        }
        libc::globfree(&mut result);
        paths
    }
}
fn trusted(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let m = file.metadata()?;
    if !m.is_file() || m.uid() != 0 || m.mode() & 0o022 != 0 || m.len() > 1024 * 1024 {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Ok(None);
    }
    Ok(Some(bytes))
}
fn copies(home: Option<&Path>, system: &Path) -> io::Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut pending = VecDeque::new();
    if let Some(home) = home {
        pending.push_back((home.join(".ssh/config"), home.join(".ssh"), true, 0));
    }
    pending.push_back((
        system.to_owned(),
        system.parent().unwrap().to_owned(),
        false,
        0,
    ));
    let mut seen = BTreeSet::new();
    let mut result = BTreeMap::new();
    while let Some((p, base, checked, depth)) = pending.pop_front() {
        let Ok(real) = p.canonicalize() else { continue };
        if !seen.insert(real.clone()) {
            continue;
        }
        if seen.len() > 100_000 {
            return Err(unavailable("ssh_config_limit"));
        }
        if checked {
            if let Ok(Some(bytes)) = trusted(&real) {
                result.insert(real.clone(), bytes);
            }
        }
        if depth >= 16 {
            continue;
        }
        // Includes are static paths only. Per-connection %/$ expansions cannot be reproduced.
        let Ok(file) = fs::File::open(&real) else {
            continue;
        };
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 1024 * 1024 {
            continue;
        }
        for line in String::from_utf8_lossy(&bytes).lines() {
            let line = line.trim_start();
            let n = line
                .find(|c: char| c.is_whitespace() || c == '=')
                .unwrap_or(line.len());
            if !line[..n].eq_ignore_ascii_case("include") {
                continue;
            }
            let Some(words) =
                words(line[n..].trim_start_matches(|c: char| c.is_whitespace() || c == '='))
            else {
                continue;
            };
            for word in words {
                if word.contains(['%', '$']) {
                    continue;
                }
                let p = if let Some(s) = word.strip_prefix("~/") {
                    home.map(|h| h.join(s)).unwrap_or_else(|| base.join(&word))
                } else {
                    let p = PathBuf::from(&word);
                    if p.is_absolute() {
                        p
                    } else {
                        base.join(p)
                    }
                };
                for p in glob(&p) {
                    pending.push_back((p, base.clone(), true, depth + 1));
                }
            }
        }
    }
    Ok(result)
}
fn display(env: &BTreeMap<OsString, OsString>) -> BTreeMap<OsString, OsString> {
    let mut out = BTreeMap::new();
    for (key, pattern) in [
        ("DISPLAY", r"\A[A-Za-z0-9._/-]*:[0-9]+(\.[0-9]+)?\z"),
        ("WAYLAND_DISPLAY", r"\A[A-Za-z0-9._/-]+\z"),
        ("XAUTHORITY", r"\A/[^\x00\n]*\z"),
        ("XDG_RUNTIME_DIR", r"\A/[^\x00\n]*\z"),
        ("DBUS_SESSION_BUS_ADDRESS", r"\Aunix:[A-Za-z0-9=,/._%\-]+\z"),
    ] {
        if let Some(value) = env.get(OsStr::new(key)) {
            if regex::Regex::new(pattern)
                .unwrap()
                .is_match(&value.to_string_lossy())
            {
                out.insert(key.into(), value.clone());
            }
        }
    }
    if !out.contains_key(OsStr::new("DISPLAY")) && !out.contains_key(OsStr::new("WAYLAND_DISPLAY"))
    {
        out.clear();
    }
    out
}
fn askpass(s: &mut Sandbox, home: Option<&Path>) -> io::Result<()> {
    let base = policy::env_path(&s.environment, "XDG_CONFIG_HOME")
        .or_else(|| home.map(|h| h.join(".config")));
    let enabled = base
        .and_then(|p| fs::read(p.join("cortex/ssh-askpass.json")).ok())
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .is_some_and(|v| v["enabled"] == true);
    if !enabled {
        return Ok(());
    }
    let display = display(&s.environment);
    if display.is_empty() {
        return Ok(());
    }
    let dialog = ["zenity", "kdialog"].into_iter().find_map(|name| {
        ["/usr/local/bin", "/usr/bin", "/bin"]
            .into_iter()
            .map(|dir| Path::new(dir).join(name))
            .find(|p| p.is_file() && fs::metadata(p).is_ok_and(|m| m.mode() & 0o111 != 0))
            .map(|p| (name, p))
    });
    let Some((kind, program)) = dialog else {
        return Ok(());
    };
    let q = |text: &str| format!("'{}'", text.replace('\'', "'\"'\"'"));
    let title = q("Cortex: ssh");
    let (hidden, visible, confirm, notice) = if kind == "zenity" {
        (
            format!("--entry --hide-text --title={title} --text=\"$prompt\""),
            format!("--entry --title={title} --text=\"$prompt\""),
            format!("--question --title={title} --text=\"$prompt\""),
            format!("--info --title={title} --text=\"$prompt\""),
        )
    } else {
        (
            format!("--title {title} --password \"$prompt\""),
            format!("--title {title} --inputbox \"$prompt\""),
            format!("--title {title} --yesno \"$prompt\""),
            format!("--title {title} --msgbox \"$prompt\""),
        )
    };
    let p = q(&program.to_string_lossy());
    let script = format!(
        "#!/bin/sh\nprompt=\u{24}{{1:-Password:}}\ncase \u{24}{{SSH_ASKPASS_PROMPT:-}} in\n confirm) exec {p} {confirm} ;;\n none) exec {p} {notice} ;;\nesac\ncase $prompt in\n *\"(yes/no\"*|*[Uu]sername*) exec {p} {visible} ;;\nesac\nexec {p} {hidden}\n"
    );
    let dir = s.scratch.join("ssh-askpass");
    fs::create_dir(&dir)?;
    let path = dir.join("ssh-askpass");
    fs::write(&path, script)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o500))?;
    s.mount("--ro-bind", &dir, &dir)?;
    s.ssh_environment.extend(display);
    s.ssh_environment
        .insert("SSH_ASKPASS".into(), path.into_os_string());
    s.ssh_environment
        .insert("SSH_ASKPASS_REQUIRE".into(), "force".into());
    Ok(())
}
pub(super) fn configure(s: &mut Sandbox, home: Option<&Path>) -> io::Result<()> {
    let dir = s.scratch.join("ssh-config");
    fs::create_dir(&dir)?;
    for (number, (target, bytes)) in copies(home, Path::new("/etc/ssh/ssh_config"))?
        .into_iter()
        .enumerate()
    {
        let p = dir.join(format!("{number}.conf"));
        fs::write(&p, bytes)?;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o444))?;
        s.mount("--ro-bind", &p, &target)?;
    }
    s.mount("--ro-bind", &dir, &dir)?;
    askpass(s, home)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_display_and_include_words_are_bounded_static_shapes() {
        assert_eq!(
            words("'some file' other\\ file # no"),
            Some(vec!["some file".into(), "other file".into()])
        );
        assert!(words("'unterminated").is_none());
        let mut env = BTreeMap::new();
        env.insert("DISPLAY".into(), ":13".into());
        env.insert("XAUTHORITY".into(), "/tmp/auth\ncode".into());
        env.insert(
            "DBUS_SESSION_BUS_ADDRESS".into(),
            "unix:path=/tmp/bus".into(),
        );
        assert_eq!(display(&env).len(), 2);
        env.insert("DISPLAY".into(), "$(bad)".into());
        assert!(display(&env).is_empty());
    }
    #[test]
    fn trusted_copies_and_askpass_opt_in_use_real_readonly_mounts() {
        use super::super::tests::{capability, run, Fixture};
        let f = Fixture::new();
        let home = f.0.join("home");
        let root = f.root();
        fs::create_dir(home.join(".ssh")).unwrap();
        fs::write(
            home.join(".ssh/config"),
            "Include /etc/ssh/ssh_config\nInclude \"missing *\"\n",
        )
        .unwrap();
        assert!(trusted(&home.join(".ssh/config")).unwrap().is_none());
        // Docker supplies a root-owned, non-secret Include fixture.
        // Trust/copy/mount policy is tested without a remote SSH connection.
        let system = Path::new("/etc/hosts");
        fs::remove_file(home.join(".ssh/config")).unwrap();
        if system.exists() {
            std::os::unix::fs::symlink(system, home.join(".ssh/config")).unwrap();
        }
        let all = copies(Some(&home), system).unwrap();
        // A nested worker namespace can already show host root as nobody.
        // Only the disposable namespace-capable container requires positive root evidence.
        let expected = trusted(system).ok().flatten();
        if std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_some() {
            assert!(
                expected.is_some(),
                "required container lacks a trusted root-owned SSH fixture"
            );
        }
        if let Some(expected) = expected {
            assert_eq!(all.get(&system.canonicalize().unwrap()), Some(&expected));
        } else {
            assert!(all.is_empty());
        }
        let config = f.0.join("config");
        fs::create_dir_all(config.join("cortex")).unwrap();
        let path = config.join("cortex/ssh-askpass.json");
        let mut env = BTreeMap::new();
        env.insert("HOME".into(), home.clone().into_os_string());
        env.insert("XDG_CONFIG_HOME".into(), config.clone().into_os_string());
        env.insert("DISPLAY".into(), ":13".into());
        env.insert("XAUTHORITY".into(), f.0.join("xauth").into_os_string());
        let socket = f.0.join("agent");
        // sockaddr_un has a short path bound; the gateway's private TMPDIR
        // may already be long. Bind through a pinned directory descriptor.
        use std::os::fd::AsRawFd;
        let socket_parent = fs::File::open(&f.0).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(format!(
            "/proc/self/fd/{}/agent",
            socket_parent.as_raw_fd()
        ))
        .unwrap();
        std::os::unix::fs::symlink(&socket, f.0.join("agent-alias")).unwrap();
        env.insert(
            "SSH_AUTH_SOCK".into(),
            f.0.join("agent-alias").into_os_string(),
        );
        fs::write(&path, r#"{"enabled":"true"}"#).unwrap();
        let s = Sandbox::with_environment(&root, Profile::Guarded, &[], &[], false, env.clone());
        if !capability() {
            assert!(s.is_err());
            return;
        }
        let s = s.unwrap();
        assert!(s.ssh_environment.is_empty());
        drop(s);
        fs::write(&path, r#"{"enabled":true}"#).unwrap();
        let s = Sandbox::with_environment(&root, Profile::Guarded, &[], &[], false, env.clone())
            .unwrap();
        let have_dialog = [
            "/usr/bin/zenity",
            "/usr/bin/kdialog",
            "/usr/local/bin/zenity",
            "/usr/local/bin/kdialog",
        ]
        .iter()
        .any(|p| Path::new(p).is_file());
        if std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_some() {
            assert!(have_dialog);
        }
        if have_dialog {
            assert!(!s.ssh_environment.is_empty());
            run(
                &s,
                r#"test "$SSH_ASKPASS_REQUIRE" = force; test "$DISPLAY" = :13
                test -n "$XAUTHORITY"; test -S "$SSH_AUTH_SOCK"
                test -x "$SSH_ASKPASS"; ! chmod 0700 "$SSH_ASKPASS"
                ! sh -c 'echo injected > "$SSH_ASKPASS"'
            "#,
                &[],
            );
            let script = PathBuf::from(s.ssh_environment.get(OsStr::new("SSH_ASKPASS")).unwrap());
            let text = fs::read_to_string(script).unwrap();
            assert!(text.contains("--hide-text") || text.contains("--password"));
            assert!(text.contains("SSH_ASKPASS_PROMPT"));
        }
        if system.exists() {
            run(
                &s,
                "test \"$(stat -c %u /etc/hosts)\" = \"$(id -u)\"; ! touch /etc/hosts",
                &[],
            );
        }
        drop(s);
        env.remove(OsStr::new("DISPLAY"));
        let s = Sandbox::with_environment(&root, Profile::Guarded, &[], &[], false, env).unwrap();
        assert!(s.ssh_environment.is_empty());
    }
}
