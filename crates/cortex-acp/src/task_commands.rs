//! Python `acp.commands` grammar and `acp.agent._run_command_body` reply text
//! for the task-level commands. Store and lifecycle effects stay behind
//! `IncomingHost`; this module only parses arguments and words replies.
use regex::Regex;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

/// Python `commands.AVAILABLE_COMMANDS`, in its order and copy.
pub(crate) fn available() -> Value {
    json!([
        {"name":"cortex-clear","description":"Preview old project task deletion; trailing confirm deletes; force bypasses stale active protection except the invoking task. No model call.","input":{"hint":"<N|Nd|days N|before YYYY-MM-DD|YYYY-MM-DD> [force] [confirm]"}},
        {"name":"cortex-status","description":"Show model/thought-level, connection state, and engine execution mode. Never changes anything.","input":null},
        {"name":"cortex-model","description":"Select the model by its route label or route id.","input":{"hint":"<route label or route id>"}},
        {"name":"cortex-effort","description":"Select the thought_level (reasoning effort) value for the current model.","input":{"hint":"<value>"}},
        {"name":"cortex-setup","description":"Show onboarding guidance for connecting a model provider. Never changes anything, never prints a secret.","input":null},
        {"name":"cortex-new-task","description":"Start a brand-new, separate task without touching the current active task.","input":{"hint":"<task text>"}},
        {"name":"cortex-kill","description":"Stop one running worker command with a reason; the worker continues.","input":{"hint":"<command-ref> <reason>"}},
        {"name":"cortex-stop","description":"Urgently stop the active task: cancels every assignment, queued open, and open question. Does not restart through a later message.","input":null},
        {"name":"cortex-finish","description":"Close this task now at the user's request, without claiming unfinished review, verification, or synthesis passed. Cancels active work.","input":null},
        {"name":"cortex-answer","description":"Answer one specific open coordinator question by its listed number (use when more than one question is open).","input":{"hint":"<question-number> <answer text>"}},
        {"name":"cortex-language","description":"Set the language Cortex uses for its own status/question/stop lines: auto, en, or ru.","input":{"hint":"auto|en|ru"}},
        {"name":"harvest","description":"Start or continue an explicit knowledge harvest. Optional text narrows the documentation scope; bare command covers the project.","input":{"hint":"<optional scope>"}},
        {"name":"harvest-refresh","description":"Explicitly refresh project documentation from current source. Text is an optional scope.","input":{"hint":"<optional scope>"}},
        {"name":"skill-management","description":"Create, revise, or import a project skill. Use only when the user explicitly invokes this command.","input":{"hint":"<request>"}}
    ])
}

pub(crate) const CLEAR_USAGE: &str = "Usage: /cortex-clear <N|Nd|days N|before YYYY-MM-DD|YYYY-MM-DD> [force] [confirm] (N: 0..36500; date: non-future, 00:00 UTC). Omit confirm to preview.";
pub(crate) const CLEAR_BUSY: &str = "Cortex cleanup requires an idle session; no deletion started.";
pub(crate) const CLEAR_REFUSED: &str = "Cortex cleanup refused or incomplete: store busy, active-task conflict, or storage failure. No success is claimed; retry only after activity is idle and storage is healthy.";
/// This host's own truthful refusal: it previews retention but does not yet
/// delete task stores, sessions or reclaim space.
pub(crate) const CLEAR_DELETE_UNAVAILABLE: &str = "Cortex cleanup refused: this Rust ACP host previews retention but cannot delete tasks or sessions yet; no deletion started. Omit confirm to preview.";
pub(crate) const KILL_USAGE: &str = "Usage: /cortex-kill <command-ref> <reason>";
pub(crate) const KILL_REQUESTED: &str =
    "Stop requested. The runner will confirm whether it stopped the command; the worker continues.";
pub(crate) const FINISH_USAGE: &str = "Usage: /cortex-finish (no arguments)";
pub(crate) const STOP_NONE: &str = "There is no active task on this session to stop.";
pub(crate) const FINISH_NONE: &str = "There is no active task on this session to close.";
/// Python `messages.render(..., "en")`: these commands select the language
/// with `select_language(None, [])`, which is always English.
pub(crate) const STOP_CONFIRMED: &str =
    "Stopped this task: every assignment, queued open, and pending question was cancelled.";
