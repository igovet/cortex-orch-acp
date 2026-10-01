"""No-paid regressions for ACP layer ingestion and actual automatic dispatch.

Equivalent assertions to the retained causal report; no historical task/log
fixture is reused. Catalogs, owner policy, credentials and usage are all fake.
"""
from dataclasses import replace
import json

import pytest

from cortex_runtime.catalog import store as catalog_store
from cortex_runtime.configuration import store as config_store
from cortex_runtime.configuration.policy import FallbackPolicy, PermissionLevel, PolicyCeiling, RouteProfile
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.routing import auto_route, store as routing_store
from model_policy_fixtures import exact_policy
from test_acp_config_options import _isolate, _configured_connection, _observation_raw, _new_agent
from test_auto_route_delegation import _make_connection, _snapshot, _bootstrap, _FakeConfigAdapter
from engine_fakes import isolated_environ


class _ConfigBoundary:
    def __init__(self):
        self.parameters = []

    def configure(self, session_id, revision):
        self.parameters.append(revision['parameters'])
        return 'fake-confirmed-configuration'


@pytest.mark.parametrize('selection', ['pending', 'default', 'portable'])
@pytest.mark.parametrize('change', ['effort', 'route'])
def test_acp_first_prompt_and_mid_task_keep_disk_project_task_ceiling(tmp_path, monkeypatch, selection, change):
    env = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=env)
    from cortex_runtime.catalog.discovery import build_catalog, parse_observation
    raw = _observation_raw('model-a', 'Model A', 'low', ('low', 'high'))
    raw[0]['options'].append(dict(value='model-b', name='Model B'))
    snapshot = build_catalog({record.id: record}, {record.id: parse_observation(raw)})
    catalog_store.save_snapshot(snapshot, env)
    entry = next(e for e in snapshot.entries if e.route.model == 'model-a')
    owner = PolicyCeiling(allowed_routes={e.route.id for e in snapshot.entries}, allowed_connections={record.id},
                         allowed_data_recipients={record.provider}, permission_level=PermissionLevel.STANDARD,
                         limits={'attempts': 10, 'calls': 20, 'tokens': 100, 'concurrency': 4},
                         spend_limit=100, spend_limit_enforceable=True,
                         fallback_policy=FallbackPolicy(allowed_target_routes={entry.route.id},
                             allowed_failure_classes={'engine_failure'}, budget=3, data_recipient_consent=True))
    project_ceiling = replace(owner, limits={'attempts': 8, 'calls': 15, 'tokens': 90, 'concurrency': 3},
                              spend_limit=80, fallback_policy=replace(owner.fallback_policy, budget=2))
    defaults = dict(route_id=entry.route.id, route_connection_id=record.id, route_model='model-a',
                    parameters={'thought_level': 'low'}, language='en')
    if selection == 'portable':
        defaults = dict(coordinator=dict(engine=record.engine, model='model-a', parameters={'thought_level': 'low'}), language='en')
    config_store.write_user_defaults(defaults, owner, env)
    agent, transport = _new_agent(tmp_path, env)
    project = tmp_path / 'project'
    project.mkdir()
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))['sessionId']
    session = agent.sessions[session_id]
    store = agent._store_for(session['project'])
    config_store.write_project_defaults(store, session['project'], {'parameters': {'thought_level': 'high'}}, project_ceiling)
    if selection == 'pending':
        agent._session_set_config_option(1, dict(sessionId=session_id, configId='model', value='model-a'))
    disk = config_store.read_user_defaults(env)
    assert disk['ceiling'] == owner
    assert session.get('pending_selection') or session.get('default_selection')
    # Real first prompt bootstraps and selects; only the model turn is a no-op.
    agent._session_prompt(2, dict(sessionId=session_id, prompt=[dict(type='text', text='run')]))
    agent._workers[-1].join(timeout=5)
    assert not agent._workers[-1].is_alive()
    service = ConfigurationService(store)
    first = service.history(session['task_id'])[-1]
    assert first['state'] == 'selected'
    assert first['ceiling_snapshot'] == project_ceiling
    boundary = _ConfigBoundary()
    active = service.apply(session['task_id'], first['id'], boundary, 'es_' + 'a' * 12, session['owner_epoch'])
    assert active['state'] == 'active' and active['ceiling_snapshot'] == project_ceiling
    assert set(boundary.parameters[-1]) <= {'thought_level', 'mode'}
    # Add a task-local narrowing via the real service, then change effort.
    task_ceiling = replace(project_ceiling, permission_level=PermissionLevel.READ_ONLY,
                           limits={'attempts': 6, 'calls': 12, 'tokens': 70, 'concurrency': 2}, spend_limit=60,
                           fallback_policy=replace(project_ceiling.fallback_policy, budget=1))
    row = service.propose(session['task_id'], entry, ConfigurationInputs(route=entry.route, ceiling_override=task_ceiling),
                          first['revision'], session['owner_epoch'], user_layer=disk,
                          project_layer=config_store.read_project_defaults(store, session['project']))
    service.apply(session['task_id'], row['id'], boundary, 'es_' + 'a' * 12, session['owner_epoch'])
    config_id, value = ('model', 'model-b') if change == 'route' else ('thought_level', 'high')
    agent._session_set_config_option(3, dict(sessionId=session_id, configId=config_id, value=value))
    selected = service.history(session['task_id'])[-1]
    assert selected['revision'] == 3 and selected['ceiling_snapshot'] == task_ceiling
    active = service.apply(session['task_id'], selected['id'], boundary, 'es_' + 'a' * 12, session['owner_epoch'])
    assert active['state'] == 'active' and active['ceiling_snapshot'] == task_ceiling
    if change == 'effort':
        assert boundary.parameters[-1]['thought_level'] == 'high'
    else:
        assert active['route_id'] == next(e.route.id for e in snapshot.entries if e.route.model == 'model-b')
    assert set(boundary.parameters[-1]) <= {'thought_level', 'mode'}
    # No owner file rewrite on mid-task selection.
    assert config_store.read_user_defaults(env) == disk
    assert session['pending_selection'] is None and session['default_selection'] is None


