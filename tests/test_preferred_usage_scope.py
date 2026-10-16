"""Plan usage never reaches past the owner's preferred models for a work kind."""
from types import SimpleNamespace

from cortex_runtime.routing import auto_route


def _entry(connection, model):
    engine = "codex-acp" if connection.startswith("codex") else "claude-agent-acp"
    route = SimpleNamespace(id=f"{connection}:{model}", engine=engine, model=model,
                            supported_parameters=tuple(("thought_level", e) for e in ("low", "medium", "high")))
    return SimpleNamespace(connection_id=connection, route=route)


A, B, C = _entry("codex-a", "pref-a"), _entry("claude-x", "pref-b"), _entry("codex-b", "other-c")
D = _entry("claude-y", "other-d")


def _models(preferred=("pref-a", "pref-b")):
    tiers = {"codex-acp": {"pref-a": "deep", "other-c": "deep"}, "claude-agent-acp": {"pref-b": "deep", "other-d": "deep"}}
    return {engine: {"tiers": models, "policy": {m: {"preferred_for": ["review"]} for m in models if m in preferred}}
            for engine, models in tiers.items()}


def _rank(entries, used, preferred=("pref-a", "pref-b")):
    return auto_route.rank("deep", entries, lambda e: used.get(e.route.model), role="worker-security-auditor",
                           models=_models(preferred))


def test_usage_compares_only_the_preferred_models():
    # The non-preferred model's unknown usage no longer switches usage off,
    # and its lower usage does not move the preferred models' bands.
    picked, decision = _rank([A, B, C], {"pref-a": 10.0, "pref-b": 80.0})
    assert picked is A and decision["decided_by"] == "usage"
    assert decision["usage"]["scope"] == "preferred" and decision["usage"]["least_used_percent"] == 10.0
    picked, decision = _rank([B, A, C], {"pref-a": 25.0, "pref-b": 34.0, "other-c": 0.0})
    bands = {c["route"]: c["usage_band"] for c in decision["candidates"]}
    assert bands == {"codex-acp/pref-a": 0, "claude-agent-acp/pref-b": 0, "codex-acp/other-c": None}
    assert decision["decided_by"] != "usage" and picked in (A, B)


def test_used_up_preferred_models_hand_over_without_usage_ranking():
    for used in ({"pref-a": 100.0, "pref-b": 100.0, "other-c": 60.0, "other-d": 5.0},
                 {"pref-a": 97.0, "pref-b": 98.0, "other-c": 60.0, "other-d": 5.0}):
        picked, decision = _rank([A, B, C, D], used)
        assert picked in (C, D)
        assert decision["purpose"]["status"] in ("none", "ignored:near_quota_limit")
        assert decision["usage"] == dict(decision["usage"], scope="none", active=False,
                                         status="not_applied:owner_preferred_unavailable")
        assert decision["decided_by"] not in ("usage", "used_percent")
        assert all(c["usage_band"] is None for c in decision["candidates"])


def test_without_preferred_models_usage_ranks_every_candidate():
    picked, decision = _rank([A, B, C, D], {"pref-a": 60.0, "pref-b": 60.0, "other-c": 60.0, "other-d": 5.0},
                             preferred=())
    assert picked is D and decision["decided_by"] == "usage" and decision["usage"]["scope"] == "all"


def test_preferred_models_absent_from_the_live_catalog_still_suppress_usage():
    """F4 regression: the owner's preference is configured in routing.json
    ``models`` for pref-a/pref-b, but neither reaches ``rank`` at all (not
    connected, dropped by eligibility filtering, or otherwise absent from
    the live catalog). Usage ranking must not fall back to comparing every
    remaining candidate (``scope=all``) merely because no *surviving*
    candidate carries the preference -- it must behave exactly like the
    exhausted-but-present case: scope=none, decided by the other criteria
    (here, engine affinity for a review work kind picks the Claude model)."""
    picked, decision = _rank([C, D], {"other-c": 5.0, "other-d": 60.0})
    assert decision["purpose"]["status"] == "none"
    assert decision["usage"] == dict(decision["usage"], scope="none", active=False,
                                     status="not_applied:owner_preferred_unavailable")
    assert decision["decided_by"] not in ("usage", "used_percent")
    assert picked is D
