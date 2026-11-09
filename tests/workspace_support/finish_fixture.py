"""Explicit fake catalogue and maintenance clock; this file is never packaged."""
import json
from pathlib import Path
import time


def catalogue():
    from cortex_runtime.connections import onboarding
    from cortex_runtime.catalog import discovery, store
    from cortex_runtime.configuration import management
    native = Path.home() / ".codex"
    native.mkdir(mode=0o700, exist_ok=True)
    (native / "auth.json").write_text("{}")  # Synthetic existence marker, not authentication.
    record = onboarding.create_referenced_connection(
        name="Offline fixture", engine="codex-acp", provider="openai",
        launch_command=("/usr/bin/false",))
    options = [
        dict(id="model", name="Model", type="select", category="model", currentValue="fixture-model",
             options=[dict(value="fixture-model", name="Fixture model")]),
        dict(id="thought_level", name="Effort", type="select", category="thought_level",
             currentValue="high", options=[dict(value=v, name=v) for v in ("low", "medium", "high")]),
    ]
    snapshot = discovery.build_catalog({record.id: record}, {record.id: discovery.parse_observation(options)})
    store.save_snapshot(snapshot)
    current = management.execute(dict(action="read", surface="routing"))["result"]
    user = next((s for s in current["sources"] if s["source"] == "user"), {})
    # Only the fake engine catalogue/policy is supplied; no tasks or product rows.
    reply = management.execute(dict(action="update", surface="routing", target="user",
        expected_revision=user.get("revision") or "absent",
        rules=dict(model_policy_version=2, enabled={"codex-acp": ["fixture-model"]},
                   models={"codex-acp": dict(auto=["fixture-model"], tiers={"fixture-model": ["deep", "standard", "light"]})})))
    assert reply["ok"], reply
    return snapshot


def cleanup(state, api):
    import machine_tools
    path = Path(api.store.project_root) / ".git/maintenance-clock.json"
    at = json.loads(path.read_text()) if path.exists() else int(time.time() * 1000)
    return machine_tools.cleanup(state, [{"id": api.project_id, "root": api.store.project_root, "name": "Project"}],
                                 clock=lambda: at)
