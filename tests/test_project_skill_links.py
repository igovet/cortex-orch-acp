"""project_skill_link: the one narrow write into .claude.

A writing worker may expose a project skill in .agents/skills/<name> to Claude
as the relative link .claude/skills/<name> -> ../../.agents/skills/<name>.
Every other .claude path stays private to the generic file tools.
"""
import os
import shutil
import subprocess

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration import consent, gateway, skill_links
from cortex_runtime.orchestration.gateway import dispatch
from cortex_runtime.orchestration.state import validate_plan
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture

TARGET = '../../.agents/skills/release-notes'


@pytest.fixture
def project(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='skill-links', event_id='one'), 'Link skills', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, _ = provision_fixture(store, coordinator)
    return store, coordinator, worker, tmp_path


def _skill(root, name='release-notes', body='---\nname: release-notes\n---\n'):
    directory = root / '.agents' / 'skills' / name
    directory.mkdir(parents=True)
    (directory / 'SKILL.md').write_text(body)
    return directory


def _link(store, channel, key, **args):
    return dispatch(store, channel, 'project_skill_link', dict(dict(action='link', name='release-notes'), request_key=key, **args))


def _refused(store, channel, code, key, **args):
    with pytest.raises(StoreError) as error:
        dispatch(store, channel, 'project_skill_link', dict(dict(action='link', name='release-notes'), request_key=key, **args))
    assert str(error.value) == code
    return error.value


def test_link_creates_the_exact_relative_link_and_is_idempotent(project):
    store, _, worker, root = project
    _skill(root)
    receipt = _link(store, worker, 'one')
    link = root / '.claude' / 'skills' / 'release-notes'
    assert link.is_symlink() and os.readlink(link) == TARGET
    assert (link / 'SKILL.md').read_text().startswith('---\nname: release-notes')
    assert (root / '.claude').is_dir() and not (root / '.claude').is_symlink()
    assert {k: receipt[k] for k in ('action', 'path', 'target', 'changed', 'previous')} == dict(
        action='link', path='.claude/skills/release-notes', target=TARGET, changed=True, previous=None)
    # The exact retry replays the receipt; a new request leaves the link as is.
    assert _link(store, worker, 'one') == receipt
    assert _link(store, worker, 'two')['changed'] is False
    assert os.readlink(link) == TARGET


@pytest.mark.parametrize('name', ['Release', 'a/b', '../x', '-a', 'a-', 'a--b', '.hidden', 'a_b', 'x' * 65])
def test_names_outside_the_skill_name_rule_are_refused(project, name):
    store, _, worker, root = project
    with pytest.raises(StoreError, match='skill_link_name_invalid|invalid_arguments'):
        dispatch(store, worker, 'project_skill_link', dict(action='link', name=name, request_key='n'))
    assert not (root / '.claude').exists()


def test_the_source_must_be_a_real_in_project_skill_directory(project, tmp_path_factory):
    store, _, worker, root = project
    _refused(store, worker, 'skill_source_missing', 'absent')
    source = root / '.agents' / 'skills' / 'release-notes'
    source.mkdir(parents=True)
    _refused(store, worker, 'skill_source_missing', 'no-skill-md')
    outside = tmp_path_factory.mktemp('outside')
    (outside / 'SKILL.md').write_text('outside')
    (source / 'SKILL.md').symlink_to(outside / 'SKILL.md')
    _refused(store, worker, 'skill_source_missing', 'linked-skill-md')
    source.joinpath('SKILL.md').unlink(); source.rmdir()
    source.symlink_to(outside, target_is_directory=True)
    _refused(store, worker, 'skill_source_missing', 'linked-source')
    source.unlink(); (root / '.agents' / 'skills').rmdir(); (root / '.agents').rmdir()
    (root / '.agents').symlink_to(outside, target_is_directory=True)
    _refused(store, worker, 'skill_source_missing', 'linked-agents')
    assert not (root / '.claude').exists()


def test_a_symlinked_or_non_directory_claude_parent_is_never_followed(project, tmp_path_factory):
    store, _, worker, root = project
    _skill(root)
    outside = tmp_path_factory.mktemp('shared-claude')
    (root / '.claude').symlink_to(outside, target_is_directory=True)
    _refused(store, worker, 'skill_link_parent_unsafe', 'linked-claude')
    assert list(outside.iterdir()) == []
    (root / '.claude').unlink(); (root / '.claude').mkdir()
    (root / '.claude' / 'skills').write_text('not a directory')
    _refused(store, worker, 'skill_link_parent_unsafe', 'file-skills')
    assert (root / '.claude' / 'skills').read_text() == 'not a directory'


