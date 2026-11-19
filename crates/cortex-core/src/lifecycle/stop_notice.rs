//! The runtime's own stop question, explained from recorded facts. When the runtime
//! stops a task and asks the owner what to do, the question says what happened
//! (which step, what it needed, what prevented it), what each answer does, which
//! answer is recommended when one clearly is, and carries structured targets (a
//! blocking task, a review report, a denied path) the desktop can open. Nothing
//! here changes state; enforcement and answers are unchanged: the two answers stay
//! `domain::STOP_OPTIONS_*` (continue with the coordinator, or close as is).
use super::{domain, MAX_REVIEW_CYCLES, MAX_VERIFICATION_REPAIRS};
use crate::{scheduler, CoreResult};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};

/// Every blocker family the runtime stop question explains specifically. Any other
/// code gets the generic explanation, which names the code and says that nothing
/// more was recorded.
pub const KINDS: &[&str] = &[
    "review_blocked",
    "review_incomplete",
    "review_cycle_limit",
    "review_no_progress",
    "verification_failed",
    "verification_review_revision_changed",
    "verification_revision_unavailable",
    "review_revision_unavailable",
    "worker_failed",
    "worker_failure_not_retryable",
    "worker_retry_limit",
    "worker_stopped_by_owner",
    "no_runnable_nodes",
    "native_continuation_unavailable",
    "scheduler_capacity_timeout",
    "lost_worker_action_outcome_unknown",
    "lost_worker_",
    "completion_",
    "native_session_start_failed",
    "coordinator_session_start_failed",
    "native_progress_limit",
    "coordinator_no_progress_limit",
    "project_revision_unavailable",
    "project_unobservable_",
];

/// One answer of the stop question: its label (exactly as offered), what choosing
/// it does, and its action (`continue` or `finish`).
#[derive(Debug, Clone)]
pub struct Answer {
    pub label: &'static str,
    pub detail: String,
    pub action: &'static str,
}
#[derive(Debug, Clone)]
pub struct Notice {
    /// The complete question text, readable without any buttons.
    pub text: String,
    /// One line: what stopped the task.
    pub headline: String,
    /// What happened, from recorded facts.
    pub what: String,
    pub answers: Vec<Answer>,
    pub recommended: Option<usize>,
    /// `{"kind":"task","taskId","runtimeTaskId","title","state"}`,
    /// `{"kind":"report","reportId","role"}` or `{"kind":"rule","path"}`.
    pub targets: Vec<Value>,
}
impl Notice {
    /// The fields recorded on `USER_QUESTION_REQUESTED` beside its text and options.
    pub fn payload(&self) -> Value {
        json!({
            "headline": self.headline,
            "explanation": self.what,
            "option_details": self.answers.iter().map(|a| json!({"label":a.label,"detail":a.detail,"action":a.action})).collect::<Vec<_>>(),
            "recommended": self.recommended,
            "targets": self.targets,
        })
    }
}