pub(crate) const FINISH_CONFIRMED: &str = "Closed this task at your request. Active work was cancelled; unfinished review, verification, and synthesis are not certified as passed.";
pub(crate) const ANSWER_USAGE: &str = "Usage: /cortex-answer <question-number> <answer text>";
pub(crate) const ANSWER_NONE_OPEN: &str = "There is no open question to answer right now.";
pub(crate) const ANSWER_UNKNOWN_INDEX: &str =
    "No open question with that number. Use /cortex-status or check the open questions listed above.";
pub(crate) const NEW_TASK_USAGE: &str = "Usage: /cortex-new-task <task text>";
pub(crate) const SKILL_USAGE: &str = "Usage: /skill-management <request>";
pub(crate) const BUSY: &str = "A prompt is already in flight for this session.";
/// Python `_session_prompt` refusals of typed answers on a busy session.
pub(crate) const BUSY_ANSWER_USAGE: &str = "Use /cortex-answer <question-number> <answer text>.";
pub(crate) const BUSY_MULTIPLE: &str =
    "Multiple questions are open; use /cortex-answer <question-number> <answer text>.";

/// Python's typed card answer: a number naming one of the offered options
/// selects that option; any other text is the answer itself, unchanged.
pub(crate) fn typed_choice(text: &str, options: &[&str]) -> String {
    let value = text.trim_matches(space);
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        if let Some(option) = value
            .trim_start_matches('0')
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=options.len()).contains(n))
            .map(|n| options[n - 1])
        {
            return option.to_owned();
        }
    }
    text.to_owned()
}

/// Python `str.isspace()`.
fn space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}
/// Python `str.split()`.
fn words(s: &str) -> Vec<&str> {
    s.split(space).filter(|w| !w.is_empty()).collect()
}
/// Python `str.split(None, 1)` of an already stripped argument.
fn head(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start_matches(space);
    let at = s.find(space)?;
    let rest = s[at..].trim_start_matches(space);
    (!rest.is_empty()).then_some((&s[..at], rest))
}

/// Microseconds since the Unix epoch (UTC).
pub(crate) fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or_default()
}
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}
/// Python `datetime.isoformat(timespec="microseconds")` of a UTC instant.
pub(crate) fn iso(micros: i64) -> String {
    let (days, rest) = (
        micros.div_euclid(86_400_000_000),
        micros.rem_euclid(86_400_000_000),
    );
    let (y, m, d) = civil_from_days(days);
    let seconds = rest / 1_000_000;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:06}+00:00",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        rest % 1_000_000
    )
}
/// Python `datetime.strptime(value, "%Y-%m-%d")` range checks.
fn date(y: i64, m: i64, d: i64) -> Option<i64> {
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let last = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (y >= 1 && (1..=last).contains(&d)).then(|| days_from_civil(y, m, d) * 86_400_000_000)
}

