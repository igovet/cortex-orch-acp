//! Automatic, prompt-free model catalogue refresh for connections the service reads.
//!
//! A connection with no saved model list is observed once in the background, as
//! soon as the first settings read asks for it, and a list older than a day is
//! observed again while the saved one keeps being served. A failed observation is
//! started again by a later read after a short wait that doubles with each failure
//! (5 seconds up to 10 minutes). One bounded attempt per
//! connection at a time, side by side, under the same per-connection refresh lock and the
//! same engine launch shape as the startup first-run pass: no model prompt, no paid
//! call, nothing replayed. The outcome is reported with the settings read, so a
//! screen can wait for it honestly instead of showing a list that is merely absent.
use super::*;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime},
};

/// A saved list older than this is observed again in the background.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// A connection observed well is not started again by a read for this long; it is
/// also the longest wait after failures.
const COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// After a failed observation the next read may start another one this soon, the
/// wait doubling with each further failure up to [`COOLDOWN`]. A first launch whose
/// refresh failed for a passing reason gets its models seconds later, not minutes.
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// How long after its last outcome a connection may be observed again.
fn wait_after(phase: &Phase, failures: u32) -> Duration {
    match phase {
        Phase::Failed(_) => RETRY_AFTER
            .saturating_mul(1u32 << failures.saturating_sub(1).min(16))
            .min(COOLDOWN),
        _ => COOLDOWN,
    }
}
/// One connection's whole attempt, including waiting for another process's refresh of it.
const ATTEMPT: Duration = Duration::from_secs(45);
const OFF: &str = "CORTEX_WORKSPACE_CATALOG_AUTOREFRESH";

