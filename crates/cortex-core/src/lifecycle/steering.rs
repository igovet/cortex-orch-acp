//! Owner messages to a task that is working: recorded once, never interrupting the
//! running work, and handed to the coordinator at its next turn boundary.
//! Python oracle: orchestration/policy.py (`parse_denials`, `steer`,
//! `apply_pending_steering`).
//!
//! A message recorded while workers, review, verification or a coordinator turn run
//! is a `USER_STEERING` event with `"delivery":"boundary"`. It stays pending until the
//! runtime applies it at a safe boundary (`STEERING_APPLIED`, Python's event name):
//! the task is in PLANNING, PLAN_READY, ANSWER_READY or RECONCILING and no node of the
//! current revision runs. Applying it returns the task to PLANNING (a plan that has
//! not started is discarded, an unconfirmed synthesis is cleared). The next coordinator
//! turn receives it (`STEERING_HANDED`), and once the engine's reply to that prompt is
//! observed it is delivered (`STEERING_DELIVERED`); a turn that never got that far hands
//! it again. All records are keyed by the message's request id.
//! A message to a task whose coordinator waits for the owner, is held for recovery,
//! paused or finished keeps its restart delivery (`"delivery":"restart"`): applied at
//! admission and handed to the relaunched coordinator the same way.
use super::{domain, event_tx};
use crate::{error::refuse, CoreError, CoreResult, TaskState};
use rusqlite::{Connection, Transaction};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub const BOUNDARY: &str = "boundary";
pub const RESTART: &str = "restart";
/// Python `project_paths.PRIVATE`: never an owner denial path either.
const PRIVATE: [&str; 5] = [
    ".cortex",
    ".cortex-dev",
    ".cortex-dev-owner.json",
    ".cortex-workspace-dev.json",
    ".git",
];
/// Python `str.isspace` / `re` `\s` for text: Unicode white space plus the
/// information separators U+001C..U+001F.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}
/// Python `policy.parse_denials`: only an explicit `deny-write: <path>` directive
/// (ASCII case-insensitive; the path bare, single-, double- or backtick-quoted) is a
/// denial. Natural language creates none. A value with `*`, `?`, `[` or `]` is
/// skipped. The value is stripped of surrounding white space and then of trailing
/// `.`, `,` and `;`, and stored as its normalized path parts (`./a//b/` is `a/b`); a
/// value naming the project root only (`.`) is skipped. A path outside the project
/// (absolute, `..`, `\`, NUL) or a private path (`.git`, `.cortex`, ...) refuses the
/// whole message with `gateway_path_denied`, as Python's `_parts` does.
pub fn parse_denials(text: &str) -> CoreResult<Vec<String>> {
    parse_directive(text, "deny-write")
}
/// The owner's lift of a denial: `allow-write: <path>`, parsed exactly like
/// `deny-write:` (same forms, normalization and refusals). It removes the owner's own
/// recorded denial of exactly that normalized path and grants nothing else; a path
/// that is not denied is left as it is. Natural language lifts nothing.
pub fn parse_lifts(text: &str) -> CoreResult<Vec<String>> {
    parse_directive(text, "allow-write")
}
fn parse_directive(text: &str, key: &str) -> CoreResult<Vec<String>> {
    let key: Vec<char> = key.chars().collect();
    let chars: Vec<char> = text.chars().collect();
    let mut found = BTreeSet::new();
    let mut i = 0;
    while i + key.len() <= chars.len() {
        if !chars[i..i + key.len()]
            .iter()
            .zip(&key)
            .all(|(c, k)| c.to_ascii_lowercase() == *k)
        {
            i += 1;
            continue;
        }
        let mut j = i + key.len();
        while j < chars.len() && py_space(chars[j]) {
            j += 1;
        }
        if chars.get(j) != Some(&':') {
            i += 1;
            continue;
        }
        j += 1;
        while j < chars.len() && py_space(chars[j]) {
            j += 1;
        }
        // `"([^"]+)"|'([^']+)'|`([^`]+)`|(\S+)`, tried in that order at `j`.
        let mut matched: Option<(String, usize)> = None;
        if let Some(&first) = chars.get(j) {
            if ['"', '\'', '`'].contains(&first) {
                if let Some(len) = chars[j + 1..].iter().position(|c| *c == first) {
                    if len > 0 {
                        matched = Some((chars[j + 1..j + 1 + len].iter().collect(), j + len + 2));
                    }
                }
            }
            if matched.is_none() && !py_space(first) {
                let mut k = j;
                while k < chars.len() && !py_space(chars[k]) {
                    k += 1;
                }
                matched = Some((chars[j..k].iter().collect(), k));
            }
        }
        let Some((raw, end)) = matched else {
            i += 1;
            continue;
        };
        i = end;
        let value = raw.trim_matches(py_space).trim_end_matches(['.', ',', ';']);
        if value.is_empty() || value.contains(['*', '?', '[', ']']) {
            continue;
        }
        let parts: Vec<&str> = value
            .split('/')
            .filter(|p| !p.is_empty() && *p != ".")
            .collect();
        if value.starts_with('/')
            || value.contains(['\\', '\0'])
            || parts.iter().any(|p| *p == ".." || PRIVATE.contains(p))
        {
            return refuse("gateway_path_denied");
        }
        if !parts.is_empty() {
            found.insert(parts.join("/"));
        }
    }
    Ok(found.into_iter().collect())
}
/// One write denial the owner has in force: its path and when it was set.
#[derive(Clone, Debug, PartialEq)]
pub struct Denial {
    pub path: String,
    pub set_at: String,
}
/// The owner's write denials in force for this task, oldest first. Messages add
/// (`deny_write`) and lift (`allow_write`) them in the order recorded; the desktop's
/// Remove control lifts one (`OWNER_WRITE_ALLOWED`). A lift removes only an exact
/// recorded path. A denial an earlier runtime stored (wildcards included) is kept.
pub fn denial_records(db: &Connection, task: &str) -> CoreResult<Vec<Denial>> {
    let rows = db
        .prepare(
            "SELECT type,payload_json,created_at FROM runtime_events WHERE task_id=?
            AND type IN ('USER_STEERING','OWNER_WRITE_ALLOWED') ORDER BY id",
        )?
        .query_map([task], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut held: Vec<Denial> = Vec::new();
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str().map(str::to_owned))
            .collect()
    };
    for (kind, raw, at) in rows {
        let value: Value = serde_json::from_str(&raw)?;
        let lifted = if kind == "OWNER_WRITE_ALLOWED" {
            strings(&json!([value["path"]]))
        } else {
            for path in strings(&value["deny_write"]) {
                if !held.iter().any(|d| d.path == path) {
                    held.push(Denial {
                        path,
                        set_at: at.clone(),
                    });
                }
            }
            strings(&value["allow_write"])
        };
        held.retain(|d| !lifted.contains(&d.path));
    }
    Ok(held)
}
/// The paths of `denial_records`, sorted: what every write check enforces.
pub fn denials(db: &Connection, task: &str) -> CoreResult<Vec<String>> {
    let mut paths: Vec<String> = denial_records(db, task)?
        .into_iter()
        .map(|d| d.path)
        .collect();
    paths.sort();
    Ok(paths)
}
/// The project's own agent and process-rule files. A task never writes them unless
/// the owner asks: its goal or one of the owner's messages names the path (see
/// [`names_path`]), or the owner lifts it with `allow-write: <path>` or the desktop's
/// Remove control. Directories cover everything below them. These protections are
/// enforced at every write (project file tools and command sandboxes, through
/// [`write_denials`]); plan and claim scope checks use the owner's own [`denials`]
/// only, so a broad scope such as `.` or `docs` is never refused for them.
pub const RULE_FILES: [&str; 7] = [
    ".agents",
    ".claude",
    "AGENTS.md",
    "CLAUDE.local.md",
    "CLAUDE.md",
    "GEMINI.md",
    "docs/project/verification.md",
];
/// The explicit skill-management command: its whole purpose is editing project
/// skills, so it names `.agents` and `.claude`.
const SKILL_COMMAND: &str = "/skill-management";
/// Whether `text` names the project-relative `path` explicitly: the exact path
/// (ASCII case-insensitive), optionally `./`-prefixed or followed by `/...`, as a
/// whole token. A longer name (`AGENTS.md.bak`, `MY_AGENTS.md`), a nested file
/// (`crates/x/AGENTS.md`) or a home path (`~/.claude`) does not name it.
pub fn names_path(text: &str, path: &str) -> bool {
    let hay = text.as_bytes();
    let needle = path.as_bytes();
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    for start in 0..=hay.len() - needle.len() {
        if !hay[start..start + needle.len()].eq_ignore_ascii_case(needle) {
            continue;
        }
        let before_ok = match start {
            0 => true,
            _ => {
                let b = hay[start - 1];
                if b == b'/' {
                    // `./AGENTS.md` at a token start; any other `/` is a longer path.
                    start >= 2
                        && hay[start - 2] == b'.'
                        && (start == 2
                            || !(word(hay[start - 3]) || b"./~".contains(&hay[start - 3])))
                } else {
                    !(word(b) || b == b'.' || b == b'~')
                }
            }
        };
        let end = start + needle.len();
        let after_ok = match hay.get(end) {
            None => true,
            Some(&b) if word(b) => false,
            // A sentence's full stop ends the token; `.bak` does not.
            Some(&b'.') => hay.get(end + 1).is_none_or(|n| !word(*n)),
            Some(_) => true,
        };
        if before_ok && after_ok {
            return true;
        }
    }
    false
}
/// The [`RULE_FILES`] still protected for this task, sorted. A task without a
/// runtime row has none. The owner's texts (the goal and every recorded message)
/// that name a path, an exact `allow-write:` lift or a desktop Remove of the path
/// release it for this task.
pub fn rule_file_protections(db: &Connection, task: &str) -> CoreResult<Vec<String>> {
    use rusqlite::OptionalExtension;
    let Some(goal) = db
        .query_row(
            "SELECT goal FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(Vec::new());
    };
    let rows = db
        .prepare(
            "SELECT type,payload_json FROM runtime_events WHERE task_id=?
            AND type IN ('USER_STEERING','OWNER_WRITE_ALLOWED') ORDER BY id",
        )?
        .query_map([task], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut texts = vec![goal];
    let mut lifted = BTreeSet::new();
    for (kind, raw) in rows {
        let value: Value = serde_json::from_str(&raw)?;
        if kind == "OWNER_WRITE_ALLOWED" {
            if let Some(path) = value["path"].as_str() {
                lifted.insert(path.to_owned());
            }
            continue;
        }
        if let Some(text) = value["text"].as_str() {
            texts.push(text.to_owned());
        }
        for path in value["allow_write"].as_array().into_iter().flatten() {
            if let Some(path) = path.as_str() {
                lifted.insert(path.to_owned());
            }
        }
    }
    let skills = texts.iter().any(|t| t.contains(SKILL_COMMAND));
    Ok(RULE_FILES
        .iter()
        .filter(|path| !lifted.contains(**path))
        .filter(|path| !(skills && [".agents", ".claude"].contains(*path)))
        .filter(|path| !texts.iter().any(|t| names_path(t, path)))
        .map(|path| (*path).to_owned())
        .collect())
}
/// Every path a write of this task must avoid: the owner's [`denials`] and the
/// [`rule_file_protections`], sorted and unique. Write-time checks use this;
/// plan/claim scope checks use [`denials`].
pub fn write_denials(db: &Connection, task: &str) -> CoreResult<Vec<String>> {
    let mut paths: BTreeSet<String> = denials(db, task)?.into_iter().collect();
    paths.extend(rule_file_protections(db, task)?);
    Ok(paths.into_iter().collect())
}
/// The desktop's Remove control: lift exactly one denial in force (an owner denial
/// or a default rule-file protection). Anything else is refused
/// (`write_denial_unknown`); it never grants a path that was not denied.
pub fn lift_tx(db: &Transaction<'_>, task: &str, path: &str, request: &str) -> CoreResult<()> {
    if !write_denials(db, task)?.iter().any(|p| p == path) {
        return refuse("write_denial_unknown");
    }
    event_tx(
        db,
        task,
        "OWNER_WRITE_ALLOWED",
        json!({"path":path,"request":request}),
    )?;
    Ok(())
}
/// Whether an owner message admitted now waits for the coordinator's next boundary
/// instead of restarting it: the task works and the coordinator itself does not wait
/// for the owner. A worker's open question stays open and answerable meanwhile.
pub fn boundary_tx(db: &Transaction<'_>, task: &str, state: TaskState) -> CoreResult<bool> {
    if !matches!(
        state,
        TaskState::Planning
            | TaskState::PlanReady
            | TaskState::Executing
            | TaskState::Reviewing
            | TaskState::Verifying
            | TaskState::Reconciling
            | TaskState::AnswerReady
    ) {
        return Ok(false);
    }
    // A task-level question (the coordinator's, or a worker's without its own wait)
    // is the coordinator waiting for the owner: a message there keeps the restart.
    let asked: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pending_questions WHERE task_id=? AND status='open' AND assignment_ref IS NULL)",
        [task],
        |r| r.get(0),
    )?;
    Ok(!asked)
}
/// The delivery recorded for one owner message (`boundary` or `restart`), if any.
pub fn delivery(db: &Connection, task: &str, request: &str) -> CoreResult<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(db
        .query_row(
            "SELECT json_extract(payload_json,'$.delivery') FROM runtime_events WHERE task_id=? AND type='USER_STEERING'
             AND json_extract(payload_json,'$.request')=? ORDER BY id DESC LIMIT 1",
            (task, request),
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}
const PENDING: &str = "SELECT e.payload_json FROM runtime_events e WHERE e.task_id=?1 AND e.type='USER_STEERING'
    AND json_extract(e.payload_json,'$.delivery')='boundary'
    AND NOT EXISTS(SELECT 1 FROM runtime_events a WHERE a.task_id=e.task_id AND a.type='STEERING_APPLIED'
        AND json_extract(a.payload_json,'$.request')=json_extract(e.payload_json,'$.request'))
    ORDER BY e.id";
fn rows(db: &Connection, sql: &str, task: &str) -> CoreResult<Vec<Value>> {
    let raw = db
        .prepare(sql)?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    raw.iter()
        .map(|s| serde_json::from_str(s).map_err(CoreError::from))
        .collect()
}
/// Owner messages recorded for boundary delivery and not applied yet, oldest first.
pub fn pending(db: &Connection, task: &str) -> CoreResult<Vec<Value>> {
    rows(db, PENDING, task)
}
/// Python `apply_pending_steering`: at a safe boundary, apply every pending owner
/// message and return the task to PLANNING in the same transaction. Outside a
/// boundary, or with a node of the current revision still running, nothing changes.
pub fn apply_pending_tx(db: &Transaction<'_>, task: &str) -> CoreResult<bool> {
    let (state, revision) = domain::task_tx(db, task)?;
    if !matches!(
        state,
        TaskState::Planning
            | TaskState::PlanReady
            | TaskState::AnswerReady
            | TaskState::Reconciling
    ) {
        return Ok(false);
    }
    let pending = pending(db, task)?;
    if pending.is_empty() {
        return Ok(false);
    }
    let running: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND revision=? AND status='running')
         OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND kind='worker' AND state!='released')",
        (task, revision, task),
        |r| r.get(0),
    )?;
    if running {
        return Ok(false);
    }
    if state != TaskState::Planning {
        if state == TaskState::PlanReady {
            // The plan has not started: no node of it ran, so nothing is lost.
            db.execute(
                "UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND revision=? AND status='pending'",
                (task, revision),
            )?;
        }
        domain::state_tx(db, task, TaskState::Recovering, None)?;
        db.execute(
            "UPDATE runtime_tasks SET synthesis=NULL WHERE task_id=?",
            [task],
        )?;
        domain::settle_recovery_tx(db, task, TaskState::Planning)?;
    }
    for message in &pending {
        event_tx(
            db,
            task,
            "STEERING_APPLIED",
            json!({"request":message["request"],"state":state,"revision":revision}),
        )?;
    }
    Ok(true)
}
/// Applied owner messages (boundary or restart) whose delivery the engine has not
/// confirmed yet, oldest first: never handed to a coordinator turn, or handed to a
/// turn whose prompt reply was never observed.
const UNDELIVERED: &str = "SELECT json_set(e.payload_json,'$.repeated',json(CASE WHEN EXISTS(SELECT 1 FROM runtime_events h
        WHERE h.task_id=e.task_id AND h.type='STEERING_HANDED'
        AND json_extract(h.payload_json,'$.request')=json_extract(e.payload_json,'$.request')) THEN 'true' ELSE 'false' END))
    FROM runtime_events e WHERE e.task_id=?1 AND e.type='USER_STEERING'
    AND json_extract(e.payload_json,'$.delivery') IN ('boundary','restart')
    AND EXISTS(SELECT 1 FROM runtime_events a WHERE a.task_id=e.task_id AND a.type='STEERING_APPLIED'
        AND json_extract(a.payload_json,'$.request')=json_extract(e.payload_json,'$.request'))
    AND NOT EXISTS(SELECT 1 FROM runtime_events d WHERE d.task_id=e.task_id AND d.type='STEERING_DELIVERED'
        AND json_extract(d.payload_json,'$.request')=json_extract(e.payload_json,'$.request'))
    ORDER BY e.id";