pub(crate) struct Clear {
    pub cutoff: String,
    pub confirm: bool,
    pub force: bool,
    pub confirm_command: String,
}
/// Python `parse_clear_argument`: strict grammar, no user text echoed.
pub(crate) fn parse_clear(argument: &str, now: i64) -> Option<Clear> {
    let mut words = words(argument);
    let confirm = words.last() == Some(&"confirm");
    if confirm {
        words.pop();
    }
    let force = words.last() == Some(&"force");
    if force {
        words.pop();
    }
    let suffix = if force { " force" } else { "" };
    let value = words.join(" ");
    let days = Regex::new(r"^(?:days )?([0-9]{1,5})(d)?$").ok()?;
    if let Some(m) = days.captures(&value) {
        let n: i64 = m[1].parse().ok()?;
        if !(value.starts_with("days ") && m.get(2).is_some()) && n <= 36_500 {
            return Some(Clear {
                cutoff: iso(now - n * 86_400_000_000),
                confirm,
                force,
                confirm_command: format!("/cortex-clear days {n}{suffix} confirm"),
            });
        }
    }
    let before = Regex::new(r"^(?:before )?(([0-9]{4})-([0-9]{2})-([0-9]{2}))$").ok()?;
    let m = before.captures(&value)?;
    let part = |i: usize| m[i].parse::<i64>().ok();
    let cutoff = date(part(2)?, part(3)?, part(4)?)?;
    (cutoff <= now).then(|| Clear {
        cutoff: iso(cutoff),
        confirm,
        force,
        confirm_command: format!("/cortex-clear before {}{suffix} confirm", &m[1]),
    })
}
fn force_text(c: &Clear) -> &'static str {
    if c.force {
        "; force=true (assumes stale processes are dead; no OS processes killed)"
    } else {
        ""
    }
}
/// Python's dry-run reply.
pub(crate) fn dry_run(c: &Clear, tasks: u64, sessions: u64, skipped: u64) -> String {
    format!(
        "Dry run: cutoff={}; would_delete_tasks={tasks}; would_delete_orphan_sessions={sessions}; skipped_protected={skipped}{}\nConfirm: {}",
        c.cutoff,
        force_text(c),
        c.confirm_command
    )
}
/// Python's confirmed reply when nothing was removed (no vacuum is needed).
pub(crate) fn confirmed_nothing(c: &Clear, skipped: u64) -> String {
    format!(
        "cutoff={}; deleted_tasks=0; deleted_orphan_sessions=0; skipped_protected={skipped}{}; vacuum=not_needed; reclaimed_bytes=0",
        c.cutoff,
        force_text(c)
    )
}

/// Python `validate_stop` after `argument.split(None, 1)`: the stop request
/// `(ref, reason)`, or the exact refusal/usage reply.
pub(crate) fn kill(argument: &str) -> Result<(String, String), String> {
    let Some((reference, reason)) = head(argument) else {
        return Err(KILL_USAGE.into());
    };
    let valid_ref = Regex::new(r"^gateway:[0-9a-f]{32}$")
        .map(|r| r.is_match(reference))
        .unwrap_or(false);
    if !valid_ref {
        return Err(refused("command_stop_invalid_ref"));
    }
    let trimmed = reason.trim_matches(space);
    if trimmed.is_empty() || reason.chars().count() > 2000 {
        return Err(refused("command_stop_invalid_reason"));
    }
    Ok((reference.into(), trimmed.into()))
}
/// Python `f"Command stop refused: {error}. {error.correction or ''}".rstrip()`.
pub(crate) fn refused(code: &str) -> String {
    format!("Command stop refused: {code}.")
}
/// Python `parse_answer_argument`: a 1-based `(index, text)` pair.
pub(crate) fn answer(argument: &str) -> Option<(usize, String)> {
    let (number, text) = head(argument)?;
    if !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let trimmed = number.trim_start_matches('0');
    let index = if trimmed.len() > 18 {
        usize::MAX
    } else {
        trimmed.parse().unwrap_or(0)
    };
    let text = text.trim_matches(space);
    (index >= 1 && !text.is_empty()).then(|| (index, text.to_owned()))
}
/// Python `_run_harvest`: the explicit harvest request, with a complete goal
/// before the session has a task.
pub(crate) fn harvest(refresh: bool, argument: &str, language: &str, task: bool) -> String {
    let scope = argument.trim_matches(space);
    let shown = if scope.is_empty() {
        String::new()
    } else {
        format!(" ({scope})")
    };
    let instruction = match (refresh, language == "ru") {
        (false, false) => format!("The user explicitly requested a knowledge harvest{shown}. Read `knowledge-harvest-scope` with `read_skill` and carry it out for this task; this is the explicit request that skill requires, not an inference from anything said earlier."),
        (false, true) => format!("Пользователь явно запросил сбор знаний{shown}. Прочитай `knowledge-harvest-scope` через `read_skill` и выполни его для этой задачи: это и есть явный запрос, которого требует этот скилл, а не вывод из сказанного ранее."),
        (true, false) => format!("The user explicitly requested a documentation refresh{shown}. Read `knowledge-harvest-scope` with `read_skill` and update the project's source-backed documentation for this task; this is an explicit request, not an inference."),
        (true, true) => format!("Пользователь явно запросил обновление документации{shown}. Прочитай `knowledge-harvest-scope` через `read_skill` и обнови подтверждённую источниками документацию проекта для этой задачи: это явный запрос, а не вывод."),
    };
    if task {
        return instruction;
    }
    let goal = if !scope.is_empty() {
        scope
    } else if refresh {
        "Update the project's existing source-backed documentation"
    } else {
        "Create complete source-backed documentation for the project"
    };
    format!("{goal}\n\n{instruction}")
}
/// Python `_run_skill_management` with `skill_proposals.workflow_instruction`.
pub(crate) fn skill_management(argument: &str) -> Option<String> {
    let request = argument.trim_matches(space);
    (!request.is_empty()).then(|| format!("{request}\n\nThe user explicitly invoked /skill-management. Read the coordinator-only shared skill skill-management using read_skill before planning. Treat the supplied request as the complete skill-management request. This workflow is active only because the user invoked the registered command; ordinary prose does not activate it."))
}
/// Python `messages.render("new_task_created", "en", task_id=...)`.
pub(crate) fn new_task_created(task: &str) -> String {
    format!("Started a new, separate task ({task}); the active task is unaffected.")
}