@pytest.fixture
def automatic_fixture(tmp_path, monkeypatch):
    store, task_id, channel, epoch, project = _bootstrap(tmp_path)
    env = isolated_environ(tmp_path)
    a = _make_connection(tmp_path, env, engine='codex-acp', model='ceiling-demo-a', request_log=tmp_path/'a.log')
    b = _make_connection(tmp_path, env, engine='claude-agent-acp', model='ceiling-demo-b', request_log=tmp_path/'b.log')
    a, b = replace(a, provider='vendor-a'), replace(b, provider='vendor-b')
    from cortex_runtime.connections import registry
    with registry.transaction(env) as state:
        state.connections.update({a.id: a, b.id: b})
    snapshots = [_snapshot(a, 'ceiling-demo-a'), _snapshot(b, 'ceiling-demo-b')]
    for snapshot in snapshots:
        catalog_store.save_snapshot(snapshot, env)
    entries = [s.entries[0] for s in snapshots]
    rules = exact_policy({'codex-acp': {'ceiling-demo-a': 'standard'}, 'claude-agent-acp': {'ceiling-demo-b': 'standard'}},
                         automatic={'codex-acp': ['ceiling-demo-a'], 'claude-agent-acp': ['ceiling-demo-b']})
    rules['engine_affinity'] = {'*': 'claude-agent-acp'}
    path = project/'.cortex'/'routing.json'
    path.write_text(json.dumps(rules))
    from cortex_runtime.connections import quota
    monkeypatch.setattr(quota, 'used_percent', lambda *args, **kwargs: None)
    adapter = EngineSessionManagerAdapter(lambda t: a, lambda t: str(project), environ=env, store=store)
    calls = {'capacity': 0, 'engines': 0}

    class CapacityReached(Exception):
        pass

    def capacity(*args, **kwargs):
        calls['capacity'] += 1
        raise CapacityReached()

    def engine(*args, **kwargs):
        calls['engines'] += 1
        raise AssertionError('No engine may be constructed in this invariant')

    monkeypatch.setattr(adapter, '_reserve_capacity_blocking', capacity)
    monkeypatch.setattr('cortex_runtime.engine.session_manager.EngineSessionManager', engine)
    return store, task_id, epoch, env, adapter, entries, rules, path, calls, CapacityReached


