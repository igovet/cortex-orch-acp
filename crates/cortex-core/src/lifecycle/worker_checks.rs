//! Worker check history is evidence, not a model-provided certification.
//! Parity reference: delegation/evidence.py current_outcomes/accept_checks.
use super::{completion::trivial_command, probe_correction};
use crate::{CoreError, CoreResult};
use rusqlite::Connection;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Run {
    pub operation: String,
    pub receipt: Value,
    pub order: i64,
    pub output_verified: bool,
}
impl Run {
    fn reference(&self) -> &str {
        self.receipt["tool_call_ref"].as_str().unwrap_or("")
    }
    fn identity(&self) -> bool {
        self.receipt["argv"]
            .as_array()
            .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_string))
            && self.receipt["cwd"].is_string()
            && self.receipt["profile"].is_string()
    }
    fn same(&self, other: &Self) -> bool {
        self.identity()
            && other.identity()
            && self.receipt["argv"] == other.receipt["argv"]
            && self.receipt["cwd"] == other.receipt["cwd"]
            && (self.operation != other.operation
                || self.receipt["profile"] == other.receipt["profile"])
    }
    pub fn verified_failure(&self) -> bool {
        self.complete() && self.receipt["exit_code"].as_i64().is_some_and(|c| c != 0)
    }
    pub fn presented_failure(&self) -> bool {
        self.verified_failure()
            || self.receipt["complete"] != true
            || !self.receipt["failure"].is_null()
            || self.receipt["exit_code"].as_i64().is_none()
    }
    fn complete(&self) -> bool {
        self.receipt["complete"] == true
            && self.receipt["failure"].is_null()
            && self.receipt["exit_code"].as_i64().is_some()
            && self.output_verified
    }
}

/// Only the authenticated attempt's channels may be passed here. Keep pending
/// requests too: a later unknown identity must prevent closing an old failure.
pub fn load_runs(db: &Connection, task: &str, channel: &str) -> CoreResult<Vec<Run>> {
    // Request intents are inserted serially before launch. Use their single
    // order domain even for pending requests without a started event.
    let mut q = db.prepare(
        "SELECT r.operation,r.result_json,r.rowid
        FROM runtime_requests r WHERE r.task_id=? AND r.actor IN
          (SELECT channel_id FROM trusted_channels WHERE task_id=? AND
            (channel_id=? OR (attempt_id IS NOT NULL AND attempt_id=
              (SELECT attempt_id FROM trusted_channels WHERE channel_id=?))))
        AND r.operation IN ('command_run','outside_command') ORDER BY 3,r.rowid",
    )?;
    let rows = q
        .query_map((task, task, channel, channel), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(operation, raw, order)| {
            let saved: Value = serde_json::from_str(&raw)?;
            let receipt = if operation == "outside_command" {
                saved["reply"]["structuredContent"].clone()
            } else {
                saved
            };
            Ok(Run {
                operation,
                receipt,
                order,
                output_verified: false,
            })
        })
        .collect()
}

/// Only cited commands and their matching history need full output hashing.
/// Unrelated identities still remain in history, including unknown identities.
pub fn relevant_references(checks: &[Value], runs: &[Run]) -> std::collections::BTreeSet<String> {
    let cited: std::collections::BTreeSet<_> = checks
        .iter()
        .filter(|c| matches!(c["outcome"].as_str(), Some("passed" | "failed")))
        .filter_map(|c| c["tool_call_ref"].as_str())
        .collect();
    let identities: Vec<_> = runs
        .iter()
        .filter(|r| cited.contains(r.reference()))
        .collect();
    runs.iter()
        .filter(|r| cited.contains(r.reference()) || identities.iter().any(|c| r.same(c)))
        .map(|r| r.reference().to_owned())
        .collect()
}

