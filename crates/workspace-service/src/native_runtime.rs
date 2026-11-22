//! Service-owned native Start composition. No model or socket supplied launch authority.
use crate::{
    actor, failure,
    native_sessions::{NativeHost, SessionRole, SessionRoute},
    Service,
};
use cortex_core::{
    ownership::OwnerEpoch, CoreError, CoreResult, OpenMode, ProjectKey, Store, StoreChannel,
};
use futures::{stream::FuturesUnordered, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{watch, Mutex};

static RETRY_WINDOW: std::sync::Mutex<Option<cortex_core::scheduler::retry::RetryWindow>> =
    std::sync::Mutex::new(None);
/// Python reads its lost-worker retry window at call time
/// (`engine/retry_policy.py::default_window`, `runtime._engine_retry_window`)
/// so its tests can shorten it. This is the same seam; production never sets it.
#[doc(hidden)]
pub fn set_retry_window(window: Option<cortex_core::scheduler::retry::RetryWindow>) {
    *RETRY_WINDOW.lock().unwrap_or_else(|e| e.into_inner()) = window;
}

#[cfg(test)]
mod refill_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    async fn fixture() -> (
        Arc<Runtime>,
        cortex_core::ownership::OwnerEpoch,
        String,
        std::path::PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!("refill-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let owner = runtime.host.executor.publication(|store| {
            let owner = cortex_core::lifecycle::TaskMachine::new(store).create("t", "g", "owner")?;
            store.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'next','worker-devops-engineer','Work','[]','[\"src\"]','execution','pending',NULL,NULL)", [])?;
                Ok(())
            })?;
            Ok(owner)
        }).unwrap().await.unwrap().unwrap();
        (runtime, owner, "t".into(), root)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refill_rejects_stale_revision_without_issuing() {
        let (runtime, owner, task, root) = fixture().await;
        let out = runtime
            .host
            .refill_execution(task.clone(), owner, 2)
            .await
            .unwrap();
        assert!(out.is_empty());
        let status = runtime
            .host
            .executor
            .read(move |db| {
                Ok(db.query_row(
                    "SELECT status FROM runtime_nodes WHERE task_id='t' AND node_id='next'",
                    [],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status, "pending");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refill_claims_once_and_spends_budget_only_for_claim() {
        let (runtime, owner, task, root) = fixture().await;
        let first = runtime
            .host
            .refill_execution(task.clone(), owner.clone(), 1)
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        let second = runtime
            .host
            .refill_execution(task.clone(), owner, 1)
            .await
            .unwrap();
        assert!(second.is_empty());
        let used: i64 = runtime.host.executor.read(|db| Ok(db.query_row(
            "SELECT json_extract(result_json,'$.used') FROM runtime_requests WHERE task_id='t' AND actor='native-progress'", [], |r| r.get(0))?)).unwrap().await.unwrap().unwrap();
        assert_eq!(used, 1);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refill_does_not_claim_while_steering_is_pending() {
        let (runtime, owner, task, root) = fixture().await;
        runtime
            .host
            .executor
            .write(|db| {
                cortex_core::lifecycle::event_tx(
                    db,
                    "t",
                    "USER_STEERING",
                    json!({"request":"r","delivery":"boundary"}),
                )
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let out = runtime.host.refill_execution(task, owner, 1).await.unwrap();
        assert!(out.is_empty());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
pub(crate) fn retry_window() -> cortex_core::scheduler::retry::RetryWindow {
    RETRY_WINDOW
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unwrap_or_default()
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The models (`{engine, model}`) on which `node`'s lineage (the node and its review
/// fixes, in every plan revision since the task was last reopened) was lost to
/// infrastructure at least twice: a fresh attempt is routed away from them.
pub fn lost_models(db: &rusqlite::Connection, task: &str, node: &str) -> CoreResult<Vec<Value>> {
    use cortex_core::scheduler::retry::{infrastructure_class, node_base};
    let rows = db
        .prepare(
            "SELECT json_extract(r.payload_json,'$.node'),coalesce(json_extract(r.payload_json,'$.failure_class'),''),
                json_extract(s.payload_json,'$.route_engine'),json_extract(s.payload_json,'$.model')
             FROM runtime_events r JOIN runtime_events s ON s.task_id=r.task_id AND s.type='NATIVE_ROUTE_SELECTED'
               AND json_extract(s.payload_json,'$.assignment_ref')=json_extract(r.payload_json,'$.previous_assignment')
             WHERE r.task_id=?1 AND r.type='ASSIGNMENT_RETRIED' AND json_extract(r.payload_json,'$.failed_at') IS NOT NULL
               AND r.id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)",
        )?
        .query_map([task], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let base = node_base(node);
    let mut counts: std::collections::BTreeMap<(Option<String>, String), usize> =
        Default::default();
    for (lost, class, engine, model) in rows {
        let (Some(lost), Some(model)) = (lost, model) else {
            continue;
        };
        if node_base(&lost) != base
            || !infrastructure_class(&class)
            || matches!(class.as_str(), "owner_paused" | "runtime_restart")
        {
            continue;
        }
        *counts.entry((engine, model)).or_default() += 1;
    }
    Ok(counts
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|((engine, model), n)| json!({"engine":engine,"model":model,"losses":n}))
        .collect())
}
/// At most this many characters (the last lines) of an engine's already redacted
/// stderr tail are kept with an attempt's end.
pub const ENDING_STDERR_CHARS: usize = 400;
/// The last lines of an engine's redacted stderr tail, bounded by
/// [`ENDING_STDERR_CHARS`] characters.
pub fn stderr_tail(redacted: &str) -> String {
    let chars: Vec<char> = redacted.trim_end().chars().collect();
    let start = chars.len().saturating_sub(ENDING_STDERR_CHARS);
    chars[start..].iter().collect()
}
/// Facts the store holds about how one worker attempt ended: the results it had
/// refused (count and the last code), a runtime cancel of one of its permission
/// requests under the read-only rule, a degenerate turn, an owner pause after it
/// started, and whether its last command could not get a sandbox.
pub fn worker_ending_facts(
    db: &rusqlite::Connection,
    task: &str,
    channel: &str,
    attempt: &str,
) -> CoreResult<Value> {
    use rusqlite::OptionalExtension;
    // A refusal because the checkout kept changing (`review::DRIFT_REFUSALS`) is the
    // environment's, never the worker's quality.
    let (refused, drift, last): (i64, i64, Option<String>) = db.query_row(
        "SELECT coalesce(sum(coalesce(json_extract(payload_json,'$.code'),'') NOT IN ('review_project_revision_changed','review_project_revision_unstable','review_resubmit_required')),0),
            coalesce(sum(coalesce(json_extract(payload_json,'$.code'),'') IN ('review_project_revision_changed','review_project_revision_unstable','review_resubmit_required')),0),
            (SELECT json_extract(payload_json,'$.code') FROM runtime_events WHERE task_id=?1
            AND type='WORKER_RESULT_REFUSED' AND json_extract(payload_json,'$.attempt')=?2 ORDER BY id DESC LIMIT 1)
         FROM runtime_events WHERE task_id=?1 AND type='WORKER_RESULT_REFUSED' AND json_extract(payload_json,'$.attempt')=?2",
        (task, attempt),
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let cancelled: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_PERMISSION_DECIDED'
         AND json_extract(payload_json,'$.channel_id')=? AND json_extract(payload_json,'$.decider')='runtime'
         AND json_extract(payload_json,'$.outcome.optionId')='cancel')",
        (task, channel),
        |r| r.get(0),
    )?;
    let degenerate: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_TURN_DEGENERATE'
         AND json_extract(payload_json,'$.channel_id')=?)",
        (task, channel),
        |r| r.get(0),
    )?;
    let paused: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events p WHERE p.task_id=?1 AND p.type='OWNER_TASK_PAUSED'
         AND p.id>coalesce((SELECT min(id) FROM runtime_events WHERE task_id=?1 AND type='NATIVE_ATTEMPT_DELIVERY'
            AND json_extract(payload_json,'$.attempt')=?2),
          (SELECT max(id) FROM runtime_events WHERE task_id=?1)))",
        (task, attempt),
        |r| r.get(0),
    )?;
    let last_failure: Option<String> = db
        .query_row(
            "SELECT coalesce(json_extract(result_json,'$.failure'),json_extract(result_json,'$.reply.structuredContent.failure'))
             FROM runtime_requests WHERE task_id=? AND actor=? AND operation IN ('command_run','outside_command')
             ORDER BY rowid DESC LIMIT 1",
            (task, channel),
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let sandbox = last_failure.as_deref().is_some_and(|f| {
        f.contains("sandbox_unavailable") || f.starts_with("git_context_unavailable")
    });
    Ok(
        json!({"refused_results":refused,"drift_refusals":drift,"last_refusal":last,
        "runtime_cancel":if cancelled {json!("read_only_permission")} else {Value::Null},
        "degenerate":degenerate,"owner_paused":paused,"sandbox_unavailable":sandbox}),
    )
}
/// The failure class of a worker attempt that ended without an accepted result, from
/// how its turn ended (`cause`) and [`worker_ending_facts`]. Only `result_refused`,
/// `degenerate_turn` and `indeterminate` are the attempt's own quality; every other
/// class is infrastructure (`cortex_core::scheduler::retry::INFRASTRUCTURE_CLASSES`).
pub fn worker_ending_class(cause: &str, facts: &Value) -> &'static str {
    let refused = facts["refused_results"].as_i64().unwrap_or(0) > 0;
    let sandbox = facts["sandbox_unavailable"] == true;
    match cause {
        "native_turn_degenerate" => "degenerate_turn",
        _ if facts["degenerate"] == true => "degenerate_turn",
        "owner_stopped" => "owner_paused",
        "native_task_interrupted" if facts["owner_paused"] == true => "owner_paused",
        "native_service_stopped" | "native_task_interrupted" => "runtime_restart",
        // The model ended its turn: refused results without an accepted one are its own.
        "turn_ended" if refused => "result_refused",
        "turn_ended" if sandbox => "sandbox_unavailable",
        // Only refused for another actor's checkout drift: retried on the runtime's budget.
        "turn_ended" if facts["drift_refusals"].as_i64().unwrap_or(0) > 0 => "worker_lost",
        "turn_ended" => "indeterminate",
        // The process vanished without the model ending its turn.
        "process_exited" if sandbox => "sandbox_unavailable",
        "process_exited" => "worker_lost",
        code if code.ends_with("outcome_unknown") || code.contains("transport") => {
            "transport_failure"
        }
        code if code.contains("authentication") => "authorization_loss",
        _ => "engine_failure",
    }
}

/// Per-task backoff of environment rechecks for open owner actions (in memory: a
/// restart rechecks once at once, which is harmless).
static ENVIRONMENT_RECHECKS: std::sync::Mutex<
    Option<std::collections::HashMap<String, (std::time::Instant, usize)>>,
> = std::sync::Mutex::new(None);

fn environment_recheck_due(task: &str) -> bool {
    let mut map = ENVIRONMENT_RECHECKS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(Default::default);
    let now = std::time::Instant::now();
    match map.get(task).copied() {
        Some((at, n))
            if now.duration_since(at).as_secs()
                < cortex_core::lifecycle::obstacles::backoff(n.saturating_sub(1)) as u64 =>
        {
            false
        }
        Some((_, n)) => {
            map.insert(task.to_owned(), (now, n + 1));
            true
        }
        None => {
            map.insert(task.to_owned(), (now, 1));
            true
        }
    }
}

/// One public address for this attempt, or refuse the entire DNS answer if
/// it contains a non-public destination. Do not reinterpret mapped/tunnel
/// addresses or scope identifiers as public IPv6 addresses.
fn pinned_external_address(
    addresses: impl IntoIterator<Item = std::net::SocketAddr>,
) -> Result<std::net::SocketAddr, &'static str> {
    use std::net::SocketAddr;
    let mut first = None;
    for address in addresses {
        let public = match address {
            SocketAddr::V4(addr) => {
                let ip = addr.ip();
                let [a, b, c, _] = ip.octets();
                !ip.is_private()
                    && !ip.is_loopback()
                    && !ip.is_link_local()
                    && !ip.is_documentation()
                    && !matches!(
                        (a, b, c),
                        (0 | 224..=255, _, _)
                            | (100, 64..=127, _)
                            | (192, 0, 0)
                            | (192, 88, 99)
                            | (198, 18..=19, _)
                    )
            }
            SocketAddr::V6(addr) => {
                let [a, b, ..] = addr.ip().segments();
                addr.scope_id() == 0
                    && (0x2000..=0x3fff).contains(&a)
                    // Protocol assignments (incl. Teredo), documentation, and 6to4.
                    && !matches!((a, b), (0x2001, 0..=0x01ff | 0x0db8) | (0x2002, _) | (0x3fff, _))
            }
        };
        if !public || address.port() != 443 {
            return Err("outage probe refused a non-public or mismatched DNS address");
        }
        first.get_or_insert(address);
    }
    first.ok_or("outage probe DNS returned no addresses")
}

fn configure_external_head(
    command: &mut tokio::process::Command,
    host: &str,
    address: std::net::SocketAddr,
) {
    let port = address.port();
    let ip = match address.ip() {
        std::net::IpAddr::V4(ip) => ip.to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
    };
    command
        .env_clear()
        .args([
            "-q", // Must be first: even with no HOME, curl may otherwise read curlrc.
            "--silent",
            "--head",
            "--output",
            "/dev/null",
            "--max-time",
            "10",
            "--proto",
            "=https",
            "--noproxy",
            "*",
            "--no-location",
            "--resolve",
            &format!("{host}:{port}:{ip}"),
            "--write-out",
            "%{http_code}",
            "--",
            &format!("https://{host}:{port}/"),
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
}

/// A wait (including a retained wait from a previous version) is not proof of
/// destination authority. Validate before any DNS lookup and bound the whole
/// probe, including DNS, to 10 seconds.
pub(crate) async fn probe_external(wait: &Value) -> (bool, String) {
    probe_external_with_lookup(wait, |host| tokio::net::lookup_host((host, 443))).await
}

async fn probe_external_with_lookup<A, L, F>(wait: &Value, lookup: L) -> (bool, String)
where
    A: IntoIterator<Item = std::net::SocketAddr>,
    L: FnOnce(&'static str) -> F,
    F: std::future::Future<Output = io::Result<A>>,
{
    use cortex_core::lifecycle::obstacles::external_probe_host;
    let host = wait["host"].as_str().unwrap_or("");
    let scheme = wait["scheme"].as_str().unwrap_or("");
    let port = wait["port"].as_u64().and_then(|p| u16::try_from(p).ok());
    let Some(host) = port.and_then(|p| external_probe_host(host, scheme, p)) else {
        return (false, "outage probe destination unsupported".into());
    };
    let Some(probe @ ("tcp" | "http")) = wait["probe"].as_str() else {
        return (false, "outage probe kind unsupported".into());
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let addresses = lookup(host).await.map_err(|_| "outage probe DNS failed")?;
        let address = pinned_external_address(addresses)?;
        if probe == "tcp" {
            tokio::net::TcpStream::connect(address)
                .await
                .map_err(|_| "outage probe connect failed")?;
            return Ok((true, format!("{host}:443 accepts connections")));
        }
        // The URL retains the fixed TLS server name; --resolve supplies the
        // exact checked address, so curl neither looks it up again nor uses a proxy.
        let mut command = tokio::process::Command::new("/usr/bin/curl");
        configure_external_head(&mut command, host, address);
        let out = command
            .output()
            .await
            .map_err(|_| "outage HEAD unavailable")?;
        if !out.status.success() {
            return Err("outage HEAD failed");
        }
        let code = std::str::from_utf8(&out.stdout)
            .ok()
            .and_then(|status| status.trim().parse::<u16>().ok())
            .filter(|code| (200..600).contains(code))
            .ok_or("outage HEAD returned no final HTTP status")?;
        Ok((code < 500 && code != 429, format!("HTTP {code}")))
    })
    .await;
    match result {
        Ok(Ok(result)) => result,
        Ok(Err(detail)) => (false, detail.into()),
        Err(_) => (false, "outage probe timed out".into()),
    }
}

#[cfg(test)]
mod external_probe_tests {
    use super::*;
    use std::{future::ready, net::SocketAddr};

    #[tokio::test]
    async fn unsupported_stored_targets_do_not_reach_dns() {
        for (host, scheme, port, probe) in [
            ("untrusted.example", "https", 443, "http"),
            ("api.github.com.untrusted.example", "https", 443, "tcp"),
            ("api.github.com", "http", 443, "http"),
            ("api.github.com", "https", 8443, "tcp"),
            ("api.github.com", "https", 65_979, "http"),
            ("api.github.com", "https", 443, "unknown"),
        ] {
            let wait = json!({"host":host,"scheme":scheme,"port":port,"probe":probe});
            let result = probe_external_with_lookup(
                &wait,
                |_| -> std::future::Ready<io::Result<Vec<SocketAddr>>> {
                    panic!("unsupported wait must not resolve");
                },
            )
            .await;
            assert!(!result.0);
            assert!(result.1.ends_with("unsupported"));
        }
    }

    #[tokio::test]
    async fn allowed_hostname_with_non_public_dns_cannot_connect() {
        let wait = json!({"host":"api.github.com","scheme":"https","port":443,"probe":"http"});
        for address in [
            "0.0.0.0:443",
            "10.0.0.1:443",
            "127.0.0.1:443",
            "169.254.169.254:443",
            "172.16.0.1:443",
            "192.168.0.1:443",
            "100.64.0.1:443",
            "[::]:443",
            "[::1]:443",
            "[::ffff:127.0.0.1]:443",
            "[fe80::1]:443",
            "[fc00::1]:443",
            "[ff02::1]:443",
            "[64:ff9b::a00:1]:443",
        ] {
            let result = probe_external_with_lookup(&wait, |host| {
                assert_eq!(host, "api.github.com");
                // Even a mixed response cannot authorize a connection.
                ready(Ok([
                    "1.1.1.1:443".parse::<SocketAddr>().unwrap(),
                    address.parse::<SocketAddr>().unwrap(),
                ]))
            })
            .await;
            assert_eq!(
                result,
                (
                    false,
                    "outage probe refused a non-public or mismatched DNS address".into()
                ),
                "{address}"
            );
        }
    }

    #[test]
    fn external_probe_selects_only_a_numeric_public_address() {
        let first: SocketAddr = "[2606:4700::1]:443".parse().unwrap();
        let second: SocketAddr = "1.1.1.1:443".parse().unwrap();
        assert_eq!(pinned_external_address([first, second]), Ok(first));
        assert!(pinned_external_address([]).is_err());
        assert!(pinned_external_address(["1.1.1.1:8443".parse().unwrap()]).is_err());
    }

    #[tokio::test]
    async fn head_uses_a_config_free_client_with_an_exact_dns_pin() {
        for (address, pin) in [
            ("1.1.1.1:443", "api.github.com:443:1.1.1.1"),
            ("[2606:4700::1]:443", "api.github.com:443:[2606:4700::1]"),
        ] {
            let mut command = tokio::process::Command::new("/usr/bin/curl");
            configure_external_head(&mut command, "api.github.com", address.parse().unwrap());
            let command = command.as_std();
            assert_eq!(command.get_program(), "/usr/bin/curl");
            assert_eq!(command.get_envs().count(), 0);
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [
                    "-q",
                    "--silent",
                    "--head",
                    "--output",
                    "/dev/null",
                    "--max-time",
                    "10",
                    "--proto",
                    "=https",
                    "--noproxy",
                    "*",
                    "--no-location",
                    "--resolve",
                    pin,
                    "--write-out",
                    "%{http_code}",
                    "--",
                    "https://api.github.com:443/",
                ]
            );
        }

        // Execute only a local environment oracle, not curl. printenv treats
        // our HEAD arguments as further names after its own end-of-options.
        let mut oracle = tokio::process::Command::new("/usr/bin/printenv");
        oracle
            .args(["--", "HOME", "PATH", "HTTPS_PROXY", "CORTEX_TEST_SENTINEL"])
            .env("CORTEX_TEST_SENTINEL", "synthetic");
        configure_external_head(
            &mut oracle,
            "api.github.com",
            "1.1.1.1:443".parse().unwrap(),
        );
        let out = oracle.output().await.unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
    }
}

/// Trusted embedding authorization, not a route candidate or desktop JSON.
/// Implementations must qualify every returned engine/account/model and role.
/// Generate the absent first-run profile from routing/defaults/MCP/connections
/// bytes. The caller owns the configuration lock and no-replacement publication;
/// this function has no filesystem or engine effects. Custom settings return None.
pub fn seed_default_profile(
    bytes: &[Option<Vec<u8>>; 4],
    now: &str,
) -> io::Result<Option<Vec<u8>>> {
    crate::machine::seed_default_profile(bytes, now)
}

pub trait NativeRoutes: Send + Sync {
    fn default_language(&self) -> Option<String> {
        None
    }
    /// Persist a pre-task language choice as the user-wide default. Returns
    /// false when this provider owns no user defaults (the choice stays
    /// session-local); an error means nothing was persisted.
    fn persist_default_language(&self, _language: Option<&str>) -> CoreResult<bool> {
        Ok(false)
    }
    /// Persist a pre-task coordinator `model`/`thought_level` choice (`option`)
    /// as the user-default route, from the session's `previous` and new
    /// `settings`. Same contract as `persist_default_language`.
    fn persist_default_route(
        &self,
        _root: &Path,
        _previous: &Value,
        _settings: &Value,
        _option: &str,
    ) -> CoreResult<bool> {
        Ok(false)
    }
    fn route(&self, root: &Path, role: &str) -> CoreResult<SessionRoute>;
    /// The exact coordinator route a task starting with `settings` resolves to, as
    /// a durable pin `{route_id, parameters}`. Persisted at start so a later change
    /// of the owner default never re-routes a running task. None: not pinnable.
    fn coordinator_pin(&self, _root: &Path, _settings: &Value) -> CoreResult<Option<Value>> {
        Ok(None)
    }
    /// The session route of a pin from `coordinator_pin`. `effort` is the task's
    /// explicit thought-level choice, if any.
    fn pinned_route(
        &self,
        root: &Path,
        _pin: &Value,
        _effort: Option<&str>,
    ) -> CoreResult<SessionRoute> {
        self.route(root, "orchestrator")
    }
    /// A durable coordinator pin `{route_id, parameters}` for another qualified model
    /// than `current` (preferably another engine or account), for the no-stop policy's
    /// change of approach (a degenerate model, the `reassign` rung, an unavailable
    /// coordinator route). None: no other model qualifies.
    fn alternate_coordinator_pin(
        &self,
        _root: &Path,
        _current: &SessionRoute,
    ) -> CoreResult<Option<Value>> {
        Ok(None)
    }
    fn worker_route(
        &self,
        root: &Path,
        role: &str,
        _node: &Value,
        _evidence: &cortex_core::routing::Evidence,
        _escalated: bool,
        _previous: Option<&str>,
    ) -> CoreResult<SessionRoute> {
        self.route(root, role)
    }
    fn worker_selection(
        &self,
        root: &Path,
        role: &str,
        node: &Value,
        evidence: &cortex_core::routing::Evidence,
        escalated: bool,
        previous: Option<&str>,
    ) -> CoreResult<(SessionRoute, Value)> {
        Ok((
            self.worker_route(root, role, node, evidence, escalated, previous)?,
            json!({}),
        ))
    }
    fn menu(&self, _root: &Path, _evidence: &cortex_core::routing::Evidence) -> CoreResult<Value> {
        Err(CoreError::Refused("native_route_menu_unavailable"))
    }
    fn previews(
        &self,
        _root: &Path,
        _plan: &Value,
        _evidence: &cortex_core::routing::Evidence,
    ) -> CoreResult<Value> {
        Ok(json!([]))
    }
    fn mcp_registry(&self) -> CoreResult<crate::native_mcp::Registry> {
        Ok(crate::native_mcp::Registry::default())
    }
    /// Only host-qualified identities may be exposed to an incoming client.
    fn options(&self, root: &Path, settings: &Value) -> Value {
        match self.task_route(root, settings) {
            Ok(route) => json!([{"id":"model","name":"Model","category":"model","type":"select",
                "currentValue":route.model,"options":[{"value":route.model,"name":route.model}]}]),
            Err(_) => json!([]),
        }
    }
    /// Selectable routes `{route, model, label, disambiguated_label}` for
    /// `/cortex-model` resolution.
    fn listing(&self, _root: &Path) -> Vec<Value> {
        Vec::new()
    }
    /// Every registered connection's persisted state, in registry order.
    fn connection_states(&self) -> Vec<String> {
        Vec::new()
    }
    /// Registered connection ids, in registry order.
    fn connection_ids(&self) -> Vec<String> {
        Vec::new()
    }
    /// A new, prompt-free qualified observation of the same connection.
    fn authentication_observed_after(
        &self,
        _root: &Path,
        _connection: &str,
        _after: &str,
    ) -> CoreResult<bool> {
        Ok(false)
    }
    /// Automatic coordination respects the same observed quota as automatic worker routing.
    fn automatic_coordinator_health(&self, _root: &Path, _route: &SessionRoute) -> CoreResult<()> {
        Ok(())
    }
    /// One prompt-free catalogue refresh of a registered connection (Python's
    /// per-session `_refresh_catalog_for_session`); true only when a new
    /// observation was published. Never a model prompt.
    fn refresh_catalog(&self, _connection: &str) -> bool {
        false
    }
    /// The route a `provider`/`model`/`thought_level` choice selects (Python
    /// `_apply_config_selection`): `{route, model, connection, echo}`, where
    /// `echo` marks a pre-task re-selection of the saved or pending route.
    fn select(&self, _root: &Path, settings: &Value, id: &str, value: &str) -> CoreResult<Value> {
        let route = if id == "thought_level" {
            settings["model"].clone()
        } else {
            json!(value)
        };
        Ok(json!({"route":route,"model":route,"connection":null,"echo":false}))
    }
    fn task_route(&self, root: &Path, settings: &Value) -> CoreResult<SessionRoute> {
        let route = self.route(root, "orchestrator")?;
        if settings["model"]
            .as_str()
            .is_some_and(|m| !m.is_empty() && m != route.model)
            || settings["effort"].as_str().is_some_and(|e| {
                !e.is_empty()
                    && route.parameters.get("thought_level").map(String::as_str) != Some(e)
            })
        {
            return Err(CoreError::Refused("native_task_route_unavailable"));
        }
        Ok(route)
    }
}
/// `history.page` is served from the core projection (see `Service::workspace`
/// and `native.query`); without it a client never reads the task conversation.
pub(crate) fn capabilities() -> io::Result<Value> {
    let mut value = workspace_protocol::capabilities(
        "workspace-service-native-1.0.6",
        "1",
        &[
            "capabilities.get",
            "tasks.create",
            "tasks.start",
            "decisions.list",
            "decisions.get",
            "decisions.answer",
            "history.page",
            "reports.read",
        ],
    )
    .map_err(|_| failure("invalid_capabilities"))?;
    // Retained, delivered files: Start, owner messages (also Request changes) and answers
    // keep the bodies privately and the engine prompt carries them (`native_attachments`).
    // Worker guidance does not carry files; its composer says so itself.
    value["attachments"] = json!(true);
    workspace_protocol::validate("Capabilities", &value)
        .map_err(|_| failure("invalid_capabilities"))?;
    Ok(value)
}
pub struct UnqualifiedRoutes;
impl NativeRoutes for UnqualifiedRoutes {
    fn route(&self, _: &Path, _: &str) -> CoreResult<SessionRoute> {
        Err(CoreError::Refused("native_route_authorization_unavailable"))
    }
}
/// The answer to one request to send an Automatic task back to its coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepairAdmission {
    /// The task was reopened and its coordinator continues it.
    Started,
    /// Nothing was decided now (another control, maintenance, or the service stopping).
    Busy,
    /// Not admitted: already spent, bounded, or the task is not in the expected state.
    Declined,
}
pub(crate) struct Runtime {
    pub(super) host: Arc<NativeHost>,
    pub(super) routes: Arc<dyn NativeRoutes>,
    pub(super) controls: Mutex<()>,
    pub(super) command_controls: Mutex<()>,
    pumps: Mutex<HashMap<String, Pump>>,
    pub(super) guidance: Mutex<HashMap<String, tokio::sync::mpsc::Sender<Value>>>,
    pub(super) worker_stops: Mutex<HashMap<String, (watch::Sender<bool>, watch::Receiver<bool>)>>,
    pub(super) wake: tokio::sync::Notify,
    jobs: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    pub(super) stop: watch::Sender<bool>,
    /// Backoff of the live re-check of unconfirmed exits (`native_recovery`).
    pub(super) exit_recheck: std::sync::Mutex<crate::native_recovery::ExitRecheck>,
    /// Backoff of the live re-settle of orphaned command receipts (`native_recovery`).
    pub(super) command_recheck: std::sync::Mutex<crate::native_recovery::CommandRecheck>,
    /// Whether a driver's end starts a continuation follower (the no-stop policy).
    /// Only scheduler fixtures that drive every pass themselves turn it off.
    pub(super) followers: std::sync::atomic::AtomicBool,
}
struct Pump {
    cancel: watch::Sender<bool>,
    done: watch::Receiver<bool>,
}
fn err(e: CoreError) -> io::Error {
    failure(e.code())
}
pub(crate) fn key(root: &Path) -> CoreResult<ProjectKey> {
    ProjectKey::new(
        root,
        if workspace_protocol::channel::current().is_dev() {
            StoreChannel::Dev
        } else {
            StoreChannel::Release
        },
    )
}
/// Incoming history-only ingress still refuses unadopted legacy projects.
pub(crate) fn legacy(root: &Path) -> CoreResult<bool> {
    let key = key(root)?;
    if !key.database().try_exists()? {
        return Ok(false);
    }
    Store::open(key, OpenMode::LiveReadOnly)?.read(|db| Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_tasks t WHERE NOT EXISTS(SELECT 1 FROM runtime_events e
         WHERE e.task_id=t.task_id AND e.type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED') AND json_extract(e.payload_json,'$.execution_host')='rust'))",
        [], |r| r.get(0))?))
}
impl Runtime {
    #[cfg(test)]
    pub(crate) fn open(
        root: &Path,
        project: &str,
        routes: Arc<dyn NativeRoutes>,
    ) -> CoreResult<Self> {
        Self::open_with(root, project, routes, false, None)
    }
    /// Incoming ACP agents may share a project: see `StoreExecutor::open_shared`.
    pub(crate) fn open_shared(
        root: &Path,
        project: &str,
        routes: Arc<dyn NativeRoutes>,
    ) -> CoreResult<Self> {
        Self::open_with(root, project, routes, true, None)
    }
    fn open_with(
        root: &Path,
        project: &str,
        routes: Arc<dyn NativeRoutes>,
        shared: bool,
        workspace_state: Option<PathBuf>,
    ) -> CoreResult<Self> {
        let mut host = if shared {
            NativeHost::open_shared_for_recovery(key(root)?, routes.mcp_registry()?)?
        } else {
            NativeHost::open_for_recovery(key(root)?, routes.mcp_registry()?)?
        };
        host.workspace_state = workspace_state;
        *host.routes.write().unwrap() = Some(routes.clone());
        let project = project.to_owned();
        let root = root.to_string_lossy().into_owned();
        host.executor.write(move |db| {
            let actor = actor();
            db.execute("INSERT OR IGNORE INTO workspace_projects VALUES (?,?,?, ?,1)",
                (&project,&root,&actor,cortex_core::now()))?;
            let valid: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM workspace_projects WHERE project_id=? AND project_root=? AND activated_by=?)",
                (&project,&root,&actor), |r| r.get(0))?;
            if !valid { return Err(CoreError::Refused("project_context_conflict")); }
            Ok(())
        })?.blocking_recv().map_err(|_| CoreError::Refused("native_host_closed"))??;
        let (stop, _) = watch::channel(false);
        Ok(Self {
            host: Arc::new(host),
            routes,
            controls: Mutex::new(()),
            command_controls: Mutex::new(()),
            pumps: Mutex::new(HashMap::new()),
            guidance: Mutex::new(HashMap::new()),
            worker_stops: Mutex::new(HashMap::new()),
            wake: tokio::sync::Notify::new(),
            jobs: Mutex::new(Vec::new()),
            stop,
            exit_recheck: Default::default(),
            followers: std::sync::atomic::AtomicBool::new(true),
            command_recheck: Default::default(),
        })
    }
    pub(crate) fn open_workspace(
        root: &Path,
        project: &str,
        routes: Arc<dyn NativeRoutes>,
        state: PathBuf,
    ) -> CoreResult<Self> {
        Self::open_with(root, project, routes, false, Some(state))
    }
    pub(crate) async fn draft_attachments(
        &self,
        project: String,
        task: String,
    ) -> CoreResult<Value> {
        self.host
            .executor
            .publication(move |store| {
                cortex_core::workspace::draft_attachments(store, &project, &actor(), &task)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    pub(crate) async fn command(
        self: &Arc<Self>,
        project: String,
        request: Value,
    ) -> io::Result<Value> {
        self.client_command(project, request, None).await
    }
    pub(super) async fn start_automatic(
        self: &Arc<Self>,
        project: String,
        request: Value,
    ) -> io::Result<Value> {
        use cortex_core::workspace::NativeExecutionPolicy;
        if request["operation"] != "tasks.start" {
            return Err(failure("native_operation_unavailable"));
        }
        self.command_with_policy(
            project,
            request,
            None,
            Some(NativeExecutionPolicy::AutomaticContinuation),
        )
        .await
    }
    /// Start a sprint's read-only planning task: bound in the Start transaction.
    pub(super) async fn start_sprint_planning(
        self: &Arc<Self>,
        project: String,
        request: Value,
    ) -> io::Result<Value> {
        use cortex_core::workspace::NativeExecutionPolicy;
        if request["operation"] != "tasks.start" {
            return Err(failure("native_operation_unavailable"));
        }
        self.command_with_policy(
            project,
            request,
            None,
            Some(NativeExecutionPolicy::SprintPlanning),
        )
        .await
    }
    pub(super) async fn client_command(
        self: &Arc<Self>,
        project: String,
        request: Value,
        client: Option<(String, Value, Option<cortex_core::input::Capture>)>,
    ) -> io::Result<Value> {
        self.command_with_policy(project, request, client, None)
            .await
    }
    async fn command_with_policy(
        self: &Arc<Self>,
        project: String,
        request: Value,
        client: Option<(String, Value, Option<cortex_core::input::Capture>)>,
        policy: Option<cortex_core::workspace::NativeExecutionPolicy>,
    ) -> io::Result<Value> {
        if *self.stop.borrow() {
            return Err(failure("service_stopping"));
        }
        let host = self.host.clone();
        let operation = request["operation"].as_str().unwrap_or("");
        if operation == "capabilities.get" {
            return capabilities();
        }
        if operation == "tasks.create" {
            return host
                .executor
                .publication(move |store| {
                    Ok(
                        cortex_core::workspace::create_draft(store, &project, &actor(), &request)?
                            .receipt,
                    )
                })
                .map_err(err)?
                .await
                .map_err(|_| failure("native_host_closed"))?
                .map_err(err);
        }
        if matches!(operation, "decisions.list" | "decisions.get") {
            return self.decisions(project, request).await;
        }
        if operation == "decisions.answer" {
            return self.answer_command(project, request).await;
        }
        if operation == "reports.read" {
            let report = request["target"].as_str().unwrap_or("").to_owned();
            let cursor = request["cursor"].as_str().map(str::to_owned);
            let limit = request["limit"].as_u64().unwrap_or(4000).clamp(1, 4000) as usize;
            let value = host
                .executor
                .publication(move |store| {
                    cortex_core::workspace::read_report_page(
                        store,
                        &project,
                        &report,
                        cursor.as_deref(),
                        limit,
                    )
                })
                .map_err(err)?
                .await
                .map_err(|_| failure("native_host_closed"))?
                .map_err(err)?;
            return workspace_protocol::validate("ReportPage", &value)
                .map(|_| value)
                .map_err(|_| failure("invalid_report_page"));
        }
        if operation != "tasks.start" {
            return Err(failure("native_operation_unavailable"));
        }
        let _control = self.controls.lock().await;
        let replay_project = project.clone();
        let replay_request = request.clone();
        if let Some(receipt) = host
            .executor
            .publication({
                let incoming = client.is_some();
                move |store| {
                    if incoming {
                        return Ok(None);
                    }
                    cortex_core::workspace::replay_native_start_with_policy(
                        store,
                        &replay_project,
                        &actor(),
                        &replay_request,
                        policy,
                    )
                }
            })
            .map_err(err)?
            .await
            .map_err(|_| failure("native_host_closed"))?
            .map_err(err)?
        {
            return Ok(receipt);
        }
        // Candidate parsing is deliberately not authorization. Refuse before admission/effects.
        // A desktop draft's saved model/effort is qualified exactly as its coordinator
        // turns will route it (`task_route`), never against the owner default alone.
        let saved = if client.is_none() {
            let (p, t) = (
                project.clone(),
                request["target"].as_str().unwrap_or("").to_owned(),
            );
            host.executor
                .publication(move |store| {
                    cortex_core::workspace::draft_settings(store, &p, &actor(), &t)
                })
                .map_err(err)?
                .await
                .map_err(|_| failure("native_host_closed"))?
                .map_err(err)?
        } else {
            None
        };
        // A task's own route never needs a resolvable owner default (incoming
        // clients carry their settings; a desktop draft its saved ones).
        let pin_settings = saved
            .clone()
            .or_else(|| client.as_ref().map(|(_, settings, _)| settings.clone()));
        let coordinator = match &pin_settings {
            Some(settings) => self.routes.task_route(&host.root, settings),
            None => self.routes.route(&host.root, "orchestrator"),
        }
        .map_err(err)?;
        let pin = match &pin_settings {
            Some(settings) => self.routes.coordinator_pin(&host.root, settings),
            None => self.routes.coordinator_pin(&host.root, &json!({})),
        }
        .map_err(err)?;
        let mut jobs = self.jobs.lock().await;
        if *self.stop.borrow() {
            return Err(failure("service_stopping"));
        }
        jobs.retain(|j| !j.is_finished());
        if jobs.len() >= 32 {
            return Err(failure("native_capacity_unavailable"));
        }
        // Own admission before awaiting SQLite. Losing a window/request must not
        // strand a committed Start or cause a retry to launch a second process.
        let (send, reply) = tokio::sync::oneshot::channel();
        let runtime = self.clone();
        jobs.push(tokio::spawn(async move {
            let maintenance = runtime.host.workspace_state.clone();
            let admitted = async {
                host.executor
                    .publication(move |store| {
                        let _guard = maintenance
                            .as_deref()
                            .map(crate::machine::maintenance::admission)
                            .transpose()
                            .map_err(|e| CoreError::Refused(e.code))?;
                        cortex_core::workspace::start_native_input_with_policy(
                            store,
                            &project,
                            &actor(),
                            &request,
                            client
                                .as_ref()
                                .map(|(session, settings, _)| (session.as_str(), settings)),
                            client.as_ref().and_then(|(_, _, capture)| capture.as_ref()),
                            policy,
                        )
                    })
                    .map_err(err)?
                    .await
                    .map_err(|_| failure("native_host_closed"))?
                    .map_err(err)
            }
            .await;
            match admitted {
                Err(error) => {
                    let _ = send.send(Err(error));
                }
                Ok(admitted) => {
                    if admitted["fresh"] != true {
                        let _ = send.send(Ok(admitted["receipt"].clone()));
                        return;
                    }
                    let task = admitted["task"].as_str().unwrap().to_owned();
                    let owner = OwnerEpoch {
                        owner: admitted["owner"]["owner"].as_str().unwrap().into(),
                        epoch: admitted["owner"]["epoch"].as_i64().unwrap(),
                    };
                    let goal = admitted["goal"].as_str().unwrap().to_owned();
                    // Pin the resolved coordinator route with the task's start.
                    if let Some(pin) = pin {
                        let t = task.clone();
                        if let Ok(reply) = runtime.host.executor.write(move |db| {
                            cortex_core::lifecycle::event_tx(
                                db,
                                &t,
                                "NATIVE_COORDINATOR_ROUTE",
                                pin,
                            )?;
                            Ok(())
                        }) {
                            let _ = reply.await;
                        }
                    }
                    crate::native_preflight::record(
                        &runtime.host.executor,
                        &task,
                        &runtime.host.root,
                    )
                    .await;
                    let (cancel, cancelled) = watch::channel(false);
                    let (done, finished) = watch::channel(false);
                    runtime.install_pump(task.clone(), cancel, finished).await;
                    let _ = send.send(Ok(admitted["receipt"].clone()));
                    if let Err(error) = runtime
                        .drive(&task, &owner, &goal, coordinator, cancelled.clone())
                        .await
                    {
                        if *cancelled.borrow() || *runtime.stop.borrow() {
                            done.send_replace(true);
                            return;
                        }
                        runtime
                            .stop_after_error(&task, &owner, error, cancelled.clone())
                            .await;
                    }
                    runtime.driver_ended(&task, &cancelled).await;
                    done.send_replace(true);
                }
            }
        }));
        drop(jobs);
        reply
            .await
            .map_err(|_| failure("native_start_outcome_unknown"))?
    }
    /// One runtime turn with the degenerate-turn watchdog (see
    /// `native_sessions::degenerate`). A coordinator turn that degenerated without
    /// changing the task's state or plan revision runs once more, with the same
    /// prompt, in a fresh engine session; when that degenerates too, the coordinator
    /// moves to another qualified model (`NATIVE_COORDINATOR_ESCALATED`, preferably
    /// the other engine) and runs the turn there. Only when no other model exists, or
    /// it degenerates as well, does the turn end with `native_turn_degenerate`, which
    /// the no-stop policy turns into a continuation on an escalated rung, never an
    /// owner hold. A worker's degenerate turn ends with its node still open and its
    /// session terminated, so the lost-worker path replaces it in a fresh session on
    /// an escalated route (another model or engine) at the next step, every time.
    async fn turn(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        role: SessionRole,
        route: SessionRoute,
        prompt: &str,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        const DEGENERATE: &str = "native_turn_degenerate";
        let assignment = match &role {
            SessionRole::Worker { assignment } => Some(assignment.clone()),
            SessionRole::Coordinator => None,
        };
        let retry = route.clone();
        let before = self.turn_marker(task).await?;
        match self
            .turn_once(task, owner, role, route, prompt, cancelled.clone())
            .await
        {
            Err(error) if error.code() == DEGENERATE => {}
            other => return other,
        }
        if assignment.is_some() {
            // The replacement routes as an escalated redispatch of the same node.
            return Ok(());
        }
        // A degenerate turn that still changed the task (a plan, a question) stands;
        // the driver continues from that state as after any other turn.
        if self.turn_marker(task).await? != before {
            return Ok(());
        }
        match self
            .turn_once(
                task,
                owner,
                SessionRole::Coordinator,
                retry.clone(),
                prompt,
                cancelled.clone(),
            )
            .await
        {
            Err(error) if error.code() == DEGENERATE => {}
            other => return other,
        }
        if self.turn_marker(task).await? != before {
            return Ok(());
        }
        // Never the same model a third time: move the coordinator to another one.
        let current = self.coordinator_route(task, Some(retry)).await?;
        let Some(alternate) = self
            .escalate_coordinator(task, &current, DEGENERATE)
            .await?
        else {
            return Err(CoreError::Refused(DEGENERATE));
        };
        match self
            .turn_once(
                task,
                owner,
                SessionRole::Coordinator,
                alternate,
                prompt,
                cancelled,
            )
            .await
        {
            Err(error) if error.code() == DEGENERATE => Err(CoreError::Refused(DEGENERATE)),
            other => other,
        }
    }
    /// Move the task's coordinator to another qualified model than `current`
    /// (preferably another engine or account), recorded as
    /// `NATIVE_COORDINATOR_ESCALATED`. Later coordinator turns of the task use it
    /// until the owner chooses a model again. `None` when no other model qualifies.
    pub(super) async fn escalate_coordinator(
        &self,
        task: &str,
        current: &SessionRoute,
        reason: &str,
    ) -> CoreResult<Option<SessionRoute>> {
        let Some(pin) = self
            .routes
            .alternate_coordinator_pin(&self.host.root, current)?
        else {
            return Ok(None);
        };
        let route = match self.routes.pinned_route(&self.host.root, &pin, None) {
            Ok(route) => route,
            Err(_) => return Ok(None),
        };
        let t = task.to_owned();
        let record = json!({"pin":pin,"reason":reason,"from":{"connection":current.connection,"model":current.model},
            "to":{"connection":route.connection,"model":route.model}});
        self.host
            .executor
            .write(move |db| {
                cortex_core::lifecycle::event_tx(db, &t, "NATIVE_COORDINATOR_ESCALATED", record)?;
                Ok(())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        Ok(Some(route))
    }
    /// The task's state and plan revision: what a turn changed, if anything.
    async fn turn_marker(&self, task: &str) -> CoreResult<(String, i64)> {
        let t = task.to_owned();
        self.host
            .executor
            .read(move |db| {
                Ok(db.query_row(
                    "SELECT state,revision FROM runtime_tasks WHERE task_id=?",
                    [&t],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    async fn turn_once(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        role: SessionRole,
        mut route: SessionRoute,
        prompt: &str,
        mut cancelled: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        // The task's settings must still be readable before any turn starts.
        self.task_settings(task).await?;
        if matches!(role, SessionRole::Coordinator) {
            route = self.coordinator_route(task, Some(route)).await?;
            let t = task.to_owned();
            if self
                .host
                .executor
                .read(move |db| cortex_core::lifecycle::automatic::enabled(db, &t))?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??
            {
                self.routes
                    .automatic_coordinator_health(&self.host.root, &route)?;
            }
        }
        // The owner's language (an explicit en/ru setting, otherwise the goal's language)
        // is recorded in task_language and stated per role: owner-facing text follows it,
        // worker-to-worker material stays English.
        let t = task.to_owned();
        let language = self
            .host
            .executor
            .write(move |db| cortex_core::assignment_brief::record_language_tx(db, &t))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        route.instructions.push_str(&if matches!(role, SessionRole::Coordinator) {
            format!("\nOwner-selected response language: {language}. Write in it everything the owner reads directly: \
your messages and questions to the owner (question text and option labels), the final result text and its not_verified entries. \
Write plans, objectives and every instruction to workers in English; worker reports stay English, so compose the owner-facing text \
from them in {language}.")
        } else {
            format!("\nOwner-selected response language: {language}. user_question_language: {language}. It applies only to \
a question to the owner (question text and option labels). Write reports, report headings, summaries and every other message in English.")
        });
        let mut prompt = prompt.to_owned();
        if matches!(role, SessionRole::Coordinator) {
            if let Some(state) = self.host.workspace_state.clone() {
                let t = task.to_owned();
                let summary = self
                    .host
                    .executor
                    .read(move |db| {
                        crate::machine::sprint_graph::Scope::load(db, &state, &t)?
                            .map(|scope| scope.start(db))
                            .transpose()
                    })?
                    .await
                    .map_err(|_| CoreError::Refused("native_host_closed"))?;
                match summary {
                    Ok(Some(summary)) => prompt.push_str(&format!("\n\nSprint dependencies (Workspace records). Use sprint_graph to explore the graph and sprint_task_result to read a member’s outcome.\n{summary}")),
                    Ok(None) => {},
                    Err(error) => prompt.push_str(&format!("\n\nSprint context unavailable ({}).", error.code())),
                }
            }
            let t = task.to_owned();
            let instruction = self
                .host
                .executor
                .read(move |db| {
                    cortex_core::lifecycle::skill_proposals::planning_instruction(db, &t)
                })?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??;
            if !instruction.is_empty() {
                prompt.push_str(&format!("\n\n{instruction}"));
            }
        }
        let assignment = match &role {
            SessionRole::Worker { assignment } => Some(assignment.clone()),
            _ => None,
        };
        let is_worker = assignment.is_some();
        // Register owner guidance/stop before dispatch: dispatch already reports the
        // worker ready, and a message sent at that moment must reach it.
        let (sender, mut guidance) = tokio::sync::mpsc::channel(8);
        let (worker_stop, mut worker_cancelled) = watch::channel(false);
        let (worker_done, worker_finished) = watch::channel(false);
        if let Some(a) = &assignment {
            self.guidance.lock().await.insert(a.clone(), sender);
            self.worker_stops
                .lock()
                .await
                .insert(a.clone(), (worker_stop, worker_finished));
        }
        let driver = match self
            .host
            .dispatch(task.into(), owner.clone(), role, route)
            .await
        {
            Ok(driver) => driver,
            Err(error) => {
                if let Some(a) = &assignment {
                    self.guidance.lock().await.remove(a);
                    self.worker_stops.lock().await.remove(a);
                }
                return Err(error);
            }
        };
        driver.treat_refusal_as_degenerate();
        let mut stop = self.stop.subscribe();
        // How the turn ended, as the runtime observed it; recorded for a worker below.
        let turn: CoreResult<&'static str> = if *stop.borrow_and_update() || *cancelled.borrow() {
            Err(CoreError::Refused("native_service_stopped"))
        } else {
            tokio::select! {
                result = async {
                    let mut handled_refusal = if is_worker { driver.event_cursor().await? } else { 0 };
                    let mut continuations = 0;
                    self.guided_prompt(&driver, &prompt, &mut guidance).await?;
                    loop {
                        let wake = self.wake.notified();
                        tokio::pin!(wake);
                        wake.as_mut().enable();
                        use crate::native_sessions::SessionWait;
                        match driver.wait_state().await? {
                            SessionWait::Ready => { driver.continue_answer().await?; },
                            SessionWait::Open(retry) => tokio::select! {
                                _ = &mut wake => {},
                                _ = tokio::time::sleep(retry.unwrap_or_default()), if retry.is_some() => {},
                                // A worker process that dies while its question is open is
                                // noticed now, not when the answer arrives. The turn ends
                                // and the exited process is settled; the lost-worker path
                                // then replaces the assignment and retires its wait.
                                _ = driver.process_exited(), if is_worker => return Ok("process_exited"),
                                Some(message) = guidance.recv() => {
                                    let result = driver.prompt(&crate::native_owner_controls::guidance_prompt(&message)).await;
                                    driver.guidance_delivery(message, result.is_ok(), false).await?;
                                    result?;
                                }
                            },
                            _ if is_worker && continuations < 2 => {
                                let next=tokio::select!{
                                    result=driver.command_continuation(handled_refusal)=>result?,
                                    _=driver.process_exited()=>return Ok("process_exited"),
                                };
                                let Some((id,message))=next else {break;};
                                handled_refusal=id;
                                continuations+=1;
                                self.guided_prompt(&driver,&message,&mut guidance).await?;
                            }
                            _ => break,
                        }
                    }
                    Ok("turn_ended")
                } => result,
                _ = stop.changed() => Err(CoreError::Refused("native_service_stopped")),
                _ = cancelled.changed() => Err(CoreError::Refused("native_task_interrupted")),
                _ = worker_cancelled.changed() => Ok("owner_stopped"),
            }
        };
        // Always observe the owned wait, including prompt failure or service shutdown.
        if let Some(a) = &assignment {
            self.guidance.lock().await.remove(a);
        }
        let diagnostics = driver.diagnostics();
        let channel = driver.channel_id().to_owned();
        let settled = driver.shutdown().await;
        if settled.is_ok() {
            worker_done.send_replace(true);
        }
        if let Some(a) = &assignment {
            // Why this attempt ended, for the lost-worker path's classification.
            let cause = match &turn {
                Ok(cause) => *cause,
                Err(error) => error.code(),
            };
            if let Err(error) = self
                .record_worker_ending(task, a, &channel, cause, diagnostics.snapshot())
                .await
            {
                eprintln!(
                    "workspace-service: the end of a worker attempt was not recorded ({})",
                    error.code()
                );
            }
        }
        if let Some(a) = assignment {
            self.worker_stops.lock().await.remove(&a);
        }
        settled?;
        // result_submit can revoke its channel before ACP returns. Only a persisted
        // result/plan checked by advance permits progress, never transport loss alone.
        if let Err(e) = turn {
            if e.code() != "native_turn_outcome_unknown" {
                return Err(e);
            }
        }
        Ok(())
    }
    /// Records why a worker attempt that has no accepted result ended
    /// (`scheduler::retry::record_attempt_ending_tx`): the turn's end as the runtime
    /// observed it, with the engine's exit status or signal, transport close, a runtime
    /// cancel of a permission, an owner pause and a short redacted stderr tail. A turn
    /// that ended after refused results is the attempt's own quality
    /// (`result_refused`), as is a degenerate one (`degenerate_turn`); a process that
    /// vanished without the model ending its turn is `worker_lost`; the owner's pause
    /// or stop is `owner_paused`, a service stop `runtime_restart`, a command sandbox
    /// that could not be prepared `sandbox_unavailable`.
    async fn record_worker_ending(
        &self,
        task: &str,
        assignment: &str,
        channel: &str,
        cause: &str,
        diagnostics: cortex_engine::engine::DiagnosticSnapshot,
    ) -> CoreResult<()> {
        let (t, a, ch, cause) = (
            task.to_owned(),
            assignment.to_owned(),
            channel.to_owned(),
            cause.to_owned(),
        );
        self.host
            .executor
            .write(move |db| {
                use rusqlite::OptionalExtension;
                let Some(attempt) = db
                    .query_row(
                        "SELECT attempt_id FROM trusted_channels WHERE task_id=? AND channel_id=?",
                        (&t, &ch),
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten()
                else {
                    return Ok(());
                };
                let open: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND assignment_ref=?
                     AND result_json IS NULL AND status IN ('running','failed'))",
                    (&t, &a),
                    |r| r.get(0),
                )?;
                if !open {
                    return Ok(());
                }
                let facts = worker_ending_facts(db, &t, &ch, &attempt)?;
                let class = worker_ending_class(&cause, &facts);
                let mut ending = facts;
                ending["cause"] = json!(cause);
                ending["exit"] = json!(diagnostics.exit);
                ending["exited_unprompted"] = json!(diagnostics.exited_unprompted);
                ending["transport_close"] = json!(diagnostics.transport_close);
                ending["stderr_bytes"] = json!(diagnostics.stderr_bytes);
                ending["stderr_tail"] = json!(stderr_tail(&diagnostics.stderr_tail));
                cortex_core::scheduler::retry::record_attempt_ending_tx(
                    db, &t, &a, &attempt, &cause, class, &ending,
                )?;
                Ok(())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// Stop a driver that failed. Under the no-stop policy (every task) the stop is a
    /// continuation on the next rung of the strategy ladder
    /// (`cortex_core::lifecycle::automatic`), never an owner question or hold: the
    /// driver's end starts a [`Runtime::follow_continuation`] that relaunches the
    /// coordinator with a fresh plan once the host has settled the processes. A task
    /// that already waits on a question keeps it.
    pub(super) async fn stop_after_error(
        self: &Arc<Self>,
        task: &str,
        owner: &OwnerEpoch,
        mut error: CoreError,
        cancelled: watch::Receiver<bool>,
    ) {
        for round in 0..3 {
            if *cancelled.borrow() || *self.stop.borrow() {
                return;
            }
            let code = error.code().to_owned();
            let gap = code.starts_with("completion_");
            let t = task.to_owned();
            let owner_for_stop = owner.clone();
            let state = match self.host.executor.write(move |db| {
                cortex_core::lifecycle::event_tx(
                    db,
                    &t,
                    "NATIVE_DRIVER_STOPPED",
                    json!({"reason":code}),
                )?;
                // A task already waiting on an earlier version's open runtime stop
                // question keeps it: its answer relaunches the coordinator.
                let (current, _) = cortex_core::lifecycle::domain::task_tx(db, &t)?;
                let asking: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events e JOIN pending_questions q
                        ON q.task_id=e.task_id AND q.id=json_extract(e.payload_json,'$.question_id') AND q.status='open'
                     WHERE e.task_id=? AND e.type='USER_QUESTION_REQUESTED' AND json_extract(e.payload_json,'$.runtime_stop')=1)",
                    [&t],
                    |r| r.get(0),
                )?;
                if current == cortex_core::TaskState::WaitingUser && asking {
                    return Ok(current);
                }
                cortex_core::ownership::check_tx(db, &t, &owner_for_stop)?;
                cortex_core::lifecycle::domain::stop_tx(db, &t, &code)
            }) {
                Ok(reply) => reply.await.ok().and_then(|r| r.ok()),
                Err(_) => None,
            };
            // A completion gap that needs the owner's decision about the work (a
            // replaced failed check) stays RECONCILING for its coordinator, which asks:
            // drive that turn here, or nothing would ask. Every other stop is a
            // continuation the driver's end follows.
            if round == 2 || !gap || state != Some(cortex_core::TaskState::Reconciling) {
                return;
            }
            match self.progress(task, owner, cancelled.clone()).await {
                Ok(()) => return,
                Err(next) => error = next,
            }
        }
    }
    /// A driver of `task` ended on its own (not cancelled, the service not stopping):
    /// continue whatever the no-stop policy now owes it.
    pub(super) async fn driver_ended(
        self: &Arc<Self>,
        task: &str,
        cancelled: &watch::Receiver<bool>,
    ) {
        if !*cancelled.borrow() && !*self.stop.borrow() {
            self.follow_continuation(task.to_owned()).await;
        }
    }
    /// The runtime's own recheck of what a parked task waits for, without any model
    /// call: each due wait is checked against the built-in endpoint policy before
    /// any TCP connect or HTTPS HEAD, and the outcome is recorded
    /// (`EXTERNAL_WAIT_RECHECKED`, `EXTERNAL_WAIT_ENDED`); an open owner action that an
    /// environment preflight can show fixed (GitHub CLI, Codex or Claude login,
    /// passwordless sudo) is rechecked with the same backoff and resolved
    /// (`OWNER_ACTION_RESOLVED`, `by: environment_recheck`) once the fact is present.
    pub(super) async fn recheck_obstacles(&self, task: &str) -> CoreResult<()> {
        let now = unix_seconds();
        let t = task.to_owned();
        let (due, services) = self
            .host
            .executor
            .read(move |db| {
                Ok((
                    cortex_core::lifecycle::obstacles::due_waits(db, &t, now)?,
                    cortex_core::lifecycle::obstacles::environment_services(db, &t)?,
                ))
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        for wait in due {
            let (ok, detail) = probe_external(&wait).await;
            let (t, id) = (
                task.to_owned(),
                wait["id"].as_str().unwrap_or("").to_owned(),
            );
            self.host
                .executor
                .write(move |db| {
                    cortex_core::lifecycle::obstacles::record_recheck_tx(
                        db, &t, &id, ok, now, &detail,
                    )
                    .map(|_| ())
                })?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??;
        }
        if !services.is_empty() && environment_recheck_due(task) {
            let facts = crate::native_preflight::facts(&self.host.root).await;
            let t = task.to_owned();
            self.host
                .executor
                .write(move |db| {
                    cortex_core::lifecycle::obstacles::resolve_by_environment_tx(db, &t, &facts)
                        .map(|_| ())
                })?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??;
        }
        Ok(())
    }
    async fn drive(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        goal: &str,
        route: SessionRoute,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        if *self.stop.borrow() {
            return Err(CoreError::Refused("native_service_stopped"));
        }
        // Owner messages recorded for the next boundary (for example before a restart
        // or a later message's relaunch), and queued ones, reach this coordinator turn.
        self.take_queued(task).await?;
        let t = task.to_owned();
        let messages = self
            .host
            .executor
            .write(move |db| cortex_core::lifecycle::steering::deliver_tx(db, &t))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        let goal = if messages.is_empty() {
            goal.to_owned()
        } else {
            format!(
                "{goal}\n\n{}",
                cortex_core::lifecycle::steering::guidance_prompt(&messages)
            )
        };
        // The initial (or next) runtime pipeline edition precedes the coordinator turn.
        crate::native_pipeline::publish(&self.host.executor, task, Some(&route)).await;
        let turn = self
            .turn(
                task,
                owner,
                SessionRole::Coordinator,
                route,
                &goal,
                cancelled.clone(),
            )
            .await;
        self.confirm_messages(task).await?;
        turn?;
        self.progress(task, owner, cancelled).await
    }
    /// The owner's queued messages for this task (the machine's `product.message`
    /// queue) are taken by the service, window open or not, when the task is at a
    /// boundary of the coordinator's work: each is recorded as a boundary message under
    /// its own id, in queue order, so it reaches the next coordinator turn. Until taken a
    /// message stays editable; a take interrupted by a restart is finished by the next one
    /// (the message id makes it idempotent).
    pub(super) async fn take_queued(&self, task: &str) -> CoreResult<()> {
        let Some(state) = self.host.workspace_state.clone() else {
            return Ok(());
        };
        let t = task.to_owned();
        let bound: Option<(String, String, String, String)> = self
            .host
            .executor
            .read(move |db| {
                use rusqlite::OptionalExtension;
                Ok(db
                    .query_row(
                        "SELECT w.project_id,w.task_id,e.execution_id,r.state FROM workspace_executions e
                         JOIN workspace_tasks w ON w.task_id=e.task_id AND w.execution_id=e.execution_id
                         JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE e.runtime_task_id=?",
                        [&t],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        let Some((project, draft, execution, current)) = bound else {
            return Ok(());
        };
        if !["PLANNING", "PLAN_READY", "RECONCILING", "ANSWER_READY"].contains(&current.as_str()) {
            return Ok(());
        }
        let (s, p, d) = (state.clone(), project.clone(), draft.clone());
        let taken =
            tokio::task::spawn_blocking(move || crate::machine::take_queued_messages(&s, &p, &d))
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))?
                .map_err(|e| CoreError::Refused(e.code))?;
        for message in taken {
            let id = message["id"].as_str().unwrap_or_default().to_owned();
            let outcome = self
                .record_queued(&state, &project, &execution, &message)
                .await;
            let settled = outcome.map_err(|e| e.code().to_owned());
            let (s, p) = (state.clone(), project.clone());
            tokio::task::spawn_blocking(move || {
                crate::machine::settle_taken_message(&s, &p, &id, settled)
            })
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
            .map_err(|e| CoreError::Refused(e.code))?;
        }
        Ok(())
    }
    /// Admit and finish one taken queued message as a boundary message, idempotently.
    async fn record_queued(
        &self,
        state: &std::path::Path,
        project: &str,
        execution: &str,
        message: &Value,
    ) -> CoreResult<()> {
        let mut files = json!({"attachments":message["attachments"].clone()});
        if !files["attachments"].is_array() {
            files["attachments"] = json!([]);
        }
        crate::machine::attachment_files::inline_for_delivery(state, &mut files);
        let capture = match files["attachments"].as_array() {
            Some(items) if !items.is_empty() => Some(cortex_core::input::capture_from_wire(
                &files["attachments"],
            )?),
            _ => None,
        };
        let request = cortex_core::workspace::control::canonical_request(
            json!({"projectId":project,"executionId":execution,"requestId":message["id"],"action":"prompt",
                "text":message["text"],"attachments":[],"queued":true}),
            capture.as_ref(),
        )?;
        let p = project.to_owned();
        self.host
            .executor
            .publication(move |s| {
                use cortex_core::workspace::control;
                let actor = actor();
                match control::replay(s, &p, &actor, &request) {
                    Ok(Some(_)) => return Ok(()),
                    // Admitted by an earlier take that stopped before its finish.
                    Err(e) if e.code() == "native_control_outcome_unknown" => {}
                    Err(e) if e.code() == "payload_conflict" => return Ok(()),
                    Err(e) => return Err(e),
                    Ok(None) => {
                        match control::admit_input(s, &p, &actor, &request, capture.as_ref()) {
                            Ok(_) => {}
                            // Sent by the owner (Send now) under the same id meanwhile.
                            Err(e)
                                if matches!(
                                    e.code(),
                                    "native_control_already_recorded" | "payload_conflict"
                                ) =>
                            {
                                return Ok(())
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
                control::finish(s, &p, &actor, &request, false).map(|_| ())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// Record the owner messages a finished coordinator turn's confirmed prompt carried.
    async fn confirm_messages(&self, task: &str) -> CoreResult<()> {
        let t = task.to_owned();
        self.host
            .executor
            .write(move |db| cortex_core::lifecycle::steering::confirm_tx(db, &t).map(|_| ()))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// Whether an owner message waits for the next coordinator turn.
    pub(super) async fn recover_planning(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        // SessionDriver rebuilds the capsule on its serial writer lane immediately
        // before delivery; never reuse a snapshot captured before replacement.
        let mut goal=String::from("Coordinator recovery on a new engine session. The prior process has a confirmed terminal observation. Do not replay commands, permissions or claimed deliveries. Continue from the fresh retained-state capsule; missing outcomes remain unresolved.");
        goal.push_str(&self.recovery_failure_guidance(task).await?);
        let route = self.coordinator_route(task, None).await?;
        self.drive(task, owner, &goal, route, cancelled).await
    }

    /// Point the coordinator at durable failed reports. These are historical
    /// references; the coordinator reads the actual checks before proposing a repair.
    async fn recovery_failure_guidance(&self, task: &str) -> CoreResult<String> {
        let task = task.to_owned();
        self.host.executor.read(move |db| {
            let mut stmt = db.prepare(
                "SELECT json_extract(result_json,'$.report_ref') FROM runtime_nodes WHERE task_id=?
                 AND status='failed' AND json_type(result_json,'$.report_ref')='text'
                 ORDER BY revision DESC,rowid DESC LIMIT 9",
            )?;
            let mut reports = stmt.query_map([&task], |r| r.get::<_,String>(0))?
                .collect::<Result<Vec<_>,_>>()?;
            if reports.is_empty() { return Ok(String::new()); }
            let more = reports.len() > 8;
            reports.truncate(8);
            Ok(format!("\nPreviously failed worker reports: {}. {}Read relevant reports, assign in-scope fixes to a writer, and rerun each outstanding check with the same argv and working directory. A passing final check can cover a project command at the same working directory.", reports.join(", "),
                if more { "More reports are available in the task catalogue. " } else { "" }))
        })?.await.map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    pub(super) async fn install_pump(
        &self,
        task: String,
        cancel: watch::Sender<bool>,
        done: watch::Receiver<bool>,
    ) {
        // The map lock is held across the wait so two installers cannot interleave:
        // a task never has two drivers. A dropped sender means the prior driver is gone.
        let mut pumps = self.pumps.lock().await;
        if let Some(prior) = pumps.remove(&task) {
            prior.cancel.send_replace(true);
            let mut done = prior.done;
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        pumps.insert(task, Pump { cancel, done });
    }
    pub(super) async fn spawn_recovery(
        &self,
        job: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let mut jobs = self.jobs.lock().await;
        jobs.retain(|j| !j.is_finished());
        jobs.push(tokio::spawn(job));
    }
    pub(super) async fn progress(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        loop {
            if *self.stop.borrow() || *cancelled.borrow() {
                return Err(CoreError::Refused("native_service_stopped"));
            }
            let t = task.to_owned();
            if self
                .host
                .executor
                .read(move |db| cortex_core::lifecycle::automatic::held(db, &t))?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??
            {
                return Ok(());
            }
            self.spend_progress(task).await?;
            loop {
                if *self.stop.borrow() || *cancelled.borrow() {
                    return Err(CoreError::Refused("native_service_stopped"));
                }
                let t = task.to_owned();
                let waiting = self
                    .host
                    .executor
                    .write(move |db| {
                        // A lost worker's wait that can never continue is closed
                        // here, or the driver would wait on it forever.
                        cortex_core::lifecycle::waits::cancel_lost_tx(db, &t)?;
                        Ok(cortex_core::lifecycle::domain::task_tx(db, &t)?.0
                            == cortex_core::TaskState::WaitingUser
                            || cortex_core::lifecycle::waits::projection_tx(db, &t)?
                                .iter()
                                .any(|w| {
                                    w["status"] == "open" && w["continuation_required"] == true
                                }))
                    })?
                    .await
                    .map_err(|_| CoreError::Refused("native_host_closed"))??;
                if !waiting {
                    break;
                }
                tokio::select! {
                    _ = self.wake.notified() => {},
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
                }
            }
            let step = loop {
                if *self.stop.borrow() || *cancelled.borrow() {
                    return Err(CoreError::Refused("native_service_stopped"));
                }
                // Messages the owner queued are taken at a boundary, before the step
                // that could otherwise start a plan or complete past them.
                self.take_queued(task).await?;
                let step = self
                    .host
                    .advance_step(task.into(), owner.clone(), true)
                    .await?;
                // Verification just passed: take queued messages before reconciling.
                if step["automaticHold"] == true {
                    return Ok(());
                }
                if step["recheck"] == true {
                    continue;
                }
                if !step["assignments"].as_array().is_some_and(|a| a.is_empty()) {
                    break step;
                }
                // A settled replacement waits out its retained retry backoff.
                // Waiting does not consume the bounded progress/dispatch budget.
                let t = task.to_owned();
                let delayed = self.host.executor.read(move |db| {
                    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                        .map_err(|_|CoreError::Refused("clock_unavailable"))?.as_secs_f64();
                    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_nodes n
                        JOIN runtime_tasks t USING(task_id) WHERE n.task_id=? AND n.revision=t.revision
                        AND n.status='pending' AND EXISTS(SELECT 1 FROM runtime_events e
                            WHERE e.id=(SELECT max(x.id) FROM runtime_events x WHERE x.task_id=n.task_id
                                AND x.type='ASSIGNMENT_RETRIED' AND json_extract(x.payload_json,'$.node')=n.node_id
                                AND json_extract(x.payload_json,'$.revision')=n.revision)
                            AND json_extract(e.payload_json,'$.retry_not_before')>?))",
                        (&t,now),|r|r.get::<_,bool>(0))?)
                })?.await.map_err(|_|CoreError::Refused("native_host_closed"))??;
                if !delayed {
                    // A ready step whose files another task's worker still holds is
                    // queued behind that holding, never stopped as a dead end. The wait is
                    // recorded once per holder; it ends when the holding is released.
                    let waited = self.wait_for_write_holding(task, owner, &cancelled).await?;
                    // The holder may finish between the failed claim and this lookup.
                    if !waited && step["writeConflict"] != true {
                        break step;
                    }
                    continue;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            };
            // Each boundary publishes the pipeline edition its durable state calls for.
            crate::native_pipeline::publish(&self.host.executor, task, None).await;
            // The runtime itself stopped the task with its own question while advancing
            // (a blocking review, a failed final check, a failed worker): that question is
            // the stop, and its answer relaunches the coordinator (`continue_owed_tasks`).
            // This driver ends here; it is not a step that could not continue, and it
            // never replaces the owner's question with a second, different stop.
            if matches!(
                step["state"].as_str(),
                Some("COMPLETED" | "FAILED" | "CANCELLED" | "WAITING_USER")
            ) {
                return Ok(());
            }
            if matches!(
                step["state"].as_str(),
                Some("RECOVERING" | "PLANNING" | "RECONCILING")
            ) {
                // Continue on the task's coordinator regardless of mode or steering.
                // Resolve it before recording delivery of any owner messages.
                self.take_queued(task).await?;
                let route = self.coordinator_route(task, None).await?;
                let t = task.to_owned();
                let o = owner.clone();
                let messages = self.host.executor.write(move|db|{
                    cortex_core::ownership::check_tx(db,&t,&o)?;
                    if cortex_core::lifecycle::domain::task_tx(db,&t)?.0==cortex_core::TaskState::Recovering {
                        cortex_core::lifecycle::domain::settle_recovery_tx(db,&t,cortex_core::TaskState::Planning)?;
                    }
                    let count:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='NATIVE_COORDINATOR_CONTINUED'
                        AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=? AND type IN
                        ('PLAN_ACCEPTED','VERIFICATION_PASSED','VERIFICATION_FAILED','REPLAN_REQUEST_ACCEPTED','USER_ANSWER_RECEIVED','USER_STEERING','AUTOMATIC_RESUMED')),0)",(&t,&t),|r|r.get(0))?;
                    if count>=3 { return Err(CoreError::Refused("coordinator_no_progress_limit")); }
                    let messages=cortex_core::lifecycle::steering::deliver_tx(db,&t)?;
                    cortex_core::lifecycle::event_tx(db,&t,"NATIVE_COORDINATOR_CONTINUED",json!({"state":cortex_core::lifecycle::domain::task_tx(db,&t)?.0}))?;
                    Ok(messages)
                })?.await.map_err(|_|CoreError::Refused("native_host_closed"))??;
                let mut prompt = String::from("Continue from the retained-state capsule. Reconcile verified work with synthesis_submit, ask a structured user question only for a decision about the work itself (never about runtime failures, retries or limits) and keep working on everything it does not block, propose a scoped repair plan for failing project code and keep each unresolved verification command in it. Use verification_submit only to correct the verification procedure or rerun the reviewed work. Never certify missing evidence.");
                if step["state"] != "RECONCILING" {
                    prompt.push_str(&self.recovery_failure_guidance(task).await?);
                }
                if !messages.is_empty() {
                    prompt.push_str("\n\n");
                    prompt.push_str(&cortex_core::lifecycle::steering::guidance_prompt(
                        &messages,
                    ));
                }
                let turn = self
                    .turn(
                        task,
                        owner,
                        SessionRole::Coordinator,
                        route,
                        &prompt,
                        cancelled.clone(),
                    )
                    .await;
                self.confirm_messages(task).await?;
                turn?;
                continue;
            }
            let assignments = step["assignments"]
                .as_array()
                .filter(|a| !a.is_empty())
                .ok_or(CoreError::Refused("native_continuation_unavailable"))?;
            // Claimed workers of one batch run their turns concurrently, each with its own
            // session, channel, write reservation and result. One failing turn, a stale plan
            // or a cancellation stops the others through `batch`; nothing is dropped mid-turn.
            let (batch, batch_cancelled) = watch::channel(false);
            let stale = std::sync::atomic::AtomicBool::new(false);
            let revision = step["revision"]
                .as_i64()
                .ok_or(CoreError::Refused("native_revision_missing"))?;
            let turn = |assignment: Value| {
                let stop = batch_cancelled.clone();
                let batch = &batch;
                let stale = &stale;
                async move {
                    if *stop.borrow() {
                        return Ok(());
                    }
                    let result = self.run_assignment(task, owner, &assignment, stop).await;
                    self.settle_batch_turn(task, result, batch, stale).await
                }
            };
            let mut turns: FuturesUnordered<_> = assignments.iter().cloned().map(turn).collect();
            let mut parent = cancelled.clone();
            let mut first: Option<CoreError> = None;
            while !turns.is_empty() {
                if *parent.borrow() || *self.stop.borrow() {
                    batch.send_replace(true);
                }
                let result = tokio::select! {
                    biased;
                    Ok(()) = parent.changed(), if !*batch_cancelled.borrow() => continue,
                    Some(result) = turns.next() => result,
                };
                if let Err(error) = result {
                    if first
                        .as_ref()
                        .is_none_or(|e| e.code() == "native_task_interrupted")
                    {
                        first = Some(error);
                    }
                    batch.send_replace(true);
                }
                // Always settle every launched session, including on a refill error.
                if turns.is_empty()
                    || *batch_cancelled.borrow()
                    || *parent.borrow()
                    || *self.stop.borrow()
                {
                    continue;
                }
                match self
                    .host
                    .refill_execution(task.into(), owner.clone(), revision)
                    .await
                {
                    Ok(next) => turns.extend(next.into_iter().map(turn)),
                    Err(error) => {
                        first = Some(error);
                        batch.send_replace(true);
                    }
                }
            }
            if stale.load(std::sync::atomic::Ordering::SeqCst)
                && first
                    .as_ref()
                    .is_some_and(|e| e.code() == "native_task_interrupted")
            {
                first = None;
            }
            if let Some(error) = first {
                return Err(error);
            }
        }
    }
    /// Close one worker turn of a batch. Every exit path, including a failed state read,
    /// signals the batch, so siblings never keep running after this turn is lost.
    ///
    /// A worker question normally leaves the task Executing/Reviewing (only the asking
    /// worker waits). The task moves to WAITING_USER only when the worker has no engine
    /// session reference; then the whole task waits and sibling channels are no longer
    /// authorised (workers act only while EXECUTING/REVIEWING), so they are stopped.
    async fn settle_batch_turn(
        &self,
        task: &str,
        result: CoreResult<()>,
        batch: &watch::Sender<bool>,
        stale: &std::sync::atomic::AtomicBool,
    ) -> CoreResult<()> {
        if result.is_err() {
            batch.send_replace(true);
            return result;
        }
        let t = task.to_owned();
        let state = async {
            Ok::<_, CoreError>(
                self.host
                    .executor
                    .write(move |db| cortex_core::lifecycle::domain::task_tx(db, &t))?
                    .await
                    .map_err(|_| CoreError::Refused("native_host_closed"))??
                    .0,
            )
        }
        .await;
        match state {
            Ok(state) => {
                // A worker proposal can cancel siblings already claimed in this batch.
                if !matches!(
                    state,
                    cortex_core::TaskState::Executing | cortex_core::TaskState::Reviewing
                ) {
                    stale.store(true, std::sync::atomic::Ordering::SeqCst);
                    batch.send_replace(true);
                }
                Ok(())
            }
            Err(error) => {
                batch.send_replace(true);
                Err(error)
            }
        }
    }
    /// The progress budget is persisted (`cortex_core::lifecycle::progress`), so a
    /// restarted driver continues the count instead of resetting it.
    async fn spend_progress(&self, task: &str) -> CoreResult<()> {
        let t = task.to_owned();
        self.host
            .executor
            .write(move |db| {
                cortex_core::lifecycle::progress::spend_tx(
                    db,
                    &t,
                    cortex_core::lifecycle::progress::PROGRESS_BUDGET,
                )
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    /// Whether a ready step of the task waited for another task's write holding
    /// (`cortex_core::scheduler::policy::write_conflict_tx`). Returns `false` at once
    /// when nothing is held; otherwise records `NATIVE_WRITE_CONFLICT_WAITING` once for
    /// the holder, waits until the holding is gone (or the driver is stopped) and
    /// returns `true` so the caller claims again. Waiting spends no progress budget.
    async fn wait_for_write_holding(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        cancelled: &watch::Receiver<bool>,
    ) -> CoreResult<bool> {
        let mut waited = false;
        loop {
            if *self.stop.borrow() || *cancelled.borrow() {
                return Err(CoreError::Refused("native_service_stopped"));
            }
            let (t, o) = (task.to_owned(), owner.clone());
            let conflict = self
                .host
                .executor
                .write(move |db| {
                    cortex_core::ownership::check_tx(db, &t, &o)?;
                    // Another task's open request for these files is queued ahead of this
                    // step the same way: it waits for that request, never stops.
                    let queued = || -> CoreResult<Option<Value>> {
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                            .map_err(|_| CoreError::Refused("clock_unavailable"))?.as_secs_f64();
                        Ok(cortex_core::scheduler::policy::claim_obstacles_tx(db, &t, now)?.into_iter()
                            .find(|o| o["reason"] == "access_wait" && o["holder_task"].as_str().is_some_and(|h| h != t))
                            .map(|o| json!({"node":o["node"],"holder_task":o["holder_task"],"paths":o["scope"],"access_request":true})))
                    };
                    let Some(conflict) = cortex_core::scheduler::policy::write_conflict_tx(db, &t)?.map(Ok).or_else(|| queued().transpose()).transpose()? else {
                        return Ok(false);
                    };
                    // One record per holder since the latest plan or owner message.
                    let recorded: bool = db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND type='NATIVE_WRITE_CONFLICT_WAITING'
                         AND json_extract(payload_json,'$.holder_task')=?2
                         AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type IN
                            ('PLAN_ACCEPTED','USER_STEERING','USER_ANSWER_RECEIVED','ASSIGNMENT_CREATED')),0))",
                        (&t, conflict["holder_task"].as_str().unwrap_or_default()),
                        |r| r.get(0),
                    )?;
                    if !recorded {
                        cortex_core::lifecycle::event_tx(db, &t, "NATIVE_WRITE_CONFLICT_WAITING", conflict)?;
                    }
                    Ok(true)
                })?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??;
            if !conflict {
                return Ok(waited);
            }
            waited = true;
            tokio::select! {
                _ = self.wake.notified() => {},
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
            }
        }
    }
    /// Select a route for one claimed assignment, record it and run the worker's turn.
    async fn run_assignment(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        assignment: &Value,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        let role = assignment["role"]
            .as_str()
            .ok_or(CoreError::Refused("native_role_missing"))?;
        let t = task.to_owned();
        let a = assignment["assignment_ref"].as_str().unwrap().to_owned();
        let root = self.host.root.clone();
        let assigned_role = role.to_owned();
        let (node,evidence,escalated,previous,revision)=self.host.executor.read(move |db| {
            let (plan,node_id,stage,revision):(Option<String>,String,String,i64)=db.query_row(
                "SELECT t.plan_json,n.node_id,n.stage,t.revision FROM runtime_nodes n JOIN runtime_tasks t USING(task_id) WHERE n.task_id=? AND n.assignment_ref=?",
                (&t,&a),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            let plan:Value=serde_json::from_str(plan.as_deref().unwrap_or("{}"))?;
            // Planned nodes route by their own fields; fix and review nodes by the
            // routing recorded when the runtime created them.
            let mut node=cortex_core::lifecycle::plan::node_routing(&plan,&node_id);
            if !node.is_object(){node=json!({});}
            node["id"]=json!(node_id);
            // Only work that fell short escalates (`automatic::node_escalated`): this
            // node's own failure, a repeated review fix, or the one plan revision right
            // after an escalating ladder rung. A retry after an infrastructure failure
            // keeps its normal tier. An escalated dispatch moves away from the engine of
            // the attempt that fell short.
            let escalated=cortex_core::lifecycle::automatic::node_escalated(db,&t,&node_id,revision)?;
            let mut previous=None;
            if escalated {
                previous=cortex_core::routing::previous_dispatch_engine(db,&t,&node_id,revision)?;
                if previous.is_none() {
                    previous=cortex_core::lifecycle::automatic::escalation_engine(db,&t,&node_id,&assigned_role)?;
                }
            }
            if stage=="review"{
                // Independence: the reviewer should run on a model other than the
                // reviewed execution nodes' models (and preferably another engine).
                // Each author carries its node and the number of files its sessions
                // recorded writing or changing, which decides the dominant author.
                let mut stmt=db.prepare("SELECT json_extract(e.payload_json,'$.route_engine'),json_extract(e.payload_json,'$.model'),json_extract(e.payload_json,'$.node'),
                    (SELECT count(DISTINCT json_extract(w.payload_json,'$.path')) FROM runtime_events w JOIN trusted_channels c
                        ON c.task_id=w.task_id AND c.channel_id=json_extract(w.payload_json,'$.channel_id')
                     WHERE w.task_id=e.task_id AND w.type IN ('NATIVE_PROJECT_WRITTEN','NATIVE_PROJECT_CHANGED')
                     AND c.assignment_id=json_extract(e.payload_json,'$.assignment_ref'))
                    FROM runtime_events e WHERE e.task_id=? AND e.type='NATIVE_ROUTE_SELECTED' AND json_extract(e.payload_json,'$.revision')=?
                    AND json_extract(e.payload_json,'$.node') IN (SELECT node_id FROM runtime_nodes WHERE task_id=e.task_id AND revision=? AND stage='execution') ORDER BY e.id")?;
                let reviewed=stmt.query_map((&t,revision,revision),|r|Ok(json!({"engine":r.get::<_,Option<String>>(0)?,"model":r.get::<_,Option<String>>(1)?,
                    "node":r.get::<_,Option<String>>(2)?,"changes":r.get::<_,i64>(3)?})))?
                    .collect::<Result<Vec<_>,_>>()?;
                node["avoid_routes"]=json!(reviewed);
                // Earlier reviews of this revision, and which rerun this review is.
                let previous=db.prepare("SELECT json_extract(e.payload_json,'$.route_engine'),json_extract(e.payload_json,'$.model') FROM runtime_events e
                    WHERE e.task_id=? AND e.type='NATIVE_ROUTE_SELECTED' AND json_extract(e.payload_json,'$.revision')=? AND json_extract(e.payload_json,'$.node')!=?
                    AND json_extract(e.payload_json,'$.node') IN (SELECT node_id FROM runtime_nodes WHERE task_id=e.task_id AND revision=? AND stage='review') ORDER BY e.id")?
                    .query_map((&t,revision,&node_id,revision),|r|Ok(json!({"engine":r.get::<_,Option<String>>(0)?,"model":r.get::<_,Option<String>>(1)?})))?
                    .collect::<Result<Vec<_>,_>>()?;
                node["previous_reviewers"]=json!(previous);
                let rerun:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=?1 AND type='REVIEW_RERUN' AND json_extract(payload_json,'$.revision')=?2
                    AND id<=coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='REVIEW_RERUN' AND json_extract(payload_json,'$.rerun')=?3),0)",
                    (&t,revision,&node_id),|r|r.get(0))?;
                node["review_rerun"]=json!(rerun);
            }
            // A model that lost this node's lineage twice to infrastructure (the session
            // vanished, the engine failed) is routed away from, without escalating the
            // tier: another model when one is usable, preferably another engine.
            let lost=lost_models(db,&t,&node_id)?;
            if !lost.is_empty(){
                let field=if stage=="review"{"previous_reviewers"}else{"avoid_routes"};
                let mut list=node[field].as_array().cloned().unwrap_or_default();
                list.extend(lost.iter().cloned());
                node[field]=json!(list);
                node["lost_routes"]=json!(lost);
            }
            // What a restart left with an unknown outcome is re-verified by the fresh attempt.
            let reverify:Option<String>=db.query_row("SELECT json_extract(payload_json,'$.reverify') FROM runtime_events
                WHERE task_id=? AND type='ASSIGNMENT_RETRIED' AND json_extract(payload_json,'$.node')=? AND json_extract(payload_json,'$.revision')=?
                ORDER BY id DESC LIMIT 1",(&t,&node_id,revision),|r|r.get(0)).or_else(|e|match e{rusqlite::Error::QueryReturnedNoRows=>Ok(None),e=>Err(e)})?;
            if let Some(reverify)=reverify.and_then(|r|serde_json::from_str::<Value>(&r).ok()).filter(|r|r.as_array().is_some_and(|a|!a.is_empty())){
                node["reverify_effects"]=reverify;
            }
            Ok((node,cortex_core::routing::evidence(db,&t,&root)?,escalated,previous,revision))
        })?.await.map_err(|_|CoreError::Refused("native_host_closed"))??;
        let (route, mut decision) = match self.routes.worker_selection(
            &self.host.root,
            role,
            &node,
            &evidence,
            escalated,
            previous.as_deref(),
        ) {
            Ok(selected) => selected,
            Err(error) => {
                if cortex_core::lifecycle::automatic::route_blocked(error.code()) {
                    let t = task.to_owned();
                    let probe = json!({"reason":error.code(),"revision":revision,"role":role,
                        "node":node,"escalated":escalated,"previous":previous});
                    self.host
                        .executor
                        .write(move |db| {
                            cortex_core::lifecycle::event_tx(
                                db,
                                &t,
                                "NATIVE_WORKER_ROUTE_BLOCKED",
                                probe,
                            )?;
                            Ok(())
                        })?
                        .await
                        .map_err(|_| CoreError::Refused("native_host_closed"))??;
                }
                return Err(error);
            }
        };
        // Review independence facts come from the route actually chosen, whoever
        // produced it: the same model as the reviewed work is a visible warning.
        if let Some(avoid) = node["avoid_routes"].as_array().filter(|a| !a.is_empty()) {
            let same_model = avoid.iter().any(|a| a["model"] == json!(route.model));
            let same_engine = !decision["route_engine"].is_null()
                && avoid
                    .iter()
                    .any(|a| a["engine"] == decision["route_engine"]);
            decision["independence"] = if same_model {
                json!({"status":"warning","mode":"same_model_fresh_session","model":route.model,"same_model_as_reviewed":true,
                    "same_engine_as_reviewed":same_engine,
                    "warning":"The reviewer runs on a model that also wrote part of the work it reviews, in a fresh session: no other model the owner prefers for review was usable."})
            } else {
                json!({"status":"independent","model":route.model,"same_engine_as_reviewed":same_engine})
            };
        }
        let t = task.to_owned();
        let a = assignment.clone();
        let selected = json!({"assignment_ref":a["assignment_ref"],"node":a["node"],"revision":revision,"route_engine":decision["route_engine"],"engine":match route.launch.engine {
            cortex_engine::environment::EngineKind::Codex=>"codex-acp",
            cortex_engine::environment::EngineKind::Claude=>"claude-agent-acp",
            cortex_engine::environment::EngineKind::OpenCode=>"opencode"},
            "model":route.model,"connection_id":route.connection,"parameters":route.parameters,"decision":decision});
        self.host
            .executor
            .write(move |db| {
                cortex_core::lifecycle::event_tx(db, &t, "NATIVE_ROUTE_SELECTED", selected)?;
                Ok(())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        // A restart left effects of the previous attempt with an unknown outcome: the
        // fresh attempt re-verifies them from the files, Git state and receipts first.
        let mut brief = assignment.clone();
        if let Some(effects) = node.get("reverify_effects") {
            brief["reverify_effects"] = effects.clone();
            brief["reverify_instruction"] = json!("The service restarted while the previous attempt of this assignment ran. These effects of that attempt may or may not have taken place: re-verify each one from the files, Git state and receipts before redoing it; never replay it blindly.");
        }
        self.turn(
            task,
            owner,
            SessionRole::Worker {
                assignment: assignment["assignment_ref"].as_str().unwrap().into(),
            },
            route,
            &brief.to_string(),
            cancelled,
        )
        .await
    }
    /// Whether a driver of this task still runs (installed and not finished).
    pub(super) async fn driven(&self, task: &str) -> bool {
        self.pumps
            .lock()
            .await
            .get(task)
            .is_some_and(|pump| pump.done.has_changed().is_ok() && !*pump.done.borrow())
    }
    pub(super) async fn settle_task(&self, task: &str) -> CoreResult<()> {
        let done = {
            let pumps = self.pumps.lock().await;
            pumps.get(task).map(|pump| {
                pump.cancel.send_replace(true);
                pump.done.clone()
            })
        };
        // A dropped sender means the driver job is gone (it ended without signalling,
        // e.g. it panicked): no driver of this task runs any more. This only fences
        // drivers; whether their engine processes exited is decided separately by the
        // durable provisioning/reservation rows that `control::finish` checks.
        if let Some(mut done) = done {
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        Ok(())
    }
    pub(super) async fn continue_task(
        self: &Arc<Self>,
        task: String,
        owner: OwnerEpoch,
        prompt: String,
        route: SessionRoute,
    ) -> CoreResult<()> {
        let (cancel, cancelled) = watch::channel(false);
        let (done, finished) = watch::channel(false);
        self.install_pump(task.clone(), cancel, finished).await;
        let runtime = self.clone();
        let mut jobs = self.jobs.lock().await;
        jobs.retain(|j| !j.is_finished());
        jobs.push(tokio::spawn(async move {
            if let Err(error) = runtime
                .drive(&task, &owner, &prompt, route, cancelled.clone())
                .await
            {
                runtime
                    .stop_after_error(&task, &owner, error, cancelled.clone())
                    .await;
            }
            runtime.driver_ended(&task, &cancelled).await;
            done.send_replace(true);
        }));
        Ok(())
    }

    /// Relaunch an Automatic task after a runtime-owned result repair. This path
    /// deliberately bypasses the owner-input admission used by `control_input`.
    /// A scheduler repair is runtime evidence, never an owner message, permission
    /// grant, or an acknowledgement of an unresolved outcome.
    ///
    /// With an expected result version, it reopens exactly that completed result.
    /// Without one, it continues a run that ended as failed with every effect known.
    /// Every distinct failure fingerprint gets one repair (no repair cap); an owner
    /// consent on the queue hold (`owner`) is recorded the same way.
    ///
    /// `Busy` (another control, maintenance, or the service stopping) decides nothing:
    /// the caller asks again on its next pass. `Declined` and an error are refusals.
    pub(crate) async fn continue_automatic_repair(
        self: &Arc<Self>,
        project: &str,
        execution: &str,
        expected_source_version: Option<&str>,
        prompt: String,
        fingerprint: &str,
        owner: bool,
    ) -> CoreResult<RepairAdmission> {
        let Ok(_controls) = self.controls.try_lock() else {
            return Ok(RepairAdmission::Busy);
        };
        if *self.stop.borrow() {
            return Ok(RepairAdmission::Busy);
        }
        let _maintenance = match self
            .host
            .workspace_state
            .as_ref()
            .map(|state| crate::machine::maintenance::admission(state))
            .transpose()
        {
            Ok(guard) => guard,
            Err(_) => return Ok(RepairAdmission::Busy),
        };
        let project_id = project.to_owned();
        let binding_project = project_id.clone();
        let execution_id = execution.to_owned();
        let binding = self
            .host
            .executor
            .publication(move |s| {
                s.read(|db| {
                    cortex_core::workspace::control::binding(
                        db,
                        &s.key().root().to_string_lossy(),
                        &binding_project,
                        &actor(),
                        &execution_id,
                    )
                })
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        let task_id = binding.task.clone();
        let expected_event: Option<i64> = expected_source_version
            .map(|v| v.parse().map_err(|_| CoreError::Refused("stale_result")))
            .transpose()?;
        if expected_event.is_some_and(|e| e <= 0) {
            return Err(CoreError::Refused("stale_result"));
        }
        let route = self.coordinator_route(&task_id, None).await?;
        let fp = fingerprint.to_owned();
        let guidance = prompt.clone();
        let root = self.host.root.clone();
        let project_for_settled = project.to_owned();
        let draft = binding.draft.clone();
        let current_execution = execution.to_owned();
        let epoch = binding.owner.epoch;
        let runtime_task = task_id.clone();
        let reopened = self
            .host
            .executor
            .write(move |db| {
                // The caller (`task_auto_repair::dispatch`) admitted this task under the
                // machine lock as a member of a queue-driven sprint (Hybrid or Automatic).
                let already: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_REPAIR_REQUESTED' AND json_extract(payload_json,'$.fingerprint')=?)",
                    (&runtime_task, &fp),
                    |r| r.get(0),
                )?;
                if already {
                    return Ok(false);
                }
                // No repair cap (no-stop policy): each distinct failure fingerprint gets
                // its repair; only an identical one is never repeated.
                let _ = owner;
                let (state, _) = cortex_core::lifecycle::domain::task_tx(db, &runtime_task)?;
                let expected_state = if expected_event.is_some() {
                    cortex_core::TaskState::Completed
                } else {
                    cortex_core::TaskState::Failed
                };
                if state != expected_state {
                    return Ok(false);
                }
                let current_event: i64 = db.query_row(
                    "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=?",
                    [&runtime_task],
                    |r| r.get(0),
                )?;
                if expected_event.is_some_and(|e| current_event != e) {
                    return Ok(false);
                }
                crate::machine::task_lifecycle::settled(
                    db,
                    &root,
                    &project_for_settled,
                    &draft,
                )?;
                let current_binding = cortex_core::workspace::control::binding(
                    db, &root.to_string_lossy(), &project_for_settled, &actor(), &current_execution,
                )?;
                if current_binding.task != runtime_task || current_binding.owner.epoch != epoch {
                    return Ok(false);
                }
                cortex_core::lifecycle::automatic::require_settled_execution(db, &runtime_task)?;
                let waiting: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pending_questions WHERE task_id=? AND status='open')",
                    [&runtime_task], |r| r.get(0),
                )?;
                if waiting { return Ok(false); }
                let reopened = cortex_core::lifecycle::domain::reopen_tx(db, &runtime_task)?;
                if !reopened {
                    return Ok(false);
                }
                cortex_core::lifecycle::domain::settle_recovery_tx(db, &runtime_task, cortex_core::TaskState::Planning)?;
                cortex_core::lifecycle::event_tx(
                    db,
                    &runtime_task,
                    "AUTOMATIC_REPAIR_REQUESTED",
                    serde_json::json!({"fingerprint": fp, "prompt": guidance, "ownerAllowed": owner}),
                )?;
                Ok(true)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        if !reopened {
            return Ok(RepairAdmission::Declined);
        }
        self.continue_task(task_id, binding.owner, prompt, route)
            .await?;
        Ok(RepairAdmission::Started)
    }
    pub(crate) async fn shutdown(&self) {
        self.stop.send_replace(true);
        let _serial = self.controls.lock().await;
        // Jobs are taken out before they are awaited: a job that ends by starting a
        // continuation follower (which registers itself) must not wait on this lock.
        loop {
            let jobs: Vec<_> = self.jobs.lock().await.drain(..).collect();
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                let _ = job.await;
            }
        }
    }
}
impl Service {
    /// Native embedding/test entry. It accepts host authority, never wire launch fields.
    pub async fn bind_native(
        mut config: crate::Config,
        routes: Arc<dyn NativeRoutes>,
    ) -> io::Result<Arc<Self>> {
        config.launch.native_routes = Some(routes);
        Self::bind(config).await
    }
}
#[cfg(test)]
mod pump_tests {
    use super::*;
    use std::time::Duration;
    // A turn whose state read fails still signals the batch (siblings must not keep running).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_state_read_after_a_turn_still_signals_the_batch() {
        let root = std::env::temp_dir().join(format!("batch-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let (batch, sibling) = watch::channel(false);
        let stale = std::sync::atomic::AtomicBool::new(false);
        let outcome = runtime
            .settle_batch_turn("no-such-task", Ok(()), &batch, &stale)
            .await;
        assert!(outcome.is_err());
        assert!(
            *sibling.borrow(),
            "siblings must be signalled on every exit path"
        );
        // A failed turn signals the batch too.
        let (batch, sibling) = watch::channel(false);
        let _ = runtime
            .settle_batch_turn("no-such-task", Err(CoreError::Refused("x")), &batch, &stale)
            .await;
        assert!(*sibling.borrow());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    // Installing a pump for a task that already has one cancels it and waits for it to
    // finish; the new driver is never visible while the old one may still be running.
    #[tokio::test(flavor = "multi_thread")]
    async fn installing_a_pump_cancels_and_awaits_the_prior_driver() {
        let root = std::env::temp_dir().join(format!("pump-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let (cancel1, mut cancelled1) = watch::channel(false);
        let (done1, finished1) = watch::channel(false);
        runtime.install_pump("t".into(), cancel1, finished1).await;
        let (cancel2, _cancelled2) = watch::channel(false);
        let (_done2, finished2) = watch::channel(false);
        let r = runtime.clone();
        let second =
            tokio::spawn(async move { r.install_pump("t".into(), cancel2, finished2).await });
        cancelled1.changed().await.unwrap();
        assert!(*cancelled1.borrow());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !second.is_finished(),
            "replacement must wait for the prior driver"
        );
        done1.send_replace(true);
        second.await.unwrap();
        assert_eq!(runtime.pumps.lock().await.len(), 1);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    // A driver job that ended without signalling (its sender dropped, e.g. a panic)
    // no longer runs: settling the task must not fail every later owner control.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_driver_gone_without_signalling_is_settled() {
        let root = std::env::temp_dir().join(format!("pump-gone-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let (cancel, mut cancelled) = watch::channel(false);
        let (done, finished) = watch::channel(false);
        runtime.install_pump("t".into(), cancel, finished).await;
        let job = tokio::spawn(async move {
            let _ = cancelled.changed().await;
            drop(done);
        });
        runtime.settle_task("t").await.unwrap();
        job.await.unwrap();
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    // A ready step whose file another unfinished task's worker holds waits for it (one
    // recorded wait), instead of stopping the task as `native_continuation_unavailable`;
    // it claims again once the holder's task is cancelled.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_step_held_by_another_tasks_writer_waits_until_the_holding_ends() {
        let root = std::env::temp_dir().join(format!("held-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let owners = runtime.host.executor.publication(|store| {
            let machine = cortex_core::lifecycle::TaskMachine::new(store);
            let waiting = machine.create("waiting", "Reset test", "owner")?;
            machine.create("holder", "formatAll", "owner")?;
            store.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('holder',1,'formatAll','worker-backend-dev','o','[]','[\"test/counter.test.js\"]','execution','running','ha',NULL)", [])?;
                db.execute("INSERT INTO delegation_provisioning VALUES ('ha','holder',NULL,'hx','worker-backend-dev','ready','ambiguous',NULL,'hc',NULL,NULL,NULL,NULL,NULL,1,'0','0')", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('waiting',1,'reset','worker-backend-dev','o','[]','[\"test\"]','execution','pending',NULL,NULL)", [])?;
                Ok(())
            })?;
            Ok(waiting)
        }).unwrap().await.unwrap().unwrap();
        let (_cancel, cancelled) = watch::channel(false);
        let r = runtime.clone();
        let wait = tokio::spawn(async move {
            r.wait_for_write_holding("waiting", &owners, &cancelled)
                .await
        });
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!wait.is_finished(), "the held step did not wait");
        let count = |runtime: Arc<Runtime>| async move {
            runtime.host.executor.read(|db| Ok(db.query_row(
                "SELECT count(*)||':'||coalesce(max(json_extract(payload_json,'$.holder_task')),'') FROM runtime_events WHERE type='NATIVE_WRITE_CONFLICT_WAITING' AND task_id='waiting'",
                [], |r| r.get::<_, String>(0))?)).unwrap().await.unwrap().unwrap()
        };
        assert_eq!(count(runtime.clone()).await, "1:holder");
        runtime
            .host
            .executor
            .write(|db| {
                db.execute(
                    "UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id='holder'",
                    [],
                )?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .unwrap()
            .unwrap()
            .unwrap());
        assert_eq!(count(runtime.clone()).await, "1:holder");
        // Nothing held: no wait at all.
        let (_cancel, cancelled) = watch::channel(false);
        let owner = runtime
            .host
            .executor
            .read(|db| {
                Ok(db.query_row(
                    "SELECT owner_ref,epoch FROM task_owners WHERE task_id='waiting'",
                    [],
                    |r| {
                        Ok(OwnerEpoch {
                            owner: r.get(0)?,
                            epoch: r.get(1)?,
                        })
                    },
                )?)
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(!runtime
            .wait_for_write_holding("waiting", &owner, &cancelled)
            .await
            .unwrap());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    // A blocking review while the driver advances is a continuation on the strategy
    // ladder (no-stop policy): the runtime asks the owner nothing, the driver ends
    // without recording the step as one that could not continue, and the continuation
    // cites the review report. A later driver error never records a second stop.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_blocking_review_is_a_continuation_and_the_driver_ends() {
        let root = std::env::temp_dir().join(format!("blocked-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        // The review is bound to an observed project revision.
        assert!(std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let owner = runtime.host.executor.publication(|store| {
            let owner = cortex_core::lifecycle::TaskMachine::new(store).create("t", "Serve HTTPS", "owner")?;
            store.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=4,review_cycle=1", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',4,'trace','worker-security-auditor','Trace the proxy path','[]','[\"traefik/**\"]','execution','completed','a1','{}')", [])?;
                let review = json!({"verdict":"BLOCK","summary":"The request chain was not verified.","report_ref":"r_2413b9f526b9","checks":[],
                    "findings":[{"node":"trace","files":["traefik/tls.yml"],"cross_cutting":false,"fix":"Forward the visitor address"}]});
                db.execute("INSERT INTO runtime_nodes VALUES ('t',4,'_review_1','worker-code-reviewer','Review','[\"trace\"]','[\"**\"]','review','completed','rv',?)",
                    [review.to_string()])?;
                Ok(())
            })?;
            Ok(owner)
        }).unwrap().await.unwrap().unwrap();
        let (_cancel, cancelled) = watch::channel(false);
        tokio::time::timeout(
            Duration::from_secs(20),
            runtime.progress("t", &owner, cancelled),
        )
        .await
        .expect("the driver kept running after the continuation")
        .unwrap();
        let facts = runtime
            .host
            .executor
            .read(|db| {
                let stopped: i64 = db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='NATIVE_DRIVER_STOPPED'",
                    [],
                    |r| r.get(0),
                )?;
                let questions: i64 = db.query_row(
                    "SELECT count(*) FROM pending_questions WHERE task_id='t'",
                    [],
                    |r| r.get(0),
                )?;
                let (state, blocker): (String, String) = db.query_row(
                    "SELECT state,blocker FROM runtime_tasks WHERE task_id='t'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let owed = cortex_core::lifecycle::automatic::owed(db, "t")?.map(|o| o.evidence);
                Ok((stopped, questions, state, blocker, owed))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            facts.0, 0,
            "the driver recorded a stop after the continuation"
        );
        assert_eq!(facts.1, 0, "the owner was asked");
        assert_eq!(
            (facts.2.as_str(), facts.3.as_str()),
            ("RECOVERING", "automatic_continuation_pending")
        );
        let evidence = facts.4.expect("a continuation is owed");
        assert_eq!(evidence["reason"], "review_blocked");
        assert_eq!(evidence["strategy"], "repair");
        assert!(evidence["reports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reportId"] == "r_2413b9f526b9"));
        // A driver error arriving now records no second continuation.
        runtime
            .stop_after_error(
                "t",
                &owner,
                CoreError::Refused("native_continuation_unavailable"),
                watch::channel(false).1,
            )
            .await;
        let count = runtime
            .host
            .executor
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='AUTOMATIC_CONTINUATION'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count, 1);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[cfg(test)]
mod discover_probe_tests {
    use super::*;
    // A writing worker's catalogue discovery without arguments (as Codex workers call
    // it) answers with the catalogue's first page, never an authority refusal.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_worker_discovers_its_tool_catalogue_without_arguments() {
        let root = std::env::temp_dir().join(format!("discover-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let outcome = runtime.host.executor.publication(|store| {
            let owner = cortex_core::lifecycle::TaskMachine::new(store).create("t", "g", "owner")?;
            let c = store.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-devops-engineer','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
                let c = cortex_core::channels::issue_tx(db, "t", &owner, cortex_core::channels::Principal::Worker { assignment: "a" })?;
                cortex_core::channels::bind_session_tx(db, &c, "engine")?;
                Ok(c)
            })?;
            let i = store.read(|db| cortex_core::channels::authenticate(db, &c.channel_id, c.secret(), true))?;
            let mut out = Vec::new();
            for args in [json!({}), json!({"limit":20}), json!({"limit":50})] {
                out.push(crate::native_documents::execute(store, &c, &i, "tools_discover", &args).map(|v| v["tools"].as_array().map(Vec::len)).map_err(|e| e.code()));
            }
            Ok(out)
        }).unwrap().await.unwrap().unwrap();
        for o in &outcome {
            assert!(matches!(o, Ok(Some(n)) if *n > 0), "{outcome:?}");
        }
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
