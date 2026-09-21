"""PermissionBridge + BindingRegistry: correlation, invalidation, and
schema-valid outcome shaping (T014). Unit-level (no subprocess) -- see
``test_engine_resource_dispatch.py`` for the end-to-end wire-level cases."""
from __future__ import annotations

import threading
import sqlite3
from contextlib import contextmanager

import pytest

from cortex_runtime.engine.correlation import CorrelationTables
from cortex_runtime.engine.permissions import (
    DuplicateEnginePermissionRequestId,
    PermissionBridge,
    requires_confirmation,
)
from cortex_runtime.engine.resource_binding import BindingRegistry, ResourceBinding, ResourceBindingError


def _binding(tmp_path, **overrides):
    fields = dict(
        engine_instance_id="eirt_a", engine_session_id="es_a", context_generation_id="gen_1",
        assignment_id="asg_1", attempt_id="att_1", workspace_binding_id="wb_1",
        workspace_root=str(tmp_path), ownership_epoch="epoch_1", role="worker",
    )
    fields.update(overrides)
    return ResourceBinding(**fields)


def _bridge(*, host_permission_requester=None, correlation=None, bindings=None):
    return PermissionBridge(
        correlation=correlation or CorrelationTables(), bindings=bindings or BindingRegistry(),
        host_permission_requester=host_permission_requester, call_timeout=1.0,
    )


PARAMS = dict(
    toolCall=dict(toolCallId="tc-1"),
    options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")],
)


def test_binding_requires_every_field_nonempty(tmp_path):
    with pytest.raises(ResourceBindingError):
        ResourceBinding(
            engine_instance_id="", engine_session_id="es_a", context_generation_id="gen_1",
            assignment_id="asg_1", attempt_id="att_1", workspace_binding_id="wb_1",
            workspace_root=str(tmp_path), ownership_epoch="epoch_1", role="worker",
        )


def test_binding_requires_resolved_absolute_workspace_root(tmp_path):
    with pytest.raises(ResourceBindingError):
        _binding(tmp_path, workspace_root=str(tmp_path) + "/../" + tmp_path.name)


def test_no_binding_for_session_is_cancelled(tmp_path):
    bindings = BindingRegistry()
    bridge = _bridge(bindings=bindings)
    result = bridge.handle(engine_session_id="es_missing", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_no_host_requester_is_cancelled(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    bridge = _bridge(bindings=bindings, host_permission_requester=None)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


@pytest.mark.parametrize("command", (
    "python3 counter.py 4",
    "python3 -m pytest -q",
    'python3 counter.py 4; echo "exit:$?"; python3 counter.py 4 --step 5; echo "exit:$?"; python3 -m pytest -q 2>&1 | tail -20',
))
def test_worker_claude_other_read_test_commands_auto_allow_without_host(tmp_path, command):
    calls = []
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    bridge = _bridge(bindings=bindings, host_permission_requester=lambda *args: calls.append(args))
    params = {
        "toolCall": {"toolCallId": "tc-read-test", "kind": "other", "title": "Bash",
                     "rawInput": {"command": command}},
        "options": PARAMS["options"],
    }
    assert bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params) == {
        "outcome": {"outcome": "selected", "optionId": "allow"}
    }
    assert calls == []


def test_worker_native_read_of_private_store_is_refused_before_host(tmp_path):
    """A declared Claude/ACP Read path must use mediated MCP store reads."""
    private = tmp_path / ".cortex" / "t_task" / "pipeline.md"
    private.parent.mkdir(parents=True)
    private.write_text("private", encoding="utf-8")
    calls = []
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    bridge = _bridge(
        bindings=bindings,
        host_permission_requester=lambda *args: calls.append(args) or {
            "outcome": {"outcome": "selected", "optionId": "allow"}
        },
    )
    params = {
        "toolCall": {
            "toolCallId": "tc-private-read", "kind": "read", "title": "Read pipeline",
            "locations": [{"path": str(private)}],
        },
        "options": PARAMS["options"],
    }
    assert bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params) == {
        "outcome": {"outcome": "selected", "optionId": "deny"}
    }
    assert calls == []


def test_worker_private_store_read_through_symlink_and_cat_is_refused(tmp_path):
    """The declared symlink/``cat`` vectors are denied before auto-allow."""
    private = tmp_path / ".cortex" / "t_task" / "pipeline.md"
    private.parent.mkdir(parents=True)
    private.write_text("private", encoding="utf-8")
    link = tmp_path / "pipeline-link.md"
    link.symlink_to(private)
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    calls = []
    bridge = _bridge(
        bindings=bindings,
        host_permission_requester=lambda *args: calls.append(args),
    )
    params = {
        "toolCall": {
            "toolCallId": "tc-private-cat", "kind": "other", "title": "Bash",
            "rawInput": {"command": "cat pipeline-link.md"},
        },
        "options": PARAMS["options"],
    }
    assert bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=2, params=params) == {
        "outcome": {"outcome": "selected", "optionId": "deny"}
    }
    assert calls == []


@pytest.mark.parametrize("command", (
    "rm -- sentinel.txt", "python3 -c 'open(\"x\", \"w\").write(\"x\")'",
    "python3 ../evil.py 4", "python3 -m pytest ../test_counter.py",
    "python3 -m pytest --rootdir=../outside", "python3 -m pytest -o addopts=--do-something",
    "python3 -m pytest -p evil_plugin", "pytest --confcutdir=../outside",
    "python3 counter.py 4; python3 -c 'open(\"x\", \"w\").write(\"x\")'",
    "python3 counter.py 4 > output.txt",
))
def test_worker_claude_other_non_deletion_commands_are_auto_approved(tmp_path, command):
    calls = []
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    bridge = _bridge(
        bindings=bindings,
        host_permission_requester=lambda *args: calls.append(args) or {
            "outcome": {"outcome": "selected", "optionId": "deny"}
        },
    )
    params = {"toolCall": {"toolCallId": "tc-ambiguous", "kind": "other", "title": "Bash",
                            "rawInput": {"command": command}}, "options": PARAMS["options"]}
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    if command in {
        "rm -- sentinel.txt",
        "python3 -c 'open(\"x\", \"w\").write(\"x\")'",
        "python3 counter.py 4; python3 -c 'open(\"x\", \"w\").write(\"x\")'",
    }:
        assert result == {"outcome": {"outcome": "selected", "optionId": "deny"}}
        assert len(calls) == 1
    else:
        assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}
        assert calls == []


def test_host_selects_offered_option_is_relayed(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "selected", "optionId": "allow"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}


def test_host_selects_unknown_option_id_is_cancelled(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "selected", "optionId": "not-offered"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_duplicate_wire_request_id_errors(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    release_event = threading.Event()

    def slow_requester(engine_session_id, params, timeout):
        release_event.wait(2.0)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=slow_requester)
    results = []

    def first():
        results.append(bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=42, params=PARAMS))

    worker = threading.Thread(target=first)
    worker.start()
    import time
    time.sleep(0.05)
    with pytest.raises(DuplicateEnginePermissionRequestId):
        bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=42, params=PARAMS)
    release_event.set()
    worker.join(timeout=2.0)
    assert results == [{"outcome": {"outcome": "selected", "optionId": "allow"}}]


