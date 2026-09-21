from pathlib import Path
import json
import subprocess
import tomllib
import pytest

from cortex_package import (
    PAYLOAD_ENTRIES,
    PLUGIN,
    ROOT,
    paseo_plugin_payload_digest,
    payload_digest,
    payload_paths,
    validate,
)
from generate_agent_profiles import (
    check as check_agent_profiles,
    expected_agent_references,
    expected_profiles,
    expected_worker_references,
)


def test_stamped_package_and_profiles():
    assert validate().startswith('1.0.0+sha256.')
    assert len(list((PLUGIN/'agents').glob('*.toml')))==23
    payload=json.loads((PLUGIN/'runtime-payload.json').read_text())['files']
    assert all((PLUGIN/path).is_file() for path in payload)
    expected_runtime={
        Path(path).name for path in payload
        if Path(path).parent==Path('src/cortex_runtime') and path.endswith('.py')
    }
    assert {p.name for p in (PLUGIN/'src/cortex_runtime').glob('*.py')}==expected_runtime


def test_paseo_provider_is_part_of_release_payload_and_has_integrity_stamp():
    assert 'integrations' in PAYLOAD_ENTRIES
    provider = PLUGIN / 'integrations' / 'paseo'
    assert provider.is_dir()
    assert (provider / 'plugin-integrity.json').is_file()
    names = {relative for relative, _path in payload_paths(PLUGIN)}
    assert 'integrations/paseo/plugin-integrity.json' in names


def test_paseo_provider_waits_for_initial_acp_commands():
    """The native bridge must not cache an empty slash-command list."""
    source = (PLUGIN / 'integrations' / 'paseo' / 'server' / 'provider.ts').read_text()
    assert 'waitForInitialCommands: true' in source
    assert 'initialCommandsTimeoutMs: 2000' in source


def test_paseo_provider_repairs_only_null_failed_timeline_errors():
    """Paseo 0.8 must be able to reload canceled failed tool cards."""
    source = (PLUGIN / 'integrations' / 'paseo' / 'server' / 'provider.ts').read_text()
    assert 'event.item.status === "failed"' in source
    assert 'event.item.error === null' in source
    assert 'error: `Tool call failed: ${event.item.name}`' in source
    # The fallback must remain plain text for the compact Paseo card, and the
    # source must preserve already supplied provider errors.
    assert 'return event;' in source


def test_paseo_provider_replays_history_after_session_opened():
    """Historical timeline events must not be dropped before session.opened."""
    source = (PLUGIN / 'integrations' / 'paseo' / 'server' / 'provider.ts').read_text()
    assert 'MAX_BUFFERED_TIMELINE_ITEMS = 4096' in source
    assert 'MAX_BUFFERED_TIMELINE_BYTES = 16 * 1024 * 1024' in source
    assert 'const bufferedTimeline = new Map<string, ProviderEvent[]>()' in source
    assert 'event.type === "timeline.item" && !openedSessions.has(event.sessionId)' in source
    assert 'for (const historical of pending ?? [])' in source
    assert 'listener(event);' in source
    assert 'bufferedTimeline.clear();' in source
    assert 'bufferedTimelineBytes.clear();' in source
    assert 'event.type === "session.closed" || event.type === "session.runtime_failed"' in source



def test_acp_profiles_keep_roles_and_use_mcp_task_documents():
    check_agent_profiles()
    assert len(expected_profiles()) == 23
    assert len(expected_agent_references()) == 3
    assert len(expected_worker_references()) == 69
    for path,body in expected_profiles().items():
        assert path.read_bytes() == body
        instructions=tomllib.loads(body.decode())['developer_instructions']
        assert len(instructions)<12900
        assert 'structured result' in instructions
        assert 'bounded report' in instructions
        assert 'runtime' in instructions.lower()
        assert '.codex/plugins/' not in instructions
        if path.stem=='senior-consultant':
            assert 'Do not run commands, tests or project checks' in instructions
        else:
            assert '## Specialist workflow' in instructions



def test_pipeline_publication_is_runtime_owned():
    source=(PLUGIN/'src/cortex_runtime/orchestration/pipeline.py').read_text()
    assert 'create_draft' in source and 'write_report' in source
    reference=(PLUGIN/'skills/orchestrator/references/pipeline-publication.md').read_text()
    assert 'runtime' in reference.lower()


def test_coordinator_cannot_accept_or_publish_pipeline_through_channel():
    from cortex_runtime.delegation.contracts import schemas_for
    from cortex_runtime.channeled_tools import channeled_base_tool_names
    assert not {'decide','complete_task','delegate','set_task_state'} & set(schemas_for('coordinator'))
    assert not {'create_task','set_governance'} & set(channeled_base_tool_names())



