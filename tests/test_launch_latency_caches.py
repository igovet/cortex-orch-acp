"""Per-launch work that repeated identical results: pinned install hashes and
the per-model catalog probe before every first prompt."""
import os
import time

from cortex_runtime.engine import install_digest


def test_install_digest_is_reused_until_the_file_changes(tmp_path, monkeypatch):
    path = tmp_path / 'codex'
    path.write_bytes(b'a' * 4096)
    reads = []
    real = install_digest.hashlib.sha256

    def counting():
        reads.append(1)
        return real()

    monkeypatch.setattr(install_digest.hashlib, 'sha256', counting)
    first = install_digest.sha256_file(path)
    assert install_digest.sha256_file(path) == first and len(reads) == 1
    path.write_bytes(b'b' * 4096)
    os.utime(path, ns=(time.time_ns(), time.time_ns()))
    changed = install_digest.sha256_file(path)
    assert changed != first and len(reads) == 2


def test_an_identical_catalog_observation_is_not_probed_again(tmp_path, monkeypatch):
    from types import SimpleNamespace
    from cortex_runtime.catalog import model_efforts
    from cortex_runtime.delegation import live_adapters
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    from engine_fakes import isolated_environ, new_connection_id
    from test_catalog_refresh import _connection

    monkeypatch.setattr(live_adapters, '_CATALOG_OBSERVED', {})
    environ = isolated_environ(tmp_path)
    record = _connection(new_connection_id(), 'claude-agent-acp')
    probes = []
    monkeypatch.setattr(model_efforts, 'observe_model_efforts', lambda *args, **kwargs: probes.append(1) or {})

    def manager(current):
        options = [{'id': 'model', 'name': 'Model', 'category': 'model', 'type': 'select',
                    'currentValue': current, 'options': [{'value': 'a', 'name': 'A'}, {'value': 'b', 'name': 'B'}]}]
        return SimpleNamespace(observed_config_options=lambda _: options,
                               _client=SimpleNamespace(call=lambda *args: None),
                               correlation=SimpleNamespace(wire_session_id=lambda _: 'wire'))

    adapter = EngineSessionManagerAdapter(lambda _: record, lambda _: str(tmp_path), environ=environ)
    adapter._capture_catalog_observation(manager('a'), record, 's1')
    # Another session of the same install selecting another model: same catalog.
    adapter._capture_catalog_observation(manager('b'), record, 's2')
    assert len(probes) == 1
    changed = manager('a')
    changed.observed_config_options('x')[0]['options'].append({'value': 'c', 'name': 'C'})
    adapter._capture_catalog_observation(changed, record, 's3')
    assert len(probes) == 2
    # A new Cortex process (another chat) reuses the saved observation too.
    monkeypatch.setattr(live_adapters, '_CATALOG_OBSERVED', {})
    adapter._capture_catalog_observation(changed, record, 's4')
    assert len(probes) == 2


def test_a_try_again_configuration_rejection_is_repeated_and_others_are_not():
    import pytest
    from types import SimpleNamespace
    from cortex_runtime.engine.client import EngineProtocolError
    from cortex_runtime.engine.session_manager import EngineSessionManager

    def manager(failures):
        calls = []

        def call(method, params, timeout=None):
            calls.append(params)
            if failures:
                raise failures.pop(0)
            return {"configOptions": []}
        observed = []
        return SimpleNamespace(_client=SimpleNamespace(call=call), _call_timeout=5, _TRANSIENT_RETRY_DELAYS=(0, 0),
                               _process=SimpleNamespace(logs=SimpleNamespace(observe=lambda *a, **k: observed.append(a)))), calls

    transient = lambda: EngineProtocolError(-32603, "Internal error", {"details": "Couldn't confirm model \"m\" with the API. Try again, or run /model."})
    fake, calls = manager([transient(), transient()])
    assert EngineSessionManager._set_config_option_retrying(fake, "w", "model", "m") == {"configOptions": []}
    assert len(calls) == 3
    fake, calls = manager([transient(), transient(), transient()])
    with pytest.raises(EngineProtocolError):
        EngineSessionManager._set_config_option_retrying(fake, "w", "model", "m")
    assert len(calls) == 3
    fake, calls = manager([EngineProtocolError(-32602, "Invalid params", {"details": "unknown model"})])
    with pytest.raises(EngineProtocolError):
        EngineSessionManager._set_config_option_retrying(fake, "w", "model", "m")
    assert len(calls) == 1
