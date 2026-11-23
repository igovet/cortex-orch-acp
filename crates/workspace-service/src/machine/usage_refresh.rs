//! Plan-window refresh for Codex and Claude, on demand when settings are read.
//!
//! A settings read never waits for it. Each read asks for one pass, which runs in
//! the background unless a pass is already running or one started less than
//! [`PASS_FLOOR`] ago; nothing runs between reads, so with no window reading the
//! settings (nothing on screen shows the limits) no engine CLI is ever started.
//! Inside a pass each connection is read at most once per five minutes
//! (`usage_refresh::refresh`), one after another, so connections are staggered
//! rather than read at once. The app's own settings reads are lined up with that
//! five-minute snapshot, so each of them finds a due connection and the next one,
//! moments later, shows its fresh limits. Both reads are free and start no model
//! turn: Claude Code's local `/usage` and the Codex app server's
//! `account/rateLimits/read`. Each runs the verified native CLI of the qualified
//! adapter with the user's real `HOME` and the connection's own native directory
//! (`CLAUDE_CONFIG_DIR`/`CODEX_HOME`): the account a session uses, never a copied
//! credential or a login. Nothing is written when no verified CLI exists.
use crate::{
    config_read_model::{usage_refresh, ConfigurationReader},
    native_routes::RouteEnvironment,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{mpsc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

/// Shortest time between the starts of two passes for one state root, however
/// often the settings are read.
const PASS_FLOOR: Duration = Duration::from_secs(15);
/// One read, including the CLI's exit.
const RUN_BUDGET: Duration = Duration::from_secs(20);
const OFF: &str = "CORTEX_WORKSPACE_USAGE_REFRESH";
const CLAUDE_ARGS: [&str; 7] = [
    "-p",
    "/usage",
    "--output-format",
    "json",
    "--restricted",
    "--strict-mcp-config",
    "--no-session-persistence",
];

#[derive(Default)]
struct Slot {
    running: bool,
    started: Option<Instant>,
}
fn passes() -> std::sync::MutexGuard<'static, BTreeMap<PathBuf, Slot>> {
    static PASSES: OnceLock<Mutex<BTreeMap<PathBuf, Slot>>> = OnceLock::new();
    PASSES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
fn now() -> Option<f64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64())
}
/// Whether a settings read starts a pass, given when the last one started.
fn pass_due(running: bool, since_start: Option<Duration>) -> bool {
    !running && since_start.is_none_or(|d| d >= PASS_FLOOR)
}

/// Note a settings read: start one background pass when one is due. Returns at once.
pub(super) fn ensure(config: &crate::Config, env: &RouteEnvironment) {
    if std::env::var(OFF).is_ok_and(|v| matches!(v.trim(), "off" | "0" | "false" | "no")) {
        return;
    }
    let key = env.state.clone();
    {
        let mut table = passes();
        let slot = table.entry(key.clone()).or_default();
        if !pass_due(slot.running, slot.started.map(|s| s.elapsed())) {
            return;
        }
        slot.running = true;
        slot.started = Some(Instant::now());
    }
    let done = |key: &PathBuf| {
        if let Some(slot) = passes().get_mut(key) {
            slot.running = false;
        }
    };
    let (config, env) = (config.clone(), env.clone());
    let spawned = std::thread::Builder::new()
        .name("usage-refresh".into())
        .spawn({
            let key = key.clone();
            move || {
                let _ =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pass(&config, &env)));
                done(&key);
            }
        });
    if spawned.is_err() {
        done(&key);
    }
}

fn pass(config: &crate::Config, env: &RouteEnvironment) {
    let Ok(reader) = ConfigurationReader::new(env.clone()) else {
        return;
    };
    for engine in ["claude-agent-acp", "codex-acp"] {
        let Ok(targets) = reader.usage_targets(engine) else {
            continue;
        };
        let mut cli: Option<Option<PathBuf>> = None;
        for (id, directory) in targets {
            let Some(now) = now() else { return };
            if usage_refresh::checked_recently(&env.state, &id, now) {
                continue;
            }
            // Resolve (and verify) the CLI once per pass, and only when a read is due.
            let Some(binary) = cli
                .get_or_insert_with(|| super::engine_connection::native_cli(config, engine))
                .clone()
            else {
                break;
            };
            let cwd = env.state.join("cortex/quota/cli-cwd");
            let _ = usage_refresh::refresh(&env.state, &id, now, || {
                cortex_core::store::locks::private_directory(&cwd, true).ok()?;
                if engine == "codex-acp" {
                    let response = codex(&binary, &env.home, &directory, &cwd, None)?;
                    usage_refresh::parse_codex(&response, now)
                } else {
                    let out = claude(&binary, &env.home, &directory, &cwd, None)?;
                    usage_refresh::parse_claude(&out, now)
                }
            });
        }
    }
}