def test_session_invalidated_while_host_deciding_never_relays_selected(tmp_path):
    """The invalidation happens *during* the host round-trip (simulated by
    the host_permission_requester callback itself invalidating the
    session) -- the bridge must re-check binding currency after the host
    replies, not only before calling it, and this is exactly the
    'late_permission_response' path."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))

    def requester(engine_session_id, params, timeout):
        bindings.invalidate_session(engine_session_id, "session_replaced")
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_epoch_change_invalidates_pending_permission(tmp_path):
    bindings = BindingRegistry()
    binding = _binding(tmp_path)
    bindings.bind(binding)
    bindings.invalidate_epoch(binding.workspace_binding_id, "epoch_2")
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "selected", "optionId": "allow"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_generation_change_invalidates_a_reply_already_in_flight(tmp_path):
    """A request whose host round-trip was already outstanding when the
    generation bumped (it captured its ``ResourceBinding`` before the bump)
    is correctly refused as stale. See
    ``test_generation_bump_after_the_fact_does_not_block_a_new_request``
    below for the flip side this same T035 EXT-01 #3 fix guarantees: a
    request made *after* the bump is not also refused."""
    bindings = BindingRegistry()
    binding = _binding(tmp_path)
    bindings.bind(binding)

    def requester(engine_session_id, params, timeout):
        bindings.invalidate_generation(binding.engine_session_id, "gen_2")
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_generation_bump_after_the_fact_does_not_block_a_new_request(tmp_path):
    """T035 EXT-01 #3: invalidating the generation before a *new* request
    is even made must not itself refuse that new request -- only a reply
    to a request already in flight before the bump (the case above) is
    stale."""
    bindings = BindingRegistry()
    binding = _binding(tmp_path)
    bindings.bind(binding)
    bindings.invalidate_generation(binding.engine_session_id, "gen_2")
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "selected", "optionId": "allow"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}


def test_action_supersession_invalidates_only_that_action(tmp_path):
    correlation = CorrelationTables()
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    # Pre-allocate the portable action ref exactly as the bridge would.
    action_ref = correlation.portable_tool_call_ref("w1", "tc-1")
    bindings.invalidate_action("es_a", action_ref)
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "selected", "optionId": "allow"}}
    bridge = _bridge(bindings=bindings, correlation=correlation, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}
    # A *different* tool call (never superseded) on the same session/binding
    # is unaffected.
    other_params = dict(PARAMS, toolCall=dict(toolCallId="tc-2"))
    result2 = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=2, params=other_params)
    assert result2 == {"outcome": {"outcome": "selected", "optionId": "allow"}}


def test_host_cancelled_outcome_is_passed_through(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    requester = lambda engine_session_id, params, timeout: {"outcome": {"outcome": "cancelled"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_host_timeout_or_error_is_cancelled(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))

    def raising_requester(engine_session_id, params, timeout):
        raise TimeoutError("host_timeout")

    bridge = _bridge(bindings=bindings, host_permission_requester=raising_requester)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=PARAMS)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_worker_a_binding_never_visible_to_worker_b_engine_session(tmp_path):
    (tmp_path / "a").mkdir()
    (tmp_path / "b").mkdir()
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path / "a", engine_session_id="es_a", workspace_binding_id="wb_a"))
    bindings.bind(_binding(tmp_path / "b", engine_session_id="es_b", workspace_binding_id="wb_b"))
    binding_a = bindings.get("es_a")
    binding_b = bindings.get("es_b")
    assert binding_a.workspace_root != binding_b.workspace_root
    # Invalidating B's epoch never affects A's snapshot.
    bindings.invalidate_epoch("wb_b", "epoch_2")
    snapshot_a = bindings.snapshot(binding_a)
    assert binding_a.is_current(snapshot_a)


def test_allow_always_never_authorizes_a_deletion(tmp_path):
    """An engine-offered persistent choice cannot stand in for one explicit
    action approval, even when the host selects it twice."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    calls = []

    def requester(engine_session_id, params, timeout):
        calls.append(params)
        return {"outcome": {"outcome": "selected", "optionId": "allow_always"}}

    params_with_always = dict(PARAMS, options=[dict(optionId="allow_always", kind="allow_always")])
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    first = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params_with_always)
    second = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=2, params=params_with_always)
    assert first == {"outcome": {"outcome": "cancelled"}}
    assert second == {"outcome": {"outcome": "cancelled"}}
    # The host requester was consulted for *both* -- nothing was decided
    # locally from a remembered "always" choice.
    assert len(calls) == 2


# -- T035: worker full-access-except-deletion auto-approval -----------------


