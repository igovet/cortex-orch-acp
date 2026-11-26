//! The owner moments of paired machines as this phone's own notifications.
//!
//! Each paired machine sends its whole set of open owner moments (`notifications.subscribe`)
//! when the phone subscribes and again after every change: a kind, an opaque fingerprint of
//! the moment's state, the task or sprint title and the closed target the desktop opens. No
//! push service is involved; the phone raises the notification itself.
//!
//! Nothing a machine sends is trusted as is: a moment of an unknown kind, a malformed
//! fingerprint or a target outside the closed shapes is dropped, and a title is bounded.
//! A fingerprint already announced is never announced again, so a reconnect, which sends
//! the same set again, raises nothing. The fingerprints announced are kept on the phone, so
//! an app restart does not either. The first set a machine sends only records what is open.
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use workspace_client::policy;

/// The fingerprints remembered per machine.
pub const SEEN_LIMIT: usize = 500;
/// The moments one snapshot may carry; the rest are ignored.
pub const MOMENTS_LIMIT: usize = 200;
const TITLE: usize = 120;

/// One validated owner moment of one machine.
#[derive(Clone, Debug, PartialEq)]
pub struct Moment {
    pub kind: &'static str,
    pub fingerprint: String,
    pub title: String,
    pub target: Value,
}

const KINDS: [&str; 8] = [
    "question", "stop", "access", "planning", "result", "failed", "paused", "stalled",
];

/// The Android notification channel of a kind: a question that needs an answer, a finished
/// task's result, or a failure.
pub fn channel(kind: &str) -> &'static str {
    match kind {
        "question" | "stop" | "access" | "planning" => "answer",
        "result" => "done",
        _ => "failure",
    }
}

fn summary(kind: &str) -> &'static str {
    match kind {
        "question" => "Question waiting",
        "stop" => "Task stopped: your decision",
        "access" => "Task needs access",
        "planning" => "Planning question waiting",
        "result" => "Result ready for review",
        "failed" => "Task failed",
        "paused" => "Sprint paused",
        _ => "No progress",
    }
}

