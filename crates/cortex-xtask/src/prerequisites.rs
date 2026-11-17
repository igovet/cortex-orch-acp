//! `prerequisite-probe`: a read-only, no-paid inventory of the native-build prerequisites.
//!
//! Missing items do not fail the inventory: exit 0 means the inventory completed, NOT that
//! a native build is qualified. Child output is discarded: only fixed probe commands, exit
//! codes and present/missing states are printed, never environment or process listings.

use crate::json::{pretty_ascii, Res, Value};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SECONDS: u64 = 8;

fn probe(name: &str, command: &[&str]) -> Value {
    let (code, reason) = match Command::new(command[0]).args(&command[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (127, Some("executable_not_found")),
        Err(_) => (126, Some("execution_unavailable")),
        Ok(mut child) => {
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break (status.code().unwrap_or(1), None),
                    Ok(None) if start.elapsed() < Duration::from_secs(SECONDS) => std::thread::sleep(Duration::from_millis(20)),
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break (124, Some("timeout"));
                    }
                }
            }
        }
    };
    let mut item = Value::obj();
    item.set("name", Value::str(name));
    item.set("command", Value::Arr(command.iter().map(|c| Value::str(*c)).collect()));
    item.set("exit_code", Value::int(i64::from(code)));
    item.set("status", Value::str(if code == 0 { "present" } else { "missing" }));
    if let Some(reason) = reason {
        item.set("reason", Value::str(reason));
    }
    item
}

pub fn run() -> Res<i32> {
    let mut items = vec![
        probe("node", &["node", "--version"]),
        probe("npm", &["npm", "--version"]),
        probe("rustc", &["rustc", "--version"]),
        probe("cargo", &["cargo", "--version"]),
        probe("user-service-manager", &["systemctl", "--user", "show", "--property=Version", "--value"]),
    ];
    for library in ["webkit2gtk-4.1", "gtk+-3.0", "openssl", "ayatana-appindicator3-0.1", "librsvg-2.0", "xdo"] {
        items.push(probe(library, &["pkg-config", "--exists", library]));
    }
    let mut report = Value::obj();
    report.set("items", Value::Arr(items));
    report.set("qualification", Value::str("inventory_only"));
    println!("{}", pretty_ascii(&report)?);
    Ok(0)
}