/// Snapshot hashes are runtime observations, never model-supplied links.
pub fn observe_outputs(
    db: &Connection,
    task: &str,
    runs: &mut [Run],
    artifacts: &std::collections::BTreeMap<String, String>,
) -> CoreResult<()> {
    use rusqlite::OptionalExtension;
    for run in runs {
        let Some(id) = run.receipt["large_output_id"].as_str() else {
            continue;
        };
        if !artifacts.contains_key(id) {
            continue;
        }
        let row:Option<(String,i64,bool,Option<i64>,Option<String>)>=db.query_row(
            "SELECT source_ref,byte_count,truncated,exit_code,failure FROM large_outputs WHERE task_id=? AND id=?",
            (task,id),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        if let Some((hash, count, truncated, code, failure)) = row {
            run.output_verified = artifacts.get(id) == Some(&hash)
                && !truncated
                && run.receipt["byte_count"].as_i64() == Some(count)
                && run.receipt["exit_code"].as_i64() == code
                && run.receipt["failure"] == json!(failure);
        }
    }
    Ok(())
}

#[derive(Clone)]
struct Assessment {
    run: Option<usize>,
    strength: &'static str,
    gap: Option<&'static str>,
}
fn assess(check: &Value, runs: &[Run], digest: Option<&str>, duplicate: bool) -> Assessment {
    let weak = |gap| Assessment {
        run: None,
        strength: "weak",
        gap: Some(gap),
    };
    if !matches!(check["outcome"].as_str(), Some("passed" | "failed")) {
        return Assessment {
            run: None,
            strength: "none",
            gap: None,
        };
    }
    let Some(reference) = check["tool_call_ref"].as_str().filter(|s| !s.is_empty()) else {
        return weak("tool_call_ref_missing");
    };
    if duplicate {
        return weak("ref_already_cited");
    }
    let found: Vec<_> = runs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.reference() == reference)
        .collect();
    if found.len() != 1 {
        return weak(if found.is_empty() {
            "tool_call_ref_unknown"
        } else {
            "ambiguous_ref"
        });
    }
    let (index, run) = found[0];
    let gap = if !run.complete() {
        Some("call_incomplete")
    } else if check["outcome"] == "passed" && run.receipt["exit_code"] != 0 {
        Some("nonzero_exit")
    } else if check["outcome"] == "failed" && run.receipt["exit_code"] == 0 {
        Some("zero_exit")
    } else if !run.identity() {
        Some("command_unrecorded")
    } else if check["outcome"] == "passed"
        && serde_json::from_value::<Vec<String>>(run.receipt["argv"].clone())
            .map(|a| trivial_command(&a))
            .unwrap_or(true)
    {
        Some("trivial_command")
    } else if check["outcome"] == "passed"
        && run.receipt["revision_digest"]
            .as_str()
            .is_none_or(|d| d.len() != 64 || Some(d) != digest)
    {
        Some("revision_mismatch")
    } else {
        None
    };
    Assessment {
        run: Some(index),
        strength: if gap.is_none() { "verified" } else { "weak" },
        gap,
    }
}

