//! Byte parity of every store reader with the recorded behavior of the
//! retired Python readers.
//!
//! Fixtures are real format-12 stores created by cortex-core's initializing
//! open; the readers then open them read-only. Each case compares the stdout
//! bytes and exit status of `cortex-paseo-tools` with the golden outcome in
//! `tests/parity-golden.json`, which was recorded once from the Python
//! readers that used to be embedded in the plugin's TS sources. The test
//! needs no Python. Re-recording is only possible from those retired sources
//! (`CORTEX_PARITY_RECORD=<golden path> CORTEX_PARITY_ORACLE_SOURCES=<dir of
//! the old integrations/paseo/server>` and `python3`).
use cortex_core::store::{OpenMode, ProjectKey, Store, StoreChannel};
use rusqlite::types::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/parity-golden.json");

/// Temp-dir specifics a golden output must not depend on.
fn normalize(text: &str) -> String {
    let base = std::env::temp_dir().canonicalize().unwrap();
    text.replace(base.to_str().unwrap(), "TMPBASE")
        .replace(&format!("-{}", std::process::id()), "-PID")
}
fn denormalize(text: &str) -> String {
    let base = std::env::temp_dir().canonicalize().unwrap();
    text.replace("TMPBASE", base.to_str().unwrap())
        .replace("-PID", &format!("-{}", std::process::id()))
}

const TOOL: &str = env!("CARGO_BIN_EXE_cortex-paseo-tools");
const TASK: &str = "t_parity00001";
const SESSION: &str = "acps_0123456789ab";
const RUNNING_REF: &str = "gateway:0123456789abcdef0123456789abcdef";
const EXITED_REF: &str = "gateway:fedcba9876543210fedcba9876543210";

/// The body of `<marker>String.raw\`` ... "`;" in one TS source.
fn raw_template(source: &str, marker: &str) -> String {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing {marker}"))
        + marker.len();
    let end = source[start..].find("`;").expect("unterminated template") + start;
    source[start..end].to_owned()
}

struct Scripts {
    worker_history: String,
    project_root: String,
    task_id: String,
    worker_results: String,
    running_commands: String,
    pipeline: String,
}

/// The retired Python readers' sources: only when re-recording the goldens.
fn scripts() -> Scripts {
    let Some(sources) = std::env::var_os("CORTEX_PARITY_ORACLE_SOURCES") else {
        let empty = || String::new();
        return Scripts {
            worker_history: empty(),
            project_root: empty(),
            task_id: empty(),
            worker_results: empty(),
            running_commands: empty(),
            pipeline: empty(),
        };
    };
    let read = |name: &str| fs::read_to_string(Path::new(&sources).join(name)).unwrap();
    let history = read("worker-history.ts");
    let helpers = raw_template(&history, "export const STORE_PYTHON_HELPERS = String.raw`");
    let expand = |template: String| {
        let text = template.replace("${STORE_PYTHON_HELPERS}", &helpers);
        assert!(!text.contains("${"), "unexpected template interpolation");
        text
    };
    let task_start = history
        .find("async function readTaskId")
        .expect("readTaskId");
    let literal = &history[task_start..];
    let opening = "runStoreReader(\"task-id\", \"";
    let begin = literal.find(opening).expect("task-id script") + opening.len();
    let end = literal[begin..]
        .find("\", [projectRoot, sessionId]")
        .expect("task-id end")
        + begin;
    Scripts {
        worker_history: expand(raw_template(&history, "\nconst PYTHON = String.raw`")),
        project_root: expand(raw_template(
            &history,
            "export const PROJECT_ROOT_PYTHON = String.raw`",
        )),
        task_id: literal[begin..end].to_owned(),
        worker_results: expand(raw_template(
            &read("worker-results.ts"),
            "const PYTHON = String.raw`",
        )),
        running_commands: expand(raw_template(
            &read("command-stop.ts"),
            "const PYTHON = String.raw`",
        )),
        pipeline: expand(raw_template(
            &read("pipeline.ts"),
            "const PYTHON = String.raw`",
        )),
    }
}

fn python(script: &str, args: &[&str]) -> (Vec<u8>, Option<i32>) {
    let output = Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg("-c")
        .arg(script)
        .args(args)
        .current_dir("/")
        .output()
        .expect("python3 is required to record the parity goldens");
    (output.stdout, output.status.code())
}

fn rust(command: &str, args: &[&str]) -> (Vec<u8>, Option<i32>) {
    let output = Command::new(TOOL)
        .arg(command)
        .args(args)
        .current_dir("/")
        .env_clear()
        .output()
        .expect("cortex-paseo-tools");
    (output.stdout, output.status.code())
}