@pytest.mark.parametrize('kind', ['file', 'directory'])
def test_an_existing_file_or_directory_is_never_replaced(project, monkeypatch, kind):
    store, _, worker, root = project
    _skill(root)
    monkeypatch.setattr(consent, 'request', lambda *a, **k: pytest.fail('no confirmation for a collision'))
    entry = root / '.claude' / 'skills' / 'release-notes'
    entry.parent.mkdir(parents=True)
    if kind == 'file':
        entry.write_text('claude-only skill')
    else:
        entry.mkdir(); (entry / 'SKILL.md').write_text('claude-only skill')
    _refused(store, worker, 'skill_link_collision', 'plain')
    _refused(store, worker, 'skill_link_conflict', 'forced', replace_symlink_target='anything')
    assert (entry if kind == 'file' else entry / 'SKILL.md').read_text() == 'claude-only skill'
    assert not entry.is_symlink()


def test_a_stale_link_is_replaced_only_with_its_exact_text_and_the_users_confirmation(project, monkeypatch):
    store, _, worker, root = project
    _skill(root)
    link = root / '.claude' / 'skills' / 'release-notes'
    link.parent.mkdir(parents=True)
    link.symlink_to('../../.agents/skills/old-name')  # dangling: a renamed skill
    asked = []
    monkeypatch.setattr(consent, 'request', lambda store, channel, kind, subject, **k: asked.append((kind, subject)) or False)
    error = _refused(store, worker, 'skill_link_occupied', 'plain')
    assert error.received == '../../.agents/skills/old-name'
    _refused(store, worker, 'skill_link_conflict', 'wrong', replace_symlink_target='../../.agents/skills/other')
    assert asked == []  # a wrong text never reaches the user
    _refused(store, worker, 'skill_link_replace_not_confirmed', 'denied', replace_symlink_target='../../.agents/skills/old-name')
    assert asked == [('project_skill_link', '.claude/skills/release-notes -> ../../.agents/skills/old-name becomes -> ' + TARGET)]
    assert os.readlink(link) == '../../.agents/skills/old-name'
    monkeypatch.setattr(consent, 'request', lambda *a, **k: True)
    receipt = _link(store, worker, 'allowed', replace_symlink_target='../../.agents/skills/old-name')
    assert receipt['changed'] is True and receipt['previous'] == dict(kind='symlink', target='../../.agents/skills/old-name')
    assert os.readlink(link) == TARGET
    assert sorted(p.name for p in link.parent.iterdir()) == ['release-notes']  # no temporary left


def test_unlink_removes_only_the_exact_managed_link(project):
    store, _, worker, root = project
    source = _skill(root)
    args = dict(action='unlink', name='release-notes')
    assert dispatch(store, worker, 'project_skill_link', dict(args, request_key='u0'))['changed'] is False
    _link(store, worker, 'link')
    link = root / '.claude' / 'skills' / 'release-notes'
    receipt = dispatch(store, worker, 'project_skill_link', dict(args, request_key='u1'))
    assert receipt['changed'] is True and not os.path.lexists(link)
    assert (source / 'SKILL.md').exists()
    link.symlink_to('../../elsewhere')
    with pytest.raises(StoreError, match='skill_link_not_managed'):
        dispatch(store, worker, 'project_skill_link', dict(args, request_key='u2'))
    assert os.readlink(link) == '../../elsewhere'
    link.unlink(); link.mkdir()
    with pytest.raises(StoreError, match='skill_link_not_managed'):
        dispatch(store, worker, 'project_skill_link', dict(args, request_key='u3'))
    assert link.is_dir()
    with pytest.raises(StoreError, match='invalid_arguments'):
        dispatch(store, worker, 'project_skill_link', dict(args, replace_symlink_target=TARGET, request_key='u4'))


