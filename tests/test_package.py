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
    assert all(path.read_bytes() == body for path, body in expected_profiles().items())
    assert len(expected_agent_references())==3
    assert len(expected_worker_references())==69
    for path in (PLUGIN/'agents').glob('*.toml'):
        profile=tomllib.loads(path.read_text())
        instructions=profile['developer_instructions']
        headings={
            line[3:] for line in instructions.splitlines() if line.startswith('## ')
        }
        if profile['name'] == 'senior_consultant':
            assert '## Access boundary' in instructions
            assert 'Do not run commands, tests or project checks' in instructions
            assert 'read_skill("report-publication")' in instructions
            assert 'You may investigate, implement' not in instructions
            continue
        assert {
            'Role and responsibility',
            'Assignment contract',
            'Evidence and verification',
            'Report and handoff',
            'Report class selection',
            'Specialist workflow',
            'Quality criteria',
            'Recovery',
        } <= headings
        # Keep the complete role within the package's instruction budget.
        assert len(instructions) < 12_900
        assert 'Read assigned published reports through bounded' in instructions
        assert 'page of at most 4,000 characters' not in instructions
        # References are named for read_skill, not resolved as project paths.
        assert 'read_skill("report-publication")' in instructions
        assert 'read_skill("code-and-evidence")' in instructions
        assert 'read_skill("interactive-resources")' in instructions
        assert '](references/' not in instructions
        assert 'Private Cortex evidence has a strict boundary:' in instructions
        assert '`read_report`' in instructions
        assert 'Missing evidence is a stated gap/impact' in instructions
        assert 'first and only general catalogue query' not in instructions
        assert 'final `__`-delimited segment' not in instructions
        assert '.codex/plugins/' not in instructions
    control=(PLUGIN/'skills/cortex-control/SKILL.md').read_text()
    assert '[report example catalogue](references/index.md)' in control
    publication=(PLUGIN/'agent-sources/references/report-publication.md').read_text()
    assert 'read its complete live declaration' in publication
    assert 'Do not probe with speculative calls' in publication
    assert 'request_key' not in publication
    assert 'UUID' not in publication
    orchestrator=(PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    assert 'Delegate discovery AND requested files (notes included)' in orchestrator
    assert 'missing output to that worker; never create it yourself' in orchestrator
    pipeline_publication=(PLUGIN/'skills/orchestrator/references/pipeline-publication.md').read_text()
    assert 'Follow each operation\'s advertised\nretry contract' in pipeline_publication
    assert 'request_key' not in pipeline_publication
    assert 'ordered `replaceable_markers` list as authoritative' in pipeline_publication
    assert 'replace every exact\nlisted marker in place' in pipeline_publication
    assert 'coordinator publishes a current\n`pipeline` edition, while a worker publishes its own non-pipeline report' in pipeline_publication
    assert 'keep this same draft, correct every exact remaining\nmarker in place' in pipeline_publication
    assert 'draft or replay an acknowledged publication' in pipeline_publication



def test_coordinator_publication_guidance_closes_observed_draft_failure():
    orchestrator=(PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    reference=(PLUGIN/'skills/orchestrator/references/pipeline-publication.md').read_text()
    assert '(references/pipeline-publication.md)' in orchestrator
    assert reference.index('Before `write_report`') < reference.index('If `write_report` returns the deterministic')
    assert 'verify that no listed marker (or other exact template\nplaceholder) remains' in reference


def test_pre_accept_pipeline_guidance_describes_durable_decision_basis():
    orchestrator=(PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    reference=(PLUGIN/'skills/orchestrator/references/pipeline-publication.md').read_text()
    for guidance in (orchestrator, reference):
        normalized=' '.join(guidance.split())
        assert 'factual basis' in normalized
        assert 'durable task decision and state' in normalized.lower()
        assert 'Do not claim acceptance before the `decide` receipt' in normalized or (
            'Do not say acceptance was already recorded before the `decide` receipt' in normalized
        )
        assert 'anticipated acceptance' not in normalized



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



def test_worker_safety_and_post_wait_rules_are_payload_guidance():
    worker = (PLUGIN/'agent-sources/worker-protocol.md').read_text()
    orchestrator = (PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    assert 'PYTHONDONTWRITEBYTECODE=1' in worker
    for rule in ('rm -rf', 'find ... -delete', 'git clean', 'reset/checkout', 'recursive cleanup'):
        assert rule in worker
    assert 'Checks `PYTHONDONTWRITEBYTECODE=1`' in worker
    assert 'These role instructions are delivered' in worker
    assert 'most specific native tool' in worker
    assert 'avoid Python/ad-hoc parsing' in ' '.join(worker.split())
    consultant = (PLUGIN/'agent-sources/consultant-protocol.md').read_text()
    assert 'most specific suitable native tool' in consultant
    assert 'fallback is valid otherwise' in consultant
    assert 'read_skill` for the named situational guidance' in worker
    assert 'one bounded command per wrapper' in worker
    assert "`publish_result` hands off only this worker's own current assignment report" in worker
    assert 'other-worker report evidence\nonly in the saved report' in worker
    assert 'request_key' not in worker
    assert 'Timeout alone never replaces its active owner' in orchestrator
    assert 'Reconcile outcomes against published reports and public evidence' in orchestrator



def test_p0_p1_evidence_protocol_remains_model_owned_and_receipt_bound():
    orchestrator = (PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    worker = (PLUGIN/'agent-sources/worker-protocol.md').read_text()
    pipeline = (PLUGIN/'report-templates/pipeline.md').read_text()
    assert 'implementation_state' in orchestrator
    assert 'delivery_state' in orchestrator
    assert 'acceptance_state' in orchestrator
    assert ' '.join(orchestrator.split()).find(
        'CI, reports and commands prove only their own boundary') >= 0
    assert '(artifact_revision, acceptance_boundary, check_identity)' in orchestrator
    assert 'basis, uncertainty, and\ndisconfirmation' in worker
    assert 'receipt-backed' in worker
    assert 'rollout/review/recheck/' in worker
    assert "Distinguish implementation and observed delivery from the evidence supporting the coordinator's task decision" in pipeline



def test_coordinator_reads_worker_handoffs_through_acp_events():
    orchestrator = (PLUGIN/'skills/orchestrator/references/worker-routing.md').read_text()
    payload = json.loads((PLUGIN/'runtime-payload.json').read_text())['files']
    # What this used to require was the plugin harness's own vocabulary --
    # `collaboration.spawn_agent`, Codex app threads -- none of which
    # exists in an ACP session, checked against the shipped engines on
    # 2026-09-19. It also told the coordinator that "native final
    # responses deliver the handoff automatically", which is the same
    # falsehood the worker protocol carried and the reason workers ended
    # their turns having published a report and said nothing.
    assert 'you read them with `list_events`' in orchestrator
    assert 'Nothing about a handoff is automatic' in orchestrator
    assert '`publish_result`' in orchestrator
    assert 'collaboration.' not in orchestrator
    assert 'create_thread' not in orchestrator
    assert 'src/cortex_runtime/store.py' in payload
    assert (PLUGIN/'skills/orchestrator/SKILL.md').is_file()
    package_version = json.loads((PLUGIN/'package-identity.json').read_text())['version']
    assert package_version.startswith('1.0.0+sha256.')
    assert payload_digest(PLUGIN).startswith(package_version.rsplit('.', 1)[-1])
    for path, body in expected_profiles().items():
        if path.stem == 'senior-consultant':
            continue
        # The rule this replaced forbade `codex_app.send_message_to_thread`,
        # a surface of the plugin harness T033 removed -- checked against
        # the shipped engine on 2026-09-19, codex-acp 1.12.0 does not
        # contain that name at all. It cost every worker profile a
        # paragraph to forbid something that cannot happen, while what a
        # worker actually gets wrong went unsaid: the same profiles told it
        # "final handoffs are automatic", and workers duly ended their
        # turns having published a report and said nothing.
        assert 'Nothing about\nyour handoff is automatic' in body.decode()
        assert 'publish_result' in body.decode()



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



def test_ordinary_completion_documentation_branches_are_semantic_and_conditional():
    def completion_text(source):
        start = source.index('5. Before ordinary acceptance')
        end = source.index('\n\nFor production', start)
        return ' '.join(source[start:end].split())

    def assert_contract(completion):
        related = "Include related tests and affected documentation in the implementing worker's assignment when practical"
        sync = 'that worker applies `documentation-sync` (readable with `read_skill`) and reports its changes and proportionate verification'
        separate = 'Assign a separate technical writer when documentation is independent work or needs distinct ownership'
        read_only = 'It may inspect bounded project source and public evidence read-only to reconcile the worker\'s report and cited revisions'
        uncertain = 'If impact or verification remains uncertain, gather the missing evidence or keep acceptance open'
        no_impact = 'A supported no-impact conclusion needs no token edit or extra worker'
        acceptance = 'Accepting is `decide`, and only `decide`'
        clauses = (related, sync, separate, read_only, uncertain, no_impact,
                   'Then accept the observed outcome', acceptance)
        assert all(clause in completion for clause in clauses)
        assert [completion.index(clause) for clause in clauses] == sorted(
            completion.index(clause) for clause in clauses)
        assert 'missing inspection is not evidence of no impact' in completion
        assert "The coordinator does not read another role's `SKILL.md`" in completion
        assert 'coordinator does not read another role\'s `SKILL.md`, edit project files or rerun project checks' in completion
        assert 'Keep `cortex:knowledge-harvest` explicit-only: this review is not a broad census' in completion
        assert 'model-owned/advisory decisions are not runtime gates' in completion
        assert 'For every ordinary task, apply `documentation-sync`' not in completion

    source = (PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    completion = completion_text(source)
    assert_contract(completion)

    # Branch polarity fixtures: these mutations must be rejected by the contract.
    unconditional = completion.replace(
        'Include related tests and affected documentation',
        'For every ordinary task, apply `documentation-sync`. Include related tests and affected documentation',
        1)
    with pytest.raises(AssertionError):
        assert_contract(unconditional)
    weakened_hold = completion.replace(
        'gather the missing evidence or keep acceptance open',
        'continue to acceptance without documentation evidence', 1)
    with pytest.raises(AssertionError):
        assert_contract(weakened_hold)
    mandatory_writer = completion.replace(
        'Assign a separate technical writer when documentation is independent work or needs distinct ownership',
        'Assign a separate technical writer for every documentation change', 1)
    with pytest.raises(AssertionError):
        assert_contract(mandatory_writer)
    coordinator_writes = completion.replace(
        'edit project files or rerun project checks',
        'edit project files and rerun project checks', 1)
    with pytest.raises(AssertionError):
        assert_contract(coordinator_writes)
    no_read = completion.replace(
        'It may inspect bounded project source and public evidence read-only',
        'It may inspect only report summaries', 1)
    with pytest.raises(AssertionError):
        assert_contract(no_read)
    automatic_harvest = completion.replace(
        'Keep `cortex:knowledge-harvest` explicit-only: this review is not a broad census',
        '`cortex:knowledge-harvest` runs after every ordinary task', 1)
    with pytest.raises(AssertionError):
        assert_contract(automatic_harvest)

    documentation_sync = ' '.join((PLUGIN/'skills/documentation-sync/SKILL.md').read_text().split())
    assert "include related documentation in the implementing worker's assignment when practical" in documentation_sync
    assert 'Use a separate technical writer when the documentation is independent work or needs distinct ownership' in documentation_sync
    assert 'requires the affected updates and proportionate verification before acceptance' in documentation_sync
    assert 'The coordinator may inspect bounded project source, report evidence and public artifacts read-only' in documentation_sync
    assert 'the coordinator does not rerun project checks' in documentation_sync



def test_shared_worker_protocol_routes_rare_interactive_procedure():
    protocol=(PLUGIN/'agent-sources/worker-protocol.md').read_text()
    reference=(PLUGIN/'agent-sources/references/interactive-resources.md').read_text()
    # By name, not by path: references are delivered by the Cortex skill reader.
    assert 'read_skill("interactive-resources")' in protocol
    assert 'Never inspect\nglobal processes, ports, tabs or devices' in reference
    assert 'select, create or reuse only owned resources' in reference
    assert 'Stop or close only sessions created by this assignment.' in reference
    assert 'Accessibility element numbers belong only to the snapshot' not in protocol



def test_acp_instruction_boundaries_cover_observed_failures():
    coordinator=(PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    communication=(PLUGIN/'skills/coordinator-communication/SKILL.md').read_text()
    discipline=(PLUGIN/'skills/tool-discipline/SKILL.md').read_text()
    publication=(PLUGIN/'agent-sources/references/report-publication.md').read_text()
    assert "Use the user's language" in coordinator
    assert "user's latest own prose" in communication
    assert 'Reuse retained results while relevant state is unchanged.' in discipline
    assert 'Re-read after user steering' in discipline
    assert 'A wrapper must expose the full nested' in discipline
    assert 'never replay a mutation for reassurance' in discipline
    # The coordinator rule was delivered in the observed live run. Workers
    # received only a catalog pointer to tool-discipline, so the same rule
    # must also appear in each worker's first-turn role instructions.
    assert "Never use your engine's own\n   subagent or wait tools" in coordinator
    assert 'Your engine offers subagent and spawn tools of its own. Never use them.' in discipline
    assert 'A worker delegates nothing at all' in discipline
    worker_rule = "Do not use your engine's native subagent or spawn tools. A worker delegates nothing"
    assert worker_rule in ' '.join((PLUGIN / 'agent-sources/worker-protocol.md').read_text().split())
    for path in sorted((PLUGIN / 'skills').glob('worker-*/SKILL.md')):
        body = ' '.join(path.read_text().split())
        if path.name == 'SKILL.md' and path.parent.name == 'worker-senior-consultant':
            assert 'create subagents' in body, path
        else:
            assert worker_rule in body, path
    assert 'Use the draft reader only after compaction, restart' in publication
    assert 'Correct deterministic edit or argument errors once' in publication
    assert 'After successful publication, call `publish_result`' in publication
    assert len(discipline) < 4_000



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
    assert 'read_skill` for the named situational guidance' in protocol



def test_write_report_description_matches_artifact_schema():
    source=(ROOT/'src/cortex_runtime/contracts.py').read_text()
    assert 'Metadata arrays use the advertised item schemas' in source
    assert 'Never put Markdown bodies or shell interpolation' in source
    assert 'writer metadata, arrays, chunks' not in source