/// Applied owner messages the coordinator has not been confirmed to receive, oldest first.
pub fn undelivered(db: &Connection, task: &str) -> CoreResult<Vec<Value>> {
    rows(db, UNDELIVERED, task)
}
/// Confirm handed messages: the engine's reply to a coordinator prompt of this task
/// (`NATIVE_TURN_SETTLED` of a coordinator channel) was observed after the latest
/// hand-over, so the first prompt of that turn, which carried the messages, reached
/// the engine. Each confirmed message gets one `STEERING_DELIVERED`.
pub fn confirm_tx(db: &Transaction<'_>, task: &str) -> CoreResult<usize> {
    let handed = db
        .prepare("SELECT json_extract(h.payload_json,'$.request'),max(h.id) FROM runtime_events h
            WHERE h.task_id=?1 AND h.type='STEERING_HANDED' AND NOT EXISTS(SELECT 1 FROM runtime_events d
                WHERE d.task_id=h.task_id AND d.type='STEERING_DELIVERED'
                AND json_extract(d.payload_json,'$.request')=json_extract(h.payload_json,'$.request'))
            GROUP BY json_extract(h.payload_json,'$.request') ORDER BY min(h.id)")?
        .query_map([task], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut confirmed = 0;
    for (request, at) in handed {
        let replied: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_events s JOIN trusted_channels c
                ON c.channel_id=json_extract(s.payload_json,'$.channel_id') AND c.role='coordinator'
                WHERE s.task_id=? AND s.type='NATIVE_TURN_SETTLED' AND s.id>?)",
            (task, at),
            |r| r.get(0),
        )?;
        if replied {
            event_tx(db, task, "STEERING_DELIVERED", json!({"request":request}))?;
            confirmed += 1;
        }
    }
    Ok(confirmed)
}
/// Immediately before a coordinator turn: confirm earlier hand-overs, apply pending
/// messages if the task is at a boundary, then hand every applied, unconfirmed message
/// to this turn (`STEERING_HANDED`) and return them, oldest first. A message handed to
/// a turn whose prompt was never confirmed is handed again (`repeated`), so a service
/// death between hand-over and the engine's reply never loses it.
pub fn deliver_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Vec<Value>> {
    confirm_tx(db, task)?;
    apply_pending_tx(db, task)?;
    let messages = rows(db, UNDELIVERED, task)?;
    for message in &messages {
        event_tx(
            db,
            task,
            "STEERING_HANDED",
            json!({"request":message["request"]}),
        )?;
    }
    Ok(messages)
}
/// Every owner message of the task with its delivery state, oldest first:
/// `recorded` (waiting for the coordinator's next step) or `delivered` (with the time
/// the engine's reply to the prompt carrying it was observed).
pub fn message_states(db: &Connection, task: &str) -> CoreResult<Vec<Value>> {
    let rows = db
        .prepare("SELECT e.payload_json,e.created_at,(SELECT min(d.created_at) FROM runtime_events d WHERE d.task_id=e.task_id
                AND d.type='STEERING_DELIVERED' AND json_extract(d.payload_json,'$.request')=json_extract(e.payload_json,'$.request'))
            FROM runtime_events e WHERE e.task_id=? AND e.type='USER_STEERING'
            AND json_extract(e.payload_json,'$.delivery') IN ('boundary','restart') ORDER BY e.id DESC LIMIT 50")?
        .query_map([task], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for (raw, at, delivered) in rows.into_iter().rev() {
        let v: Value = serde_json::from_str(&raw)?;
        out.push(json!({"request":v["request"],"text":v["text"],"at":at,
            "delivery":if delivered.is_some() {"delivered"} else {"recorded"},"deliveredAt":delivered}));
    }
    Ok(out)
}
const BOUNDARY_HEADER: &str = "Owner messages received while the work ran";
/// The coordinator prompt section for handed owner messages. Attached files reach the
/// engine as prompt content blocks with every coordinator prompt; they are named here.
pub fn guidance_prompt(messages: &[Value]) -> String {
    let boundary = messages.iter().any(|m| m["delivery"] == BOUNDARY);
    let mut out = if boundary {
        format!("{BOUNDARY_HEADER} (oldest first; they are also retained in the capsule). \
Running work was not stopped for them. Apply them now: if one changes the work, submit a new plan through plan_submit; \
an explicit `deny-write:` path is already enforced by the runtime.")
    } else {
        String::from("Owner input (oldest first; also retained in the capsule):")
    };
    if messages.iter().any(|m| m["repeated"] == true) {
        out.push_str(" A message marked (repeated) was handed to an earlier turn whose reply was never observed; you may have seen it already.");
    }
    for (n, message) in messages.iter().enumerate() {
        out.push_str(&format!(
            "\n\n{}. {}{}",
            n + 1,
            if message["repeated"] == true {
                "(repeated) "
            } else {
                ""
            },
            message["text"].as_str().unwrap_or_default()
        ));
        let names: Vec<&str> = message["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["name"].as_str())
            .collect();
        if !names.is_empty() {
            out.push_str(&format!("\n   Attached: {}", names.join(", ")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{names_path, parse_denials};

    #[test]
    fn a_rule_file_is_named_only_as_a_whole_project_path() {
        for text in [
            "Update AGENTS.md with the new rule",
            "fix agents.md.",
            "edit `./AGENTS.md`",
            "AGENTS.md",
            "(AGENTS.md)",
        ] {
            assert!(names_path(text, "AGENTS.md"), "{text}");
        }
        for text in [
            "update the rules",
            "MY_AGENTS.md",
            "AGENTS.md.bak",
            "crates/x/AGENTS.md",
            "AGENTS.mdx",
        ] {
            assert!(!names_path(text, "AGENTS.md"), "{text}");
        }
        assert!(names_path("add .claude/skills/x", ".claude"));
        assert!(!names_path("see ~/.claude/settings.json", ".claude"));
        assert!(!names_path("my.claude file", ".claude"));
        assert!(names_path(
            "rewrite docs/project/verification.md now",
            "docs/project/verification.md"
        ));
        assert!(!names_path(
            "verification.md",
            "docs/project/verification.md"
        ));
    }

    #[test]
    fn explicit_directives_only_in_every_quoted_form() {
        assert_eq!(
            parse_denials("Please deny-write: src/a.rs, DENY-WRITE : 'b dir/x.txt' and deny-write:`c.md`; deny-write:\"d\".").unwrap(),
            ["b dir/x.txt", "c.md", "d", "src/a.rs"]
        );
        assert!(parse_denials("do not modify secrets/key.txt")
            .unwrap()
            .is_empty());
        assert!(parse_denials("deny-write secrets/key.txt")
            .unwrap()
            .is_empty());
        assert!(parse_denials("deny-write:   ").unwrap().is_empty());
    }

    #[test]
    fn normalized_and_never_a_wildcard() {
        assert_eq!(
            parse_denials(
                "deny-write: ./secrets//key.txt deny-write: secrets/key.txt deny-write: docs/./x/"
            )
            .unwrap(),
            ["docs/x", "secrets/key.txt"]
        );
        assert!(
            parse_denials("deny-write: src/*.rs deny-write: a?b deny-write: [x]")
                .unwrap()
                .is_empty()
        );
        assert!(parse_denials("deny-write: . deny-write: ./")
            .unwrap()
            .is_empty());
        // An unclosed quote is a bare value, quote included (the regex's last branch).
        assert_eq!(parse_denials("deny-write: \"open").unwrap(), ["\"open"]);
    }

    #[test]
    fn a_path_outside_the_project_or_private_refuses_the_message() {
        for text in [
            "deny-write: /etc/passwd",
            "deny-write: ../x",
            "deny-write: a/../../x",
            "deny-write: .git/config",
            "deny-write: .cortex",
            "deny-write: a\\b",
        ] {
            assert_eq!(
                parse_denials(text).unwrap_err().code(),
                "gateway_path_denied",
                "{text}"
            );
        }
    }
}
