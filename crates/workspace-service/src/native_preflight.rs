//! Environment preflight at task start: cheap host facts, no model call.
//!
//! Before the coordinator plans, the runtime records what typical tasks depend on
//! as one `ENVIRONMENT_PREFLIGHT` event, which the coordinator's recovery capsule
//! carries (`environment`): GitHub CLI login, native Codex and Claude logins,
//! whether `sudo` works without a password, free space of the project volume and
//! `/tmp`, display/session availability, Docker, and the project's Git state.
//!
//! Every probe runs in parallel under one total budget ([`BUDGET`]). Logins and
//! sudo are exit codes only: stdin, stdout and stderr are closed, so no credential
//! or account text is read or recorded. An engine CLI is asked only where its
//! native login files already exist (the passive marker settings also reads): such
//! a CLI creates its native home on first run, and a probe must never create an
//! owner's engine home or touch a disposable one. Nothing here ever blocks a task: a missing
//! tool, a failed login or a probe that ran out of time is a recorded fact the
//! coordinator plans around. Host facts are cached per service start ([`warm`])
//! and refreshed at each task start; a probe that times out on refresh reports the
//! cached value with its age. One probe can also be rerun alone ([`recheck`]): an
//! owner action's "check again" and its background recheck (`native_owner_actions`).
use cortex_core::store::executor::StoreExecutor;
use serde_json::{json, Map, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Total wall-clock budget of one preflight, all probes together.
pub const BUDGET: Duration = Duration::from_secs(3);
/// Bytes of `git status` read; the rest is reported as truncated.
const GIT_OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Login,
    Sudo,
    Docker,
}
struct Probe {
    name: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    kind: Kind,
    /// The engine whose native login files gate this probe, and their folder below `HOME`.
    native: Option<(&'static str, &'static str)>,
}
const PROBES: [Probe; 5] = [
    Probe {
        name: "gh_auth",
        program: "gh",
        args: &["auth", "status"],
        kind: Kind::Login,
        native: None,
    },
    Probe {
        name: "codex_login",
        program: "codex",
        args: &["login", "status"],
        kind: Kind::Login,
        native: Some(("codex-acp", ".codex")),
    },
    Probe {
        name: "claude_login",
        program: "claude",
        args: &["auth", "status"],
        kind: Kind::Login,
        native: Some(("claude-agent-acp", ".claude")),
    },
    Probe {
        name: "sudo",
        program: "sudo",
        args: &["-n", "true"],
        kind: Kind::Sudo,
        native: None,
    },
    Probe {
        name: "docker",
        program: "docker",
        args: &["info", "--format", "{{.ServerVersion}}"],
        kind: Kind::Docker,
        native: None,
    },
];

/// What one bounded process run observed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outcome {
    /// The program is not on the owner's tool search path.
    Missing,
    Exit(i32),
    /// Ended by a signal (no exit code).
    Signal,
    /// Still running at the deadline; it was killed.
    Timeout,
    SpawnFailed,
}

/// The recorded fact for one probe outcome. Exit codes only.
fn fact(kind: Kind, outcome: &Outcome) -> Value {
    let status = match (kind, outcome) {
        (_, Outcome::Missing) => "not_installed",
        (_, Outcome::Timeout) => "unknown_timeout",
        (_, Outcome::Signal | Outcome::SpawnFailed) => "unknown",
        (Kind::Login, Outcome::Exit(0)) => "logged_in",
        (Kind::Login, Outcome::Exit(_)) => "not_logged_in",
        (Kind::Sudo, Outcome::Exit(0)) => "passwordless",
        // `sudo -n` fails when a password would be needed (or sudo is not permitted).
        (Kind::Sudo, Outcome::Exit(_)) => "password_needed",
        (Kind::Docker, Outcome::Exit(0)) => "available",
        (Kind::Docker, Outcome::Exit(_)) => "daemon_unavailable",
    };
    let mut v = json!({ "status": status });
    if let Outcome::Exit(code) = outcome {
        v["exit"] = json!(code);
    }
    v
}

struct Cached {
    at: Instant,
    facts: Map<String, Value>,
}
static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

fn environment() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let mut env: Vec<_> = crate::tool_path::environment().into_iter().collect();
    // Never prompt: a probe that would ask for input fails instead.
    for (k, v) in [
        ("GH_PROMPT_DISABLED", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("NO_COLOR", "1"),
    ] {
        env.retain(|(name, _)| name != k);
        env.push((k.into(), v.into()));
    }
    env
}