/// Recompute, including all uncited later runs. Retain failed history and derive
/// closure/retraction links; never accept a link supplied by the model.
pub fn evaluate(checks: &[Value], runs: &[Run], digest: Option<&str>) -> CoreResult<Vec<Value>> {
    let mut cited = std::collections::BTreeSet::new();
    let assessments: Vec<_> = checks
        .iter()
        .map(|c| {
            let duplicate = c["tool_call_ref"]
                .as_str()
                .is_some_and(|r| !cited.insert(r.to_owned()));
            assess(c, runs, digest, duplicate)
        })
        .collect();
    let mut out = Vec::new();
    for (check, a) in checks.iter().zip(&assessments) {
        let mut c = check.clone();
        for field in [
            "closed_by",
            "retracted_by",
            "superseded_by",
            "current_outcome",
            "evidence_gap",
            "claimed_outcome",
            "revision_digest",
        ] {
            c.as_object_mut()
                .ok_or(CoreError::Refused("invalid_native_result"))?
                .remove(field);
        }
        c["evidence_strength"] = json!(a.strength);
        if let Some(gap) = a.gap {
            c["evidence_gap"] = json!(gap)
        }
        if check["outcome"] == "passed" {
            let mut gap = a.gap;
            if a.strength == "verified" {
                let run = &runs[a.run.unwrap()];
                c["revision_digest"] = run.receipt["revision_digest"].clone();
                if let Some(latest) = runs.iter().filter(|r| r.same(run)).max_by_key(|r| r.order) {
                    if latest.order > run.order {
                        if !latest.complete() {
                            gap = Some("later_run_unresolved")
                        } else if latest.receipt["exit_code"] != 0 {
                            gap = Some("later_run_failed")
                        }
                    }
                }
            }
            if let Some(gap) = gap {
                c["outcome"] = json!(if matches!(gap, "nonzero_exit" | "later_run_failed") {
                    "failed"
                } else {
                    "unverified"
                });
                c["claimed_outcome"] = json!("passed");
                c["evidence_gap"] = json!(gap);
                c["evidence_strength"] = json!(if c["outcome"] == "unverified" {
                    "none"
                } else {
                    assess(&c, runs, digest, false).strength
                });
                if c["outcome"] == "unverified" {
                    c["blocker"] =
                        json!(format!("Runtime kept this passed claim unverified: {gap}."));
                }
            }
        } else if check["outcome"] == "failed" && a.strength == "verified" {
            let run = &runs[a.run.unwrap()];
            let latest = runs.iter().filter(|r| r.same(run)).max_by_key(|r| r.order);
            if let Some(latest) = latest
                .filter(|r| r.order > run.order && r.complete() && r.receipt["exit_code"] == 0)
            {
                let cited: Vec<_> = checks
                    .iter()
                    .zip(&assessments)
                    .filter(|(c, a)| {
                        c["outcome"] == "passed"
                            && c["name"] == check["name"]
                            && c["tool_call_ref"] == latest.reference()
                            && a.strength == "verified"
                    })
                    .collect();
                if cited.len() == 1 && !runs.iter().any(|r| r.order > run.order && !r.identity()) {
                    c["closed_by"] = json!(latest.reference());
                }
            }
        }
        out.push(c);
    }
    for (index, check) in checks.iter().enumerate() {
        let a = &assessments[index];
        if check["probe_withdrawn"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
            && check["outcome"] == "failed"
            && a.strength == "verified"
            && out[index]["closed_by"].is_null()
        {
            out[index]["retracted_by"] = json!("probe_withdrawn");
            out[index]["current_outcome"] = json!("retracted");
        }
        for (field, derived, code) in [
            (
                "invocation_error",
                "retracted_by",
                "invalid_check_retraction",
            ),
            (
                "command_correction",
                "superseded_by",
                "invalid_check_supersession",
            ),
        ] {
            let Some(request) = check.get(field) else {
                continue;
            };
            let target = checks
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c["outcome"] == "passed"
                        && c["name"] == check["name"]
                        && c["tool_call_ref"] == request["corrected_by"]
                })
                .collect::<Vec<_>>();
            let uses = checks
                .iter()
                .flat_map(|c| ["invocation_error", "command_correction"].map(move |f| &c[f]))
                .filter(|r| r.is_object() && r["corrected_by"] == request["corrected_by"])
                .count();
            let eligible = || -> Option<&str> {
                if check["outcome"] != "failed"
                    || a.strength != "verified"
                    || target.len() != 1
                    || uses != 1
                    || request["reason"]
                        .as_str()
                        .is_none_or(|s| s.trim().is_empty())
                {
                    return None;
                }
                let (t, _) = target[0];
                if assessments[t].strength != "verified" || out[t]["outcome"] != "passed" {
                    return None;
                }
                let bad = &runs[a.run?];
                let good = &runs[assessments[t].run?];
                if bad.order >= good.order
                    || bad.operation != good.operation
                    || bad.receipt["cwd"] != good.receipt["cwd"]
                    || bad.receipt["profile"] != good.receipt["profile"]
                    || !bad.receipt["runtime_verification"].is_null()
                    || !good.receipt["runtime_verification"].is_null()
                    || runs.iter().any(|r| {
                        r.order > bad.order
                            && (!r.identity()
                                || (r.same(bad) && (!r.complete() || r.receipt["exit_code"] != 0)))
                    })
                {
                    return None;
                }
                let argv =
                    serde_json::from_value::<Vec<String>>(bad.receipt["argv"].clone()).ok()?;
                let corrected =
                    serde_json::from_value::<Vec<String>>(good.receipt["argv"].clone()).ok()?;
                let output = bad.receipt["output"].as_str()?;
                if bad.receipt["byte_count"].as_u64() != Some(output.len() as u64) {
                    return None;
                }
                if !probe_correction::eligible(
                    &argv,
                    &corrected,
                    output,
                    bad.receipt["exit_code"].as_i64()?,
                    field == "command_correction",
                ) {
                    return None;
                }
                Some(good.reference())
            };
            let reference = eligible().ok_or(CoreError::Refused(code))?;
            for key in ["closed_by", "retracted_by", "superseded_by"] {
                out[index].as_object_mut().unwrap().remove(key);
            }
            out[index][derived] = json!(reference);
            out[index]["current_outcome"] = json!(if field == "command_correction" {
                "superseded"
            } else {
                "retracted"
            });
        }
    }
    Ok(out)
}

