"""Access-class metadata selects instructions without changing role authorization."""

import json
from pathlib import Path
import shutil

import pytest

from cortex_runtime.orchestration.contracts import READ_ONLY
from generate_agent_profiles import PLUGIN, VERIFICATION_SOURCES, expected_skills


def test_catalogue_access_classes_exactly_match_runtime_read_only_roles():
    profiles = json.loads((PLUGIN / 'profiles.json').read_text())['profiles']
    assert len(profiles) == 23
    assert len({profile['name'] for profile in profiles}) == 23
    assert {profile['access_class'] for profile in profiles} == set(VERIFICATION_SOURCES)
    assert [p['name'] for p in profiles if p['access_class'] == 'consultant'] == ['senior_consultant']
    non_writing = {
        'worker-' + Path(profile['source']).stem
        for profile in profiles if profile['access_class'] != 'write'
    }
    # The runtime also accepts this legacy non-worker alias.
    assert READ_ONLY == non_writing | {'senior-consultant'}


@pytest.fixture
def catalogue(tmp_path):
    shutil.copytree(PLUGIN / 'agent-sources', tmp_path / 'agent-sources')
    profile = json.loads((PLUGIN / 'profiles.json').read_text())['profiles'][0]
    return tmp_path, profile


def render(catalogue, access_class):
    root, profile = catalogue
    if access_class is None:
        profile.pop('access_class', None)
    else:
        profile['access_class'] = access_class
    (root / 'profiles.json').write_text(json.dumps({'profiles': [profile]}))
    return next(iter(expected_skills(root).values())).decode()


@pytest.mark.parametrize('access_class', ['write', 'read_only', 'consultant'])
def test_generator_selects_verification_by_field_not_role_name(catalogue, access_class):
    body = render(catalogue, access_class)
    source = (catalogue[0] / 'agent-sources' / VERIFICATION_SOURCES[access_class]).read_text().strip()
    assert source in body
    assert '{{' not in body
    assert 'structured result' in body
    if access_class == 'write':
        assert 'Verify every behavior your assignment changes' in body
        assert 'Change nothing;' not in body
    elif access_class == 'read_only':
        assert 'Change nothing;' in body
        assert 'Verify every behavior your assignment changes' not in body
    else:
        assert '## Evidence limits' in body
        assert 'Run no commands, tests, installs' in body
        assert '## Evidence and verification' not in body


@pytest.mark.parametrize('access_class', [None, '', 'readonly', 'admin'])
def test_generator_rejects_missing_or_unknown_access_class(catalogue, access_class):
    with pytest.raises(ValueError, match='invalid access class'):
        render(catalogue, access_class)
