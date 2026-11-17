//! Run the rendered journey several times in a row and report each run in one line.
//!
//! Runs are strictly one at a time. Every run's complete output stays in `--logs`; only step names, process evidence and
//! counts are printed, never message bodies. A run is complete only with exit 0, a valid binary SHA-256, at least one passing
//! screen audit and exactly one consistent native-evidence/visual-summary/cleanup sequence.

use super::pretty_line;
use crate::installed::util::*;
use crate::installed::Cli;
use crate::json::{parse, Res, Value};
use std::process::{Command, Stdio};
use std::time::Instant;

fn name(step: &Value) -> &str {
    step.get("step").and_then(Value::as_str).unwrap_or("")
}

fn status(step: &Value) -> Option<&str> {
    step.get("status").and_then(Value::as_str)
}

fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Accept only one complete, consistent run; summaries cannot erase failures.
pub fn summarize(text: &str, exit_code: i32) -> Value {
    let mut steps: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for line in text.lines() {
        match parse(line) {
            Ok(step) => {
                if matches!(step, Value::Obj(_)) && step.get("step").is_some() {
                    steps.push(step);
                }
            }
            Err(_) => {
                if line.trim_start().starts_with(['{', '[']) {
                    errors.push("malformed_json_evidence".into());
                }
            }
        }
    }
    let empty = Value::obj();
    let last = |wanted: &str| steps.iter().rev().find(|step| name(step) == wanted).unwrap_or(&empty).clone();
    let summary = last("visual_audit_summary");
    let incomplete = last("journey_incomplete");
    // Exception evidence is never a successful terminal record, even if a later run_evidence or passing summary claims otherwise.
    let native = last("native_process_evidence");
    let evidence = if native.truthy() { native.clone() } else { last("run_evidence") };
    let passed: Vec<&Value> = steps.iter().filter(|step| status(step) == Some("passed")).collect();
    let failures: Vec<Value> = steps.iter().filter(|step| status(step) == Some("failed")).map(|step| s(name(step))).collect();
    let required = ["bundled_native_window", "run_evidence", "visual_audit_summary", "owned_process_cleanup"];
    for wanted in required {
        if steps.iter().filter(|step| name(step) == wanted).count() != 1 {
            errors.push(format!("missing_or_duplicate:{wanted}"));
        }
    }
    if steps.iter().any(|step| !matches!(status(step), Some("passed") | Some("diagnostic") | Some("not_applicable"))) {
        errors.push("unsuccessful_step".into());
    }
    if incomplete.truthy() || native.truthy() {
        errors.push("journey_incomplete".into());
    }
    let tail: Vec<&str> = steps.iter().rev().take(3).rev().map(|step| name(step)).collect();
    if tail != required[1..] {
        errors.push("invalid_terminal_order".into());
    }
    let binary = last("bundled_native_window");
    let digest = binary.get("binary_sha256").and_then(Value::as_str);
    if status(&binary) != Some("passed") || !digest.is_some_and(is_digest) {
        errors.push("invalid_binary_identity".into());
    }
    if !steps.iter().any(|step| name(step) == "visual_audit" && status(step) == Some("passed")) {
        errors.push("missing_visual_audit".into());
    }
    if status(&summary) != Some("passed") || summary.get("failures").and_then(Value::as_i64) != Some(0) {
        errors.push("invalid_visual_summary".into());
    }
    if status(&last("owned_process_cleanup")) != Some("passed") {
        errors.push("cleanup_not_passed".into());
    }
    // A stage the host cannot offer is named, with the page's own words, as `not_applicable`; it is listed here and never counted as a pass.
    let not_applicable: Vec<Value> = steps.iter().filter(|step| status(step) == Some("not_applicable")).map(|step| s(name(step))).collect();
    // The common journey closes the app before final evidence, restarts the service, and leaves the outer Wayland compositor
    // running (absent on X11).
    let expected: [(&str, Value); 8] = [
        ("status", s("diagnostic")),
        ("failure_class", Value::Null),
        ("app", Value::Null),
        ("app_processes", arr(vec![])),
        ("page_answers", Value::Null),
        ("web_process_terminations", arr(vec![])),
        ("cores", arr(vec![])),
        ("service", obj(vec![("state", s("running"))])),
    ];
    if expected.iter().any(|(key, value)| evidence.get(key).is_none_or(|got| canon(got) != canon(value))) {
        errors.push("invalid_native_evidence".into());
    }
    let compositor_ok = match evidence.get("compositor") {
        None => false,
        Some(Value::Null) => true,
        Some(other) => canon(other) == canon(&obj(vec![("state", s("running"))])),
    };
    if !compositor_ok {
        errors.push("invalid_compositor_evidence".into());
    }
    if evidence.get("app_launches").and_then(Value::as_i64).is_none_or(|n| n < 1) {
        errors.push("missing_app_launches".into());
    }
    let complete = exit_code == 0 && errors.is_empty();
    let ev = |key: &str| evidence.get(key).cloned().unwrap_or(Value::Null);
    let stale = steps.iter().filter(|step| name(step) == "accessibility_node_stale").count() as i64;
    let screens = steps.iter().filter(|step| name(step) == "visual_audit" && status(step) == Some("passed")).count() as i64;
    obj(vec![
        ("complete", b(complete)),
        ("exit_code", n(exit_code as i64)),
        ("steps_passed", n(passed.len() as i64)),
        ("last_step", passed.last().map(|step| s(name(step))).unwrap_or(Value::Null)),
        ("screens_audited", n(screens)),
        ("failures", arr(failures)),
        ("evidence_errors", arr(errors.iter().map(|e| s(e.as_str())).collect())),
        ("reason", incomplete.get("reason").cloned().unwrap_or(Value::Null)),
        ("at", incomplete.get("at").cloned().unwrap_or(Value::Null)),
        ("failure_class", ev("failure_class")),
        ("app", ev("app")),
        ("page_answers", ev("page_answers")),
        ("app_processes", ev("app_processes")),
        ("web_process_terminations", ev("web_process_terminations")),
        ("compositor", ev("compositor")),
        ("service", ev("service")),
        ("app_stderr", ev("app_stderr")),
        ("compositor_stderr", ev("compositor_stderr")),
        ("app_launches", ev("app_launches")),
        ("largest_app_stderr_bytes", ev("largest_app_stderr_bytes")),
        ("cores", ev("cores")),
        ("core_limit", ev("core_limit")),
        ("stale_nodes_tolerated", n(stale)),
        ("binary_sha256", last("bundled_native_window").get("binary_sha256").cloned().unwrap_or(Value::Null)),
        ("not_applicable", arr(not_applicable)),
        ("requested_closes", ev("requested_closes")),
    ])
}