def _revision(fixture, ceiling=None, profile=None):
    store, task, epoch, env, adapter, entries, rules, path, calls, sentinel = fixture
    service = ConfigurationService(store)
    row = service.propose(task, entries[0], ConfigurationInputs(route=entries[0].route), 0, epoch,
                          user_layer=dict(defaults={}, ceiling=ceiling or PolicyCeiling()), route_profile=profile)
    service.apply(task, row['id'], _FakeConfigAdapter(), 'es_'+'a'*12, epoch)
    return row


@pytest.mark.parametrize('role', ['worker-backend-dev', 'worker-code-reviewer'])
@pytest.mark.parametrize('pin', [False, True])
@pytest.mark.parametrize('allowed', [True, False])
def test_actual_automatic_route_ceiling_before_capacity(automatic_fixture, role, pin, allowed):
    f = automatic_fixture
    store, task, epoch, env, adapter, entries, rules, path, calls, sentinel = f
    ceiling = PolicyCeiling(allowed_routes={e.route.id for e in entries} if allowed else {entries[0].route.id})
    row = _revision(f, ceiling)
    if pin:
        rules['pin'] = {'*': 'claude-agent-acp/ceiling-demo-b:high'}
        path.write_text(json.dumps(rules))
    launch = dict(task_id=task, attempt_id='xa_boundary', assignment_id='asg-boundary', role_id=role, config_revision=row['id'])
    if pin and not allowed:
        with pytest.raises(PortableError) as error:
            adapter.open('shared_project_root', launch, 'fake instructions')
        assert error.value.code == 'role_route_not_authorized'
        assert calls == {'capacity': 0, 'engines': 0}
    else:
        with pytest.raises(sentinel):
            adapter.open('shared_project_root', launch, 'fake instructions')
        expected = entries[1] if allowed else entries[0]
        assert launch['resolved_route_id'] == expected.route.id
        assert launch['resolved_connection_id'] == expected.connection_id
        assert launch['resolved_model'] == expected.route.model
        assert calls == {'capacity': 1, 'engines': 0}


@pytest.mark.parametrize('dimension', ['allowed_routes', 'allowed_connections', 'allowed_data_recipients'])
@pytest.mark.parametrize('value', ['absent', 'null', 'empty'])
def test_automatic_empty_is_denial_not_unrestricted(automatic_fixture, dimension, value):
    f = automatic_fixture
    store, task, epoch, env, adapter, entries, rules, path, calls, sentinel = f
    ceiling = PolicyCeiling(**({} if value == 'absent' else {dimension: None if value == 'null' else []}))
    row = _revision(f, ceiling)
    launch = dict(task_id=task, attempt_id='xa_empty', assignment_id='asg-boundary', role_id='worker-code-reviewer', config_revision=row['id'])
    with pytest.raises(PortableError if value == 'empty' else sentinel):
        adapter.open('shared_project_root', launch, 'fake instructions')
    assert calls == {'capacity': 0 if value == 'empty' else 1, 'engines': 0}


@pytest.mark.parametrize('profile_case', ['missing_role', 'empty_role', 'allow_b', 'only_a'])
def test_explicit_profiles_narrow_automatic_candidates(automatic_fixture, profile_case):
    f = automatic_fixture
    store, task, epoch, env, adapter, entries, rules, path, calls, sentinel = f
    routes = {'missing_role': {'other': {entries[0].route.id}}, 'empty_role': {'worker-code-reviewer': set()},
              'allow_b': {'worker-code-reviewer': {entries[1].route.id}}, 'only_a': {'worker-code-reviewer': {entries[0].route.id}}}[profile_case]
    row = _revision(f, profile=RouteProfile(role_routes=routes))
    launch = dict(task_id=task, attempt_id='xa_profile', assignment_id='asg-boundary', role_id='worker-code-reviewer', config_revision=row['id'])
    with pytest.raises(PortableError if profile_case in ('missing_role', 'empty_role') else sentinel):
        adapter.open('shared_project_root', launch, 'fake instructions')
    if calls['capacity']:
        assert launch['resolved_route_id'] == entries[1 if profile_case == 'allow_b' else 0].route.id
    assert calls['engines'] == 0


