//! Where a desktop notification leads, and which of this window's notifications are still out.
//!
//! The web view names a notification's target when it asks for one. The shell keeps that target,
//! keyed by the id the notification server returned, and hands it back to the same window only when
//! the server reports the default action on that exact id. A click never carries data of its own: an
//! id this window did not send, a second click on the same id, or any other action leads nowhere.
//! The web view still resolves the target against what it knows, so a stale one opens the nearest
//! valid place.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;

/// The notifications remembered at once. An older one is long read or long replaced.
pub const MAX_OUTSTANDING: usize = 64;
/// Longest identifier accepted in a target.
pub const MAX_ID: usize = 256;
/// Longest activation token kept for the next raise.
pub const MAX_TOKEN: usize = 512;
/// Longest label shown in a notification.
pub const MAX_LABEL: usize = 1000;

/// The one message a notification click sends to the window: the place to open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum NotificationTarget {
    /// A question waiting in a task: its Conversation, at that question.
    Question {
        project: String,
        task: String,
        decision: String,
    },
    /// A question waiting in a sprint's planning: that sprint's planning.
    Planning {
        project: String,
        sprint: String,
        decision: String,
    },
    /// A result ready for the owner's review: the task, with its result in view.
    Result { project: String, task: String },
    /// A task that failed: the task.
    Task { project: String, task: String },
    /// A sprint that paused itself: the sprint.
    Sprint { project: String, sprint: String },
}