struct Cases {
    count: usize,
    failures: Vec<String>,
    last: String,
    /// Recorded Python outcomes by case key (replay mode).
    golden: BTreeMap<String, (String, Option<i64>)>,
    /// `Some` while re-recording from the Python sources.
    recording: Option<BTreeMap<String, (String, Option<i64>)>>,
    seen: BTreeMap<String, usize>,
}

fn load_golden() -> BTreeMap<String, (String, Option<i64>)> {
    let value: serde_json::Value = serde_json::from_slice(&fs::read(GOLDEN).expect("tests/parity-golden.json")).unwrap();
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, outcome)| {
            (
                key.clone(),
                (outcome["stdout"].as_str().unwrap().to_owned(), outcome["code"].as_i64()),
            )
        })
        .collect()
}

impl Cases {
    fn compare(
        &mut self,
        label: &str,
        script: &str,
        command: &str,
        args: &[&str],
        expect_ok: bool,
    ) {
        self.count += 1;
        let occurrence = self.seen.entry(label.to_owned()).or_default();
        *occurrence += 1;
        let key = format!("{label}#{occurrence}");
        let expected = if let Some(recording) = self.recording.as_mut() {
            let outcome = python(script, args);
            let text = String::from_utf8(outcome.0.clone()).expect("recorded output must be UTF-8");
            recording.insert(key.clone(), (normalize(&text), outcome.1.map(i64::from)));
            outcome
        } else if let Some((stdout, code)) = self.golden.get(&key) {
            (denormalize(stdout).into_bytes(), code.map(|c| c as i32))
        } else {
            self.failures.push(format!("{label}: no recorded Python outcome for {key}"));
            return;
        };
        let actual = rust(command, args);
        self.last = String::from_utf8_lossy(&actual.0).into_owned();
        if expect_ok && expected.1 != Some(0) {
            self.failures.push(format!(
                "{label}: the Python oracle itself failed ({:?})",
                expected.1
            ));
            return;
        }
        if std::env::var_os("CORTEX_PARITY_SHOW").is_some() {
            println!(
                "== {label} ({:?})\n{}",
                actual.1,
                String::from_utf8_lossy(&actual.0)
            );
        }
        if expected != actual {
            self.failures.push(format!(
                "{label}: mismatch\n  recorded python ({:?}): {}\n  rust            ({:?}): {}",
                expected.1,
                String::from_utf8_lossy(&expected.0),
                actual.1,
                String::from_utf8_lossy(&actual.0)
            ));
        }
    }