#[cfg(test)]
mod tests {
    use super::*;
    const NOW: i64 = 1_763_164_800_000_000; // 2025-11-15T00:00:00Z
    #[test]
    fn clear_grammar_matches_python() {
        assert_eq!(iso(NOW), "2025-11-15T00:00:00.000000+00:00");
        assert_eq!(iso(NOW + 1_234_567), "2025-11-15T00:00:01.234567+00:00");
        let c = parse_clear("days 007 force confirm", NOW).unwrap();
        assert!(c.confirm && c.force);
        assert_eq!(c.confirm_command, "/cortex-clear days 7 force confirm");
        assert_eq!(c.cutoff, "2025-11-08T00:00:00.000000+00:00");
        assert_eq!(
            parse_clear("30d", NOW).unwrap().confirm_command,
            "/cortex-clear days 30 confirm"
        );
        let c = parse_clear("2024-02-29", NOW).unwrap();
        assert_eq!(c.cutoff, "2024-02-29T00:00:00.000000+00:00");
        assert_eq!(c.confirm_command, "/cortex-clear before 2024-02-29 confirm");
        for bad in [
            "",
            "days 5d",
            "36501",
            "123456",
            "2023-02-29",
            "0000-01-01",
            "2025-13-01",
            "2099-01-01",
            "confirm force",
            "٣",
            "before",
        ] {
            assert!(parse_clear(bad, NOW).is_none(), "{bad}");
        }
        assert_eq!(parse_clear("0", NOW).unwrap().cutoff, iso(NOW));
    }
    #[test]
    fn kill_answer_and_harvest_text_match_python() {
        assert_eq!(kill("only").unwrap_err(), KILL_USAGE);
        assert_eq!(
            kill("a b").unwrap_err(),
            "Command stop refused: command_stop_invalid_ref."
        );
        let r = format!("gateway:{}", "a".repeat(32));
        assert_eq!(
            kill(&format!("{r} {}", "x".repeat(2001))).unwrap_err(),
            "Command stop refused: command_stop_invalid_reason."
        );
        assert_eq!(
            kill(&format!("{r}\t two words")).unwrap(),
            (r.clone(), "two words".into())
        );
        assert_eq!(answer("2 yes please"), Some((2, "yes please".into())));
        assert_eq!(answer("0 x"), None);
        assert_eq!(answer("x y"), None);
        assert_eq!(answer("1"), None);
        assert!(harvest(true, "", "en", false)
            .starts_with("Update the project's existing source-backed documentation\n\n"));
        assert!(harvest(false, "api", "ru", true)
            .starts_with("Пользователь явно запросил сбор знаний (api)."));
        assert!(skill_management(" ").is_none());
    }
    #[test]
    fn typed_choice_matches_python_card_answers() {
        let options = ["Detailed", "Brief"];
        assert_eq!(typed_choice(" 2 ", &options), "Brief");
        assert_eq!(typed_choice("01", &options), "Detailed");
        assert_eq!(typed_choice("3", &options), "3");
        assert_eq!(typed_choice("0", &options), "0");
        assert_eq!(typed_choice(" free text ", &options), " free text ");
        assert_eq!(typed_choice("1", &[]), "1");
        assert_eq!(typed_choice(&"9".repeat(40), &options), "9".repeat(40));
    }
}
