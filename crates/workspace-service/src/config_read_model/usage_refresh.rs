//! Free, model-less plan-window reads that keep the usage cache current.
//!
//! - Claude: the Claude Code CLI's own local `/usage` command. Its JSON is trusted
//!   only when it proves no model ran (`local_command: "usage"`, `num_turns: 0`,
//!   `total_cost_usd: 0`, zero tokens). Only exact `Current week (...)` lines with
//!   a percentage and a UTC reset become windows: `all models` is `seven_day`,
//!   `Fable` is `seven_day_fable`.
//! - Codex: the Codex app server's `account/rateLimits/read` account request,
//!   which starts no thread or turn. Its `primary`/`secondary` windows keep the
//!   rollout names and fields.
//!
//! Anything else leaves every window as it was; an unparseable window reports
//! nothing, never a guess or zero. [`refresh`] runs a read at most once per
//! connection per [`REFRESH_AFTER_SECONDS`] (failures included) under a
//! nonblocking per-connection lock, and writes the private
//! `<state>/cortex/quota/<id>.json` cache that display and routing read.
use crate::native_routes::read;
use cortex_core::{
    store::locks::{open_private, private_directory, AccessLock},
    CoreError, CoreResult,
};
use serde_json::{json, Map, Value};
use std::{io::Write, path::Path, time::Duration};

/// A connection's plan windows are read again only after this many seconds.
pub const REFRESH_AFTER_SECONDS: f64 = 300.0;
/// Largest accepted output of one read.
pub const MAX_OUTPUT: usize = 1024 * 1024;
const DAY: f64 = 86_400.0;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
/// Windows of one trusted read, by cache name (`{"used","resets_at"[,"window_minutes"]}`).
pub type Windows = Map<String, Value>;

/// What one [`refresh`] call did.
#[derive(Debug, PartialEq, Eq)]
pub enum Refresh {
    /// Checked less than [`REFRESH_AFTER_SECONDS`] ago; nothing was run.
    Fresh,
    /// Another refresh of this connection holds the lock; nothing was run.
    Busy,
    /// The read was run (or could not be); `ok` says whether its output was trusted.
    Checked { ok: bool },
}

fn zero(v: &Value) -> bool {
    v.as_f64() == Some(0.0)
}

/// The windows of one Claude CLI JSON result, or `None` when the result is not a
/// proven free local `/usage` run or its windows are ambiguous.
pub fn parse_claude(stdout: &[u8], now: f64) -> Option<Windows> {
    if stdout.len() > MAX_OUTPUT || !now.is_finite() {
        return None;
    }
    let v: Value = serde_json::from_slice(stdout).ok()?;
    let usage = v["usage"].as_object()?;
    let free = v["type"] == "result"
        && v["subtype"] == "success"
        && v["is_error"] == false
        && v["local_command"] == "usage"
        && v["num_turns"].as_u64() == Some(0)
        && zero(&v["total_cost_usd"])
        && [
            "input_tokens",
            "output_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
        ]
        .iter()
        .all(|k| usage.get(*k).is_some_and(zero))
        && v.get("duration_api_ms").is_none_or(zero)
        && v.get("modelUsage")
            .is_none_or(|m| m.as_object().is_some_and(Map::is_empty));
    if !free {
        return None;
    }
    parse_claude_text(v["result"].as_str()?, now)
}

/// Window name of one `Current week (<scope>)` label: exact `all models`, or a
/// plain model name such as `Fable`.
fn window_name(scope: &str) -> Option<String> {
    if scope == "all models" {
        return Some("seven_day".into());
    }
    let words: Vec<_> = scope.split(' ').collect();
    let plain = scope.len() <= 32
        && scope.starts_with(|c: char| c.is_ascii_uppercase())
        && words
            .iter()
            .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric()));
    plain.then(|| format!("seven_day_{}", words.join("_").to_ascii_lowercase()))
}

