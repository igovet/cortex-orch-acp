from copy import deepcopy

from cortex_runtime.orchestration.prompt_context import coordinator_prompt_context
from cortex_runtime.orchestration.state import encoded


def _node(node_id, *, role="worker-backend-dev", padding=0, result=None):
    return {
        "node_id": node_id,
        "role": role,
        "status": "completed",
        "stage": "review" if node_id.startswith("_review_") else "execution",
        "objective": "historical objective " + ("x" * padding),
        "scope": ["src/" + ("s" * padding)],
        "dependencies": [],
        "result": {
            "summary": "historical result " + ("x" * padding),
            "report_ref": "r_" + node_id,
            "checks": [{
                "name": "old passing check",
                "outcome": "passed",
                "detail": "p" * padding,
            }],
            **(result or {}),
        },
    }


def _three_cycle_context(padding=0):
    gap = {
        "name": "frontend checks",
        "outcome": "unverified",
        "blocker": "frontend toolchain unavailable",
        "command": ["npm", "test"],
    }
    original = _node("feature", padding=padding, result={"checks": [
        {"name": "frontend checks", **gap},
        {"name": "old passing check", "outcome": "passed", "detail": "p" * padding},
    ]})
    return {
        "state": "RECONCILING",
        "revision": 1,
        "review_cycle": 3,
        "goal": "Implement the requested feature.",
        "constraints": ["Preserve the existing public API."],
        "plan": {
            "tasks": [{"id": "feature", "objective": "Implement the feature."}],
            "verification": [["python3", "-B", "-m", "pytest", "-q"]],
        },
        "user_answers": [{"question_id": "q1", "answer_text": "Keep the API stable."}],
        "steering": [{"text": "Prioritize correctness."}],
        "runtime_proposals": [{"kind": "replan", "status": "open"}],
        "verification": {"status": "not_run", "blocker": "awaiting synthesis"},
        "unresolved_gaps": [{"surface": "frontend", "reason": "not verified"}],
        "nodes": [
            original,
            _node("_review_1", role="worker-code-reviewer", padding=padding,
                  result={"verdict": "REQUEST_CHANGES"}),
            _node("_fix_1_feature", padding=padding),
            _node("_review_2", role="worker-code-reviewer", padding=padding,
                  result={"verdict": "REQUEST_CHANGES"}),
            _node("_fix_2_feature"),
            _node("_review_3", role="worker-code-reviewer",
                  result={"verdict": "APPROVE", "summary": "Current review result"}),
        ],
    }


def test_three_cycle_prompt_compacts_old_payloads_and_preserves_gaps_and_context():
    small = _three_cycle_context()
    large = _three_cycle_context(padding=20_000)
    original_nodes = deepcopy(large["nodes"])

    projected = coordinator_prompt_context(large)
    projected_nodes = {node["node_id"]: node for node in projected["nodes"]}

    assert set(projected_nodes["feature"]) == {
        "node_id", "role", "status", "report_ref", "verdict",
    }
    assert projected_nodes["feature"] == {
        "node_id": "feature",
        "role": "worker-backend-dev",
        "status": "completed",
        "report_ref": "r_feature",
        "verdict": None,
    }
    assert set(projected_nodes["_review_2"]) == {
        "node_id", "role", "status", "report_ref", "verdict",
    }
    assert projected_nodes["_fix_2_feature"]["objective"] == "historical objective "
    assert projected_nodes["_review_3"]["result"]["verdict"] == "APPROVE"
    assert projected["historical_check_gaps"] == [{
        "node_id": "feature",
        "check": {
            "name": "frontend checks",
            "outcome": "unverified",
            "blocker": "frontend toolchain unavailable",
            "command": ["npm", "test"],
        },
    }]

    for key, value in large.items():
        if key != "nodes":
            assert projected[key] == value
    assert large["nodes"] == original_nodes
    assert len(encoded(coordinator_prompt_context(small))) == len(encoded(projected))


def test_latest_review_and_its_active_fix_batch_stay_full():
    context = _three_cycle_context()
    active_fix = _node("_fix_3_feature", result={"summary": "Current fix"})
    context["nodes"].append(active_fix)

    projected = coordinator_prompt_context(context)
    projected_nodes = {node["node_id"]: node for node in projected["nodes"]}

    assert projected_nodes["_review_3"]["result"]["verdict"] == "APPROVE"
    assert projected_nodes["_fix_3_feature"]["result"]["summary"] == "Current fix"
    assert set(projected_nodes["_fix_2_feature"]) == {
        "node_id", "role", "status", "report_ref", "verdict",
    }


def test_first_review_keeps_original_plan_nodes_full():
    context = {
        "review_cycle": 1,
        "plan": {"tasks": [{"id": "feature"}]},
        "nodes": [
            _node("feature", result={"summary": "Original implementation"}),
            _node("_review_1", role="worker-code-reviewer",
                  result={"verdict": "APPROVE"}),
        ],
    }

    assert coordinator_prompt_context(context) is context
    assert "objective" in context["nodes"][0]