    /// Guard against a vacuous match: the last output must carry real data.
    fn expect(&mut self, needle: &str) {
        if !self.last.contains(needle) {
            self.failures
                .push(format!("output lacks {needle:?}: {}", self.last));
        }
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let base = std::env::temp_dir().canonicalize().unwrap();
    let path = base.join(format!("cortex-paseo-parity-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn insert(db: &rusqlite::Connection, table: &str, row: &[(&str, Value)]) {
    let columns: Vec<&str> = row.iter().map(|(c, _)| *c).collect();
    let marks = vec!["?"; row.len()].join(",");
    let values: Vec<&Value> = row.iter().map(|(_, v)| v).collect();
    db.execute(
        &format!(
            "insert into {table} ({}) values ({marks})",
            columns.join(",")
        ),
        rusqlite::params_from_iter(values),
    )
    .unwrap_or_else(|e| panic!("insert into {table}: {e}"));
}

fn t(value: &str) -> Value {
    Value::Text(value.to_owned())
}

fn i(value: i64) -> Value {
    Value::Integer(value)
}

const NOW: &str = "2026-11-15T00:00:00+00:00";

fn event(
    db: &rusqlite::Connection,
    kind: &str,
    reference: Option<&str>,
    payload: &str,
    created_at: &str,
) {
    let key = format!(
        "k{}",
        db.query_row("select count(*) from task_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap()
    );
    insert(
        db,
        "task_events",
        &[
            ("task_id", t(TASK)),
            ("event_type", t(kind)),
            ("assignment_ref", reference.map(t).unwrap_or(Value::Null)),
            ("dedup_key", t(&key)),
            ("payload_json", t(payload)),
            ("origin", t("test")),
            ("created_at", t(created_at)),
        ],
    );
}

fn digest_file(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// A populated format-12 store exercising every reader branch the fixtures can reach.
fn populate(db: &rusqlite::Connection, root: &str) {
    insert(
        db,
        "tasks",
        &[
            ("id", t(TASK)),
            ("project_root", t(root)),
            ("created_at", t(NOW)),
        ],
    );
    insert(
        db,
        "acp_host_sessions",
        &[
            ("session_id", t(SESSION)),
            ("project_root", t(root)),
            ("task_id", t(TASK)),
            ("client_info", t("{}")),
            ("client_capabilities", t("{}")),
            ("mcp_servers", t("[]")),
            ("created_at", t(NOW)),
        ],
    );
    for (id, node, key, status) in [
        ("wa_1", "n1", "ref-1", "running"),
        ("wa_2", "n2", "ref-2", "completed"),
        ("wa_3", "n3", "ref-3", "pending"),
        ("wa_4", "n4", "ref-4", "running"),
    ] {
        insert(
            db,
            "work_assignments",
            &[
                ("assignment_id", t(id)),
                ("task_id", t(TASK)),
                ("node_key", t(node)),
                ("requirement_bindings_json", t("[]")),
                ("status", t(status)),
                ("idempotency_key", t(key)),
                ("created_at", t(NOW)),
            ],
        );
    }
    for (id, assignment, number, status, failure) in [
        ("att_1", "wa_1", 1, "running", Value::Null),
        ("att_2", "wa_2", 1, "completed", Value::Null),
        (
            "att_3",
            "wa_2",
            2,
            "running",
            t("Lost at /home/someone/project, token=abc"),
        ),
        ("att_4", "wa_4", 1, "running", Value::Null),
    ] {
        insert(
            db,
            "work_assignment_attempts",
            &[
                ("attempt_id", t(id)),
                ("task_id", t(TASK)),
                ("assignment_id", t(assignment)),
                ("attempt_no", i(number)),
                ("requirement_bindings_json", t("[]")),
                ("status", t(status)),
                ("failure_reason", failure),
                ("idempotency_key", t(&format!("idem-{id}"))),
                ("created_at", t(NOW)),
            ],
        );
    }
    // Delegation requests: the assignment text, title and role of each attempt.
    event(
        db,
        "user_message",
        None,
        r#"{"content":"Сделай всё по плану, пожалуйста"}"#,
        "2026-11-15T00:00:01Z",
    );
    event(
        db,
        "user_message",
        None,
        r#"{"content":"  second message with token: hunter2  "}"#,
        "2026-11-15T00:00:02Z",
    );
    event(
        db,
        "delegation_requested",
        Some("ref-1"),
        r#"{"title":"Implement A","role":"worker-fullstack-dev","brief":"Do A carefully","acceptance":"Tests pass","plan_assignment_ref":"wa_1","work_attempt_ref":"att_1"}"#,
        "2026-11-15T00:00:03Z",
    );
    event(
        db,
        "delegation_requested",
        Some("ref-2"),
        r#"{"title":"Review B","role":"worker-reviewer","brief":"{\"objective\":\"Check /home/u/b\",\"n\":[1,2.5,null,true]}","acceptance":"Reported","work_attempt_ref":"att_2"}"#,
        "2026-11-15T00:00:04Z",
    );
    event(
        db,
        "delegation_requested",
        Some("ref-2b"),
        r#"{"title":"Review B again","brief":"Retry B","acceptance":"Done","plan_assignment_ref":"wa_2","work_attempt_ref":"att_3"}"#,
        "2026-11-15T00:00:05Z",
    );
    // (task_events has a json_extract expression index: every payload is valid JSON.)
    event(
        db,
        "delegation_requested",
        Some("ref-4"),
        r#"{"note":"no attempt","title":"   "}"#,
        "2026-11-15T00:00:06Z",
    );
    event(
        db,
        "delegation_requested",
        Some("ref-4"),
        r#"{"title":"Stuck D","role":"worker-tester","brief":"D","acceptance":"E","work_attempt_ref":"att_4"}"#,
        "2026-11-15T00:00:06Z",
    );
    for (key, attempt, channel) in [
        ("ref-1", "xa_exec1", "ch_1"),
        ("ref-2", "xa_exec2", "ch_2"),
        ("ref-2b", "xa_exec3", "ch_3"),
        ("ref-4", "xa_exec4", "ch_4"),
    ] {
        insert(
            db,
            "delegation_provisioning",
            &[
                ("idempotency_key", t(key)),
                ("task_id", t(TASK)),
                ("attempt_id", t(attempt)),
                ("role_id", t("worker")),
                ("step", t("ready")),
                ("status", t("ready")),
                ("channel_id", t(channel)),
                ("owner_epoch", i(1)),
                ("created_at", t(NOW)),
                ("updated_at", t(NOW)),
            ],
        );
    }
    // Worker transcript of att_1 (execution attempt xa_exec1).
    let brief_sha = digest("Implement A\n\nDo A carefully\n\nAcceptance: Tests pass");
    event(
        db,
        "progress",
        Some("xa_exec1"),
        &format!(r#"{{"kind":"worker_brief_sent","assignment_sha256":"{brief_sha}"}}"#),
        "2026-11-15T00:01:00Z",
    );
    event(
        db,
        "progress",
        Some("xa_exec1"),
        r#"{"kind":"worker_activity","activity_ref":"a1","title":"Read src/lib.rs","tool_kind":"read","output":"secret contents","status":"completed"}"#,
        "2026-11-15T00:01:01Z",
    );
    event(
        db,
        "progress",
        Some("xa_exec1"),
        r#"{"kind":"worker_activity","activity_ref":"a2","title":"mcp__cortex__project_read","raw_input":{"server":"cortex","tool":"project_read","path":"a"},"content":[{"type":"text","text":"body"}],"byte_count":NaN}"#,
        "2026-11-15T00:01:02Z",
    );
    event(
        db,
        "progress",
        Some("xa_exec1"),
        r#"{"kind":"worker_activity","activity_ref":"a3","title":"cat  a |  grep b","raw_input":{"command":"cat a | grep b"},"output":"ok /tmp/x","status":"completed","diff":{"path":"x","old":"a","new":"b"}}"#,
        "2026-11-15T00:01:03Z",
    );
    event(
        db,
        "progress",
        Some("xa_exec1"),
        r#"{"kind":"worker_activity","activity_ref":"a4","title":"view notes","tool_kind":"other","error":"password=xyz failed"}"#,
        "2026-11-15T00:01:04Z",
    );
    event(
        db,
        "progress",
        Some("xa_exec1"),
        r#"{"kind":"worker_activity","activity_ref":"a5","message":0,"summary":"","detail":[],"error":{"code":1}}"#,
        "2026-11-15T00:01:05Z",
    );
    event(
        db,
        "progress",
        Some("xa_exec1"),
        r#"{"kind":"worker_activity","activity_ref":"context","content":{"used":4000,"size":200000}}"#,
        "2026-11-15T00:01:06Z",
    );
    event(
        db,
        "progress",
        Some("xa_exec1"),
        r#"{"kind":"note","text":"coordinator marker"}"#,
        "2026-11-15T00:01:07Z",
    );
    event(
        db,
        "worker_question",
        Some("xa_exec1"),
        r#"{"question":"Which?","title":"Question","status":"open"}"#,
        "2026-11-15T00:01:08Z",
    );
    // att_2: a legacy worker_result with checks; its brief marker does not match.
    event(
        db,
        "progress",
        Some("xa_exec2"),
        r#"{"kind":"worker_brief_sent","assignment_sha256":"0000"}"#,
        "2026-11-15T00:02:00Z",
    );
    event(
        db,
        "worker_result",
        Some("xa_exec2"),
        r#"{"summary":"  Reviewed; api_key=abc123 is fine  ","checks":[{"name":"unit /home/me/t","outcome":"passed"},{"name":"lint","outcome":"failed","blocker":"token: zzz","closed_by":"gateway:1"},{"name":"e2e","outcome":"not_run","blocker":"no browser","claimed_outcome":"passed","evidence_gap":"missing"}]}"#,
        "2026-11-15T00:02:01Z",
    );
    event(
        db,
        "coordinator_decision",
        Some("ref-2"),
        r#"{"decision":"accept","rationale":"Looks right at /home/u/r"}"#,
        "2026-11-15T00:02:02Z",
    );
    event(
        db,
        "coordinator_decision",
        Some("ref-9"),
        r#"{"decision":"reject"}"#,
        "2026-11-15T00:02:03Z",
    );
    // att_3: written off after the host was lost.
    event(
        db,
        "assignment_failed",
        Some("xa_exec3"),
        r#"{"error":"recovery_worker_lost","attempt_id":"xa_exec3"}"#,
        "2026-11-15T00:03:00Z",
    );
    event(
        db,
        "progress",
        None,
        r#"{"kind":"host_disconnected","interrupted_workers":[{"attempt_id":"xa_exec4"},"x",{"attempt_id":""}]}"#,
        "2026-11-15T00:03:01Z",
    );
    // Route audit with a predecessor chain and a structured route decision.
    for (id, attempt, predecessor, detail) in [
        ("ra_prev", "xa_prev", Value::Null, r#"{"effort":"high"}"#.to_owned()),
        (
            "ra_1",
            "xa_exec1",
            t("xa_prev"),
            r#"{"effort":"medium","model_name":"Opus 5","route_decision":{"tier":"heavy","role_tier":"standard","model_hint_status":"auto","decided_by":"user_request","chosen":"claude/opus","chosen_connection_id":"cn_1","candidates":["skip",{"route":"claude/opus","connection_id":"cn_1","used_percent":42}]}}"#.to_owned(),
        ),
        (
            "ra_2",
            "xa_exec2",
            Value::Null,
            r#"{"route_decision":{"tier":"light","chosen_cross_tier":true,"cross_tier":{"from_tier":"standard"},"decided_by":"policy","candidates":{"a":1}}}"#.to_owned(),
        ),
    ] {
        insert(
            db,
            "attempt_route_audit",
            &[
                ("id", t(id)),
                ("attempt_id", t(attempt)),
                ("task_id", t(TASK)),
                ("role", t("worker")),
                ("engine", t("claude-agent-acp")),
                ("connection_id", t("cn_1")),
                ("model", t("opus")),
                ("params_fingerprint", t("p")),
                ("decision", t("authorized")),
                ("decision_detail_json", t(&detail)),
                ("predecessor_attempt_id", predecessor),
                ("recorded_at", t(NOW)),
            ],
        );
    }
    for (id, attempt, basis, tokens, stop, failed) in [
        (
            "u1",
            "xa_exec1",
            "engine_reported",
            r#"{"inputTokens":100,"outputTokens":20,"cachedReadTokens":5,"totalTokens":130}"#,
            Value::Null,
            0,
        ),
        (
            "u2",
            "xa_prev",
            "engine_reported",
            r#"{"inputTokens":123456789012345678901234567890,"outputTokens":true}"#,
            Value::Null,
            0,
        ),
        (
            "u3",
            "xa_exec1",
            "unknown",
            "{}",
            t("released_after_result"),
            0,
        ),
        ("u4", "xa_exec1", "unknown", "{}", Value::Null, 1),
        (
            "u5",
            "co_coordinator",
            "observed",
            r#"{"inputTokens":7,"outputTokens":3}"#,
            Value::Null,
            0,
        ),
        ("u6", "xa_exec2", "observed", "not json", Value::Null, 0),
    ] {
        insert(
            db,
            "usage_records",
            &[
                ("id", t(id)),
                ("task_id", t(TASK)),
                ("attempt_id", t(attempt)),
                ("engine_session_ref", t("es")),
                ("call_sequence", i(1)),
                ("basis", t(basis)),
                ("tokens_json", t(tokens)),
                ("stop_reason", stop),
                ("failed", i(failed)),
                ("created_at", t(NOW)),
            ],
        );
    }
    // Process registry: xa_exec4's worker is terminated; its host is on another boot.
    for (attempt, role, boot, terminated) in [
        ("xa_exec4", "worker", "other-boot", t(NOW)),
        ("acp_host:xa_exec4", "acp_host", "other-boot", Value::Null),
    ] {
        insert(
            db,
            "attempt_process_registry",
            &[
                ("attempt_id", t(attempt)),
                ("task_id", t(TASK)),
                ("role", t(role)),
                ("pid", i(999_999)),
                ("pgid", i(999_999)),
                ("start_ticks", t("1")),
                ("boot_id", t(boot)),
                ("launch_command_digest", t("d")),
                ("recorded_at", t(NOW)),
                ("terminated_at", terminated),
            ],
        );
    }
    // Runtime plan: node results are authoritative over legacy events.
    insert(
        db,
        "runtime_tasks",
        &[
            ("task_id", t(TASK)),
            ("state", t("EXECUTING")),
            ("revision", i(1)),
            ("goal", t("goal")),
            ("blocker", t("Waiting on /home/u/x, secret=1")),
            ("created_at", t(NOW)),
            ("updated_at", t(NOW)),
        ],
    );
    for (node, role, status, reference, dependencies, result) in [
        (
            "n1",
            "worker-fullstack-dev",
            "running",
            t("ref-1"),
            "[]",
            Value::Null,
        ),
        (
            "n2",
            "worker-reviewer",
            "failed",
            t("ref-2"),
            "[]",
            t(
                r#"{"summary":"Runtime summary","checks":[{"name":"unit","outcome":"passed"},{"name":"lint","outcome":"failed","closed_by":"gateway:2","claimed_outcome":"passed"}]}"#,
            ),
        ),
        (
            "n3",
            "",
            "pending",
            Value::Null,
            r#"["n1","n2","n0",5]"#,
            Value::Null,
        ),
        (
            "n4",
            "worker-tester",
            "pending",
            t("ref-x"),
            "{\"n9\":1}",
            t("[]"),
        ),
    ] {
        insert(
            db,
            "runtime_nodes",
            &[
                ("task_id", t(TASK)),
                ("revision", i(1)),
                ("node_id", t(node)),
                ("role", t(role)),
                (
                    "objective",
                    t(&format!("Objective of {node} at /tmp/{node}")),
                ),
                ("dependencies_json", t(dependencies)),
                ("scope_json", t("[]")),
                ("stage", t("execution")),
                ("status", t(status)),
                ("assignment_ref", reference),
                ("result_json", result),
            ],
        );
    }
    // Running commands of a live worker channel.
    insert(
        db,
        "task_owners",
        &[
            ("task_id", t(TASK)),
            ("owner_ref", t("o")),
            ("epoch", i(1)),
            ("acquired_at", t(NOW)),
        ],
    );
    for (channel, attempt) in [("ch_1", "xa_exec1"), ("ch_2", "xa_exec2")] {
        insert(
            db,
            "trusted_channels",
            &[
                ("channel_id", t(channel)),
                ("secret_hash", t("h")),
                ("task_id", t(TASK)),
                ("principal_thread_id", t("p")),
                ("role", t("worker")),
                ("attempt_id", t(attempt)),
                ("owner_epoch", i(1)),
                ("issued_at", t(NOW)),
            ],
        );
    }
    for (actor, kind, payload) in [
        (
            "ch_1",
            "COMMAND_PROCESS_STARTED",
            format!(
                r#"{{"tool_call_ref":"{RUNNING_REF}","argv":["bash","-c","echo 'hi' /home/u/x",3],"operation":"command_run"}}"#
            ),
        ),
        (
            "ch_2",
            "COMMAND_PROCESS_STARTED",
            format!(r#"{{"tool_call_ref":"{EXITED_REF}","argv":"not a list"}}"#),
        ),
        (
            "ch_2",
            "COMMAND_PROCESS_EXITED",
            format!(r#"{{"tool_call_ref":"{EXITED_REF}"}}"#),
        ),
        (
            "ch_1",
            "COMMAND_PROCESS_STARTED",
            r#"{"tool_call_ref":"gateway:short"}"#.to_owned(),
        ),
        (
            "ch_1",
            "COMMAND_STOP_REQUESTED",
            format!(r#"{{"tool_call_ref":"{RUNNING_REF}","reason":"Too slow","origin":5}}"#),
        ),
    ] {
        insert(
            db,
            "runtime_events",
            &[
                ("task_id", t(TASK)),
                ("type", t(kind)),
                ("actor", t(actor)),
                ("payload_json", t(&payload)),
                ("created_at", t(NOW)),
            ],
        );
    }
    // Requirements and orchestration state.
    insert(
        db,
        "work_requirements",
        &[
            ("requirement_id", t("r1")),
            ("task_id", t(TASK)),
            ("current_revision", i(2)),
            ("created_at", t(NOW)),
        ],
    );
    for (revision, statement) in [(1, "Old"), (2, "Ship it without /home/u leaks")] {
        insert(
            db,
            "work_requirement_revisions",
            &[
                ("requirement_id", t("r1")),
                ("revision", i(revision)),
                ("statement", t(statement)),
                ("criteria_json", t(r#"["fast",1,"token=x safe"]"#)),
                ("digest", t("d")),
                ("created_at", t(NOW)),
            ],
        );
    }
    insert(
        db,
        "task_orchestration",
        &[
            ("task_id", t(TASK)),
            ("state", t("WAITING")),
            ("updated_at", t(NOW)),
            ("wait_reason", t("user_input")),
            (
                "wait_detail_json",
                t(r#"{"explanation":"  Needs an answer about /private/x  "}"#),
            ),
        ],
    );
}

fn make_store(root: &Path, fill: impl FnOnce(&rusqlite::Connection, &str)) {
    let store = Store::open(
        ProjectKey::new(root, StoreChannel::Release).unwrap(),
        OpenMode::Initialize,
    )
    .unwrap();
    let text = root.to_str().unwrap().to_owned();
    store
        .write(|tx| {
            fill(tx, &text);
            Ok(())
        })
        .unwrap();
}

fn git(args: &[&str], cwd: &Path) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn main() {
    let scripts = scripts();
    let recording = std::env::var_os("CORTEX_PARITY_RECORD").is_some();
    let mut cases = Cases {
        count: 0,
        failures: Vec::new(),
        last: String::new(),
        golden: if recording { BTreeMap::new() } else { load_golden() },
        recording: recording.then(BTreeMap::new),
        seen: BTreeMap::new(),
    };

    // A populated store.
    let root = temp_dir("full");
    make_store(&root, populate);
    let r = root.to_str().unwrap();
    let wh = &scripts.worker_history;
    for (label, args) in [
        ("history: all", vec![r, TASK, "", "0", "200", "", "", ""]),
        (
            "history: compact cards",
            vec![r, TASK, "", "0", "200", "", "", "1"],
        ),
        (
            "history: one attempt",
            vec![r, TASK, "att_1", "0", "200", "", "", ""],
        ),
        (
            "history: by assignment ref",
            vec![r, TASK, "", "0", "200", "", "ref-2", ""],
        ),
        (
            "history: retry attempt",
            vec![r, TASK, "att_3", "0", "200", "", "ref-2b", ""],
        ),
        (
            "history: stuck attempt",
            vec![r, TASK, "", "0", "200", "", "ref-4", ""],
        ),
        (
            "history: unknown ref",
            vec![r, TASK, "", "0", "200", "", "ref-none", ""],
        ),
        ("history: paged", vec![r, TASK, "", "0", "2", "", "", ""]),
        (
            "history: cursor past events",
            vec![r, TASK, "", "9007199254740991", "1", "", "", ""],
        ),
        (
            "history: assignment cursor",
            vec![r, TASK, "", " +1_0 ", "0", "wa_1", "", ""],
        ),
        (
            "history: other task",
            vec![r, "t_other", "", "0", "200", "", "", ""],
        ),
        (
            "history: bad cursor",
            vec![r, TASK, "", "x", "200", "", "", ""],
        ),
    ] {
        cases.compare(
            label,
            wh,
            "worker-history",
            &args,
            label != "history: bad cursor",
        );
        match label {
            "history: all" => {
                for needle in [
                    "\"title\": \"Implement A\"",
                    "\"status\": \"interrupted\"",
                    "\"planUsedPercent\": 42.0",
                    "[redacted]",
                ] {
                    cases.expect(needle);
                }
            }
            "history: one attempt" => cases.expect("\"state\": \"sent\""),
            "history: paged" => cases.expect("\"truncated\": true, \"nextCursor\": 21"),
            _ => {}
        }
    }
    cases.compare(
        "results",
        &scripts.worker_results,
        "worker-results",
        &[r, TASK, r#"["ref-2","ref-1","missing","ref-2"]"#],
        true,
    );
    cases.compare(
        "results: none",
        &scripts.worker_results,
        "worker-results",
        &[r, TASK, "[]"],
        true,
    );
    cases.compare(
        "results: bad json",
        &scripts.worker_results,
        "worker-results",
        &[r, TASK, "{"],
        false,
    );
    for (label, assignment) in [
        ("commands: all", ""),
        ("commands: ref-1", "ref-1"),
        ("commands: other", "other"),
    ] {
        cases.compare(
            label,
            &scripts.running_commands,
            "running-commands",
            &[r, TASK, assignment],
            true,
        );
        if assignment != "other" {
            cases.expect(RUNNING_REF);
        }
    }
    cases.compare("pipeline", &scripts.pipeline, "pipeline", &[r, TASK], true);
    cases.expect("\"waiting\": \"Waiting for n1, n2, n0\"");
    cases.compare(
        "pipeline: other task",
        &scripts.pipeline,
        "pipeline",
        &[r, "t_other"],
        true,
    );
    cases.compare("task-id", &scripts.task_id, "task-id", &[r, SESSION], true);
    cases.compare(
        "task-id: unknown",
        &scripts.task_id,
        "task-id",
        &[r, "acps_ffffffffffff"],
        true,
    );

    // An empty initialized store.
    let empty = temp_dir("empty");
    make_store(&empty, |db, root| {
        insert(
            db,
            "tasks",
            &[
                ("id", t(TASK)),
                ("project_root", t(root)),
                ("created_at", t(NOW)),
            ],
        );
    });
    let e = empty.to_str().unwrap();
    cases.compare(
        "empty: history",
        wh,
        "worker-history",
        &[e, TASK, "", "0", "200", "", "", ""],
        true,
    );
    cases.compare(
        "empty: pipeline",
        &scripts.pipeline,
        "pipeline",
        &[e, TASK],
        true,
    );
    cases.compare(
        "empty: commands",
        &scripts.running_commands,
        "running-commands",
        &[e, TASK, ""],
        true,
    );
    cases.compare(
        "empty: results",
        &scripts.worker_results,
        "worker-results",
        &[e, TASK, r#"["a"]"#],
        true,
    );

    // A malformed worker payload: Python raises, so both exit 1 with no output.
    let crash = temp_dir("crash");
    make_store(&crash, |db, root| {
        populate(db, root);
        event(
            db,
            "worker_result",
            Some("xa_exec1"),
            "[1]",
            "2026-11-15T00:09:00Z",
        );
    });
    let c = crash.to_str().unwrap();
    cases.compare(
        "crash: history",
        wh,
        "worker-history",
        &[c, TASK, "", "0", "200", "", "", ""],
        false,
    );

    // No store: the history reader reports it cannot read; the others fail.
    let none = temp_dir("none");
    let n = none.to_str().unwrap();
    cases.compare(
        "missing store: history",
        wh,
        "worker-history",
        &[n, TASK, "", "0", "200", "", "", ""],
        true,
    );
    cases.compare(
        "missing store: pipeline",
        &scripts.pipeline,
        "pipeline",
        &[n, TASK],
        false,
    );

    // Project roots: plain, normalized, missing, relative, linked worktree.
    let pr = &scripts.project_root;
    let trailing = format!("{r}/");
    let dotted = format!("{r}/./");
    let missing = format!("{r}/missing");
    for (label, value, ok) in [
        ("root: plain", r, true),
        ("root: trailing slash", trailing.as_str(), true),
        ("root: dot part", dotted.as_str(), true),
        ("root: missing", missing.as_str(), false),
        ("root: relative", "relative/path", false),
    ] {
        cases.compare(label, pr, "project-root", &[value], ok);
    }
    let repo_root = temp_dir("git");
    let main_repo = repo_root.join("main");
    fs::create_dir_all(&main_repo).unwrap();
    let worktree = repo_root.join("linked");
    let have_git = git(&["init", "-q"], &main_repo)
        && git(
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "i",
            ],
            &main_repo,
        )
        && git(
            &["worktree", "add", "-q", worktree.to_str().unwrap()],
            &main_repo,
        );
    assert!(
        have_git,
        "git is required for the linked-worktree parity case"
    );
    cases.compare(
        "root: main repo",
        pr,
        "project-root",
        &[main_repo.to_str().unwrap()],
        true,
    );
    cases.compare(
        "root: linked worktree",
        pr,
        "project-root",
        &[worktree.to_str().unwrap()],
        true,
    );
    cases.expect(&format!("{}\n", main_repo.to_str().unwrap()));
    fs::write(worktree.join(".git"), "gitdir: /nonexistent\n").unwrap();
    cases.compare(
        "root: forged worktree",
        pr,
        "project-root",
        &[worktree.to_str().unwrap()],
        false,
    );

    // The readers never write: the database bytes are unchanged by a full Rust pass.
    let database = root.join(".cortex/cortex.sqlite3");
    let before = digest_file(&database);
    for (command, args) in [
        (
            "worker-history",
            vec![r, TASK, "att_1", "0", "200", "", "ref-1", ""],
        ),
        ("worker-results", vec![r, TASK, r#"["ref-2"]"#]),
        ("running-commands", vec![r, TASK, ""]),
        ("pipeline", vec![r, TASK]),
        ("task-id", vec![r, SESSION]),
    ] {
        assert_eq!(rust(command, &args).1, Some(0), "{command}");
    }
    assert_eq!(before, digest_file(&database), "a reader changed the store");

    for dir in [&root, &empty, &crash, &none, &repo_root] {
        let _ = fs::remove_dir_all(dir);
    }
    if !cases.failures.is_empty() {
        for failure in &cases.failures {
            eprintln!("{failure}\n");
        }
        panic!(
            "{} of {} parity cases differ",
            cases.failures.len(),
            cases.count
        );
    }
    if let Some(recorded) = cases.recording.take() {
        let document: serde_json::Map<String, serde_json::Value> = recorded
            .into_iter()
            .map(|(key, (stdout, code))| (key, serde_json::json!({"stdout": stdout, "code": code})))
            .collect();
        let path = std::env::var("CORTEX_PARITY_RECORD").unwrap();
        fs::write(&path, serde_json::to_string_pretty(&document).unwrap() + "\n").unwrap();
        println!("parity: recorded {} golden outcomes to {path}", cases.count);
        return;
    }
    assert_eq!(
        cases.golden.len(),
        cases.count,
        "the golden file holds cases this run did not exercise (or the reverse)"
    );
    println!("parity: {} cases identical to the recorded Python outcomes", cases.count);
}