@pytest.mark.parametrize('identity', ['missing', 'stale', 'unmatched', 'ambiguous'])
def test_automatic_identity_fails_closed_before_capacity(automatic_fixture, monkeypatch, identity):
    f = automatic_fixture
    store, task, epoch, env, adapter, entries, rules, path, calls, sentinel = f
    row = _revision(f)
    launch = dict(task_id=task, attempt_id='xa_identity', assignment_id='asg-boundary', role_id='worker-backend-dev', config_revision=row['id'])
    original = auto_route.choose

    def changed(*args, **kwargs):
        choice = original(*args, **kwargs)
        if identity == 'missing':
            return replace(choice, route_id=None)
        if identity == 'stale':
            return replace(choice, route_id=entries[0].route.id)
        if identity == 'unmatched':
            return replace(choice, model='not-observed')
        # Simulate an ambiguous current catalog after the real choice.
        original_entries = auto_route._entries
        monkeypatch.setattr(auto_route, '_entries', lambda *a: original_entries(*a) + [entries[1]])
        return choice

    monkeypatch.setattr(auto_route, 'choose', changed)
    with pytest.raises(PortableError) as error:
        adapter.open('shared_project_root', launch, 'fake instructions')
    assert error.value.code == 'automatic_route_identity_unavailable'
    assert calls == {'capacity': 0, 'engines': 0}


@pytest.mark.parametrize('role', ['coordinator', 'worker-backend-dev', 'worker-code-reviewer'])
@pytest.mark.parametrize('allowed', [True, False])
def test_explicit_dispatch_ceiling_controls(automatic_fixture, role, allowed):
    f = automatic_fixture
    store, task, epoch, env, adapter, entries, rules, path, calls, sentinel = f
    # Both the explicit role profile and ceiling permit only A.
    profile = RouteProfile(role_routes={role: {entries[0].route.id}})
    row = _revision(f, PolicyCeiling(allowed_routes={entries[0].route.id}), profile)
    target = entries[0 if allowed else 1]
    launch = dict(task_id=task, attempt_id='xa_explicit', role_id=role, config_revision=row['id'],
                  resolved_route_id=target.route.id, resolved_connection_id=target.connection_id, resolved_model=target.route.model)
    if role != 'coordinator':
        launch['assignment_id'] = 'asg-boundary'
    with pytest.raises(sentinel if allowed else PortableError):
        adapter.open('shared_project_root', launch, 'fake instructions')
    assert calls == {'capacity': 1 if allowed else 0, 'engines': 0}