#[derive(Clone)]
enum Phase {
    Queued,
    Running,
    Done,
    Failed(String),
}
struct Slot {
    phase: Phase,
    since: Instant,
    /// Failed observations in a row; a success resets it.
    failures: u32,
}
type Key = (PathBuf, String);
fn passes() -> &'static Mutex<BTreeMap<Key, Slot>> {
    static PASSES: OnceLock<Mutex<BTreeMap<Key, Slot>>> = OnceLock::new();
    PASSES.get_or_init(Default::default)
}
fn locked() -> std::sync::MutexGuard<'static, BTreeMap<Key, Slot>> {
    passes().lock().unwrap_or_else(|e| e.into_inner())
}
fn set(key: &Key, phase: Phase) {
    let mut table = locked();
    let before = table.get(key).map_or(0, |s| s.failures);
    let failures = match phase {
        Phase::Failed(_) => before.saturating_add(1),
        Phase::Done => 0,
        Phase::Queued | Phase::Running => before,
    };
    table.insert(
        key.clone(),
        Slot {
            phase,
            since: Instant::now(),
            failures,
        },
    );
}
/// Newest saved list of a connection, by file time. `None`: nothing is saved.
fn newest(env: &RouteEnvironment, id: &str) -> Option<SystemTime> {
    fs::read_dir(env.state.join("cortex/catalog").join(id))
        .ok()?
        .flatten()
        .filter(|f| {
            f.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("cs_") && n.ends_with(".json"))
        })
        .filter_map(|f| f.metadata().ok()?.modified().ok())
        .max()
}
fn safe_reason(reason: &str) -> String {
    if !reason.is_empty()
        && reason.len() <= 96
        && reason
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':')
    {
        reason.to_owned()
    } else {
        "refresh_failed".to_owned()
    }
}
/// What a settings read reports about the background refresh:
/// `{"running": bool, "failed": [{"connectionId", "reason"}]}`. `failed` lists only
/// connections still without any saved list.
pub(crate) fn status(config: &crate::Config) -> Value {
    let Ok((_, env, _, _)) = state(config) else {
        return json!({"running": false, "failed": []});
    };
    let table = locked();
    let mut running = false;
    let mut failed = vec![];
    for ((root, id), slot) in table.iter().filter(|((root, _), _)| *root == env.state) {
        let _ = root;
        match &slot.phase {
            Phase::Queued | Phase::Running => running = true,
            Phase::Failed(reason) if newest(&env, id).is_none() => {
                failed.push(json!({"connectionId": id, "reason": reason}))
            }
            _ => {}
        }
    }
    json!({"running": running, "failed": failed})
}
/// Plan and start the refresh of every connection that needs one, then report
/// `status`. Planning is synchronous, so the reply that triggers a refresh already
/// says it is running. Never fails a read.
pub(crate) fn ensure(config: &crate::Config) -> Value {
    if std::env::var(OFF).is_ok_and(|v| matches!(v.trim(), "off" | "0" | "false" | "no")) {
        return json!({"running": false, "failed": []});
    }
    if let Ok((_, env, _, records)) = state(config) {
        // The channel's own state root may not exist before anything was ever saved; admission needs it.
        if !env.state.exists() {
            use std::os::unix::fs::DirBuilderExt;
            let _ = fs::DirBuilder::new().recursive(true).mode(0o700).create(&env.state);
        }
        let now = SystemTime::now();
        let mut planned = vec![];
        {
            let mut table = locked();
            let mut rows: Vec<_> = records["connections"]
                .as_object()
                .into_iter()
                .flat_map(|m| m.values())
                .collect();
            rows.sort_by_key(|r| (r["engine"].as_str(), r["name"].as_str(), r["id"].as_str()));
            for r in rows {
                let Some(id) = r["id"].as_str() else { continue };
                if r["state"] != "configured" || !producer::admitted(&env, r) {
                    continue;
                }
                let needed = match newest(&env, id) {
                    None => true,
                    Some(at) => now.duration_since(at).is_ok_and(|age| age > STALE_AFTER),
                };
                let key = (env.state.clone(), id.to_owned());
                let slot = table.get(&key);
                let busy = slot.is_some_and(|s| match s.phase {
                    Phase::Queued | Phase::Running => true,
                    _ => s.since.elapsed() < wait_after(&s.phase, s.failures),
                });
                if needed && !busy {
                    let failures = slot.map_or(0, |s| s.failures);
                    table.insert(
                        key,
                        Slot {
                            phase: Phase::Queued,
                            since: Instant::now(),
                            failures,
                        },
                    );
                    planned.push(id.to_owned());
                }
            }
        }
        // One thread per connection: engines start side by side, and one slow or
        // failing connection never delays another's list. Publication of each
        // snapshot is still serialized by the registry fence.
        for id in planned {
            let spawned = std::thread::Builder::new()
                .name("catalogue-refresh".into())
                .spawn({
                    let (env, id) = (env.clone(), id.clone());
                    move || attempt(&env, &env.state.clone(), &id)
                });
            if spawned.is_err() {
                set(
                    &(env.state.clone(), id),
                    Phase::Failed("refresh_unavailable".into()),
                );
            }
        }
    }
    status(config)
}
fn attempt(env: &RouteEnvironment, root: &std::path::Path, id: &str) {
    let key = (root.to_path_buf(), id.to_owned());
    set(&key, Phase::Running);
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loop {
        match observe_session_refresh(env, id) {
            Err("refresh_in_progress") if started.elapsed() < ATTEMPT => {
                // Another process is observing this connection now: its list is what is wanted.
                if newest(env, id).is_some() && started.elapsed() > Duration::from_secs(1) {
                    break Phase::Done;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(code) => break Phase::Failed(safe_reason(code)),
            Ok(Some(row)) if row["success"] == true => break Phase::Done,
            Ok(Some(row)) => {
                break Phase::Failed(safe_reason(row["reason"].as_str().unwrap_or_default()))
            }
            Ok(None) => break Phase::Failed("native_catalog_unavailable".into()),
        }
    }))
    .unwrap_or_else(|_| Phase::Failed("refresh_failed".into()));
    set(&key, outcome);
}
#[cfg(test)]
pub(crate) fn forget_all() {
    locked().clear();
}
#[cfg(test)]
mod tests {
    use super::*;
    /// A failure is tried again within seconds, then less and less often, never more
    /// rarely than a good list is; a success goes back to the long wait.
    #[test]
    fn a_failed_observation_waits_a_short_doubling_bounded_time() {
        let failed = Phase::Failed("refresh_lock_unavailable".into());
        let waits: Vec<_> = (1..=10).map(|n| wait_after(&failed, n).as_secs()).collect();
        assert_eq!(waits, [5, 10, 20, 40, 80, 160, 320, 600, 600, 600]);
        assert_eq!(wait_after(&failed, u32::MAX), COOLDOWN);
        assert_eq!(wait_after(&Phase::Done, 0), COOLDOWN);
    }
    #[test]
    fn failures_count_up_and_a_success_clears_them() {
        let key: Key = (
            PathBuf::from("/nonexistent/cw-startup-backoff"),
            "cn_00000000000f".into(),
        );
        let count = || locked().get(&key).map(|s| s.failures);
        set(&key, Phase::Running);
        assert_eq!(count(), Some(0));
        set(&key, Phase::Failed("x".into()));
        set(&key, Phase::Queued);
        set(&key, Phase::Running);
        set(&key, Phase::Failed("x".into()));
        assert_eq!(count(), Some(2));
        set(&key, Phase::Done);
        assert_eq!(count(), Some(0));
        locked().remove(&key);
    }
}