/// Cleared environment with only the native account's own variables, an empty
/// private working directory and its own process group.
fn command(
    binary: &Path,
    home: &Path,
    cwd: &Path,
    view: Option<&cortex_engine::filesystem_view::Confinement>,
) -> Option<Command> {
    let path: Vec<_> = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .filter(|p| p.is_absolute())
                .collect()
        })
        .unwrap_or_default();
    let mut command = match view {
        Some(view) => view.command(binary, std::iter::empty::<&str>()),
        None => Command::new(binary),
    };
    command
        .env_clear()
        .envs(view.map(|v| v.environment().clone()).unwrap_or_default())
        .env("HOME", home)
        .env("PATH", std::env::join_paths(path).ok()?)
        .current_dir(cwd)
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .process_group(0);
    for key in ["LANG", "LC_ALL"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    Some(command)
}
/// End the child and anything else in its process group.
fn end(child: &mut Child) {
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

/// One bounded `claude -p /usage` run: stdout of a successful exit, else `None`.
pub(crate) fn claude(
    binary: &Path,
    home: &Path,
    directory: &Path,
    cwd: &Path,
    view: Option<&cortex_engine::filesystem_view::Confinement>,
) -> Option<Vec<u8>> {
    let mut command = command(binary, home, cwd, view)?;
    command
        .args(CLAUDE_ARGS)
        .env("CLAUDE_CONFIG_DIR", directory)
        // Reset times print as UTC, so no time zone table is needed to read them.
        .env("TZ", "UTC")
        .stdin(Stdio::null());
    #[cfg(test)]
    crate::native_account_fixture::on_exec(&mut command, "usage", home).ok()?;
    let deadline = Instant::now() + RUN_BUDGET;
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, received) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let read = (&mut stdout)
            .take(usage_refresh::MAX_OUTPUT as u64 + 1)
            .read_to_end(&mut out);
        let _ = sender.send(read.ok().map(|_| out));
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => break None,
        }
    };
    end(&mut child);
    let left = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_secs(1));
    let out = received.recv_timeout(left).ok().flatten()?;
    (status?.success() && out.len() <= usage_refresh::MAX_OUTPUT).then_some(out)
}

