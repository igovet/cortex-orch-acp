"""Disposable, no-paid plan/error scenarios; no retained task is replayed."""
import copy
import json

import pytest

from cortex_runtime.server import Server
from test_stdio import bootstrap, exchange


def plan():
    return dict(tasks=[
        dict(id="repair", role="backend-dev", complexity="normal",
             model="codex-acp/example", model_reason="An explicitly selected fixture route.",
             objective="PRIVATE_OBJECTIVE", scope=["src/**"], dependencies=[]),
        dict(id="verify", role="qa-engineer", complexity="normal",
             model="codex-acp/example", model_reason="The same fixture route.",
             objective="Verify the disposable result.", scope=["."], dependencies=["repair"]),
    ], verification=[["python3", "-B", "-m", "pytest", "-q"]])


@pytest.fixture
def scenario(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.delenv("CORTEX_ROUTING", raising=False)
    store, boot = bootstrap(tmp_path)
    server = Server(channel_credentials=(boot.channel_id, boot.secret, str(tmp_path)))
    return store, boot, server


def call(scenario, arguments=None):
    return scenario[2].dispatch("tools/call", dict(name="plan_submit", arguments=arguments or
                               dict(plan=plan(), request_key="fixture-plan")))


def body(reply):
    return json.loads(reply["content"][0]["text"])


def snapshot(store):
    with store.connection() as db:
        tables = ("runtime_tasks", "runtime_nodes", "runtime_events", "runtime_requests",
                  "delegation_provisioning", "attempt_route_audit")
        return {table: [tuple(row) for row in db.execute(f"SELECT * FROM {table}")]
                for table in tables}


@pytest.mark.parametrize("wire", [False, True], ids=["dispatch", "stdio"])
@pytest.mark.parametrize("operation", ["plan_submit", "list_route_options"])
@pytest.mark.parametrize("policy,code", [
    (None, "model_policy_migration_required"),
    ('{"model_policy_version": 999}', "model_policy_version_unsupported"),
    ('{"engine_affinity":{"PRIVATE_KEY":"PRIVATE_VALUE"}}', "invalid_routing_overrides"),
])
def test_real_policy_preflight_is_not_an_uncertain_storage_failure(scenario, tmp_path, policy, code, wire, operation):
    store, boot, _ = scenario
    path = tmp_path / ".cortex" / "routing.json"
    if policy is not None:
        path.write_text(policy)
    before = snapshot(store)
    arguments = dict(plan=plan(), request_key="fixture-plan") if operation == "plan_submit" else {}
    if wire:
        reply = exchange(tmp_path, [dict(jsonrpc="2.0", id="plan-error", method="tools/call",
                         params=dict(name=operation, arguments=arguments))],
                         channel=boot)[0]["result"]
    else:
        reply = scenario[2].dispatch("tools/call", dict(name=operation, arguments=arguments))
    error = body(reply)
    assert reply["isError"] and error["error"] == code
    assert error["category"] == "validation"
    assert error["write_state"] == "rejected"
    assert error["retry_eligible"] is False
    assert error["correction"] and "disk" not in error["correction"]
    assert all(value not in json.dumps(reply) for value in ("PRIVATE_", str(tmp_path), "Traceback"))
    assert snapshot(store) == before


def automatic_plan():
    value = plan()
    for node in value["tasks"]:
        node["model"] = "auto"
        node.pop("model_reason")
    return value


def assert_committed_once(store, boot):
    with store.connection() as db:
        task = db.execute("SELECT state,revision,plan_json FROM runtime_tasks WHERE task_id=?",
                          (boot.task_id,)).fetchone()
        assert task["state"] == "PLAN_READY" and task["revision"] == 1
        assert json.loads(task["plan_json"]) == automatic_plan()
        assert db.execute("SELECT count(*) FROM runtime_nodes").fetchone()[0] == 2
        assert db.execute("SELECT count(*) FROM runtime_requests WHERE operation='plan_submit'").fetchone()[0] == 1
        assert db.execute("SELECT count(*) FROM runtime_events WHERE type='PLAN_ACCEPTED'").fetchone()[0] == 1
        assert db.execute("SELECT count(*) FROM delegation_provisioning").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM attempt_route_audit").fetchone()[0] == 0


def portable(category, code="fixture_failure"):
    from cortex_runtime.portable.errors import PortableError, PortableErrorCategory
    return PortableError(PortableErrorCategory(category), code,
                         "PRIVATE_REMEDIATION /private/path", category == "transport",
                         field="/private/field", details="PRIVATE_DETAILS")


@pytest.mark.parametrize("category", ["validation", "authorization", "transport", "engine_failure",
                                      "indeterminate_write"])
def test_portable_preflight_preserves_category_without_copying_values(scenario, monkeypatch, capsys, category):
    from cortex_runtime.routing import auto_route
    def fail(*args, **kwargs):
        raise portable(category)
    monkeypatch.setattr(auto_route, "route_menu", fail)
    before = snapshot(scenario[0])
    error = body(call(scenario))
    assert error["error"] == "fixture_failure" and error["category"] == category
    assert error["write_state"] == ("uncertain" if category == "indeterminate_write" else "rejected")
    assert error["retry_eligible"] == (category == "transport")
    assert "PRIVATE" not in json.dumps(error) and "/private" not in json.dumps(error)
    assert snapshot(scenario[0]) == before
    assert capsys.readouterr().err == ""


@pytest.mark.parametrize("name", ["plan_submit", "create_draft"])
@pytest.mark.parametrize("category", ["validation", "authorization", "transport", "engine_failure",
                                      "indeterminate_write"])
def test_unlocated_portable_failure_never_claims_transaction_disposition(scenario, monkeypatch, capsys,
                                                                       name, category):
    from cortex_runtime import server as server_module
    def fail(*args, **kwargs):
        raise portable(category)
    if name == "plan_submit":
        monkeypatch.setattr(server_module.delegation_mcp_tools, "dispatch", fail)
        arguments = dict(plan=automatic_plan(), request_key="unknown-stage")
    else:
        monkeypatch.setattr(Server, "_dispatch_channeled", fail)
        arguments = dict(template="general", request_key="unknown-stage")
    before = snapshot(scenario[0])
    reply = scenario[2].dispatch("tools/call", dict(name=name, arguments=arguments))
    error = body(reply)
    assert error["error"] == "fixture_failure" and error["category"] == category
    assert error["write_state"] == "uncertain" and error["retry_eligible"] is False
    assert "identical request and delivery key" in error["correction"]
    assert "storage_error" not in json.dumps(error)
    assert "PRIVATE" not in json.dumps(error) and "/private" not in json.dumps(error)
    assert snapshot(scenario[0]) == before
    assert capsys.readouterr().err == ""


def test_real_validation_and_authority_rejections_leave_plan_state_unchanged(scenario):
    from cortex_runtime.orchestration.state import TaskMachine
    store, boot, _ = scenario
    before = snapshot(store)
    invalid = automatic_plan()
    invalid["tasks"][0]["scope"] = [".cortex/**"]
    error = body(call(scenario, dict(plan=invalid, request_key="invalid")))
    assert error["error"] == "forbidden_assignment_scope" and error["write_state"] == "rejected"
    assert snapshot(store) == before
    TaskMachine(store).cancel(boot.task_id)
    before = snapshot(store)
    error = body(call(scenario, dict(plan=automatic_plan(), request_key="inactive")))
    assert error["error"] == "gateway_task_inactive" and error["write_state"] == "rejected"
    assert snapshot(store) == before


@pytest.mark.parametrize("stage", ["precommit", "commit_before", "commit_after", "reply_storage",
                                   "reply_portable", "commit_before_portable", "commit_after_portable"])
def test_fault_stage_atomicity_and_exact_reconciliation(scenario, monkeypatch, stage):
    from contextlib import contextmanager
    import sqlite3
    from cortex_runtime import server as server_module
    from cortex_runtime.store import Store

    store, boot, _ = scenario
    before = snapshot(store)
    arguments = dict(plan=automatic_plan(), request_key="exact-fixture")
    original_connection = Store.connection
    original_reply = server_module._reply_content
    fired = False

    class FaultConnection:
        def __init__(self, db):
            self.db = db
            self.plan_write = False
        def __getattr__(self, name):
            return getattr(self.db, name)
        def execute(self, sql, parameters=()):
            nonlocal fired
            result = self.db.execute(sql, parameters)
            if sql.startswith("INSERT INTO runtime_nodes"):
                self.plan_write = True
                if stage == "precommit" and not fired:
                    fired = True
                    raise sqlite3.OperationalError("PRIVATE_STORAGE")
            return result
        def commit(self):
            nonlocal fired
            if self.plan_write and stage.startswith("commit_") and not fired:
                fired = True
                if stage.startswith("commit_after"):
                    self.db.commit()
                if stage.endswith("_portable"):
                    raise portable("validation")
                raise sqlite3.OperationalError("PRIVATE_STORAGE")
            return self.db.commit()

    @contextmanager
    def connection(self, **kwargs):
        with original_connection(self, **kwargs) as db:
            yield FaultConnection(db)

    def reply(data):
        nonlocal fired
        if stage.startswith("reply_") and not fired and data.get("state") == "PLAN_READY":
            fired = True
            if stage == "reply_portable":
                raise portable("validation")
            raise OSError(5, "PRIVATE_STORAGE", "/private/path")
        return original_reply(data)

    with monkeypatch.context() as fault:
        fault.setattr(Store, "connection", connection)
        fault.setattr(server_module, "_reply_content", reply)
        error = body(call(scenario, arguments))
    assert fired and error["write_state"] == "uncertain"
    assert error["error"] == ("fixture_failure" if stage.endswith("_portable") else "storage_error")
    assert "PRIVATE" not in json.dumps(error) and "/private" not in json.dumps(error)
    committed = stage.startswith("commit_after") or stage.startswith("reply_")
    if committed:
        assert_committed_once(store, boot)
    else:
        assert snapshot(store) == before  # observed rollback, not inferred from the envelope

    accepted = call(scenario, arguments)
    assert not accepted["isError"]
    assert_committed_once(store, boot)
    after = snapshot(store)
    replay = call(scenario, copy.deepcopy(arguments))
    assert not replay['isError'] and body(replay) == body(accepted)
    assert replay['structuredContent'] == accepted['structuredContent']
    assert snapshot(store) == after
    changed = copy.deepcopy(arguments)
    changed["plan"]["tasks"][0]["objective"] = "different"
    assert body(call(scenario, changed))["error"] == "request_key_conflict"
    assert body(call(scenario, dict(arguments, request_key="changed-key")))["error"] == "plan_not_expected"
    assert snapshot(store) == after


def test_exact_replay_and_conflict_precede_current_route_policy_reads(scenario, monkeypatch):
    from cortex_runtime.routing import auto_route
    monkeypatch.setattr(auto_route, "route_menu", lambda *a, **k:
                        dict(tiers=dict(standard=[dict(route="codex-acp/example")])))
    monkeypatch.setattr(auto_route, "user_model_request", lambda *a: None)
    monkeypatch.setattr(auto_route, "ambiguous_user_model_request", lambda *a: None)
    arguments = dict(plan=plan(), request_key="replayed")
    accepted = call(scenario, arguments)
    assert not accepted["isError"]
    after = snapshot(scenario[0])
    def unavailable(*args, **kwargs):
        pytest.fail("reconciliation must not consult current policy")
    monkeypatch.setattr(auto_route, "route_menu", unavailable)
    replay = call(scenario, copy.deepcopy(arguments))
    assert not replay['isError'] and body(replay) == body(accepted)
    assert replay['structuredContent'] == accepted['structuredContent']
    changed = copy.deepcopy(arguments)
    changed["plan"]["tasks"][0]["objective"] = "different"
    assert body(call(scenario, changed))["error"] == "request_key_conflict"
    assert snapshot(scenario[0]) == after


def test_storage_fault_in_preflight_is_conservative_and_rolls_back(scenario, monkeypatch):
    import sqlite3
    from cortex_runtime.routing import auto_route
    def fail(*args, **kwargs):
        raise sqlite3.OperationalError("PRIVATE_STORAGE")
    monkeypatch.setattr(auto_route, "route_menu", fail)
    before = snapshot(scenario[0])
    error = body(call(scenario))
    assert error["error"] == "storage_error" and error["write_state"] == "uncertain"
    assert error["received"] == "OperationalError" and "PRIVATE" not in json.dumps(error)
    assert snapshot(scenario[0]) == before


def test_public_diagnostics_do_not_copy_exception_owned_values():
    from cortex_runtime.contracts import PortableToolError, storage_failure
    error = body(Server.error(PortableToolError(portable("validation", "/private/PRIVATE_CODE"),
                                               rejected=True)))
    assert error["error"] == "portable_error"
    assert "PRIVATE" not in json.dumps(error) and "/private" not in json.dumps(error)
    error = body(Server.error(storage_failure(OSError(28, "PRIVATE_REASON", "/private/file"))))
    assert error["received"] == "OSError: No space left on device"
    assert "PRIVATE" not in json.dumps(error) and "/private" not in json.dumps(error)


def test_concurrent_identical_plan_requests_commit_once(scenario):
    from concurrent.futures import ThreadPoolExecutor
    arguments = dict(plan=automatic_plan(), request_key="concurrent")
    store, boot, _ = scenario
    def submit(_):
        server = Server(channel_credentials=scenario[2].channel_credentials)
        return server.dispatch("tools/call", dict(name="plan_submit", arguments=copy.deepcopy(arguments)))
    with ThreadPoolExecutor(max_workers=2) as pool:
        replies = list(pool.map(submit, range(2)))
    assert all(not reply["isError"] for reply in replies)
    assert body(replies[0]) == body(replies[1])
    assert_committed_once(store, boot)
