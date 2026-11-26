//! Desktop notifications raised by the service, so they arrive while no window is open.
//!
//! The service already learns of every runtime change of a project (`native_delta` and
//! `native_resync` events) and of its own owner-state writes (`invalidation`). On each, after a
//! short settle, it reads that one project once (its runtime tasks, its owner board and the
//! pending questions of tasks that have one) and works out the moments that need the owner:
//! a question or permission in a task, the runtime's own stop question, a result to review, a
//! task that failed, an access card, a sprint planning question, and a sprint that paused itself. Each item
//! has one notification: a newer state of the same task or sprint replaces it, and an item
//! that is gone (answered, accepted, closed, archived) withdraws it. Nothing is read on a timer.
//!
//! A notification goes to `org.freedesktop.Notifications` on the owner's session bus with the
//! installed desktop entry and the default action. A click arrives here as
//! `ActivationToken`/`ActionInvoked` for an id the service raised. If a desktop window is
//! connected it receives the target as a `notification_open` service event and answers with
//! `notifications.opened`, which hands it the activation token so the window raises itself;
//! if none answers within two seconds the service starts the desktop app through the user
//! manager with the token as `DESKTOP_STARTUP_ID`/`XDG_ACTIVATION_TOKEN`, keeping the target
//! for the new window's `notifications.take` (single use, one minute). Without a session bus
//! (a headless machine) nothing is raised and `service.status` says so.
//!
//! One kind is also looked at on a timer: a working task that has made no progress for the
//! owner's threshold (3 hours unless the window reports another, `stallHours`) is announced
//! once, and again every 3 hours while it stays stuck (see `progress`). A light read of each
//! active project's working tasks every five minutes rereads a project only when such a moment
//! is due. The notification never pauses or changes the task; the runtime records
//! `NO_PROGRESS_NOTIFIED` with its numbers.
use crate::session_bus::{self, Arg, Bus, Message};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "native_progress.rs"]
pub(crate) mod progress;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// Changes arriving together are read once.
const SETTLE: Duration = Duration::from_millis(400);
/// How long a window has to claim a click before the app is started instead.
const CLAIM: Duration = Duration::from_secs(2);
/// How long a started window may take to collect its target.
const PENDING: Duration = Duration::from_secs(60);
/// A window's report of what it shows holds this long unless renewed.
const FOCUS_TTL: Duration = Duration::from_secs(90);
/// The newest item states remembered as announced.
const SEEN_LIMIT: usize = 1000;
/// Bound on ids and tokens taken from the window or the bus.
const MAX_ID: usize = 256;
const MAX_TOKEN: usize = 512;
/// The no-progress thresholds the owner can choose, in hours, and the default.
const STALL_HOURS: [u32; 4] = [1, 2, 3, 6];
const STALL_DEFAULT: u32 = 3;
/// A task still stuck is announced again after this long.
const STALL_REPEAT_MS: i64 = 3 * 3_600_000;
/// How often the working tasks are looked at for a no-progress moment.
const STALL_TICK: Duration = Duration::from_secs(300);
/// The moments one snapshot for paired phones carries at most.
const MOMENTS_LIMIT: usize = 200;
/// A moment's title on a phone, in characters.
const MOMENT_TITLE: usize = 120;

/// The moments that need the owner, one setting each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Question,
    Stop,
    Result,
    Failed,
    Planning,
    Paused,
    /// A working task that has made no progress for the owner's threshold.
    Stalled,
}
impl Kind {
    pub(crate) const ALL: [Kind; 7] = [
        Kind::Question,
        Kind::Stop,
        Kind::Result,
        Kind::Failed,
        Kind::Planning,
        Kind::Paused,
        Kind::Stalled,
    ];
    fn summary(self) -> &'static str {
        match self {
            Kind::Question => "Question waiting",
            Kind::Stop => "Task stopped: your decision",
            Kind::Result => "Result ready for review",
            Kind::Failed => "Task failed",
            Kind::Planning => "Planning question waiting",
            Kind::Paused => "Sprint paused",
            Kind::Stalled => "No progress",
        }
    }
}

/// A task or sprint of one project: what one notification is about.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Place {
    pub project: String,
    pub sprint: bool,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    pub kind: Kind,
    pub place: Place,
    pub decision: Option<String>,
    pub label: String,
    pub detail: Option<String>,
    /// The item's state: a new state is a new notification for the same place.
    pub state: String,
}
impl Item {
    /// What the window opens; the same closed shapes the desktop shell validates.
    pub(crate) fn target(&self) -> Value {
        let p = &self.place;
        match (self.kind, &self.decision) {
            (Kind::Question | Kind::Stop, Some(d)) => {
                json!({"kind":"question","project":p.project,"task":p.id,"decision":d})
            }
            (Kind::Planning, Some(d)) => {
                json!({"kind":"planning","project":p.project,"sprint":p.id,"decision":d})
            }
            (Kind::Result, _) => json!({"kind":"result","project":p.project,"task":p.id}),
            _ if p.sprint => json!({"kind":"sprint","project":p.project,"sprint":p.id}),
            _ => json!({"kind":"task","project":p.project,"task":p.id}),
        }
    }
    /// Access cards have their own slots: neither a question nor another
    /// access card for the same task may replace or hide one.
    fn slot(&self) -> (Place, Option<String>) {
        (
            self.place.clone(),
            self.state.strip_prefix("oa:").map(str::to_owned),
        )
    }
    fn summary(&self) -> &'static str {
        if self.state.starts_with("mcp:") {
            return "MCP sign-in needed";
        }
        if self.state.starts_with("oa:") {
            "Task needs access"
        } else {
            self.kind.summary()
        }
    }
    fn fingerprint(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.place.project,
            if self.place.sprint { "s" } else { "t" },
            self.place.id,
            self.state
        )
    }
    fn body(&self) -> String {
        let clean = |s: &str, n: usize| -> String {
            s.chars().filter(|c| !c.is_control()).take(n).collect()
        };
        let mut body = escape(&clean(&self.label, 200));
        if let Some(detail) = &self.detail {
            body.push('\n');
            body.push_str(&escape(&clean(&plain_detail(detail), 300)));
        }
        body
    }
}