impl NotificationTarget {
    /// One notification per place: a newer state of the same task or sprint replaces the older one.
    pub fn place(&self) -> (&'static str, &str, &str) {
        match self {
            Self::Question { project, task, .. }
            | Self::Result { project, task }
            | Self::Task { project, task } => ("task", project, task),
            Self::Planning {
                project, sprint, ..
            }
            | Self::Sprint { project, sprint } => ("sprint", project, sprint),
        }
    }
    fn ids(&self) -> Vec<&str> {
        match self {
            Self::Question {
                project,
                task,
                decision,
            } => vec![project, task, decision],
            Self::Planning {
                project,
                sprint,
                decision,
            } => vec![project, sprint, decision],
            Self::Result { project, task } | Self::Task { project, task } => vec![project, task],
            Self::Sprint { project, sprint } => vec![project, sprint],
        }
    }
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The closed shape of a notification request from the window: a bounded label and one target.
pub fn notification_target(label: &str, target: Value) -> Result<NotificationTarget, String> {
    if label.chars().count() > MAX_LABEL {
        return Err("notification_label_too_long".into());
    }
    let target: NotificationTarget =
        serde_json::from_value(target).map_err(|_| "notification_target_refused")?;
    if !target.ids().iter().all(|id| id_ok(id)) {
        return Err("notification_target_refused".into());
    }
    Ok(target)
}

#[derive(Debug)]
struct Entry {
    id: u32,
    target: NotificationTarget,
    token: Option<String>,
}

/// This window's notifications that the server still shows, newest last.
#[derive(Debug, Default)]
pub struct Outstanding {
    entries: VecDeque<Entry>,
}

/// What a click on one of this window's notifications asks for.
#[derive(Debug, PartialEq, Eq)]
pub struct Opened {
    pub target: NotificationTarget,
    /// The server's activation token, when it sent one before the action (xdg-activation on Wayland,
    /// a startup-notification id on X11).
    pub token: Option<String>,
}

impl Outstanding {
    /// The id to replace for a notification about the same place, or 0 for a new one.
    pub fn replaces(&self, target: &NotificationTarget) -> u32 {
        self.entries
            .iter()
            .rev()
            .find(|e| e.target.place() == target.place())
            .map_or(0, |e| e.id)
    }
    /// Records the id the server returned. Ids are the server's; 0 is never a notification.
    pub fn sent(&mut self, id: u32, target: NotificationTarget) {
        if id == 0 {
            return;
        }
        self.entries
            .retain(|e| e.id != id && e.target.place() != target.place());
        self.entries.push_back(Entry {
            id,
            target,
            token: None,
        });
        while self.entries.len() > MAX_OUTSTANDING {
            self.entries.pop_front();
        }
    }
    /// Keeps the server's activation token for a known id until its action arrives.
    pub fn token(&mut self, id: u32, token: &str) {
        let usable = !token.is_empty()
            && token.len() <= MAX_TOKEN
            && token.chars().all(|c| c.is_ascii_graphic());
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.token = usable.then(|| token.to_owned());
        }
    }
    /// The default action (a click on the notification body) on a known id, once.
    pub fn invoked(&mut self, id: u32, action: &str) -> Option<Opened> {
        if action != "default" {
            return None;
        }
        let at = self.entries.iter().position(|e| e.id == id)?;
        let entry = self.entries.remove(at)?;
        Some(Opened {
            target: entry.target,
            token: entry.token,
        })
    }
    /// The server closed it (expired, dismissed, replaced elsewhere): it can no longer be clicked.
    pub fn closed(&mut self, id: u32) {
        self.entries.retain(|e| e.id != id);
    }
    /// Every id still out, for withdrawal when the window closes.
    pub fn drain(&mut self) -> Vec<u32> {
        self.entries.drain(..).map(|e| e.id).collect()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn question(task: &str, decision: &str) -> NotificationTarget {
        NotificationTarget::Question {
            project: "p".into(),
            task: task.into(),
            decision: decision.into(),
        }
    }

    #[test]
    fn a_click_returns_the_target_once_and_only_for_its_own_id() {
        let mut out = Outstanding::default();
        out.sent(7, question("t", "q"));
        assert_eq!(out.invoked(8, "default"), None);
        assert_eq!(out.invoked(7, "open"), None);
        out.token(7, "token-1");
        assert_eq!(
            out.invoked(7, "default"),
            Some(Opened {
                target: question("t", "q"),
                token: Some("token-1".into())
            })
        );
        assert_eq!(out.invoked(7, "default"), None);
    }

    #[test]
    fn one_notification_per_place_replaces_the_older_one() {
        let mut out = Outstanding::default();
        assert_eq!(out.replaces(&question("t", "q1")), 0);
        out.sent(3, question("t", "q1"));
        assert_eq!(out.replaces(&question("t", "q2")), 3);
        out.sent(3, question("t", "q2"));
        assert_eq!(out.len(), 1);
        assert_eq!(
            out.invoked(3, "default").unwrap().target,
            question("t", "q2")
        );
        out.sent(4, question("a", "x"));
        out.sent(5, question("b", "x"));
        assert_eq!(out.replaces(&question("c", "x")), 0);
    }

    #[test]
    fn closed_and_withdrawn_ids_no_longer_open_anything() {
        let mut out = Outstanding::default();
        out.sent(1, question("a", "x"));
        out.sent(2, question("b", "x"));
        out.sent(0, question("c", "x"));
        out.closed(1);
        assert_eq!(out.invoked(1, "default"), None);
        assert_eq!(out.drain(), vec![2]);
        assert!(out.is_empty());
        assert_eq!(out.invoked(2, "default"), None);
    }

    #[test]
    fn the_registry_and_tokens_stay_bounded() {
        let mut out = Outstanding::default();
        for id in 1..=(MAX_OUTSTANDING as u32 + 10) {
            out.sent(id, question(&format!("t{id}"), "q"));
        }
        assert_eq!(out.len(), MAX_OUTSTANDING);
        assert_eq!(out.invoked(1, "default"), None);
        let last = MAX_OUTSTANDING as u32 + 10;
        out.token(last, &"x".repeat(MAX_TOKEN + 1));
        assert_eq!(out.invoked(last, "default").unwrap().token, None);
        out.sent(1, question("t", "q"));
        out.token(1, "bad token");
        assert_eq!(out.invoked(1, "default").unwrap().token, None);
    }

    #[test]
    fn targets_are_a_closed_bounded_shape() {
        let ok = json!({"kind":"question","project":"p","task":"t","decision":"d"});
        assert_eq!(notification_target("Fix", ok).unwrap(), question("t", "d"));
        let planning = json!({"kind":"planning","project":"p","sprint":"s","decision":"d"});
        assert!(notification_target("Plan", planning).is_ok());
        for ok in [
            json!({"kind":"result","project":"p","task":"t"}),
            json!({"kind":"task","project":"p","task":"t"}),
            json!({"kind":"sprint","project":"p","sprint":"s"}),
        ] {
            assert!(notification_target("", ok.clone()).is_ok(), "{ok}");
        }
        for bad in [
            json!({"kind":"result","project":"p","task":"t","decision":"d"}),
            json!({"kind":"sprint","project":"p","task":"t"}),
            json!({"kind":"task","project":"p","task":""}),
            json!({"kind":"question","project":"p","task":"t"}),
            json!({"kind":"question","project":"p","task":"t","decision":"d","href":"/x"}),
            json!({"kind":"url","href":"https://example.invalid"}),
            json!({"kind":"planning","project":"p","task":"t","decision":"d"}),
            json!({"kind":"question","project":"","task":"t","decision":"d"}),
            json!({"kind":"question","project":"p","task":"t\n","decision":"d"}),
            json!({"kind":"question","project":"p","task":"a b","decision":"d"}),
            json!({"kind":"question","project":"p","task":"x".repeat(MAX_ID + 1),"decision":"d"}),
            json!({"kind":"question","project":1,"task":"t","decision":"d"}),
            json!("question"),
        ] {
            assert!(notification_target("Fix", bad.clone()).is_err(), "{bad}");
        }
        let ok = json!({"kind":"question","project":"p","task":"t","decision":"d"});
        assert_eq!(
            notification_target(&"x".repeat(MAX_LABEL + 1), ok),
            Err("notification_label_too_long".into())
        );
    }

    #[test]
    fn the_window_receives_exactly_the_validated_shape() {
        let value = serde_json::to_value(question("t", "d")).unwrap();
        assert_eq!(
            value,
            json!({"kind":"question","project":"p","task":"t","decision":"d"})
        );
    }
}
