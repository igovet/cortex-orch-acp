//! The runtime-owned `pipeline.md` of each task: a human-readable projection of
//! durable task state, published through the ordinary document publication path
//! (`Publisher::create_pipeline_draft` / `publish`), so every edition is a
//! journaled, hash-checked edition of the one pipeline report, newest first
//! with older editions below in the same file.
//!
//! [`sync`] is idempotent and derived only from durable state. It publishes the
//! initial edition when a task has none, and a new edition whenever the
//! task's (plan revision, state, review cycle, open questions) differs from the
//! newest edition's. A crash between a state transition and its edition is
//! repaired by the next call, which publishes the edition that is missing; a
//! crash inside publication is reconciled by the publication journal, and the
//! deterministic request keys replay instead of duplicating an edition.
use crate::{
    store::documents::{Publication, Publisher},
    CoreResult, Store,
};
use rusqlite::OptionalExtension;
use serde_json::Value;

/// The principal that owns runtime pipeline drafts. Models never hold it.
pub const OWNER: &str = "runtime";
const INITIAL_TITLE: &str = "Initial pipeline";
const INITIAL_SUMMARY: &str =
    "Runtime-published initial pipeline; the coordinator has not yet run.";
const GOAL_CHARACTERS: usize = 3600;
const NOT_DETERMINED: &str =
    "Not yet determined: this section is model-owned guidance the runtime does not fill in; the coordinator has not yet run.";

/// The coordinator route an initial edition names, when it is known.
pub struct Route<'a> {
    pub engine: &'a str,
    pub model: &'a str,
    pub effort: Option<&'a str>,
}