/// One owner moment as a paired phone reads it (`notifications.subscribe`): its kind, an
/// opaque fingerprint of its state, the task or sprint title and the closed target the
/// desktop opens. Never a detail, a question body, a path or a runtime id beyond the target.
pub(crate) fn moment(item: &Item) -> Value {
    use sha2::{Digest, Sha256};
    let kind = if item.state.starts_with("oa:") {
        Value::from("access")
    } else {
        serde_json::to_value(item.kind).unwrap_or(Value::Null)
    };
    let title: String = item
        .label
        .chars()
        .filter(|c| !c.is_control())
        .take(MOMENT_TITLE)
        .collect();
    let digest = format!("{:x}", Sha256::digest(item.fingerprint().as_bytes()));
    json!({"kind":kind,"fingerprint":&digest[..32],"title":title,"target":item.target()})
}

/// Notification text is read by the owner: a bare internal code becomes a
/// generic sentence, and a snake_case code inside a sentence is spelled out.
/// Tokens touching `/` or `.` (paths, file names) are kept as written.
fn plain_detail(detail: &str) -> String {
    let code = |w: &str| {
        w.contains('_')
            && w.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    let trimmed = detail.trim().trim_end_matches('.');
    if code(trimmed) {
        return "Open it to see what is needed.".into();
    }
    let mut out = String::with_capacity(detail.len());
    let mut word = String::new();
    let mut prev: Option<char> = None;
    let flush = |word: &mut String, out: &mut String, next: Option<char>, prev: Option<char>| {
        let pathlike = matches!(prev, Some('/' | '.')) || next == Some('/');
        if !pathlike && code(word) {
            out.push_str(&word.replace('_', " "));
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    let chars: Vec<char> = detail.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_alphanumeric() || c == '_' {
            if word.is_empty() {
                prev = i.checked_sub(1).map(|j| chars[j]);
            }
            word.push(c);
        } else {
            let next_after = chars.get(i + 1).copied();
            // A '.' followed by a letter is a file extension: keep the word.
            let ext = c == '.' && next_after.is_some_and(|n| n.is_ascii_alphanumeric());
            if ext {
                out.push_str(&word);
                word.clear();
            } else {
                flush(&mut word, &mut out, Some(c), prev);
            }
            out.push(c);
        }
    }
    flush(&mut word, &mut out, None, prev);
    out
}
/// Notification bodies can be markup; project text is plain.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}
fn first_line(text: &str, limit: usize) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(if line.chars().count() > limit {
        format!("{}…", line.chars().take(limit).collect::<String>())
    } else {
        line.to_owned()
    })
}

/// The owner moments of one project, from its runtime tasks (`tasks.page` items), its owner
/// board (`product.board`) and the decisions of the tasks with an open question.
pub(crate) fn items(
    project: &str,
    tasks: &[Value],
    product: &Value,
    decisions: &HashMap<String, Vec<Value>>,
    cards: &[crate::native_owner_actions::Open],
) -> Vec<Item> {
    let records = &product["tasks"];
    let sprints: Vec<&Value> = product["sprints"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| {
            s["archivedAt"].is_null()
                && !s
                    .get("deleting")
                    .is_some_and(|d| !d.is_null() && *d != false)
        })
        .collect();
    // A queue-driven sprint accepts its members' results itself; one it cannot accept pauses the
    // sprint, which is its own notification.
    let automatic: HashSet<&str> = sprints
        .iter()
        .filter(|s| crate::machine::queue_driven_mode(s))
        .flat_map(|s| {
            s["taskIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })
        .collect();
    let unattended: HashSet<&str> = sprints
        .iter()
        .filter(|s| s["runMode"] == "automatic")
        .flat_map(|s| {
            s["taskIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })
        .collect();
    // A sprint's planning task is part of its sprint, never an owner moment of its own.
    let planning: HashSet<&str> = product["sprints"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["planningTaskId"].as_str())
        .collect();
    let mut out = Vec::new();
    for t in tasks {
        let Some(id) = t["taskId"].as_str().filter(|id| id_ok(id)) else {
            continue;
        };
        let record = &records[id];
        if !record["archive"].is_null()
            || record["purged"] == true
            || record["retiredPlanner"] == true
            || planning.contains(id)
        {
            continue;
        }
        let place = Place {
            project: project.into(),
            sprint: false,
            id: id.into(),
        };
        let label = record["title"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| first_line(t["description"].as_str().unwrap_or(""), 80))
            .unwrap_or_else(|| "A task".into());
        let settled = matches!(record["status"].as_str(), Some("ready_to_merge" | "done"))
            || !record["closedAs"].is_null();
        let mut has_access_card = false;
        for card in cards.iter().filter(|c| c.task.as_deref() == Some(id) && id_ok(&c.id)) {
            let Some(task_execution) = t["executionId"].as_str() else { continue };
            // The card's task id alone cannot distinguish a superseded execution.
            if settled || card.execution.as_deref() != Some(task_execution) {
                continue;
            }
            has_access_card = true;
            out.push(Item {
                kind: Kind::Stop,
                state: format!("oa:{}", card.id),
                decision: None,
                detail: Some(crate::native_owner_actions::access(&card.service).title.into()),
                place: place.clone(),
                label: label.clone(),
            });
        }
        let pending = decisions.get(id).into_iter().flatten().find(|d| {
            d["status"] == "pending"
                && d["decisionId"].as_str().is_some_and(id_ok)
                && !(t["automaticContinuation"].is_object() && d["runtimeStop"] == true)
        });
        let execution = t["executionId"].as_str().unwrap_or("");
        let at = t["lastConfirmedAt"].as_str().unwrap_or("");
        let item = if let Some(d) = pending {
            let decision = d["decisionId"].as_str().unwrap().to_owned();
            let stop = d["runtimeStop"] == true;
            Some(Item {
                kind: if stop { Kind::Stop } else { Kind::Question },
                state: format!("q:{decision}"),
                decision: Some(decision),
                detail: stop
                    .then(|| d["headline"].as_str().map(str::to_owned))
                    .flatten(),
                place,
                label,
            })
        } else {
            match t["phase"].as_str() {
                Some("result") if !settled && !automatic.contains(id) => Some(Item {
                    kind: Kind::Result,
                    state: format!("r:{execution}:{at}"),
                    decision: None,
                    detail: None,
                    place,
                    label,
                }),
                Some("failed" | "blocked")
                    if !settled && !has_access_card && !unattended.contains(id) => Some(Item {
                    kind: Kind::Failed,
                    state: format!("f:{execution}:{at}"),
                    decision: None,
                    detail: None,
                    place,
                    label,
                }),
                _ => None,
            }
        };
        out.extend(item);
    }
    for s in sprints {
        let Some(id) = s["id"].as_str().filter(|id| id_ok(id)) else {
            continue;
        };
        let place = Place {
            project: project.into(),
            sprint: true,
            id: id.into(),
        };
        let label = s["title"]
            .as_str()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or("Sprint")
            .to_owned();
        let asked_now =
            s["planningTaskId"].as_str().and_then(|task| {
                decisions.get(task).into_iter().flatten().find(|d| {
                    d["status"] == "pending" && d["decisionId"].as_str().is_some_and(id_ok)
                })
            });
        if let Some(d) = asked_now.filter(|_| s["planning"] == "planning") {
            // The planning task asks the owner: the sprint's planning question.
            let decision = d["decisionId"].as_str().unwrap().to_owned();
            out.push(Item {
                kind: Kind::Planning,
                state: format!("pq:{decision}"),
                decision: Some(decision),
                detail: None,
                place,
                label,
            });
        } else if s["planning"] == "question" {
            let asked: Vec<&Value> = s["conversation"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["kind"] == "question")
                .collect();
            let decision = asked
                .last()
                .and_then(|m| m["id"].as_str())
                .filter(|d| id_ok(d))
                .map(str::to_owned)
                .unwrap_or_else(|| format!("planning-{}", asked.len()));
            out.push(Item {
                kind: Kind::Planning,
                state: format!("pq:{}", asked.len()),
                decision: Some(decision),
                detail: None,
                place,
                label,
            });
        } else if s["queueState"] == "paused" {
            let hold = &s["automaticQueue"];
            // A Hybrid or Automatic queue hold notifies only when the owner acts next;
            // one the runtime rechecks or an agent fixes is not the owner's moment.
            let automatic =
                s["runMode"] == "automatic" || s["runMode"] == "hybrid" && hold.is_object();
            if automatic
                && (hold["needsOwner"] != true
                    || hold["nextActor"]
                        .as_str()
                        .is_some_and(|actor| actor != "owner"))
            {
                continue;
            }
            let reason = if automatic {
                hold["reason"]
                    .as_str()
                    .or_else(|| s["pauseReason"].as_str())
            } else {
                s["pauseReason"].as_str()
            };
            let Some(reason) = reason.filter(|r| !r.trim().is_empty()) else {
                continue;
            };
            // The owner's own pause, closing and deletion, and Manual mode's stop after each
            // result (that result is its own notification), are not news to the owner.
            if reason == "This sprint is in manual mode."
                || reason == "Paused by you"
                || reason == "Sprint closed by you"
                || reason.starts_with("Deletion pending")
            {
                continue;
            }
            out.push(Item {
                kind: Kind::Paused,
                state: if automatic {
                    format!(
                        "p:{}:{}:{reason}",
                        hold["code"].as_str().unwrap_or_default(),
                        hold["taskId"].as_str().unwrap_or_default()
                    )
                } else {
                    format!("p:{reason}")
                },
                decision: None,
                detail: Some(reason.to_owned()),
                place,
                label,
            });
        }
    }
    out
}

/// A working task's no-progress moment: its item, its runtime task and the numbers the
/// runtime records when it is announced (`NO_PROGRESS_NOTIFIED`).
#[derive(Debug)]
pub(crate) struct Stall {
    pub item: Item,
    pub runtime: String,
    pub numbers: Value,
}
/// How long a working task has been without progress, and which announcement that is
/// (0 at the threshold, one more every 3 hours after it), once it reached the threshold.
pub(crate) fn stall_round(since: i64, now: i64, hours: u32) -> Option<(i64, i64)> {
    let threshold = i64::from(hours) * 3_600_000;
    let elapsed = now - since;
    (hours > 0 && elapsed >= threshold).then(|| (elapsed, (elapsed - threshold) / STALL_REPEAT_MS))
}
/// Every working task's due announcement, as `task|round`: what the timer compares.
pub(crate) fn stall_keys(working: &[progress::Working], now: i64, hours: u32) -> BTreeSet<String> {
    working
        .iter()
        .filter_map(|w| {
            let since = w.spend.stalled_since?;
            let (_, round) = stall_round(since, now, hours)?;
            Some(format!("{}|{since}:{round}", w.task))
        })
        .collect()
}
/// The no-progress moments of one project's working tasks: never a sprint's planning task, an
/// archived or settled task, or one that has another owner moment (`taken`).
pub(crate) fn stalls(
    project: &str,
    tasks: &[Value],
    product: &Value,
    working: &[progress::Working],
    taken: &BTreeSet<Place>,
    now: i64,
    hours: u32,
) -> Vec<Stall> {
    let records = &product["tasks"];
    let planning: HashSet<&str> = product["sprints"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["planningTaskId"].as_str())
        .collect();
    let mut out = Vec::new();
    for w in working {
        let Some(t) = tasks.iter().find(|t| t["taskId"] == w.task.as_str()) else {
            continue;
        };
        let record = &records[&w.task];
        let settled = matches!(record["status"].as_str(), Some("ready_to_merge" | "done"))
            || !record["closedAs"].is_null();
        if !id_ok(&w.task)
            || !record["archive"].is_null()
            || record["purged"] == true
            || settled
            || planning.contains(w.task.as_str())
        {
            continue;
        }
        let place = Place {
            project: project.into(),
            sprint: false,
            id: w.task.clone(),
        };
        let (Some(since), false) = (w.spend.stalled_since, taken.contains(&place)) else {
            continue;
        };
        let Some((elapsed, round)) = stall_round(since, now, hours) else {
            continue;
        };
        let title = record["title"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| first_line(t["description"].as_str().unwrap_or(""), 80))
            .unwrap_or_else(|| "A task".into());
        let spend = &w.spend;
        out.push(Stall {
            item: Item {
                kind: Kind::Stalled,
                place,
                decision: None,
                label: format!(
                    "{title} has made no progress for {} h.",
                    elapsed / 3_600_000
                ),
                detail: Some(progress::summary(spend)),
                state: format!("np:{since}:{round}"),
            },
            runtime: w.runtime.clone(),
            numbers: json!({"threshold_hours":hours,"stalled_ms":elapsed,"round":round,
                "working_ms":spend.working_ms,"sessions":spend.sessions(),"turns":spend.turns(),
                "reported_turns":spend.reported_turns(),"tokens":spend.tokens(),"failures":spend.reasons.len()}),
        });
    }
    out
}

#[derive(Debug)]
struct Shown {
    id: u32,
    state: String,
    target: Value,
    token: Option<String>,
}

/// What the notification server must do after one project was read.
#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    Raise { item: Item, replaces: u32 },
    Withdraw { id: u32 },
}

/// This service's notifications that can still be clicked, and the item states already announced.
#[derive(Debug, Default)]
pub(crate) struct Book {
    shown: BTreeMap<(Place, Option<String>), Shown>,
    seen: VecDeque<String>,
    seen_set: HashSet<String>,
}
impl Book {
    fn see(&mut self, fingerprint: String) {
        if self.seen_set.insert(fingerprint.clone()) {
            self.seen.push_back(fingerprint);
            while self.seen.len() > SEEN_LIMIT {
                if let Some(old) = self.seen.pop_front() {
                    self.seen_set.remove(&old);
                }
            }
        }
    }
    /// One complete read of `project`. `prime` marks everything as already announced without
    /// raising anything (the first start, so an upgrade does not announce every open item).
    pub(crate) fn plan(
        &mut self,
        project: &str,
        items: Vec<Item>,
        focus: Option<&Place>,
        enabled: &BTreeSet<Kind>,
        prime: bool,
    ) -> Vec<Action> {
        let current: BTreeMap<_, Item> = items
            .into_iter()
            .filter(|i| i.place.project == project)
            .map(|i| (i.slot(), i))
            .collect();
        let mut actions = Vec::new();
        let gone: Vec<_> = self
            .shown
            .keys()
            .filter(|p| p.0.project == project && !current.contains_key(*p))
            .cloned()
            .collect();
        for place in gone {
            if let Some(shown) = self.shown.remove(&place) {
                actions.push(Action::Withdraw { id: shown.id });
            }
        }
        for (place, item) in current {
            let fingerprint = item.fingerprint();
            let looked_at = focus == Some(&place.0);
            if looked_at || prime || !enabled.contains(&item.kind) {
                // Looking at it is seeing it; so is a kind the owner turned off.
                self.see(fingerprint);
                if looked_at || !enabled.contains(&item.kind) {
                    if let Some(shown) = self.shown.remove(&place) {
                        actions.push(Action::Withdraw { id: shown.id });
                    }
                }
                continue;
            }
            if self.seen_set.contains(&fingerprint) {
                // Announced before. A notification for an older state of it gives way.
                if self
                    .shown
                    .get(&place)
                    .is_some_and(|s| s.state != item.state)
                {
                    let shown = self.shown.remove(&place).unwrap();
                    actions.push(Action::Withdraw { id: shown.id });
                }
                continue;
            }
            self.see(fingerprint);
            let replaces = self.shown.get(&place).map_or(0, |s| s.id);
            actions.push(Action::Raise { item, replaces });
        }
        actions
    }
    /// The server's id for a raised item.
    pub(crate) fn raised(&mut self, item: &Item, id: u32) {
        if id == 0 {
            return;
        }
        self.shown.retain(|_, s| s.id != id);
        self.shown.insert(
            item.slot(),
            Shown {
                id,
                state: item.state.clone(),
                target: item.target(),
                token: None,
            },
        );
    }
    pub(crate) fn token(&mut self, id: u32, token: &str) {
        let usable = !token.is_empty()
            && token.len() <= MAX_TOKEN
            && token.chars().all(|c| c.is_ascii_graphic());
        if let Some(shown) = self.shown.values_mut().find(|s| s.id == id) {
            shown.token = usable.then(|| token.to_owned());
        }
    }
    /// A click on the body of one of this service's notifications, once.
    pub(crate) fn invoked(&mut self, id: u32, action: &str) -> Option<(Value, Option<String>)> {
        if action != "default" {
            return None;
        }
        let place = self
            .shown
            .iter()
            .find(|(_, s)| s.id == id)
            .map(|(p, _)| p.clone())?;
        let shown = self.shown.remove(&place)?;
        Some((shown.target, shown.token))
    }
    pub(crate) fn closed(&mut self, id: u32) {
        self.shown.retain(|_, s| s.id != id);
    }
    fn seen_list(&self) -> Vec<String> {
        self.seen.iter().cloned().collect()
    }
}

/// A click waiting for a window to claim it, and a target kept for a window being started.
#[derive(Debug, Default)]
pub(crate) struct Handoff {
    waiting: Option<(String, Value, Option<String>, Instant)>,
    pending: Option<(Value, Instant)>,
    next: u64,
}
impl Handoff {
    pub(crate) fn clicked(&mut self, target: Value, token: Option<String>, now: Instant) -> String {
        self.next += 1;
        let nonce = format!("nc{:016x}{}", random(), self.next);
        self.waiting = Some((nonce.clone(), target, token, now));
        nonce
    }
    /// A window claims the click: it gets the activation token and the click is done.
    pub(crate) fn opened(&mut self, nonce: &str) -> Option<Option<String>> {
        match self.waiting.take() {
            Some((n, _, token, _)) if n == nonce => Some(token),
            other => {
                self.waiting = other;
                None
            }
        }
    }
    /// No window claimed it in time: the app is started and the target kept for it.
    pub(crate) fn unclaimed(&mut self, nonce: &str, now: Instant) -> Option<Option<String>> {
        match self.waiting.take() {
            Some((n, target, token, _)) if n == nonce => {
                self.pending = Some((target, now));
                Some(token)
            }
            other => {
                self.waiting = other;
                None
            }
        }
    }
    /// The started window's target, once, while it is fresh.
    pub(crate) fn take(&mut self, now: Instant) -> Option<Value> {
        let (target, at) = self.pending.take()?;
        (now.duration_since(at) < PENDING).then_some(target)
    }
}
fn random() -> u64 {
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = io::Read::read_exact(&mut f, &mut bytes);
    }
    u64::from_le_bytes(bytes)
}

/// What a desktop window reports: the task or sprint on view in the focused window (if any)
/// and the kinds the owner wants. Exactly these fields.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Report {
    pub focused: Option<ReportPlace>,
    pub kinds: Vec<Kind>,
    /// The no-progress threshold in hours (one of `STALL_HOURS`), when the window names one.
    #[serde(default, rename = "stallHours")]
    pub stall_hours: Option<u32>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReportPlace {
    pub project: String,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub sprint: Option<String>,
}
#[cfg(test)]
pub(crate) fn report(params: Value) -> io::Result<(Option<Place>, BTreeSet<Kind>)> {
    report_with_threshold(params).map(|(place, kinds, _)| (place, kinds))
}
/// `report` with the no-progress threshold the window named, if any.
pub(crate) fn report_with_threshold(
    params: Value,
) -> io::Result<(Option<Place>, BTreeSet<Kind>, Option<u32>)> {
    let bad = || crate::failure("invalid_params");
    // `focused` is always named, null when no task or sprint is on view.
    if params.get("focused").is_none() {
        return Err(bad());
    }
    let r: Report = serde_json::from_value(params).map_err(|_| bad())?;
    if r.stall_hours.is_some_and(|h| !STALL_HOURS.contains(&h)) {
        return Err(bad());
    }
    let stall = r.stall_hours;
    let place = match r.focused {
        None => None,
        Some(p) => {
            let (sprint, id) = match (p.task, p.sprint) {
                (Some(t), None) => (false, t),
                (None, Some(s)) => (true, s),
                _ => return Err(bad()),
            };
            if !id_ok(&p.project) || !id_ok(&id) {
                return Err(bad());
            }
            Some(Place {
                project: p.project,
                sprint,
                id,
            })
        }
    };
    Ok((place, r.kinds.into_iter().collect(), stall))
}

/// The command that starts the desktop app through the user manager, so it runs in the
/// graphical session's environment rather than the service's.
pub(crate) fn launch_command(token: Option<&str>) -> Vec<String> {
    let channel = workspace_protocol::channel::current();
    let mut argv: Vec<String> = [
        "systemd-run",
        "--user",
        "--collect",
        "--quiet",
        "--no-block",
    ]
    .map(str::to_owned)
    .into();
    if let Some(token) = token.filter(|t| {
        !t.is_empty() && t.len() <= MAX_TOKEN && t.chars().all(|c| c.is_ascii_graphic())
    }) {
        argv.push(format!("--setenv=DESKTOP_STARTUP_ID={token}"));
        argv.push(format!("--setenv=XDG_ACTIVATION_TOKEN={token}"));
    }
    argv.push("--".into());
    let installed = PathBuf::from("/usr/bin").join(&channel.desktop_binary);
    argv.push(if installed.is_file() {
        installed.to_string_lossy().into_owned()
    } else {
        channel.desktop_binary.clone()
    });
    argv
}

#[derive(Default)]
struct Inner {
    book: Book,
    handoff: Handoff,
    focus: Option<(Place, Instant)>,
    enabled: Option<BTreeSet<Kind>>,
    /// The owner's no-progress threshold in hours; `STALL_DEFAULT` until a window names one.
    stall_hours: Option<u32>,
    /// Per project, the due no-progress announcements its last read found (`stall_keys`).
    stall_seen: HashMap<String, BTreeSet<String>>,
    /// The runtime task and numbers of each no-progress item, for its runtime record.
    stall_numbers: HashMap<Place, (String, Value)>,
    /// No saved state yet: the first read of each project only records what is open.
    first: bool,
    primed: BTreeSet<String>,
    /// Started by the real service: projects are read for moments, with or without a bus.
    running: bool,
    /// Per project, the moments its last read found, for paired phones.
    moments: BTreeMap<String, Vec<Value>>,
    bus: Option<Arc<Bus>>,
    server: Option<String>,
    status: &'static str,
    state_file: Option<PathBuf>,
}

/// The service's notifier. Inactive (and silent) until the real service starts it.
#[derive(Default)]
pub(crate) struct Notifier {
    inner: Mutex<Inner>,
    dirty: Mutex<BTreeSet<String>>,
    wake: tokio::sync::Notify,
    /// Every project's moments as one snapshot (`{"moments":[…]}`), for paired phones.
    moments: tokio::sync::watch::Sender<Value>,
}
impl Notifier {
    /// The moments snapshot and its later changes (`notifications.subscribe`).
    pub(crate) fn moments(&self) -> tokio::sync::watch::Receiver<Value> {
        self.moments.subscribe()
    }
    /// One project's moments after a read; the snapshot changes only when they did.
    fn publish_moments(&self, inner: &mut Inner, project: &str, found: Vec<Value>) {
        if found.is_empty() {
            inner.moments.remove(project);
        } else {
            inner.moments.insert(project.to_owned(), found);
        }
        let all: Vec<Value> = inner
            .moments
            .values()
            .flatten()
            .take(MOMENTS_LIMIT)
            .cloned()
            .collect();
        let next = json!({"moments": all});
        self.moments.send_if_modified(|current| {
            let changed = *current != next;
            if changed {
                *current = next;
            }
            changed
        });
    }
    pub(crate) fn status(&self) -> Value {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        json!({"state": if inner.status.is_empty() { "not_started" } else { inner.status }})
    }
    fn raising(&self) -> bool {
        self.inner.lock().map(|i| i.bus.is_some()).unwrap_or(false)
    }
    /// A project changed in a way that can move an item: read it again soon.
    pub(crate) fn touch(&self, project: &str) {
        let running = self.inner.lock().map(|i| i.running).unwrap_or(false);
        if !running || !id_ok(project) {
            return;
        }
        if let Ok(mut dirty) = self.dirty.lock() {
            dirty.insert(project.to_owned());
        }
        self.wake.notify_one();
    }
    fn enabled(inner: &Inner) -> BTreeSet<Kind> {
        inner
            .enabled
            .clone()
            .unwrap_or_else(|| Kind::ALL.into_iter().collect())
    }
    fn stall_hours(inner: &Inner) -> u32 {
        inner.stall_hours.unwrap_or(STALL_DEFAULT)
    }
    fn save(inner: &Inner) {
        let Some(path) = &inner.state_file else {
            return;
        };
        let value = json!({"kinds": Self::enabled(inner), "seen": inner.book.seen_list(),
            "stallHours": Self::stall_hours(inner)});
        let _ = write_private(path, &serde_json::to_vec(&value).unwrap_or_default());
    }
}
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    io::Write::write_all(&mut file, bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
}

/// One event's project, when it can move an owner moment.
fn changed_project(event: &Value) -> Option<&str> {
    match event["type"].as_str()? {
        "native_delta" | "native_resync" | "invalidation" => event["projectId"].as_str(),
        _ => None,
    }
}

fn notify_body(item: &Item, replaces: u32) -> Vec<u8> {
    let channel = workspace_protocol::channel::current();
    let mut body = session_bus::Body::default();
    body.str(&channel.title)
        .u32(replaces)
        .str("dialog-question")
        .str(item.summary())
        .str(&item.body())
        .strings(&["default", "Open"])
        .string_hints(&[("desktop-entry", &channel.package)])
        .i32(-1);
    body.bytes
}

impl crate::Service {
    /// Connects to the owner's session bus and starts raising notifications. Called once by
    /// the real service; a machine without a session bus stays silent and reports why.
    pub async fn start_notifications(self: &Arc<Self>) {
        let file = self.config.state_dir.join("notifications.json");
        let saved: Option<Value> = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let (signals, mut incoming) = tokio::sync::mpsc::unbounded_channel::<Message>();
        let connected = match session_bus::session_address() {
            None => Err("no_session_bus"),
            Some(path) => tokio::task::spawn_blocking(move || {
                let bus = Bus::connect(&path, move |m| {
                    let _ = signals.send(m);
                })
                .map_err(|_| "session_bus_unreachable")?;
                // Broadcast forms of the server's signals; GNOME sends them to the sender only.
                let mut rule = session_bus::Body::default();
                rule.str(&format!("type='signal',interface='{NAME}',path='{PATH}'"));
                bus.call(
                    "org.freedesktop.DBus",
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "AddMatch",
                    "s",
                    &rule.bytes,
                )
                .map_err(|_| "session_bus_unreachable")?;
                Ok(bus)
            })
            .await
            .unwrap_or(Err("session_bus_unreachable")),
        };
        {
            let mut inner = self
                .notifier
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            inner.state_file = Some(file);
            inner.first = saved.is_none();
            if let Some(saved) = &saved {
                inner.enabled = serde_json::from_value(saved["kinds"].clone()).ok();
                inner.stall_hours = saved["stallHours"]
                    .as_u64()
                    .and_then(|h| u32::try_from(h).ok())
                    .filter(|h| STALL_HOURS.contains(h));
                for seen in saved["seen"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    inner.book.see(seen.to_owned());
                }
            }
            // Without a bus nothing is raised here, but projects are still read for the
            // moments paired phones subscribe to.
            inner.running = true;
            match connected {
                Ok(bus) => {
                    inner.bus = Some(bus);
                    inner.status = "raising";
                }
                Err(code) => {
                    inner.status = code;
                    eprintln!(
                        "{}",
                        json!({"event":"notifications_unavailable","code":code})
                    );
                }
            }
        }
        let service = self.clone();
        tokio::spawn(async move {
            let mut shutdown = service.shutdown.subscribe();
            let mut events = service.events.subscribe();
            let mut stall_tick = tokio::time::interval(STALL_TICK);
            stall_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            for p in service
                .state
                .lock()
                .await
                .projects
                .iter()
                .filter(|p| p.activation.is_some())
            {
                service.notifier.touch(&p.project_id);
            }
            loop {
                if *shutdown.borrow() {
                    break;
                }
                tokio::select! {
                    _ = shutdown.changed() => {}
                    event = events.recv() => match event {
                        Ok(event) => if let Some(project) = changed_project(&event) { service.notifier.touch(project); },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            for p in service.state.lock().await.projects.iter().filter(|p| p.activation.is_some()) {
                                service.notifier.touch(&p.project_id);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                    Some(message) = incoming.recv() => service.bus_message(message).await,
                    _ = stall_tick.tick() => service.stall_check().await,
                    _ = service.notifier.wake.notified() => {
                        tokio::time::sleep(SETTLE).await;
                        let due = std::mem::take(&mut *service.notifier.dirty.lock().unwrap_or_else(|e| e.into_inner()));
                        for project in due {
                            // One project's unreadable state skips that read; nothing is withdrawn on a failed read.
                            let _ = service.notification_pass(&project).await;
                        }
                    }
                }
            }
            // The service is stopping: its notifications could no longer be clicked.
            let (bus, ids) = {
                let mut inner = service
                    .notifier
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let ids: Vec<u32> = std::mem::take(&mut inner.book.shown)
                    .into_values()
                    .map(|s| s.id)
                    .collect();
                (inner.bus.take(), ids)
            };
            if let Some(bus) = bus {
                for id in ids {
                    let mut body = session_bus::Body::default();
                    body.u32(id);
                    let _ =
                        bus.notify_only(NAME, PATH, NAME, "CloseNotification", "u", &body.bytes);
                }
            }
        });
    }

    async fn notification_pass(&self, project: &str) -> io::Result<()> {
        let runtime = self.state.lock().await.native.get(project).cloned();
        let mut tasks = Vec::new();
        let mut decisions = HashMap::new();
        let mut cards = Vec::new();
        let (stalled_on, hours) = {
            let inner = self
                .notifier
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (
                Notifier::enabled(&inner).contains(&Kind::Stalled),
                Notifier::stall_hours(&inner),
            )
        };
        let now = progress::now_ms();
        let mut working = Vec::new();
        let product = if let Some(runtime) = &runtime {
            cards = runtime.host.executor.read(crate::native_owner_actions::open_actions)
                .map_err(|_| crate::failure("storage_unavailable"))?
                .await.map_err(|_| crate::failure("storage_unavailable"))?
                .map_err(|_| crate::failure("storage_unavailable"))?;
            if stalled_on {
                // A store that cannot be read for this skips only the no-progress moments.
                if let Ok(read) = runtime
                    .host
                    .executor
                    .read(move |db| progress::working_tasks(db, now))
                {
                    if let Ok(Ok(found)) = read.await {
                        working = found;
                    }
                }
            }
            let mut cursor = Value::Null;
            for _ in 0..200 {
                let page = self.native_query(json!({"protocol":{"major":2,"mapping":2},"requestId":"notifications",
                    "projectId":project,"operation":"tasks.page","payload":{"target":project,"cursor":cursor,"limit":50}})).await?;
                let value = &page["data"]["value"];
                tasks.extend(value["items"].as_array().cloned().unwrap_or_default());
                cursor = value["nextCursor"].clone();
                if cursor.is_null() {
                    break;
                }
            }
            let asking: Vec<String> = runtime.host.executor.read(|db| {
                let mut statement = db.prepare("SELECT DISTINCT e.task_id FROM pending_questions q JOIN workspace_executions e ON e.runtime_task_id=q.task_id WHERE q.status='open' LIMIT 1000")?;
                let ids = statement.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                Ok(ids)
            }).map_err(|_| crate::failure("storage_unavailable"))?.await.map_err(|_| crate::failure("storage_unavailable"))?
                .map_err(|_| crate::failure("storage_unavailable"))?;
            for task in tasks.iter().filter(|t| {
                t["taskId"]
                    .as_str()
                    .is_some_and(|id| asking.iter().any(|a| a == id))
            }) {
                let (Some(id), Some(execution)) =
                    (task["taskId"].as_str(), task["executionId"].as_str())
                else {
                    continue;
                };
                let page = runtime
                    .decisions(
                        project.to_owned(),
                        json!({"operation":"decisions.list","target":execution,"limit":100}),
                    )
                    .await?;
                decisions.insert(
                    id.to_owned(),
                    page["items"].as_array().cloned().unwrap_or_default(),
                );
            }
            let call = serde_json::from_value(
                json!({"op":"product.board","projectId":project,"params":{}}),
            )
            .map_err(|_| crate::failure("invalid_request"))?;
            let reply = self.machine(call).await?;
            if reply["ok"] != true {
                return Err(crate::failure("machine_refused"));
            }
            reply["result"].clone()
        } else {
            // An inactive or removed project has nothing to announce.
            json!({})
        };
        let mut found = items(project, &tasks, &product, &decisions, &cards);
        if project == crate::native_mcp_signin::MACHINE {
            for (id, name, since) in crate::native_mcp_signin::notices() {
                found.push(Item {
                    kind: Kind::Stop,
                    state: format!("mcp:{id}:{since}"),
                    decision: None,
                    detail: Some(format!("Sign in to {name} to let sessions use it.")),
                    place: Place { project: project.into(), sprint: false, id: id.clone() },
                    label: name,
                });
            }
        }
        let taken: BTreeSet<Place> = found.iter().map(|i| i.place.clone()).collect();
        let stalled = stalls(project, &tasks, &product, &working, &taken, now, hours);
        let (actions, bus) = {
            let mut inner = self
                .notifier
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            inner
                .stall_seen
                .insert(project.to_owned(), stall_keys(&working, now, hours));
            inner.stall_numbers.retain(|p, _| p.project != project);
            for s in stalled {
                inner
                    .stall_numbers
                    .insert(s.item.place.clone(), (s.runtime, s.numbers));
                found.push(s.item);
            }
            // Paired phones read every moment, whatever a desktop window shows or wants.
            let moments = found.iter().map(moment).collect();
            self.notifier.publish_moments(&mut inner, project, moments);
            if inner.bus.is_none() {
                // Nothing is raised, so nothing is marked as announced either.
                return Ok(());
            }
            let now = Instant::now();
            let focus = inner
                .focus
                .as_ref()
                .filter(|(_, until)| *until > now)
                .map(|(p, _)| p.clone());
            let enabled = Notifier::enabled(&inner);
            let prime = inner.first && inner.primed.insert(project.to_owned());
            let actions = inner
                .book
                .plan(project, found, focus.as_ref(), &enabled, prime);
            Notifier::save(&inner);
            (actions, inner.bus.clone())
        };
        let Some(bus) = bus else { return Ok(()) };
        for action in actions {
            let bus = bus.clone();
            match action {
                Action::Withdraw { id } => {
                    let mut body = session_bus::Body::default();
                    body.u32(id);
                    let _ =
                        bus.notify_only(NAME, PATH, NAME, "CloseNotification", "u", &body.bytes);
                }
                Action::Raise { item, replaces } => {
                    let body = notify_body(&item, replaces);
                    let reply = tokio::task::spawn_blocking(move || {
                        bus.call(NAME, PATH, NAME, "Notify", "susssasa{sv}i", &body)
                    })
                    .await;
                    if let Ok(Ok(reply)) = reply {
                        if let Some(id) = reply.body.first().and_then(Arg::u32) {
                            let record = {
                                let mut inner = self
                                    .notifier
                                    .inner
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                inner.server = Some(reply.sender);
                                inner.book.raised(&item, id);
                                (item.kind == Kind::Stalled)
                                    .then(|| inner.stall_numbers.get(&item.place).cloned())
                                    .flatten()
                            };
                            if let (Some((task, numbers)), Some(runtime)) = (record, &runtime) {
                                // The owner was told; the task itself is left exactly as it is.
                                if let Ok(write) = runtime.host.executor.write(move |db| {
                                    cortex_core::lifecycle::event_tx(
                                        db,
                                        &task,
                                        "NO_PROGRESS_NOTIFIED",
                                        numbers,
                                    )?;
                                    Ok(())
                                }) {
                                    let _ = write.await;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The timer's light look at every active project's working tasks: a project is read
    /// again only when a no-progress announcement fell due or went away since its last read.
    async fn stall_check(&self) {
        let (on, hours) = {
            let inner = self
                .notifier
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (
                inner.running && Notifier::enabled(&inner).contains(&Kind::Stalled),
                Notifier::stall_hours(&inner),
            )
        };
        if !on {
            return;
        }
        let runtimes: Vec<_> = self
            .state
            .lock()
            .await
            .native
            .iter()
            .map(|(p, r)| (p.clone(), r.clone()))
            .collect();
        for (project, runtime) in runtimes {
            let now = progress::now_ms();
            let Ok(read) = runtime
                .host
                .executor
                .read(move |db| progress::working_tasks(db, now))
            else {
                continue;
            };
            let Ok(Ok(working)) = read.await else {
                continue;
            };
            let keys = stall_keys(&working, now, hours);
            let changed = {
                let inner = self
                    .notifier
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                inner
                    .stall_seen
                    .get(&project)
                    .map_or(!keys.is_empty(), |k| *k != keys)
            };
            if changed {
                self.notifier.touch(&project);
            }
        }
    }

    async fn bus_message(self: &Arc<Self>, m: Message) {
        if m.kind != 4 || m.interface != NAME || m.path != PATH {
            return;
        }
        let first = m.body.first().and_then(Arg::u32);
        let text = m.body.get(1).and_then(Arg::str).unwrap_or("").to_owned();
        let click = {
            let mut inner = self
                .notifier
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // Only the server that took this service's notifications speaks for them.
            if inner.server.as_deref().is_some_and(|s| s != m.sender) {
                return;
            }
            let Some(id) = first else { return };
            match m.member.as_str() {
                "ActivationToken" => {
                    inner.book.token(id, &text);
                    None
                }
                "NotificationClosed" => {
                    inner.book.closed(id);
                    None
                }
                "ActionInvoked" => inner.book.invoked(id, &text).map(|(target, token)| {
                    let nonce = inner.handoff.clicked(target.clone(), token, Instant::now());
                    (nonce, target)
                }),
                _ => None,
            }
        };
        let Some((nonce, target)) = click else { return };
        let _ = self
            .events
            .send(json!({"type":"notification_open","nonce":nonce,"target":target}));
        let service = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(CLAIM).await;
            let token = service
                .notifier
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .handoff
                .unclaimed(&nonce, Instant::now());
            if let Some(token) = token {
                let argv = launch_command(token.as_deref());
                let started = tokio::process::Command::new(&argv[0])
                    .args(&argv[1..])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await;
                if !started.is_ok_and(|s| s.success()) {
                    eprintln!("{}", json!({"event":"notification_launch_failed"}));
                }
            }
        });
    }

    /// `notifications.subscribe`: the current moments snapshot at once, then a new one after
    /// every change, as `notification_moments` service events. A frame from the caller, its
    /// close or the service stopping ends it.
    pub(crate) async fn serve_moments(
        &self,
        mut reader: tokio::net::unix::OwnedReadHalf,
        mut writer: tokio::net::unix::OwnedWriteHalf,
        mut moments: tokio::sync::watch::Receiver<Value>,
    ) -> io::Result<()> {
        let mut shutdown = self.shutdown.subscribe();
        let frame = crate::read_frame(&mut reader);
        tokio::pin!(frame);
        loop {
            let mut event = {
                let current = moments.borrow_and_update();
                if current.is_object() {
                    current.clone()
                } else {
                    json!({"moments": []})
                }
            };
            event["type"] = json!("notification_moments");
            tokio::time::timeout(
                self.config.request_timeout,
                crate::write_frame(
                    &mut writer,
                    &json!({"jsonrpc":"2.0","method":"service.event","params":event}),
                ),
            )
            .await
            .map_err(|_| crate::failure("client_write_timeout"))??;
            tokio::select! {
                changed = moments.changed() => if changed.is_err() { return Ok(()) },
                _ = &mut frame => return Ok(()),
                _ = shutdown.changed() => return Ok(()),
            }
        }
    }

    /// `notifications.report`, `notifications.opened` and `notifications.take`.
    pub(crate) async fn notifications_call(
        &self,
        method: &str,
        params: Value,
    ) -> io::Result<Value> {
        let raising = self.notifier.raising();
        if method == "notifications.report" {
            let (place, kinds, stall) = report_with_threshold(params)?;
            let (mut projects, changed) = {
                let mut inner = self
                    .notifier
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let before = inner.focus.as_ref().map(|(p, _)| p.project.clone());
                let after = place.as_ref().map(|p| p.project.clone());
                inner.focus = place.map(|p| (p, Instant::now() + FOCUS_TTL));
                let changed = inner.enabled.as_ref() != Some(&kinds)
                    || stall.is_some_and(|h| inner.stall_hours != Some(h));
                if changed {
                    inner.enabled = Some(kinds);
                    if stall.is_some() {
                        inner.stall_hours = stall;
                    }
                    Notifier::save(&inner);
                }
                (
                    before
                        .into_iter()
                        .chain(after)
                        .collect::<BTreeSet<String>>(),
                    changed,
                )
            };
            // What is on view now, or a kind just turned off, may withdraw a notification.
            if changed {
                projects.extend(
                    self.state
                        .lock()
                        .await
                        .projects
                        .iter()
                        .map(|p| p.project_id.clone()),
                );
            }
            for p in projects {
                self.notifier.touch(&p);
            }
            return Ok(json!({"raising":raising}));
        }
        let mut inner = self
            .notifier
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match method {
            "notifications.opened" => {
                let nonce = params
                    .get("nonce")
                    .and_then(Value::as_str)
                    .filter(|n| id_ok(n) && params.as_object().is_some_and(|m| m.len() == 1))
                    .ok_or_else(|| crate::failure("invalid_params"))?;
                Ok(match inner.handoff.opened(nonce) {
                    Some(token) => json!({"claimed":true,"token":token}),
                    None => json!({"claimed":false,"token":null}),
                })
            }
            "notifications.take" if params == json!({}) => {
                Ok(json!({"target":inner.handoff.take(Instant::now())}))
            }
            _ => Err(crate::failure("invalid_params")),
        }
    }
}

#[cfg(test)]
#[path = "notifier_tests.rs"]
mod tests;