/// One bounded `codex app-server` exchange: `initialize`, then
/// `account/rateLimits/read`. The response to that request, else `None`. The App
/// Server carries the same non-persistent layers as every Cortex-launched one, so
/// the owner's plugin marketplaces are not synced (a `git ls-remote` to each
/// configured remote source) every time the limits are read.
pub(crate) fn codex(
    binary: &Path,
    home: &Path,
    directory: &Path,
    cwd: &Path,
    view: Option<&cortex_engine::filesystem_view::Confinement>,
) -> Option<Value> {
    let mut command = command(binary, home, cwd, view)?;
    command
        .args(crate::native_codex_runtime::BASE)
        .arg("app-server")
        .env("CODEX_HOME", directory)
        .stdin(Stdio::piped());
    #[cfg(test)]
    crate::native_account_fixture::on_exec(&mut command, "usage", home).ok()?;
    let deadline = Instant::now() + RUN_BUDGET;
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (sender, received) = mpsc::channel();
    std::thread::spawn(move || {
        let mut total = 0usize;
        for line in BufReader::new(stdout)
            .take(usage_refresh::MAX_OUTPUT as u64)
            .lines()
        {
            let Ok(line) = line else { break };
            total += line.len();
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v["id"] == 2 || total > usage_refresh::MAX_OUTPUT {
                let _ = sender.send(v);
                break;
            }
        }
    });
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"cortex-workspace","version":env!("CARGO_PKG_VERSION")}}}),
        json!({"jsonrpc":"2.0","method":"initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"account/rateLimits/read","params":{"excludeResetCreditDetails":true}}),
    ];
    let mut stdin = child.stdin.take()?;
    let written = requests
        .iter()
        .try_for_each(|r| writeln!(stdin, "{r}"))
        .and_then(|_| stdin.flush());
    let response = written.ok().and_then(|_| {
        received
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()
    });
    drop(stdin);
    end(&mut child);
    response.filter(|v| v["id"] == 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_pass_runs_only_on_a_read_and_never_more_often_than_the_floor() {
        // A connection is read at most every five minutes; passes start at most every 15 seconds.
        assert_eq!(PASS_FLOOR, Duration::from_secs(15));
        assert_eq!(usage_refresh::REFRESH_AFTER_SECONDS, 300.0);
        // The first read starts one; a read while it runs, or within the floor of its start, does not.
        assert!(pass_due(false, None));
        assert!(!pass_due(true, None));
        assert!(!pass_due(true, Some(PASS_FLOOR * 10)));
        assert!(!pass_due(
            false,
            Some(PASS_FLOOR - Duration::from_millis(1))
        ));
        assert!(pass_due(false, Some(PASS_FLOOR)));
    }
    /// Settings read continuously (a window showing the limits), so passes start PASS_FLOOR apart, through the real `refresh` gate: each connection is read
    /// again five minutes and at most one pass (plus the reads before it) after its
    /// last read, never sooner, so what the app shows can stay within about six minutes.
    #[test]
    fn a_connection_is_read_again_within_one_pass_of_five_minutes() {
        let state = std::env::temp_dir().join(format!(
            "cw-usage-cadence-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&state);
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&state)
                .unwrap();
        }
        let state = state.canonicalize().unwrap();
        let ids = ["cn_000000000001", "cn_000000000002"];
        // Each read takes up to 4 s; the next pass starts PASS_FLOOR after one ends.
        let (read, start) = (4.0, 1_800_000_000.0);
        let mut reads: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        let mut now = start;
        while now < start + 3600.0 {
            for id in ids {
                if usage_refresh::checked_recently(&state, id, now) {
                    continue;
                }
                let at = now;
                let outcome = usage_refresh::refresh(&state, id, at, || {
                    Some(serde_json::Map::from_iter([(
                        "seven_day".to_owned(),
                        json!({"used":1.0,"resets_at":at + 86_400.0}),
                    )]))
                })
                .unwrap();
                assert_eq!(outcome, usage_refresh::Refresh::Checked { ok: true });
                reads.entry(id).or_default().push(at);
                now += read;
            }
            now += PASS_FLOOR.as_secs_f64();
        }
        let _ = std::fs::remove_dir_all(&state);
        let worst = PASS_FLOOR.as_secs_f64() + read * ids.len() as f64;
        for id in ids {
            let times = &reads[id];
            assert!(times.len() >= 11, "{id}: {times:?}");
            for gap in times.windows(2).map(|w| w[1] - w[0]) {
                assert!(
                    gap >= usage_refresh::REFRESH_AFTER_SECONDS,
                    "{id} read early: {gap}"
                );
                assert!(
                    gap <= usage_refresh::REFRESH_AFTER_SECONDS + worst,
                    "{id} read late: {gap}"
                );
            }
        }
        // The app's grace after a due snapshot (USAGE_GRACE_MS, 45 s in port/real.ts) covers that lateness.
        assert!(worst <= 45.0);
    }
    /// Opt-in, free: runs the two reads against the owner's installed CLIs and native
    /// login, writing no Cortex state. `CORTEX_USAGE_LIVE_HOME`, `_CLAUDE` and `_CODEX`
    /// name the real HOME and the two verified executables. Not part of the default suite.
    #[test]
    #[ignore]
    fn live_free_reads_parse_with_the_installed_clis() {
        let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
        let (Some(home), Some(claude_cli), Some(codex_cli)) = (
            var("CORTEX_USAGE_LIVE_HOME"),
            var("CORTEX_USAGE_LIVE_CLAUDE"),
            var("CORTEX_USAGE_LIVE_CODEX"),
        ) else {
            return;
        };
        let cwd = std::env::temp_dir().join(format!("cw-usage-live-{}", std::process::id()));
        std::fs::create_dir(&cwd).unwrap();
        let now = now().unwrap();
        let out =
            claude(&claude_cli, &home, &home.join(".claude"), &cwd, None).expect("claude ran");
        let windows = usage_refresh::parse_claude(&out, now).expect("free local /usage");
        let codex_answer =
            codex(&codex_cli, &home, &home.join(".codex"), &cwd, None).expect("codex answered");
        let codex_windows =
            usage_refresh::parse_codex(&codex_answer, now).expect("documented shape");
        let _ = std::fs::remove_dir_all(&cwd);
        let shape = |w: &usage_refresh::Windows| {
            w.iter()
                .map(|(k, v)| format!("{k}={}%", v["used"]))
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!(
            "claude: {}; codex: {}",
            shape(&windows),
            shape(&codex_windows)
        );
        assert!(windows.contains_key("seven_day"));
    }
    /// The limits read starts the App Server without plugins (whose marketplace sync runs a
    /// `git ls-remote` to every configured remote source) and with the rest of the layers every
    /// Cortex-launched App Server carries. A fake binary records its argv; no engine runs.
    #[test]
    fn the_codex_limits_read_starts_the_app_server_without_plugin_sync() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let root = std::env::temp_dir().join(format!("cw-usage-argv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let root = root.canonicalize().unwrap();
        let fake = root.join("codex");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}/argv'\nwhile read -r line; do case \"$line\" in *'\"id\":2'*) echo '{{\"id\":2,\"result\":{{}}}}'; exit 0;; esac; done\n",
                root.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        let response = codex(&fake, &root, &root, &root, None).expect("the fake answered");
        assert_eq!(response["id"], 2);
        let argv: Vec<String> = std::fs::read_to_string(root.join("argv"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        let mut expected: Vec<String> = crate::native_codex_runtime::BASE
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        expected.push("app-server".into());
        assert_eq!(argv, expected);
        assert!(argv
            .windows(2)
            .any(|w| w == ["-c", "features.plugins=false"]));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