def _params(tool_call, options=None):
    return dict(
        toolCall=tool_call,
        options=options or [dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")],
    )


def test_worker_ordinary_operation_is_auto_approved_without_host(tmp_path):
    """A read/edit/search/etc. tool call for a worker binding is answered
    locally -- the host requester is never called at all."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []

    def requester(engine_session_id, params, timeout):
        calls.append(params)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    params = _params(dict(toolCallId="tc-1", kind="edit", rawInput=dict(command="apply_patch")))
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert calls == []


def test_worker_non_deletion_uses_claude_allow_always_when_it_is_the_only_allow_option(tmp_path):
    """Claude ACP may offer only its persistent-shaped allow option.

    The runtime's policy is still per-request: it does not store or reuse a
    decision.  It must nevertheless answer this harmless worker Bash call
    locally, instead of leaving Paseo waiting for a user permission card.
    """
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []
    bridge = _bridge(
        bindings=bindings,
        host_permission_requester=lambda *args: calls.append(args),
    )
    params = _params(
        dict(
            toolCallId="tc-claude-bash-http-server",
            kind="other",
            title="Bash",
            rawInput={"command": "python3 -m http.server 8000 --directory public"},
        ),
        options=[dict(optionId="allow-always", kind="allow_always"),
                 dict(optionId="deny", kind="reject_once")],
    )
    assert bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=params,
    ) == {"outcome": {"outcome": "selected", "optionId": "allow-always"}}
    assert calls == []


@pytest.mark.parametrize("command", [
    "echo generated > result.txt",
    "cat source.txt > generated.txt",
    "echo generated > nested/result.txt",
])
def test_worker_in_scope_redirect_is_auto_approved_without_host(tmp_path, command):
    (tmp_path / "nested").mkdir()
    calls = []
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    bridge = _bridge(bindings=bindings, host_permission_requester=lambda *args: calls.append(args))
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=_params(dict(toolCallId="tc-redirect", kind="other", title="Bash", rawInput={"command": command})),
    )
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert calls == []


@pytest.mark.parametrize("command", [
    "echo generated > /tmp/outside.txt",
    "echo generated > .cortex/private.md",
])
def test_worker_redirect_outside_workspace_or_private_store_is_host_confirmed(tmp_path, command):
    calls = []
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    bridge = _bridge(
        bindings=bindings,
        host_permission_requester=lambda *args: calls.append(args) or {"outcome": {"outcome": "cancelled"}},
    )
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=_params(dict(toolCallId="tc-redirect-guard", kind="other", title="Bash", rawInput={"command": command})),
    )
    if ".cortex" in command:
        assert result == {"outcome": {"outcome": "selected", "optionId": "deny"}}
        assert calls == []
    else:
        assert result == {"outcome": {"outcome": "cancelled"}}
        assert len(calls) == 1


def test_worker_deletion_tool_kind_requires_a_fresh_host_decision_each_time(tmp_path):
    """A rejected delete stays rejected, and a retry asks the host again."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []

    def requester(engine_session_id, params, timeout):
        calls.append(params)
        return {"outcome": {"outcome": "selected", "optionId": "deny"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    params = _params(dict(toolCallId="tc-1", kind="delete", title="Delete scratch.txt"))
    first = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    second = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=2, params=params)
    assert first == second == {"outcome": {"outcome": "selected", "optionId": "deny"}}
    assert len(calls) == 2
    assert all(call["toolCall"]["title"] == "Delete request" for call in calls)


def test_worker_rm_shell_command_is_not_auto_approved(tmp_path):
    """A shell 'execute' tool call whose command is 'rm ...' is recognized
    as a deletion even though toolCall.kind itself is only 'execute'."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []

    def requester(engine_session_id, params, timeout):
        calls.append(params)
        return {"outcome": {"outcome": "cancelled"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    params = _params(dict(toolCallId="tc-1", kind="execute", rawInput=dict(command="rm -- /tmp/disposable-sentinel.txt")))
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    assert result == {"outcome": {"outcome": "cancelled"}}
    assert len(calls) == 1


def test_worker_read_only_python_heredoc_is_auto_approved_without_host(tmp_path):
    """Claude's native Bash shape can run a pure in-memory contrast check."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []

    def requester(*args):
        calls.append(args)
        return {"outcome": {"outcome": "cancelled"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    command = """python3 -B <<'EOF'
def luminance(rgb):
    return sum(rgb) / len(rgb)

text = (255, 255, 255)
background = (0, 0, 0)
print(round((luminance(text) + 0.05) / (luminance(background) + 0.05), 2))
EOF"""
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=_params(dict(toolCallId="tc-1", kind="other", rawInput={"command": command})),
    )
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert calls == []


def test_worker_native_claude_inline_python_stdin_shape_is_auto_approved(tmp_path):
    """The native Claude ACP Bash payload uses ``python3 -B -`` on stdin."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path))
    calls = []
    bridge = _bridge(bindings=bindings, host_permission_requester=lambda *args: calls.append(args))
    command = """PYTHONDONTWRITEBYTECODE=1 python3 -B - <<'PY'
def luminance(rgb):
    return sum(rgb) / len(rgb)

text = (255, 255, 255)
background = (0, 0, 0)
print(round((luminance(text) + 0.05) / (luminance(background) + 0.05), 2))
PY"""
    params = _params(dict(
        toolCallId="tc-inline-contrast", kind="other", title="Bash",
        rawInput={"command": command},
    ))
    assert bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params,
    ) == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert calls == []


def test_worker_native_bash_other_with_imported_read_only_calculation_is_auto_approved(tmp_path):
    """The policy must not require an AST allowlist for opaque Bash.

    This is the captured failure shape: Claude labels Bash as ``other`` and
    sends a Python heredoc.  The import is harmless for this policy boundary,
    while the command remains subject to the destructive-shell scan.
    """
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []
    bridge = _bridge(bindings=bindings, host_permission_requester=lambda *args: calls.append(args))
    command = """python3 -B <<'PY'
from math import pow
def luminance(rgb):
    return sum(rgb) / len(rgb)
print(pow(luminance((255, 255, 255)) + 0.05, 1))
PY"""
    params = _params(dict(
        toolCallId="tc-live-contrast", kind="other", title="Bash",
        rawInput={"command": command},
    ))
    assert bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params,
    ) == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert calls == []


@pytest.mark.parametrize("command", [
    "rm -- sentinel.txt",
    "python3 -B - <<'PY'\nimport os\nos.remove('sentinel.txt')\nPY",
    "python3 -B - <<'PY'\nimport shutil\nshutil.rmtree('build')\nPY",
    "python3 -B - <<'PY'\nfrom pathlib import Path\nPath('sentinel.txt').unlink()\nPY",
    "python3 -B - <<'PY'\nimport subprocess\nsubprocess.run(['rm', 'sentinel.txt'])\nPY",
    "python3 -B - <<'PY'\nopen('sentinel.txt', 'w')\nPY",
    "find . -name '*.tmp' -delete",
    "mv sentinel.txt old.txt",
    "echo gone > sentinel.txt",
])
def test_worker_native_bash_other_destructive_command_is_host_confirmed(tmp_path, command):
    """Known shell deletion/truncation forms still reach the host.

    Directly spelled deletion APIs are observable and must reach the host.
    """
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []
    bridge = _bridge(
        bindings=bindings,
        host_permission_requester=lambda *args: calls.append(args) or {"outcome": {"outcome": "cancelled"}},
    )
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=_params(dict(toolCallId="tc-destructive", kind="other", title="Bash", rawInput={"command": command})),
    )
    if command == "echo gone > sentinel.txt":
        assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}
        assert calls == []
    else:
        assert result == {"outcome": {"outcome": "cancelled"}}
        assert len(calls) == 1


def test_worker_native_bash_dynamic_delete_remains_opaque_limitation(tmp_path):
    """Computed API names remain outside lexical prevention guarantees."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []
    bridge = _bridge(bindings=bindings, host_permission_requester=lambda *args: calls.append(args))
    command = "python3 -B - <<'PY'\nimport os\ngetattr(os, 'remove')('sentinel.txt')\nPY"
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=_params(dict(toolCallId="tc-dynamic", kind="other", title="Bash", rawInput={"command": command})),
    )
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert calls == []


@pytest.mark.parametrize("body", [
    "import os\nprint(os.getcwd())",
    "open('out.txt', 'w').write('changed')",
    "__import__('os').system('rm -f sentinel')",
])
def test_worker_python_heredoc_with_opaque_effect_is_auto_approved_with_boundary(tmp_path, body):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []

    def requester(*args):
        calls.append(args)
        return {"outcome": {"outcome": "cancelled"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    command = f"python3 -B <<'EOF'\n{body}\nEOF"
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=_params(dict(toolCallId="tc-1", kind="other", rawInput={"command": command})),
    )
    if "open('out.txt', 'w')" in body or "rm -f" in body:
        assert result == {"outcome": {"outcome": "cancelled"}}
        assert len(calls) == 1
    else:
        assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}
        assert calls == []

@pytest.mark.parametrize("command", ("rm -- disposable.txt", "/bin/rm -- disposable.txt", "sudo /usr/bin/rm -- disposable.txt"))
def test_rm_spellings_never_take_the_ordinary_auto_allow_path(tmp_path, command):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []
    def requester(engine_session_id, params, timeout):
        calls.append(params)
        return {"outcome": {"outcome": "cancelled"}}
    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=_params(dict(toolCallId="tc-1", kind="execute", rawInput=dict(command=command))),
    )
    assert result == {"outcome": {"outcome": "cancelled"}}
    assert len(calls) == 1


def test_worker_unclassifiable_tool_call_asks_rather_than_allows(tmp_path):
    """No toolCall.kind at all -- the classifier refuses to guess and this
    still reaches the host instead of being silently auto-approved."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    calls = []

    def requester(engine_session_id, params, timeout):
        calls.append(params)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    params = _params(dict(toolCallId="tc-1"))  # kind omitted entirely
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert len(calls) == 1


def test_coordinator_binding_never_reaches_the_host(tmp_path):
    """T035 FINDING-12 closure: a coordinator binding's permission decision
    is always made locally, in either direction -- never relayed to a host,
    unlike the pre-FINDING-12 behaviour this test used to assert."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    calls = []

    def requester(engine_session_id, params, timeout):
        calls.append(params)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    params = _params(dict(toolCallId="tc-1", kind="read"))
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert calls == []


def test_coordinator_read_request_is_auto_approved_without_host(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    bridge = _bridge(bindings=bindings, host_permission_requester=None)
    for kind in ("read", "search", "fetch", "think", "switch_mode"):
        params = _params(dict(toolCallId=f"tc-{kind}", kind=kind))
        result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=hash(kind) % 1000, params=params)
        assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}