struct Facts {
    state: String,
    revision: i64,
    review_cycle: i64,
    goal: String,
    synthesis: Option<String>,
    blocker: Option<String>,
    open_questions: Vec<String>,
    nodes: Vec<(String, String, String, Value, Value)>,
    verification: Option<(String, Value)>,
    review_waiver: Option<Value>,
}
fn facts(store: &Store, task: &str) -> CoreResult<Facts> {
    store.read(|db| {
        let (state, revision, review_cycle, goal, synthesis, blocker): (String, i64, i64, String, Option<String>, Option<String>) =
            db.query_row(
                "SELECT state,revision,review_cycle,goal,synthesis,blocker FROM runtime_tasks WHERE task_id=?",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )?;
        let open_questions = db
            .prepare("SELECT id FROM pending_questions WHERE task_id=? AND status='open' ORDER BY id")?
            .query_map([task], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let nodes = db
            .prepare(
                "SELECT node_id,role,status,dependencies_json,result_json FROM runtime_nodes
                 WHERE task_id=? AND revision=? ORDER BY rowid",
            )?
            .query_map((task, revision), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|(id, role, status, deps, result)| {
                Ok((
                    id,
                    role,
                    status,
                    serde_json::from_str(&deps)?,
                    result.map(|r| serde_json::from_str(&r)).transpose()?.unwrap_or(Value::Null),
                ))
            })
            .collect::<CoreResult<Vec<_>>>()?;
        let event = |kinds: &str| -> CoreResult<Option<(String, Value)>> {
            let row: Option<(String, String)> = db
                .query_row(
                    &format!(
                        "SELECT type,payload_json FROM runtime_events WHERE task_id=? AND type IN ({kinds})
                         AND coalesce(json_extract(payload_json,'$.revision'),?)=? ORDER BY id DESC LIMIT 1"
                    ),
                    (task, revision, revision),
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            row.map(|(k, p)| Ok((k, serde_json::from_str(&p)?))).transpose()
        };
        let verification = event("'VERIFICATION_PASSED','VERIFICATION_FAILED'")?;
        let review_waiver = event("'REVIEW_NOT_REQUIRED'")?.map(|v| v.1);
        Ok(Facts { state, revision, review_cycle, goal, synthesis, blocker, open_questions, nodes, verification, review_waiver })
    })
}
fn summary(f: &Facts) -> String {
    let mut s = format!("Plan revision {}; stage {}.", f.revision, f.state);
    if f.review_cycle > 0 {
        s.push_str(&format!(" Review cycle {}.", f.review_cycle));
    }
    if !f.open_questions.is_empty() {
        s.push_str(&format!(
            " Waiting on {} open question(s).",
            f.open_questions.len()
        ));
    }
    s.chars().take(320).collect()
}
fn quoted(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return "The user's original request text is not recorded for this task.".into();
    }
    let text: String = text.chars().take(GOAL_CHARACTERS).collect();
    format!("> {}", text.replace('\n', "\n> "))
}
fn bounded_goal(goal: &str) -> String {
    let total = goal.trim().chars().count();
    let mut q = quoted(goal);
    if total > GOAL_CHARACTERS {
        q.push_str(&format!("\n\n(The request is {total} characters; the first {GOAL_CHARACTERS} are shown. The full text is the task's retained original goal.)"));
    }
    q
}
fn initial(task: &str, goal: &str, route: Option<&Route<'_>>) -> String {
    let route = match route {
        Some(r) => format!(
            "Bound coordinator route: engine `{}`, model `{}`, effort `{}`.",
            r.engine,
            r.model,
            r.effort.unwrap_or("not recorded for this route")
        ),
        None => "The bound engine, model and effort for this task's coordinator route are not yet determined.".into(),
    };
    let fill = |line: &str| -> Option<String> {
        Some(match line {
            "{{CURRENT_OBJECTIVE_AND_DECISION_BASIS}}" => format!(
                "Task `{task}` was just opened from this exact user request:\n\n{}\n\nStatus: not yet worked. This initial edition was published by the runtime itself, before any coordinator turn ran.",
                bounded_goal(goal)
            ),
            "{{CURRENT_REQUIREMENTS_AND_CONSTRAINTS}}" => "Not yet determined: the coordinator has not yet reviewed the request for explicit requirements or constraints. The user's own words above are the only source of truth until it does.".into(),
            "{{CURRENT_WORK_GRAPH}}" => "Not yet determined: no work has been planned or delegated yet.".into(),
            "{{CURRENT_ASSIGNMENTS_AND_ROUTING}}" => format!("{route} No assignment has been delegated yet."),
            "{{CURRENT_EVIDENCE_AND_VERIFICATION}}" => "Not yet determined: no work has been checked or verified yet.".into(),
            "{{CURRENT_DECISIONS_QUESTIONS_AND_REMAINING_WORK}}" => "Not yet determined: no decisions have been made and no work remains scoped yet.".into(),
            l if l.starts_with("<!--") && l.ends_with("-->") => NOT_DETERMINED.into(),
            _ => return None,
        })
    };
    include_str!("../../../report-templates/pipeline.md")
        .lines()
        .map(|l| fill(l).unwrap_or_else(|| l.to_owned()))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}
fn projection(f: &Facts) -> String {
    let mut rows = vec![
        "# Runtime pipeline".to_owned(),
        String::new(),
        format!("State: {}", f.state),
        format!("Plan revision: {}", f.revision),
        String::new(),
        "## Original objective".into(),
        String::new(),
        bounded_goal(&f.goal),
        String::new(),
        "## Assignments".into(),
        String::new(),
    ];
    if f.nodes.is_empty() {
        rows.push("No assignment in this plan revision.".into());
    }
    for (id, role, status, deps, result) in &f.nodes {
        let deps = deps
            .as_array()
            .map(|d| {
                d.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "none".into());
        let report = result["report_ref"].as_str().unwrap_or("not published");
        rows.push(format!(
            "- {id} ({role}): {status}; dependencies: {deps}; report: {report}"
        ));
    }
    if !f.open_questions.is_empty() {
        rows.extend([String::new(), "## Open questions".into(), String::new()]);
        rows.push(format!(
            "{} question(s) wait for the owner's answer.",
            f.open_questions.len()
        ));
    }
    if let Some(waiver) = &f.review_waiver {
        rows.extend([String::new(), "## Independent review".into(), String::new()]);
        rows.push(format!(
            "Not required ({}): the runtime observed no project file changes.",
            waiver["reason"].as_str().unwrap_or("no project change")
        ));
    }
    if let Some((kind, payload)) = &f.verification {
        rows.extend([
            String::new(),
            "## Runtime verification".into(),
            String::new(),
        ]);
        rows.push(if kind == "VERIFICATION_PASSED" {
            "Passed.".into()
        } else {
            "Failed.".into()
        });
        rows.push(String::new());
        let detail = serde_json::to_string_pretty(payload).unwrap_or_default();
        rows.push(format!(
            "```json\n{}\n```",
            detail.chars().take(8000).collect::<String>()
        ));
    }
    if let Some(synthesis) = f.synthesis.as_deref().filter(|s| !s.trim().is_empty()) {
        rows.extend([
            String::new(),
            "## Synthesis".into(),
            String::new(),
            synthesis.to_owned(),
        ]);
    }
    if let Some(blocker) = f.blocker.as_deref().filter(|s| !s.trim().is_empty()) {
        rows.extend([
            String::new(),
            "## Blocker".into(),
            String::new(),
            blocker.to_owned(),
        ]);
    }
    rows.join("\n") + "\n"
}

/// Publish the edition the task's durable state calls for, if it is not the
/// newest one already. Returns the pipeline report id when an edition was
/// published. Callers run it on the store's serial publication lane.
pub fn sync(store: &Store, task: &str, route: Option<&Route<'_>>) -> CoreResult<Option<String>> {
    let f = facts(store, task)?;
    let newest: Option<(String, String, i64)> = store.read(|db| {
        Ok(db
            .query_row(
                "SELECT e.title,e.summary,(SELECT count(*) FROM editions WHERE report_id=r.id)
                 FROM reports r JOIN editions e ON e.report_id=r.id WHERE r.task_id=? AND r.kind='pipeline'
                 ORDER BY e.sequence DESC LIMIT 1",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    })?;
    let key = summary(&f);
    let untouched = matches!(f.state.as_str(), "CREATED" | "PLANNING")
        && f.revision == 0
        && f.review_cycle == 0
        && f.open_questions.is_empty()
        && f.nodes.is_empty();
    let (title, summary, body) = match &newest {
        None if untouched => (
            INITIAL_TITLE.to_owned(),
            INITIAL_SUMMARY.to_owned(),
            initial(task, &f.goal, route),
        ),
        Some((_, s, _)) if *s == key => return Ok(None),
        Some((t, _, _)) if t == INITIAL_TITLE && untouched => return Ok(None),
        _ => (format!("Runtime {}", f.state), key.clone(), projection(&f)),
    };
    let editions = newest.as_ref().map_or(0, |n| n.2);
    let request = format!(
        "pipeline-{}-{}",
        editions,
        &crate::digest(format!("{title}\n{summary}").as_bytes())[..16]
    );
    let publisher = Publisher::new(store);
    let draft = publisher.create_pipeline_draft(task, OWNER, &request)?;
    let markdown = format!("Cortex draft ID: `{}`\n\n{body}", draft.draft_id);
    if draft.draft_path.symlink_metadata().is_ok() {
        publisher.write_draft(task, OWNER, &draft.draft_id, &markdown)?;
    }
    let published = publisher.publish(
        task,
        OWNER,
        &draft.draft_id,
        &format!("{request}:publish"),
        Publication {
            title: &title,
            summary: &summary,
            author: OWNER,
            source_revision: 0,
            artifacts: serde_json::json!([]),
        },
    )?;
    Ok(Some(published.report_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lifecycle::TaskMachine,
        store::{OpenMode, ProjectKey, StoreChannel},
    };
    use std::os::unix::fs::DirBuilderExt;
    struct Root(std::path::PathBuf);
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn editions(s: &Store) -> Vec<(String, String)> {
        s.read(|db| {
            Ok(db
                .prepare("SELECT e.title,e.summary FROM editions e JOIN reports r ON r.id=e.report_id WHERE r.kind='pipeline' ORDER BY e.sequence")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .unwrap()
    }
    #[test]
    fn task_start_and_each_later_state_publish_one_edition_newest_first() {
        let root =
            Root(std::env::temp_dir().join(format!("cortex-pipeline-{}", uuid::Uuid::new_v4())));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root.0)
            .unwrap();
        let s = Store::open(
            ProjectKey::new(&root.0, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        TaskMachine::new(&s)
            .create("t", "Build the thing\nwith care", "owner")
            .unwrap();
        let route = Route {
            engine: "codex-acp",
            model: "fixture-model",
            effort: Some("low"),
        };
        let id = sync(&s, "t", Some(&route)).unwrap().unwrap();
        // Unchanged durable state publishes nothing more.
        assert_eq!(sync(&s, "t", None).unwrap(), None);
        assert_eq!(
            editions(&s),
            vec![(INITIAL_TITLE.to_owned(), INITIAL_SUMMARY.to_owned())]
        );
        let file = s.key().directory().join("t/pipeline.md");
        let first = std::fs::read_to_string(&file).unwrap();
        assert!(first.contains("> Build the thing\n> with care"));
        assert!(first.contains("engine `codex-acp`, model `fixture-model`, effort `low`"));
        assert!(!first.contains("{{") && !first.contains("<!--"));
        // A transition committed without its edition (a crash before publication)
        // gets the missing edition from the next boundary.
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(sync(&s, "t", None).unwrap().as_deref(), Some(id.as_str()));
        assert_eq!(sync(&s, "t", None).unwrap(), None);
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='COMPLETED',synthesis='Done and verified.' WHERE task_id='t'", [])?;
            db.execute("UPDATE runtime_nodes SET status='completed',result_json='{\"report_ref\":\"r_000000000001\"}'", [])?;
            Ok(())
        })
        .unwrap();
        sync(&s, "t", None).unwrap().unwrap();
        let titles: Vec<_> = editions(&s).into_iter().map(|e| e.0).collect();
        assert_eq!(
            titles,
            ["Initial pipeline", "Runtime EXECUTING", "Runtime COMPLETED"]
        );
        let body = std::fs::read_to_string(&file).unwrap();
        let (newest, older) = body.split_once("\n\n---\n\n").unwrap();
        assert!(
            newest.contains("State: COMPLETED")
                && newest.contains("## Synthesis\n\nDone and verified.")
        );
        assert!(newest.contains(
            "- n (worker-backend-dev): completed; dependencies: none; report: r_000000000001"
        ));
        assert!(
            older.contains("State: EXECUTING")
                && older.ends_with(&first[first.find("# Pipeline").unwrap()..])
        );
        // One pipeline report per task; a model draft of the pipeline stays refused.
        assert_eq!(
            Publisher::new(&s)
                .create_draft("t", "coordinator", "pipeline", "k")
                .unwrap_err()
                .code(),
            "runtime_owned_operation"
        );
        let page = Publisher::new(&s)
            .read_report("t", &id, None, 4000)
            .unwrap();
        assert!(page.markdown.starts_with("Cortex draft ID: `"));
    }
}
