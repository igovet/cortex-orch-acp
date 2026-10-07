"""Regression coverage for budgeted worker brief provisioning."""
from __future__ import annotations

import hashlib
import json
import subprocess
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation.provisioning import (
    ProvisioningService,
    WORKER_BRIEF_MAX_CHARS,
)
from cortex_runtime.orchestration.artifacts import project_git_evidence
from cortex_runtime.orchestration.worker_brief import build_worker_brief
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery.large_output_store import read_output
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel


class _Workspace:
    def __init__(self):
        self.created = set()

    def create(self, attempt_id):
        self.created.add(attempt_id)
        return "wb_" + attempt_id[3:]

    def lookup(self, attempt_id):
        return "present" if attempt_id in self.created else "absent"

    def remove(self, attempt_id):
        self.created.discard(attempt_id)


class _Engine:
    def __init__(self):
        self.opened = set()

    def open(self, workspace_binding_id, launch, instructions):
        attempt_id = launch["attempt_id"]
        self.opened.add(attempt_id)
        return "es_" + attempt_id[3:]

    def lookup_owned(self, attempt_id):
        return "alive" if attempt_id in self.opened else "absent"

    def terminate_owned(self, attempt_id):
        self.opened.discard(attempt_id)


class _Config:
    def snapshot(self, task_id, role_id=None):
        return {
            "engine": "codex-acp",
            "version": "1.12.0",
            "config_revision": "cr_test",
        }


def _task(tmp_path, goal):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    created = bootstrap_task(
        store,
        SourceEvent(origin="cli", event_id=str(uuid.uuid4())),
        goal,
        "owner-1",
    )
    channel = resolve_channel(store, created.channel_id, created.secret)
    return store, created, channel


def _assignment_ref(task_id, channel_id, request_key):
    return hashlib.sha256(f"{task_id}|{channel_id}|{request_key}".encode()).hexdigest()


def _build(goal, evidence, assignment_ref):
    objective = "Implement the requested change."
    scope = ["src/feature.py", "tests/test_feature.py"]
    dependencies = [{"node": "foundation", "report_ref": "r_exact-dependency-ref"}]
    brief = build_worker_brief(
        objective=objective,
        scope=scope,
        dependency_reports=dependencies,
        original_goal=goal,
        review_evidence=evidence,
        assignment_ref=assignment_ref,
    )
    return brief, objective, scope, dependencies


def _read_large_output(store, task_id, owner_scope, output_id):
    chunks = []
    offset = 0
    while True:
        result = read_output(
            store,
            task_id=task_id,
            owner_scope=owner_scope,
            record_id=output_id,
            offset=offset,
        )
        assert result["encoding"] == "utf-8"
        assert result["failure"] is None
        assert result["truncated"] is False
        chunks.append(result["chunk"])
        if result["next_offset"] is None:
            return "".join(chunks)
        offset = result["next_offset"]


def _provision(store, channel, brief, request_key):
    service = ProvisioningService(
        store,
        _Workspace(),
        _Engine(),
        _Config(),
        allow_unqualified_delivery=True,
    )
    result = service.delegate(
        channel,
        "Budgeted worker",
        brief,
        "backend-dev",
        "Publish a truthful result.",
        request_key,
    )
    assert result.status == "ready"
    assert result.assignment_ref
    assert result.attempt_id
    return result


def test_twenty_thousand_character_goal_is_marked_and_provisioned(tmp_path):
    goal = "🙂" * 20_000
    store, created, channel = _task(tmp_path, goal)
    request_key = "long-goal"
    assignment_ref = _assignment_ref(created.task_id, channel.channel_id, request_key)
    brief, objective, scope, dependencies = _build(goal, [], assignment_ref)

    assert len(brief) <= WORKER_BRIEF_MAX_CHARS
    payload = json.loads(brief)
    assert payload["objective"] == objective
    assert payload["scope"] == scope
    assert payload["dependency_reports"] == dependencies
    goal_value = payload["original_goal"]
    assert goal_value["truncated"] is True
    assert goal_value["original_characters"] == 20_000
    assert goal_value["original_utf8_bytes"] == len(goal.encode("utf-8"))
    assert goal_value["shown_characters"] == len(goal_value["preview"])
    assert goal_value["preview"] == goal[:goal_value["shown_characters"]]
    assert goal_value["full_text_reference"] == {
        "tool": "read_capsule",
        "arguments": {},
        "section": "constraints",
    }

    result = _provision(store, channel, brief, request_key)
    assert result.status == "ready"


