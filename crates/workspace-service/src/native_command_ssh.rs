//! Explicitly opted-in desktop askpass for SSH in confined commands. SSH reads the
//! owner's real configuration and agent; no configuration copy or credential capture.
use super::*;
use std::{ffi::OsStr, os::unix::fs::{MetadataExt, PermissionsExt}};

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

    s.ssh_environment.extend(display);
    s.ssh_environment
        .insert("SSH_ASKPASS".into(), path.into_os_string());
    s.ssh_environment
        .insert("SSH_ASKPASS_REQUIRE".into(), "force".into());
    Ok(())
}
pub(super) fn configure(s: &mut Sandbox, home: Option<&Path>) -> io::Result<()> {
    askpass(s, home)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_display_words_are_bounded_static_shapes() {
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
}
