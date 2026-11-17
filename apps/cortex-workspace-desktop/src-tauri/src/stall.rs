//! Evidence for a window that stops answering. The main thread is probed once a second; when a
//! probe waits longer than `STALL_AFTER`, one line names how long it waited and which requests and
//! service events the window handled just before and during the wait. Names only, never payloads.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

pub const PROBE_EVERY: Duration = Duration::from_secs(1);
pub const STALL_AFTER: Duration = Duration::from_secs(2);
/// Distinct names kept per window; anything beyond is counted under one key.
const NAMES: usize = 32;

#[derive(Default)]
pub struct Traffic {
    requests: Mutex<BTreeMap<String, u64>>,
    events: AtomicU64,
}

impl Traffic {
    /// One request from the page. A helper call is named by its operation, which policy has already bounded.
    pub fn request(&self, operation: &str, message: &Value) {
        let name = match message["op"].as_str() {
            Some(op) if operation == "machine.call" && plain(op) => format!("{operation}:{op}"),
            _ if plain(operation) => operation.to_owned(),
            _ => "other".to_owned(),
        };
        if let Ok(mut requests) = self.requests.lock() {
            let key = if requests.len() < NAMES || requests.contains_key(&name) {
                name
            } else {
                "other".to_owned()
            };
            *requests.entry(key).or_default() += 1;
        }
    }
    /// One service event passed to the page.
    pub fn event(&self) {
        self.events.fetch_add(1, Ordering::Relaxed);
    }
    /// What was counted since the last take; counting then begins again from zero.
    pub fn take(&self) -> Value {
        let requests = self
            .requests
            .lock()
            .map(|mut r| std::mem::take(&mut *r))
            .unwrap_or_default();
        json!({"requests": requests, "events": self.events.swap(0, Ordering::Relaxed)})
    }
}

fn plain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// One diagnostic line: `main_thread_stall` while still waiting, `main_thread_recovered` once the probe ran.
pub fn line(event: &str, waited: Duration, traffic: Value) -> String {
    json!({"event": event, "waitedMs": waited.as_millis() as u64, "traffic": traffic}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_names_only_and_resets_after_each_take() {
        let traffic = Traffic::default();
        traffic.request(
            "machine.call",
            &json!({"op":"repo.file","params":{"file":"secret.txt"}}),
        );
        traffic.request("machine.call", &json!({"op":"repo.file"}));
        traffic.request("project.snapshot", &json!({"projectId":"p"}));
        traffic.request("machine.call", &json!({"op":"a b"}));
        traffic.event();
        let taken = traffic.take();
        assert_eq!(
            taken,
            json!({"requests":{"machine.call:repo.file":2,"machine.call":1,"project.snapshot":1},"events":1})
        );
        assert!(!taken.to_string().contains("secret"));
        assert_eq!(traffic.take(), json!({"requests":{},"events":0}));
    }

    #[test]
    fn a_flood_of_names_stays_bounded() {
        let traffic = Traffic::default();
        for i in 0..100 {
            traffic.request(&format!("op{i}"), &json!({}));
        }
        let taken = traffic.take();
        let requests = taken["requests"].as_object().unwrap();
        assert_eq!(requests.len(), NAMES + 1);
        assert_eq!(requests["other"], 100 - NAMES as u64);
    }

    #[test]
    fn a_stall_line_says_how_long_and_what_ran() {
        let traffic = Traffic::default();
        traffic.request("service.status", &json!({}));
        let line = line(
            "main_thread_stall",
            Duration::from_millis(2500),
            traffic.take(),
        );
        let row: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(row["event"], "main_thread_stall");
        assert_eq!(row["waitedMs"], 2500);
        assert_eq!(row["traffic"]["requests"]["service.status"], 1);
    }
}
