"""Audit finding C-25 (owner D9, recommended option): the Claude engine
session for Cortex coordinator/workers must not expose native EnterWorktree,
ExitWorktree, PushNotification, EnterPlanMode, ExitPlanMode or TaskStop --
their outcome belongs to the Cortex runtime's own workspace scope, plan/
execution lifecycle and cancellation/completion path, never the model.
WebFetch/WebSearch stay available. Also covers disabling native auto-memory
injection via the installed SDK's ``managedSettings.autoMemoryEnabled``.
"""
from __future__ import annotations

import pytest

from cortex_runtime.engine.claude_adapter_acp import (
    CLAUDE_GATEWAY_ALTERNATIVES,
    CLAUDE_NATIVE_LIFECYCLE_TOOLS,
    CLAUDE_SUBAGENT_TOOLS,
    build_claude_session_new_meta,
    claude_native_subagent_restriction_meta,
)

LIFECYCLE_TOOLS = ("EnterWorktree", "ExitWorktree", "PushNotification",
                   "EnterPlanMode", "ExitPlanMode", "TaskStop")


def test_lifecycle_tools_tuple_matches_the_audit_finding():
    assert set(CLAUDE_NATIVE_LIFECYCLE_TOOLS) == set(LIFECYCLE_TOOLS)
    assert len(CLAUDE_NATIVE_LIFECYCLE_TOOLS) == len(LIFECYCLE_TOOLS)


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_native_lifecycle_tools_are_disallowed_for_every_role(role):
    meta = claude_native_subagent_restriction_meta(role=role)
    disallowed = set(meta["claudeCode"]["options"]["disallowedTools"])
    for tool in LIFECYCLE_TOOLS:
        assert tool in disallowed
    # Existing subagent and native-gateway-alternative restrictions survive.
    assert set(CLAUDE_SUBAGENT_TOOLS) <= disallowed
    assert set(CLAUDE_GATEWAY_ALTERNATIVES) <= disallowed


def test_web_fetch_and_web_search_remain_available():
    meta = claude_native_subagent_restriction_meta(role="worker")
    disallowed = set(meta["claudeCode"]["options"]["disallowedTools"])
    assert "WebFetch" not in disallowed
    assert "WebSearch" not in disallowed


def test_managed_settings_disables_native_auto_memory_alongside_hooks():
    meta = claude_native_subagent_restriction_meta(role="worker")
    managed = meta["claudeCode"]["options"]["managedSettings"]
    assert managed["disableAllHooks"] is True
    assert managed["autoMemoryEnabled"] is False


def test_build_claude_session_new_meta_carries_the_same_restrictions():
    meta = build_claude_session_new_meta("Cortex instructions", role="coordinator")
    options = meta["claudeCode"]["options"]
    disallowed = set(options["disallowedTools"])
    for tool in LIFECYCLE_TOOLS:
        assert tool in disallowed
    assert options["managedSettings"]["autoMemoryEnabled"] is False