def test_two_hundred_kilobyte_diff_is_marked_and_provisioned(tmp_path):
    goal = "Review the complete working tree."
    store, created, channel = _task(tmp_path, goal)
    request_key = "large-review-diff"
    assignment_ref = _assignment_ref(created.task_id, channel.channel_id, request_key)
    diff = "Δ" * 100_000
    evidence = [
        {
            "kind": "status",
            "available": True,
            "large_output_id": "lo_status_exact",
        },
        {
            "kind": "diff",
            "available": True,
            "large_output_id": "lo_diff_exact",
            "output": diff,
        },
    ]
    brief, objective, scope, dependencies = _build(goal, evidence, assignment_ref)

    assert len(diff.encode("utf-8")) == 200_000
    assert len(brief) <= WORKER_BRIEF_MAX_CHARS
    payload = json.loads(brief)
    assert payload["objective"] == objective
    assert payload["scope"] == scope
    assert payload["dependency_reports"] == dependencies
    evidence_value = payload["review_evidence"]
    assert evidence_value["truncated"] is True
    assert evidence_value["original_utf8_bytes"] == len(
        json.dumps(evidence, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")
    )
    assert evidence_value["shown_characters"] == len(evidence_value["preview"])
    refs = evidence_value["full_text_references"]
    assert {
        (ref["tool"], ref["arguments"].get("kind"))
        for ref in refs if ref["tool"] == "project_git"
    } == {("project_git", "status"), ("project_git", "diff")}
    assert {
        ref["arguments"]["large_output_id"]
        for ref in refs if ref["tool"] == "read_large_output"
    } == {"lo_status_exact", "lo_diff_exact"}
    diff_command = next(
        ref for ref in refs
        if ref["tool"] == "project_git" and ref["arguments"]["kind"] == "diff"
    )
    assert diff_command["arguments"]["request_key"] == f"worker-brief:{assignment_ref}:diff"

    result = _provision(store, channel, brief, request_key)
    assert result.status == "ready"


def test_two_hundred_kilobyte_diff_from_real_project_git_is_marked_and_provisioned(tmp_path):
    goal = "🙂" * 20_000
    store, created, channel = _task(tmp_path, goal)
    request_key = "large-real-review-diff"
    assignment_ref = _assignment_ref(created.task_id, channel.channel_id, request_key)

    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    source = tmp_path / "large-review.txt"
    source.write_text("baseline\n")
    subprocess.run(["git", "-C", str(tmp_path), "add", "large-review.txt"], check=True)
    subprocess.run(
        [
            "git", "-C", str(tmp_path), "-c", "user.name=Test",
            "-c", "user.email=test@example.invalid", "commit", "-qm", "baseline",
        ],
        check=True,
    )
    source.write_text("".join(f"line-{index:06d}-" + "x" * 90 + "\n" for index in range(3_000)))

    evidence = [
        project_git_evidence(store, created.task_id, assignment_ref, tmp_path, kind)
        for kind in ("status", "diff")
    ]
    status_evidence, diff_evidence = evidence
    for observed in evidence:
        assert observed["available"] and observed["evidence"] == "context only, not a check receipt"
        assert not {"complete", "exit_code", "tool_call_ref"} & observed.keys()
    assert diff_evidence["byte_count"] >= 200_000
    assert status_evidence["large_output_id"]
    assert diff_evidence["large_output_id"]
    for record in evidence:
        record["output"] = _read_large_output(
            store, created.task_id, assignment_ref, record["large_output_id"]
        )
    assert len(diff_evidence["output"].encode("utf-8")) >= 200_000

    brief, objective, scope, dependencies = _build(goal, evidence, assignment_ref)

    assert len(brief) <= WORKER_BRIEF_MAX_CHARS
    payload = json.loads(brief)
    assert payload["objective"] == objective
    assert payload["scope"] == scope
    assert payload["dependency_reports"] == dependencies
    goal_value = payload["original_goal"]
    assert goal_value["truncated"] is True
    assert goal_value["original_characters"] == 20_000
    assert goal_value["full_text_reference"]["tool"] == "read_capsule"
    evidence_value = payload["review_evidence"]
    assert evidence_value["truncated"] is True
    encoded_evidence = json.dumps(
        evidence, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    )
    assert evidence_value["original_characters"] == len(encoded_evidence)
    assert evidence_value["original_utf8_bytes"] == len(encoded_evidence.encode("utf-8"))
    assert evidence_value["shown_characters"] == len(evidence_value["preview"])
    expected_ids = {
        "status": status_evidence["large_output_id"],
        "diff": diff_evidence["large_output_id"],
    }
    refs = evidence_value["full_text_references"]
    git_refs = {
        ref["kind"]: ref for ref in refs if ref["tool"] == "project_git"
    }
    read_refs = {
        ref["kind"]: ref for ref in refs if ref["tool"] == "read_large_output"
    }
    assert set(git_refs) == {"status", "diff"}
    assert set(read_refs) == {"status", "diff"}
    for kind in ("status", "diff"):
        assert git_refs[kind]["arguments"] == {
            "kind": kind,
            "request_key": f"worker-brief:{assignment_ref}:{kind}",
        }
        assert read_refs[kind]["arguments"] == {
            "large_output_id": expected_ids[kind],
            "offset": 0,
        }

    result = _provision(store, channel, brief, request_key)
    assert result.status == "ready"


def test_small_brief_has_no_truncation_marker():
    goal = "Keep the existing line breaks."
    evidence = [{"kind": "status", "available": True, "large_output_id": "lo_small"}]
    brief, objective, scope, dependencies = _build(goal, evidence, "assignment-test")
    payload = json.loads(brief)

    assert len(brief) <= WORKER_BRIEF_MAX_CHARS
    assert payload["objective"] == objective
    assert payload["scope"] == scope
    assert payload["dependency_reports"] == dependencies
    assert payload["original_goal"] == goal
    assert payload["review_evidence"] == evidence
    assert "truncated" not in brief


def test_mandatory_brief_content_over_budget_fails_closed():
    with pytest.raises(StoreError) as caught:
        build_worker_brief(
            objective="x" * WORKER_BRIEF_MAX_CHARS,
            scope=["src/feature.py"],
            dependency_reports=[],
            original_goal="Original goal.",
            review_evidence=[],
            assignment_ref="assignment-test",
        )

    assert str(caught.value) == "invalid_provisioning_request"
    assert caught.value.field == "brief"
    assert "mandatory fields" in caught.value.expected
