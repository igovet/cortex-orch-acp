from pathlib import Path
import json
import re
import sqlite3
import subprocess


ROOT = Path(__file__).resolve().parents[1]
SOURCE = (ROOT / "integrations/paseo/server/worker-history.ts").read_text()
RUNTIME_SOURCE = (ROOT / "src/cortex_runtime/acp/orchestrating_runtime.py").read_text()


def test_worker_history_uses_authoritative_workspace_handle_directory():
    assert "workspaces.ref(input.workspaceId)" in SOURCE
    assert "const directory = workspaceHandle.directory" in SOURCE
    # resolveProjectRoot now runs the python3 subprocess through the async,
    # concurrency-bounded runPython helper (python-runner.ts) instead of a
    # blocking spawnSync, so every caller awaits it. See runPython's own
    # comment (report r_6df0ca1ceeb1) for the measured event-loop blocking
    # this replaced.
    assert "const projectRoot = await resolveProjectRoot(directory)" in SOURCE
    assert "input.projectRoot" not in SOURCE


def test_worker_history_is_cursor_bounded_and_uri_escapes_paths():
    assert "and id>? and (event_type in" in SOURCE
    assert "order by id limit ?" in SOURCE
    assert "limit + 1" in SOURCE
    assert "quote(db_path, safe='/')" in SOURCE


def test_worker_history_redaction_replacement_cannot_raise_on_token_text():
    assert "r'\\1: [redacted]'" not in SOURCE
    assert "def scrub(value):" in SOURCE
    assert "[redacted]" in SOURCE
    assert "text[:12000]" not in SOURCE
    assert "event_out.append" in SOURCE
    assert "if more_assignments and assignments" in SOURCE


def test_worker_history_unwraps_only_the_known_paseo_v1_session_wrapper():
    assert 'const prefix = "plugin:"' in SOURCE
    assert "payload.version !== 1" in SOURCE
    assert 'payload.data.sessionId' in SOURCE
    assert "ACP_SESSION_ID = /^acps_[0-9a-f]{12}$/" in SOURCE
    assert "MAX_RUNTIME_SESSION_WRAPPER_LENGTH = 512" in SOURCE
    assert "MAX_RUNTIME_SESSION_PAYLOAD_LENGTH = 480" in SOURCE


def test_worker_history_rejects_malformed_or_ambiguous_wrappers():
    assert "JSON.parse(payloadText)" in SOURCE
    assert 'payloadText.includes("\\\\")' in SOURCE
    assert "occurrences !== 1" in SOURCE
    assert "payloadKeys.length !== 2" in SOURCE
    assert "dataKeys.length !== 1" in SOURCE
    assert "at['attempt_no'] == 1" in SOURCE
    assert "a['assignment_id']" in SOURCE


def test_worker_history_does_not_accept_foreign_identity_or_root_overrides():
    assert "parsePaseoRuntimeSessionId(agent?.runtimeInfo)" in SOURCE
    assert "input.projectRoot" not in SOURCE
    assert "runtimeInfo.projectRoot" not in SOURCE
    assert "runtimeInfo.taskId" not in SOURCE
    assert "select task_id from acp_host_sessions where session_id=? and project_root=?" in SOURCE


def test_worker_activity_payload_fields_are_persisted_and_projected():
    assert 'payload["input"] = input_value' in RUNTIME_SOURCE
    assert 'payload["output"] = output_value' in RUNTIME_SOURCE
    assert 'payload["error"] = error_value' in RUNTIME_SOURCE
    assert "input_value=_client_safe_raw(raw_input)" in RUNTIME_SOURCE
    assert "output_value=_client_safe_raw(raw_output)" in RUNTIME_SOURCE
    assert "for field in ('activity_ref', 'input', 'output', 'status', 'error', 'diff'" in SOURCE
    assert 'json.dumps(payload, sort_keys=True' in RUNTIME_SOURCE
    assert 'payload["tool_kind"] = kind' in RUNTIME_SOURCE
    assert 'payload["locations"] = _client_safe_raw(locations_value)' in RUNTIME_SOURCE
    assert 'payload["content"] = _client_safe_raw(content_value)' in RUNTIME_SOURCE
    assert 'payload["raw_input"] = input_value' in RUNTIME_SOURCE
    assert 'payload["raw_output"] = output_value' in RUNTIME_SOURCE