fn fingerprint_ok(text: &str) -> bool {
    (8..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The moments of one `notification_moments` event, or `None` when it is not one.
pub fn parse(event: &Value) -> Option<Vec<Moment>> {
    if event["type"] != "notification_moments" {
        return None;
    }
    let list = event["moments"].as_array()?;
    Some(
        list.iter()
            .take(MOMENTS_LIMIT)
            .filter_map(|m| {
                let kind = KINDS.into_iter().find(|k| m["kind"] == *k)?;
                let fingerprint = m["fingerprint"].as_str().filter(|f| fingerprint_ok(f))?;
                let target = &m["target"];
                if !policy::notification_target(target) {
                    return None;
                }
                let title: String = m["title"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(TITLE)
                    .collect();
                let title = match title.trim() {
                    "" => "A task".to_owned(),
                    t => t.to_owned(),
                };
                Some(Moment {
                    kind,
                    fingerprint: fingerprint.to_ascii_lowercase(),
                    title,
                    target: target.clone(),
                })
            })
            .collect(),
    )
}

/// The words the interface hands over in its own language: the title of each kind of moment
/// (kept here) and the ongoing notification and channel names (`android`, for the plugin).
/// Each is bounded, without control characters; a missing or invalid one keeps the English.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Words {
    pub titles: HashMap<String, String>,
    pub android: Value,
}
const WORD: usize = 200;
fn word(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty() && text.chars().count() <= WORD && !text.chars().any(char::is_control))
        .then(|| text.to_owned())
}
pub fn words(value: &Value) -> Words {
    let titles = KINDS
        .into_iter()
        .filter_map(|kind| Some((kind.to_owned(), word(&value["kinds"][kind])?)))
        .collect();
    let mut android = serde_json::Map::new();
    for key in ["ongoingTitle", "ongoingText"] {
        if let Some(text) = word(&value[key]) {
            android.insert(key.into(), text.into());
        }
    }
    let mut channels = serde_json::Map::new();
    for channel in ["ongoing", "answer", "done", "failure"] {
        let given = &value["channels"][channel];
        let mut out = serde_json::Map::new();
        for key in ["name", "note"] {
            if let Some(text) = word(&given[key]) {
                out.insert(key.into(), text.into());
            }
        }
        if !out.is_empty() {
            channels.insert(channel.into(), out.into());
        }
    }
    android.insert("channels".into(), channels.into());
    Words {
        titles,
        android: android.into(),
    }
}

/// What the phone's notification side does next.
#[derive(Clone, Debug, PartialEq)]
pub enum Delivery {
    /// Raise (or replace) the notification `tag`; a tap opens `target` on `machine_id`.
    Show {
        tag: String,
        channel: &'static str,
        title: String,
        text: String,
        machine_id: String,
        target: Value,
    },
    /// The moment is gone (answered, accepted, closed) or its machine was forgotten.
    Withdraw { tag: String },
}

/// One notification per task or sprint of one machine: a newer moment of it replaces it.
fn tag(machine_id: &str, target: &Value) -> String {
    let (kind, id) = match target["task"].as_str() {
        Some(task) => ("t", task),
        None => ("s", target["sprint"].as_str().unwrap_or_default()),
    };
    format!(
        "{machine_id}|{}|{kind}|{id}",
        target["project"].as_str().unwrap_or_default()
    )
}

#[derive(Default)]
struct Seen {
    order: VecDeque<String>,
    set: HashSet<String>,
    /// This phone's notifications still out, by tag, with the fingerprint each shows.
    shown: BTreeMap<String, String>,
}
impl Seen {
    fn see(&mut self, fingerprint: &str) {
        if self.set.insert(fingerprint.to_owned()) {
            self.order.push_back(fingerprint.to_owned());
            while self.order.len() > SEEN_LIMIT {
                if let Some(old) = self.order.pop_front() {
                    self.set.remove(&old);
                }
            }
        }
    }
}

/// The fingerprints announced per machine, and the notifications out.
#[derive(Default)]
pub struct Book {
    machines: HashMap<String, Seen>,
    /// The title of each kind in the interface's language; English where none is given.
    titles: HashMap<String, String>,
}
impl Book {
    /// The titles the interface gave (see [`words`]).
    pub fn set_titles(&mut self, titles: HashMap<String, String>) {
        self.titles = titles;
    }
    /// The book saved by [`Book::saved`]; anything unreadable is an empty book.
    pub fn from_saved(saved: &Value) -> Self {
        let mut book = Self::default();
        for (machine, list) in saved.as_object().into_iter().flatten() {
            let seen = book.machines.entry(machine.clone()).or_default();
            for fingerprint in list
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if fingerprint_ok(fingerprint) {
                    seen.see(fingerprint);
                }
            }
        }
        book
    }
    pub fn saved(&self) -> Value {
        Value::Object(
            self.machines
                .iter()
                .map(|(machine, seen)| (machine.clone(), json!(seen.order)))
                .collect(),
        )
    }

    /// One complete set of a machine's moments.
    pub fn apply(
        &mut self,
        machine_id: &str,
        machine_name: &str,
        moments: Vec<Moment>,
    ) -> Vec<Delivery> {
        let prime = !self.machines.contains_key(machine_id);
        let seen = self.machines.entry(machine_id.to_owned()).or_default();
        let current: BTreeMap<String, Moment> = moments
            .into_iter()
            .map(|m| (tag(machine_id, &m.target), m))
            .collect();
        let mut out = Vec::new();
        let gone: Vec<String> = seen
            .shown
            .keys()
            .filter(|t| !current.contains_key(*t))
            .cloned()
            .collect();
        for tag in gone {
            seen.shown.remove(&tag);
            out.push(Delivery::Withdraw { tag });
        }
        for (tag, moment) in current {
            if seen.set.contains(&moment.fingerprint) {
                // Announced before. A notification for an older state of it gives way.
                if seen
                    .shown
                    .get(&tag)
                    .is_some_and(|f| *f != moment.fingerprint)
                {
                    seen.shown.remove(&tag);
                    out.push(Delivery::Withdraw { tag });
                }
                continue;
            }
            seen.see(&moment.fingerprint);
            if prime {
                continue;
            }
            seen.shown.insert(tag.clone(), moment.fingerprint.clone());
            out.push(Delivery::Show {
                tag,
                channel: channel(moment.kind),
                title: self
                    .titles
                    .get(moment.kind)
                    .cloned()
                    .unwrap_or_else(|| summary(moment.kind).to_owned()),
                text: if machine_name.is_empty() {
                    moment.title
                } else {
                    format!("{} · {machine_name}", moment.title)
                },
                machine_id: machine_id.to_owned(),
                target: moment.target,
            });
        }
        out
    }

    /// A forgotten machine: its notifications are withdrawn and its record dropped.
    pub fn forget(&mut self, machine_id: &str) -> Vec<Delivery> {
        self.machines
            .remove(machine_id)
            .map(|seen| {
                seen.shown
                    .into_keys()
                    .map(|tag| Delivery::Withdraw { tag })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moment(kind: &str, fingerprint: &str, task: &str) -> Value {
        json!({"kind":kind,"fingerprint":fingerprint,"title":format!("Task {task}"),
            "target":{"kind":"result","project":"p","task":task}})
    }
    fn snapshot(moments: Vec<Value>) -> Vec<Moment> {
        parse(&json!({"type":"notification_moments","moments":moments})).unwrap()
    }
    fn shown(out: &[Delivery]) -> Vec<&str> {
        out.iter()
            .filter_map(|d| match d {
                Delivery::Show { tag, .. } => Some(tag.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn only_valid_moments_pass_and_titles_are_bounded() {
        let long = format!("A\u{7}{}", "b".repeat(300));
        let list = snapshot(vec![
            json!({"kind":"question","fingerprint":"00aa11bb22cc33dd","title":long,
                "target":{"kind":"question","project":"p","task":"t","decision":"d"}}),
            json!({"kind":"question","fingerprint":"00aa11bb22cc33dd","title":"",
                "target":{"kind":"question","project":"p","task":"t"}}),
            json!({"kind":"open_url","fingerprint":"00aa11bb22cc33dd","title":"x",
                "target":{"kind":"task","project":"p","task":"t"}}),
            json!({"kind":"failed","fingerprint":"not hex!","title":"x",
                "target":{"kind":"task","project":"p","task":"t"}}),
            json!({"kind":"failed","fingerprint":"00aa11bb22cc33dd","title":"x",
                "target":{"kind":"task","project":"p","task":"t","url":"https://x"}}),
            json!({"kind":"paused","fingerprint":"00AA11BB22CC33DE","title":"  ",
                "target":{"kind":"sprint","project":"p","sprint":"s"}}),
        ]);
        assert_eq!(list.len(), 2, "{list:?}");
        assert_eq!(list[0].title.chars().count(), TITLE);
        assert!(!list[0].title.contains('\u{7}'));
        assert_eq!(list[1].title, "A task");
        assert_eq!(list[1].fingerprint, "00aa11bb22cc33de");
        assert!(parse(&json!({"type":"native_delta"})).is_none());
        assert!(parse(&json!({"type":"notification_moments","moments":{}})).is_none());
        assert_eq!(channel("question"), "answer");
        assert_eq!(channel("access"), "answer");
        assert_eq!(channel("result"), "done");
        assert_eq!(channel("failed"), "failure");
        assert_eq!(channel("stalled"), "failure");
    }

    #[test]
    fn the_first_set_is_recorded_and_later_moments_raise_once() {
        let mut book = Book::default();
        let open = moment("result", "0000000000000001", "a");
        // What was open before the phone first subscribed is not announced.
        assert!(book
            .apply("m", "Desk", snapshot(vec![open.clone()]))
            .is_empty());
        let fresh = moment("failed", "0000000000000002", "b");
        let out = book.apply("m", "Desk", snapshot(vec![open.clone(), fresh.clone()]));
        assert_eq!(
            out,
            vec![Delivery::Show {
                tag: "m|p|t|b".into(),
                channel: "failure",
                title: "Task failed".into(),
                text: "Task b · Desk".into(),
                machine_id: "m".into(),
                target: json!({"kind":"result","project":"p","task":"b"}),
            }]
        );
        // A reconnect sends the same set again: nothing is raised twice.
        assert!(book
            .apply("m", "Desk", snapshot(vec![open.clone(), fresh.clone()]))
            .is_empty());
        // Nor after an app restart, from the saved book.
        let mut restarted = Book::from_saved(&book.saved());
        assert!(restarted
            .apply("m", "Desk", snapshot(vec![open, fresh]))
            .is_empty());
    }

    #[test]
    fn a_newer_state_replaces_and_a_gone_moment_withdraws() {
        let mut book = Book::default();
        book.apply("m", "", snapshot(vec![]));
        let first = book.apply(
            "m",
            "",
            snapshot(vec![moment("question", "0000000000000003", "a")]),
        );
        assert_eq!(shown(&first), ["m|p|t|a"]);
        // The same task's next moment replaces its notification (same tag).
        let next = book.apply(
            "m",
            "",
            snapshot(vec![moment("result", "0000000000000004", "a")]),
        );
        assert_eq!(shown(&next), ["m|p|t|a"]);
        // Answered or accepted: it goes.
        assert_eq!(
            book.apply("m", "", snapshot(vec![])),
            vec![Delivery::Withdraw {
                tag: "m|p|t|a".into()
            }]
        );
        // A machine is primed on its own: another machine's first set announces nothing.
        assert!(book
            .apply(
                "n",
                "",
                snapshot(vec![moment("failed", "0000000000000005", "z")])
            )
            .is_empty());
    }

    #[test]
    fn forgetting_a_machine_withdraws_its_notifications() {
        let mut book = Book::default();
        book.apply("m", "", snapshot(vec![]));
        book.apply(
            "m",
            "",
            snapshot(vec![moment("failed", "0000000000000006", "a")]),
        );
        assert_eq!(
            book.forget("m"),
            vec![Delivery::Withdraw {
                tag: "m|p|t|a".into()
            }]
        );
        assert!(book.forget("m").is_empty());
        assert!(book.saved().get("m").is_none());
    }

    #[test]
    fn notifications_speak_the_interface_language_and_keep_english_otherwise() {
        let given = words(&json!({
            "ongoingTitle": "На связи с компьютерами", "ongoingText": "\u{7}bad",
            "channels": {"answer": {"name": "Нужен ваш ответ", "note": "x".repeat(201)}, "other": {"name": "x"}},
            "kinds": {"failed": "Сбой задачи", "result": "", "open_url": "x"},
        }));
        assert_eq!(
            given.titles,
            HashMap::from([("failed".to_owned(), "Сбой задачи".to_owned())])
        );
        assert_eq!(
            given.android,
            json!({"ongoingTitle":"На связи с компьютерами","channels":{"answer":{"name":"Нужен ваш ответ"}}})
        );
        assert_eq!(words(&Value::Null).android, json!({"channels":{}}));
        let mut book = Book::default();
        book.set_titles(given.titles);
        book.apply("m", "", snapshot(vec![]));
        let out = book.apply(
            "m",
            "",
            snapshot(vec![
                moment("failed", "0000000000000007", "a"),
                moment("result", "0000000000000008", "b"),
            ]),
        );
        let titles: Vec<&str> = out
            .iter()
            .filter_map(|d| match d {
                Delivery::Show { title, .. } => Some(title.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(titles, ["Сбой задачи", "Result ready for review"]);
    }

    #[test]
    fn the_remembered_fingerprints_are_bounded() {
        let mut book = Book::default();
        book.apply("m", "", snapshot(vec![]));
        for n in 0..(SEEN_LIMIT + 10) {
            book.apply(
                "m",
                "",
                snapshot(vec![moment("failed", &format!("{n:016x}"), "a")]),
            );
        }
        assert_eq!(book.saved()["m"].as_array().unwrap().len(), SEEN_LIMIT);
    }
}