def test_source_check_is_read_only():
    before=payload_digest(PLUGIN)
    result=subprocess.run([str(ROOT/'scripts/sync-cortex.sh'),'--check'],capture_output=True,text=True)
    assert result.returncode==0 and payload_digest(PLUGIN)==before


def test_payload_enumeration_excludes_local_python_bytecode():
    paths = [relative for relative, _path in payload_paths(PLUGIN)]
    assert all('__pycache__' not in path for path in paths)
    assert all(not path.endswith(('.pyc', '.pyo')) for path in paths)


def test_payload_enumeration_prunes_dependencies_and_build_output_but_keeps_sources(tmp_path):
    paseo = tmp_path / 'integrations' / 'paseo'
    (paseo / 'client').mkdir(parents=True)
    (paseo / 'node_modules' / '.bin').mkdir(parents=True)
    (paseo / 'dist').mkdir()
    (paseo / 'build').mkdir()
    (paseo / 'client' / 'worker.ts').write_text('export {}\n')
    (paseo / 'node_modules' / 'sdk.js').write_text('dependency\n')
    (paseo / 'dist' / 'bundle.js').write_text('generated\n')
    (paseo / 'build' / 'bundle.js').write_text('generated\n')
    # The actual checkout has npm bin symlinks; an excluded dependency tree
    # must not make release enumeration fail on them.
    (paseo / 'node_modules' / '.bin' / 'sdk').symlink_to('../sdk.js')

    names = {relative for relative, _path in payload_paths(tmp_path)}
    assert 'integrations/paseo/client/worker.ts' in names
    assert not any('/node_modules/' in name for name in names)
    assert not any('/dist/' in name or '/build/' in name for name in names)


def test_paseo_integrity_digest_ignores_local_dependency_tree(tmp_path):
    paseo = tmp_path / 'paseo'
    (paseo / 'server').mkdir(parents=True)
    (paseo / 'node_modules').mkdir()
    source = paseo / 'server' / 'provider.ts'
    dependency = paseo / 'node_modules' / 'sdk.js'
    source.write_text('export const provider = true;\n')
    dependency.write_text('dependency-v1\n')
    first = paseo_plugin_payload_digest(paseo)
    dependency.write_text('dependency-v2\n')
    assert paseo_plugin_payload_digest(paseo) == first
    source.write_text('export const provider = false;\n')
    assert paseo_plugin_payload_digest(paseo) != first



def test_worker_protocol_describes_evidence_without_native_authority():
    worker=(PLUGIN/'agent-sources/worker-protocol.md').read_text()
    assert 'These role instructions are delivered' in worker
    assert 'Unrun, failed, truncated and unverifiable checks remain explicit.' in worker
    assert 'Discover tools through their live schemas' in worker
    assert 'request_key' not in worker



def test_evidence_protocol_is_runtime_owned_and_receipt_bound():
    from cortex_runtime.orchestration.contracts import RESULT
    assert RESULT['properties']['checks']['items']['properties']['outcome']['enum']==['passed','failed','not_run','unverified']
    assert 'report_ref' in RESULT['required']
    assert 'checks' in RESULT['required']
    assert 'verdict' in RESULT['properties']



def test_coordinator_handoffs_use_retained_runtime_results():
    from cortex_runtime.delegation.contracts import schemas_for
    payload=json.loads((PLUGIN/'runtime-payload.json').read_text())['files']
    assert 'src/cortex_runtime/orchestration/controller.py' in payload
    assert 'task_get_context' in schemas_for('coordinator')
    assert 'result_submit' in schemas_for('worker')
    assert 'publish_result' not in schemas_for('worker')
    package_version=json.loads((PLUGIN/'package-identity.json').read_text())['version']
    assert package_version.startswith('1.0.0+sha256.')
    assert payload_digest(PLUGIN).startswith(package_version.rsplit('.',1)[-1])



def test_sync_cannot_install_without_isolated_entrypoint():
    import os
    env=dict(os.environ);env.pop('CORTEX_DEV_OWNER_HOME',None)
    result=subprocess.run([str(ROOT/'scripts/sync-cortex.sh')],capture_output=True,text=True,env=env)
    assert result.returncode!=0



def test_markdown_local_links():
    import re
    paths=[ROOT/'README.md',ROOT/'SECURITY.md',ROOT/'PRIVACY.md',
           *(ROOT/'docs').rglob('*.md'),
           *list((PLUGIN/'skills').rglob('*.md')),*list((PLUGIN/'agents').glob('*.toml'))]
    for path in paths:
        for link in re.findall(r'\]\(([^)]+)\)',path.read_text()):
            if '://' in link or link.startswith('#'):continue
            assert (path.parent/link.split('#')[0]).exists(),(path,link)



