//! `read_batch` admission: the advertised bounds, checked before any member runs.
//! Execution re-enters the ordinary handler of each member (see
//! `CorePolicy::batch`), so a batched read has exactly a direct call's authority.
use serde_json::Value;
use std::time::Duration;

/// Evidence reads a batch may contain: never a write, command, project or skill read.
pub(crate) const READS: [&str; 6] = [
    "list_reports",
    "list_sources",
    "read_draft",
    "read_large_output",
    "read_report",
    "read_source",
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
        for (args, code, field) in [
            (
                json!({"calls":[{"id":"a","tool":"project_read","arguments":{"path":"x"}}]}),
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