/// Run `program args` until `deadline`, discarding all output (or keeping up to
/// `GIT_OUTPUT_LIMIT` bytes of stdout when `capture`).
async fn run(
    program: Option<PathBuf>,
    args: &[&str],
    cwd: Option<&Path>,
    deadline: tokio::time::Instant,
    capture: bool,
) -> (Outcome, Vec<u8>) {
    use tokio::io::AsyncReadExt;
    let Some(program) = program else {
        return (Outcome::Missing, Vec::new());
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env_clear()
        .envs(environment())
        .stdin(Stdio::null())
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let Ok(mut child) = command.spawn() else {
        return (Outcome::SpawnFailed, Vec::new());
    };
    let group = child.id();
    let mut out = Vec::new();
    let stdout = child.stdout.take();
    let work = async {
        if let Some(stdout) = stdout {
            let _ = stdout
                .take(GIT_OUTPUT_LIMIT as u64 + 1)
                .read_to_end(&mut out)
                .await;
        }
        child.wait().await
    };
    let status = tokio::time::timeout_at(deadline, work).await;
    let outcome = match status {
        Ok(Ok(status)) => match status.code() {
            Some(code) => Outcome::Exit(code),
            None => Outcome::Signal,
        },
        Ok(Err(_)) => Outcome::SpawnFailed,
        Err(_) => Outcome::Timeout,
    };
    // A probe that ran out of time takes everything it started with it. Its
    // group leader is not reaped yet, so the group id cannot have been reused.
    if let (Outcome::Timeout, Some(pid)) = (&outcome, group) {
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
    (outcome, out)
}

fn locate(program: &str) -> Option<PathBuf> {
    crate::tool_path::locate(program).map(|(p, _)| p)
}

/// The bounded Git facts of `git status --porcelain=v2 --branch` output.
pub(crate) fn parse_git_status(text: &str, truncated: bool) -> Value {
    let mut head = Value::Null;
    let mut upstream = Value::Null;
    let (mut ahead, mut behind) = (Value::Null, Value::Null);
    let (mut changed, mut untracked, mut conflicted) = (0u64, 0u64, 0u64);
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("# branch.head ") {
            head = json!(v.chars().take(200).collect::<String>());
        } else if let Some(v) = line.strip_prefix("# branch.upstream ") {
            upstream = json!(v.chars().take(200).collect::<String>());
        } else if let Some(v) = line.strip_prefix("# branch.ab ") {
            let mut parts = v.split_whitespace();
            ahead = parts
                .next()
                .and_then(|a| a.trim_start_matches('+').parse::<u64>().ok())
                .map_or(Value::Null, |n| json!(n));
            behind = parts
                .next()
                .and_then(|b| b.trim_start_matches('-').parse::<u64>().ok())
                .map_or(Value::Null, |n| json!(n));
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            changed += 1;
        } else if line.starts_with("u ") {
            conflicted += 1;
        } else if line.starts_with("? ") {
            untracked += 1;
        }
    }
    let detached = head == json!("(detached)");
    json!({"status":"repository","branch":if detached {Value::Null} else {head},"detached":detached,
        "upstream":upstream,"ahead":ahead,"behind":behind,"changed":changed,"untracked":untracked,
        "conflicted":conflicted,"clean":changed+untracked+conflicted==0 && !truncated,
        "counts_complete":!truncated})
}

async fn git(root: &Path, deadline: tokio::time::Instant) -> Value {
    let (outcome, out) = run(
        locate("git"),
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal",
        ],
        Some(root),
        deadline,
        true,
    )
    .await;
    match outcome {
        Outcome::Exit(0) => {
            let truncated = out.len() > GIT_OUTPUT_LIMIT;
            let text = String::from_utf8_lossy(&out[..out.len().min(GIT_OUTPUT_LIMIT)]);
            parse_git_status(&text, truncated)
        }
        Outcome::Exit(128) => json!({"status":"not_a_repository","exit":128}),
        Outcome::Exit(code) => json!({"status":"failed","exit":code}),
        other => fact(Kind::Login, &other),
    }
}

/// Free space of the file system holding `path`, for unprivileged users.
pub(crate) fn free_space(path: &Path) -> Value {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return json!({"status":"unknown"});
    };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return json!({"status":"unknown"});
    }
    let free = (s.f_bavail as u128 * s.f_frsize as u128).min(u64::MAX as u128) as u64;
    let total = (s.f_blocks as u128 * s.f_frsize as u128).min(u64::MAX as u128) as u64;
    json!({"free_bytes":free,"free_gib":(free as f64 / 1_073_741_824.0 * 10.0).round() / 10.0,
        "total_gib":(total as f64 / 1_073_741_824.0 * 10.0).round() / 10.0})
}

