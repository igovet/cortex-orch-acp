//! Read-only recovery and preflight tools: `task_get_context` (coordinator),
//! `read_capsule` (every channel) and `check_preflight` (every role that runs
//! checks). The recipient is always the authenticated channel itself; no
//! argument selects another task, role or assignment.
use cortex_core::{channels::Identity, recovery_context, CoreError, CoreResult, Store};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

pub(super) fn supports(name: &str) -> bool {
    matches!(
        name,
        "task_get_context"
            | "read_capsule"
            | "check_preflight"
            | "sprint_graph"
            | "sprint_task_result"
    )
}

const NOTHING_NEEDED: &str = "Nothing here needs you: workers are dispatched and the runtime decides next steps. End this turn now instead of polling task_get_context again; the runtime resumes you with a fresh message when a result, question or decision needs you.";
/// Digest of each channel's last `task_get_context` reply, so an unchanged repeat
/// poll costs a compact receipt. Bounded; a lost entry only costs one full resend.
const CONTEXT_MEMORY: usize = 256;
fn last_reads() -> &'static Mutex<HashMap<String, String>> {
    static LAST: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(HashMap::new()))
}
fn served_tools(i: &Identity) -> Vec<Value> {
    cortex_engine::schema::catalogue(&i.role, i.profile.as_deref(), i.stage.as_deref())
        .unwrap_or_default()
        .into_iter()
        .filter(|t| {
            crate::native_instructions::supports(
                i.stage.as_deref(),
                t["name"].as_str().unwrap_or(""),
            )
        })
        .map(|t| t["name"].clone())
        .collect()
}

/// The task's current runtime context: state, original goal, validated plan,
/// node results, questions with their answers and retained constraints.
pub(super) fn task_context(store: &Store, i: &Identity, channel: &str) -> CoreResult<Value> {
    if i.role != "coordinator" {
        return Err(CoreError::Refused("tool_refused"));
    }
    let mut value = store.read(|db| recovery_context::snapshot(db, &i.task))?;
    let nodes_failed = value["current_nodes"]
        .as_array()
        .is_some_and(|n| n.iter().any(|n| n["status"] == "failed"));
    let open_question = value["questions"]
        .as_array()
        .is_some_and(|q| q.iter().any(|q| q["status"] != "answered"));
    let nothing = matches!(
        value["state"].as_str(),
        Some("PLAN_READY" | "EXECUTING" | "REVIEWING")
    ) && !nodes_failed
        && !open_question;
    if nothing {
        value["hint"] = json!(NOTHING_NEEDED);
    }
    let digest = crate::native_project::digest(value.to_string().as_bytes());
    let unchanged = {
        let mut last = last_reads().lock().unwrap();
        let unchanged = last.get(channel) == Some(&digest);
        if last.len() >= CONTEXT_MEMORY && !last.contains_key(channel) {
            last.clear();
        }
        last.insert(channel.to_owned(), digest);
        unchanged
    };
    if !unchanged {
        return crate::native_commands::defer_json(store, i, value);
    }
    let mut compact = json!({"unchanged_since_last_read":true,"state":value["state"],"revision":value["revision"]});
    if nothing {
        compact["hint"] = json!(NOTHING_NEEDED);
    }
    Ok(compact)
}

/// This channel's own recovery capsule. `at_revision` bounds the captured
/// `sources` section; every other section is current durable state.
pub(super) fn capsule(store: &Store, i: &Identity, args: &Value) -> CoreResult<Value> {
    if i.role == "worker" && i.assignment.is_none() {
        return Err(CoreError::Refused("capsule_unavailable"));
    }
    let mut value =
        store.read(|db| recovery_context::snapshot_for(db, &i.task, i.assignment.as_deref()))?;
    if let Some(bound) = args["at_revision"].as_i64() {
        let current: i64 = store.read(|db| {
            Ok(db.query_row(
                "SELECT coalesce(max(revision),0) FROM source_revisions WHERE task_id=?",
                [&i.task],
                |r| r.get(0),
            )?)
        })?;
        if bound > current {
            return Err(CoreError::Refused("at_revision_out_of_range"));
        }
        if let Some(sources) = value["sources"].as_array_mut() {
            sources.retain(|s| s["revision"].as_i64().is_some_and(|r| r <= bound));
        }
        value["at_revision"] = json!({"revision":bound,"bounds":["sources"]});
    }
    value["recipient"] = json!({"role":i.role,"profile":i.profile,"stage":i.stage,
        "assignment":i.assignment,"attempt":i.attempt});
    value["role_and_tool_access"] = json!({"tools":served_tools(i)});
    crate::native_commands::defer_json(store, i, value)
}

const PROGRAM_MISSING: &str = "The check program is unavailable in its execution environment. Install the required tool in an isolated project-private tools directory (for example .venv), without sudo or system changes, or choose another executable check. Probe again before depending on it. If neither is possible, ask the owner now through ask_question (workers) or user_question_request (coordinator); do not defer the decision until the result.";

/// Availability only, through the same probe that plan and verification
/// admission use. Never runs the check and never counts as test evidence.
pub(super) async fn preflight(root: std::path::PathBuf, args: &Value) -> CoreResult<Value> {
    let argv = args["argv"].clone();
    let (available, reason, message) =
        match crate::native_effects::verification_preflight(root, &json!([argv])).await {
            Ok(()) => (true, Value::Null, "Check program available; the check has not run."),
            Err(CoreError::Refused("verification_executable_unavailable")) => {
                (false, json!("program_missing"), PROGRAM_MISSING)
            }
            Err(CoreError::Refused("native_verification_sandbox_unavailable")) => (
                false,
                json!("probe_unavailable"),
                "The verification environment could not complete the availability probe. Restore the verification sandbox and probe again; this does not establish a missing program.",
            ),
            Err(error) => return Err(error),
        };
    Ok(json!({"available":available,"reason":reason,"message":message,"evidence":false}))
}

/// A sibling's published evidence is readable only under the calling coordinator's
/// live Workspace membership. Ordinary document tools keep their original scope.
pub(super) fn sprint_read(
    store: &Store,
    credential: &cortex_core::channels::Credential,
    identity: &Identity,
    state: Option<&std::path::Path>,
    recipient: &std::path::Path,
    name: &str,
    args: &Value,
) -> CoreResult<Value> {
    use crate::machine::{sprint_graph::Scope, sprint_result};
    if identity.role != "coordinator" {
        return Err(CoreError::Refused("tool_refused"));
    }
    crate::native_documents::validate(identity, name, args)?;
    let state = state.ok_or(CoreError::Refused("sprint_membership_required"))?;
    let scope = store
        .read(|db| Scope::load(db, state, &identity.task))?
        .ok_or(CoreError::Refused("sprint_membership_required"))?;
    let value = match name {
        "sprint_graph" => scope.graph(args)?,
        "sprint_task_result" => sprint_result::read(store, &scope, args, recipient)?,
        _ => return Err(CoreError::Refused("tool_refused")),
    };
    let current = store.read(|db| Scope::load(db, state, &identity.task))?;
    if !current
        .as_ref()
        .is_some_and(|current| scope.same_members(current))
    {
        return Err(CoreError::Refused("sprint_membership_changed"));
    }
    crate::native_documents::authenticate(store, credential, identity)?;
    crate::native_commands::defer_json(store, identity, value)
}
