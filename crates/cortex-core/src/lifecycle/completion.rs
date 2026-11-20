//! Runtime-only certification. Python oracle: orchestration/state.py:246-298,
//! 1861-1946,1993-2018,2104-2150,2244-2297. No model-provided pass flag
//! can complete a task. Observations must come from the daemon's effect ports.
use super::{
    domain, event_tx, plan, request_tx, waits, MAX_REVIEW_CYCLES, MAX_VERIFICATION_REPAIRS,
};
use crate::{
    error::refuse,
    ownership::{self, OwnerEpoch},
    ports::{CommandObservation, ProcessObservation},
    CoreError, CoreResult, TaskState,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Non-deserializable effect snapshot, collected outside the SQLite transaction.
/// The host must use safe artifact reads and fresh project/process observations.
#[derive(Default)]
pub struct EvidenceSnapshot {
    pub project_digest: Option<String>,
    /// Metadata digest (type, mode, size, mtime, link target) of Git-ignored files.
    /// Absent when they could not be observed; a review waiver then cannot be granted.
    pub ignored_digest: Option<String>,
    pub head: Option<String>,
    /// A deliverable entry (a file past a safety bound, a special file) was recorded by
    /// size/mtime/mode only; restoring the mtime would defeat it, so no review waiver.
    /// Ignored files are not deliverable: only `ignored_digest` equality governs them.
    pub metadata_only: bool,
    /// A per-file or total content limit was reached during the observation.
    pub cap_reached: bool,
    /// The observation could not cover everything (e.g. a nested checkout); no waiver.
    pub incomplete: bool,
    pub artifacts: BTreeMap<String, String>,
    pub workers: BTreeMap<String, ProcessObservation>,
    /// The current state of each file a review binds instead of the whole checkout
    /// (`bound_files_tx`): its content hash and mode, link target, `absent`, or its
    /// metadata past the host's byte bound. A bound file missing here makes that
    /// binding unavailable, never satisfied.
    pub task_files: BTreeMap<String, String>,
}
fn digest(snapshot: &EvidenceSnapshot) -> CoreResult<&str> {
    snapshot
        .project_digest
        .as_deref()
        .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or(CoreError::Refused("project_revision_unavailable"))
}
/// The task files one review binds, from the binding the host recorded with its
/// packet (`reconciliation::review_packet_tx`), or `None` when it binds the whole
/// checkout: attribution was incomplete, the set was too large, or the packet
/// predates task-file bindings.
pub fn review_bound_files(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
) -> CoreResult<Option<Vec<String>>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT json_extract(payload_json,'$.binding') FROM runtime_events WHERE task_id=? AND type='REVIEW_PACKET'
             AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(raw) = raw else { return Ok(None) };
    let binding: Value = serde_json::from_str(&raw)?;
    if binding["kind"] != "task_files" {
        return Ok(None);
    }
    Ok(Some(
        serde_json::from_value(binding["paths"].clone())
            .map_err(|_| CoreError::Refused("review_revision_unavailable"))?,
    ))
}
/// The digest of the bound task files' current states. Domain-separated from the
/// checkout digest, so the two never compare equal.
pub fn task_files_digest(paths: &[String], snapshot: &EvidenceSnapshot) -> CoreResult<String> {
    let mut input = b"cortex-review-task-files-v1\n".to_vec();
    for path in paths {
        let state = snapshot
            .task_files
            .get(path)
            .ok_or(CoreError::Refused("review_revision_unavailable"))?;
        input.extend_from_slice(path.as_bytes());
        input.push(0);
        input.extend_from_slice(state.as_bytes());
        input.push(b'\n');
    }
    Ok(crate::digest(&input))
}
/// What one review binds now: its task files' digest, or the whole checkout's.
pub(super) fn review_digest_tx(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<(String, &'static str)> {
    match review_bound_files(db, task, assignment)? {
        Some(paths) => Ok((task_files_digest(&paths, snapshot)?, "task_files")),
        None => Ok((
            snapshot
                .project_digest
                .clone()
                .filter(|s| s.len() == 64)
                .ok_or(CoreError::Refused("project_revision_unavailable"))?,
            "checkout",
        )),
    }
}
/// Every path a review of the task's current revision binds by its bytes: the host
/// identifies exactly these in an `EvidenceSnapshot` (`task_files`).
pub fn bound_files_tx(db: &rusqlite::Connection, task: &str) -> CoreResult<BTreeSet<String>> {
    let assignments = db
        .prepare(
            "SELECT n.assignment_ref FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
             WHERE n.task_id=? AND n.revision=t.revision AND n.stage='review' AND n.assignment_ref IS NOT NULL",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut paths = BTreeSet::new();
    for assignment in assignments {
        paths.extend(
            review_bound_files(db, task, &assignment)?
                .into_iter()
                .flatten(),
        );
    }
    Ok(paths)
}
/// The current digest the task's review binding is compared with: that of the
/// latest review of the revision (its task files or the whole checkout), or the
/// whole checkout for a review waiver.
fn bound_digest_tx(
    db: &rusqlite::Connection,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<String> {
    let review: Option<Option<String>> = db
        .query_row(
            "SELECT n.assignment_ref FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
             WHERE n.task_id=? AND n.revision=t.revision AND n.stage='review' ORDER BY n.rowid DESC LIMIT 1",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    match review.flatten() {
        Some(assignment) => Ok(review_digest_tx(db, task, &assignment, snapshot)?.0),
        None => Ok(digest(snapshot)?.to_owned()),
    }
}
/// The task's current review-binding digest for the host (verification drift
/// checks); see `bound_digest_tx`.
pub fn current_binding_digest(
    db: &rusqlite::Connection,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<String> {
    bound_digest_tx(db, task, snapshot)
}
fn base(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}
fn assignment(s: &str) -> bool {
    s.split_once('=').is_some_and(|(k, _)| name(k))
}
fn name(k: &str) -> bool {
    !k.is_empty()
        && k.bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit())
}
/// The command left after leading `NAME=value` assignments and the launchers
/// `env`, `timeout`, `nice`, `nohup`, `command`, `exec` and `time`.
fn command_words(argv: &[String]) -> &[String] {
    let mut w = argv;
    loop {
        let Some(head) = w.first() else { return w };
        if assignment(head) {
            w = &w[1..];
            continue;
        }
        match base(head) {
            "env" => {
                w = &w[1..];
                while let Some(s) = w.first() {
                    if s.starts_with('-') || assignment(s) {
                        let n = if ["-u", "--unset", "-C", "--chdir"].contains(&s.as_str()) {
                            2
                        } else {
                            1
                        };
                        w = w.get(n..).unwrap_or(&[]);
                    } else {
                        break;
                    }
                }
            }
            "timeout" | "nice" => {
                let timeout = base(head) == "timeout";
                w = &w[1..];
                while let Some(s) = w.first().filter(|s| s.starts_with('-')) {
                    let n = if ["-s", "--signal", "-k", "--kill-after", "-n", "--adjustment"]
                        .contains(&s.as_str())
                    {
                        2
                    } else {
                        1
                    };
                    w = w.get(n..).unwrap_or(&[]);
                }
                if timeout {
                    w = w.get(1..).unwrap_or(&[]);
                }
            }
            "nohup" | "command" | "exec" | "time" => {
                w = &w[1..];
                while w.first().is_some_and(|s| s.starts_with('-')) {
                    w = &w[1..];
                }
            }
            _ => break,
        }
    }
    w
}
/// One word of an inline shell script, within the small literal grammar
/// `shell_script` accepts: `raw` as written, `text` with its quotes removed
/// (expansions stay literal text).
struct Word {
    raw: String,
    text: String,
}
/// Split an inline script into simple commands joined by `;` or `&&`, or `None`
/// for anything outside a deliberately small grammar: words of unquoted letters,
/// digits and `_./:,+@%=-~*?^`, single-quoted text, double-quoted text, and
/// plain `$NAME`/`${NAME}` expansions. Every other operator (pipes, `||`, a single
/// `&`, redirections, grouping, subshells, here-documents, comments, newlines),
/// command or process substitution, backslash, backtick, `$'...'` and any other
/// `$` form is refused, so the exit status of the script is the status of its
/// last command or of a failing `&&` prefix.
fn shell_script(script: &str) -> Option<Vec<Vec<Word>>> {
    let chars: Vec<char> = script.chars().collect();
    let mut commands = vec![Vec::new()];
    let mut i = 0;
    let expansion = |i: usize| -> Option<usize> {
        // `chars[i]` is `$`: accept `$NAME` or `${NAME}`, returning the next index.
        let rest = &chars[i + 1..];
        if rest.first() == Some(&'{') {
            let end = rest.iter().position(|c| *c == '}')?;
            let n: String = rest[1..end].iter().collect();
            return name(&n).then_some(i + 1 + end + 1);
        }
        let len = rest
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
            .count();
        let n: String = rest[..len].iter().collect();
        name(&n).then_some(i + 1 + len)
    };
    while i < chars.len() {
        let c = chars[i];
        if c == ' ' || c == '\t' {
            i += 1;
            continue;
        }
        if c == ';' || (c == '&' && chars.get(i + 1) == Some(&'&')) {
            if commands.last().is_none_or(Vec::is_empty) {
                return None;
            }
            commands.push(Vec::new());
            i += if c == ';' { 1 } else { 2 };
            continue;
        }
        let (mut raw, mut text) = (String::new(), String::new());
        while i < chars.len() {
            let c = chars[i];
            match c {
                ' ' | '\t' | ';' => break,
                '&' if chars.get(i + 1) == Some(&'&') => break,
                '\'' => {
                    let end = chars[i + 1..].iter().position(|c| *c == '\'')?;
                    let body: String = chars[i + 1..i + 1 + end].iter().collect();
                    raw.push('\'');
                    raw.push_str(&body);
                    raw.push('\'');
                    text.push_str(&body);
                    i += end + 2;
                }
                '"' => {
                    let mut j = i + 1;
                    let mut body = String::new();
                    loop {
                        match chars.get(j)? {
                            '"' => break,
                            '`' | '\\' => return None,
                            '$' => {
                                let next = expansion(j)?;
                                body.extend(&chars[j..next]);
                                j = next;
                            }
                            c => {
                                body.push(*c);
                                j += 1;
                            }
                        }
                    }
                    raw.push('"');
                    raw.push_str(&body);
                    raw.push('"');
                    text.push_str(&body);
                    i = j + 1;
                }
                '$' => {
                    let next = expansion(i)?;
                    let part: String = chars[i..next].iter().collect();
                    raw.push_str(&part);
                    text.push_str(&part);
                    i = next;
                }
                c if c.is_ascii_alphanumeric() || "_./:,+@%=-~*?^".contains(c) => {
                    raw.push(c);
                    text.push(c);
                    i += 1;
                }
                _ => return None,
            }
        }
        commands.last_mut()?.push(Word { raw, text });
    }
    if commands.last().is_none_or(Vec::is_empty) {
        return None;
    }
    Some(commands)
}
/// An inline `sh`/`bash`/`dash` `-c` script that sets up the environment and then
/// runs one check is judged by that check: any number of `umask NNN`,
/// `export NAME=value...`, `cd DIR` and `set -e/-u/-x/-o pipefail` commands joined
/// by `;` or `&&`, followed by exactly one simple command, which is classified like
/// direct argv. Its program word may start with `$HOME/` (when the script does not
/// assign HOME) but carry no other expansion. Anything else is trivial: its exit
/// status is not proven to come from a nontrivial command.
fn trivial_script(script: &str) -> bool {
    let Some(commands) = shell_script(script) else {
        return true;
    };
    let (last, setup) = commands.split_last().expect("at least one command");
    let mut assigns_home = false;
    for command in setup {
        let words: Vec<&str> = command.iter().map(|w| w.raw.as_str()).collect();
        let valid = match words.as_slice() {
            ["umask", mode] => {
                (3..=4).contains(&mode.len()) && mode.bytes().all(|b| (b'0'..=b'7').contains(&b))
            }
            ["cd", _] => true,
            ["export", assignments @ ..] if !assignments.is_empty() => {
                assignments.iter().all(|a| {
                    assigns_home |= a.starts_with("HOME=");
                    a.split_once('=').is_some_and(|(k, _)| name(k))
                })
            }
            ["set", options @ ..] if !options.is_empty() => {
                let mut options = options.iter();
                let mut ok = true;
                while let Some(o) = options.next() {
                    ok &= if *o == "-o" {
                        options.next() == Some(&"pipefail")
                    } else {
                        o.len() > 1
                            && o.starts_with('-')
                            && o[1..].bytes().all(|b| b"eux".contains(&b))
                    };
                }
                ok
            }
            _ => false,
        };
        if !valid {
            return true;
        }
    }
    let argv: Vec<String> = last.iter().map(|w| w.text.clone()).collect();
    assigns_home |= last.iter().any(|w| w.raw.starts_with("HOME="));
    let Some(head) = command_words(&argv).first() else {
        return true;
    };
    // The program must be literal: an expansion could name any program.
    let literal = head
        .strip_prefix("$HOME/")
        .or_else(|| head.strip_prefix("${HOME}/"))
        .filter(|_| !assigns_home)
        .unwrap_or(head);
    // Shell keywords and builtins that run other code, background it, invert its
    // status or only change shell state never prove a check's status.
    const SHELL_WORDS: [&str; 26] = [
        "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until",
        "do", "done", "function", "coproc", "eval", "source", ".", "builtin", "trap", "alias",
        "umask", "wait", "let", "read", "unset",
    ];
    if literal.contains(['$', '*', '?', '~']) || SHELL_WORDS.contains(&literal) {
        return true;
    }
    trivial_command(&argv)
}
/// Whether a command cannot verify a check: its exit status is not proven to come
/// from a nontrivial command (see `command_words`, `trivial_script`). A script file
/// is an ordinary project check, not an inline program.
pub fn trivial_command(argv: &[String]) -> bool {
    let w = command_words(argv);
    let Some(head) = w.first() else { return true };
    let program = base(head);
    if [
        ":", "[", "ag", "basename", "cat", "cd", "date", "df", "dirname", "du", "echo", "env",
        "export", "fd", "file", "find", "grep", "head", "hostname", "id", "less", "ls", "more",
        "printenv", "printf", "pwd", "readlink", "realpath", "rg", "set", "sleep", "sort", "stat",
        "tail", "tee", "test", "tr", "tree", "true", "type", "uname", "uniq", "wc", "which",
        "whoami", "yes", "exit", "return", "",
    ]
    .contains(&program)
    {
        return true;
    }
    if program == "git" {
        let mut rest = &w[1..];
        while let Some(s) = rest.first().filter(|s| s.starts_with('-')) {
            let n = if ["-C", "-c", "--git-dir", "--work-tree"].contains(&s.as_str()) {
                2
            } else {
                1
            };
            rest = rest.get(n..).unwrap_or(&[]);
        }
        return match rest.first().map(String::as_str) {
            None => true,
            Some("diff") => !rest
                .iter()
                .any(|s| ["--check", "--exit-code", "--quiet"].contains(&s.as_str())),
            Some(s) => [
                "branch",
                "config",
                "describe",
                "log",
                "ls-files",
                "remote",
                "rev-parse",
                "show",
                "status",
            ]
            .contains(&s),
        };
    }
    if ["sh", "bash", "dash", "zsh", "ksh"].contains(&program) {
        let mut words = w[1..].iter();
        let mut inline = false;
        while let Some(word) = words.next() {
            if word == "-o" || word == "+o" {
                words.next();
            } else if word.starts_with('-') && !word.starts_with("--") {
                inline |= word[1..].contains('c');
            } else if !word.starts_with("--") && !word.starts_with('+') {
                if !inline {
                    // A script file is an ordinary project check.
                    return false;
                }
                // zsh and ksh scripts stay outside the recognized grammar.
                return !["sh", "bash", "dash"].contains(&program) || trivial_script(word);
            }
        }
        return inline;
    }
    false
}
#[cfg(test)]
mod evidence_policy_tests {
    use super::*;
    #[test]
    fn wrappers_searches_git_and_script_files_classify_by_real_command() {
        for argv in [
            vec!["/usr/bin/rg", "needle", "src"],
            vec![
                "env", "-u", "SECRET", "A=1", "timeout", "-k", "1", "30", "nice", "-n", "2", "ls",
            ],
            vec!["git", "-C", "project", "status"],
            vec!["git", "diff", "--stat"],
            vec!["sh", "-c", "pytest; echo masked"],
            vec!["sh", "-c", "exit 0"],
            vec!["printf", "passed"],
        ] {
            assert!(
                trivial_command(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                "{argv:?}"
            );
        }
        for argv in [
            vec!["timeout", "180", "python3", "-m", "pytest"],
            vec!["env", "A=1", "cargo", "test"],
            vec!["git", "-c", "core.fsmonitor=false", "diff", "--check"],
            vec!["/bin/sh", "scripts/check.sh"],
            vec!["npm", "run", "test"],
        ] {
            assert!(
                !trivial_command(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                "{argv:?}"
            );
        }
    }
    /// Owner stores 2026-11: AGENTS.md requires `umask 022`, which pushed workers
    /// into `bash -c "umask 022; ..."`, and every such check was kept unverified as
    /// `trivial_command`. Setup prefixes followed by one simple command are judged
    /// by that command; any other operator or expansion still proves nothing.
    #[test]
    fn shell_setup_prefixes_are_judged_by_their_one_check_command() {
        let argv = |shell: &str, script: &str| vec![shell.to_owned(), "-c".into(), script.into()];
        for (shell, script) in [
            ("bash", "umask 022; cargo test --locked"),
            ("/bin/sh", "umask 022 && cd packages/workspace-ui && timeout --kill-after=5 600 npx vitest run test/a.test.tsx"),
            ("bash", "export PATH=\"$HOME/.cargo/bin:$PATH\" CARGO_TERM_COLOR=never; cargo test -p core"),
            ("bash", "umask 022; exec timeout --kill-after=5 1500 \"$HOME/.cargo/bin/cargo\" test --locked -p x"),
            ("dash", "set -eu; set -o pipefail; cd 'crates/a b' && RUST_LOG=off cargo test -- 'name with spaces'"),
            ("bash", "umask 0022;cargo fmt --check"),
        ] {
            assert!(!trivial_command(&argv(shell, script)), "{shell} {script}");
            let mut flags = vec![shell.to_owned(), "-e".into(), "-o".into(), "pipefail".into()];
            flags.extend(argv("x", script).into_iter().skip(1));
            assert!(!trivial_command(&flags), "{flags:?}");
        }
        for (shell, script) in [
            ("bash", "umask 022"),
            ("bash", "umask 022; ls"),
            ("bash", "umask 022; cargo test | tail -5"),
            ("bash", "umask 022; cargo test || true"),
            ("bash", "cargo test > log 2>&1"),
            ("bash", "cargo test & wait"),
            ("bash", "umask 022\ncargo test"),
            ("bash", "umask 022; cargo test $(echo -q)"),
            ("bash", "umask 022; cargo test `echo`"),
            ("bash", "$CHECK test"),
            ("bash", "export HOME=/tmp; \"$HOME/cargo\" test"),
            ("bash", "HOME=/tmp; cargo test"),
            ("bash", "touch x; cargo test"),
            ("bash", "coproc cargo test"),
            ("bash", "eval 'cargo test'"),
            ("bash", "! grep -n x file"),
            ("bash", "{ cargo test; }"),
            ("bash", "umask 022; cargo test 'unterminated"),
            ("bash", "umask 022;; cargo test"),
            ("bash", "umask 022; cargo test;"),
            ("bash", "cd x && pytest; echo done"),
            ("bash", "umask 022; bash -c 'cargo test; true'"),
            ("zsh", "umask 022; cargo test"),
            ("ksh", "umask 022; cargo test"),
        ] {
            assert!(trivial_command(&argv(shell, script)), "{shell} {script}");
        }
        // A nested inline script is judged the same way.
        assert!(!trivial_command(&argv(
            "bash",
            "umask 022; bash -c 'umask 022; cargo test'"
        )));
        // No script after -c proves nothing; a script file is a project check.
        assert!(trivial_command(&["bash".into(), "-c".into()]));
        assert!(!trivial_command(&[
            "bash".into(),
            "scripts/check.sh".into()
        ]));
    }
}
fn latest(db: &Transaction<'_>, task: &str, kind: &str) -> CoreResult<Option<(i64, Value)>> {
    let row:Option<(i64,String)>=db.query_row("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type=? ORDER BY id DESC LIMIT 1",
        (task,kind),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    row.map(|(id, s)| Ok((id, serde_json::from_str(&s)?)))
        .transpose()
}
pub fn execution_baseline_tx(
    db: &Transaction<'_>,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::PlanReady {
        return refuse("execution_not_expected");
    }
    let d = digest(snapshot)?;
    event_tx(
        db,
        task,
        "EXECUTION_BASELINE",
        json!({"revision":revision,"kind":"observed","revision_digest":d,"head":snapshot.head,
            "ignored_digest":snapshot.ignored_digest,
            "metadata_only":snapshot.metadata_only,"cap_reached":snapshot.cap_reached,"incomplete":snapshot.incomplete}),
    )?;
    domain::state_tx(db, task, TaskState::Executing, None)
}
/// First baseline since reopen, not latest baseline, prevents a replan hiding
/// earlier unreviewed work. A fix cycle that already has a reviewer is re-reviewed.
pub fn add_review_tx(
    db: &Transaction<'_>,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    schedule_review_tx(db, task, snapshot, false)
}

/// Invalidate an approval (including a waiver) after observed verification drift.
/// The usual review-cycle budget bounds repeated external changes.
pub fn review_changed_checkout_tx(
    db: &Transaction<'_>,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    if domain::task_tx(db, task)?.0 != TaskState::Verifying {
        return refuse("verification_not_expected");
    }
    let (review_event, reviewed) = review_binding_tx(db, task)?;
    let current = bound_digest_tx(db, task, snapshot)?;
    if reviewed == current {
        return refuse("verification_revision_unchanged");
    }
    event_tx(
        db,
        task,
        "VERIFICATION_REVISION_CHANGED",
        json!({"review_event":review_event,"reviewed_digest":reviewed,"observed_digest":current}),
    )?;
    schedule_review_tx(db, task, snapshot, true)
}

fn schedule_review_tx(
    db: &Transaction<'_>,
    task: &str,
    snapshot: &EvidenceSnapshot,
    after_drift: bool,
) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state
        != (if after_drift {
            TaskState::Verifying
        } else {
            TaskState::Executing
        })
    {
        return refuse("review_not_expected");
    }
    let nodes=db.prepare("SELECT node_id,stage,status FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid")?
        .query_map((task,revision),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?
        .collect::<Result<Vec<_>,_>>()?;
    if nodes.is_empty() || nodes.iter().any(|n| n.2 != "completed") {
        return refuse("execution_incomplete");
    }
    let observed = digest(snapshot).ok();
    let mut waiver = None;
    // Metadata-only or incomplete observations can be forged (mtime restored), so they
    // never support a waiver; the review then runs.
    let waivable = !after_drift
        && !snapshot.metadata_only
        && !snapshot.incomplete
        && !super::automatic::enabled(db, task)?;
    if waivable && !nodes.iter().any(|n| n.1 == "review") {
        if let Some(d) = observed {
            let baseline:Option<String>=db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='EXECUTION_BASELINE'
                AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED'),0) ORDER BY id LIMIT 1",
                (task,task),|r|r.get(0)).optional()?;
            if let Some(s) = baseline {
                let b: Value = serde_json::from_str(&s)?;
                if b["kind"] == "observed"
                    && b["metadata_only"] != true
                    && b["incomplete"] != true
                    && b["revision_digest"] == d
                    && b["head"] == json!(snapshot.head)
                    && snapshot.ignored_digest.is_some()
                    && b["ignored_digest"] == json!(snapshot.ignored_digest)
                {
                    waiver = Some(
                        json!({"reason":"no_project_changes","reference":"execution_baseline","reference_revision":b["revision"]}),
                    );
                }
            }
            if waiver.is_none() {
                let approved:Option<i64>=db.query_row("SELECT revision FROM runtime_nodes WHERE task_id=? AND stage='review'
                    AND status='completed' AND role='worker-code-reviewer' AND json_extract(result_json,'$.verdict')='APPROVE'
                    AND json_extract(result_json,'$.project_digest')=? AND json_extract(result_json,'$.ignored_digest')=?
                    ORDER BY rowid DESC LIMIT 1",(task,d,snapshot.ignored_digest.as_deref().unwrap_or("")),|r|r.get(0)).optional()?;
                if let Some(r) = approved {
                    waiver = Some(
                        json!({"reason":"no_changes_since_approved_review","reference":"approved_review","reference_revision":r}),
                    );
                }
            }
        }
    }
    if let Some(mut w) = waiver {
        w["revision"] = json!(revision);
        w["revision_digest"] = json!(observed);
        w["head"] = json!(snapshot.head);
        event_tx(db, task, "REVIEW_NOT_REQUIRED", w)?;
        return domain::state_tx(db, task, TaskState::Verifying, None);
    }
    let cycle: i64 = db.query_row(
        "SELECT review_cycle+1 FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    if cycle > MAX_REVIEW_CYCLES as i64 {
        domain::stop_tx(db, task, "review_cycle_limit")?;
        return Ok(());
    }
    db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status)
        VALUES (?,?,?,'worker-code-reviewer','Independently review the objective, implementation and retained reports. Use the review packet change_boundary: inspect task-written paths across plans and relevant baseline changes. Check the current bytes when a write hash differs or is unavailable. Report any material coverage gap.',?,'[\"**\"]','review','pending')",
        (task,revision,format!("_review_{cycle}"),json!(nodes.iter().map(|n|&n.0).collect::<Vec<_>>()).to_string()))?;
    // The reviewer routes as review work at the highest complexity of the work it
    // reviews; it names no provider or model (independence is chosen by routing).
    let accepted: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let accepted: Value = serde_json::from_str(accepted.as_deref().unwrap_or("{}"))?;
    let rank = |c: &str| ["low", "normal", "high"].iter().position(|v| *v == c);
    let complexity = nodes
        .iter()
        .filter(|n| n.1 == "execution")
        .filter_map(|n| {
            let routing = plan::node_routing(&accepted, &n.0);
            routing["complexity"].as_str().and_then(rank)
        })
        .max()
        .map_or("normal", |i| ["low", "normal", "high"][i]);
    plan::record_node_routing_tx(
        db,
        task,
        &format!("_review_{cycle}"),
        &json!({"work_kind":"review","complexity":complexity}),
    )?;
    db.execute(
        "UPDATE runtime_tasks SET review_cycle=? WHERE task_id=?",
        (cycle, task),
    )?;
    domain::state_tx(db, task, TaskState::Reviewing, None)
}
pub fn start_review_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    if domain::task_tx(db, task)?.0 != TaskState::Reviewing {
        return refuse("review_not_expected");
    }
    let current: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
        WHERE n.task_id=? AND n.assignment_ref=? AND n.revision=t.revision AND n.stage='review'
        AND n.role='worker-code-reviewer' AND n.status='running')",
        (task, assignment),
        |r| r.get(0),
    )?;
    if !current {
        return refuse("stale_assignment_result");
    }
    let bound: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'
        AND json_extract(payload_json,'$.assignment_ref')=?)",
        (task, assignment),
        |r| r.get(0),
    )?;
    if bound {
        return refuse("review_already_bound");
    }
    // The host records the packet first, so its binding is known here.
    let (revision_digest, binding) = review_digest_tx(db, task, assignment, snapshot)?;
    event_tx(
        db,
        task,
        "REVIEW_STARTED",
        json!({"assignment_ref":assignment,"revision_digest":revision_digest,"binding":binding}),
    )?;
    Ok(())
}
/// Result publication/ownership is checked in the transaction. A report is not
/// acceptance, and finishing a node never releases an engine reservation.
enum Independence {
    Independent(Value),
    Unverified(Value),
    Warning(Value),
    Refused,
}