def test_embedded_history_query_binds_lifecycle_and_worker_events_to_one_attempt(tmp_path):
    """Exercise the real bounded Python query with the two event identities."""
    project = tmp_path / "project"
    store_dir = project / ".cortex"
    store_dir.mkdir(parents=True)
    db_path = store_dir / "cortex.sqlite3"
    db = sqlite3.connect(db_path)
    db.executescript("""
        create table tasks (id text primary key, project_root text);
        create table work_assignments (
            assignment_id text primary key, task_id text, node_key text,
            requirement_bindings_json text, candidate_ref text, status text,
            idempotency_key text, created_at text
        );
        create table work_assignment_attempts (
            attempt_id text primary key, task_id text, assignment_id text,
            attempt_no integer, predecessor_attempt_id text,
            input_candidate_ref text, requirement_bindings_json text,
            status text, failure_reason text, idempotency_key text, created_at text
        );
        create table task_events (
            id integer primary key, task_id text, event_type text,
            assignment_ref text, created_at text, payload_json text
        );
        create table attempt_route_audit (
            id text primary key, attempt_id text, task_id text, engine text,
            model text, decision_detail_json text, recorded_at text
        );
        create table delegation_provisioning (
            idempotency_key text primary key, task_id text, attempt_id text
        );
    """)
    task = "t_fixture"
    db.execute("insert into tasks values (?,?)", (task, str(project)))
    db.execute("insert into work_assignments values (?,?,?,?,?,?,?,?)",
               ("a_fixture", task, "worker", "{}", None, "planned", "request_fixture", "now"))
    db.execute("insert into work_assignment_attempts values (?,?,?,?,?,?,?,?,?,?,?)",
               ("xa_first", task, "a_fixture", 1, None, None, "{}", "reported", None, "attempt_first", "now"))
    db.execute("insert into work_assignment_attempts values (?,?,?,?,?,?,?,?,?,?,?)",
               ("xa_revision", task, "a_fixture", 2, "xa_first", None, "{}", "running", None, "attempt_revision", "now"))
    db.execute("insert into work_assignments values (?,?,?,?,?,?,?,?)",
               ("a_pending", task, "worker-pending", "{}", None, "planned", "request_pending", "now"))
    rows = [
        (1, task, "delegation_requested", "a_fixture", "1", {"title": "first", "work_attempt_ref": "xa_revision"}),
        (2, task, "assignment_ready", "request_fixture", "2", {"status": "running"}),
        (3, task, "progress", "xa_first", "3", {"kind": "worker_activity", "text": "old"}),
        (4, task, "worker_result", "xa_first", "4", {"summary": "old result"}),
        (5, task, "progress", "exec_revision", "5", {
            "kind": "worker_activity", "text": "current", "status": "failed",
            "activity_ref": "tool:fixture-edit",
            "input": {"path": "src/app.py"}, "output": "permission denied",
            "error": "permission denied",
            "tool_kind": "edit", "title": "Edit file",
            "locations": [{"path": "src/app.py", "line": 7}],
            "content": {"type": "text", "text": "updated"},
            "raw_input": {"path": "src/app.py", "oldText": "a", "newText": "b"},
            "raw_output": {"error": "permission denied"},
        }),
        (6, task, "worker_result", "exec_revision", "6", {"summary": "current result"}),
        # Legacy lifecycle markers can arrive after the worker has already
        # published its result. They must not downgrade the exact attempt.
        (7, task, "assignment_failed", "request_fixture", "7", {"error": "late compatibility failure"}),
        (8, task, "assignment_cancelled", "request_fixture", "8", {"reason": "late compatibility cancellation"}),
    ]
    db.executemany("insert into task_events values (?,?,?,?,?,?)",
                   [(i, t, typ, ref, created, json.dumps(payload)) for i, t, typ, ref, created, payload in rows])
    db.execute("insert into delegation_provisioning values (?,?,?)", ("a_fixture", task, "exec_revision"))
    db.execute("insert into attempt_route_audit values (?,?,?,?,?,?,?)",
               ("audit_revision", "exec_revision", task, "codex", "model-fixture",
                json.dumps({"effort": "high", "model_name": "Model Fixture Resolved"}), "7"))
    db.commit()
    db.close()

    match = re.search(r"const PYTHON = String\.raw`\n(.*?)\n`;", SOURCE, re.S)
    assert match, "embedded history query was not found"
    helpers = re.search(r"export const STORE_PYTHON_HELPERS = String\.raw`(.*?)`;", SOURCE, re.S)
    assert helpers, "shared store helpers were not found"
    query = match.group(1).replace("${STORE_PYTHON_HELPERS}", helpers.group(1))
    result = subprocess.run(
        ["python3", "-B", "-c", query, str(project), task, "xa_revision", "0", "200", "", ""],
        capture_output=True, text=True, check=True,
    )
    output = json.loads(result.stdout)
    events = output["assignments"][0]["attempts"][0]["events"]
    assert [event["id"] for event in events] == [5, 6]
    assert output["assignments"][0]["title"] == "first"
    assert output["assignments"][0]["status"] == "completed"
    assert output["assignments"][0]["attempts"][0]["status"] == "completed"
    assert output["assignments"][0]["attempts"][0]["route"] == {
        "provider": "codex", "model": "model-fixture", "modelName": "Model Fixture Resolved", "effort": "high",
    }
    assert output["assignments"][0]["attempts"][0]["completedAt"] == "6"
    assert events[0]["input"] == {"path": "src/app.py"}
    assert events[0]["output"] == "permission denied"
    assert events[0]["status"] == "failed"
    assert events[0]["activity_ref"] == "tool:fixture-edit"
    assert events[0]["error"] == "permission denied"
    assert events[0]["tool_kind"] == "edit"
    assert events[0]["title"] == "Edit file"
    assert events[0]["locations"] == [{"path": "src/app.py", "line": 7}]
    assert events[0]["content"] == {"type": "text", "text": "updated"}
    assert events[0]["raw_input"]["newText"] == "b"
    assert events[0]["raw_output"] == {"error": "permission denied"}

    # The terminal marker is outside a deliberately tiny history page; the
    # exact-attempt status query must still report the durable completion.
    bounded = subprocess.run(
        ["python3", "-B", "-c", query, str(project), task, "xa_revision", "0", "1", "", ""],
        capture_output=True, text=True, check=True,
    )
    bounded_output = json.loads(bounded.stdout)
    assert bounded_output["assignments"][0]["attempts"][0]["status"] == "completed"
    assert bounded_output["assignments"][0]["attempts"][0]["completedAt"] == "6"

    # Simulate the live worker appending a message after the first read. A
    # cursor read returns the new durable row before another terminal result
    # is needed, which is the server half of the panel's incremental stream.
    db = sqlite3.connect(db_path)
    db.execute("insert into task_events values (?,?,?,?,?,?)", (
        9, task, "progress", "exec_revision", "9",
        json.dumps({"kind": "worker_activity", "text": "still working"}),
    ))
    db.commit()
    db.close()
    incremental = subprocess.run(
        ["python3", "-B", "-c", query, str(project), task, "xa_revision", "8", "200", "", ""],
        capture_output=True, text=True, check=True,
    )
    incremental_output = json.loads(incremental.stdout)
    assert [event["id"] for event in incremental_output["assignments"][0]["attempts"][0]["events"]] == [9]
    # Terminal state is read independently of the bounded event page, so the
    # exact attempt remains completed even when this cursor page contains only
    # a later progress event.
    assert incremental_output["assignments"][0]["attempts"][0]["status"] == "completed"

    pending = subprocess.run(
        ["python3", "-B", "-c", query, str(project), task, "", "0", "200", "", "request_pending"],
        capture_output=True, text=True, check=True,
    )
    pending_output = json.loads(pending.stdout)
    assert pending_output["assignments"] == [{
        "taskId": task,
        "id": "a_pending",
        "assignmentRef": "request_pending",
        "nodeKey": "worker-pending",
        "status": "planned",
        "attempts": [],
    }]

    # Explicit plans have distinct graph, delegation and execution identities.
    # This is the production shape: clicking the delegate receipt must resolve
    # the exact graph attempt and its execution transcript, not an empty feed.
    db = sqlite3.connect(db_path)
    db.execute("update task_events set assignment_ref=?, payload_json=? where id=1", (
        "delegate_hash", json.dumps({"title": "first", "plan_assignment_ref": "a_fixture", "work_attempt_ref": "xa_revision"}),
    ))
    db.execute("update delegation_provisioning set idempotency_key=?", ("delegate_hash",))
    db.commit()
    db.close()
    for selected_ref in ("", "delegate_hash"):
        explicit = subprocess.run(
            ["python3", "-B", "-c", query, str(project), task, "", "0", "200", "", selected_ref],
            capture_output=True, text=True, check=True,
        )
        resolved = json.loads(explicit.stdout)
        assignment = next(a for a in resolved["assignments"] if a["id"] == "a_fixture")
        assert assignment["assignmentRef"] == "delegate_hash"
        current = next(a for a in assignment["attempts"] if a["id"] == "xa_revision")
        assert [e["id"] for e in current["events"]] == [5, 6, 9]
        assert current["route"]["provider"] == "codex"
        if selected_ref:
            assert len(assignment["attempts"]) == 1
    mismatch = subprocess.run(
        ["python3", "-B", "-c", query, str(project), task, "xa_first", "0", "200", "", "delegate_hash"],
        capture_output=True, text=True, check=True,
    )
    assert json.loads(mismatch.stdout)["assignments"] == []

    # Publication is not acceptance: runtime validation remains authoritative
    # even when its failure falls outside the selected history page.
    db = sqlite3.connect(db_path)
    db.execute('create table runtime_nodes (task_id text, assignment_ref text, status text, result_json text)')
    db.execute('insert into runtime_nodes values (?,?,?,?)',
               (task, 'delegate_hash', 'failed', json.dumps({'checks': [{'name': 'actual check', 'outcome': 'unverified'}]})))
    db.commit()
    db.close()
    failed = subprocess.run(
        ["python3", "-B", "-c", query, str(project), task, "xa_revision", "8", "1", "", "delegate_hash"],
        capture_output=True, text=True, check=True,
    )
    failed_assignment = json.loads(failed.stdout)['assignments'][0]
    assert failed_assignment['status'] == 'completed_with_issues'
    assert failed_assignment['attempts'][0]['status'] == 'completed_with_issues'
    assert failed_assignment['attempts'][0]['failureReason'].startswith('Completed with issues.')
    assert 'actual check: unverified' in failed_assignment['attempts'][0]['failureReason']

    # A failed node without a retained worker result is a real execution
    # failure, rather than a completed result with rejected checks.
    db = sqlite3.connect(db_path)
    db.execute("update runtime_nodes set result_json=null where task_id=? and assignment_ref=?", (task, 'delegate_hash'))
    db.commit()
    db.close()
    execution_failed = subprocess.run(
        ["python3", "-B", "-c", query, str(project), task, "xa_revision", "8", "1", "", "delegate_hash"],
        capture_output=True, text=True, check=True,
    )
    assert json.loads(execution_failed.stdout)['assignments'][0]['status'] == 'failed'

    # Existing worker reads lose document bodies on projection; paths/status survive.
    db = sqlite3.connect(db_path)
    for i, title in [(10, 'mcp.cortex.project_read'), (11, 'mcp__cortex__read_skill')]:
        db.execute('insert into task_events values (?,?,?,?,?,?)', (i, task, 'progress', 'exec_revision', str(i), json.dumps({
            'kind': 'worker_activity', 'title': title, 'text': 'READ_BODY_MARKER',
            'tool_kind': 'read', 'status': 'completed', 'activity_ref': f'tool:read-{i}',
            'raw_input': {'path': 'README.md'}, 'raw_output': 'READ_BODY_MARKER',
            'content': [{'type': 'text', 'text': 'READ_BODY_MARKER'}],
        })))
    db.commit()
    db.close()
    reads = subprocess.run(['python3', '-B', '-c', query, str(project), task, 'xa_revision', '9', '200', '', 'delegate_hash'], capture_output=True, text=True, check=True)
    assert 'READ_BODY_MARKER' not in reads.stdout
    assert 'README.md' in reads.stdout
    assert 'completed' in reads.stdout

    # A context-fill row is the attempt's status, never a transcript event.
    db = sqlite3.connect(db_path)
    db.execute('insert into task_events values (?,?,?,?,?,?)', (12, task, 'progress', 'exec_revision', '12', json.dumps({
        'kind': 'worker_activity', 'activity_ref': 'context', 'text': 'context 84000/200000',
        'tool_kind': 'usage_update', 'content': {'sessionUpdate': 'usage_update', 'used': 84000, 'size': 200000},
    })))
    db.commit()
    db.close()
    fill = json.loads(subprocess.run(['python3', '-B', '-c', query, str(project), task, 'xa_revision', '11', '200', '', 'delegate_hash'], capture_output=True, text=True, check=True).stdout)
    attempt = fill['assignments'][0]['attempts'][0]
    assert attempt['events'] == []
    assert attempt['context'] == {'used': 84000, 'size': 200000}