def test_coordinator_non_destructive_execute_is_auto_approved_without_host(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    bridge = _bridge(bindings=bindings, host_permission_requester=None)
    params = _params(dict(toolCallId="tc-1", kind="execute", rawInput=dict(command="git log -n 5")))
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}


def test_coordinator_edit_request_is_refused_locally(tmp_path):
    """Coordinator source edits never reach the host permission requester."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    calls = []

    def requester(engine_session_id, params, timeout):
        calls.append(params)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = _bridge(bindings=bindings, host_permission_requester=requester)
    params = _params(dict(toolCallId="tc-1", kind="edit", rawInput=dict(command="apply_patch")))
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    assert result == {"outcome": {"outcome": "selected", "optionId": "deny"}}
    assert calls == []


def test_coordinator_refusal_user_denial_is_forwarded(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    bridge = _bridge(
        bindings=bindings,
        host_permission_requester=lambda *_: {"outcome": {"outcome": "selected", "optionId": "deny"}},
    )
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=_params(dict(toolCallId="tc-1", kind="edit")),
    )
    assert result == {"outcome": {"outcome": "selected", "optionId": "deny"}}


def test_coordinator_refusal_host_timeout_falls_back_to_reject(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    bridge = _bridge(bindings=bindings, host_permission_requester=lambda *_: (_ for _ in ()).throw(TimeoutError()))
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
        params=_params(dict(toolCallId="tc-1", kind="edit")),
    )
    assert result == {"outcome": {"outcome": "selected", "optionId": "deny"}}


def test_coordinator_delete_and_destructive_execute_are_refused(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    bridge = _bridge(bindings=bindings, host_permission_requester=None)
    for tool_call in (
        dict(toolCallId="tc-1", kind="delete"),
        dict(toolCallId="tc-2", kind="move"),
        dict(toolCallId="tc-3", kind="execute", rawInput=dict(command="rm -rf build/")),
    ):
        params = _params(tool_call)
        result = bridge.handle(
            engine_session_id="es_a", wire_session_id="w1", wire_request_id=hash(tool_call["toolCallId"]), params=params,
        )
        assert result == {"outcome": {"outcome": "selected", "optionId": "deny"}}


def test_coordinator_unclassifiable_request_is_refused_not_allowed(tmp_path):
    """The worker/coordinator asymmetry: an unrecognized kind asks the user
    for a worker (see test_worker_unclassifiable_tool_call_asks_rather_than_allows)
    but is refused automatically for a coordinator when no host requester is
    available; a connected user may review an otherwise actionable mutation."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    bridge = _bridge(bindings=bindings, host_permission_requester=None)
    params = _params(dict(toolCallId="tc-1"))  # kind omitted entirely
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    assert result == {"outcome": {"outcome": "selected", "optionId": "deny"}}


def test_coordinator_refusal_falls_back_to_cancelled_without_reject_option(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    bridge = _bridge(bindings=bindings, host_permission_requester=None)
    params = dict(toolCall=dict(toolCallId="tc-1", kind="edit"), options=[dict(optionId="allow", kind="allow_once")])
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    assert result == {"outcome": {"outcome": "cancelled"}}


def test_coordinator_refusal_notifier_is_called_with_the_reason(tmp_path):
    from cortex_runtime.engine.permissions import COORDINATOR_EDIT_REFUSAL_REASON

    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    notified = []
    bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings, host_permission_requester=None,
        coordinator_refusal_notifier=lambda session_id, reason: notified.append((session_id, reason)),
    )
    params = _params(dict(toolCallId="tc-1", kind="edit"))
    bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    assert notified == [("es_a", COORDINATOR_EDIT_REFUSAL_REASON)]


def test_coordinator_refusal_notifier_failure_never_breaks_the_refusal(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))

    def raising_notifier(session_id, reason):
        raise RuntimeError("boom")

    bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings, host_permission_requester=None,
        coordinator_refusal_notifier=raising_notifier,
    )
    params = _params(dict(toolCallId="tc-1", kind="edit"))
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1, params=params)
    assert result == {"outcome": {"outcome": "selected", "optionId": "deny"}}


@pytest.mark.parametrize(
    "tool_call,expected",
    [
        (dict(kind="delete"), True),
        (dict(kind="move"), True),
        (dict(kind="read"), False),
        (dict(kind="edit"), False),
        (dict(kind="search"), False),
        (dict(kind="fetch"), False),
        (dict(kind="think"), False),
        (dict(kind="switch_mode"), False),
        (dict(kind="other"), True),
        (dict(), True),
        (dict(kind="execute", rawInput=dict(command="pytest -q")), False),
        (dict(kind="execute", rawInput=dict(command="rm -rf /tmp/x")), True),
        (dict(kind="execute", rawInput=dict(command="rmdir empty/")), True),
        (dict(kind="execute", rawInput=dict(command="git clean -fd")), True),
        (dict(kind="execute", rawInput=dict(command="find . -name '*.tmp' -delete")), True),
        (dict(kind="execute", rawInput=dict(command="mv a.txt b.txt")), True),
        (dict(kind="execute", rawInput=dict(command="echo hi > out.txt")), True),
        (dict(kind="execute", rawInput=dict(command="echo hi >> out.txt")), False),
        (dict(kind="execute", title="Delete the scratch file"), True),
    ],
)
def test_requires_confirmation_classification(tool_call, expected):
    params = _params(dict(toolCallId="tc-1", **tool_call))
    assert requires_confirmation(params) is expected