def test_orchestrator_routes_conditional_procedures_with_valid_links():
    import re
    root = PLUGIN/'skills/orchestrator'
    body = (root/'SKILL.md').read_text()
    # Keep the source compact while allowing the explicit owner/
    # reconciliation branch, and (T035) the rule about what never reaches
    # the user: watching a live session, every coordinator reply opened
    # with "Using the Cortex Orchestrator to hand the implementation to a
    # worker, publish the assignment and check the result" -- machinery the
    # user is already inside and never asked about. Saying that plainly
    # costs a few hundred characters and is worth them; this guard exists
    # to stop drift, not to price out a rule the product needs. D-T035-04
    # adds the one rule that makes the delivered per-role route table
    # usable: that choosing the worker's provider/model/effort is the
    # coordinator's own job.
    # Raised again for the rule a live run showed missing: the
    # coordinator told the user "принимаю результат" and called nothing,
    # so the task stayed unaccepted while the user believed it was done.
    # Raised once more for the ordering a live run proved the coordinator
    # cannot infer: a reviewer delegated before integration reported the
    # implementation "not present", because it was looking at the project
    # and the change was still in another worker's copy.
    assert len(body) < 8600
    links = re.findall(r'\]\((references/[^)]+)\)', body)
    assert len(set(links)) >= 7
    for link in links:
        assert (root/link).is_file()
    routing = (root/'references/worker-routing.md').read_text()
    for profile in json.loads((PLUGIN/'profiles.json').read_text())['profiles']:
        assert '`'+profile['name']+'`' in routing



def test_completion_requires_runtime_review_and_verification():
    from cortex_runtime.orchestration.state import TRANSITIONS
    assert TRANSITIONS['EXECUTING']=={'REVIEWING','WAITING_USER'}
    assert 'COMPLETED' not in TRANSITIONS['REVIEWING']
    assert 'COMPLETED' not in TRANSITIONS['VERIFYING']
    source=(PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    assert 'runtime' in source.lower()
    assert 'decide' not in source



def test_shared_worker_protocol_routes_rare_interactive_procedure():
    protocol=(PLUGIN/'agent-sources/worker-protocol.md').read_text()
    assert 'interactive-resources' in protocol
    reference=(PLUGIN/'agent-sources/references/interactive-resources.md').read_text()
    assert 'select, create or reuse only owned resources' in reference



def test_acp_instruction_boundaries_cover_observed_failures():
    discipline=(PLUGIN/'skills/tool-discipline/SKILL.md').read_text()
    assert 'Reuse retained results while relevant state is unchanged.' in discipline
    assert 'never replay a mutation for reassurance' in discipline
    assert 'native tools are disabled' in discipline
    assert len(discipline)<4000
    publication=(PLUGIN/'agent-sources/references/report-publication.md').read_text()
    assert 'immutable report' in publication
    assert 'does not replace the structured result' in publication



def test_packaged_skills_deliver_profiles_and_progressive_references():
    from generate_agent_profiles import (
        expected_agent_references,
        expected_skills,
        expected_worker_references,
    )
    skills=expected_skills()
    agent_references=expected_agent_references()
    references=expected_worker_references()
    assert len(skills)==23
    assert len(agent_references)==3
    assert len(references)==69
    assert all(path.read_bytes()==body for path,body in agent_references.items())
    assert all(path.read_bytes()==body for path,body in references.items())
    for path,body in skills.items():
        assert path.read_bytes()==body
        name=path.parent.name.removeprefix('worker-')
        profile=tomllib.loads((PLUGIN/'agents'/f'{name}.toml').read_text())
        skill_body=body.decode().split('---\n',2)[2].lstrip()
        profile_body=skill_body.removesuffix('\n<!-- END OF COMPLETE CORTEX WORKER SKILL -->\n')
        assert profile_body==profile['developer_instructions']



def test_prompt_and_skill_editing_subagents_use_the_explicit_sol_medium_override():
    instructions = (ROOT / 'AGENTS.md').read_text()
    assert 'Any subagent assigned to edit prompts or skills must still use only `gpt-5.6-sol` at' in ' '.join(instructions.split())
    assert '`medium` reasoning effort' in ' '.join(instructions.split())
    protocol = (PLUGIN / 'agent-sources/worker-protocol.md').read_text()
    assert 'These role instructions are delivered' in protocol
    assert 'through the advertised skill reader' in protocol



def test_write_report_description_matches_artifact_schema():
    from cortex_runtime.contracts import BY_NAME
    declaration=BY_NAME['write_report']
    artifacts=declaration['inputSchema']['properties']['artifacts']['items']
    assert set(artifacts['required'])=={'reference','version'}
    assert 'draft_write' in declaration['description']
    assert 'result_submit' in declaration['description']