/// The host records the exact packet delivered to a reviewer and the native
/// project channel records every file read.  A reviewer result is not allowed
/// to claim a complete verdict until those two authoritative records agree.
/// This is deliberately scoped to the packet (not the checkout): unrelated
/// files must never become a surprise review obligation.
/// The changed files a review has not read completely on its own channel, from its
/// latest review packet. Each entry is `{"path"|"kind","reason"}`. An INCOMPLETE
/// review is not asked for parts it declared blocked by a dead end, nor for paths
/// outside its read scope that it cannot resolve itself; any other verdict must have
/// read every listed changed file the packet marks `required`: the task's own
/// changes (write receipts and paths its commands changed). A file another actor
/// changed in the shared checkout is listed as context (`required: false`) and is
/// never a missing item. A packet without the mark (an earlier build), or one
/// whose attribution was incomplete, requires every listed file.
///
/// A change set the host could not enumerate completely (no usable baseline
/// manifest, or more changed files than the packet lists) is a host limit no
/// reviewer can resolve, so it is not a missing item: the verdict covers the listed
/// files, and `review_change_set_limits_tx` records what was not listed with the
/// result, never as verified. A listed file whose current bytes have no reviewable
/// text (binary, an image the review was not shown, opaque encoded or minified
/// content) is likewise not a missing item but such a recorded limit.
pub fn review_coverage_missing_tx(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
    result: &Value,
) -> CoreResult<Vec<Value>> {
    let Some(packet) = review_packet_of(db, task, assignment)? else {
        return Ok(Vec::new());
    };
    let incomplete = result["verdict"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case("INCOMPLETE"));
    let blocked = super::review::blocked_items(result);
    let changed = &packet["changed_files"];
    let mut missing = Vec::new();
    if !incomplete {
        if packet["read_scope_covers_change"] != true {
            for path in packet["outside_read_scope"]
                .as_array()
                .into_iter()
                .flatten()
            {
                missing.push(json!({"path":path,"reason":"outside reviewer read scope"}));
            }
        }
    }
    let channel = review_channel(db, task, assignment)?;
    let outside: BTreeSet<&str> = packet["outside_read_scope"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if let Some(files) = changed["files"].as_array() {
        for file in files {
            let Some(path) = file["path"].as_str() else {
                continue;
            };
            // Deletions are proven by the complete baseline/current manifest;
            // there are no current bytes for project_read to inspect.
            // Only current regular-file bytes the host hashed can be read and bound:
            // a directory or an unobservable entry has no `current_sha256`.
            if file["change"] == "deleted"
                || !file["current_sha256"].is_string()
                || file["required"] == false
                || blocked.contains(path)
            {
                continue;
            }
            if incomplete && outside.contains(path) {
                continue;
            }
            match file_coverage(db, task, channel.as_deref(), file)? {
                FileCoverage::Covered | FileCoverage::NoText => {}
                FileCoverage::Missing(reason) => missing.push(json!({"path":path,"reason":reason})),
            }
        }
    }
    Ok(missing)
}
/// The review assignment's current channel, whose reads count as its coverage.
fn review_channel(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
) -> CoreResult<Option<String>> {
    Ok(db
        .query_row(
            "SELECT channel_id FROM trusted_channels WHERE task_id=? AND assignment_id=?
             AND revoked_at IS NULL ORDER BY rowid DESC LIMIT 1",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?)
}
/// Whether a review covered one listed changed file with current bytes.
enum FileCoverage {
    Covered,
    /// Text the review can still read; the reason names what is missing.
    Missing(&'static str),
    /// Current bytes with no reviewable text (binary, an image the review did not
    /// read directly, opaque encoded or minified content): never a missing item; an approval
    /// discloses it as an `unverified` limit (`review_change_set_limits_tx`).
    NoText,
}
/// Content kinds the host marks as having no reviewable text (`review_content`).
const NO_TEXT: [&str; 3] = ["binary", "image", "opaque"];
fn file_coverage(
    db: &rusqlite::Connection,
    task: &str,
    channel: Option<&str>,
    file: &Value,
) -> CoreResult<FileCoverage> {
    let no_text = file["review_content"]
        .as_str()
        .is_some_and(|k| NO_TEXT.contains(&k));
    let Some(channel) = channel else {
        return Ok(if no_text {
            FileCoverage::NoText
        } else {
            FileCoverage::Missing("review channel unavailable")
        });
    };
    let path = file["path"].as_str().unwrap_or("");
    let key = format!("file-read:{}", crate::digest(path.as_bytes()));
    let read: Option<Value> = db
        .query_row(
            "SELECT result_json FROM runtime_requests WHERE task_id=? AND request_key=?
             AND actor=? AND operation='native_file_read'",
            (task, key, channel),
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|v: &Value| v["sha256"] == file["current_sha256"]);
    let Some(read) = read else {
        return Ok(if no_text {
            FileCoverage::NoText
        } else {
            FileCoverage::Missing("current complete read is missing or stale")
        });
    };
    // A read of these exact bytes that found no text proves there is nothing to
    // read as text.
    if read["content"] == "binary" {
        return Ok(FileCoverage::NoText);
    }
    if read["complete"] == true || review_lines_read(&file["review_lines"], &read["lines"]) {
        return Ok(FileCoverage::Covered);
    }
    Ok(if no_text {
        FileCoverage::NoText
    } else {
        FileCoverage::Missing("current complete read is missing or stale")
    })
}
/// Whether a read of the current bytes covered every line of the packet's
/// `review_lines` ranges (the lines that differ from HEAD, with context). A file
/// without such ranges is never covered this way: it needs a complete read.
fn review_lines_read(ranges: &Value, lines: &Value) -> bool {
    let (Some(ranges), Some(lines)) = (ranges.as_array(), lines.as_array()) else {
        return false;
    };
    let read: BTreeSet<u64> = lines.iter().filter_map(Value::as_u64).collect();
    !ranges.is_empty()
        && ranges
            .iter()
            .all(|range| match (range[0].as_u64(), range[1].as_u64()) {
                (Some(start), Some(end)) if start >= 1 && start <= end => {
                    (start..=end).all(|n| read.contains(&n))
                }
                _ => false,
            })
}
/// The latest review packet the host recorded for `assignment`.
fn review_packet_of(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
) -> CoreResult<Option<Value>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_PACKET'
             AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let mut event: Value = serde_json::from_str(&raw)?;
    Ok(Some(event["packet"].take()))
}
/// What an approving review could not cover because the host could not list it,
/// from its review packet, and the listed changed files whose current bytes have no
/// reviewable text (`FileCoverage::NoText`): each is an `unverified` check (never a
/// pass) that the final result must disclose, so the owner is told to inspect them.
/// Empty when the packet listed every changed file and each one is text or was
/// read completely.
pub fn review_change_set_limits_tx(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
) -> CoreResult<Vec<Value>> {
    let Some(packet) = review_packet_of(db, task, assignment)? else {
        return Ok(Vec::new());
    };
    let changed = &packet["changed_files"];
    let mut limits = Vec::new();
    // With complete attribution the task's own command changes are listed even
    // without a baseline comparison, and an unlisted file outside them is another
    // actor's change: neither is a limit of this review.
    let attributed = changed["attribution_complete"] == true;
    if changed["complete"] != true && !attributed {
        limits.push(json!({"name":"Changes outside the recorded writes","outcome":"unverified",
            "blocker":"Changes made by commands outside the recorded writes could not be listed; inspect the folder.",
            "source":"runtime"}));
    }
    let beyond = if attributed {
        &changed["more"]["required"]
    } else {
        &changed["more"]["count"]
    };
    if let Some(more) = beyond.as_i64().filter(|n| *n > 0) {
        limits.push(json!({"name":"Changed files beyond the review list","outcome":"unverified",
            "blocker":format!("{more} changed files beyond the review list were not read individually by the reviewer; inspect the folders named in the review packet."),
            "source":"runtime"}));
    }
    let channel = review_channel(db, task, assignment)?;
    let mut unreviewable = Vec::new();
    for file in changed["files"].as_array().into_iter().flatten() {
        if file["change"] == "deleted"
            || !file["current_sha256"].is_string()
            || file["required"] == false
        {
            continue;
        }
        if let FileCoverage::NoText = file_coverage(db, task, channel.as_deref(), file)? {
            unreviewable.extend(file["path"].as_str().map(str::to_owned));
        }
    }
    if !unreviewable.is_empty() {
        let shown = unreviewable
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = unreviewable.len().saturating_sub(10);
        limits.push(json!({"name":"Changed files without reviewable text","outcome":"unverified",
            "blocker":format!("{} changed files have no reviewable text (binary, an image the reviewer did not read directly, or encoded/minified content) and were not reviewed as text: {shown}{}; inspect them directly.",
                unreviewable.len(), if more > 0 { format!(" and {more} more") } else { String::new() }),
            "paths":unreviewable.iter().take(50).collect::<Vec<_>>(),"source":"runtime"}));
    }
    Ok(limits)
}
/// What a submitted result leaves undone that its worker can still do, as the
/// refusal code and the exact outstanding items. `None` when the result may be
/// recorded. A worker finishes its own assignment: only a dead end it names with
/// its blocker category (see `review::COVERAGE_BLOCKERS`) is accepted as unfinished.
pub fn outstanding_tx(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
    stage: &str,
    result: &Value,
) -> CoreResult<Option<(&'static str, Vec<String>)>> {
    if result["assignment_outcome"] == "failed"
        && !(super::review::valid_blocker(&result["blocker"]["kind"])
            && result["blocker"]["detail"]
                .as_str()
                .is_some_and(|d| !d.trim().is_empty()))
    {
        return Ok(Some(("assignment_blocker_required", Vec::new())));
    }
    if stage != "review" {
        return Ok(None);
    }
    // An incomplete review names what it did not cover; only that verdict does.
    let incomplete = result["verdict"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case("INCOMPLETE"));
    if incomplete
        != result["not_covered"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    {
        return Ok(Some(("review_not_covered_required", Vec::new())));
    }
    if super::review::block_without_findings(result) {
        return Ok(Some(("review_block_findings_required", Vec::new())));
    }
    let unfinished = super::review::unfinished_coverage(result);
    if !unfinished.is_empty() {
        return Ok(Some(("review_coverage_unfinished", unfinished)));
    }
    let missing = review_coverage_missing_tx(db, task, assignment, result)?;
    if !missing.is_empty() {
        let items = missing
            .iter()
            .map(|m| match m["path"].as_str() {
                Some(path) => format!("{path} ({})", m["reason"].as_str().unwrap_or("")),
                None => format!(
                    "{} ({})",
                    m["kind"].as_str().unwrap_or("change"),
                    m["reason"].as_str().unwrap_or("")
                ),
            })
            .collect();
        return Ok(Some(("review_file_coverage_required", items)));
    }
    Ok(None)
}

fn review_independence_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    revision: i64,
) -> CoreResult<Independence> {
    let route = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> CoreResult<Vec<Value>> {
        Ok(db
            .prepare(sql)?
            .query_map(params, |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect())
    };
    let own = route(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_ROUTE_SELECTED'
        AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
        &[&task, &assignment],
    )?;
    let reviewed = route(
        "SELECT e.payload_json FROM runtime_events e WHERE e.task_id=? AND e.type='NATIVE_ROUTE_SELECTED'
        AND json_extract(e.payload_json,'$.revision')=? AND json_extract(e.payload_json,'$.node') IN
        (SELECT node_id FROM runtime_nodes WHERE task_id=e.task_id AND revision=? AND stage='execution') ORDER BY e.id",
        &[&task, &revision, &revision],
    )?;
    let (Some(own), false) = (own.first(), reviewed.is_empty()) else {
        return Ok(Independence::Unverified(
            json!({"status":"unverified","reason":"route_not_recorded"}),
        ));
    };
    let model = own["model"].as_str().unwrap_or("");
    let models: BTreeSet<&str> = reviewed
        .iter()
        .filter_map(|r| r["model"].as_str())
        .collect();
    if model.is_empty() || models.is_empty() {
        return Ok(Independence::Unverified(
            json!({"status":"unverified","reason":"model_not_recorded"}),
        ));
    }
    let engines: BTreeSet<&str> = reviewed
        .iter()
        .filter_map(|r| r["route_engine"].as_str())
        .collect();
    let same_engine = own["route_engine"]
        .as_str()
        .is_some_and(|e| engines.contains(e));
    if !models.contains(model) {
        return Ok(Independence::Independent(
            json!({"status":"independent","model":model,"same_engine_as_reviewed":same_engine}),
        ));
    }
    let warning = &own["decision"]["independence"];
    if warning["status"] == "warning" {
        let mut facts = json!({"status":"warning","model":model,
            "same_engine_as_reviewed":same_engine,"warning":warning["warning"]});
        if warning["mode"].is_string() {
            facts["mode"] = warning["mode"].clone();
        }
        return Ok(Independence::Warning(facts));
    }
    Ok(Independence::Refused)
}
pub fn record_result_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    result: &Value,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if !matches!(state, TaskState::Executing | TaskState::Reviewing) {
        return refuse("stale_assignment_result");
    }
    waits::check_mutation_tx(db, task, assignment)?;
    let row:Option<(i64,String,String,Option<String>)>=db.query_row("SELECT revision,stage,status,result_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
        (task,assignment),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (rev, stage, status, old) = row.ok_or(CoreError::Refused("stale_assignment_result"))?;
    if rev != revision {
        return refuse("stale_assignment_result");
    }
    if let Some(old) = old {
        let mut old: Value = serde_json::from_str(&old)?;
        if let Some(o) = old.as_object_mut() {
            o.remove("report_digest");
            o.remove("project_digest");
            o.remove("ignored_digest");
        }
        return if old == *result {
            Ok(())
        } else {
            refuse("conflicting_assignment_result")
        };
    }
    if status != "running" {
        return refuse("assignment_not_running");
    }
    let report = result["report_ref"]
        .as_str()
        .ok_or(CoreError::Refused("assignment_report_missing"))?;
    let attempt: Option<Option<String>> = db
        .query_row(
            "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let attempt = attempt.ok_or(CoreError::Refused("assignment_report_missing"))?;
    let published:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM reports r JOIN task_events e ON e.task_id=r.task_id
        WHERE r.task_id=? AND r.id=? AND e.assignment_ref IN (?,?) AND e.event_type='worker_result' AND e.superseded=0
        AND json_extract(e.payload_json,'$.report_ref')=r.id)",(task,report,assignment,&attempt),|r|r.get(0))?;
    if !published {
        return refuse("assignment_report_mismatch");
    }
    let artifact = snapshot
        .artifacts
        .get(report)
        .filter(|s| s.len() == 64)
        .ok_or(CoreError::Refused("assignment_report_missing"))?;
    let mut result = result.clone();
    if !result.is_object() {
        return refuse("invalid_assignment_result");
    }
    result["report_digest"] = json!(artifact);
    if let Some(digest) = &snapshot.project_digest {
        result["project_digest"] = json!(digest);
    }
    if let Some(digest) = &snapshot.ignored_digest {
        result["ignored_digest"] = json!(digest);
    }
    // Nothing the worker can still do may be left undone; result_submit refuses the
    // same before publication with the exact items, and the assignment stays active.
    if let Some((code, _)) = outstanding_tx(db, task, assignment, &stage, &result)? {
        if code == "review_file_coverage_required" {
            let missing = review_coverage_missing_tx(db, task, assignment, &result)?;
            event_tx(
                db,
                task,
                "REVIEW_COVERAGE_REQUIRED",
                json!({"assignment_ref":assignment,"missing":missing,
                    "message":"Read every listed changed file marked required with project_read before submitting this verdict, or submit INCOMPLETE naming each part you cannot reach with its blocker."}),
            )?;
        }
        return refuse(code);
    }
    // A reviewer that requests changes may cite a failing check as the finding;
    // that is a completed review that starts a fix cycle, not a failed reviewer.
    // Only an explicit assignment_outcome=failed marks the review itself failed.
    // The same holds for BLOCK and INCOMPLETE: the verdict, not the cited check, routes it.
    let requests_changes = stage == "review"
        && ["REQUEST_CHANGES", "BLOCK", "INCOMPLETE"]
            .contains(&result["verdict"].as_str().unwrap_or(""));
    let failed = result["assignment_outcome"] == "failed"
        || (!requests_changes
            && result["checks"].as_array().is_some_and(|c| {
                c.iter()
                    .any(|c| c["outcome"] == "failed" && !super::worker_checks::resolved(c))
            }));
    // Passed worker checks are not promoted into final verification evidence.
    if stage == "review" {
        if !["APPROVE", "REQUEST_CHANGES", "BLOCK", "INCOMPLETE"]
            .contains(&result["verdict"].as_str().unwrap_or(""))
        {
            return refuse("review_verdict_required");
        }
        let start: String = db
            .query_row(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'
            AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
                (task, assignment),
                |r| r.get(0),
            )
            .optional()?
            .ok_or(CoreError::Refused("review_revision_unavailable"))?;
        let start: Value = serde_json::from_str(&start)?;
        let (bound, binding) = review_digest_tx(db, task, assignment, snapshot)?;
        if start["assignment_ref"] != assignment || start["revision_digest"] != bound {
            return refuse("review_resubmit_required");
        }
        // Independence rests on the recorded routes: a reviewer on the model of the
        // work it judges is refused unless routing found no other model and said so.
        // A reviewer is never the attempt that executed work (hard guard); routing
        // evidence adds model independence on top of it.
        let same_attempt: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM delegation_provisioning p JOIN runtime_nodes n
            ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
            WHERE p.task_id=? AND n.stage='execution' AND p.attempt_id=?)",
            (task, &attempt),
            |r| r.get(0),
        )?;
        if same_attempt {
            return refuse("review_not_independent");
        }
        match review_independence_tx(db, task, assignment, revision)? {
            Independence::Independent(facts) => {
                result["review_independence"] = facts;
            }
            // Independence that cannot be established is shown like a known
            // violation, never as a silent field.
            Independence::Warning(facts) | Independence::Unverified(facts) => {
                event_tx(
                    db,
                    task,
                    "REVIEW_INDEPENDENCE_WARNING",
                    json!({"assignment_ref":assignment,"independence":facts}),
                )?;
                result["review_independence"] = facts;
            }
            Independence::Refused => return refuse("review_not_independent"),
        }
        // An approval covers the changed files its packet listed. What the host
        // could not list stays with the result as unverified checks the final
        // result must disclose, never as covered.
        if result["verdict"] == "APPROVE" {
            let limits = review_change_set_limits_tx(db, task, assignment)?;
            if !limits.is_empty() {
                event_tx(
                    db,
                    task,
                    "REVIEW_CHANGE_SET_LIMITED",
                    json!({"assignment_ref":assignment,"limits":limits}),
                )?;
                match result["checks"].as_array_mut() {
                    Some(checks) => checks.extend(limits),
                    None => result["checks"] = json!(limits),
                }
            }
        }
        // `project_digest` stays the whole checkout observed with the verdict (when
        // observed); `review_digest` is what the verdict binds and what verification
        // and completion compare.
        if binding == "checkout" {
            result["project_digest"] = json!(digest(snapshot)?);
        }
        result["review_digest"] = json!(bound);
        result["review_binding"] = json!(binding);
    }
    db.execute(
        "UPDATE runtime_nodes SET status=?,result_json=? WHERE task_id=? AND assignment_ref=?",
        (
            if failed { "failed" } else { "completed" },
            result.to_string(),
            task,
            assignment,
        ),
    )?;
    domain::revoke_tx(db, task, Some(assignment))?;
    domain::graph_attempt_tx(
        db,
        task,
        assignment,
        if failed { "failed" } else { "reported" },
        if failed { Some("worker_failed") } else { None },
    )?;
    event_tx(
        db,
        task,
        if failed {
            "WORKER_FAILED"
        } else {
            "WORKER_COMPLETED"
        },
        json!({"assignment_ref":assignment,"attempt_id":attempt,"result":result}),
    )?;
    Ok(())
}
pub(super) fn review_binding_tx(db: &Transaction<'_>, task: &str) -> CoreResult<(i64, String)> {
    let (_, revision) = domain::task_tx(db, task)?;
    let review: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT status,result_json FROM runtime_nodes
        WHERE task_id=? AND revision=? AND stage='review' ORDER BY rowid DESC LIMIT 1",
            (task, revision),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((status, result)) = review {
        let v: Value = serde_json::from_str(&result.unwrap_or_default())
            .map_err(|_| CoreError::Refused("completion_review_missing"))?;
        if status != "completed" || v["verdict"] != "APPROVE" {
            return refuse("completion_review_missing");
        }
        // The approval's WORKER_COMPLETED event must follow its REVIEW_STARTED.
        let event:Option<(i64,String)>=db.query_row("SELECT e.id,e.payload_json FROM runtime_events e
            WHERE e.task_id=? AND e.type='WORKER_COMPLETED' AND json_extract(e.payload_json,'$.result.report_ref')=?
            ORDER BY e.id DESC LIMIT 1",(task,v["report_ref"].as_str()),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (id, _) = event.ok_or(CoreError::Refused("completion_review_missing"))?;
        // An approval recorded before task-file bindings binds the whole checkout.
        return Ok((
            id,
            v["review_digest"]
                .as_str()
                .or(v["project_digest"].as_str())
                .ok_or(CoreError::Refused("completion_review_missing"))?
                .into(),
        ));
    }
    let (id, v) = latest(db, task, "REVIEW_NOT_REQUIRED")?
        .ok_or(CoreError::Refused("completion_review_missing"))?;
    if v["revision"] != revision {
        return refuse("completion_review_missing");
    }
    Ok((
        id,
        v["revision_digest"]
            .as_str()
            .ok_or(CoreError::Refused("completion_review_missing"))?
            .into(),
    ))
}
/// Host preflight before any command effect; retain_check repeats these gates.
pub fn verification_preflight_tx(
    db: &Transaction<'_>,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    if domain::task_tx(db, task)?.0 != TaskState::Verifying {
        return refuse("verification_not_expected");
    }
    let (_, reviewed) = review_binding_tx(db, task)?;
    if reviewed != bound_digest_tx(db, task, snapshot)? {
        return refuse("verification_review_revision_changed");
    }
    let active: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND kind='worker' AND state!='released')", [task], |r| r.get(0))?;
    if active {
        return refuse("workers_still_live_or_unknown");
    }
    Ok(())
}
/// Runtime CommandExecutor returns this observation; no model JSON is accepted.
/// A receipt belongs to exactly one argv, revision and observed digest.
pub fn retain_check_tx(
    db: &Transaction<'_>,
    task: &str,
    owner: &OwnerEpoch,
    key: &str,
    argv: &[String],
    observed: &CommandObservation,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<Value> {
    ownership::check_tx(db, task, owner)?;
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::Verifying || observed.receipt_ref.is_empty() || argv.is_empty() {
        return refuse("verification_not_expected");
    }
    let (review_event, reviewed) = review_binding_tx(db, task)?;
    if reviewed != bound_digest_tx(db, task, snapshot)? {
        return refuse("verification_review_revision_changed");
    }
    let value = json!({"argv":argv,"complete":observed.complete,"exit_code":observed.exit_code,
        "receipt_ref":observed.receipt_ref,"revision":revision,"revision_digest":reviewed,"owner_epoch":owner.epoch,"review_event":review_event});
    // Why an incomplete or failing run failed (a timeout, a signal, an unavailable
    // sandbox), from the runtime's own retained output receipt of the same run;
    // without one the cause stays unknown (null), never invented.
    let cause: Option<String> = db
        .query_row(
            "SELECT json_extract(result_json,'$.observation') FROM runtime_requests
             WHERE task_id=? AND actor='native-verifier-output' AND operation='native_command_output'
             AND json_extract(result_json,'$.tool_call_ref')=? LIMIT 1",
            (task, &observed.receipt_ref),
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let cause: Value = cause
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let mut retained = value.clone();
    retained["failure"] = cause["failure"].clone();
    retained["signal"] = cause["signal"].clone();
    // The request identity stays the command observation itself.
    request_tx(
        db,
        task,
        "runtime-verifier",
        key,
        "verification_command",
        &value,
        || Ok(retained),
    )
}
pub fn record_verification_tx(
    db: &Transaction<'_>,
    task: &str,
    receipt_keys: &[String],
    snapshot: &EvidenceSnapshot,
) -> CoreResult<bool> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::Verifying {
        return refuse("verification_not_expected");
    }
    let (review_event, reviewed) = review_binding_tx(db, task)?;
    if reviewed != bound_digest_tx(db, task, snapshot)? {
        return refuse("verification_review_revision_changed");
    }
    let plan: String = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let plan: Value = serde_json::from_str(&plan)?;
    let commands = plan["verification"]
        .as_array()
        .ok_or(CoreError::Refused("verification_commands_missing"))?;
    if commands.is_empty() {
        return refuse("verification_commands_missing");
    }
    let mut checks = Vec::new();
    let mut ids = BTreeSet::new();
    let mut passed = commands.len() == receipt_keys.len();
    let mut unresolved = Vec::new();
    for (index, key) in receipt_keys.iter().enumerate() {
        let s:Option<String>=db.query_row("SELECT result_json FROM runtime_requests WHERE task_id=? AND actor='runtime-verifier'
            AND request_key=? AND operation='verification_command'",(task,key),|r|r.get(0)).optional()?;
        let v: Value =
            serde_json::from_str(&s.ok_or(CoreError::Refused("verification_receipt_missing"))?)?;
        let receipt = v["receipt_ref"]
            .as_str()
            .ok_or(CoreError::Refused("verification_receipt_missing"))?;
        if !ids.insert(receipt.to_owned()) {
            return refuse("verification_receipt_replayed");
        }
        let reused: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_events e, json_each(e.payload_json,'$.checks') c
            WHERE e.task_id=? AND e.type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED')
            AND json_extract(c.value,'$.receipt_ref')=?)",
            (task, receipt),
            |r| r.get(0),
        )?;
        if reused {
            return refuse("verification_receipt_replayed");
        }
        let epoch: i64 = db.query_row(
            "SELECT epoch FROM task_owners WHERE task_id=?",
            [task],
            |r| r.get(0),
        )?;
        let valid = v["complete"] == true
            && v["exit_code"] == 0
            && v["revision"] == revision
            && v["revision_digest"] == reviewed
            && v["review_event"] == review_event
            && v["owner_epoch"] == epoch
            && commands.get(index) == Some(&v["argv"]);
        if !valid {
            passed = false;
            unresolved.extend(commands.get(index).cloned());
        }
        checks.push(v);
    }
    let mut verification = json!({"revision":revision,"revision_digest":reviewed,"review_event":review_event,"checks":checks});
    // The whole checkout the commands ran against, which a worker's check receipt
    // names; `revision_digest` is the review binding (it equals this digest when
    // the review bound the whole checkout).
    if let Some(checkout) = snapshot.project_digest.as_deref() {
        verification["checkout_digest"] = json!(checkout);
    }
    if !passed {
        unresolved.extend(commands.iter().skip(receipt_keys.len()).cloned());
        verification["unresolved_commands"] = json!(unresolved);
    }
    if !passed && super::automatic::enabled(db, task)? {
        verification["commands"] = json!(commands);
    }
    event_tx(
        db,
        task,
        if passed {
            "VERIFICATION_PASSED"
        } else {
            "VERIFICATION_FAILED"
        },
        verification,
    )?;
    if passed {
        domain::state_tx(db, task, TaskState::Reconciling, None)?;
    } else {
        let failures: i64 = db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='VERIFICATION_FAILED'",
            [task],
            |r| r.get(0),
        )?;
        if failures <= MAX_VERIFICATION_REPAIRS as i64 {
            domain::state_tx(db, task, TaskState::Planning, None)?;
        } else {
            domain::stop_tx(db, task, "verification_failed")?;
        }
    }
    Ok(passed)
}
pub fn submit_answer_tx(db: &Transaction<'_>, task: &str, text: &str) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    let informational:bool=db.query_row("SELECT plan_json IS NULL
        AND NOT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=?)
        AND NOT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator')
        FROM runtime_tasks WHERE task_id=?",(task,task,task),|r|r.get(0))?;
    if state != TaskState::Planning
        || revision != 0
        || !informational
        || text.trim().is_empty()
        || super::automatic::enabled(db, task)?
    {
        return refuse("direct_answer_not_allowed");
    }
    db.execute(
        "UPDATE runtime_tasks SET synthesis=? WHERE task_id=?",
        (text, task),
    )?;
    event_tx(db, task, "ANSWER_SUBMITTED", json!({"text":text}))?;
    domain::state_tx(db, task, TaskState::AnswerReady, None)
}
/// Older format-12 events retain the individual receipts only.
pub(super) fn unresolved_commands(failure: &Value, planned: &Value) -> Vec<Value> {
    if let Some(commands) = failure["unresolved_commands"].as_array() {
        return commands.clone();
    }
    if let Some(commands) = failure["commands"]
        .as_array()
        .or_else(|| planned.as_array())
    {
        return commands
            .iter()
            .enumerate()
            .filter(|(index, argv)| {
                let check = &failure["checks"][*index];
                check["complete"] != true
                    || check["exit_code"] != 0
                    || &check["argv"] != *argv
                    || check["revision"] != failure["revision"]
                    || check["revision_digest"] != failure["revision_digest"]
                    || check["review_event"] != failure["review_event"]
            })
            .map(|(_, argv)| argv.clone())
            .collect();
    }
    failure["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["exit_code"] != 0 || c["complete"] != true)
        .map(|c| c["argv"].clone())
        .collect()
}

/// Carry open verification failures across repair plans. A replacement under
/// verification_submit may still be acknowledged through the existing owner
/// question route, but answering a retry prompt never waives an undropped check.
fn outstanding_commands(
    db: &Transaction<'_>,
    task: &str,
    include_retained: bool,
) -> CoreResult<Vec<Value>> {
    let mut stmt = db.prepare("SELECT id,type,payload_json FROM runtime_events WHERE task_id=?
        AND type IN ('VERIFICATION_FAILED','VERIFICATION_REVISED','VERIFICATION_PASSED','USER_QUESTION_REQUESTED','USER_ANSWER_RECEIVED') ORDER BY id")?;
    let rows = stmt.query_map([task], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    // Only an explicitly dropped command has a removal event that an owner may resolve.
    let mut outstanding: BTreeMap<String, (Value, Option<i64>)> = BTreeMap::new();
    let mut questions = BTreeMap::new();
    for row in rows {
        let (id, kind, raw) = row?;
        let v: Value = serde_json::from_str(&raw)?;
        match kind.as_str() {
            "VERIFICATION_FAILED" if include_retained => {
                // Older runs may have stopped before every planned argv got a receipt.
                let planned = if v["unresolved_commands"].is_null() && v["commands"].is_null() {
                    let raw: Option<String> = db.query_row(
                        "SELECT coalesce(json_extract(payload_json,'$.plan.verification'),json_extract(payload_json,'$.verification'),'null')
                         FROM runtime_events WHERE task_id=? AND id<? AND type IN ('PLAN_ACCEPTED','VERIFICATION_REVISED')
                         ORDER BY id DESC LIMIT 1", (task,id), |r|r.get(0)).optional()?;
                    serde_json::from_str(raw.as_deref().unwrap_or("null"))?
                } else {
                    Value::Null
                };
                for argv in unresolved_commands(&v, &planned) {
                    outstanding.insert(argv.to_string(), (argv, None));
                }
            }
            "VERIFICATION_REVISED" => {
                for argv in v["dropped_failed"].as_array().into_iter().flatten() {
                    outstanding.insert(argv.to_string(), (argv.clone(), Some(id)));
                }
            }
            "VERIFICATION_PASSED" => {
                for c in v["checks"].as_array().into_iter().flatten() {
                    if c["complete"] == true && c["exit_code"] == 0 {
                        outstanding.remove(&c["argv"].to_string());
                    }
                }
            }
            "USER_QUESTION_REQUESTED" => {
                if let Some(question) = v["question_id"].as_str() {
                    questions.insert(question.to_owned(), id);
                }
            }
            "USER_ANSWER_RECEIVED" => {
                if let Some(asked) = v["question_id"].as_str().and_then(|q| questions.remove(q)) {
                    outstanding
                        .retain(|_, (_, removed)| !removed.is_some_and(|event| event < asked));
                }
            }
            _ => {}
        }
    }
    Ok(outstanding.into_values().map(|(argv, _)| argv).collect())
}

pub fn dropped_failures_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Vec<Value>> {
    outstanding_commands(db, task, false)
}

pub(super) fn require_repair_commands(
    db: &Transaction<'_>,
    task: &str,
    proposed: &Value,
) -> CoreResult<()> {
    let commands = proposed
        .as_array()
        .ok_or(CoreError::Refused("verification_commands_missing"))?;
    if outstanding_commands(db, task, true)?
        .iter()
        .any(|argv| !commands.contains(argv))
    {
        return refuse("verification_failed_check_missing");
    }
    Ok(())
}
/// Runtime alone calls this after reconciling processes and safe artifact reads.
pub fn complete_tx(
    db: &Transaction<'_>,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if !matches!(state, TaskState::Reconciling | TaskState::AnswerReady) {
        return refuse("completion_not_expected");
    }
    let (blocker, synthesis): (Option<String>, Option<String>) = db.query_row(
        "SELECT blocker,synthesis FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if blocker.is_some() || synthesis.as_deref().is_none_or(|s| s.trim().is_empty()) {
        return refuse("completion_synthesis_or_blocker");
    }
    let stopped: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=?)",
        [task],
        |r| r.get(0),
    )?;
    if stopped {
        return refuse("completion_task_stopped");
    }
    let pending:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM pending_questions WHERE task_id=? AND status='open')
        OR EXISTS(SELECT 1 FROM task_events WHERE task_id=? AND event_type IN ('user_message','coordinator_clarification') AND consumed_by IS NULL AND superseded=0)",
        (task,task),|r|r.get(0))?;
    // An owner message still waiting for the coordinator is never completed past.
    if pending
        || !super::steering::pending(db, task)?.is_empty()
        || !super::steering::undelivered(db, task)?.is_empty()
        || waits::projection_tx(db, task)?
            .iter()
            .any(|w| ["open", "ready", "resuming"].contains(&w["status"].as_str().unwrap_or("")))
    {
        return refuse("completion_pending_input");
    }
    let attempts=db.prepare("SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator'")?
        .query_map([task],|r|r.get::<_,Option<String>>(0))?.collect::<Result<Vec<_>,_>>()?;
    for a in attempts {
        if a.as_ref().and_then(|a| snapshot.workers.get(a))
            != Some(&ProcessObservation::ConfirmedExited)
        {
            return refuse("workers_still_live_or_unknown");
        }
    }
    let capacity:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND kind='worker' AND state!='released')",
        [task],|r|r.get(0))?;
    if capacity {
        return refuse("workers_still_live_or_unknown");
    }
    let informational:bool=db.query_row("SELECT revision=0 AND plan_json IS NULL AND NOT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=?)\n            AND NOT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator') FROM runtime_tasks WHERE task_id=?",(task,task,task),|r|r.get(0))?;
    if state == TaskState::AnswerReady && informational {
        if super::automatic::enabled(db, task)? {
            return refuse("completion_answer_invariant");
        }
        let informational:bool=db.query_row("SELECT plan_json IS NULL AND NOT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=?)
            AND NOT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator')
            FROM runtime_tasks WHERE task_id=?",(task,task,task),|r|r.get(0))?;
        if revision != 0 || !informational {
            return refuse("completion_answer_invariant");
        }
        let answer = latest(db, task, "ANSWER_SUBMITTED")?
            .ok_or(CoreError::Refused("completion_answer_missing"))?
            .1;
        if answer["text"] != json!(synthesis) {
            return refuse("completion_answer_missing");
        }
    } else {
        let nodes=db.prepare("SELECT status,result_json,assignment_ref FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid")?
            .query_map((task,revision),|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
        if nodes.is_empty()
            || nodes
                .iter()
                .any(|(s, r, _)| s != "completed" || r.is_none())
        {
            return refuse("completion_nodes_incomplete");
        }
        if state == TaskState::AnswerReady {
            if !super::reconciliation::direct_answer_allowed(db, task)? {
                return refuse("completion_answer_invariant");
            }
            let answer = latest(db, task, "ANSWER_SUBMITTED")?
                .ok_or(CoreError::Refused("completion_answer_missing"))?
                .1;
            if answer["text"] != json!(synthesis) {
                return refuse("completion_answer_missing");
            }
        }
        for (_, r, assignment) in nodes {
            let v: Value = serde_json::from_str(&r.unwrap())?;
            if let Some(checks) = v["checks"].as_array() {
                let channel: Option<String> = db.query_row(
                    "SELECT c.channel_id FROM trusted_channels c JOIN delegation_provisioning p ON p.task_id=c.task_id AND p.attempt_id=c.attempt_id WHERE c.task_id=? AND p.idempotency_key=? ORDER BY c.rowid DESC LIMIT 1",
                    (task,assignment.as_deref()), |r|r.get(0)).optional()?;
                let mut runs = match channel {
                    Some(channel) => super::worker_checks::load_runs(db, task, &channel)?,
                    None => Vec::new(),
                };
                super::worker_checks::observe_outputs(db, task, &mut runs, &snapshot.artifacts)?;
                super::worker_checks::accept_recorded_bindings(db, task, checks, &mut runs)?;
                let current =
                    super::worker_checks::evaluate(checks, &runs, v["project_digest"].as_str())?;
                for (check, now) in checks.iter().zip(&current) {
                    if super::worker_checks::resolved(check) {
                        if !super::worker_checks::resolved(now)
                            || ["closed_by", "retracted_by", "superseded_by"]
                                .iter()
                                .any(|k| check[*k] != now[*k])
                        {
                            return refuse("completion_unresolved_worker_check");
                        }
                        continue;
                    }
                    if matches!(check["outcome"].as_str(), Some("not_run" | "unverified")) {
                        if !super::reconciliation::disclosed_gap(db, task, check)? {
                            return refuse("completion_unresolved_worker_check");
                        }
                        continue;
                    }
                    if check["outcome"] != "passed" {
                        return refuse("completion_unresolved_worker_check");
                    }
                    if now["outcome"] != "passed" {
                        return refuse("completion_worker_check_unverified");
                    }
                    let reference = check["tool_call_ref"]
                        .as_str()
                        .ok_or(CoreError::Refused("completion_worker_check_unverified"))?;
                    let receipt: Option<String> = db.query_row("SELECT CASE WHEN r.operation='outside_command' THEN json_extract(r.result_json,'$.reply.structuredContent') ELSE r.result_json END FROM runtime_requests r
                        JOIN trusted_channels c ON c.channel_id=r.actor
                        JOIN delegation_provisioning p ON p.task_id=c.task_id AND p.attempt_id=c.attempt_id
                        WHERE r.task_id=? AND p.idempotency_key=? AND r.operation IN ('command_run','outside_command')
                        AND coalesce(json_extract(r.result_json,'$.tool_call_ref'),json_extract(r.result_json,'$.reply.structuredContent.tool_call_ref'))=?",
                        (task, assignment.as_deref(), reference), |r| r.get(0)).optional()?;
                    let receipt: Value = serde_json::from_str(
                        &receipt.ok_or(CoreError::Refused("completion_worker_check_unverified"))?,
                    )?;
                    // A worker's `checks[].command` is descriptive text, never proof: the
                    // cited receipt's own argv is the run. Submission accepts the check on
                    // that receipt alone, so completion never refuses a verified pass because
                    // the description differs (for example an abbreviated `python3 -c "..."`);
                    // older stores retain such text in their node results.
                    // A work-set binding was re-established above from the retained
                    // observation (`accept_recorded_bindings`); the checkout digest of
                    // such a run may be absent (the checkout changed elsewhere).
                    let work_set = now["revision_binding"]["kind"] == "work_set"
                        && now["revision_binding"] == check["revision_binding"];
                    if receipt["complete"] != true
                        || receipt["exit_code"] != 0
                        || !receipt["failure"].is_null()
                        || (check["evidence_strength"] == "verified"
                            && (receipt["revision_digest"] != check["revision_digest"]
                                || (!work_set
                                    && receipt["revision_digest"]
                                        .as_str()
                                        .is_none_or(|s| s.len() != 64))))
                        || serde_json::from_value::<Vec<String>>(receipt["argv"].clone())
                            .map(|a| trivial_command(&a))
                            .unwrap_or(true)
                        || (receipt["output_truncated"] == true
                            && receipt["large_output_id"].is_null())
                    {
                        return refuse("completion_worker_check_unverified");
                    }
                    if let Some(id) = receipt["large_output_id"].as_str() {
                        let hash:Option<String>=db.query_row("SELECT source_ref FROM large_outputs WHERE task_id=? AND id=? AND truncated=0",
                            (task,id),|r|r.get(0)).optional()?;
                        if hash.is_none() || snapshot.artifacts.get(id) != hash.as_ref() {
                            return refuse("completion_worker_output_changed");
                        }
                    }
                }
            }
            let report = v["report_ref"]
                .as_str()
                .ok_or(CoreError::Refused("completion_artifact_changed_or_missing"))?;
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM reports WHERE task_id=? AND id=?)",
                (task, report),
                |r| r.get(0),
            )?;
            if !exists
                || snapshot.artifacts.get(report).map(String::as_str) != v["report_digest"].as_str()
                || v["report_digest"].as_str().is_none()
            {
                return refuse("completion_artifact_changed_or_missing");
            }
        }
        if !outstanding_commands(db, task, true)?.is_empty() {
            return refuse("completion_dropped_failed_check");
        }
        let (review_id, reviewed) = review_binding_tx(db, task)?;
        let latest: Option<(i64, String, String)> = db
            .query_row(
                "SELECT id,type,payload_json FROM runtime_events WHERE task_id=?
            AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (id, kind, s) = latest.ok_or(CoreError::Refused("completion_verification_missing"))?;
        let v: Value = serde_json::from_str(&s)?;
        if kind != "VERIFICATION_PASSED"
            || id <= review_id
            || v["revision"] != revision
            || v["review_event"] != review_id
        {
            return refuse("completion_verification_missing");
        }
        if v["revision_digest"] != reviewed || bound_digest_tx(db, task, snapshot)? != reviewed {
            return refuse("completion_project_revision_changed");
        }
        super::repair_checks::require_prior_worker_repairs(db, task, revision, &v)?;
    }
    domain::state_tx(db, task, TaskState::Completed, None)
}
