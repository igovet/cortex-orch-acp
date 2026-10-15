"""Safety and metric coverage for the offline engine-quality analyzer."""
import importlib.util
import json
from pathlib import Path
import sqlite3

from cortex_runtime.store import Store

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "cortex_engine_quality", ROOT / "scripts/cortex_engine_quality.py"
)
quality = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(quality)


def _fixture_store(tmp_path, *, reviewer_model="model-review", coordinator_model="model-coord"):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    with store.connection() as db:
        db.execute("INSERT INTO tasks(id,project_root,created_at) VALUES(?,?,?)",
                   ("task-1", str(tmp_path), "2026-01-01T00:00:00Z"))
        db.execute(
            "INSERT INTO runtime_tasks(task_id,state,revision,goal,plan_json,review_cycle,created_at,updated_at) "
            "VALUES(?,?,?,?,?,?,?,?)",
            ("task-1", "FAILED", 1, "fixture", '{"verification":[["pytest"]]}', 0,
             "2026-01-01T00:00:00Z", "2026-01-01T00:01:00Z"),
        )
        db.execute(
            "INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,"
            "owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
            ("exec-ref", "task-1", "attempt-impl", "backend-dev", "ready", "ready", 1,
             "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,"
            "owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
            ("review-ref", "task-1", "attempt-review", "reviewer", "ready", "ready", 1,
             "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
        )
        for attempt, role, model in (
            ("attempt-impl", "backend-dev", "model-impl"),
            ("attempt-review", "reviewer", reviewer_model),
        ):
            db.execute(
                "INSERT INTO attempt_route_audit(id,attempt_id,task_id,role,engine,connection_id,model,"
                "params_fingerprint,decision,decision_detail_json,recorded_at) "
                "VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                (f"audit-{attempt}", attempt, "task-1", role, "fake-engine", "fixture",
                 model, "fixture", "authorized", "{}", "2026-01-01T00:00:00Z"),
            )
        db.execute(
            "INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,"
            "scope_json,stage,status,assignment_ref,result_json) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            ("task-1", 1, "impl", "backend-dev", "fixture", "[]", "[]", "execution",
             "failed", "exec-ref", "{}"),
        )
        db.execute(
            "INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,"
            "scope_json,stage,status,assignment_ref,result_json) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            ("task-1", 1, "review", "reviewer", "fixture", "[]", "[]", "review",
             "completed", "review-ref", '{"verdict":"APPROVE"}'),
        )
        verification = {
            "revision": 1,
            "revision_digest": "a" * 64,
            "checks": [{
                "argv": ["pytest"], "exit_code": 0, "complete": True,
                "revision_digest": "a" * 64, "command_ref": "verification:1:0:0",
            }],
        }
        db.execute(
            "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)",
            ("task-1", "VERIFICATION_PASSED", "runtime", json.dumps(verification),
             "2026-01-01T00:00:30Z"),
        )
        db.execute(
            "INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,"
            "attempt_id,owner_epoch,issued_at) VALUES(?,?,?,?,?,?,?,?)",
            ("worker-channel", "hash", "task-1", "thread", "worker", "attempt-impl", 1,
             "2026-01-01T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,"
            "owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
            ("coord-ref", "task-1", "attempt-coord", "coordinator", "ready", "ready", 1,
             "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO attempt_route_audit(id,attempt_id,task_id,role,engine,connection_id,model,"
            "params_fingerprint,decision,decision_detail_json,recorded_at) "
            "VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            ("audit-coord", "attempt-coord", "task-1", "coordinator", "fake-engine", "fixture",
             coordinator_model, "fixture", "authorized", "{}", "2026-01-01T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,"
            "attempt_id,owner_epoch,issued_at) VALUES(?,?,?,?,?,?,?,?)",
            ("coordinator-channel", "hash", "task-1", "thread", "coordinator", "attempt-coord", 1,
             "2026-01-01T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO acp_host_sessions(session_id,project_root,task_id,client_info,"
            "client_capabilities,mcp_servers,created_at) VALUES(?,?,?,?,?,?,?)",
            ("session-1", str(tmp_path), "task-1", "{}", "{}", "[]", "2026-01-01T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO host_transcript(session_id,seq,kind,text,created_at) VALUES(?,?,?,?,?)",
            ("session-1", 1, "agent_message", "Route receipt PRIVATE_REPLY_SECRET", "2026-01-01T00:00:20Z"),
        )
        db.execute(
            "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)",
            ("task-1", "TOOL_AUTHORIZED", "worker-channel",
             '{"operation":"ask_question","args":{"text":"PRIVATE_TOOL_SECRET"}}', "2026-01-01T00:00:10Z"),
        )

        # Second model-impl attempt, on a different task, with a command_run
        # call instead of a process-tool call, so the per-attempt rates must
        # divide two distinct totals by two attempts rather than one.
        db.execute("INSERT INTO tasks(id,project_root,created_at) VALUES(?,?,?)",
                   ("task-2", str(tmp_path), "2026-01-02T00:00:00Z"))
        db.execute(
            "INSERT INTO runtime_tasks(task_id,state,revision,goal,plan_json,review_cycle,created_at,updated_at) "
            "VALUES(?,?,?,?,?,?,?,?)",
            ("task-2", "COMPLETED", 1, "fixture", '{}', 0,
             "2026-01-02T00:00:00Z", "2026-01-02T00:01:00Z"),
        )
        db.execute(
            "INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,"
            "owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
            ("exec-ref-2", "task-2", "attempt-impl-2", "backend-dev", "ready", "ready", 1,
             "2026-01-02T00:00:00Z", "2026-01-02T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO attempt_route_audit(id,attempt_id,task_id,role,engine,connection_id,model,"
            "params_fingerprint,decision,decision_detail_json,recorded_at) "
            "VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            ("audit-attempt-impl-2", "attempt-impl-2", "task-2", "backend-dev", "fake-engine", "fixture",
             "model-impl", "fixture", "authorized", "{}", "2026-01-02T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,"
            "scope_json,stage,status,assignment_ref,result_json) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            ("task-2", 1, "impl", "backend-dev", "fixture", "[]", "[]", "execution",
             "completed", "exec-ref-2", "{}"),
        )
        db.execute(
            "INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,"
            "attempt_id,owner_epoch,issued_at) VALUES(?,?,?,?,?,?,?,?)",
            ("worker-channel-2", "hash", "task-2", "thread", "worker", "attempt-impl-2", 1,
             "2026-01-02T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)",
            ("task-2", "TOOL_AUTHORIZED", "worker-channel-2",
             '{"operation":"command_run","args":{"argv":["PRIVATE_COMMAND_SECRET"]}}', "2026-01-02T00:00:10Z"),
        )

        # A model with an authorized process-tool call but no execution node
        # is outside the measured population and must not enter the totals.
        db.execute(
            "INSERT INTO attempt_route_audit(id,attempt_id,task_id,role,engine,connection_id,model,"
            "params_fingerprint,decision,decision_detail_json,recorded_at) "
            "VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            ("audit-noexec", "attempt-noexec", "task-1", "backend-dev", "fake-engine", "fixture",
             "model-noattempt", "fixture", "authorized", "{}", "2026-01-01T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,"
            "attempt_id,owner_epoch,issued_at) VALUES(?,?,?,?,?,?,?,?)",
            ("channel-noexec", "hash", "task-1", "thread", "worker", "attempt-noexec", 1,
             "2026-01-01T00:00:00Z"),
        )
        db.execute(
            "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)",
            ("task-1", "TOOL_AUTHORIZED", "channel-noexec",
             '{"operation":"read_skill","args":{"skill":"PRIVATE_SKILL_SECRET"}}', "2026-01-01T00:00:15Z"),
        )
        db.commit()
    return store


def test_cli_emits_only_aggregates_and_computes_requested_metrics(tmp_path, capsys):
    _fixture_store(tmp_path)
    assert quality.main(["--project", str(tmp_path)]) == 0
    output = capsys.readouterr().out
    data = json.loads(output)

    assert data["models"]["model-impl"]["attempts"] == 2
    assert data["models"]["model-impl"]["failed"] == 1
    assert data["models"]["model-impl"]["completed"] == 1
    assert data["models"]["model-impl"]["ask_question"] == 1
    assert data["models"]["model-impl"]["process_tool_calls"] == 1
    assert data["models"]["model-impl"]["command_run_calls"] == 1
    assert data["models"]["model-impl"]["failed_with_passing_final_verification"] == 1
    # Per-attempt rates: raw totals divided by this model's 2 attempts.
    assert data["models"]["model-impl"]["process_tool_calls_per_attempt"] == 0.5
    assert data["models"]["model-impl"]["command_run_calls_per_attempt"] == 0.5
    # Calls without a counted execution attempt cannot create a model row.
    assert "model-noattempt" not in data["models"]
    assert data["review_pairs"]["model-impl -> model-review"] == {
        "reviews": 1, "approved": 1, "approval_rate": 1.0
    }
    assert data["coordinator_replies"]["model-coord"] == {
        "count": 1, "mean_words": 3.0, "process_word_density": 2 / 3
    }
    assert "PRIVATE_REPLY_SECRET" not in output
    assert "PRIVATE_TOOL_SECRET" not in output
    assert "PRIVATE_COMMAND_SECRET" not in output
    assert "PRIVATE_SKILL_SECRET" not in output
    assert "fixture" not in output
    assert "report" not in output


def test_call_rates_exclude_same_model_review_and_coordinator_attempts(tmp_path):
    store = _fixture_store(tmp_path, reviewer_model="model-impl", coordinator_model="model-impl")
    with store.connection() as db:
        db.execute(
            "INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,"
            "attempt_id,owner_epoch,issued_at) VALUES(?,?,?,?,?,?,?,?)",
            ("review-channel", "hash", "task-1", "thread", "worker", "attempt-review", 1,
             "2026-01-01T00:00:00Z"),
        )
        # The same model makes both kinds of counted calls in all three roles.
        # Only the execution calls belong with its two execution attempts.
        for channel in ("worker-channel", "review-channel", "coordinator-channel"):
            for operation in ("read_skill", "command_run"):
                db.execute(
                    "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) "
                    "VALUES(?,?,?,?,?)",
                    ("task-1", "TOOL_AUTHORIZED", channel,
                     json.dumps({"operation": operation}), "2026-01-01T00:00:25Z"),
                )
        db.commit()

    data = quality.read_metrics(tmp_path)
    counts = data["models"]["model-impl"]
    assert counts["attempts"] == 2
    assert counts["failed"] == 1
    assert counts["completed"] == 1
    assert counts["ask_question"] == 1
    assert counts["read_skill"] == 1
    assert counts["process_tool_calls"] == 2
    assert counts["command_run_calls"] == 2
    assert counts["process_tool_calls_per_attempt"] == 1.0
    assert counts["command_run_calls_per_attempt"] == 1.0
    assert data["review_pairs"]["model-impl -> model-impl"] == {
        "reviews": 1, "approved": 1, "approval_rate": 1.0
    }