/// Display and session variables of the service, by presence (values are short
/// non-secret names such as `:0`, clipped).
pub(crate) fn display(get: impl Fn(&str) -> Option<String>) -> Value {
    let short = |v: Option<String>| -> Value {
        v.filter(|s| !s.is_empty()).map_or(Value::Null, |s| {
            json!(s.chars().take(40).collect::<String>())
        })
    };
    let x11 = short(get("DISPLAY"));
    let wayland = short(get("WAYLAND_DISPLAY"));
    let available = !x11.is_null() || !wayland.is_null();
    json!({"available":available,"x11_display":x11,"wayland_display":wayland,
        "session_type":short(get("XDG_SESSION_TYPE")),
        "note":if available {"a graphical session is visible to the service"} else {"no display variables in the service environment; use a private Xvfb for GUI work"}})
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The recorded fact of an engine whose native login files are absent or unusable,
/// so its CLI is not run (`marker` from [`crate::config_read_model::native_login_marker`]).
pub(crate) fn without_native_login(marker: &str) -> Option<Value> {
    match marker {
        "configured" => None,
        "authorization_required" => {
            Some(json!({"status":"not_logged_in","source":"no_native_login_files"}))
        }
        _ => Some(json!({"status":"unknown","source":"native_home_unusable"})),
    }
}

/// The gate of `probe` in `home`: a fact when its engine CLI must not run.
fn gate(probe: &Probe, home: Option<&Path>) -> Option<Value> {
    let (engine, folder) = probe.native?;
    let marker = match home {
        Some(home) => crate::config_read_model::native_login_marker(engine, &home.join(folder)),
        None => "invalid",
    };
    without_native_login(marker)
}

/// Host facts (logins, sudo, Docker), refreshed now within `deadline`. A probe that
/// timed out reuses its cached value, marked with its age.
async fn host(deadline: tokio::time::Instant) -> Map<String, Value> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let runs = PROBES.iter().map(|p| {
        let gated = gate(p, home.as_deref());
        async move {
            let result = match gated {
                Some(value) => Err(value),
                None => Ok(run(locate(p.program), p.args, None, deadline, false)
                    .await
                    .0),
            };
            (p, result)
        }
    });
    let results = futures::future::join_all(runs).await;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let mut facts = Map::new();
    let mut fresh = cache.as_ref().map(|c| c.facts.clone()).unwrap_or_default();
    for (probe, result) in results {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(value) => {
                fresh.insert(probe.name.into(), value.clone());
                facts.insert(probe.name.into(), value);
                continue;
            }
        };
        let value = match (
            &outcome,
            cache
                .as_ref()
                .and_then(|c| c.facts.get(probe.name).map(|v| (c.at, v))),
        ) {
            (Outcome::Timeout, Some((at, cached))) => {
                let mut v = cached.clone();
                v["stale_seconds"] = json!(at.elapsed().as_secs());
                v["refresh"] = json!("timed_out");
                v
            }
            _ => {
                let v = fact(probe.kind, &outcome);
                if outcome != Outcome::Timeout {
                    fresh.insert(probe.name.into(), v.clone());
                }
                v
            }
        };
        facts.insert(probe.name.into(), value);
    }
    *cache = Some(Cached {
        at: Instant::now(),
        facts: fresh,
    });
    facts
}

/// One host probe (`gh_auth`, `codex_login`, `claude_login`, `sudo` or `docker`) run
/// again now, alone, within [`BUDGET`]: an owner action's "check again" and its
/// background recheck. Exit code only, like the whole preflight; the cache keeps the
/// fresh value. `None` names no probe.
pub(crate) async fn recheck(name: &str) -> Option<Value> {
    let probe = PROBES.iter().find(|p| p.name == name)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    recheck_at(name, locate(probe.program), home.as_deref()).await
}

/// [`recheck`] with an explicit program and native home (tests pass fakes).
pub(crate) async fn recheck_at(
    name: &str,
    program: Option<PathBuf>,
    home: Option<&Path>,
) -> Option<Value> {
    let probe = PROBES.iter().find(|p| p.name == name)?;
    let value = match gate(probe, home) {
        Some(value) => value,
        None => {
            let deadline = tokio::time::Instant::now() + BUDGET;
            fact(
                probe.kind,
                &run(program, probe.args, None, deadline, false).await.0,
            )
        }
    };
    if value["status"] != "unknown_timeout" {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        match cache.as_mut() {
            Some(cached) => {
                cached.facts.insert(probe.name.into(), value.clone());
            }
            None => {
                let mut facts = Map::new();
                facts.insert(probe.name.into(), value.clone());
                *cache = Some(Cached {
                    at: Instant::now(),
                    facts,
                });
            }
        }
    }
    Some(value)
}