pub fn run(cli: &Cli) -> Res<i32> {
    let artifacts = cli.path("artifacts").ok_or("--artifacts is required")?;
    let logs = cli.path("logs").ok_or("--logs is required")?;
    let runs: u32 = cli.flag("runs").unwrap_or("5").parse().map_err(|_| "--runs must be a number")?;
    let display = cli.flag("display").unwrap_or("wayland");
    require(matches!(display, "wayland" | "x11"), "--display is wayland or x11")?;
    std::fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut bad = 0;
    for number in 1..=runs {
        let started = Instant::now();
        let log = logs.join(format!("{display}-{number}.jsonl"));
        let sink = std::fs::File::create(&log).map_err(|e| e.to_string())?;
        let err = sink.try_clone().map_err(|e| e.to_string())?;
        let mut command = Command::new(&me);
        command.arg("check-rendered").arg("--artifacts").arg(&artifacts);
        if let Some(root) = cli.path("root") {
            command.arg("--root").arg(root);
        }
        if display == "wayland" {
            command.args(["--journey-wayland", "yes"]);
        }
        let code = command.stdin(Stdio::null()).stdout(sink).stderr(err).status().map_err(|e| e.to_string())?.code().unwrap_or(-1);
        let mut result = summarize(&String::from_utf8_lossy(&std::fs::read(&log).map_err(|e| e.to_string())?), code);
        if let Some(expected) = cli.flag("expect-sha256") {
            if result.get("binary_sha256").and_then(Value::as_str) != Some(expected) {
                result.set("complete", b(false));
            }
        }
        if !matches!(result.get("complete"), Some(Value::Bool(true))) {
            bad += 1;
        }
        let mut row = obj(vec![("run", n(number as i64)), ("display", s(display)), ("seconds", n(started.elapsed().as_secs_f64().round() as i64))]);
        for (k, v) in result.as_obj().unwrap_or(&[]) {
            row.set(k, v.clone());
        }
        println!("{}", pretty_line(&row));
    }
    println!("{}", pretty_line(&obj(vec![("series", s(display)), ("runs", n(runs as i64)), ("complete", n((runs - bad) as i64)), ("incomplete", n(bad as i64))])));
    Ok(if bad > 0 { 1 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(steps: &[&str]) -> String {
        format!("not json\n{}\n", steps.join("\n"))
    }

    const WINDOW: &str = r#"{"step": "bundled_native_window", "status": "passed", "binary_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
    const AUDIT: &str = r#"{"step": "visual_audit", "status": "passed"}"#;
    const EVIDENCE: &str = r#"{"step": "run_evidence", "status": "diagnostic", "failure_class": null, "app": null, "app_processes": [], "page_answers": null, "web_process_terminations": [], "cores": [], "service": {"state": "running"}, "compositor": null, "app_launches": 2}"#;
    const SUMMARY: &str = r#"{"step": "visual_audit_summary", "status": "passed", "failures": 0}"#;
    const CLEANUP: &str = r#"{"step": "owned_process_cleanup", "status": "passed"}"#;

    fn complete(result: &Value) -> bool {
        matches!(result.get("complete"), Some(Value::Bool(true)))
    }

    fn errors(result: &Value) -> Vec<String> {
        result.get("evidence_errors").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(|e| e.as_str().map(String::from)).collect()
    }

    #[test]
    fn a_series_run_is_complete_only_with_summary_cleanup_and_exit_zero() {
        let text = lines(&[WINDOW, AUDIT, EVIDENCE, SUMMARY, CLEANUP]);
        assert!(complete(&summarize(&text, 0)));
        assert!(!complete(&summarize(&text, 1)));
    }

    #[test]
    fn a_not_applicable_stage_is_listed_and_never_hides_a_failure() {
        let na = r#"{"step": "task_worktree_choice", "status": "not_applicable", "reason": "the native host reports task worktrees unavailable"}"#;
        let result = summarize(&lines(&[WINDOW, AUDIT, na, EVIDENCE, SUMMARY, CLEANUP]), 0);
        assert!(complete(&result));
        assert_eq!(result.get("not_applicable").and_then(Value::as_arr).map(|a| a.len()), Some(1));
        let failed = r#"{"step": "native_terminal_round_trip", "status": "failed"}"#;
        assert!(!complete(&summarize(&lines(&[WINDOW, AUDIT, na, failed, EVIDENCE, SUMMARY, CLEANUP]), 0)));
    }

    #[test]
    fn each_piece_of_evidence_is_required_and_a_failed_step_cannot_be_hidden() {
        for omit in 0..5 {
            let all = [WINDOW, AUDIT, EVIDENCE, SUMMARY, CLEANUP];
            let kept: Vec<&str> = all.iter().enumerate().filter(|(i, _)| *i != omit).map(|(_, s)| *s).collect();
            assert!(!complete(&summarize(&lines(&kept), 0)), "missing piece {omit} was accepted");
        }
        let failed = lines(&[WINDOW, AUDIT, r#"{"step": "popovers", "status": "failed"}"#, EVIDENCE, SUMMARY, CLEANUP]);
        let result = summarize(&failed, 0);
        assert!(!complete(&result) && errors(&result).contains(&"unsuccessful_step".to_string()));
    }

    #[test]
    fn duplicate_terminal_records_and_a_bad_binary_identity_are_rejected() {
        assert!(!complete(&summarize(&lines(&[WINDOW, AUDIT, EVIDENCE, EVIDENCE, SUMMARY, CLEANUP]), 0)));
        let bad = WINDOW.replace("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "xyz");
        assert!(errors(&summarize(&lines(&[&bad, AUDIT, EVIDENCE, SUMMARY, CLEANUP]), 0)).contains(&"invalid_binary_identity".to_string()));
    }

    #[test]
    fn exception_evidence_cannot_be_hidden_by_run_evidence() {
        let exception = r#"{"step": "native_process_evidence", "status": "diagnostic", "failure_class": "app_exited"}"#;
        let result = summarize(&lines(&[WINDOW, AUDIT, exception, EVIDENCE, SUMMARY, CLEANUP]), 0);
        assert!(!complete(&result));
    }

    #[test]
    fn truncated_json_evidence_blocks_completion() {
        let text = lines(&[WINDOW, AUDIT, EVIDENCE, r#"{"step": "visual_audit_summary", "status": "pass"#, SUMMARY, CLEANUP]);
        assert!(errors(&summarize(&text, 0)).contains(&"malformed_json_evidence".to_string()));
    }
}