def test_only_a_writing_worker_may_link_and_a_user_denial_wins(project, monkeypatch):
    store, coordinator, worker, root = project
    _skill(root)
    reader, _ = provision_fixture(store, coordinator, key='reader', role='explorer')
    for channel in (coordinator, reader):
        _refused(store, channel, 'gateway_scope_denied', 'no-grant')
    names = lambda **kw: {t['name'] for t in mcp_tools.tool_definitions(**kw)}
    assert 'project_skill_link' not in names(role_kind='coordinator')
    assert 'project_skill_link' not in names(role_kind='worker', role_id='worker-explorer', stage='work')
    assert 'project_skill_link' not in names(role_kind='worker', role_id='worker-backend-dev', stage='review')
    assert 'project_skill_link' in names(role_kind='worker', role_id='worker-backend-dev', stage='work')
    monkeypatch.setattr(gateway, 'write_denials', lambda db, task_id: ['.claude/skills/**'])
    _refused(store, worker, 'gateway_scope_denied', 'denied')
    assert not (root / '.claude').exists()


def test_every_other_claude_path_stays_private_to_generic_tools(project):
    store, _, worker, root = project
    _skill(root)
    _link(store, worker, 'link')
    for operation, args in [
            ('project_write', dict(path='.claude/skills/release-notes/SKILL.md', content='x', expected_sha256='absent')),
            ('project_write', dict(path='.claude/settings.json', content='{}', expected_sha256='absent')),
            ('project_mkdir', dict(path='.claude/skills/other')),
            ('project_delete', dict(path='.claude/skills/release-notes', expected_sha256='0' * 64))]:
        with pytest.raises(StoreError, match='gateway_path_denied'):
            dispatch(store, worker, operation, dict(args, request_key=operation + args['path']))
    with pytest.raises(StoreError, match='gateway_path_denied'):
        dispatch(store, worker, 'project_read', dict(path='.claude/skills'))
    with pytest.raises(StoreError, match='forbidden_assignment_scope'):
        validate_plan(dict(tasks=[dict(id='t', role='worker-backend-dev', objective='Link.', dependencies=[],
                                       scope=['.claude/skills/**'])], verification=[['true']]))
    assert not (root / '.claude' / 'settings.json').exists()


@pytest.mark.parametrize('parents', ['absent', 'present'])
def test_a_failure_after_the_link_may_exist_is_fenced_as_uncertain(project, monkeypatch, parents):
    store, _, worker, root = project
    _skill(root)
    if parents == 'present':
        (root / '.claude' / 'skills').mkdir(parents=True)
    def fail(fd):
        raise OSError(5, 'simulated fsync failure')
    monkeypatch.setattr(skill_links.os, 'fsync', fail)
    with pytest.raises(StoreError) as error:
        _link(store, worker, 'uncertain')
    assert str(error.value) == 'storage_error'
    monkeypatch.undo()
    with pytest.raises(StoreError, match='gateway_action_outcome_unknown'):
        _link(store, worker, 'uncertain')


@pytest.mark.skipif(shutil.which('git') is None, reason='git is not installed')
def test_git_keeps_the_link_as_a_portable_symlink(project, tmp_path_factory):
    store, _, worker, root = project
    git = lambda *argv, cwd=root: subprocess.run(['git', '-c', 'user.name=t', '-c', 'user.email=t@example.invalid',
                                                  '-c', 'core.hooksPath=/dev/null', *argv],
                                                 cwd=cwd, check=True, capture_output=True, text=True).stdout
    git('init', '-q')
    (root / '.gitignore').write_text('.cortex/\n')
    _skill(root)
    receipt = _link(store, worker, 'link')
    assert receipt['git']['link_ignored'] is False and receipt['git']['source_ignored'] is False
    git('add', '.gitignore', '.agents', '.claude/skills/release-notes')
    git('commit', '-q', '-m', 'skill')
    mode, blob = git('ls-files', '-s', '.claude/skills/release-notes').split()[:2]
    assert mode == '120000' and git('cat-file', 'blob', blob) == TARGET
    clone = tmp_path_factory.mktemp('clone') / 'copy'
    git('clone', '-q', str(root), str(clone))
    assert (clone / '.claude' / 'skills' / 'release-notes' / 'SKILL.md').read_text().startswith('---\nname: release-notes')
    # An ignore rule that would drop the link from the repository is reported.
    (root / '.gitignore').write_text('.cortex/\n.claude/\n')
    (root / '.claude' / 'skills' / 'release-notes').unlink()
    assert _link(store, worker, 'relink')['git']['link_ignored'] is True