fn short(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        flat
    } else {
        format!(
            "{}…",
            flat.chars().take(limit).collect::<String>().trim_end()
        )
    }
}
fn role_word(role: &str) -> &str {
    role.strip_prefix("worker-").unwrap_or(role)
}
/// "role: objective" of one step, shortened.
fn step(role: &str, objective: &str) -> String {
    format!("{}: {}", role_word(role), short(objective, 90))
}
fn root_tx(db: &Transaction<'_>, task: &str) -> CoreResult<String> {
    Ok(db
        .query_row(
            "SELECT coalesce((SELECT workspace_root FROM task_workspaces WHERE task_id=t.id),project_root) FROM tasks t WHERE id=?",
            [task],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .unwrap_or_default())
}
fn paths(values: &Value, root: &str, ru: bool) -> String {
    let all: Vec<String> = values
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|p| {
                    let p = if root.is_empty() {
                        p
                    } else {
                        p.strip_prefix(root)
                            .map(|s| s.trim_start_matches('/'))
                            .unwrap_or(p)
                    };
                    short(if p.is_empty() { "." } else { p }, 80)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut shown = all.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if all.len() > 3 {
        shown.push_str(&if ru {
            format!(" и ещё {}", all.len() - 3)
        } else {
            format!(" and {} more", all.len() - 3)
        });
    }
    shown
}
/// The workspace task bound to a runtime task (its id and a short title) and its state.
fn task_target(db: &Transaction<'_>, runtime: &str, ru: bool) -> CoreResult<(Value, String)> {
    let row: Option<(String, String)> = db
        .query_row(
            "SELECT t.task_id,t.description FROM workspace_tasks t JOIN workspace_executions e USING(task_id) WHERE e.runtime_task_id=?",
            [runtime],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let goal: Option<(String, String)> = db
        .query_row(
            "SELECT goal,state FROM runtime_tasks WHERE task_id=?",
            [runtime],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let state = goal.as_ref().map(|g| g.1.clone()).unwrap_or_default();
    let text = row
        .as_ref()
        .map(|r| r.1.clone())
        .or(goal.map(|g| g.0))
        .unwrap_or_default();
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let title = short(first, 60);
    let word = match (state.as_str(), ru) {
        ("EXECUTING", false) => "running",
        ("EXECUTING", true) => "выполняется",
        ("REVIEWING", false) => "in review",
        ("REVIEWING", true) => "на ревью",
        ("WAITING_USER", false) => "waiting for an answer",
        ("WAITING_USER", true) => "ждёт ответа",
        ("PAUSED", false) => "paused",
        ("PAUSED", true) => "на паузе",
        (_, false) => "not finished",
        (_, true) => "не завершена",
    };
    let target = json!({"kind":"task","taskId":row.map(|r| r.0),"runtimeTaskId":runtime,"title":title,"state":state});
    Ok((
        target,
        if ru {
            format!("«{title}» ({word})")
        } else {
            format!("“{title}” ({word})")
        },
    ))
}
/// The latest review node of the current revision: (role, summary, report, findings).
fn review_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Option<(String, Value)>> {
    let (_, revision) = domain::task_tx(db, task)?;
    let row: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT role,result_json FROM runtime_nodes WHERE task_id=? AND revision=? AND stage='review' ORDER BY rowid DESC LIMIT 1",
            (task, revision),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((role, Some(result))) => {
            Some((role, serde_json::from_str(&result).unwrap_or(Value::Null)))
        }
        Some((role, None)) => Some((role, Value::Null)),
        None => None,
    })
}
/// The most relevant settled step of the current revision for a worker stop:
/// a failed one first, then a cancelled one, then the latest assigned one.
fn worker_step_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Option<(String, String, Value)>> {
    let (_, revision) = domain::task_tx(db, task)?;
    let row: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT role,objective,result_json FROM runtime_nodes WHERE task_id=? AND revision=? AND assignment_ref IS NOT NULL
             ORDER BY CASE status WHEN 'failed' THEN 0 WHEN 'cancelled' THEN 1 ELSE 2 END, rowid DESC LIMIT 1",
            (task, revision),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(row.map(|(role, objective, result)| {
        (
            role,
            objective,
            result
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(Value::Null),
        )
    }))
}
fn report_target(result: &Value, role: &str) -> Option<Value> {
    result["report_ref"]
        .as_str()
        .filter(|r| !r.is_empty())
        .map(|r| json!({"kind":"report","reportId":r,"role":role_word(role)}))
}
fn times(n: i64, ru: bool) -> String {
    if !ru {
        return if n == 1 {
            "once".into()
        } else {
            format!("{n} times")
        };
    }
    let word = if (n % 100 / 10) != 1 && (2..=4).contains(&(n % 10)) {
        "раза"
    } else {
        "раз"
    };
    format!("{n} {word}")
}

/// What the stop question says for `blocker`, in the task's language.
pub fn explain_tx(db: &Transaction<'_>, task: &str, blocker: &str, ru: bool) -> CoreResult<Notice> {
    let l = |en: &str, r: &str| if ru { r.to_owned() } else { en.to_owned() };
    let mut targets = Vec::new();
    let mut recommended = Some(0);
    let mut finish = l(
        "the task closes as is: changes already made stay in the project, and the unfinished part stays unverified",
        "задача закрывается как есть: уже сделанные изменения остаются в проекте, незавершённая часть остаётся неподтверждённой",
    );
    let mut cont = l(
        "the coordinator gets the recorded facts and decides how to proceed (a new plan, a retry or a question to you)",
        "координатор получает записанные факты и решает, как действовать дальше (новый план, повтор или вопрос вам)",
    );
    let what: String = match blocker {
        "review_incomplete" => {
            let (role, result) = review_tx(db, task)?.unwrap_or(("worker-code-reviewer".into(), Value::Null));
            if let Some(t) = report_target(&result, &role) {
                targets.push(t);
            }
            // The blocker category the reviewer named, in plain words.
            let why = |kind: &str| -> &'static str {
                match (kind, ru) {
                    ("missing_access", false) => "needs access or a permission the reviewer does not have",
                    ("missing_access", true) => "нужен доступ или разрешение, которого у ревьюера нет",
                    ("owner_decision", false) => "needs your decision",
                    ("owner_decision", true) => "нужно ваше решение",
                    ("other_role_work", false) => "needs work by another role first",
                    ("other_role_work", true) => "сначала нужна работа другой роли",
                    ("tool_unavailable", false) => "a needed tool is unavailable",
                    ("tool_unavailable", true) => "нужный инструмент недоступен",
                    (_, false) => "no blocker named",
                    (_, true) => "причина не названа",
                }
            };
            let items: Vec<String> = result["not_covered"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .take(5)
                        .map(|i| format!("{} ({}: {})", short(i["item"].as_str().unwrap_or(""), 80),
                            why(i["blocker"].as_str().unwrap_or("")), short(i["reason"].as_str().unwrap_or(""), 120)))
                        .collect()
                })
                .unwrap_or_default();
            let more = result["not_covered"].as_array().map(Vec::len).unwrap_or(0).saturating_sub(items.len());
            recommended = None;
            let mut out = l(
                "The independent review reached parts of the change it cannot check itself, so the result is neither approved nor rejected. Another review of the same files would meet the same obstacle, so none was started.",
                "Независимое ревью дошло до частей изменения, которые не может проверить само, поэтому результат не принят и не отклонён. Повторное ревью тех же файлов упёрлось бы в то же препятствие, поэтому оно не запускалось.",
            );
            if items.is_empty() {
                out.push_str(&l(" The review named no confirmed problem and did not say what it left uncovered.", " Ревью не назвало подтверждённых проблем и не указало, что осталось непроверенным."));
            } else {
                let mut list = items.join("; ");
                if more > 0 {
                    list.push_str(&l(&format!("; and {more} more"), &format!("; и ещё {more}")));
                }
                out.push_str(&l(&format!(" Not covered: {list}."), &format!(" Не охвачено: {list}.")));
            }
            if let Some(report) = result["report_ref"].as_str() {
                out.push_str(&l(&format!(" The review report is {report}."), &format!(" Отчёт ревью — {report}.")));
            }
            cont = l(
                "the coordinator gets these obstacles and decides how to clear them (a plan step for another role, a question to you about the decision or access needed), after which the change is reviewed again",
                "координатор получает эти препятствия и решает, как их устранить (шаг плана для другой роли, вопрос вам о нужном решении или доступе), после чего изменение проверяется заново",
            );
            finish = l(
                "the task closes as is: changes already made stay in the project, and the result stays unreviewed and unverified",
                "задача закрывается как есть: сделанные изменения остаются в проекте, а результат остаётся непроверенным ревью и неподтверждённым",
            );
            out
        }
        "review_blocked" | "review_cycle_limit" | "review_no_progress" => {
            let (role, result) = review_tx(db, task)?.unwrap_or(("worker-code-reviewer".into(), Value::Null));
            let summary = result["summary"].as_str().map(|s| short(s, 320));
            let findings = result["findings"].as_array().map(Vec::len).unwrap_or(0);
            let fixes: Vec<String> = result["findings"]
                .as_array()
                .map(|f| f.iter().filter_map(|f| f["fix"].as_str()).take(3).map(|s| short(s, 100)).collect())
                .unwrap_or_default();
            if let Some(t) = report_target(&result, &role) {
                targets.push(t);
            }
            let report = result["report_ref"].as_str().unwrap_or("");
            let mut out = if blocker == "review_blocked" {
                l(
                    &format!("The independent reviewer ({}) did not accept the result (verdict BLOCK).", role_word(&role)),
                    &format!("Независимый ревьюер ({}) не принял результат (вердикт BLOCK).", role_word(&role)),
                )
            } else if blocker == "review_no_progress" {
                recommended = None;
                l(
                    "The last round of fixes made no progress: the review found the same problems again, or the files did not change since the previous review. Another identical fix and review round would repeat the same result, so none was started.",
                    "Последний раунд исправлений ничего не изменил: ревью снова нашло те же проблемы, или файлы не изменились с прошлого ревью. Ещё один такой же раунд исправлений и ревью дал бы тот же результат, поэтому он не запускался.",
                )
            } else {
                let cycles: i64 = db.query_row("SELECT review_cycle FROM runtime_tasks WHERE task_id=?", [task], |r| r.get(0))?;
                recommended = None;
                l(
                    &format!(
                        "The independent review asked for changes {}; that is the limit of {MAX_REVIEW_CYCLES} review cycles for one plan.",
                        times(cycles.max(1), false)
                    ),
                    &format!(
                        "Независимое ревью требовало исправлений {}; это предел в {MAX_REVIEW_CYCLES} цикла ревью для одного плана.",
                        times(cycles.max(1), true)
                    ),
                )
            };
            if let Some(s) = summary {
                out.push_str(&l(&format!(" The reviewer wrote: “{s}”"), &format!(" Ревьюер написал: «{s}»")));
            }
            if findings > 0 {
                out.push_str(&l(
                    &format!(" Findings: {findings} ({}).", fixes.join("; ")),
                    &format!(" Замечаний: {findings} ({}).", fixes.join("; ")),
                ));
            }
            if !report.is_empty() {
                out.push_str(&l(
                    &format!(" The full findings are in the review report {report}."),
                    &format!(" Все замечания — в отчёте ревью {report}."),
                ));
            }
            cont = l(
                "address the findings: the coordinator plans the fixes the review asked for, then the result is reviewed and checked again",
                "устранить замечания: координатор спланирует исправления по отчёту ревью, затем результат снова пройдёт ревью и проверки",
            );
            finish = l(
                "the task closes as is: changes already made stay in the project, the review findings stay unaddressed and the result stays unverified",
                "задача закрывается как есть: сделанные изменения остаются в проекте, замечания ревью остаются неисправленными, а результат — неподтверждённым",
            );
            out
        }
        "verification_failed" => {
            let failures: i64 =
                db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='VERIFICATION_FAILED'", [task], |r| r.get(0))?;
            let payload: Option<String> = db
                .query_row(
                    "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='VERIFICATION_FAILED' ORDER BY id DESC LIMIT 1",
                    [task],
                    |r| r.get(0),
                )
                .optional()?;
            let payload: Value = payload.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
            let failed: Vec<String> = payload["checks"]
                .as_array()
                .map(|c| {
                    c.iter()
                        .filter(|c| c["exit_code"].as_i64() != Some(0) || c["complete"] != true)
                        .take(3)
                        .map(|c| {
                            let argv = c["argv"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                            let code = c["exit_code"].as_i64().map(|c| c.to_string()).unwrap_or_else(|| l("no exit code", "без кода выхода"));
                            format!("`{}` ({code})", short(&argv, 80))
                        })
                        .collect()
                })
                .unwrap_or_default();
            cont = l(
                "the coordinator plans a repair of the failing check, then the result is reviewed and checked again",
                "координатор спланирует исправление непрошедшей проверки, затем результат снова пройдёт ревью и проверку",
            );
            let mut out = l(
                &format!(
                    "The runtime's final check failed {}, more than the {MAX_VERIFICATION_REPAIRS} automatic repairs allow.",
                    times(failures.max(1), false)
                ),
                &format!(
                    "Итоговая проверка runtime не прошла {} — больше, чем допускают {MAX_VERIFICATION_REPAIRS} автоматических исправления.",
                    times(failures.max(1), true)
                ),
            );
            if !failed.is_empty() {
                out.push_str(&l(
                    &format!(" Failing commands (exit code): {}.", failed.join(", ")),
                    &format!(" Непрошедшие команды (код выхода): {}.", failed.join(", ")),
                ));
            }
            out
        }
        "verification_review_revision_changed" | "verification_revision_unavailable" | "review_revision_unavailable" => {
            cont = l(
                "the coordinator has the current files reviewed and checked again",
                "координатор заново отправит текущие файлы на ревью и проверку",
            );
            if blocker == "verification_review_revision_changed" {
                l(
                    "The project files changed after the independent review, so the reviewed result no longer matches the files being checked.",
                    "Файлы проекта изменились после независимого ревью, поэтому проверенный результат больше не совпадает с проверяемыми файлами.",
                )
            } else {
                l(
                    "The runtime could not read the state of the project files to bind the review or the check to them.",
                    "Runtime не смог прочитать состояние файлов проекта, чтобы привязать к нему ревью или проверку.",
                )
            }
        }
        b if b.starts_with("worker_") || b.starts_with("lost_worker_") => {
            let found = worker_step_tx(db, task)?;
            let label = found.as_ref().map(|(r, o, _)| step(r, o)).unwrap_or_else(|| l("a planned step", "запланированный шаг"));
            let summary = found.as_ref().and_then(|f| f.2["summary"].as_str()).map(|s| short(s, 240));
            if let Some((role, _, result)) = &found {
                if let Some(t) = report_target(result, role) {
                    targets.push(t);
                }
            }
            match b {
                "worker_stopped_by_owner" => {
                    recommended = None;
                    cont = l(
                        "the coordinator decides how to finish without the stopped step, or plans it again",
                        "координатор решит, как завершить работу без остановленного шага, или запланирует его заново",
                    );
                    l(
                        &format!("You stopped the step “{label}”, and the remaining planned work depends on it or has nothing left to run."),
                        &format!("Вы остановили шаг «{label}», а оставшаяся запланированная работа зависит от него или выполнять больше нечего."),
                    )
                }
                "worker_retry_limit" => l(
                    &format!("The step “{label}” failed on every allowed retry."),
                    &format!("Шаг «{label}» завершался с ошибкой при каждом разрешённом повторе."),
                ),
                "lost_worker_action_outcome_unknown" => {
                    recommended = None;
                    cont = l(
                        "the coordinator continues knowing that the outcome of those actions is unknown; check the project files first if they matter",
                        "координатор продолжит с учётом того, что исход этих действий неизвестен; если это важно, сначала проверьте файлы проекта",
                    );
                    l(
                        &format!("The turn of the step “{label}” ended without an accepted result, and whether some of its commands or writes finished is not known."),
                        &format!("Ход шага «{label}» закончился без принятого результата, и неизвестно, завершились ли некоторые его команды или записи."),
                    )
                }
                b if b.starts_with("lost_worker_") => l(
                    &format!("The engine process of the step “{label}” was lost (it exited or stopped answering), and retrying did not bring it back."),
                    &format!("Процесс движка шага «{label}» был потерян (завершился или перестал отвечать), и повторы не помогли."),
                ),
                _ => {
                    let mut out = l(&format!("The step “{label}” failed."), &format!("Шаг «{label}» завершился с ошибкой."));
                    if let Some(s) = summary {
                        out.push_str(&l(&format!(" Its report says: “{s}”"), &format!(" В его отчёте: «{s}»")));
                    }
                    out
                }
            }
        }
        "no_runnable_nodes" | "native_continuation_unavailable" => {
            let now = chrono::Utc::now().timestamp_micros() as f64 / 1_000_000.0;
            let root = root_tx(db, task)?;
            let state = domain::task_tx(db, task)?.0;
            let obstacles = if matches!(state, crate::TaskState::Executing | crate::TaskState::Reviewing | crate::TaskState::Recovering) {
                scheduler::policy::claim_obstacles_tx(db, task, now)?
            } else {
                Vec::new()
            };
            let mut lines = Vec::new();
            for o in obstacles.iter().take(3) {
                let label = step(o["role"].as_str().unwrap_or(""), o["objective"].as_str().unwrap_or(""));
                let files = paths(&o["paths"], &root, ru);
                lines.push(match o["reason"].as_str().unwrap_or("") {
                    "denied" => {
                        for rule in o["rules"].as_array().into_iter().flatten() {
                            targets.push(json!({"kind":"rule","path":rule}));
                        }
                        let rules = paths(&o["rules"], &root, ru);
                        cont = l(
                            "the coordinator plans around the denied files; your rule stays in force",
                            "координатор составит план в обход запрещённых файлов; ваше правило остаётся в силе",
                        );
                        l(
                            &format!("The step “{label}” must change {files}, but your rule denies writing to {rules}."),
                            &format!("Шаг «{label}» должен изменить {files}, но ваше правило запрещает запись в {rules}."),
                        )
                    }
                    "held" if o["holder_task"].as_str() == Some(task) => {
                        recommended = None;
                        let holder = o["holder_role"].as_str().map(role_word).unwrap_or("");
                        l(
                            &format!("The step “{label}” needs {files}, which an earlier step of this task ({holder}) still holds: the exit of its engine process was not confirmed."),
                            &format!("Шагу «{label}» нужны {files}, но их всё ещё удерживает более ранний шаг этой задачи ({holder}): завершение его процесса не подтверждено."),
                        )
                    }
                    "held" | "access_wait" => {
                        let (target, holder) = task_target(db, o["holder_task"].as_str().unwrap_or(""), ru)?;
                        targets.push(target);
                        l(
                            &format!("The step “{label}” needs {files}, which the task {holder} is changing."),
                            &format!("Шагу «{label}» нужны {files}, которые сейчас изменяет задача {holder}."),
                        )
                    }
                    "retry_backoff" => l(
                        &format!("The step “{label}” is waiting out the pause before its retry."),
                        &format!("Шаг «{label}» выжидает паузу перед повтором."),
                    ),
                    "no_ready_nodes" => {
                        recommended = None;
                        l(
                            "No remaining planned step can start: each one depends on a step that failed, was stopped or has not finished.",
                            "Ни один оставшийся запланированный шаг не может начаться: каждый зависит от шага, который завершился ошибкой, был остановлен или ещё не закончен.",
                        )
                    }
                    _ => l(
                        &format!("The step “{label}” was free to start, but the runtime did not start it, and it recorded no reason."),
                        &format!("Шаг «{label}» мог начаться, но runtime его не запустил и причину не записал."),
                    ),
                });
            }
            if lines.is_empty() {
                recommended = None;
                lines.push(l(
                    "The runtime found no next step to start and recorded no reason for it.",
                    "Runtime не нашёл следующего шага для запуска и не записал причину.",
                ));
            }
            lines.join(" ")
        }
        "scheduler_capacity_timeout" => l(
            "Every engine slot for workers stayed busy, and none became free for the next step in time.",
            "Все слоты движка для воркеров оставались заняты, и ни один не освободился для следующего шага вовремя.",
        ),
        b if b.starts_with("completion_") => {
            recommended = None;
            l(
                &format!("The runtime could not confirm from the recorded evidence that the work is complete (recorded reason: {}).", b.replace('_', " ")),
                &format!("Runtime не смог подтвердить по записанным данным, что работа завершена (записанная причина: {}).", b.replace('_', " ")),
            )
        }
        "native_session_start_failed" | "coordinator_session_start_failed" => {
            cont = l(
                "the coordinator tries to start the session again; check the engine connection and the selected model first",
                "координатор попробует запустить сессию снова; сначала проверьте подключение движка и выбранную модель",
            );
            if blocker == "native_session_start_failed" {
                let found = worker_step_tx(db, task)?;
                let label = found.as_ref().map(|(r, o, _)| step(r, o)).unwrap_or_else(|| l("a planned step", "запланированного шага"));
                l(
                    &format!("The engine did not start a worker session for the step “{label}”."),
                    &format!("Движок не запустил сессию воркера для шага «{label}»."),
                )
            } else {
                l("The engine did not start the coordinator's session.", "Движок не запустил сессию координатора.")
            }
        }
        "native_progress_limit" => {
            recommended = None;
            l(
                &format!("The task used up its limit of {} runtime steps.", super::progress::PROGRESS_BUDGET),
                &format!("Задача исчерпала предел в {} шагов runtime.", super::progress::PROGRESS_BUDGET),
            )
        }
        "coordinator_no_progress_limit" => {
            recommended = None;
            l(
                "The coordinator ended three turns in a row without a plan, a result or a question.",
                "Координатор трижды подряд завершил ход без плана, результата или вопроса.",
            )
        }
        // The project observation that binds the baseline, review and checks to the files.
        // A folder in no Git repository is observed without Git; this is a repository Git
        // itself could not read.
        "project_revision_unavailable" => {
            recommended = None;
            cont = l(
                "the coordinator tries again; first make sure `git status` works in the project folder (repair the repository or mark the folder as trusted)",
                "координатор попробует снова; сначала убедитесь, что `git status` работает в папке проекта (восстановите репозиторий или отметьте папку как доверенную)",
            );
            l(
                "The project folder belongs to a Git repository, but Git could not list its files or read its current commit (the repository may be damaged, locked, owned by another user or not trusted), so the runtime could not record the state of the project files.",
                "Папка проекта входит в Git-репозиторий, но Git не смог перечислить его файлы или прочитать текущий коммит (репозиторий может быть повреждён, заблокирован, принадлежать другому пользователю или не считаться доверенным), поэтому runtime не смог зафиксировать состояние файлов проекта.",
            )
        }
        b if b.starts_with("project_unobservable_") => {
            recommended = None;
            cont = l(
                "the coordinator tries again once the cause is removed; nothing about the files is assumed meanwhile",
                "координатор попробует снова, когда причина будет устранена; до тех пор о файлах ничего не предполагается",
            );
            match b {
                "project_unobservable_unreadable_content" => l(
                    "A file or folder in the project cannot be read by the runtime (check its permissions), so the state of the project files could not be recorded completely.",
                    "Runtime не может прочитать файл или папку проекта (проверьте права доступа), поэтому состояние файлов проекта не удалось зафиксировать полностью.",
                ),
                "project_unobservable_changed_while_reading" => l(
                    "A project file kept changing while the runtime read it, so the state of the project files could not be recorded.",
                    "Файл проекта менялся, пока runtime его читал, поэтому состояние файлов проекта не удалось зафиксировать.",
                ),
                "project_unobservable_too_many_paths" => l(
                    "The project folder holds more files than one observation covers, so the state of the project files could not be recorded completely.",
                    "В папке проекта больше файлов, чем охватывает одно наблюдение, поэтому состояние файлов проекта не удалось зафиксировать полностью.",
                ),
                "project_unobservable_path" => l(
                    "A project path has a name the runtime cannot record safely (not valid UTF-8, or a reserved name inside the project), so the state of the project files could not be recorded.",
                    "Путь в проекте имеет имя, которое runtime не может безопасно записать (не UTF-8 или зарезервированное имя внутри проекта), поэтому состояние файлов проекта не удалось зафиксировать.",
                ),
                _ => l(
                    "The runtime could not observe the project files completely, so their state could not be recorded.",
                    "Runtime не смог полностью прочитать файлы проекта, поэтому их состояние не удалось зафиксировать.",
                ),
            }
        }
        other => {
            recommended = None;
            l(
                &format!("The runtime stopped for an internal reason ({}) and recorded no more detail.", other.replace('_', " ")),
                &format!("Runtime остановился по внутренней причине ({}) и больше подробностей не записал.", other.replace('_', " ")),
            )
        }
    };
    let options = if ru {
        domain::STOP_OPTIONS_RU
    } else {
        domain::STOP_OPTIONS_EN
    };
    let answers = vec![
        Answer {
            label: options[0],
            detail: cont,
            action: "continue",
        },
        Answer {
            label: options[1],
            detail: finish,
            action: "finish",
        },
    ];
    let headline = domain::stop_headline(blocker, ru);
    let mut text = format!(
        "{headline}\n\n{}{what}\n\n{}",
        l("What happened: ", "Что произошло: "),
        l("What you can do:", "Что можно сделать:")
    );
    for (i, a) in answers.iter().enumerate() {
        let word = a.label.split(':').next().unwrap_or(a.label);
        let mark = if recommended == Some(i) {
            l(" (recommended)", " (рекомендуется)")
        } else {
            String::new()
        };
        text.push_str(&format!("\n{}. {word}{mark} — {}.", i + 1, a.detail));
    }
    text.push_str(&l("\n\nWhat should happen next?", "\n\nЧто делать дальше?"));
    Ok(Notice {
        text,
        headline,
        what,
        answers,
        recommended,
        targets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lifecycle, store::Store};

    fn store() -> (std::path::PathBuf, Store) {
        let root = std::env::temp_dir().join(format!("stop-notice-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let key = crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap();
        (
            root.clone(),
            Store::open(key, crate::OpenMode::Initialize).unwrap(),
        )
    }

    /// Every explained kind, in both languages, says what happened and what each
    /// answer does; nothing is a bare code or the old hedge about busy or denied files.
    #[test]
    fn every_runtime_stop_kind_explains_what_happened_and_each_answer() {
        let (root, store) = store();
        lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        let mut kinds: Vec<String> = KINDS
            .iter()
            .map(|k| {
                if k.ends_with('_') {
                    format!("{k}gone")
                } else {
                    (*k).to_owned()
                }
            })
            .collect();
        kinds.push("something_unexpected".into());
        store
            .write(|db| {
                db.execute(
                    "UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",
                    [],
                )?;
                for kind in &kinds {
                    for ru in [false, true] {
                        let n = explain_tx(db, "t", kind, ru)?;
                        assert!(
                            !n.headline.trim().is_empty() && !n.what.trim().is_empty(),
                            "{kind}"
                        );
                        assert_eq!(n.answers.len(), 2, "{kind}");
                        assert_eq!(n.answers[0].action, "continue");
                        assert_eq!(n.answers[1].action, "finish");
                        for a in &n.answers {
                            assert!(a.detail.chars().count() > 20, "{kind}: {}", a.detail);
                            assert!(n.text.contains(&a.detail), "{kind}");
                        }
                        let cyrillic = n.what.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c));
                        assert_eq!(cyrillic, ru, "{kind} {ru}: {}", n.what);
                        for hedge in [
                            "may be reserved by other work",
                            "могут быть заняты другой работой",
                        ] {
                            assert!(!n.text.contains(hedge), "{kind}");
                        }
                        if kind != "something_unexpected" {
                            assert!(!n.text.contains(kind.as_str()), "{kind}: {}", n.text);
                        }
                    }
                }
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A step that cannot start because the owner denied writing to its files names
    /// the step, the files and the rule, offers planning around them and carries the rule.
    #[test]
    fn a_step_denied_by_the_owner_names_the_path_and_the_rule() {
        let (root, store) = store();
        lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'docs','worker-technical-writer','Update the guide','[]','[\"docs/guide.md\"]','execution','pending',NULL,NULL)", [])?;
                lifecycle::event_tx(db, "t", "USER_STEERING", json!({"text":"x","deny_write":["docs/**"]}))?;
                let n = explain_tx(db, "t", "native_continuation_unavailable", false)?;
                assert!(n.what.contains("technical-writer: Update the guide"), "{}", n.what);
                assert!(n.what.contains("docs/guide.md") && n.what.contains("docs/**"), "{}", n.what);
                assert!(n.answers[0].detail.contains("around the denied files"));
                assert_eq!(n.recommended, Some(0));
                assert_eq!(n.targets, vec![json!({"kind":"rule","path":"docs/**"})]);
                let n = explain_tx(db, "t", "native_continuation_unavailable", true)?;
                assert!(n.what.contains("запрещает запись в docs/**"), "{}", n.what);
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A review stop names the reviewer, its words and its report, offers to address
    /// the findings (recommended) or to close as is, and carries the report as a target.
    #[test]
    fn a_review_stop_names_the_findings_and_offers_to_address_them() {
        let (root, store) = store();
        lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=2,review_cycle=3 WHERE task_id='t'", [])?;
                let result = json!({"verdict":"REQUEST_CHANGES","summary":"The proxy chain is not traced.","report_ref":"r_0123456789ab",
                    "findings":[{"node":"a","files":["x"],"cross_cutting":false,"fix":"Trace the request chain"},
                                {"node":"a","files":["y"],"cross_cutting":false,"fix":"Document the deployment"}]});
                db.execute("INSERT INTO runtime_nodes VALUES ('t',2,'_review_3','worker-code-reviewer','Review','[]','[\"**\"]','review','completed','rv',?)",
                    [result.to_string()])?;
                let n = explain_tx(db, "t", "review_blocked", false)?;
                assert!(n.what.contains("code-reviewer") && n.what.contains("The proxy chain is not traced."), "{}", n.what);
                assert!(n.what.contains("Findings: 2 (Trace the request chain; Document the deployment)"), "{}", n.what);
                assert!(n.answers[0].detail.starts_with("address the findings"));
                assert!(n.answers[1].detail.contains("findings stay unaddressed"));
                assert_eq!(n.recommended, Some(0));
                assert_eq!(n.targets, vec![json!({"kind":"report","reportId":"r_0123456789ab","role":"code-reviewer"})]);
                assert!(n.text.contains("1. Continue (recommended) — address the findings"), "{}", n.text);
                let n = explain_tx(db, "t", "review_cycle_limit", true)?;
                assert!(n.what.contains("3 раза") && n.what.contains("Замечаний: 2"), "{}", n.what);
                assert_eq!(n.recommended, None);
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A superseded step of the same task (an earlier revision, cancelled on replan or
    /// on a request for a new result) whose process exit was observed holds nothing: the
    /// new step is claimable. Only an unconfirmed exit keeps holding, and is named so.
    #[test]
    fn a_superseded_step_holds_its_files_only_while_its_exit_is_unconfirmed() {
        let (root, store) = store();
        lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=2 WHERE task_id='t'", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'old','worker-devops-engineer','Old TLS','[]','[\"traefik/**\"]','execution','cancelled','old',NULL)", [])?;
                db.execute("INSERT INTO delegation_provisioning VALUES ('old','t',NULL,'ox','worker-devops-engineer','ready','terminated',NULL,'oc',NULL,NULL,NULL,NULL,NULL,1,'0','0')", [])?;
                db.execute("INSERT INTO capacity_reservations VALUES ('or','c','t','old','ox','worker','released','0','0','process_exited')", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',2,'new','worker-devops-engineer','New TLS','[]','[\"traefik/dynamic.yml\"]','execution','pending',NULL,NULL)", [])?;
                let o = scheduler::policy::claim_obstacles_tx(db, "t", 0.0)?;
                assert_eq!(o.len(), 1);
                assert_eq!(o[0]["reason"], "claimable", "{o:?}");
                db.execute("UPDATE delegation_provisioning SET status='ambiguous' WHERE idempotency_key='old'", [])?;
                db.execute("UPDATE capacity_reservations SET state='ambiguous',release_observation=NULL", [])?;
                let o = scheduler::policy::claim_obstacles_tx(db, "t", 0.0)?;
                assert_eq!((o[0]["reason"].as_str(), o[0]["holder_task"].as_str()), (Some("held"), Some("t")), "{o:?}");
                let n = explain_tx(db, "t", "native_continuation_unavailable", false)?;
                assert!(n.what.contains("an earlier step of this task (devops-engineer)"), "{}", n.what);
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// An incomplete review is neither approval nor a block. Its named dead ends are
    /// routed at once as the run mode's stop (here Manual: the owner's question names
    /// each part with its blocker); no second reviewer is scheduled for the same
    /// revision. A BLOCK naming no finding takes the same stop; a BLOCK with findings
    /// stops as review_blocked.
    #[test]
    fn an_incomplete_review_routes_its_dead_ends_without_another_review() {
        let (root, store) = store();
        lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        let incomplete = json!({"verdict":"INCOMPLETE","summary":"Part of the change was not reviewed.","report_ref":"r_aaaaaaaaaaaa","checks":[],
            "not_covered":[{"item":"backend/src/modules/auth/limiter.ts","blocker":"missing_access","reason":"the file is on a host the reviewer cannot reach"},
                           {"item":"retry policy","blocker":"owner_decision","reason":"whether retries may be dropped is the owner's call"}]});
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=4,review_cycle=1,plan_json='{\"tasks\":[]}' WHERE task_id='t'", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',4,'trace','worker-security-auditor','Trace','[]','[\"traefik/**\"]','execution','completed','a1','{}')", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',4,'_review_1','worker-code-reviewer','Review','[\"trace\"]','[\"**\"]','review','completed','rv',?)", [incomplete.to_string()])?;
                lifecycle::review::advance_tx(db, "t")?;
                assert_eq!(domain::task_tx(db, "t")?.0, crate::TaskState::WaitingUser);
                assert_eq!(db.query_row("SELECT count(*) FROM runtime_nodes WHERE task_id='t' AND stage='review'", [], |r| r.get::<_, i64>(0))?, 1);
                assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE type='REVIEW_RERUN'", [], |r| r.get::<_, i64>(0))?, 0);
                let routed: String = db.query_row("SELECT payload_json FROM runtime_events WHERE task_id='t' AND type='REVIEW_DEAD_END_ROUTED'", [], |r| r.get(0))?;
                let routed: Value = serde_json::from_str(&routed)?;
                assert_eq!(routed["blocked"][0]["blocker"], "missing_access");
                assert_eq!(routed["blocked"][1]["blocker"], "owner_decision");
                let text: String = db.query_row("SELECT text FROM pending_questions WHERE status='open'", [], |r| r.get(0))?;
                assert!(text.starts_with("The task stopped: the independent review could not cover the whole change."), "{text}");
                assert!(text.contains("none was started"), "{text}");
                assert!(text.contains("Not covered: backend/src/modules/auth/limiter.ts (needs access or a permission the reviewer does not have: the file is on a host the reviewer cannot reach)"), "{text}");
                assert!(text.contains("retry policy (needs your decision: "), "{text}");
                Ok(())
            })
            .unwrap();
        // A concrete finding in an INCOMPLETE review is repairable now. The
        // coverage gap is carried into the fix's objective, and the mandatory
        // fresh review is scheduled only after that fix.
        lifecycle::TaskMachine::new(&store)
            .create("f", "g", "o")
            .unwrap();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1,review_cycle=1,plan_json='{\"tasks\":[{\"id\":\"worker\"}]}' WHERE task_id='f'", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('f',1,'worker','worker-backend-dev','Work','[]','[\"src/**\"]','execution','completed','fw','{}')", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('f',1,'_review_1','worker-code-reviewer','Review','[\"worker\"]','[\"**\"]','review','completed','fr',?)", [json!({
                    "verdict":"INCOMPLETE", "summary":"One finding is confirmed, another part needs the fix first.", "report_ref":"r_ffffffffffff",
                    "findings":[{"node":"worker","files":["src/a.rs"],"cross_cutting":false,"fix":"Fix the unsafe branch."}],
                    "not_covered":[{"item":"src/b.rs","blocker":"other_role_work","reason":"its behaviour depends on the fix to src/a.rs"}]
                }).to_string()])?;
                lifecycle::review::advance_tx(db, "f")?;
                assert_eq!(domain::task_tx(db, "f")?.0, crate::TaskState::Executing);
                let objective: String = db.query_row("SELECT objective FROM runtime_nodes WHERE task_id='f' AND node_id='_fix_1_worker'", [], |r| r.get(0))?;
                assert!(objective.contains("Fix the unsafe branch."), "{objective}");
                assert!(objective.contains("src/b.rs"), "{objective}");
                assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id='f' AND type IN ('REVIEW_RERUN','REVIEW_DEAD_END_ROUTED')", [], |r| r.get::<_, i64>(0))?, 0);
                Ok(())
            })
            .unwrap();
        // A (legacy) BLOCK naming no finding stops like an incomplete review, without
        // a rerun; a BLOCK with findings stops as review_blocked.
        for (task, result, blocker) in [
            (
                "b",
                json!({"verdict":"BLOCK","summary":"Not sure.","report_ref":"r_bbbbbbbbbbbb","checks":[]}),
                "review_incomplete",
            ),
            (
                "c",
                json!({"verdict":"BLOCK","summary":"Secrets are logged.","report_ref":"r_cccccccccccc","checks":[],
                "findings":[{"node":"w","files":["src/a.rs"],"cross_cutting":false,"fix":"Stop logging the token"}]}),
                "review_blocked",
            ),
        ] {
            lifecycle::TaskMachine::new(&store)
                .create(task, "g", "o")
                .unwrap();
            store
                .write(|db| {
                    db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1,plan_json='{\"tasks\":[]}' WHERE task_id=?", [task])?;
                    db.execute("INSERT INTO runtime_nodes VALUES (?,1,'w','worker-backend-dev','Work','[]','[\"src\"]','execution','completed',?,'{}')", (task, format!("{task}1")))?;
                    db.execute("INSERT INTO runtime_nodes VALUES (?,1,'_review_1','worker-code-reviewer','Review','[\"w\"]','[\"**\"]','review','completed',?,?)",
                        (task, format!("{task}rv"), result.to_string()))?;
                    lifecycle::review::advance_tx(db, task)?;
                    assert_eq!(db.query_row("SELECT count(*) FROM runtime_nodes WHERE task_id=? AND stage='review'", [task], |r| r.get::<_, i64>(0))?, 1);
                    let stopped: String = db.query_row(
                        "SELECT coalesce((SELECT json_extract(payload_json,'$.blocker') FROM runtime_events WHERE task_id=?1 AND type IN ('ASK_USER_REQUIRED','USER_QUESTION_REQUESTED') ORDER BY id DESC LIMIT 1),'')",
                        [task], |r| r.get(0))?;
                    assert_eq!(stopped, blocker, "{task}");
                    Ok(())
                })
                .unwrap();
        }
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A reviewer's packet lists every file the work changed, from the baseline
    /// manifest (including command-made changes) and the recorded writes with the node
    /// that made them, names the predecessor reports with their size, and confirms the
    /// read scope covers the change. Without a trustworthy manifest it says the list is
    /// the recorded writes only.
    #[test]
    fn a_review_packet_lists_the_changed_files_reports_and_read_scope() {
        let (root, store) = store();
        let owner = lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1 WHERE task_id='t'", [])?;
                lifecycle::event_tx(db, "t", "EXECUTION_BASELINE", json!({"kind":"observed","revision_digest":"d0","ignored_digest":"i0","revision":1}))?;
                let before: std::collections::BTreeMap<String, String> =
                    [("a.txt", "1"), ("gone.txt", "2"), ("same.txt", "3")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                lifecycle::reconciliation::record_baseline_manifest_tx(db, "t", "d0", &before)?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'ops','worker-devops-engineer','Ops','[]','[\"scripts/**\"]','execution','completed','a1',?)",
                    [json!({"report_ref":"r_aaaaaaaaaaaa","summary":"s","checks":[]}).to_string()])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'_review_1','worker-code-reviewer','Review','[\"ops\"]','[\"**\"]','review','running','rv',NULL)", [])?;
                db.execute("INSERT INTO reports(id,task_id,kind,filename,created_at) VALUES ('r_aaaaaaaaaaaa','t','report','r.md','0')", [])?;
                db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES ('r_aaaaaaaaaaaa','T','S','x','0','d',9001)", [])?;
                // The worker's channel was issued while its step ran.
                db.execute("UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='t'", [])?;
                db.execute("UPDATE runtime_nodes SET status='running',result_json=NULL WHERE node_id='ops'", [])?;
                let c = crate::channels::issue_tx(db, "t", &owner, crate::channels::Principal::Worker { assignment: "a1" })?;
                lifecycle::event_tx(db, "t", "NATIVE_PROJECT_WRITTEN", json!({"channel_id":c.channel_id,"path":"scripts/check.sh","add":1,"del":1,"sha256":"old"}))?;
                lifecycle::event_tx(db, "t", "NATIVE_PROJECT_WRITTEN", json!({"channel_id":c.channel_id,"path":"legacy.txt","add":1,"del":0,"sha256":"legacy"}))?;
                db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE node_id='ops'", [json!({"report_ref":"r_aaaaaaaaaaaa","summary":"s","checks":[]}).to_string()])?;
                lifecycle::event_tx(db, "t", "TASK_REOPENED", json!({"revision":1}))?;
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=2 WHERE task_id='t'", [])?;
                lifecycle::event_tx(db, "t", "EXECUTION_BASELINE", json!({"kind":"observed","revision_digest":"d1","ignored_digest":"i0","revision":2}))?;
                lifecycle::reconciliation::record_baseline_manifest_tx(db, "t", "d1", &before)?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',2,'ops2','worker-devops-engineer','Ops','[]','[\"scripts/**\"]','execution','completed','a2',?)",
                    [json!({"report_ref":"r_aaaaaaaaaaaa","summary":"s","checks":[]}).to_string()])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',2,'_review_2','worker-code-reviewer','Review','[\"ops2\"]','[\"**\"]','review','running','rv2',NULL)", [])?;
                db.execute("UPDATE runtime_nodes SET status='running',result_json=NULL WHERE node_id='ops2'", [])?;
                let c2 = crate::channels::issue_tx(db, "t", &owner, crate::channels::Principal::Worker { assignment: "a2" })?;
                lifecycle::event_tx(db, "t", "NATIVE_PROJECT_WRITTEN", json!({"channel_id":c2.channel_id,"path":"scripts/check.sh","add":3,"del":0,"sha256":"bad"}))?;
                db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE node_id='ops2'", [json!({"report_ref":"r_aaaaaaaaaaaa","summary":"s","checks":[]}).to_string()])?;
                let current: std::collections::BTreeMap<String, String> =
                    [("a.txt", "9"), ("same.txt", "3"), ("new.txt", "4"), ("scripts/check.sh", "5")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                let current_raw: std::collections::BTreeMap<String, String> = current.clone();
                let p = lifecycle::reconciliation::review_packet_tx(db, "t", "_review_2", Some(&current), Some("i0"), &current_raw)?;
                let files = &p["changed_files"];
                assert_eq!(files["complete"], true);
                assert_eq!(files["total"], 5);
                let paths: Vec<_> = files["files"].as_array().unwrap().iter().map(|f|f["path"].as_str().unwrap()).collect();
                assert_eq!(paths, ["a.txt","gone.txt","legacy.txt","new.txt","scripts/check.sh"]);
                assert_eq!(files["files"][4]["by"],json!(["ops","ops2"]));
                assert_eq!(files["files"][2]["last_write"]["revision"],1);
                assert_eq!(p["predecessor_reports"], json!([{"report_id":"r_aaaaaaaaaaaa","node":"ops2","role":"devops-engineer","title":"T","size_bytes":9001,"pages_at_most":3,"page_characters":4000}]));
                assert_eq!((p["read_scope"].clone(), p["read_scope_covers_change"].clone()), (json!(["**"]), json!(true)));
                assert!(p["change_boundary"]["baseline_event"].is_number());
                let current_write = &files["files"][4]["last_write"];
                assert_eq!(current_write["revision"], 2);
                assert_eq!(current_write["node"], "ops2");
                assert_eq!(current_write["sha256"], "bad");
                assert_eq!(current_write["matches_current"], false);
                assert!(files["files"][0]["last_write"].is_null());
                // Without a trustworthy manifest only the recorded writes are listed, and it says so.
                let p = lifecycle::reconciliation::review_packet_tx(db, "t", "_review_2", None, None, &current_raw)?;
                assert_eq!(p["changed_files"]["complete"], false);
                assert_eq!(p["changed_files"]["total"], 2);
                assert_eq!(p["changed_files"]["files"][0]["path"], "legacy.txt");
                assert_eq!(p["changed_files"]["files"][1]["last_write"]["matches_current"],false);
                assert!(p["changed_files"]["note"].as_str().unwrap().contains("files changed by commands are not listed"));
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A step whose files another unfinished task holds names that task, with a target
    /// the desktop can open.
    #[test]
    fn a_step_held_by_another_task_names_the_holder() {
        let (root, store) = store();
        let machine = lifecycle::TaskMachine::new(&store);
        machine.create("t", "g", "o").unwrap();
        machine.create("h", "Format all tests\nmore", "o").unwrap();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('h',1,'fmt','worker-backend-dev','o','[]','[\"test/a.js\"]','execution','running','ha',NULL)", [])?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'reset','worker-backend-dev','Reset the counter','[]','[\"test\"]','execution','pending',NULL,NULL)", [])?;
                let n = explain_tx(db, "t", "no_runnable_nodes", false)?;
                assert!(n.what.contains("the task “Format all tests” (running)"), "{}", n.what);
                assert_eq!(n.targets[0]["kind"], "task");
                assert_eq!(n.targets[0]["runtimeTaskId"], "h");
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
