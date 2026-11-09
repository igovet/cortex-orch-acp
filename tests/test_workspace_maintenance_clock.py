"""Clock injection has no environment, wire, or packaged fixture entry point."""
import importlib.util
import inspect
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_cleanup_clock_is_explicit_and_default_is_wall_clock(tmp_path):
    spec = importlib.util.spec_from_file_location("clock_machine", ROOT / "crates/workspace-service/machine_tools.py")
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    at = 1234567890000
    result = m.cleanup(tmp_path, [], clock=lambda: at)
    assert result["at"] == at
    assert m.read_json(tmp_path / "machine-runs.json", {})["cleanup"]["at"] == at
    assert inspect.signature(m.cleanup).parameters["clock"].default is m.now_ms
    assert m.cleanup(tmp_path, [])["at"] > at


def test_fixture_clock_and_catalogue_not_in_payload():
    import sys
    sys.path.insert(0, str(ROOT / "scripts"))
    import cortex_package
    paths = {str(p) for p in cortex_package.payload_paths(ROOT)}
    assert not any("finish_fixture" in p or "journeys_finish" in p for p in paths)
    source = (ROOT / "crates/workspace-service/agent_launcher.py").read_text()
    assert "maintenance-clock.json" not in source and "finish_fixture" not in source
