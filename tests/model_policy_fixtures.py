"""Explicit historical owner settings for routing regression tests, not runtime defaults."""
from copy import deepcopy

TIERS = {
    "codex-acp": {
        "gpt-6-astra": "flagship", "gpt-6-sol": "deep", "gpt-6.1-sol": ["deep", "standard"],
        "gpt-6-luna": "light", "gpt-5.6-sol": "standard", "gpt-5.6-terra": "standard",
        "gpt-5.6-luna": "light", "gpt-5.5": "light", "gpt-7-luna": "light", "gpt-7-terra": "standard",
        "alpha": "deep", "beta": "deep", "gamma": "standard", "delta": "standard", "epsilon": "light",
        "coordinator-model": "deep", "reviewer-model": "deep", "luna": "light",
    },
    "claude-agent-acp": {
        "claude-opus-5[1m]": "deep", "claude-opus-4-6": "deep", "claude-opus-4-8": "deep",
        "claude-opus-5-5": "deep", "opus[1m]": "deep", "opus": "deep", "default": "deep",
        "sonnet": "standard", "claude-sonnet-5": "standard", "claude-sonnet-4-6": "standard",
        "haiku": "light", "claude-haiku-5-5": "light", "claude-haiku-4-5-20251001": "light", "claude-fable-5-1[1m]": "flagship",
        "coordinator-model": "deep", "reviewer-model": "deep",
    },
}
STRETCH = {"effort_floor": "high", "effort_ladder": ["high", "xhigh", "max"], "also_standard_for": ["testing"]}
MODELS = {engine: {"tiers": deepcopy(tiers), "auto": list(tiers), "policy": {}} for engine, tiers in TIERS.items()}
for model in ("gpt-6-luna", "gpt-5.6-luna", "gpt-7-luna"):
    MODELS["codex-acp"]["policy"][model] = deepcopy(STRETCH)
MODELS["claude-agent-acp"]["policy"]["haiku"] = deepcopy(STRETCH)
MODELS["claude-agent-acp"]["policy"]["claude-haiku-5-5"] = deepcopy(STRETCH)
MODELS["codex-acp"]["policy"]["gpt-6-astra"] = {"max_effort": "high", "request_names": ["astra", "астр"]}
MODELS["claude-agent-acp"]["policy"]["claude-fable-5-1[1m]"] = {"max_effort": "high", "request_names": ["fable", "фейбл", "фэйбл"]}


def routing_fixture(overrides=None):
    """Materialize explicit test settings; a supplied model policy replaces fields."""
    result = {"model_policy_version": 2, "enabled": {e: list(t) for e, t in TIERS.items()}, "models": deepcopy(MODELS)}
    overrides = deepcopy(overrides or {})
    result["models"]["codex-acp"]["auto"] = ["gpt-6-astra", "gpt-6-sol", "gpt-6.1-sol", "gpt-6-luna", "gpt-5.6-terra", "gpt-5.5", "coordinator-model", "reviewer-model", "alpha", "beta", "gamma", "delta", "epsilon"]
    result["models"]["claude-agent-acp"]["auto"] = ["opus[1m]", "claude-fable-5-1[1m]", "claude-sonnet-5", "sonnet", "coordinator-model", "reviewer-model", "claude-opus-4-6"]
    result["enabled"].update(overrides.pop("enabled", {}))
    for engine, config in overrides.pop("models", {}).items():
        target = result["models"].setdefault(engine, {})
        for key, value in config.items():
            # Attribute nulls are intentional, but an incomplete test policy
            # no longer inherits model-specific attributes from executable code.
            if key in ("tiers", "policy"):
                target.setdefault(key, {}).update(value)
            else:
                target[key] = value
    # Translate legacy regression inputs into explicitly engine-keyed fixture data.
    for model, tier in overrides.pop("tiers", {}).items():
        for engine, config in result["models"].items():
            if model in config.get("tiers", {}):
                config["tiers"][model] = tier
    result.update(overrides)
    return result


def exact_policy(identities, *, automatic=None):
    """Minimum owner policy for explicitly named fake identities, no defaults."""
    automatic = automatic or {}
    return {
        "model_policy_version": 2,
        "enabled": {engine: list(tiers) for engine, tiers in identities.items()},
        "models": {
            engine: {"tiers": deepcopy(tiers), "auto": list(automatic.get(engine, ()))}
            for engine, tiers in identities.items()
        },
    }


def write_policy(environ, identities, *, automatic=None):
    """Write only to the caller's disposable XDG configuration root."""
    import json
    from pathlib import Path

    path = Path(environ["XDG_CONFIG_HOME"]) / "cortex" / "routing.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(exact_policy(identities, automatic=automatic)), encoding="utf-8")
    return path