def test_cortexs_own_tools_are_never_a_project_mutation():
    """Live, on claude-agent-acp: the coordinator's own `delegate` and
    `set_governance` were refused, it could do nothing at all, and it
    ended its turn asking the user to "approve tool access".

    An MCP call carries no ACP kind of its own -- it is `"other"` -- and an
    unrecognized kind is refused for a coordinator by design. That rule is
    right for an unknown tool and wrong for the runtime's own control
    surface: a coordinator using the runtime it is a coordinator *of* is
    not editing the project. Codex never surfaced this because it does not
    route MCP calls through the permission prompt at all.
    """
    from cortex_runtime.engine.permissions import (
        is_coordinator_mutating,
        is_cortex_own_tool,
        requires_confirmation,
    )

    for title in ("mcp__cortex__delegate", "mcp.cortex.create_task", "mcp__cortex__publish_result"):
        params = {"toolCall": {"kind": "other", "title": title}}
        assert is_cortex_own_tool(params), title
        assert not is_coordinator_mutating(params), title
        # A worker publishing its result is not held for confirmation
        # either, for the same reason.
        assert not requires_confirmation(params), title

    # Another server's tool is still unclassifiable, and still refused.
    other = {"toolCall": {"kind": "other", "title": "mcp__something__else"}}
    assert not is_cortex_own_tool(other)
    assert is_coordinator_mutating(other)

    # And a real edit is still a mutation, whoever asks.
    edit = {"toolCall": {"kind": "edit", "title": "Edit calc.py"}}
    assert not is_cortex_own_tool(edit)
    assert is_coordinator_mutating(edit)


def test_coordinator_accepts_claude_other_read_only_shell_request(tmp_path):
    """Claude ACP may label Bash as ``other`` instead of ``execute``.

    The exact live request that previously generated a false
    ``coordinator_mutation_refused`` event is observational and must be
    allowed locally.  The same fallback remains fail-closed for writes,
    shell composition, and unknown commands.
    """
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    read = {
        "toolCall": {
            "kind": "other",
            "title": "Bash",
            "rawInput": {"command": "cat readme.txt && echo --- && git status --short"},
        }
    }
    (tmp_path / "readme.txt").write_text("ok\n")
    assert not is_coordinator_mutating(read, workspace_root=str(tmp_path))


def test_coordinator_accepts_compound_status_and_focus_check(tmp_path):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    command = (
        "git status --porcelain README.md tests/ && "
        "python3 -m unittest tests.test_focus && "
        "python3 tests/check_focus_render.py --output-dir /tmp/coord-focus-render"
    )
    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "bash /home/igovet/.codex/skills/playwright/scripts/playwright_cli.sh --help",
    "cat ~/.codex/skills/playwright/scripts/playwright_cli.sh",
    "cat $HOME/.claude/skills/browser/SKILL.md",
])
def test_coordinator_engine_private_skill_access_is_hard_refused(tmp_path, command):
    """Native engine skill trees never become a host permission question."""
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "python3 -B -m pytest -q tests/test_page.py",
    "python3 -B -m unittest -v tests.test_page",
    "python3 tests/check_focus_render.py --output-dir /tmp/coord-focus-render",
    "npm test",
])
def test_coordinator_project_checks_are_auto_approved_when_safe(tmp_path, command):
    """Safe project verification is ordinary coordinator inspection.

    Destructive redirects, private paths, and deletion commands remain
    review-gated by the shared classifier; a test command itself is not a
    reason to manufacture a user permission prompt.
    """
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "python3 -c 'from pathlib import Path; assert \"focus-visible\" in Path(\"styles.css\").read_text()'",
    "python3 - <<'PY'\nfrom pathlib import Path\ncss = Path('styles.css').read_text()\nassert 'contrast' in css\nPY",
    "grep -n 'focus-visible' styles.css",
    "rg -n 'href=|aria-label|button' index.html",
])
def test_coordinator_inline_ui_checks_are_auto_approved_when_safe(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "grep -n 'worker' README.md",
    "rg -n 'Northstar' index.html",
    "python3 -c 'print(2 + 2)'",
])
def test_coordinator_public_inspection_remains_allowed(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


def test_coordinator_private_refusal_never_reaches_host_but_safe_check_runs_locally(tmp_path):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    host_calls = []
    notices = []
    bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings,
        host_permission_requester=lambda *args: host_calls.append(args),
        coordinator_refusal_notifier=lambda session, reason: notices.append((session, reason)),
    )
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="wire", wire_request_id=1,
        params={
            "toolCall": {"toolCallId": "tc-1", "kind": "other", "title": "Bash",
                         "rawInput": {"command": "bash /home/igovet/.codex/skills/playwright/scripts/playwright_cli.sh --help"}},
            "options": PARAMS["options"],
        },
    )
    assert result == {"outcome": {"outcome": "selected", "optionId": "deny"}}
    result = bridge.handle(
        engine_session_id="es_a", wire_session_id="wire", wire_request_id=2,
        params={
            "toolCall": {"toolCallId": "tc-2", "kind": "other", "title": "Bash",
                         "rawInput": {"command": "python3 -B -m unittest -v tests.test_page"}},
            "options": PARAMS["options"],
        },
    )
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert host_calls == []
    assert len(notices) == 1
    assert "delegate" in notices[0][1] and "worker" in notices[0][1]


def test_claude_bash_safe_checks_auto_allow_rm_is_reviewed_and_outside_is_blocked(tmp_path):
    """Exercise the exact Claude ``kind=other``/Bash wire shape.

    Safe project checks are answered locally, so Claude can execute them
    without producing a misleading user permission card.  A deletion still
    reaches the host for one-shot review, while an outside read is denied
    when no host reviewer is connected.
    """
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    host_calls = []

    def requester(*args):
        host_calls.append(args)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings,
        host_permission_requester=requester,
    )
    safe = {
        "toolCall": {"toolCallId": "safe", "kind": "other", "title": "Bash",
                     "rawInput": {"command": "python3 -m unittest -v tests.test_page"}},
        "options": PARAMS["options"],
    }
    assert bridge.handle(engine_session_id="es_a", wire_session_id="wire", wire_request_id=1, params=safe) == {
        "outcome": {"outcome": "selected", "optionId": "allow"}
    }
    assert host_calls == []

    destructive = {
        "toolCall": {"toolCallId": "rm", "kind": "other", "title": "Bash",
                     "rawInput": {"command": "rm -- fixture.txt"}},
        "options": PARAMS["options"],
    }
    assert bridge.handle(engine_session_id="es_a", wire_session_id="wire", wire_request_id=2, params=destructive) == {
        "outcome": {"outcome": "selected", "optionId": "allow"}
    }
    assert len(host_calls) == 1

    outside_calls = []
    outside_bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings,
        host_permission_requester=lambda *args: outside_calls.append(args),
    )
    outside = {
        "toolCall": {"toolCallId": "outside", "kind": "other", "title": "Bash",
                     "rawInput": {"command": "cat /etc/passwd"}},
        "options": PARAMS["options"],
    }
    assert outside_bridge.handle(engine_session_id="es_a", wire_session_id="wire", wire_request_id=3, params=outside) == {
        "outcome": {"outcome": "selected", "optionId": "deny"}
    }
    assert outside_calls == []