/// Whether a probe's fact shows the access it checks is there: a login that
/// succeeded, `sudo` that needs no password, a reachable Docker daemon.
pub(crate) fn has_access(fact: &Value) -> bool {
    matches!(
        fact["status"].as_str(),
        Some("logged_in" | "passwordless" | "available")
    )
}

/// Fill the host-fact cache once per service start, in the background. Never fails.
pub fn warm() {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async {
            // Let the owner's tool search path resolve first.
            tokio::time::sleep(Duration::from_secs(2)).await;
            host(tokio::time::Instant::now() + BUDGET).await;
        });
    }
}

/// All facts for a task in `root`, within [`BUDGET`].
pub async fn facts(root: &Path) -> Value {
    let started = tokio::time::Instant::now();
    let deadline = started + BUDGET;
    let (host, git) = tokio::join!(host(deadline), git(root, deadline));
    let mut facts = host;
    facts.insert("git".into(), git);
    facts.insert(
        "disk".into(),
        json!({"project_volume":free_space(root),"tmp":free_space(Path::new("/tmp"))}),
    );
    facts.insert("display".into(), display(|k| std::env::var(k).ok()));
    facts.insert("observed_at_unix".into(), json!(unix_seconds()));
    facts.insert(
        "elapsed_ms".into(),
        json!(started.elapsed().as_millis() as u64),
    );
    facts.insert("budget_ms".into(), json!(BUDGET.as_millis() as u64));
    Value::Object(facts)
}

