//! `read_batch` admission: the advertised bounds, checked before any member runs.
//! Execution re-enters the ordinary handler of each member (see
//! `CorePolicy::batch`), so a batched read has exactly a direct call's authority.
use serde_json::Value;
use std::time::Duration;

/// Read-only tools a batch may contain: no mutation, command launch, question,
/// submission, request, availability probe or recursion. Each still runs through
/// its own handler, so a batched read has exactly a direct call's authority.
pub(crate) const READS: [&str; 14] = [
    "command_status",
    "list_reports",
    "list_route_options",
    "list_sources",
    "project_git",
    "project_read",
    "read_capsule",
    "read_draft",
    "read_large_output",
    "read_report",
    "read_skill",
    "read_source",
    "task_get_context",
    "tools_discover",
];
pub(crate) const REQUEST_BYTES: usize = 32768;
pub(crate) const ITEM_BYTES: usize = 6144;
pub(crate) const RESULT_BYTES: usize = 65536;
pub(crate) const ADMISSION: Duration = Duration::from_secs(5);
const DEPTH: usize = 16;

fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// Reject the whole batch, naming the first offending location, before anything runs.
/// `offered` is the channel's effective tool list (what tools/list returns).
pub(crate) fn preflight(offered: &[Value], args: &Value) -> Result<(), (&'static str, String)> {
    if serde_json::to_vec(args)
        .map(|b| b.len())
        .unwrap_or(usize::MAX)
        > REQUEST_BYTES
    {
        return Err(("request_too_large", "calls".into()));
    }
    if depth(args) > DEPTH {
        return Err(("invalid_arguments", "calls".into()));
    }
    let calls = args["calls"]
        .as_array()
        .ok_or(("invalid_arguments", "calls".to_owned()))?;
    let mut seen = std::collections::BTreeSet::new();
    for (index, call) in calls.iter().enumerate() {
        let id = call["id"].as_str().unwrap_or("");
        if !seen.insert(id.to_owned()) {
            return Err(("invalid_arguments", format!("calls[{index}].id")));
        }
        let name = call["tool"].as_str().unwrap_or("");
        let Some(tool) = offered
            .iter()
            .find(|t| t["name"] == name)
            .filter(|_| READS.contains(&name))
        else {
            return Err(("tool_not_available", format!("calls[{index}].tool")));
        };
        if cortex_engine::schema::validate(&tool["inputSchema"], &call["arguments"]).is_err() {
            return Err(("invalid_arguments", format!("calls[{index}].arguments")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn admission_refuses_unlisted_unoffered_duplicate_and_oversized_batches() {
        let offered = cortex_engine::schema::catalogue(
            "worker",
            Some("worker-backend-dev"),
            Some("execution"),
        )
        .unwrap();
        let ok = json!({"calls":[{"id":"a","tool":"list_reports","arguments":{}}]});
        preflight(&offered, &ok).unwrap();
        // Every newly batchable read a worker is offered passes admission.
        let widened = json!({"calls":[
            {"id":"f","tool":"project_read","arguments":{"path":"README.md"}},
            {"id":"d","tool":"project_read","arguments":{"path":"."}},
            {"id":"g","tool":"project_git","arguments":{"kind":"status","request_key":"k"}},
            {"id":"s","tool":"read_skill","arguments":{"skill":"tool-discipline"}},
            {"id":"c","tool":"command_status","arguments":{"tool_call_ref":"native-command:x"}},
            {"id":"p","tool":"read_capsule","arguments":{}},
            {"id":"t","tool":"tools_discover","arguments":{"name":"project_read"}}]});
        preflight(&offered, &widened).unwrap();
        // Coordinator-only reads pass for the coordinator and are unoffered to a worker.
        let coordinator = cortex_engine::schema::catalogue("coordinator", None, None).unwrap();
        let planning = json!({"calls":[{"id":"r","tool":"list_route_options","arguments":{}},
            {"id":"x","tool":"task_get_context","arguments":{}}]});
        preflight(&coordinator, &planning).unwrap();
        assert_eq!(
            preflight(&offered, &planning),
            Err(("tool_not_available", "calls[0].tool".to_owned()))
        );
        // A batchable tool keeps its own argument contract.
        assert_eq!(
            preflight(
                &offered,
                &json!({"calls":[{"id":"g","tool":"project_git","arguments":{"kind":"log","request_key":"k"}}]})
            ),
            Err(("invalid_arguments", "calls[0].arguments".to_owned()))
        );
        // Every advertised read_batch enum lists only batchable tools that catalogue serves.
        for (role, profile, stage) in [
            ("coordinator", None, None),
            ("worker", Some("worker-backend-dev"), Some("review")),
            (
                "worker",
                Some("worker-senior-consultant"),
                Some("execution"),
            ),
        ] {
            let tools = cortex_engine::schema::catalogue(role, profile, stage).unwrap();
            let batch = tools.iter().find(|t| t["name"] == "read_batch").unwrap();
            for name in batch["inputSchema"]["properties"]["calls"]["items"]["properties"]["tool"]
                ["enum"]
                .as_array()
                .unwrap()
            {
                let name = name.as_str().unwrap();
                assert!(READS.contains(&name), "{name}");
                assert!(
                    tools.iter().any(|t| t["name"] == name),
                    "{role} {profile:?}: {name}"
                );
            }
        }
        for (args, code, field) in [
            (
                json!({"calls":[{"id":"a","tool":"project_write","arguments":{"path":"x"}}]}),
                "tool_not_available",
                "calls[0].tool",
            ),
            (
                json!({"calls":[{"id":"a","tool":"list_reports","arguments":{}},{"id":"a","tool":"list_reports","arguments":{}}]}),
                "invalid_arguments",
                "calls[1].id",
            ),
            (
                json!({"calls":[{"id":"a","tool":"read_report","arguments":{"report_id":"bad"}}]}),
                "invalid_arguments",
                "calls[0].arguments",
            ),
            (
                json!({"calls":[{"id":"a","tool":"read_report","arguments":{"cursor":"x".repeat(40000)}}]}),
                "request_too_large",
                "calls",
            ),
        ] {
            assert_eq!(
                preflight(&offered, &args),
                Err((code, field.to_owned())),
                "{args}"
            );
        }
        // A read the role is not offered is refused even when allowlisted.
        let without: Vec<_> = offered
            .iter()
            .filter(|t| t["name"] != "read_draft")
            .cloned()
            .collect();
        let draft = json!({"calls":[{"id":"d","tool":"read_draft","arguments":{"draft_id":"d_000000000000"}}]});
        assert_eq!(
            preflight(&without, &draft).unwrap_err().0,
            "tool_not_available"
        );
    }
}