def test_worker_outside_read_is_user_confirmed_instead_of_silently_rejected(tmp_path):
    """An outside worker read must produce a real user decision boundary."""
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="worker"))
    host_calls = []

    def requester(*args):
        host_calls.append(args)
        return {"outcome": {"outcome": "selected", "optionId": "allow"}}

    bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings,
        host_permission_requester=requester,
    )
    params = {
        "toolCall": {
            "toolCallId": "tc-find-root",
            "kind": "other",
            "title": "Bash",
            "rawInput": {"command": "find / -maxdepth 2 -type f"},
        },
        "options": PARAMS["options"],
    }
    assert bridge.handle(
        engine_session_id="es_a", wire_session_id="wire", wire_request_id=1,
        params=params,
    ) == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert len(host_calls) == 1


def test_coordinator_accepts_exact_compound_status_test_wire_shape(tmp_path):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    command = (
        "git status --porcelain README.md tests/ && git diff --stat && "
        "python3 -m unittest discover -s tests -p 'test_*.py' 2>&1; "
        "echo \"exit=$?\"; python3 tests/check_focus_render.py "
        "--output-dir /tmp/coord-focus-render 2>&1; echo \"exit=$?\""
    )
    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


def test_coordinator_other_bash_rm_still_requires_review(tmp_path):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash",
                            "rawInput": {"command": "git status --porcelain && rm -- sentinel.txt"}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "/bin/bash -lc 'git status --short'",
    "bash -lc 'git status --porcelain README.md'",
])
def test_coordinator_shell_wrapper_read_check_is_auto_approved(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


def test_coordinator_shell_wrapper_project_check_is_auto_approved(tmp_path):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash",
                            "rawInput": {"command": "/bin/sh -c 'python3 -m unittest discover -s tests'"}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


def test_coordinator_shell_wrapper_rm_still_requires_review(tmp_path):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash",
                            "rawInput": {"command": "/bin/bash -lc 'rm -- fixture.txt'"}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))


def test_coordinator_bound_workspace_pipeline_is_auto_approved(tmp_path):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    command = f"cd {tmp_path} && python3 -m pytest tests/test_page.py tests/test_focus.py -v 2>&1 | tail -40"
    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("kind", ["other", "execute"])
