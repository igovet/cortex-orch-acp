//! Bounded read-only Codex rollout observations. Absence is unknown, not zero.
use serde_json::{json, Value};
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
fn children(path: &Path) -> Vec<PathBuf> {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
        return Vec::new();
    }
    std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .take(4096)
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink()))
        .map(|e| e.path())
        .collect()
}
pub(super) fn codex(home: &Path) -> Value {
    let mut days = Vec::new();
    for year in children(&home.join("sessions")) {
        for month in children(&year) {
            days.extend(children(&month));
        }
    }
    days.sort();
    days.reverse();
    days.truncate(2);
    let mut files = Vec::new();
    for day in days {
        files.extend(
            children(&day)
                .into_iter()
                .filter(|p| p.extension().is_some_and(|e| e == "jsonl")),
        );
    }
    files.sort_by_key(|p| std::cmp::Reverse(p.metadata().and_then(|m| m.modified()).ok()));
    files.truncate(10);
    for path in files {
        let Ok(mut file) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
        else {
            continue;
        };
        let Ok(m) = file.metadata() else {
            continue;
        };
        if !m.is_file() {
            continue;
        }
        if file
            .seek(SeekFrom::Start(m.len().saturating_sub(256 * 1024)))
            .is_err()
        {
            continue;
        }
        let mut bytes = Vec::new();
        if file.take(256 * 1024).read_to_end(&mut bytes).is_err() {
            continue;
        }
        for line in String::from_utf8_lossy(&bytes).lines().rev() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let limits = &v["payload"]["rate_limits"];
            if limits.as_object().is_none_or(|m| m.is_empty()) {
                continue;
            }
            let mut windows = json!({});
            for name in ["primary", "secondary"] {
                let w = &limits[name];
                if w["used_percent"].is_number() {
                    windows[name] = json!({"used":w["used_percent"],"resets_at":w["resets_at"],"window_minutes":w["window_minutes"]});
                    if let Some(at) = v["timestamp"].as_str().and_then(timestamp) {
                        windows[name]["observed_at"] = json!(at);
                    }
                }
            }
            return windows;
        }
    }
    json!({})
}
/// Epoch seconds of an exact UTC `YYYY-MM-DDTHH:MM:SS[.fff]Z` rollout timestamp.
pub(crate) fn timestamp(text: &str) -> Option<f64> {
    let c = regex::Regex::new(r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(\.\d{1,9})?Z$")
        .unwrap()
        .captures(text)?;
    let field = |i: usize| c[i].parse::<i32>().ok();
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    t.tm_year = field(1)? - 1900;
    t.tm_mon = field(2)? - 1;
    t.tm_mday = field(3)?;
    t.tm_hour = field(4)?;
    t.tm_min = field(5)?;
    t.tm_sec = field(6)?;
    let (month, day) = (t.tm_mon, t.tm_mday);
    if !(0..24).contains(&t.tm_hour) || !(0..60).contains(&t.tm_min) || !(0..60).contains(&t.tm_sec)
    {
        return None;
    }
    let seconds = unsafe { libc::timegm(&mut t) } as f64;
    let fraction = c
        .get(7)
        .map_or(Some(0.0), |f| f.as_str().parse::<f64>().ok())?;
    (t.tm_mon == month && t.tm_mday == day).then_some(seconds + fraction)
}
#[cfg(test)]
mod tests {
    #[test]
    fn rollout_timestamps_are_exact_utc_or_unknown() {
        assert_eq!(
            super::timestamp("2026-11-16T22:54:43.056Z"),
            Some(1_794_869_683.056)
        );
        assert_eq!(
            super::timestamp("2026-11-16T22:54:43Z"),
            Some(1_794_869_683.0)
        );
        for bad in [
            "2026-11-16 22:54:43Z",
            "2026-02-30T00:00:00Z",
            "2026-11-16T24:00:00Z",
            "2026-11-16T22:54:43+02:00",
            "",
        ] {
            assert_eq!(super::timestamp(bad), None, "{bad}");
        }
    }
}