/// Record the task's preflight before its coordinator plans. Bounded by
/// [`BUDGET`] plus one store write; any failure is ignored (facts, never stops).
pub async fn record(executor: &StoreExecutor, task: &str, root: &Path) {
    let facts = facts(root).await;
    let task = task.to_owned();
    if let Ok(reply) = executor.write(move |db| {
        cortex_core::lifecycle::event_tx(db, &task, "ENVIRONMENT_PREFLIGHT", facts)?;
        Ok(())
    }) {
        let _ = reply.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_become_facts_without_output() {
        assert_eq!(
            fact(Kind::Login, &Outcome::Exit(0)),
            json!({"status":"logged_in","exit":0})
        );
        assert_eq!(
            fact(Kind::Login, &Outcome::Exit(1))["status"],
            "not_logged_in"
        );
        assert_eq!(
            fact(Kind::Login, &Outcome::Missing),
            json!({"status":"not_installed"})
        );
        assert_eq!(
            fact(Kind::Sudo, &Outcome::Exit(0))["status"],
            "passwordless"
        );
        assert_eq!(
            fact(Kind::Sudo, &Outcome::Exit(1))["status"],
            "password_needed"
        );
        assert_eq!(
            fact(Kind::Docker, &Outcome::Exit(1))["status"],
            "daemon_unavailable"
        );
        assert_eq!(
            fact(Kind::Docker, &Outcome::Timeout),
            json!({"status":"unknown_timeout"})
        );
    }

    #[test]
    fn git_status_is_parsed_into_bounded_counts() {
        let text = "# branch.oid abc\n# branch.head dev\n# branch.upstream origin/dev\n# branch.ab +2 -3\n\
1 .M N... 100644 100644 100644 a b src/x.rs\n2 R. N... 100644 100644 100644 a b R100 n\to\n\
u UU N... 1 2 3 4 a b c conflict\n? new.txt\n? other\n";
        let v = parse_git_status(text, false);
        assert_eq!(v["branch"], "dev");
        assert_eq!(v["upstream"], "origin/dev");
        assert_eq!(
            (v["ahead"].as_u64(), v["behind"].as_u64()),
            (Some(2), Some(3))
        );
        assert_eq!(
            (
                v["changed"].as_u64(),
                v["untracked"].as_u64(),
                v["conflicted"].as_u64()
            ),
            (Some(2), Some(2), Some(1))
        );
        assert_eq!(v["clean"], false);
        let v = parse_git_status("# branch.oid (initial)\n# branch.head (detached)\n", true);
        assert_eq!(v["detached"], true);
        assert!(v["branch"].is_null() && v["upstream"].is_null());
        // A truncated listing is never reported clean.
        assert_eq!(
            (v["clean"].as_bool(), v["counts_complete"].as_bool()),
            (Some(false), Some(false))
        );
    }

    #[test]
    fn display_facts_are_presence_and_short_names() {
        let v = display(|k| (k == "DISPLAY").then(|| ":0".to_owned()));
        assert_eq!(
            (v["available"].as_bool(), v["x11_display"].as_str()),
            (Some(true), Some(":0"))
        );
        assert!(v["wayland_display"].is_null());
        let v = display(|_| Some("x".repeat(500)));
        assert_eq!(v["session_type"].as_str().unwrap().len(), 40);
        assert_eq!(display(|_| None)["available"], false);
    }

    #[test]
    fn free_space_reports_bytes_or_unknown() {
        assert!(free_space(Path::new("/tmp"))["free_bytes"]
            .as_u64()
            .is_some());
        assert_eq!(
            free_space(Path::new("/nonexistent/cortex-preflight"))["status"],
            "unknown"
        );
    }

    /// An engine CLI runs only over existing native login files: an empty HOME (a
    /// fresh account or a hermetic test) records the fact without running the CLI,
    /// which would otherwise create its native home there.
    #[test]
    fn engine_logins_are_asked_only_over_existing_native_login_files() {
        use std::os::unix::fs::PermissionsExt;
        let home =
            std::env::temp_dir().join(format!("cortex-preflight-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let [_, codex, claude, sudo, _] = &PROBES;
        assert_eq!(
            gate(claude, Some(&home)),
            Some(json!({"status":"not_logged_in","source":"no_native_login_files"}))
        );
        assert!(!home.join(".claude").exists() && !home.join(".codex").exists());
        assert_eq!(gate(sudo, Some(&home)), None);
        assert_eq!(gate(codex, None).unwrap()["status"], "unknown");
        // A shared (group-writable) native home is unusable, never probed.
        std::fs::create_dir(home.join(".codex")).unwrap();
        std::fs::set_permissions(home.join(".codex"), std::fs::Permissions::from_mode(0o775))
            .unwrap();
        assert_eq!(
            gate(codex, Some(&home)).unwrap()["source"],
            "native_home_unusable"
        );
        std::fs::set_permissions(home.join(".codex"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::write(home.join(".codex/auth.json"), b"{}").unwrap();
        assert_eq!(gate(codex, Some(&home)), None);
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// One probe reruns alone against the program it is given (here a fake `gh`), by
    /// exit code; an unknown name is no probe.
    #[tokio::test]
    async fn a_single_probe_reruns_alone_by_exit_code() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("preflight-recheck-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("gh");
        let state = dir.join("state");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\nexit $(cat '{}')\n", state.display()),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&state, "1").unwrap();
        let fact = recheck_at("gh_auth", Some(fake.clone()), Some(&dir))
            .await
            .unwrap();
        assert_eq!(fact, json!({"status":"not_logged_in","exit":1}));
        assert!(!has_access(&fact));
        std::fs::write(&state, "0").unwrap();
        let fact = recheck_at("gh_auth", Some(fake.clone()), Some(&dir))
            .await
            .unwrap();
        assert!(has_access(&fact));
        assert!(recheck_at("no_such_probe", Some(fake), Some(&dir))
            .await
            .is_none());
        assert!(!has_access(&json!({"status":"password_needed"})));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A probe never outlives the budget: a hanging program is killed at the deadline.
    #[tokio::test]
    async fn a_hanging_probe_is_killed_at_the_deadline() {
        let start = tokio::time::Instant::now();
        let (outcome, _) = run(
            Some(PathBuf::from("/bin/sh")),
            &["-c", "sleep 30"],
            None,
            start + Duration::from_millis(300),
            false,
        )
        .await;
        assert_eq!(outcome, Outcome::Timeout);
        assert!(start.elapsed() < Duration::from_secs(3));
        let (outcome, _) = run(None, &[], None, start + BUDGET, false).await;
        assert_eq!(outcome, Outcome::Missing);
        let (outcome, out) = run(
            Some(PathBuf::from("/bin/sh")),
            &["-c", "echo hi; exit 3"],
            None,
            tokio::time::Instant::now() + BUDGET,
            true,
        )
        .await;
        assert_eq!((outcome, out), (Outcome::Exit(3), b"hi\n".to_vec()));
    }

    /// The whole preflight of a Git fixture finishes within the budget and records
    /// every fact, whatever this host has installed.
    #[tokio::test]
    async fn the_whole_preflight_is_bounded_and_complete() {
        let root = std::env::temp_dir().join(format!("cortex-preflight-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let start = Instant::now();
        let v = facts(&root).await;
        assert!(
            start.elapsed() < BUDGET + Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
        for key in [
            "gh_auth",
            "codex_login",
            "claude_login",
            "sudo",
            "docker",
            "git",
            "disk",
            "display",
        ] {
            assert!(v[key].is_object(), "{key}: {v}");
        }
        assert!(v["git"]["status"].is_string());
        assert!(v.to_string().len() < 2_500, "{v}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
