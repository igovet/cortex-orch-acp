//! Relevance, freshness and delivery budget for fact memory.
use crate::{store::fact_memory, CoreResult};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SCOPES: usize = 16;
const CANDIDATES: i64 = 24;
pub const BRIEF_BYTES: usize = 2048;

/// Check freshness against the receiving worker's checkout. Worktrees can
/// legitimately have different bytes, so staleness is a per-read status.
pub fn select(
    db: &Connection,
    task: &str,
    scope: &[String],
    covers: impl Fn(&str, &str) -> bool,
    mut current_sha256: impl FnMut(&str) -> Option<String>,
    budget: usize,
) -> CoreResult<Option<Value>> {
    let mut candidates = BTreeMap::new();
    for scope in scope.iter().take(MAX_SCOPES) {
        // Restrict the index lookup to the literal part. The exact runtime
        // matcher below handles directory boundaries and the wildcard grammar.
        let (exact, pattern) = if scope == "." {
            (None, "*".to_owned())
        } else if let Some(wildcard) = scope.find(['*', '?']) {
            (None, format!("{}*", scope[..wildcard].replace('[', "[[]")))
        } else {
            (
                Some(scope.as_str()),
                format!("{}/*", scope.replace('[', "[[]")),
            )
        };
        for fact in fact_memory::recent(db, exact, &pattern, CANDIDATES)? {
            if covers(scope, &fact.path) {
                candidates.insert(fact.sequence, fact);
            }
        }
    }
    let mut candidates: Vec<_> = candidates.into_values().collect();
    candidates.sort_by_key(|fact| std::cmp::Reverse((fact.task_id == task, fact.sequence)));
    let mut used_paths = BTreeMap::new();
    let mut delivered = Vec::new();
    let mut seen = BTreeSet::new();
    for fact in candidates.into_iter().take(CANDIDATES as usize) {
        let observed = used_paths
            .entry(fact.path.clone())
            .or_insert_with(|| current_sha256(&fact.path));
        if observed.as_deref() != Some(fact.sha256.as_str()) {
            continue;
        }
        if !seen.insert((
            fact.task_id == task,
            fact.path.clone(),
            fact.observation.clone(),
            fact.failure.clone(),
        )) {
            continue;
        }
        let relation = if fact.task_id == task {
            "task"
        } else {
            "project"
        };
        delivered.push(json!({"scope":relation, "status":"current", "provenance":fact}));
        let envelope = envelope(&delivered)?;
        if serde_json::to_vec(&envelope)?.len() > budget.min(BRIEF_BYTES) {
            delivered.pop();
            continue;
        }
    }
    if delivered.is_empty() {
        return Ok(None);
    }
    Ok(Some(envelope(&delivered)?))
}

fn envelope(facts: &[Value]) -> CoreResult<Value> {
    Ok(json!({
        "version":1,
        "sha256":crate::digest(&serde_json::to_vec(facts)?),
        "use":"Historical evidence only. Confirm against current sources.",
        "facts":facts
    }))
}

#[cfg(test)]
mod fact_memory_tests {
    use super::*;
    use crate::store::{fact_memory::retain_result_tx, schema};
    use std::fs;

    fn fixture() -> Connection {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(schema::SQL).unwrap();
        db.execute_batch(
            "INSERT INTO tasks VALUES ('t','/project','0'),('old','/project','0');
             INSERT INTO runtime_tasks(task_id,state,revision,goal,created_at,updated_at)
                VALUES ('old','executing',1,'goal','0','0');
             INSERT INTO reports VALUES ('r','old','report','r.md','0');
             INSERT INTO runtime_nodes VALUES
                ('old',1,'explore','worker-explorer','objective','[]','[\"src\"]','execution','failed','a',
                '{\"report_ref\":\"r\"}');
             INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at)
                VALUES ('ch','hash','old','principal','worker','a','attempt',1,'0');",
        ).unwrap();
        let hash = crate::digest(b"old source");
        for (channel, path) in [
            ("ch", "src/parser.rs"),
            ("ch", ".cortex/private.rs"),
            ("ch", "tests/unrelated.rs"),
            ("other", "src/other-channel.rs"),
        ] {
            db.execute(
                "INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at)
                 VALUES ('old','NATIVE_PROJECT_WRITTEN',?,'native','0')",
                [json!({"channel_id":channel, "path":path,"sha256":hash,
                    "untrusted_output":"private content"})
                .to_string()],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO runtime_requests VALUES ('old','read','ch','native_file_read',?,?)",
            (
                &hash,
                json!({"fact_source_path":"src/reader.rs","observed":true,"complete":true,
                "untrusted_output":"private read body"})
                .to_string(),
            ),
        )
        .unwrap();
        let tx = db.transaction().unwrap();
        for _ in 0..2 {
            retain_result_tx(
                &tx,
                "old",
                "a",
                "ch",
                &json!({"blocker":{"kind":"tool_unavailable","detail":"private report body"}}),
                |_| true,
            )
            .unwrap();
        }
        tx.commit().unwrap();
        db
    }