/// Exact weekly lines of the `/usage` text. Other lines (the session window,
/// tips) are ignored. `None` when a window is reported twice.
pub fn parse_claude_text(text: &str, now: f64) -> Option<Windows> {
    let line = regex::Regex::new(concat!(
        r"^Current week \(([^()]{1,32})\): (\d{1,3}(?:\.\d{1,2})?)% used · ",
        r"resets ([A-Z][a-z]{2}) (\d{1,2}), (\d{1,2})(?::(\d{2}))?(am|pm) \(UTC\)$"
    ))
    .unwrap();
    let mut windows = Map::new();
    for raw in text.lines() {
        let Some(c) = line.captures(raw.trim()) else {
            continue;
        };
        let Some(name) = window_name(&c[1]) else {
            continue;
        };
        if windows.contains_key(&name) {
            return None;
        }
        let used: f64 = c[2].parse().ok()?;
        let minute = c.get(6).map_or(Some(0), |m| m.as_str().parse().ok())?;
        let reset = reset_time(
            &c[3],
            c[4].parse().ok()?,
            c[5].parse().ok()?,
            minute,
            &c[7],
            now,
        );
        if let (true, Some(reset)) = ((0.0..=100.0).contains(&used), reset) {
            windows.insert(name, json!({"used":used,"resets_at":reset}));
        }
    }
    Some(windows)
}

/// The windows of one Codex app-server `account/rateLimits/read` response, or
/// `None` for an error or any shape other than the documented one.
pub fn parse_codex(response: &Value, now: f64) -> Option<Windows> {
    // The app server answers `{"id","result"}`; a `jsonrpc` member, when present, must be 2.0.
    if response["id"] != 2
        || response.get("jsonrpc").is_some_and(|v| v != "2.0")
        || response.get("error").is_some()
        || !now.is_finite()
    {
        return None;
    }
    let limits = response["result"]["rateLimits"].as_object()?;
    if !limits
        .get("limitId")
        .is_none_or(|id| id.is_null() || id == "codex")
    {
        return None;
    }
    let mut windows = Map::new();
    for name in ["primary", "secondary"] {
        let w = match limits.get(name) {
            None | Some(Value::Null) => continue,
            Some(w) => w,
        };
        let used = w["usedPercent"].as_u64().filter(|n| *n <= 100)?;
        let minutes = match &w["windowDurationMins"] {
            Value::Null => Value::Null,
            m => json!(m.as_u64().filter(|n| *n > 0)?),
        };
        let reset = match &w["resetsAt"] {
            Value::Null => None,
            r => Some(r.as_i64()? as f64),
        };
        // A window already over is not reported; its replacement is unknown.
        if reset.is_some_and(|r| r <= now) {
            continue;
        }
        windows.insert(
            name.into(),
            json!({"used":used as f64,"resets_at":reset,"window_minutes":minutes}),
        );
    }
    Some(windows)
}

fn epoch(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> Option<f64> {
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    t.tm_year = year - 1900;
    t.tm_mon = month;
    t.tm_mday = day;
    t.tm_hour = hour;
    t.tm_min = minute;
    let seconds = unsafe { libc::timegm(&mut t) };
    // timegm normalizes; a changed field means the date does not exist (Feb 30).
    (t.tm_mday == day && t.tm_mon == month).then_some(seconds as f64)
}

/// The next instant matching a printed UTC `Mon D, H[:MM]am` reset, which must be
/// in the future and within one weekly window plus a day; otherwise unknown.
fn reset_time(month: &str, day: i32, hour: i32, minute: i32, half: &str, now: f64) -> Option<f64> {
    let month = MONTHS.iter().position(|m| *m == month)? as i32;
    if !(1..=12).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }
    let hour = hour % 12 + if half == "pm" { 12 } else { 0 };
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    let seconds = now.floor() as libc::time_t;
    if unsafe { libc::gmtime_r(&seconds, &mut t) }.is_null() {
        return None;
    }
    let year = t.tm_year + 1900;
    let mut reset = epoch(year, month, day, hour, minute)?;
    if reset < now - DAY {
        reset = epoch(year + 1, month, day, hour, minute)?;
    }
    (reset > now && reset <= now + 8.0 * DAY).then_some(reset)
}