@pytest.mark.parametrize('change', ['effort', 'route'])
@pytest.mark.parametrize('omit_layers', [True, False])
def test_selection_omission_never_widens_task_or_resets_explicit_profile(automatic_fixture, change, omit_layers):
    f = automatic_fixture
    store, task, epoch, env, adapter, entries, rules, path, calls, sentinel = f
    ceiling = PolicyCeiling(allowed_routes={e.route.id for e in entries},
                            allowed_connections={e.connection_id for e in entries},
                            allowed_data_recipients={'vendor-a', 'vendor-b'}, permission_level=PermissionLevel.READ_ONLY,
                            limits={'attempts': 3, 'calls': 4, 'tokens': 5, 'concurrency': 1},
                            spend_limit=10, spend_limit_enforceable=True,
                            fallback_policy=FallbackPolicy(allowed_target_routes={entries[1].route.id},
                                allowed_failure_classes={'engine_failure'}, budget=1, data_recipient_consent=True))
    profile = RouteProfile(role_routes={'worker-code-reviewer': {entries[1].route.id}})
    service = ConfigurationService(store)
    row = service.propose(task, entries[0], ConfigurationInputs(route=entries[0].route), 0, epoch,
                          user_layer=dict(defaults={}, ceiling=ceiling), route_profile=profile,
                          route_details={entry.route.id: entry.route for entry in entries})
    target = entries[1 if change == 'route' else 0]
    layers = {} if omit_layers else dict(user_layer=dict(defaults={}, ceiling=PolicyCeiling(
        fallback_policy=FallbackPolicy(data_recipient_consent=True))))
    new = service.propose(task, target, ConfigurationInputs(route=target.route, parameters={'thought_level': 'high'}),
                          1, epoch, **layers)
    assert new['ceiling_snapshot'] == ceiling
    assert routing_store.get_route_extras(store, new['id'])['route_profile'] == profile
    assert routing_store.get_route_extras(store, new['id'])['route_details'][entries[1].route.id]['model'] == 'ceiling-demo-b'
    active = service.apply(task, new['id'], _FakeConfigAdapter(), 'es_'+'a'*12, epoch)
    assert active['state'] == 'active' and active['ceiling_snapshot'] == ceiling
    with pytest.raises(PortableError) as error:
        service.propose(task, target, ConfigurationInputs(route=target.route, ceiling_override=PolicyCeiling()), 2, epoch)
    assert error.value.code == 'ceiling_violation'


@pytest.mark.parametrize('pin', [True, False])
@pytest.mark.parametrize('dimension', ['allowed_connections', 'allowed_data_recipients'])
def test_non_route_ceiling_filters_automatic_identity_and_strict_pin(automatic_fixture, dimension, pin):
    f = automatic_fixture
    store, task, epoch, env, adapter, entries, rules, path, calls, sentinel = f
    value = {entries[0].connection_id} if dimension == 'allowed_connections' else {'vendor-a'}
    row = _revision(f, PolicyCeiling(**{dimension: value}))
    if pin:
        rules['pin'] = {'*': 'claude-agent-acp/ceiling-demo-b'}
        path.write_text(json.dumps(rules))
    launch = dict(task_id=task, attempt_id='xa_connection', assignment_id='asg-boundary',
                  role_id='worker-code-reviewer', config_revision=row['id'])
    with pytest.raises(PortableError if pin else sentinel):
        adapter.open('shared_project_root', launch, 'fake instructions')
    if not pin:
        assert launch['resolved_route_id'] == entries[0].route.id
    assert calls == {'capacity': 0 if pin else 1, 'engines': 0}


@pytest.mark.parametrize('defaults', [['not-a-mapping'], {'parameters': ['not-a-mapping']}])
def test_parameter_envelope_validation_is_atomic(automatic_fixture, defaults):
    store, task, epoch, env, adapter, entries, rules, path, calls, sentinel = automatic_fixture
    service = ConfigurationService(store)
    with pytest.raises(PortableError) as error:
        service.propose(task, entries[0], ConfigurationInputs(route=entries[0].route), 0, epoch,
                        user_layer=dict(defaults=defaults, ceiling=PolicyCeiling()))
    assert error.value.code == 'invalid_parameter_defaults'
    assert service.history(task) == ()


@pytest.mark.parametrize('missing', [True, False])
def test_automatic_revision_binding_fails_before_capacity(automatic_fixture, missing):
    f = automatic_fixture
    store, task, epoch, env, adapter, entries, rules, path, calls, sentinel = f
    row = _revision(f)
    launch = dict(task_id=task if missing else 't_'+'b'*12, attempt_id='xa_binding', assignment_id='asg-boundary',
                  role_id='worker-backend-dev', config_revision='cr_'+'c'*12 if missing else row['id'])
    with pytest.raises(PortableError) as error:
        adapter.open('shared_project_root', launch, 'fake instructions')
    assert error.value.code == 'dispatch_configuration_unavailable'
    assert calls == {'capacity': 0, 'engines': 0}