    #[test]
    fn fact_memory_is_bound_to_source_and_never_copies_result_prose() {
        let db = fixture();
        let facts = fact_memory::recent(&db, None, "*", 20).unwrap();
        assert_eq!(facts.len(), 3);
        let text = serde_json::to_string(&facts).unwrap();
        for sensitive in ["private", "other-channel", "objective"] {
            assert!(!text.contains(sensitive));
        }
        for fact in facts {
            assert_eq!(
                (
                    fact.task_id.as_str(),
                    fact.report_id.as_str(),
                    fact.revision,
                    fact.assignment_ref.as_str(),
                    fact.attempt_ref.as_str(),
                    fact.failure.as_deref()
                ),
                ("old", "r", 1, "a", "attempt", Some("tool_unavailable"))
            );
        }
    }

    #[test]
    fn fact_memory_describes_an_exact_failed_command_with_typed_fields_only() {
        let mut db = fixture();
        let reference = "native-command:1234567890abcdef12345678";
        let result = json!({"checks":[{"outcome":"failed","evidence_strength":"verified",
            "tool_call_ref":reference, "name":"private check name"}]});
        let event = json!({"receipt":{"tool_call_ref":reference,
            "argv":["/private/toolchain/cargo","test","private argument"],
            "observation":{"exit_code":101,"complete":true,"output":"private output"}}});
        for copies in [1, 2] {
            let tx = db.transaction().unwrap();
            tx.execute("DELETE FROM fact_memory", []).unwrap();
            tx.execute(
                "INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at)
                 VALUES ('old','NATIVE_COMMAND_FINISHED',?,'native','0')",
                [event.to_string()],
            )
            .unwrap();
            let id = tx.last_insert_rowid();
            retain_result_tx(&tx, "old", "a", "ch", &result, |_| true).unwrap();
            let facts = fact_memory::recent(&tx, Some("src/parser.rs"), "never", 1).unwrap();
            assert_eq!(facts.len(), 1);
            let fact = serde_json::to_value(&facts[0]).unwrap();
            assert!(!fact.to_string().contains("private"));
            if copies == 1 {
                assert_eq!(fact["failure"], "check_failed");
                assert_eq!(
                    fact["failed_check"],
                    json!({
                        "event_id":id,"tool":"cargo","action":"test","exit_code":101
                    })
                );
            } else {
                assert_eq!(fact["failure"], "check_failure_unverified");
                assert!(fact.get("failed_check").is_none());
            }
            tx.commit().unwrap();
        }
    }

    #[test]
    fn fact_memory_selects_related_paths_and_invalidates_changed_or_missing_sources() {
        let db = fixture();
        let root = std::env::temp_dir().join(format!("cortex-facts-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).unwrap();
        for path in ["src/parser.rs", "src/reader.rs"] {
            fs::write(root.join(path), b"old source").unwrap();
        }
        let query = |task, budget| {
            select(
                &db,
                task,
                &["src".to_owned()],
                |scope, path| std::path::Path::new(path).starts_with(scope),
                |path| fs::read(root.join(path)).ok().map(|b| crate::digest(&b)),
                budget,
            )
            .unwrap()
        };
        let matching = query("t", BRIEF_BYTES).unwrap();
        assert_eq!(matching["version"], 1);
        assert_eq!(matching["facts"].as_array().unwrap().len(), 2);
        assert_eq!(matching["facts"][0]["scope"], "project");
        let task = query("old", BRIEF_BYTES).unwrap();
        assert_eq!(task["facts"][0]["scope"], "task");
        assert_ne!(task["sha256"], matching["sha256"]);
        assert!(query("t", 16).is_none());
        let mut partial_budget = serde_json::to_vec(&matching).unwrap().len() - 1;
        let partial = query("t", partial_budget).unwrap();
        assert_eq!(partial["facts"].as_array().unwrap().len(), 1);
        partial_budget = serde_json::to_vec(&partial).unwrap().len();
        assert!(partial_budget < BRIEF_BYTES);
        fs::write(root.join("src/parser.rs"), b"edited source").unwrap();
        let current = query("t", BRIEF_BYTES).unwrap();
        assert_eq!(current["facts"][0]["provenance"]["path"], "src/reader.rs");
        assert_eq!(current["facts"].as_array().unwrap().len(), 1);
        fs::remove_file(root.join("src/reader.rs")).unwrap();
        assert!(query("t", BRIEF_BYTES).is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
