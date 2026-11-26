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
    pub checkout_evidence: Option<Value>,
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
pub(super) fn tag_checkout(mut payload: Value, snapshot: &EvidenceSnapshot) -> Value {
    if let Some(binding) = &snapshot.checkout_evidence {
        payload["checkout"] = binding["checkout"].clone();
        payload["cwd"] = binding["cwd"].clone();
    }
    payload
}
fn require_checkout(db: &rusqlite::Connection, task: &str, snapshot: &EvidenceSnapshot) -> CoreResult<()> {
    if !crate::workspace::worktrees::receipt_matches(
        db, task, snapshot.checkout_evidence.as_ref().unwrap_or(&Value::Null), true
    )? {
        return refuse("worktree_binding_invalid");
    }
    Ok(())
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
    require_checkout(db, task, snapshot)?;
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
    require_checkout(db, task, snapshot)?;
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
/// The program a launcher-prefixed command finally runs (`timeout 900 cargo ...`
/// runs `cargo`) when that is not `argv[0]` itself, so the verifier's availability
/// probe can resolve it on the same search path as the launcher (a bare `cargo`
/// behind `timeout` had exited 127 in verification). `None` without a launcher, or
/// when the prefix sets `PATH` itself.
pub fn launched_program(argv: &[String]) -> Option<&str> {
    let words = command_words(argv);
    let program = words.first()?;
    let prefix = &argv[..argv.len() - words.len()];
    (!prefix.is_empty() && !prefix.iter().any(|w| w.starts_with("PATH=")))
        .then_some(program.as_str())
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
/// A `grep`/`rg` run in quiet mode (`-q`, `--quiet`, `--silent`, or a short-option
/// cluster with `q`) with a pattern: it prints nothing and exits nonzero when the
/// pattern is absent, so its status is an assertion. Help, version and file-listing
/// requests never are.
fn quiet_search(args: &[String]) -> bool {
    let mut quiet = false;
    let mut operand = false;
    for a in args {
        match a.as_str() {
            "--help" | "--version" | "-V" | "--files" | "--type-list" => return false,
            "--quiet" | "--silent" => quiet = true,
            "-e" | "--regexp" | "-f" | "--file" => operand = true,
            s if s.starts_with("--") => {}
            s if s.starts_with('-') && s.len() > 1 => {
                // Only the flag letters before an attached value (`-qe alpha`, `-m1`).
                let flags: String = s[1..].chars().take_while(|c| c.is_ascii_alphabetic()).collect();
                quiet |= flags.contains('q');
                operand |= flags.ends_with('e') || flags.ends_with('f');
            }
            _ => operand = true,
        }
    }
    quiet && operand
}
/// Whether a command cannot verify a check: its exit status is not proven to come
/// from a nontrivial command (see `command_words`, `trivial_script`). A script file
/// is an ordinary project check, not an inline program.
pub fn trivial_command(argv: &[String]) -> bool {
    let w = command_words(argv);
    let Some(head) = w.first() else { return true };
    let program = base(head);
    // A file test or a quiet search asserts: its exit status is the check itself
    // (`test -f notes/a.txt`, `[ -s out ]`, `grep -q alpha notes/a.txt`). A bare
    // `test`, a printing search (`rg needle src`) or a help/version request does not.
    match program {
        "test" => return w.len() < 2,
        "[" => return w.len() < 3 || w.last().map(String::as_str) != Some("]"),
        "grep" | "egrep" | "fgrep" | "rg" => return !quiet_search(&w[1..]),
        _ => {}
    }
    if [
        ":", "ag", "basename", "cat", "cd", "date", "df", "dirname", "du", "echo", "env",
        "export", "fd", "file", "find", "head", "hostname", "id", "less", "ls", "more",
        "printenv", "printf", "pwd", "readlink", "realpath", "set", "sleep", "sort", "stat",
        "tail", "tee", "tr", "tree", "true", "type", "uname", "uniq", "wc", "which",
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
    /// 2026-11-25 Dev qualification: a plan for "create notes/a.txt containing alpha"
    /// was refused because `test -f` and `grep -q` counted as checking nothing. A file
    /// test or a quiet search with a pattern asserts; printing searches do not.
    #[test]
    fn file_tests_and_quiet_searches_are_assertions() {
        let words = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for argv in [
            &["test", "-f", "notes/a.txt"][..],
            &["/usr/bin/test", "-s", "out"],
            &["[", "-d", "dist", "]"],
            &["grep", "-q", "alpha", "notes/a.txt"],
            &["grep", "-qx", "alpha", "notes/a.txt"],
            &["grep", "--quiet", "-F", "alpha", "notes/a.txt"],
            &["grep", "-qe", "alpha", "notes/a.txt"],
            &["timeout", "10", "rg", "-q", "BETA", "notes/b.txt"],
        ] {
            assert!(!trivial_command(&words(argv)), "{argv:?}");
            // The inline-script grammar has no unquoted brackets.
            if argv[0] != "[" {
                let script = argv.join(" ");
                assert!(!trivial_command(&words(&["sh", "-c", &script])), "{script}");
            }
        }
        for argv in [
            &["test"][..],
            &["[", "-f", "x"],
            &["[", "]"],
            &["grep", "alpha", "notes/a.txt"],
            &["grep", "-q"],
            &["grep", "-q", "--version"],
            &["rg", "-q", "--files"],
            &["grep", "-c", "alpha", "notes/a.txt"],
        ] {
            assert!(trivial_command(&words(argv)), "{argv:?}");
        }
        assert!(trivial_command(&words(&["sh", "-c", "grep -q a f || true"])));
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
    require_checkout(db, task, snapshot)?;
    let d = digest(snapshot)?;
    event_tx(
        db,
        task,
        "EXECUTION_BASELINE",
        tag_checkout(json!({"revision":revision,"kind":"observed","revision_digest":d,"head":snapshot.head,
            "ignored_digest":snapshot.ignored_digest,
            "metadata_only":snapshot.metadata_only,"cap_reached":snapshot.cap_reached,"incomplete":snapshot.incomplete}), snapshot),
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
        VALUES (?,?,?,'worker-code-reviewer','Independently review the objective, implementation and retained reports against the original goal: the review packet goal lists it verbatim with the owner messages and the plan outcomes, and an approved stage that leaves an outcome undelivered is not the goal. Use the review packet change_boundary: inspect task-written paths across plans and relevant baseline changes. Check the current bytes when a write hash differs or is unavailable. Report any material coverage gap.',?,'[\"**\"]','review','pending')",
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
        tag_checkout(json!({"assignment_ref":assignment,"revision_digest":revision_digest,"binding":binding}), snapshot),
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
///
/// A required file whose exact bytes the task's review lineage already settled as
/// unread (`settled_unread_tx`) is not a missing item either: it is disclosed with
/// the approval as a file the reviewer did not read, never required again.
pub fn review_coverage_missing_tx(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
    result: &Value,
) -> CoreResult<Vec<Value>> {
    let settled = settled_unread_tx(db, task)?;
    Ok(coverage_gaps(db, task, assignment, result)?
        .into_iter()
        .filter(|(_, sha)| !sha.as_ref().is_some_and(|key| settled.contains(key)))
        .map(|(missing, _)| missing)
        .collect())
}
/// Every missing item of `review_coverage_missing_tx`, each with the
/// `(path, current_sha256)` it names when it is a listed file.
fn coverage_gaps(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
    result: &Value,
) -> CoreResult<Vec<(Value, Option<(String, String)>)>> {
    let Some(packet) = review_packet_of(db, task, assignment)? else {
        return Ok(Vec::new());
    };
    let incomplete = result["verdict"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case("INCOMPLETE"));
    let blocked = super::review::blocked_items(result);
    let changed = &packet["changed_files"];
    let sha_of = |path: &str| {
        changed["files"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|f| f["path"] == path)
            .and_then(|f| f["current_sha256"].as_str())
            .map(|sha| (path.to_owned(), sha.to_owned()))
    };
    let mut missing = Vec::new();
    if !incomplete {
        if packet["read_scope_covers_change"] != true {
            for path in packet["outside_read_scope"]
                .as_array()
                .into_iter()
                .flatten()
            {
                missing.push((
                    json!({"path":path,"reason":"outside reviewer read scope"}),
                    path.as_str().and_then(sha_of),
                ));
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
            match file_coverage(db, task, assignment, channel.as_deref(), file)? {
                FileCoverage::Covered | FileCoverage::NoText => {}
                FileCoverage::Missing(reason) => missing.push((
                    json!({"path":path,"reason":reason}),
                    file["current_sha256"]
                        .as_str()
                        .map(|sha| (path.to_owned(), sha.to_owned())),
                )),
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
/// `bulk` is text too large to require a complete read: generated data or a file
/// whose required lines span more than about eight `project_read` pages.
const NO_TEXT: [&str; 4] = ["binary", "image", "opaque", "bulk"];
fn file_coverage(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
    channel: Option<&str>,
    file: &Value,
) -> CoreResult<FileCoverage> {
    let no_text = file["review_content"]
        .as_str()
        .is_some_and(|k| NO_TEXT.contains(&k));
    // Bytes an earlier completed review of this task already read in full are
    // not read again: a later round reviews only what changed since.
    if let (Some(path), Some(sha)) = (file["path"].as_str(), file["current_sha256"].as_str()) {
        if reviewed_before(db, task, assignment, path, sha)? {
            return Ok(FileCoverage::Covered);
        }
    }
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
/// Whether an earlier completed review of `task` (not `assignment` itself) read
/// `path` at exactly `sha256` completely, or read every line of its own packet's
/// `review_lines` for that file. Its reviewer saw these exact bytes, so a later
/// review round need not read them again; any change produces new bytes and needs
/// a fresh read.
fn reviewed_before(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
    path: &str,
    sha256: &str,
) -> CoreResult<bool> {
    let key = format!("file-read:{}", crate::digest(path.as_bytes()));
    let reads = db
        .prepare(
            "SELECT n.assignment_ref, r.result_json FROM runtime_nodes n
             JOIN trusted_channels c ON c.task_id=n.task_id AND c.assignment_id=n.assignment_ref
             JOIN runtime_requests r ON r.task_id=n.task_id AND r.actor=c.channel_id
               AND r.request_key=? AND r.operation='native_file_read'
             WHERE n.task_id=? AND n.stage='review' AND n.status='completed'
               AND n.assignment_ref IS NOT NULL AND n.assignment_ref<>?",
        )?
        .query_map((&key, task, assignment), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (earlier, raw) in reads {
        let Ok(read) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if read["sha256"] != sha256 || read["content"] == "binary" {
            continue;
        }
        if read["complete"] == true {
            return Ok(true);
        }
        let ranges = review_packet_of(db, task, &earlier)?
            .and_then(|p| {
                p["changed_files"]["files"]
                    .as_array()?
                    .iter()
                    .find(|f| f["path"] == path && f["current_sha256"] == sha256)
                    .map(|f| f["review_lines"].clone())
            })
            .unwrap_or(Value::Null);
        if review_lines_read(&ranges, &read["lines"]) {
            return Ok(true);
        }
    }
    Ok(false)
}
/// Marks each listed changed file an earlier completed review of this task
/// already read at its current bytes (`reviewed_before`), so the reviewer of
/// `assignment` reads only what changed since the last review round.
pub fn mark_reviewed_before(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
    packet: &mut Value,
) -> CoreResult<()> {
    let mut marked = 0usize;
    if let Some(files) = packet["changed_files"]["files"].as_array_mut() {
        for file in files {
            let (Some(path), Some(sha)) = (file["path"].as_str(), file["current_sha256"].as_str())
            else {
                continue;
            };
            let seen = reviewed_before(db, task, assignment, path, sha)?;
            if seen {
                marked += 1;
                file["reviewed_before"] = json!(true);
            } else if let Some(entry) = file.as_object_mut() {
                entry.remove("reviewed_before");
            }
        }
    }
    if marked > 0 {
        packet["changed_files"]["reviewed_before_guidance"] = json!(
            "A file with reviewed_before: true was already read at these exact bytes by an earlier completed review of this task, so you need not read it again; read it only for context. Read the other files (or their review_lines)."
        );
    } else if let Some(changed) = packet["changed_files"].as_object_mut() {
        changed.remove("reviewed_before_guidance");
    }
    Ok(())
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
    // A packet with an incomplete record still requires only the task's own changes;
    // its unattributed files were listed as context (the packet's `attribution.note`
    // says so), and only required files beyond the list are a limit.
    let incomplete = changed["attribution"]["reason"].is_string();
    let beyond = if attributed || incomplete {
        &changed["more"]["required"]
    } else {
        &changed["more"]["count"]
    };
    if let Some(more) = beyond.as_i64().filter(|n| *n > 0) {
        limits.push(json!({"name":"Changed files beyond the review list","outcome":"unverified",
            "blocker":format!("{more} changed files beyond the review list were not read individually by the reviewer; inspect the folders named in the review packet."),
            "source":"runtime"}));
    }
    {
        // Unread after this assignment's own refusals, or settled for these exact
        // bytes by the review lineage (earlier assignments and revisions included).
        let own = coverage_settled(db, task, assignment)?;
        let settled = settled_unread_tx(db, task)?;
        let unread: Vec<String> = coverage_gaps(db, task, assignment, &json!({}))?
            .into_iter()
            .filter(|(_, sha)| own || sha.as_ref().is_some_and(|key| settled.contains(key)))
            .filter_map(|(m, _)| m["path"].as_str().map(str::to_owned))
            .collect();
        if !unread.is_empty() {
            let shown = unread
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let more = unread.len().saturating_sub(10);
            limits.push(json!({"name":"Changed files the reviewer did not read","outcome":"unverified",
                "blocker":format!("{} changed files were still unread after repeated coverage refusals (in this review or an earlier review of the same bytes) and are not reviewed: {shown}{}; inspect them directly.",
                    unread.len(), if more > 0 { format!(" and {more} more") } else { String::new() }),
                "paths":unread.iter().take(50).collect::<Vec<_>>(),"source":"runtime"}));
        }
    }
    let channel = review_channel(db, task, assignment)?;
    let mut unreviewable = Vec::new();
    let mut bulk = Vec::new();
    for file in changed["files"].as_array().into_iter().flatten() {
        if file["change"] == "deleted"
            || !file["current_sha256"].is_string()
            || file["required"] == false
        {
            continue;
        }
        if let FileCoverage::NoText = file_coverage(db, task, assignment, channel.as_deref(), file)? {
            let path = file["path"].as_str().map(str::to_owned);
            if file["review_content"] == "bulk" {
                bulk.extend(path);
            } else {
                unreviewable.extend(path);
            }
        }
    }
    if !bulk.is_empty() {
        let shown = bulk.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
        let more = bulk.len().saturating_sub(10);
        limits.push(json!({"name":"Changed files too large to require a complete read","outcome":"unverified",
            "blocker":format!("{} changed files are generated data or longer than about eight project_read pages, so the review did not have to read them completely, and they were not reviewed in full: {shown}{}; inspect them directly.",
                bulk.len(), if more > 0 { format!(" and {more} more") } else { String::new() }),
            "paths":bulk.iter().take(50).collect::<Vec<_>>(),"source":"runtime"}));
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
/// How many times a review result may be refused for unread required files before
/// the runtime records it anyway, with every file it still did not read disclosed as
/// an `unverified` limit (owner store 2026-11-21: one review was refused five times
/// for 32 small generated SVG files and the task waited on the owner). A refusal is
/// a chance to read, never a loop.
const COVERAGE_REFUSALS: i64 = 2;
fn coverage_settled(db: &rusqlite::Connection, task: &str, assignment: &str) -> CoreResult<bool> {
    let refused: i64 = db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='WORKER_RESULT_REFUSED'
           AND json_extract(payload_json,'$.assignment_ref')=?
           AND json_extract(payload_json,'$.code')='review_file_coverage_required'",
        (task, assignment),
        |r| r.get(0),
    )?;
    Ok(refused >= COVERAGE_REFUSALS)
}
/// The changed files, by `(path, sha256)` of their exact bytes, that the task's
/// review lineage already settled as unread since the task was last reopened: a
/// file named in `COVERAGE_REFUSALS` coverage refusals of any review assignment
/// (reassignments, later review cycles and later plan revisions count together,
/// so a fresh assignment does not start the count again), or one an approval
/// already disclosed under `Changed files the reviewer did not read`. Such bytes
/// are never required again; changed bytes are a new file to read (owner store
/// 2026-11-21: three review assignments were refused twice each for the same 32
/// files).
pub fn settled_unread_tx(
    db: &rusqlite::Connection,
    task: &str,
) -> CoreResult<BTreeSet<(String, String)>> {
    let rows = db
        .prepare(
            "SELECT type,payload_json FROM runtime_events WHERE task_id=?1
             AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)
             AND (type='WORKER_RESULT_REFUSED' AND json_extract(payload_json,'$.code')='review_file_coverage_required'
                  OR type='REVIEW_CHANGE_SET_LIMITED')
             ORDER BY id",
        )?
        .query_map([task], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut packets: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut counts: BTreeMap<(String, String), i64> = BTreeMap::new();
    let mut settled = BTreeSet::new();
    for (kind, raw) in rows {
        let v: Value = serde_json::from_str(&raw)?;
        let Some(assignment) = v["assignment_ref"].as_str() else {
            continue;
        };
        if !packets.contains_key(assignment) {
            let files = review_packet_of(db, task, assignment)?
                .map(|p| {
                    p["changed_files"]["files"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|f| {
                            Some((
                                f["path"].as_str()?.to_owned(),
                                f["current_sha256"].as_str()?.to_owned(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            packets.insert(assignment.to_owned(), files);
        }
        let files = &packets[assignment];
        let paths: Vec<&str> = if kind == "WORKER_RESULT_REFUSED" {
            // Each item is `<path> (<reason>)` (see `outstanding_tx`).
            v["missing"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|item| {
                    item.strip_suffix(')')
                        .and_then(|s| s.rsplit_once(" ("))
                        .map(|(path, _)| path)
                })
                .collect()
        } else {
            v["limits"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|l| l["name"] == "Changed files the reviewer did not read")
                .flat_map(|l| l["paths"].as_array().into_iter().flatten())
                .filter_map(Value::as_str)
                .collect()
        };
        for path in paths {
            let Some(sha) = files.get(path) else {
                continue;
            };
            let key = (path.to_owned(), sha.clone());
            let count = counts.entry(key.clone()).or_default();
            *count += 1;
            if kind != "WORKER_RESULT_REFUSED" || *count >= COVERAGE_REFUSALS {
                settled.insert(key);
            }
        }
    }
    Ok(settled)
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
        // A worker whose objective is only partly done names what remains, and may
        // submit that only at a dead end: as a failed assignment with its blocker,
        // which returns the remaining work to the coordinator's next plan.
        if result["objective_status"] == "partial" {
            let remaining: Vec<String> = result["remaining"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| r.as_str().filter(|r| !r.trim().is_empty()).map(str::to_owned))
                .collect();
            if remaining.is_empty() || result["assignment_outcome"] != "failed" {
                return Ok(Some(("objective_partial_blocker_required", remaining)));
            }
        }
        return Ok(None);
    }
    // An approval reports, for every goal outcome of the plan, whether the reviewed
    // work delivers it (`goal_coverage`). A sprint plan review judges a plan.
    if result["verdict"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case("APPROVE"))
        && !super::sprint_planning::is_plan_review(
            &db.query_row(
                "SELECT node_id FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                (task, assignment),
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default(),
        )
    {
        let missing = goal_coverage_missing(db, task, result)?;
        if !missing.is_empty() {
            return Ok(Some(("review_goal_coverage_required", missing)));
        }
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
    if !missing.is_empty() && !coverage_settled(db, task, assignment)? {
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

/// The plan's goal outcomes an approval's `goal_coverage` does not answer exactly
/// once (and ids it names that the plan does not have). Empty for a plan accepted
/// before plans named outcomes.
pub fn goal_coverage_missing(
    db: &rusqlite::Connection,
    task: &str,
    result: &Value,
) -> CoreResult<Vec<String>> {
    let Some(planned) = plan::task_outcomes(db, task)? else {
        return Ok(Vec::new());
    };
    let coverage = result["goal_coverage"].as_array().cloned().unwrap_or_default();
    let mut missing = Vec::new();
    for outcome in &planned {
        let answers = coverage
            .iter()
            .filter(|c| c["id"] == outcome["id"] && c["delivered"].is_boolean())
            .count();
        if answers != 1 {
            missing.push(format!(
                "{}: {}",
                outcome["id"].as_str().unwrap_or(""),
                outcome["statement"].as_str().unwrap_or("").chars().take(200).collect::<String>()
            ));
        }
    }
    for c in &coverage {
        if !planned.iter().any(|o| o["id"] == c["id"]) {
            missing.push(format!("{} (not an outcome of the plan)", c["id"].as_str().unwrap_or("?")));
        }
    }
    Ok(missing)
}
/// The plan outcomes an approval reports as not delivered.
pub fn undelivered_outcomes(db: &rusqlite::Connection, task: &str, result: &Value) -> CoreResult<Vec<Value>> {
    let Some(planned) = plan::task_outcomes(db, task)? else {
        return Ok(Vec::new());
    };
    Ok(planned
        .into_iter()
        .filter(|o| {
            !result["goal_coverage"]
                .as_array()
                .is_some_and(|c| c.iter().any(|g| g["id"] == o["id"] && g["delivered"] == true))
        })
        .map(|o| json!({"id":o["id"],"statement":o["statement"]}))
        .collect())
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
    // A partly done objective (only with its dead end, `outstanding_tx`) is a node
    // that did not succeed: the coordinator plans what remains.
    let failed = result["assignment_outcome"] == "failed"
        || result["objective_status"] == "partial"
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
        result = tag_checkout(result, snapshot);
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
    if failed && stage == "execution" {
        super::obstacles::classify_failed_tx(db, task)?;
    }
    Ok(())
}
pub(super) fn review_binding_tx(db: &Transaction<'_>, task: &str) -> CoreResult<(i64, String)> {
    let (_, revision) = domain::task_tx(db, task)?;
    let review: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT status,result_json FROM runtime_nodes
        WHERE task_id=? AND revision=? AND stage='review'
        AND substr(node_id,1,24)!='_sprint_revision_review_' ORDER BY rowid DESC LIMIT 1",
            (task, revision),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((status, result)) = review {
        let v: Value = serde_json::from_str(&result.unwrap_or_default())
            .map_err(|_| CoreError::Refused("completion_review_missing"))?;
        // An INCOMPLETE review never binds by itself. Only the owner's own decision to
        // close its unreachable parts as is (an answered owner question about exactly
        // those parts) lets it bind like an approval, with the parts disclosed as
        // unverified limits (`REVIEW_LIMIT_DISCLOSED`, `owner_authorized`).
        let disclosed = v["verdict"] == "INCOMPLETE"
            && v["review_limit_disclosed"] == true
            && v["review_limit_owner_authorized"] == true;
        if status != "completed" || !(v["verdict"] == "APPROVE" || disclosed) {
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
    let mut value = json!({"argv":argv,"complete":observed.complete,"exit_code":observed.exit_code,
        "receipt_ref":observed.receipt_ref,"revision":revision,"revision_digest":reviewed,"owner_epoch":owner.epoch,"review_event":review_event});
    // Why an incomplete or failing run failed (a timeout, a signal, an unavailable
    // sandbox), from the runtime's own retained output receipt of the same run;
    // without one the cause stays unknown (null), never invented.
    let raw: Option<String> = db
        .query_row(
            "SELECT result_json FROM runtime_requests
             WHERE task_id=? AND actor='native-verifier-output' AND operation='native_command_output'
             AND json_extract(result_json,'$.tool_call_ref')=? LIMIT 1",
            (task, &observed.receipt_ref),
            |r| r.get(0),
        )
        .optional()?;
    let raw: Value = raw.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or(Value::Null);
    if !crate::workspace::worktrees::receipt_matches(db, task, &raw, true)? {
        return refuse("verification_checkout_mismatch");
    }
    let cause = &raw["observation"];
    if let Some(binding) = raw.get("checkout") {
        if raw["task"] != task || raw["owner_epoch"] != owner.epoch || raw["argv"] != json!(argv)
            || raw["revision_digest"] != reviewed || raw["profile"] != "native-readonly-bubblewrap"
            || cause["complete"] != observed.complete || cause["exit_code"] != json!(observed.exit_code) {
            return refuse("verification_checkout_mismatch");
        }
        value["checkout"] = binding.clone();
        value["task"] = json!(task);
        value["cwd"] = raw["cwd"].clone();
    }
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
/// Why a runtime verification command's observed outcome says nothing about the
/// reviewed work: the verifier's environment failed it, so the runtime reruns the
/// identical command itself (`VERIFICATION_RERUN`) and, if it fails the same way
/// again, waits (`VERIFICATION_ENVIRONMENT_WAIT`) instead of replanning. `None`
/// for a real outcome: a complete run that passed, or a nonzero exit with complete
/// output, which stays a failure.
///
/// Environmental: an incomplete run (output that never reached its end, a launch
/// or sandbox that failed, a process killed from outside before the verifier's own
/// deadline); exit 126 or 127 (the program could not be executed or found where
/// the verifier looked); a full disk or exceeded quota; and a failure of a command
/// a worker of the task ran identically and passed on the same checkout
/// (`worker_passed`: the verifier's environment differs, for example a loopback
/// service its sandbox cannot reach). Real: exceeding the verifier's own deadline
/// (`command_timeout`), output past the retained limit (`command_output_limit`),
/// a signal, and the owner stopping the command.
pub fn verifier_environment(observation: &Value, worker_passed: bool) -> Option<&'static str> {
    let failure = observation["failure"].as_str();
    if matches!(
        failure,
        Some("command_output_limit" | "command_timeout" | "command_signalled" | "owner_stopped")
    ) {
        return None;
    }
    if observation["complete"] != true {
        return Some(match failure {
            Some("command_output_unavailable") => "output_unavailable",
            Some(
                "native_verification_sandbox_unavailable" | "native_command_sandbox_unavailable",
            ) => "sandbox_unavailable",
            Some("native_effect_unavailable") => "launch_unavailable",
            Some("command_wait_failed") => "wait_failed",
            _ => "incomplete",
        });
    }
    let code = observation["exit_code"].as_i64()?;
    match code {
        0 => return None,
        126 => return Some("not_executable"),
        127 => return Some("program_not_found"),
        _ => {}
    }
    let tail = |k: &str| {
        let s = observation[k].as_str().unwrap_or("");
        let mut start = s.len().saturating_sub(64 * 1024);
        while !s.is_char_boundary(start) {
            start += 1;
        }
        s[start..].to_owned()
    };
    let output = format!("{}\n{}", tail("stdout"), tail("stderr"));
    if output.contains("No space left on device") || output.contains("ENOSPC") {
        return Some("disk_full");
    }
    if output.contains("Disk quota exceeded") || output.contains("EDQUOT") {
        return Some("disk_quota");
    }
    if output.contains("native_command_sandbox_unavailable")
        || output.contains("native_verification_sandbox_unavailable")
    {
        return Some("sandbox_unavailable");
    }
    if worker_passed {
        let loopback = ["127.0.0.1", "localhost", "::1"]
            .iter()
            .any(|h| output.contains(h));
        let refused = output.contains("ECONNREFUSED") || output.contains("Connection refused");
        return Some(if refused && loopback {
            "network_refused"
        } else {
            "environment_difference"
        });
    }
    None
}
/// Whether a worker of the task ran exactly `argv` and it passed (complete, exit 0,
/// no failure) on one of `digests` (the checkout the verifier ran in, or the
/// review binding): the same command on the same bytes, so a verifier failure of it
/// is the verifier's environment, not the work.
pub fn worker_passed_tx(
    db: &rusqlite::Connection,
    task: &str,
    argv: &[String],
    digests: &[&str],
    verifier: &Value,
) -> CoreResult<bool> {
    if !crate::workspace::worktrees::receipt_matches(db, task, verifier, true)? {
        return Ok(false);
    }
    let argv = serde_json::to_string(argv)?;
    let checkout = verifier.get("checkout").map(Value::to_string);
    for digest in digests.iter().filter(|d| !d.is_empty()) {
        let passed: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM (SELECT CASE WHEN operation='outside_command'
                 THEN json_extract(result_json,'$.reply.structuredContent') ELSE result_json END AS r
               FROM runtime_requests WHERE task_id=? AND operation IN ('command_run','outside_command'))
             WHERE json_valid(r) AND json_extract(r,'$.argv')=json(?) AND json_extract(r,'$.complete')=1
               AND json_extract(r,'$.exit_code')=0 AND json_extract(r,'$.failure') IS NULL
               AND json_extract(r,'$.revision_digest')=?
                AND (?4 IS NULL OR (json_extract(r,'$.checkout')=json(?4) AND json_extract(r,'$.cwd')=?5)))",
            (task, &argv, digest, checkout.as_deref(), verifier["cwd"].as_str()),
            |r| r.get(0),
        )?;
        if passed {
            return Ok(true);
        }
    }
    Ok(false)
}
/// Delays between the runtime's own rechecks of a verification its environment
/// failed twice: 1, 2, 5 and 10 minutes. No model is called. If the 10-minute recheck
/// still fails the same environmental way, the runtime asks the owner one question
/// instead of waiting longer (`record_verification_wait_tx`).
pub const VERIFICATION_WAIT_BACKOFF: [u64; 4] = [60, 120, 300, 600];
/// The verification environment wait in force for the task's current revision, if
/// any: the latest `VERIFICATION_ENVIRONMENT_WAIT` not followed by a verification
/// outcome, a revised procedure, a new plan, a new review or a reopen. Its payload
/// carries `retry_not_before` (Unix seconds) and `attempt`.
pub fn verification_wait_tx(db: &rusqlite::Connection, task: &str) -> CoreResult<Option<Value>> {
    let (state, revision): (String, i64) = db.query_row(
        "SELECT state,revision FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if state != "VERIFYING" {
        return Ok(None);
    }
    let last: Option<(i64, String, String)> = db
        .query_row(
            "SELECT id,type,payload_json FROM runtime_events WHERE task_id=? AND type IN
             ('VERIFICATION_ENVIRONMENT_WAIT','VERIFICATION_PASSED','VERIFICATION_FAILED','VERIFICATION_REVISED',
              'PLAN_ACCEPTED','REVIEW_STARTED','TASK_REOPENED') ORDER BY id DESC LIMIT 1",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((id, kind, raw)) = last else {
        return Ok(None);
    };
    if kind != "VERIFICATION_ENVIRONMENT_WAIT" {
        return Ok(None);
    }
    let mut wait: Value = serde_json::from_str(&raw)?;
    if wait["revision"] != revision {
        return Ok(None);
    }
    wait["id"] = json!(id);
    Ok(Some(wait))
}
/// Records that the runtime itself reruns the identical verification command once,
/// with no model call, because its first outcome was environmental (`reason`).
pub fn record_verification_rerun_tx(
    db: &Transaction<'_>,
    task: &str,
    argv: &Value,
    reason: &str,
    receipt_ref: &Value,
) -> CoreResult<()> {
    let (_, revision) = domain::task_tx(db, task)?;
    event_tx(
        db,
        task,
        "VERIFICATION_RERUN",
        json!({"revision":revision,"argv":argv,"reason":reason,"first_receipt":receipt_ref}),
    )?;
    Ok(())
}
/// The verification of the current revision failed the same environmental way
/// after the runtime's own rerun: the task stays VERIFYING as the runtime's own
/// wait (no replan, no ladder rung, no model call) and the runtime runs the
/// verification again after the next backoff step. The receipts stay as retained
/// diagnostics; nothing is recorded as passed.
pub fn record_verification_wait_tx(
    db: &Transaction<'_>,
    task: &str,
    argv: &Value,
    reason: &str,
    receipts: &Value,
    now: f64,
) -> CoreResult<Value> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::Verifying {
        return refuse("verification_not_expected");
    }
    let attempt = verification_wait_tx(db, task)?
        .and_then(|w| w["attempt"].as_i64())
        .unwrap_or(0)
        + 1;
    let key = super::loop_guard::key("verifier_environment", "", &format!("{argv}|{reason}"));
    let mut wait = json!({"revision":revision,"argv":argv,"reason":reason,"receipts":receipts,
        "attempt":attempt,"class":"verifier_environment","key":key});
    let delay = VERIFICATION_WAIT_BACKOFF.get(attempt as usize - 1).copied();
    if let Some(delay) = delay {
        wait["retry_after_s"] = json!(delay);
        wait["retry_not_before"] = json!(now + delay as f64);
    }
    let id = event_tx(db, task, "VERIFICATION_ENVIRONMENT_WAIT", wait.clone())?;
    if attempt >= 2 {
        event_tx(
            db,
            task,
            "LOOP_OBSERVED",
            json!({"key":key,"kind":"verifier_environment","node":"","count":attempt}),
        )?;
    }
    if delay.is_none() {
        wait["owner_question"] = json!(verification_environment_question_tx(
            db, task, revision, argv, reason, &key, id
        )?);
        db.execute(
            "UPDATE runtime_events SET payload_json=? WHERE id=?",
            (wait.to_string(), id),
        )?;
    }
    Ok(wait)
}
/// What an environmental verification outcome means, in plain words.
fn environment_words(reason: &str, ru: bool) -> &'static str {
    match (reason, ru) {
        ("output_unavailable", false) => "its output never reached its end",
        ("output_unavailable", true) => "её вывод так и не дошёл до конца",
        ("sandbox_unavailable", false) => "the verifier's sandbox could not start",
        ("sandbox_unavailable", true) => "песочница проверки не запускается",
        ("launch_unavailable", false) => "it could not be started",
        ("launch_unavailable", true) => "её не удаётся запустить",
        ("wait_failed", false) => "its end could not be observed",
        ("wait_failed", true) => "не удаётся увидеть её завершение",
        ("not_executable", false) => "the program cannot be executed there",
        ("not_executable", true) => "программу там нельзя запустить",
        ("program_not_found", false) => "the program is not found there",
        ("program_not_found", true) => "программа там не найдена",
        ("disk_full", false) => "the disk is full",
        ("disk_full", true) => "диск заполнен",
        ("disk_quota", false) => "the disk quota is exceeded",
        ("disk_quota", true) => "превышена дисковая квота",
        ("network_refused", false) => "a local service it needs refuses the connection inside the verifier's sandbox, while the same command passed for a worker on the same files",
        ("network_refused", true) => "нужный ей локальный сервис отказывает в соединении внутри песочницы проверки, хотя у исполнителя та же команда на тех же файлах прошла",
        ("environment_difference", false) => "it fails there while the same command passed for a worker on the same files",
        ("environment_difference", true) => "там она падает, хотя у исполнителя та же команда на тех же файлах прошла",
        (_, false) => "it was stopped before it finished",
        (_, true) => "её остановили до завершения",
    }
}
/// The runtime's one question (no model call) when its own reruns of a verification
/// command at 1, 2, 5 and 10 minutes all failed the same environmental way: it names
/// the command and the reason in the task's language and offers the loop question's
/// answers (`automatic::LOOP_OPTIONS`): retry now, another approach (the coordinator
/// changes the verification procedure), accept as not verified (a disclosed limit,
/// never a pass) or retry later. Returns the question id.
fn verification_environment_question_tx(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    argv: &Value,
    reason: &str,
    key: &str,
    wait_event: i64,
) -> CoreResult<String> {
    let history = super::loop_guard::history(db, task, key, false)?;
    let attempt: i64 = db.query_row(
        "SELECT count(*)+1 FROM runtime_events WHERE task_id=?1 AND type='LOOP_BROKEN'
           AND json_extract(payload_json,'$.key')=?2 AND json_extract(payload_json,'$.action')='ask_owner'
           AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)",
        (task, key),
        |r| r.get(0),
    )?;
    let ru = domain::task_language_tx(db, task)? == "ru";
    let name = super::automatic::task_name(db, task)?;
    let command: String = argv
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect();
    let words = environment_words(reason, ru);
    let again = if attempt > 1 {
        if ru {
            format!("Попытка {attempt}. ")
        } else {
            format!("Attempt {attempt}. ")
        }
    } else {
        String::new()
    };
    let text = if ru {
        format!("{again}Задача «{name}»: итоговая проверка `{command}` не может выполниться в среде проверки: {words}. Runtime сам повторил её через 1, 2, 5 и 10 минут с тем же результатом; ошибка в самой работе не установлена. Как продолжить?")
    } else {
        format!("{again}Task \"{name}\": the final check `{command}` cannot run in the verifier's environment: {words}. The runtime itself ran it again after 1, 2, 5 and 10 minutes with the same result; nothing shows the work itself is at fault. How should it continue?")
    };
    let details = json!({"class":"verifier_environment","reason":"verifier_environment","revision":revision,
        "strategy":"ask_owner","loop_key":key,"switch_coordinator":false,"reports":[],
        "events":[{"id":wait_event,"type":"VERIFICATION_ENVIRONMENT_WAIT"}],
        "verification_environment":{"argv":argv,"reason":reason}});
    super::automatic::runtime_question_tx(
        db,
        task,
        revision,
        "verifier_environment",
        &history,
        &[command.clone(), reason.to_owned()],
        details,
        &text,
        attempt,
        ru,
    )?;
    let question: String = db.query_row(
        "SELECT json_extract(payload_json,'$.owner_question') FROM runtime_events WHERE task_id=?
         AND type='AUTOMATIC_CONTINUATION' ORDER BY id DESC LIMIT 1",
        [task],
        |r| r.get(0),
    )?;
    Ok(question)
}
/// Settles every verification run of the current revision that started and never
/// finished (a restart or crash mid-run): its outcome is recorded as unknown
/// (`settled_by: runtime_reverify`), never as passed or failed, and the runtime
/// reruns the verification itself (`VERIFICATION_RERUN`, reason `outcome_unknown`)
/// instead of refusing every later verification of the revision. Returns how many
/// runs it settled. Only the serialized driver calls this, so no run can still be
/// in progress.
pub fn settle_interrupted_verification_tx(db: &Transaction<'_>, task: &str) -> CoreResult<usize> {
    let (_, revision) = domain::task_tx(db, task)?;
    let runs = db
        .prepare(
            "SELECT request_key FROM runtime_requests i WHERE task_id=?1 AND operation='native_verification_intent'
             AND json_extract(result_json,'$.revision')=?2 AND NOT EXISTS(SELECT 1 FROM runtime_requests f
                WHERE f.task_id=i.task_id AND f.actor='native-verifier-finish' AND f.request_key=i.request_key)
             ORDER BY rowid",
        )?
        .query_map((task, revision), |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if runs.is_empty() {
        return Ok(0);
    }
    let plan: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let commands = plan
        .and_then(|p| serde_json::from_str::<Value>(&p).ok())
        .map(|p| p["verification"].clone())
        .unwrap_or(Value::Null);
    for run in &runs {
        let settled = json!({"settled":true,"outcome":"unknown","settled_by":"runtime_reverify","revision":revision});
        request_tx(
            db,
            task,
            "native-verifier-finish",
            run,
            "native_verification_finished",
            &json!({"settled_by":"runtime_reverify"}),
            || Ok(settled.clone()),
        )?;
        event_tx(
            db,
            task,
            "VERIFICATION_RERUN",
            json!({"revision":revision,"argv":commands,"reason":"outcome_unknown","intent":run,
                "outcome":"unknown","settled_by":"runtime_reverify"}),
        )?;
    }
    Ok(runs.len())
}
/// Normalized identity of a failed verification: each failing command's argv, exit
/// code and a digest of the normalized end of its retained output (lowercased,
/// digits, hex runs and absolute paths masked, whitespace collapsed, last 2 KB), so
/// the same failure repeated on new runs has the same key.
/// Per failed verification command, a bounded excerpt of its retained output naming
/// what failed: failing steps, panics with their next lines, assertion values,
/// compiler errors and test failures, at most [`DIAGNOSTIC_LINES`] lines and
/// [`DIAGNOSTIC_CHARS`] characters each. The full output stays the runtime's own
/// record, which no worker channel can read; without this excerpt every failed
/// verification cost a plan revision that only rediscovered the failing test
/// (owner store 2026-11-22).
pub fn verification_diagnostics(
    db: &rusqlite::Connection,
    task: &str,
    checks: &[Value],
) -> CoreResult<Vec<Value>> {
    let mut out = Vec::new();
    for check in checks {
        if check["complete"] == true && check["exit_code"] == 0 {
            continue;
        }
        let output: Option<String> = db
            .query_row(
                "SELECT coalesce(json_extract(result_json,'$.observation.test_fast_failures'),'')||char(10)||
                        coalesce(json_extract(result_json,'$.observation.stdout'),'')||char(10)||
                        coalesce(json_extract(result_json,'$.observation.stderr'),'')
                 FROM runtime_requests WHERE task_id=? AND actor='native-verifier-output'
                 AND operation='native_command_output' AND json_extract(result_json,'$.tool_call_ref')=? LIMIT 1",
                (task, check["receipt_ref"].as_str().unwrap_or("")),
                |r| r.get(0),
            )
            .optional()?;
        let excerpt = failure_excerpt(output.as_deref().unwrap_or(""));
        if excerpt.is_empty() {
            continue;
        }
        out.push(json!({"argv":check["argv"],"exit_code":check["exit_code"],"failure":check["failure"],"excerpt":excerpt}));
    }
    Ok(out)
}
const DIAGNOSTIC_LINES: usize = 40;
const DIAGNOSTIC_CHARS: usize = 4000;

/// Recognize a bounded test-fast stage and exit record.
pub fn test_fast_failure_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("test-fast: step=") else {
        return false;
    };
    let Some((index, rest)) = rest.split_once(" stage=") else {
        return false;
    };
    let Some((stage, code)) = rest.split_once(" exit=") else {
        return false;
    };
    index.parse::<usize>().is_ok_and(|n| n > 0)
        && code.parse::<u8>().is_ok_and(|n| n > 0)
        && (stage == "[unlabelled]"
            || (!stage.is_empty()
                && stage.len() <= 80
                && stage
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')))
}

/// The lines of a command's output that say what failed (see
/// [`verification_diagnostics`]); empty when nothing matches.
pub fn failure_excerpt(text: &str) -> String {
    let marks = [
        "panicked at",
        "assertion",
        "left:",
        "right:",
        "error[",
        "error:",
        " failed",
        "FAILED",
        "failures:",
        "Error:",
        "AssertionError",
        "FAIL ",
        "✗",
        "×",
        "timed out",
    ];
    let lines: Vec<&str> = text.lines().collect();
    let mut stages = Vec::new();
    let mut seen_stages = BTreeSet::new();
    let mut keep = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        if test_fast_failure_line(line) {
            if seen_stages.insert(*line) {
                stages.push(i);
            }
            continue;
        }
        if line.starts_with("test result: ok.") {
            continue;
        }
        let failing_step =
            line.starts_with("Step ") && line.contains(": exit=") && !line.contains(": exit=0,");
        if failing_step || marks.iter().any(|m| line.contains(m)) {
            keep.insert(i);
            // A panic's message follows on the next lines.
            if line.contains("panicked at") {
                for j in i + 1..(i + 4).min(lines.len()) {
                    keep.insert(j);
                }
            }
        }
    }
    let mut excerpt = String::new();
    for i in stages.into_iter().chain(keep).take(DIAGNOSTIC_LINES) {
        let line: String = lines[i].chars().take(300).collect();
        if excerpt.chars().count() + line.chars().count() + 1 > DIAGNOSTIC_CHARS {
            break;
        }
        excerpt.push_str(&line);
        excerpt.push('\n');
    }
    excerpt
}
#[cfg(test)]
mod failure_excerpt_tests {
    use super::*;

    #[test]
    fn late_stage_failure_survives_excerpt_limits() {
        let marker = "test-fast: step=28 stage=paseo-display exit=23";
        let timeout = "test-fast: step=29 stage=remote exit=124";
        let output = format!(
            "{}{}test-fast: step=27 stage=ui exit=0\n\
             test-fast: step=28 stage=/private/path exit=23\n\
             {marker}\n{timeout}\n{marker}\n",
            "test result: ok. 35 passed; 0 failed; 0 ignored\n".repeat(100),
            format!("error: {}\n", "é".repeat(400)).repeat(100),
        );
        let excerpt = failure_excerpt(&output);
        assert!(excerpt.starts_with(&format!("{marker}\n{timeout}\nerror: ")));
        // Two stage records plus twelve 300-character error lines fit in 4,000 characters.
        assert_eq!(excerpt.lines().count(), 14);
        assert_eq!(excerpt.lines().nth(2).unwrap().chars().count(), 300);
        assert!(excerpt.chars().count() <= DIAGNOSTIC_CHARS);
        assert!(!excerpt.contains("0 failed"));
        assert!(!excerpt.contains("/private/path"));
        assert!(!excerpt.contains("exit=0"));
    }

    #[test]
    fn verification_uses_stage_records_from_its_exact_receipt() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE runtime_requests(task_id, actor, operation, result_json)")
            .unwrap();
        let marker = "test-fast: step=28 stage=paseo-display exit=23";
        let observation = json!({
            "tool_call_ref": "exact-receipt",
            "observation": {
                "stdout": "test result: ok. 35 passed; 0 failed\n".repeat(100),
                "stderr": "error: synthetic context\n",
                "test_fast_failures": format!("{marker}\n")
            }
        });
        db.execute(
            "INSERT INTO runtime_requests VALUES ('task', 'native-verifier-output', 'native_command_output', ?)",
            [observation.to_string()],
        )
        .unwrap();
        let checks = [json!({
            "argv": ["scripts/cortex-xtask.sh", "test-fast"],
            "complete": false,
            "exit_code": 1,
            "failure": "command_output_limit",
            "receipt_ref": "exact-receipt"
        })];
        let diagnostics = verification_diagnostics(&db, "task", &checks).unwrap();
        assert_eq!(diagnostics[0]["exit_code"], 1);
        assert_eq!(diagnostics[0]["failure"], "command_output_limit");
        assert_eq!(
            diagnostics[0]["excerpt"],
            format!("{marker}\nerror: synthetic context\n")
        );
        assert!(verification_diagnostics(&db, "other-task", &checks)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn passing_summaries_do_not_mask_a_legacy_failure() {
        let output = format!(
            "{}test result: FAILED. 0 passed; 1 failed\nStep 2: exit=101, 2.0s\n",
            "test result: ok. 2 passed; 0 failed\n".repeat(100),
        );
        assert_eq!(
            failure_excerpt(&output),
            "test result: FAILED. 0 passed; 1 failed\nStep 2: exit=101, 2.0s\n"
        );
    }
}

fn verification_failure_key(
    db: &Transaction<'_>,
    task: &str,
    checks: &[Value],
) -> CoreResult<String> {
    let mut failing = Vec::new();
    for check in checks {
        if check["complete"] == true && check["exit_code"] == 0 {
            continue;
        }
        let output: Option<String> = db
            .query_row(
                "SELECT coalesce(json_extract(result_json,'$.observation.stdout'),'')||char(10)||
                        coalesce(json_extract(result_json,'$.observation.stderr'),'')
                 FROM runtime_requests WHERE task_id=? AND actor='native-verifier-output'
                 AND operation='native_command_output' AND json_extract(result_json,'$.tool_call_ref')=? LIMIT 1",
                (task, check["receipt_ref"].as_str().unwrap_or("")),
                |r| r.get(0),
            )
            .optional()?;
        let tail = normalized_tail(output.as_deref().unwrap_or(""), 2048);
        failing.push(json!([
            check["argv"],
            check["exit_code"],
            check["failure"],
            crate::digest(tail.as_bytes())
        ]));
    }
    Ok(crate::digest(json!(failing).to_string().as_bytes()))
}
fn normalized_tail(text: &str, limit: usize) -> String {
    let mut out = String::new();
    for word in text.split_whitespace() {
        let word = word.to_lowercase();
        let masked: String = if word.starts_with('/') {
            "/p".into()
        } else {
            let mut m = String::new();
            let mut run = String::new();
            let flush = |run: &mut String, m: &mut String| {
                if run.len() >= 6 {
                    m.push('#');
                } else {
                    m.extend(
                        run.chars()
                            .map(|c| if c.is_ascii_digit() { '0' } else { c }),
                    );
                }
                run.clear();
            };
            for c in word.chars() {
                if c.is_ascii_hexdigit() {
                    run.push(c);
                } else {
                    flush(&mut run, &mut m);
                    m.push(c);
                }
            }
            flush(&mut run, &mut m);
            m
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&masked);
    }
    let mut start = out.len().saturating_sub(limit);
    while !out.is_char_boundary(start) {
        start += 1;
    }
    out[start..].to_owned()
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
            && commands.get(index) == Some(&v["argv"])
            && crate::workspace::worktrees::receipt_matches(db, task, &v, true)?
            && (v.get("checkout").is_none() || v["task"] == task);
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
    if !passed {
        let checks = verification["checks"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        verification["failure_key"] = json!(verification_failure_key(db, task, &checks)?);
        let diagnostics = verification_diagnostics(db, task, &checks)?;
        if !diagnostics.is_empty() {
            verification["diagnostics"] = json!(diagnostics);
        }
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
        // Each rung of the strategy ladder gets its own direct repairs: past them the
        // stop is a continuation that changes approach (escalated routes, the senior
        // consultant, another role), never a stop for the owner.
        let failures: i64 = db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=?1 AND type='VERIFICATION_FAILED'
             AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1
                 AND type IN ('AUTOMATIC_RESUMED','TASK_REOPENED')),0)",
            [task],
            |r| r.get(0),
        )?;
        // Every failure carries its normalized identity (`failure_key`): the strategy
        // ladder's per-fingerprint rung choice and backoff (`automatic::stop_tx`) see
        // the same failure repeated across rungs; a rung's own direct repairs stay.
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
        || domain::owner_rework_needs_plan(db, task)?
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
    db: &rusqlite::Connection,
    task: &str,
    include_retained: bool,
) -> CoreResult<Vec<Value>> {
    let mut stmt = db.prepare("SELECT id,type,payload_json FROM runtime_events WHERE task_id=?
        AND type IN ('VERIFICATION_FAILED','VERIFICATION_REVISED','VERIFICATION_PASSED','USER_QUESTION_REQUESTED','USER_ANSWER_RECEIVED','TASK_REOPENED') ORDER BY id")?;
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
            // Explicit owner rework starts a new cycle: checks owed by the closed one do
            // not bind its first plan.
            "TASK_REOPENED" => {
                if v["origin"] == "owner_rework" {
                    outstanding.clear();
                }
            }
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

/// Failed or dropped verification commands still owed, less those the owner accepted
/// as disclosed unverified limits after a loop question
/// (`automatic::owner_accepted_commands`): a repair plan may leave those out and
/// they do not hold completion, but they stay gaps of the final result
/// (`reconciliation::gaps` reads their `LOOP_BROKEN.limits`) and are never a pass.
pub(super) fn unaccepted_commands(db: &rusqlite::Connection, task: &str) -> CoreResult<Vec<Value>> {
    let accepted = super::automatic::owner_accepted_commands(db, task)?;
    Ok(outstanding_commands(db, task, true)?
        .into_iter()
        .filter(|argv| !accepted.contains(argv))
        .collect())
}
/// Coordinator turns that carry the task's retained-state capsule, whose
/// constraints list every owner message the former runtime stored as a
/// `task_events` row: a turn of one of these kinds after a message delivered it.
const CAPSULE_TURNS: &str = "'PLAN_ACCEPTED','NATIVE_COORDINATOR_CONTINUED','SYNTHESIS_SUBMITTED','REPLAN_REQUEST_ACCEPTED'";
/// Owner messages and clarifications the former runtime stored as `task_events`
/// rows were never consumed by this runtime, which records owner messages as
/// `USER_STEERING` and lists those rows in every coordinator's capsule instead.
/// One that a later coordinator turn carried (`CAPSULE_TURNS`) has been delivered:
/// it is marked consumed (`runtime:recovery_capsule`), so it no longer holds
/// completion (an AI-Frontier store held a task's completion on a message from
/// 2026-10-05 through three passed verifications). One no coordinator has seen yet
/// still holds it and is named by `pending_input_tx`.
pub fn settle_delivered_legacy_input_tx(db: &Transaction<'_>, task: &str) -> CoreResult<usize> {
    Ok(db.execute(
        &format!(
            "UPDATE task_events SET consumed_by='runtime:recovery_capsule',delivered_at=coalesce(delivered_at,?2)
             WHERE task_id=?1 AND event_type IN ('user_message','coordinator_clarification')
             AND consumed_by IS NULL AND superseded=0 AND EXISTS(SELECT 1 FROM runtime_events e
                WHERE e.task_id=task_events.task_id AND e.type IN ({CAPSULE_TURNS})
                AND julianday(e.created_at)>julianday(task_events.created_at))"
        ),
        (task, crate::now()),
    )?)
}
/// What holds completion as `completion_pending_input`, each item named: open
/// questions, owner messages not yet applied or delivered, open waits and a former
/// runtime's owner message no coordinator turn has carried yet. Empty when nothing
/// does.
pub fn pending_input_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Vec<Value>> {
    let mut items = Vec::new();
    let questions = db
        .prepare("SELECT id FROM pending_questions WHERE task_id=? AND status='open'")?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for id in questions {
        items.push(json!({"kind":"open_question","id":id}));
    }
    let legacy = db
        .prepare(
            "SELECT id,event_type,created_at FROM task_events WHERE task_id=?
             AND event_type IN ('user_message','coordinator_clarification') AND consumed_by IS NULL AND superseded=0",
        )?
        .query_map([task], |r| {
            Ok(json!({"kind":"undelivered_message","source":"task_events","id":r.get::<_,i64>(0)?,
                "type":r.get::<_,String>(1)?,"recorded_at":r.get::<_,String>(2)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    items.extend(legacy);
    for message in super::steering::pending(db, task)? {
        items.push(json!({"kind":"pending_steering","message":message}));
    }
    for message in super::steering::undelivered(db, task)? {
        items.push(json!({"kind":"undelivered_steering","message":message}));
    }
    for wait in waits::projection_tx(db, task)? {
        if ["open", "ready", "resuming"].contains(&wait["status"].as_str().unwrap_or("")) {
            items.push(json!({"kind":"open_wait","id":wait["id"],"status":wait["status"]}));
        }
    }
    Ok(items)
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
    // The original goal decides completion, not only the stage the plan reached:
    // the final result reports every goal outcome done (or, on the answer path, not
    // done by the owner's own close-as-is decision).
    let source: Option<String> = db
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type IN (?,?) ORDER BY id DESC LIMIT 1",
            if state == TaskState::AnswerReady {
                (task, "ANSWER_SUBMITTED", "ANSWER_SUBMITTED")
            } else {
                (task, "SYNTHESIS_SUBMITTED", "SYNTHESIS_RUNTIME_COMPOSED")
            },
            |r| r.get(0),
        )
        .optional()?;
    if let Some(source) = source {
        let payload: Value = serde_json::from_str(&source)?;
        if !super::reconciliation::goal_met(db, task, &payload)? {
            return refuse("completion_goal_not_met");
        }
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
    settle_delivered_legacy_input_tx(db, task)?;
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
    // An owner change of a sprint member waits for its sprint impact verdict.
    if super::sprint_impact::pending(db, task)?.is_some() {
        return refuse("sprint_impact_pending");
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
        if !unaccepted_commands(db, task)?.is_empty() {
            return refuse("completion_dropped_failed_check");
        }
        let (review_id, reviewed) = review_binding_tx(db, task)?;
        let latest: Option<(i64, String, String)> = db
            .query_row(
                "SELECT id,type,payload_json FROM runtime_events WHERE task_id=?
            AND (type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED')
              OR type='VERIFICATION_REVISED' AND json_extract(payload_json,'$.accepted_unverified') IS NOT NULL)
            ORDER BY id DESC LIMIT 1",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (id, kind, s) = latest.ok_or(CoreError::Refused("completion_verification_missing"))?;
        let v: Value = serde_json::from_str(&s)?;
        // The owner accepted every command of the plan as not verified (an explicit
        // decision, like close as is): no passed verification exists or is needed, the
        // reviewed bytes must be unchanged, and the limits are disclosed as gaps.
        if kind == "VERIFICATION_REVISED" {
            let plan: Option<String> = db.query_row(
                "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
                [task],
                |r| r.get(0),
            )?;
            let none_left = plan
                .and_then(|p| serde_json::from_str::<Value>(&p).ok())
                .is_some_and(|p| p["verification"].as_array().is_some_and(|a| a.is_empty()));
            let accepted = super::automatic::owner_accepted_commands(db, task)?;
            let all_accepted = v["accepted_unverified"]
                .as_array()
                .is_some_and(|a| !a.is_empty() && a.iter().all(|argv| accepted.contains(argv)));
            if !none_left || !all_accepted || id <= review_id || v["revision"] != revision {
                return refuse("completion_verification_missing");
            }
            if bound_digest_tx(db, task, snapshot)? != reviewed {
                return refuse("completion_project_revision_changed");
            }
            super::repair_checks::require_prior_worker_repairs(db, task, revision, &v)?;
            return domain::state_tx(db, task, TaskState::Completed, None);
        }
        if kind != "VERIFICATION_PASSED"
            || id <= review_id
            || v["revision"] != revision
            || v["review_event"] != review_id
        {
            return refuse("completion_verification_missing");
        }
        for check in v["checks"].as_array().into_iter().flatten() {
            if !crate::workspace::worktrees::receipt_matches(db, task, check, true)?
                || (check.get("checkout").is_some() && check["task"] != task) {
                return refuse("verification_checkout_mismatch");
            }
        }
        if v["revision_digest"] != reviewed || bound_digest_tx(db, task, snapshot)? != reviewed {
            return refuse("completion_project_revision_changed");
        }
        super::repair_checks::require_prior_worker_repairs(db, task, revision, &v)?;
    }
    domain::state_tx(db, task, TaskState::Completed, None)
}
