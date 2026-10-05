"""Installer -> ACP selector -> native engine, with no paid engine or host."""
import json

import pytest

from cortex_runtime.catalog import discovery, store as catalog_store
from cortex_runtime.configuration import store as config_store
from cortex_runtime.configuration.policy import COORDINATOR_MODE_FLOOR
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import registry
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.routing import defaults
from test_acp_config_options import _new_agent, _bootstrap_task_for_session, _configured_connection
from test_auto_route_delegation import _make_connection, _MODE_BY_ENGINE
from engine_fakes import isolated_environ


@pytest.mark.parametrize('coordinator_engine', [None, 'codex-acp'])
@pytest.mark.parametrize('echo', [None, 'model', 'provider'])
@pytest.mark.parametrize('renamed', [False, True])
def test_installer_native_parameters_survive_host_identity_echo(tmp_path, monkeypatch, echo, renamed, coordinator_engine):
    env = isolated_environ(tmp_path)
    for key, value in env.items():
        monkeypatch.setenv(key, value)
    template = defaults.load_model_settings_template()
    if coordinator_engine is not None:
        template['coordinator'] = dict(
            engine=coordinator_engine,
            model=template['routing']['enabled'][coordinator_engine][0],
            parameters=dict(thought_level='medium', mode='agent'))
    engine = template['coordinator']['engine']
    if renamed:
        template['coordinator']['model'] = 'vendor/arbitrary-renamed'
        template['routing']['enabled'][engine].append('vendor/arbitrary-renamed')
    if renamed or coordinator_engine is not None:
        path = tmp_path / 'template.json'
        path.write_text(json.dumps(template))
        monkeypatch.setattr(defaults, 'TEMPLATE_PATH', path)
    # The production installer, not a synthesized defaults file.
    import cortex_acp_setup as setup
    setup.init_storage()
    selected = template['coordinator']
    model = selected['model']
    log = tmp_path / 'requests.jsonl'
    record = _make_connection(tmp_path, env, engine=engine, model=model, request_log=log)
    effort_option_id = 'reasoning_effort' if engine == 'codex-acp' else 'effort'
    raw = [dict(id='model', type='select', currentValue=model, options=[dict(value=model, name=model)]),
           dict(id=effort_option_id, type='select', currentValue='low', options=[dict(value=v, name=v) for v in ('low', 'medium', 'high')]),
           dict(id='mode', type='select', currentValue=_MODE_BY_ENGINE[engine][0]['value'],
                options=_MODE_BY_ENGINE[engine])]
    for option in raw:
        option['name'] = option['id']
        option['category'] = 'thought_level' if option['id'] == effort_option_id else option['id']
    catalog_store.save_snapshot(discovery.build_catalog({record.id: record}, {record.id: discovery.parse_observation(raw)}), env)
    agent, transport = _new_agent(tmp_path, env)
    agent.client_info = {'name': 'Zed' if echo == 'provider' else 'Paseo', 'version': 'fixture'}
    project = tmp_path / 'project'
    project.mkdir()
    sid = agent._session_new({'cwd': str(project), 'mcpServers': []})['sessionId']
    session = agent.sessions[sid]
    store = agent._store_for(str(project))
    original = (registry.config_root(env) / 'defaults.json').read_bytes()
    entries, connections = agent._selectable_entries_and_connections(str(project))
    assert entries, (registry.read_registry(env), session['default_selection'])
    assert any(entry.route.engine == engine and entry.route.model == model for entry in entries)
    if echo:
        options = agent._config_options_for_session(session, store)
        value = next(o['currentValue'] for o in options if o['id'] == echo)
        agent._session_set_config_option(1, dict(sessionId=sid, configId=echo, value=value))
        assert 'error' not in transport.responses[-1]
    after = config_store.read_user_defaults(env)['defaults']
    native = after.get('coordinator', after).get('parameters')
    assert native == selected['parameters']
    assert (registry.config_root(env) / 'defaults.json').read_bytes() == original
    store, task = _bootstrap_task_for_session(agent, sid, tmp_path)
    assert agent._propose_pending_selection_for_new_task(store, session, task)
    service = ConfigurationService(store)
    revision = service.history(task)[-1]
    # Runtime mode floor is confirmed on the wire, never written into install defaults.
    expected_parameters = dict(selected['parameters'], mode=COORDINATOR_MODE_FLOOR[engine])
    assert revision['parameters'] == expected_parameters
    # Real adapter constructor and protocol confirmation, only engine is fake.
    adapter = EngineSessionManagerAdapter(lambda t: record, lambda t: str(project), environ=env, store=store)
    try:
        engine_sid = adapter.open('fixture-workspace', dict(task_id=task, attempt_id='co_fixture',
                                 config_revision=revision['id'], parameters=revision['parameters']), 'Fixture instructions')
        active = service.current(task)
        assert active['state'] == 'active' and active['confirmation_evidence_ref']
        assert active['parameters'] == expected_parameters
        requests = [json.loads(line) for line in log.read_text().splitlines()]
        changes = [r['params'] for r in requests if r.get('method') == 'session/set_config_option']
        assert any(r['configId'] == effort_option_id and r['value'] == selected['parameters']['thought_level'] for r in changes)
        assert any(r['configId'] == 'mode' and r['value'] == expected_parameters['mode'] for r in changes)
        assert all(set(r) <= {'sessionId', 'configId', 'value'} for r in changes)
    finally:
        adapter.terminate_owned('co_fixture')


def test_explicit_effort_is_not_an_identity_reset_and_keeps_unknown_defaults(tmp_path, monkeypatch):
    from test_acp_config_options import _isolate, _build_and_save_snapshot
    from cortex_runtime.configuration.policy import PolicyCeiling
    env = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=env)
    snapshot = _build_and_save_snapshot(record, environ=env)
    entry = snapshot.entries[0]
    config_store.write_user_defaults(dict(route_id=entry.route.id, route_connection_id=record.id,
        route_model=entry.route.model, parameters={'thought_level': 'high'}, future={'owner': 'keep'}), PolicyCeiling(), env)
    agent, transport = _new_agent(tmp_path, env)
    project = tmp_path / 'project'
    project.mkdir()
    sid = agent._session_new(dict(cwd=str(project), mcpServers=[]))['sessionId']
    agent._session_set_config_option(1, dict(sessionId=sid, configId='thought_level', value='low'))
    assert 'error' not in transport.responses[-1]
    saved = config_store.read_user_defaults(env)['defaults']
    assert saved['parameters'] == {'thought_level': 'low'} and saved['future'] == {'owner': 'keep'}
    assert agent.sessions[sid]['pending_selection']['parameters'] == {'thought_level': 'low'}
