"""No-paid stage preflight for an owner-selected exact route.

The fixtures use the public installation template's identities. The preflight
must agree with the real ``auto_route.choose`` decision for the mandatory
review node, refuse visibly instead of accepting a substituted reviewer, and
never write, refresh quota or grant consent.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace

import pytest

from cortex_runtime.connections import quota, registry
from cortex_runtime.routing import auto_route
from cortex_runtime.routing.stage_preflight import REVIEW_ROLE, preflight

ROOT = Path(__file__).resolve().parents[1]
HAIKU = 'claude-agent-acp/haiku'
LUNA = 'codex-acp/gpt-6-luna'
SONNET = 'claude-agent-acp/claude-sonnet-5'
OPPOSITE = {'claude-agent-acp': 'codex-acp', 'codex-acp': 'claude-agent-acp'}


def _config(tmp_path, **changes):
    routing = json.loads((ROOT / 'policy' / 'model-settings-default.json').read_text())['routing']
    routing.update(changes)
    directory = tmp_path / 'xdg' / 'cortex'
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'routing.json').write_text(json.dumps(routing))
    return {'HOME': str(tmp_path), 'XDG_CONFIG_HOME': str(tmp_path / 'xdg')}


def _tree(tmp_path):
    return {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in tmp_path.rglob('*') if p.is_file()}


def _reviewer(environ, route, monkeypatch):
    """The real runtime reviewer decision over a fixed catalogue, no I/O."""
    def entry(label):
        engine, model = label.split('/')
        return SimpleNamespace(connection_id=engine, route=SimpleNamespace(
            id=f'{engine}:{model}', engine=engine, model=model,
            supported_parameters=tuple(('thought_level', e) for e in ('low', 'medium', 'high', 'xhigh', 'max'))))
    entries = [entry(label) for label in (HAIKU, LUNA, SONNET, 'codex-acp/gpt-6.1-sol')]
    monkeypatch.setattr(registry, 'read_registry', lambda _e: SimpleNamespace(connections={
        'claude-agent-acp': SimpleNamespace(launch_command=('claude',)), 'codex-acp': SimpleNamespace(launch_command=('codex',))}))
    monkeypatch.setattr(auto_route, '_entries', lambda *_a: entries)
    monkeypatch.setattr(auto_route, '_is_seed', lambda _e: False)
    monkeypatch.setattr(quota, 'used_percent', lambda *_a, **_k: None)
    return auto_route.choose(REVIEW_ROLE, None, environ, preference=OPPOSITE[route.split('/')[0]])


@pytest.mark.parametrize('route', [HAIKU, LUNA])
def test_owner_reviewer_pin_makes_every_stage_ready_and_matches_runtime(tmp_path, monkeypatch, route):
    environ = _config(tmp_path, pin={REVIEW_ROLE: route + ':high', '*': SONNET + ':medium'})
    before = _tree(tmp_path)
    result = preflight(route + ':high', None, environ)
    assert result['ready'] and result['refusals'] == []
    assert result['stages']['review'] == dict(status='pinned', resolved=route + ':high')
    assert result['stages']['execution'] == dict(status='requires_captured_user_request', hard_policy='allowed')
    assert result['stages']['review_fix'] == dict(status='inherits_execution')
    assert result['routing_source']['kind'] == 'user'
    assert _tree(tmp_path) == before
    choice = _reviewer(environ, route, monkeypatch)
    assert (choice.decision['chosen'], choice.basis, choice.effort) == (route, 'auto:pin', 'high')
    assert choice.decision['preference_status'] == 'ignored:pinned'


@pytest.mark.parametrize('route', [HAIKU, LUNA])
def test_wildcard_owner_pin_to_another_model_is_refused_before_launch(tmp_path, monkeypatch, route):
    # The observed dev profile: a '*' pin decided both reviewers.
    environ = _config(tmp_path, pin={'*': SONNET + ':medium'})
    result = preflight(route, None, environ)
    assert not result['ready'] and result['refusals'] == ['review:review_route_pin_mismatch']
    assert result['stages']['review']['resolved'] == SONNET + ':medium'
    choice = _reviewer(environ, route, monkeypatch)
    assert choice.decision['chosen'] == SONNET and choice.basis == 'auto:pin'


def test_unrelated_automatic_routing_is_not_a_guarantee(tmp_path, monkeypatch):
    environ = _config(tmp_path)
    result = preflight(HAIKU, None, environ)
    assert result['refusals'] == ['review:review_route_not_pinned']
    assert result['stages']['review']['resolved'] == 'automatic_ranking'
    # The real ranking never puts a light stretch model on review work.
    assert _reviewer(environ, HAIKU, monkeypatch).decision['chosen'] != HAIKU


def test_hard_owner_deny_and_providers_refuse_even_with_a_pin(tmp_path):
    routing = json.loads((ROOT / 'policy' / 'model-settings-default.json').read_text())['routing']
    environ = _config(tmp_path, pin={REVIEW_ROLE: HAIKU}, deny=routing['deny'] + [HAIKU])
    result = preflight(HAIKU, None, environ)
    assert result['refusals'] == ['execution:route_denied_by_owner_policy', 'review:route_denied_by_owner_policy']
    environ = _config(tmp_path, pin={REVIEW_ROLE: LUNA}, providers=['claude-agent-acp'])
    assert preflight(LUNA, None, environ)['refusals'] == [
        'execution:route_provider_not_allowed', 'review:route_provider_not_allowed']


def test_effort_mismatch_invalid_route_and_missing_policy_fail_closed(tmp_path):
    environ = _config(tmp_path, pin={REVIEW_ROLE: HAIKU + ':medium'})
    assert preflight(HAIKU + ':high', None, environ)['refusals'] == ['review:review_effort_mismatch']
    assert preflight('haiku; rm -rf /', None, environ)['refusals'] == ['invalid_route']
    missing = {'HOME': str(tmp_path / 'none'), 'XDG_CONFIG_HOME': str(tmp_path / 'none')}
    result = preflight(HAIKU, None, missing)
    assert not result['ready'] and result['refusals'][0].startswith('routing_policy_unavailable:')
    assert not (tmp_path / 'none').exists()


def test_selected_effort_checks_execution_and_review_policy_before_launch(tmp_path):
    sol = "codex-acp/gpt-6.1-sol"
    env = _config(tmp_path, pin={REVIEW_ROLE: sol + ":medium"})
    assert preflight(sol + ":medium", None, env)["ready"]
    file = Path(env["XDG_CONFIG_HOME"]) / "cortex" / "routing.json"
    routing = json.loads(file.read_text())
    routing["models"]["codex-acp"]["policy"]["gpt-6.1-sol"]["effort_floor"] = "high"
    file.write_text(json.dumps(routing))
    before = _tree(tmp_path)
    result = preflight(sol + ":medium", None, env)
    assert result["refusals"] == [
        "execution:selected_effort_conflicts_with_policy",
        "review:selected_effort_conflicts_with_policy",
    ]
    assert _tree(tmp_path) == before


def test_reviewer_pin_that_would_be_clamped_is_not_ready(tmp_path):
    env = _config(tmp_path, pin={REVIEW_ROLE: HAIKU + ":medium"})
    assert preflight(HAIKU + ":medium", None, env)["refusals"] == [
        "execution:selected_effort_conflicts_with_policy",
        "review:selected_effort_conflicts_with_policy",
    ]


def test_project_policy_takes_precedence(tmp_path):
    environ = _config(tmp_path, pin={'*': SONNET})
    project = tmp_path / 'project'
    (project / '.cortex').mkdir(parents=True)
    routing = json.loads((ROOT / 'policy' / 'model-settings-default.json').read_text())['routing']
    (project / '.cortex' / 'routing.json').write_text(json.dumps(dict(routing, pin={REVIEW_ROLE: LUNA})))
    result = preflight(LUNA, project, environ)
    assert result['ready'] and result['routing_source']['kind'] == 'project'


def test_dev_wrapper_reads_only_the_isolated_profile_and_exits_nonzero_on_refusal(tmp_path):
    home = tmp_path / 'paseo-dev'
    config = home / 'cortex-xdg' / 'config' / 'cortex'
    config.mkdir(parents=True)
    routing = json.loads((ROOT / 'policy' / 'model-settings-default.json').read_text())['routing']
    (config / 'routing.json').write_text(json.dumps(dict(routing, pin={'*': SONNET + ':medium'})))
    argv = [sys.executable, '-B', str(ROOT / 'scripts' / 'cortex_paseo_dev.py'), 'route-preflight',
            '--home', str(home), '--route', HAIKU + ':high']
    env = {'PATH': '/usr/bin:/bin', 'HOME': str(tmp_path)}
    refused = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=60)
    assert refused.returncode == 1, refused.stderr
    assert json.loads(refused.stdout)['refusals'] == ['review:review_route_pin_mismatch']
    (config / 'routing.json').write_text(json.dumps(dict(routing, pin={REVIEW_ROLE: HAIKU + ':high'})))
    before = _tree(tmp_path)
    ready = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=60)
    assert ready.returncode == 0 and json.loads(ready.stdout)['ready'] is True
    assert _tree(tmp_path) == before
    production = subprocess.run(argv[:4] + ['--home', str(tmp_path / '.paseo'), '--route', HAIKU],
                                capture_output=True, text=True, env=env, timeout=60)
    assert production.returncode == 2 and json.loads(production.stdout)['refusals'] == ['production_home_refused']
