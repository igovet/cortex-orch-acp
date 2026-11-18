//! Installed-package acceptance gate (Docker only): `installed-gate` on the host and its
//! container-side halves. Replaces the former Python harness under `packaging/container`.

mod bundled;
pub(crate) mod channel;
mod check_install;
mod config;
mod discovery;
pub(crate) mod ffi;
pub(crate) mod gate;
pub(crate) mod input;
mod inside;
pub(crate) mod native;
mod payload;
mod probe;
mod real_input;
pub(crate) mod seed_large;
mod session;
mod systemd;
pub(crate) mod util;

use crate::json::Res;
use std::path::PathBuf;

pub fn require_container(desktop_user: bool) -> Res<()> {
    // A freshly started container may still be execing its init under load: wait (bounded)
    // for systemd to be PID 1 before deciding this is not a disposable systemd container.
    let started = std::time::Instant::now();
    let mut comm = String::new();
    while started.elapsed() < std::time::Duration::from_secs(60) {
        comm = std::fs::read_to_string("/proc/1/comm").unwrap_or_default();
        if comm.trim() == "systemd" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if !std::path::Path::new("/.dockerenv").is_file() || comm.trim() != "systemd" {
        return Err("internal mode requires a disposable systemd container".into());
    }
    let want = if desktop_user { 1001 } else { 0 };
    if unsafe { libc::getuid() } != want {
        return Err("wrong disposable container user".into());
    }
    Ok(())
}

/// `--name value` flags and positionals; `--name=value` is accepted as well.
pub struct Cli {
    pub flags: Vec<(String, String)>,
    pub positional: Vec<String>,
}

impl Cli {
    pub fn parse(raw: &[String]) -> Res<Cli> {
        let mut cli = Cli {
            flags: Vec::new(),
            positional: Vec::new(),
        };
        let mut iter = raw.iter();
        while let Some(arg) = iter.next() {
            if let Some(name) = arg.strip_prefix("--") {
                match name.split_once('=') {
                    Some((k, v)) => cli.flags.push((k.into(), v.into())),
                    None => cli.flags.push((
                        name.into(),
                        iter.next()
                            .ok_or(format!("--{name} requires a value"))?
                            .clone(),
                    )),
                }
            } else {
                cli.positional.push(arg.clone());
            }
        }
        Ok(cli)
    }

    pub fn flag(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn path(&self, name: &str) -> Option<PathBuf> {
        self.flag(name).map(PathBuf::from)
    }
}

pub const COMMANDS: &[&str] = &[
    "installed-gate",
    "installed-inside",
    "installed-session",
    "installed-seed-large",
    "installed-drag-source",
    "installed-clipboard-owner",
    "check-install",
    "check-install-container",
    "check-install-session",
    "check-real-input",
    "check-real-input-inner",
    "check-systemd",
    "check-systemd-inside",
    "check-systemd-rpc",
    "check-bundled-engines",
    "check-bundled-engines-session",
];

pub fn run(command: &str, raw: &[String]) -> Res<i32> {
    let cli = Cli::parse(raw)?;
    let channel = || channel::Channel::new(cli.flag("channel").unwrap_or("dev"));
    let cycles = || -> Res<u32> {
        let n: u32 = cli
            .flag("close-cycles")
            .unwrap_or("1")
            .parse()
            .map_err(|_| "--close-cycles must be 1 to 10")?;
        if !(1..=10).contains(&n) {
            return Err("--close-cycles must be 1 to 10".into());
        }
        Ok(n)
    };
    let pos = |i: usize| {
        cli.positional
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| "missing argument".to_string())
    };
    match command {
        "installed-gate" => return gate::run(&cli, &channel()?, cycles()?),
        "installed-inside" => inside::run(
            &channel()?,
            pos(0)?,
            pos(1)?,
            cycles()?,
            cli.flag("python-allowed") == Some("yes"),
            cli.flag("only-phase").filter(|p| !p.is_empty()),
        )?,
        "installed-session" => session::run(&channel()?, pos(0)?, pos(1)?, cycles()?)?,
        "installed-seed-large" => match pos(0)? {
            "api" => seed_large::api(&channel()?)?,
            "store" => seed_large::store(&channel()?)?,
            _ => return Err("expected api or store".into()),
        },
        "installed-drag-source" => ffi::drag_source(
            std::path::Path::new(pos(0)?),
            std::path::Path::new(pos(1)?),
            pos(2)? == "x11",
        )?,
        "installed-clipboard-owner" => {
            ffi::clipboard_owner(std::path::Path::new(pos(0)?), pos(1)?)?
        }
        "check-install" => check_install::host(&cli)?,
        "check-install-container" => check_install::container(pos(0)?)?,
        "check-install-session" => check_install::session(pos(0)?)?,
        "check-real-input" => return real_input::host(&cli, &gate::find_root(&cli)?),
        "check-real-input-inner" => real_input::inner(&cli)?,
        "check-systemd" => systemd::host(&cli)?,
        "check-systemd-inside" => systemd::inside()?,
        "check-systemd-rpc" => systemd::rpc_client(pos(0)?, pos(1)?)?,
        "check-bundled-engines" => bundled::host(&cli)?,
        "check-bundled-engines-session" => bundled::session(pos(0)?)?,
        _ => return Err(format!("unknown command {command}")),
    }
    Ok(0)
}