def test_coordinator_execute_or_other_pytest_pipeline_fd_redirect_is_auto_approved(tmp_path, kind):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    command = "python3 -m pytest tests/test_page.py tests/test_focus.py -v 2>&1 | tail -20"
    params = {"toolCall": {"kind": kind, "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "python3 -m pytest tests/test_page.py 2>&1 | rm -- fixture.txt",
    "python3 -m pytest tests/test_page.py > result.txt",
    "python3 -m pytest tests/test_page.py 2>&1 | tee /tmp/outside.log",
    "python3 -m pytest .cortex/private.py 2>&1 | tail -20",
])
def test_coordinator_execute_pipeline_destructive_or_unbound_still_requires_review(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "execute", "title": "Bash", "rawInput": {"command": command}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("kind", ["other", "execute"])
def test_coordinator_benign_loop_pipeline_is_auto_approved(tmp_path, kind):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    command = 'ls tests && echo --- && for f in tests/*; do echo "== $f =="; cat "$f"; done'
    params = {"toolCall": {"kind": kind, "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "ls tests && for f in tests/*; do rm -- \"$f\"; done",
    "ls tests && find tests -type f -delete",
    "ls tests && echo x > tests/out.txt",
    "ls .cortex && cat .cortex/private.md",
    "ls /etc && cat /etc/passwd",
])
def test_coordinator_benign_loop_deletion_private_or_outside_still_requires_review(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("tool_call", [
    {"kind": "other", "rawInput": {"command": "echo ok", "file_path": "/outside/x"}},
    {"kind": "execute", "rawInput": {"command": "echo ok", "file_path": ".cortex/private.md"}},
    {"kind": "other", "locations": [{"path": "/outside/x"}], "rawInput": {"command": "echo ok"}},
])
def test_coordinator_declared_outside_or_private_path_is_guarded(tmp_path, tool_call):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": dict(tool_call, title="Bash")}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("tool_call", [
    {"kind": "other", "rawInput": {"command": "echo ok", "file_path": "README.md"}},
    {"kind": "execute", "locations": [{"path": "tests/test_page.py"}], "rawInput": {"command": "echo ok"}},
])
def test_coordinator_declared_bound_path_remains_ordinary(tmp_path, tool_call):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": dict(tool_call, title="Bash")}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "cd /var/tmp/other-project && python3 -m pytest -q | tail -40",
    "cd {root} && python3 -m pytest -q | rm -- fixture.txt",
])
def test_coordinator_pipeline_outside_or_rm_still_requires_review(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    command = command.format(root=tmp_path)
    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "rg -n focus index.html styles.css | head -40",
    "find . -type f -name '*.py' | sort | head -40",
    "wc -l README.md | sort -n",
    "sed -n '1,40p' README.md | awk '{print $1}'",
])
def test_coordinator_common_read_only_pipeline_is_auto_approved(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "rg -n focus index.html | tee /tmp/outside.log",
    "python3 -m pytest tests/test_page.py | tee .cortex/private.log",
    "find . -type f -delete",
])
def test_coordinator_common_pipeline_hazards_still_require_review(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "cat README.md",
    "ls -la",
    "sha256sum index.html styles.css",
    "stat README.md",
    "file README.md",
    "du -h README.md",
    "basename README.md",
    "dirname README.md",
    "git rev-parse --show-toplevel",
    "git log --oneline -5",
    "git show --stat HEAD",
])
def test_coordinator_common_bound_read_command_is_auto_approved(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    (tmp_path / "README.md").write_text("read\n")
    (tmp_path / "index.html").write_text("<html>\n")
    (tmp_path / "styles.css").write_text("body{}\n")
    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert not is_coordinator_mutating(params, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "cat /etc/passwd",
    "ls .cortex",
    "sha256sum ../outside.txt",
])
def test_coordinator_bound_read_outside_or_private_requires_review(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))


def test_coordinator_bound_read_symlink_escape_requires_review(tmp_path):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    (tmp_path / "outside-link").symlink_to("/etc/passwd")
    params = {"toolCall": {"kind": "other", "title": "Bash",
                            "rawInput": {"command": "sha256sum outside-link"}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))


def test_coordinator_in_project_edit_is_ordinary_action(tmp_path):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "edit", "rawInput": {"file_path": "src/view.py"}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))
    private = {"toolCall": {"kind": "edit", "rawInput": {"file_path": ".cortex/pipeline.md"}}}
    assert is_coordinator_mutating(private, workspace_root=str(tmp_path))


@pytest.mark.parametrize("command", [
    "git status --porcelain README.md && rm -- sentinel.txt",
    "git status --porcelain .cortex/private.md",
    "python3 -m unittest tests.test_focus > /tmp/coord-output",
    "python3 tests/check_focus_render.py --output-dir /tmp/arbitrary-output",
])
def test_coordinator_compound_check_guards_deletion_private_and_unbounded_output(tmp_path, command):
    from cortex_runtime.engine.permissions import is_coordinator_mutating

    params = {"toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}}
    assert is_coordinator_mutating(params, workspace_root=str(tmp_path))

def test_a_worker_filling_its_own_draft_is_not_a_deletion():
    """A worker writes its report by filling the draft the runtime handed
    it, and a shell heredoc does that with `>`. Treating every `>` as a
    deletion sent that to the user as an unexplained "Bash" prompt
    offering to "allow access to draft-reports/" -- a question about
    Cortex's own bookkeeping that the user has no way to judge.

    Narrow by design: only the redirect is reconsidered, and only when
    every target is inside the store."""
    from cortex_runtime.engine.permissions import requires_confirmation

    def ask(command):
        return requires_confirmation({"toolCall": {"kind": "execute", "rawInput": {"command": command}}})

    assert not ask("python3 - > .cortex/draft-reports/d_1.md")
    # `2>/dev/null` is on the end of half the commands anyone writes, and
    # it made a read-only `find` look like a deletion. Live: a worker
    # stopped to ask the user for permission to locate a file.
    assert not ask('find /p -iname "calc.py" -not -path "*/.cortex/*" 2>/dev/null')
    assert not ask("pytest -q 2>/dev/null")
    assert ask("echo hi > calc.py 2>/dev/null")
    assert ask("rm -rf build 2>/dev/null")
    assert not ask("cat > /p/.cortex/draft-reports/d_1.md <<'EOF'")
    # The user's own files still ask.
    assert ask("echo hi > calc.py")
    assert ask("echo a > .cortex/x.md && echo b > calc.py")
    # And a real deletion asks wherever it points, store included.
    assert ask("rm -rf .cortex/draft-reports")


def test_coordinator_can_fill_only_its_registered_pipeline_draft(tmp_path):
    """Compose the real bridge with the production store-backed authorizer.

    The observed Claude Write declares an absolute rawInput.file_path and
    one matching locations path. No host approval is involved in this case.
    """
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter

    project = tmp_path / "project"
    draft_dir = project / ".cortex" / "pipeline-drafts"
    draft_dir.mkdir(parents=True)
    draft_dir.parent.chmod(0o700)
    draft_dir.chmod(0o700)
    draft = draft_dir / "d_123456789abc.md"
    draft.write_text("Cortex draft ID: `d_123456789abc`\n", encoding="utf-8")
    source = project / "counter.py"
    source.write_text("x = 1\n", encoding="utf-8")
    database = tmp_path / "drafts.sqlite3"
    with sqlite3.connect(database) as db:
        db.executescript(
            "CREATE TABLE tasks(id TEXT, project_root TEXT);"
            "CREATE TABLE thread_bindings(thread_id TEXT, parent_thread_id TEXT, task_id TEXT);"
            "CREATE TABLE drafts(task_id TEXT, owner_thread_id TEXT, path TEXT, kind TEXT, published_report_id TEXT);"
        )
        db.execute("INSERT INTO tasks VALUES (?,?)", ("task-1", str(project)))
        db.execute("INSERT INTO thread_bindings VALUES (?,?,?)", ("coordinator-thread", None, "task-1"))
        db.execute("INSERT INTO drafts VALUES (?,?,?,?,?)", ("task-1", "coordinator-thread", str(draft), "pipeline", None))

    class DraftStore:
        @contextmanager
        def connection(self):
            with sqlite3.connect(database) as db:
                db.row_factory = sqlite3.Row
                yield db

    adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
    adapter._store = DraftStore()
    adapter._lock = threading.Lock()
    adapter._session_meta = {"es_a": {"task_id": "task-1", "attempt_id": "att_1", "recipient": "coordinator"}}
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role="coordinator"))
    host_calls = []
    bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings,
        host_permission_requester=lambda *args: host_calls.append(args) or {
            "outcome": {"outcome": "selected", "optionId": "deny"}
        },
        coordinator_draft_write_authorizer=adapter._coordinator_draft_write_authorizer,
    )

    def write(path, request_id):
        params = {
            "toolCall": {"toolCallId": f"tc-{request_id}", "kind": "edit",
                         "title": f"Write {path.name}",
                         "locations": [{"path": str(path)}],
                         "rawInput": {"file_path": str(path), "content": "replacement"}},
            "options": [{"optionId": "allow", "kind": "allow_once"},
                        {"optionId": "deny", "kind": "reject_once"}],
        }
        return bridge.handle(engine_session_id="es_a", wire_session_id="wire", wire_request_id=request_id, params=params)

    assert write(draft, 1) == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert write(source, 2) == {"outcome": {"outcome": "selected", "optionId": "deny"}}
    with sqlite3.connect(database) as db:
        db.execute("UPDATE drafts SET published_report_id='r_123456789abc' WHERE path=?", (str(draft),))
    assert write(draft, 3) == {"outcome": {"outcome": "selected", "optionId": "deny"}}
    assert len(host_calls) == 0


def test_worker_can_fill_only_its_active_report_draft(tmp_path):
    """A Claude native Write may fill its own Cortex report draft locally.

    This is deliberately narrower than a general ``.cortex`` exception: a
    different draft still goes through the refusal path, and a delete request
    still reaches the host for one-shot confirmation.
    """
    project = tmp_path / "project"
    draft_dir = project / ".cortex" / "draft-reports"
    draft_dir.mkdir(parents=True)
    project.joinpath(".cortex").chmod(0o700)
    draft_dir.chmod(0o700)
    draft = draft_dir / "d_worker.md"
    other = draft_dir / "d_other.md"
    draft.write_text("template", encoding="utf-8")
    other.write_text("template", encoding="utf-8")

    class DraftStore:
        @contextmanager
        def connection(self):
            with sqlite3.connect(":memory:") as db:
                db.row_factory = sqlite3.Row
                db.executescript("""
                    CREATE TABLE tasks(id TEXT PRIMARY KEY, project_root TEXT);
                    CREATE TABLE drafts(id TEXT PRIMARY KEY, task_id TEXT, owner_thread_id TEXT,
                        kind TEXT, path TEXT, published_report_id TEXT);
                    CREATE TABLE trusted_channels(task_id TEXT, principal_thread_id TEXT,
                        role TEXT, assignment_id TEXT, attempt_id TEXT);
                """)
                db.execute("INSERT INTO tasks VALUES (?,?)", ("task-1", str(project)))
                db.execute("INSERT INTO drafts VALUES (?,?,?,?,?,NULL)",
                            ("d_worker", "task-1", "worker-thread", "report", str(draft)))
                db.execute("INSERT INTO drafts VALUES (?,?,?,?,?,NULL)",
                            ("d_other", "task-1", "other-thread", "report", str(other)))
                db.execute("INSERT INTO trusted_channels VALUES (?,?,?,?,?)",
                            ("task-1", "worker-thread", "worker", "asg_1", "att_1"))
                yield db

    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
    adapter._store = DraftStore()
    adapter._lock = threading.Lock()
    adapter._session_meta = {"es_a": {"task_id": "task-1", "attempt_id": "att_1", "recipient": "worker"}}
    bindings = BindingRegistry()
    bindings.bind(_binding(project, workspace_root=str(project), assignment_id="asg_1"))
    host_calls = []
    bridge = PermissionBridge(
        correlation=CorrelationTables(), bindings=bindings,
        host_permission_requester=lambda *args: host_calls.append(args) or {
            "outcome": {"outcome": "selected", "optionId": "deny"}
        },
        worker_draft_write_authorizer=adapter._worker_draft_write_authorizer,
    )

    def request(path, kind="edit", request_id=1):
        return bridge.handle(
            engine_session_id="es_a", wire_session_id="wire", wire_request_id=request_id,
            params={"toolCall": {"toolCallId": f"tc-{request_id}", "kind": kind,
                                  "locations": [{"path": str(path)}],
                                  "rawInput": {"file_path": str(path)}},
                    "options": [{"optionId": "allow", "kind": "allow_once"},
                                {"optionId": "deny", "kind": "reject_once"}]},
        )

    assert request(draft) == {"outcome": {"outcome": "selected", "optionId": "allow"}}
    assert request(other, request_id=2) == {"outcome": {"outcome": "selected", "optionId": "deny"}}
    assert request(draft, kind="delete", request_id=3) == {"outcome": {"outcome": "selected", "optionId": "deny"}}
    assert len(host_calls) == 1


def test_a_permission_question_names_the_command_it_is_asking_about():
    """An engine names a shell call after its tool ("Bash") and leaves the
    command in rawInput. What reached the user was "Run Command / Bash"
    with a Yes/No -- a decision with the one fact needed to make it
    missing."""
    from cortex_runtime.engine.permissions import _named_for_the_user

    asked = _named_for_the_user(
        {"toolCall": {"kind": "execute", "title": "Bash", "rawInput": {"command": "rm -rf build/"}}}
    )
    assert asked["toolCall"]["title"] == "Run command: rm -rf build/"

    # Claude may label a shell request as ``other``. The user still needs
    # the actual command in the permission card, and credentials must not be
    # copied into that card.
    other = _named_for_the_user({
        "toolCall": {
            "kind": "other", "title": "Bash",
            "rawInput": {"command": "touch x && curl --token super-secret https://example.test"},
        }
    })
    assert other["toolCall"]["title"] == "Run command: touch — arguments hidden"


@pytest.mark.parametrize("command", (
    'curl --header "X-Api-Key: super-secret" https://example.test',
    '-H "Authorization: Bearer super-secret" curl https://example.test',
    "python -c '{\"token\":\"super-secret\"}'",
    "TOKEN=super-secret curl https://example.test",
    "curl 'https://example.test/?token=super-secret'",
    "bash -c 'echo unmatched",
    "TOKEN=super-secret",
    "API_KEY=super-secret",
    'TOKEN="super secret"',
    "SeCrEt=super-secret",
    "TOKEN=super-secret curl https://example.test",
    'super-secret "unterminated',
    'TOKEN=super-secret "unterminated',
))
def test_permission_title_hides_sensitive_or_malformed_arguments(command):
    from cortex_runtime.engine.permissions import _named_for_the_user

    title = _named_for_the_user({
        "toolCall": {"kind": "other", "title": "Bash", "rawInput": {"command": command}}
    })["toolCall"]["title"]
    assert "super-secret" not in title
    assert "arguments hidden" in title

    # Nothing else is touched, and a call that already names its command
    # is left exactly as it is.
    already = {"toolCall": {"kind": "execute", "title": "rm -rf build/", "rawInput": {"command": "rm -rf build/"}}}
    assert _named_for_the_user(already)["toolCall"]["title"] == "Run command: rm -rf build/"
    read = {"toolCall": {"kind": "read", "title": "Read calc.py"}}
    assert _named_for_the_user(read)["toolCall"]["title"] == "Read request"

    # The title itself is untrusted too: it must never survive into the
    # permission card, even when rawInput is present or absent.
    leaked = _named_for_the_user({
        "toolCall": {
            "kind": "execute", "title": "Bash: curl --token super-secret",
            "rawInput": {"command": "touch x"},
        }
    })
    assert "super-secret" not in leaked["toolCall"]["title"]
    title_only = _named_for_the_user({
        "toolCall": {"kind": "other", "title": "Use token super-secret"}
    })
    assert title_only["toolCall"]["title"] == "Requested action"
    assert "super-secret" not in title_only["toolCall"]["title"]


def test_a_worker_may_not_edit_the_project_only_its_own_copy(tmp_path):
    """A worker works on a copy, and its result reaches the project only
    through the conflict-checked integration step.

    That was true by convention alone -- the copy was its working
    directory. Live, a worker pointed its `Edit` at the project's real
    path instead: its own copy stayed untouched, there was no integration
    step at all, and the coordinator accepted a change nothing had checked
    against anything.
    """
    from cortex_runtime.engine.permissions import worker_write_outside_workspace

    project = tmp_path / "project"
    workspace = project / ".cortex" / "workspaces" / "xa_1" / "work"
    workspace.mkdir(parents=True)
    root = str(workspace.resolve())

    def edit(path):
        return worker_write_outside_workspace(
            {"toolCall": {"kind": "edit", "locations": [{"path": path}]}}, root,
        )

    # The project's own file: refused, and the refusal names it.
    assert edit(str(project / "calc.py")) == str(project / "calc.py")
    # Its own copy: allowed.
    assert edit(str(workspace / "calc.py")) is None
    # A relative path is relative to its own working directory.
    assert edit("calc.py") is None
    # Its own draft lives in the project's store by design, and filling it
    # is what a worker is for.
    assert worker_write_outside_workspace(
        {"toolCall": {"kind": "edit", "rawInput": {"file_path": str(project / ".cortex/draft-reports/d_1.md")}}},
        root,
    ) is None
    # Reading anything is untouched by this rule.
    assert worker_write_outside_workspace(
        {"toolCall": {"kind": "read", "locations": [{"path": "/etc/hosts"}]}}, root,
    ) is None
    # Deleting and moving count too, wherever they point.
    assert worker_write_outside_workspace(
        {"toolCall": {"kind": "delete", "locations": [{"path": str(project / "README.md")}]}}, root,
    ) == str(project / "README.md")