pub fn resolved(check: &Value) -> bool {
    check["outcome"] == "failed"
        && ["closed_by", "retracted_by", "superseded_by"]
            .iter()
            .any(|k| check[*k].is_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    fn run(reference: &str, order: i64, code: i64, argv: Value, output: &str) -> Run {
        Run {
            operation: "command_run".into(),
            order,
            output_verified: true,
            receipt: json!({"tool_call_ref":reference,"argv":argv,"cwd":"/fixture","profile":"development",
                "complete":true,"exit_code":code,"failure":null,"revision_digest":DIGEST,
                "output":output,"byte_count":output.len()}),
        }
    }
    fn check(outcome: &str, reference: &str) -> Value {
        json!({"name":"check","outcome":outcome,"tool_call_ref":reference})
    }
    fn python(source: &str) -> Value {
        json!(["python3", "-B", "-c", source])
    }
    fn evaluate_pair(checks: &[Value], runs: &[Run]) -> CoreResult<Vec<Value>> {
        evaluate(checks, runs, Some(DIGEST))
    }
    #[test]
    fn unrelated_output_is_not_hashed_but_matching_uncited_history_is() {
        let runs = vec![
            run("bad", 1, 1, json!(["cargo", "test"]), ""),
            run("rerun", 2, 0, json!(["cargo", "test"]), ""),
            run("unrelated", 3, 0, json!(["cargo", "build"]), ""),
        ];
        assert_eq!(
            relevant_references(&[check("failed", "bad")], &runs),
            ["bad".to_string(), "rerun".to_string()].into()
        );
    }
    // Ported from test_check_history / test_check_closure_fixes cross-operation cases.
    #[test]
    fn exact_latest_rerun_table() {
        for case in [
            "exact",
            "outside",
            "outside_reverse",
            "argv",
            "cwd",
            "profile",
            "name",
            "earlier",
            "uncited_failure",
            "unresolved",
            "unrecorded",
            "uncited_success",
            "duplicate",
            "missing_ref",
        ] {
            let bad = run("bad", 1, 1, json!(["cargo", "test"]), "failed");
            let mut good = run("good", 2, 0, json!(["cargo", "test"]), "passed");
            let mut runs = vec![bad, good.clone()];
            let mut checks = vec![check("failed", "bad"), check("passed", "good")];
            match case {
                "outside" => runs[1].operation = "outside_command".into(),
                "outside_reverse" => runs[0].operation = "outside_command".into(),
                "argv" => runs[1].receipt["argv"] = json!(["cargo", "test", "other"]),
                "cwd" => runs[1].receipt["cwd"] = json!("/other"),
                "profile" => runs[1].receipt["profile"] = json!("guarded"),
                "name" => checks[1]["name"] = json!("other"),
                "earlier" => runs[1].order = 0,
                "uncited_failure" => {
                    good.order = 3;
                    good.receipt["tool_call_ref"] = json!("late");
                    good.receipt["exit_code"] = json!(1);
                    runs.push(good);
                }
                "unresolved" => {
                    good.order = 3;
                    good.receipt["tool_call_ref"] = json!("late");
                    good.receipt["complete"] = json!(false);
                    runs.push(good);
                }
                "unrecorded" => {
                    good.order = 3;
                    good.receipt["tool_call_ref"] = json!("late");
                    good.receipt["argv"] = Value::Null;
                    runs.push(good);
                }
                "uncited_success" => {
                    good.order = 3;
                    good.receipt["tool_call_ref"] = json!("new");
                    runs.push(good);
                }
                "duplicate" => checks.push(check("passed", "good")),
                "missing_ref" => checks[1]["tool_call_ref"] = json!("unknown"),
                _ => (),
            }
            let result = evaluate_pair(&checks, &runs).unwrap();
            assert_eq!(result[0]["outcome"], "failed", "{case}");
            assert_eq!(
                result[0]["closed_by"] == "good",
                matches!(case, "exact" | "outside" | "outside_reverse" | "duplicate"),
                "{case}"
            );
            assert_eq!(result[0]["evidence_strength"], "verified", "{case}");
            if case == "uncited_failure" {
                assert_eq!(result[1]["outcome"], "failed");
            }
            if case == "unresolved" {
                assert_eq!(result[1]["outcome"], "unverified");
            }
        }
    }
    #[test]
    fn evidence_strength_and_kept_claims_table() {
        for (case, outcome, strength, gap) in [
            ("valid", "passed", "verified", None),
            ("trivial", "unverified", "none", Some("trivial_command")),
            (
                "unknown",
                "unverified",
                "none",
                Some("tool_call_ref_unknown"),
            ),
            (
                "missing",
                "unverified",
                "none",
                Some("tool_call_ref_missing"),
            ),
            ("incomplete", "unverified", "none", Some("call_incomplete")),
            ("truncated", "unverified", "none", Some("call_incomplete")),
            ("digest", "unverified", "none", Some("revision_mismatch")),
            ("nonzero", "failed", "verified", Some("nonzero_exit")),
            ("not_run", "not_run", "none", None),
            ("unverified", "unverified", "none", None),
        ] {
            let mut runs = vec![run("one", 1, 0, json!(["cargo", "test"]), "ok")];
            let mut c = check("passed", "one");
            match case {
                "trivial" => runs[0].receipt["argv"] = json!(["env", "A=1", "ls"]),
                "unknown" => c["tool_call_ref"] = json!("foreign"),
                "missing" => c
                    .as_object_mut()
                    .unwrap()
                    .remove("tool_call_ref")
                    .map(|_| ())
                    .unwrap(),
                "incomplete" => runs[0].receipt["complete"] = json!(false),
                "truncated" => runs[0].output_verified = false,
                "digest" => runs[0].receipt["revision_digest"] = Value::Null,
                "nonzero" => runs[0].receipt["exit_code"] = json!(1),
                "not_run" | "unverified" => {
                    c["outcome"] = json!(case);
                    c["blocker"] = json!("policy");
                }
                _ => (),
            }
            let out = evaluate_pair(&[c], &runs).unwrap();
            assert_eq!(out[0]["outcome"], outcome, "{case}");
            assert_eq!(out[0]["evidence_strength"], strength, "{case}");
            assert_eq!(out[0]["evidence_gap"], json!(gap), "{case}");
        }
    }
    #[test]
    fn probe_withdrawn_retains_verified_failed_history() {
        for (case, eligible) in [
            ("verified", true),
            ("unknown", false),
            ("incomplete", false),
            ("zero_exit", false),
            ("empty", false),
        ] {
            let mut runs = vec![run(
                "one",
                1,
                1,
                json!(["curl", "--fail", "https://invalid.test"]),
                "404",
            )];
            let mut c = check("failed", "one");
            c["probe_withdrawn"] = json!("Expected exploratory 404");
            match case {
                "unknown" => runs.clear(),
                "incomplete" => runs[0].output_verified = false,
                "zero_exit" => runs[0].receipt["exit_code"] = json!(0),
                "empty" => c["probe_withdrawn"] = json!(" "),
                _ => (),
            }
            let out = evaluate_pair(&[c], &runs).unwrap();
            assert_eq!(out[0]["outcome"], "failed");
            assert_eq!(
                out[0]["retracted_by"] == "probe_withdrawn",
                eligible,
                "{case}"
            );
        }
    }
    const BAD: &str = "import re; re.search('(', 'subject')";
    const GOOD: &str = "import re; re.search('subject', 'subject')";
    const ERROR: &str =
        "Traceback:\n  File \"<string>\", line 1\nre.error: missing ), unterminated subpattern\n";
    #[test]
    fn supported_correction_shapes_table() {
        for (bad, good, output) in [
            (BAD, GOOD, ERROR),
            (
                "assert (",
                "assert True",
                "SyntaxError: '(' was never closed\n",
            ),
            (
                "items=['a', 'b']; assert len(items)==3",
                "items=['a', 'b']; assert len(items)==2",
                "File \"<string>\"\nAssertionError\n",
            ),
            (
                "items=[2, 1]; assert sorted(items)==[2, 1]; assert sum(items)==3",
                "items=[2, 1]; assert sorted(items)==[1, 2]; assert sum(items)==3",
                "File \"<string>\"\nAssertionError\n",
            ),
        ] {
            for (field, derived, current) in [
                ("invocation_error", "retracted_by", "retracted"),
                ("command_correction", "superseded_by", "superseded"),
            ] {
                let runs = vec![
                    run("bad", 1, 1, python(bad), output),
                    run("good", 2, 0, python(good), ""),
                ];
                let mut checks = vec![check("failed", "bad"), check("passed", "good")];
                checks[0][field] = json!({"reason":"my invocation mistake","corrected_by":"good"});
                let out = evaluate_pair(&checks, &runs).unwrap();
                assert_eq!(out[0]["outcome"], "failed", "{bad}");
                assert_eq!(out[0][derived], "good");
                assert_eq!(out[0]["current_outcome"], current);
                assert_eq!(out[0]["evidence_strength"], "verified");
                assert!(out[0]["closed_by"].is_null());
                // Derived links never authorize a later lost or changed observation.
                let mut lost = runs.clone();
                lost[1].output_verified = false;
                assert!(evaluate_pair(&out, &lost).is_err());
            }
        }
    }
    #[test]
    fn unsafe_corrections_table() {
        for case in [
            "foreign",
            "claimed",
            "earlier",
            "duplicate",
            "missing",
            "different_name",
            "incomplete",
            "truncated",
            "lost_bad_body",
            "lost_good_body",
            "later_failure",
            "later_unresolved",
            "different_profile",
            "different_operation",
            "runtime_verification",
            "empty_reason",
            "real_failure",
            "changed_assertion",
            "test_runner",
            "bad_expectation",
            "unrecorded",
        ] {
            let mut runs = vec![
                run("bad", 1, 1, python(BAD), ERROR),
                run("good", 2, 0, python(GOOD), ""),
            ];
            let mut checks = vec![check("failed", "bad"), check("passed", "good")];
            checks[0]["invocation_error"] = json!({"corrected_by":"good","reason":"mistake"});
            match case {
                "foreign" => runs.remove(1).output_verified = false,
                "claimed" => checks[1]["tool_call_ref"] = json!("unknown"),
                "earlier" => runs[1].order = 0,
                "duplicate" => checks.push(checks[1].clone()),
                "missing" => {
                    checks.pop();
                }
                "different_name" => checks[1]["name"] = json!("other"),
                "incomplete" => runs[1].receipt["complete"] = json!(false),
                "truncated" | "lost_good_body" => runs[1].output_verified = false,
                "lost_bad_body" => runs[0].output_verified = false,
                "different_profile" => runs[1].receipt["profile"] = json!("other"),
                "different_operation" => runs[1].operation = "outside_command".into(),
                "runtime_verification" => {
                    runs[0].receipt["runtime_verification"] = json!({"command_ref":"v"})
                }
                "empty_reason" => checks[0]["invocation_error"]["reason"] = json!(" "),
                "real_failure" => runs[0].receipt["argv"] = json!(["cargo", "test"]),
                "changed_assertion" => {
                    runs[0].receipt["argv"] = python("import re; re.search('x','x'); assert False");
                    runs[1].receipt["argv"] = python("import re; re.search('x','x'); assert True");
                }
                "test_runner" => {
                    runs[0].receipt["argv"] =
                        python("import unittest; import re; re.search('(','x')");
                    runs[1].receipt["argv"] =
                        python("import unittest; import re; re.search('x','x')");
                }
                "bad_expectation" => {
                    runs[0].receipt["argv"] = python("items=[1,2]; assert len(items)==3");
                    runs[1].receipt["argv"] = python("items=[1,2,3]; assert len(items)==3");
                    runs[0].receipt["output"] = json!("File \"<string>\"\nAssertionError\n");
                }
                "later_failure" | "later_unresolved" | "unrecorded" => {
                    let mut late = runs[0].clone();
                    late.order = 3;
                    late.receipt["tool_call_ref"] = json!("late");
                    if case == "later_unresolved" {
                        late.receipt["complete"] = json!(false);
                    }
                    if case == "unrecorded" {
                        late.receipt["argv"] = Value::Null;
                    }
                    runs.push(late);
                }
                _ => (),
            }
            assert_eq!(
                evaluate_pair(&checks, &runs).unwrap_err().code(),
                "invalid_check_retraction",
                "{case}"
            );
        }
    }
    #[test]
    fn argparse_supersession_is_only_a_startup_error() {
        for (output,code,allowed) in [
            ("usage: check.py [-h] action\ncheck.py: error: unrecognized arguments: --check\n",2,true),
            ("usage: check.py [-h] action\ncheck.py: error: the following arguments are required: action\n",2,true),
            ("project test failed\nusage: check.py\ncheck.py: error: unrecognized arguments: --check\n",2,false),
            ("usage: check.py\ncheck.py: error: unrecognized arguments: --check\n",1,false),
        ] {
            let runs=vec![run("bad",1,code,json!(["python3","-B","check.py","--check"]),output),run("good",2,0,json!(["python3","-B","check.py","check"]),"")];
            let mut checks=vec![check("failed","bad"),check("passed","good")];
            checks[0]["command_correction"]=json!({"corrected_by":"good","reason":"argument typo"});
            let out=evaluate_pair(&checks,&runs);
            assert_eq!(out.is_ok(),allowed,"{output}");
        }
    }
}