/// The cache after one trusted snapshot: every read window is replaced unless a
/// newer observation exists, and a window an earlier read wrote but this one no
/// longer reports is removed. Windows of other sources stay as they are.
pub fn merge(cache: &Value, parsed: &Windows, now: f64) -> Value {
    let mut windows = cache.as_object().cloned().unwrap_or_default();
    windows.retain(|name, w| parsed.contains_key(name) || w["source"] != "cli");
    for (name, w) in parsed {
        if windows
            .get(name)
            .is_some_and(|old| old["observed_at"].as_f64().is_some_and(|t| t > now))
        {
            continue;
        }
        let mut item = w.clone();
        item["observed_at"] = json!(now);
        item["source"] = json!("cli");
        windows.insert(name.clone(), item);
    }
    Value::Object(windows)
}

fn write_private(path: &Path, value: &Value) -> CoreResult<()> {
    let temporary = path.with_extension("json.tmp");
    let mut file = open_private(&temporary, true, false)?;
    file.set_len(0)?;
    file.write_all(&serde_json::to_vec(value).map_err(|_| CoreError::Refused("invalid_cache"))?)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Whether `id` was checked less than [`REFRESH_AFTER_SECONDS`] before `now`.
/// Read-only; a missing or unreadable record is not recent.
pub fn checked_recently(state: &Path, id: &str, now: f64) -> bool {
    read(&probe_path(state, id), true)
        .ok()
        .flatten()
        .and_then(|p| p["checked_at"].as_f64())
        .is_some_and(|at| (0.0..REFRESH_AFTER_SECONDS).contains(&(now - at)))
}
fn probe_path(state: &Path, id: &str) -> std::path::PathBuf {
    state.join(format!("cortex/quota/{id}.cli-probe.json"))
}

/// Refresh one connection's cached windows from `read_windows` (the trusted
/// windows of one read, or `None` when it could not run or was not trusted),
/// unless checked recently or already running. A cache that cannot be read
/// safely is never overwritten.
pub fn refresh(
    state: &Path,
    id: &str,
    now: f64,
    read_windows: impl FnOnce() -> Option<Windows>,
) -> CoreResult<Refresh> {
    if !super::matches(r"^cn_[a-f0-9]{12}$", id) || !now.is_finite() {
        return Err(CoreError::Refused("invalid_connection_id"));
    }
    let root = state.join("cortex");
    let quota = root.join("quota");
    for directory in [&root, &quota] {
        private_directory(directory, true)?;
    }
    let lock = AccessLock::acquire(
        &quota.join(format!("{id}.cli-probe.lock")),
        true,
        true,
        Duration::ZERO,
    );
    let _held = match lock {
        Ok(held) => held,
        Err(CoreError::Refused("busy")) => return Ok(Refresh::Busy),
        Err(e) => return Err(e),
    };
    if checked_recently(state, id, now) {
        return Ok(Refresh::Fresh);
    }
    let path = quota.join(format!("{id}.json"));
    let store = |parsed: &Windows| -> CoreResult<()> {
        let cache = read(&path, true)?.unwrap_or(json!({}));
        if !cache.is_object() {
            return Err(CoreError::Refused("invalid_cache"));
        }
        write_private(&path, &merge(&cache, parsed, now))
    };
    // A refused cache counts as a failed check, so it is not retried on every read.
    let ok = read_windows().is_some_and(|parsed| store(&parsed).is_ok());
    write_private(&probe_path(state, id), &json!({"checked_at":now,"ok":ok}))?;
    Ok(Refresh::Checked { ok })
}

/// Codex windows for display: the rollout's, except where this cache holds a
/// newer read of the same account (`source: "cli"` with an observation time).
pub fn newest_codex(rollout: &Value, cache: &Value) -> Value {
    let mut windows = rollout.as_object().cloned().unwrap_or_default();
    for (name, w) in cache.as_object().into_iter().flatten() {
        let Some(at) = w["observed_at"].as_f64().filter(|_| w["source"] == "cli") else {
            continue;
        };
        if windows
            .get(name)
            .and_then(|r| r["observed_at"].as_f64())
            .is_none_or(|old| old < at)
        {
            windows.insert(name.clone(), w.clone());
        }
    }
    Value::Object(windows)
}
