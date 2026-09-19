from pathlib import Path
import json
import subprocess
import tomllib
import pytest

from cortex_package import PLUGIN, ROOT, payload_digest, validate
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



def test_release_readiness_keeps_retained_host_runs_historical_and_superseded():
    readiness = (ROOT/'docs/release-readiness.md').read_text()
    historical = (
        'Both retained final host runs belong to superseded payload `e4f332d43bf38024`,\n'
        'used its unchanged isolated candidate with Luna coordination/workers and no stable\n'
        'installation or configuration change, and are historical-only.'
    )
    assert historical in readiness
    assert ('they do not qualify current payload\n'
            '`07c3aeae551a990e`.') in readiness
    assert 'Both final host runs used the unchanged current candidate' not in readiness



def test_mcp_advertises_only_the_observation_environment():
    mcp = json.loads((PLUGIN/'.mcp.json').read_text())['mcpServers']['cortex']
    assert set(mcp['env_vars']) == {'CORTEX_OBSERVATION_DIR'}


def test_native_profiles_keep_roles_and_use_mcp_task_documents():
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
            assert 'references/report-publication.md' in instructions
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
        # Raised once, for the one rule that makes a worker's isolation
        # something it understands rather than something it discovers by
        # being refused: work in your own copy, the project's real paths
        # are not yours. Live, a worker edited the project directly and
        # its change reached the coordinator with no integration and no
        # check at all. This guard exists to stop drift, not to price out
        # a rule the product needs.
        assert len(instructions) < 12_900
        assert 'page of at most\n4,000 characters' in instructions
        assert 'not a total context limit' in instructions
        assert 'references/report-publication.md' in instructions
        assert 'references/code-and-evidence.md' in instructions
        assert 'references/interactive-resources.md' in instructions
        assert 'Private Cortex evidence has a strict boundary:' in instructions
        assert 'mcp__cortex__read_report' in instructions
        assert 'Missing evidence is a stated gap/impact' in instructions
        assert 'first and only general catalogue query' not in instructions
        assert 'final `__`-delimited segment' not in instructions
        assert '.codex/plugins/' not in instructions
    control=(PLUGIN/'skills/cortex-control/SKILL.md').read_text()
    assert '[report example catalogue](references/index.md)' in control
    publication=(PLUGIN/'agent-sources/references/report-publication.md').read_text()
    assert 'crypto.randomUUID()' in publication
    assert 'pass a literal UUID in the tool arguments' in publication
    assert 'supply every required field, including on the initial call' in publication
    assert 'Never probe required\nfields with an empty argument object' in publication
    assert 'Omit `request_key` for' not in publication
    orchestrator=(PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    assert 'Delegate discovery AND requested files (notes included)' in orchestrator
    assert 'missing output to that worker; never create it yourself' in orchestrator
    pipeline_publication=(PLUGIN/'skills/orchestrator/references/pipeline-publication.md').read_text()
    assert 'When a pipeline mutation schema requires `request_key`' in pipeline_publication
    assert 'ordered `replaceable_markers` list as authoritative' in pipeline_publication
    assert 'replace every exact\nlisted marker in place' in pipeline_publication
    assert 'coordinator publishes a current\n`pipeline` edition, while a worker publishes its own non-pipeline report' in pipeline_publication
    assert 'same draft and the original request key\nand metadata' in pipeline_publication
    assert 'draft or replay an acknowledged publication' in pipeline_publication



def test_coordinator_publication_guidance_closes_observed_draft_failure():
    orchestrator=(PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    reference=(PLUGIN/'skills/orchestrator/references/pipeline-publication.md').read_text()
    assert '(references/pipeline-publication.md)' in orchestrator
    assert reference.index('Before `write_report`') < reference.index('If `write_report` returns the deterministic')
    assert 'verify that no listed marker (or other exact template\nplaceholder) remains' in reference



def test_source_check_is_read_only():
    before=payload_digest(PLUGIN)
    result=subprocess.run([str(ROOT/'scripts/sync-cortex.sh'),'--check'],capture_output=True,text=True)
    assert result.returncode==0 and payload_digest(PLUGIN)==before



def test_worker_safety_and_post_wait_rules_are_payload_guidance():
    worker = (PLUGIN/'agent-sources/worker-protocol.md').read_text()
    orchestrator = (PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    assert 'PYTHONDONTWRITEBYTECODE=1' in worker
    for rule in ('rm -rf', 'find ... -delete', 'git clean', 'reset/checkout', 'recursive cleanup'):
        assert rule in worker
    assert 'Checks `PYTHONDONTWRITEBYTECODE=1`' in worker
    assert 'Before project action' in worker
    assert 'most specific native tool' in worker
    assert 'avoid Python/ad-hoc parsing' in ' '.join(worker.split())
    consultant = (PLUGIN/'agent-sources/consultant-protocol.md').read_text()
    assert 'most specific suitable native tool' in consultant
    assert 'fallback is valid otherwise' in consultant
    assert 'a coordinator read never satisfies it' in worker
    assert 'one bounded command per wrapper' in worker
    assert "native final names exactly one ID: this worker's own current" in worker
    assert 'other-worker report ID only in the saved report' in worker
    assert 'Timeout alone never replaces its active owner' in orchestrator
    assert 'Reconcile actual outcomes' in orchestrator



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
    assert 'implementation_state, delivery_state, and coordinator-owned acceptance_state' in pipeline



def test_coordinator_native_worker_tracking_is_distinct_from_app_task_management():
    orchestrator = (PLUGIN/'skills/orchestrator/references/worker-routing.md').read_text()
    payload = json.loads((PLUGIN/'runtime-payload.json').read_text())['files']
    rule = (
        'Native subagents spawned through `collaboration.spawn_agent` are tracked only with\n'
        'the native collaboration controls: `collaboration.wait_agent`,\n'
        '`collaboration.list_agents`, `collaboration.send_message` and\n'
        '`collaboration.followup_task`.'
    )
    assert rule in orchestrator
    assert '`create_thread`, `read_thread`, `wait_threads` or `send_message_to_thread`' in orchestrator
    assert 'reserved for explicit user-owned\ntask management, not worker coordination.' in orchestrator
    assert 'src/cortex_runtime/store.py' in payload
    assert (PLUGIN/'skills/orchestrator/SKILL.md').is_file()
    package_version = json.loads((PLUGIN/'package-identity.json').read_text())['version']
    assert package_version.startswith('1.0.0+sha256.')
    assert payload_digest(PLUGIN).startswith(package_version.rsplit('.', 1)[-1])
    for path, body in expected_profiles().items():
        if path.stem == 'senior-consultant':
            continue
        assert 'Never discover,\ncall or request approval for `codex_app.send_message_to_thread`' in body.decode()



def test_sync_cannot_install_without_isolated_entrypoint():
    import os
    env=dict(os.environ);env.pop('CORTEX_DEV_OWNER_HOME',None)
    result=subprocess.run([str(ROOT/'scripts/sync-cortex.sh')],capture_output=True,text=True,env=env)
    assert result.returncode!=0



def test_markdown_local_links():
    import re
    # T033: docs/project/verification.md is a frozen historical evidence
    # record (out of scope for this task -- see INVARIANTS.md) that cites
    # test files by their exact name at the time of that verification pass,
    # including several Codex-plugin-harness-era test files removed at
    # T033. Its links are expected to go stale as history accumulates; it
    # is intentionally excluded here rather than edited.
    paths=[ROOT/'README.md',ROOT/'SECURITY.md',ROOT/'PRIVACY.md',
           *(p for p in (ROOT/'docs').rglob('*.md') if p!=ROOT/'docs/project/verification.md'),
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
    import re

    def completion_text(source):
        start = source.index('5. Before ordinary acceptance')
        end = source.index('\n\nFor production', start)
        return ' '.join(source[start:end].split())

    def assert_contract(completion):
        positive = re.search(
            r'If impact is supported.*?assign the appropriate documentation owner to apply '
            r'`documentation-sync` \(readable with `read_skill`\); give that owner exact affected documentation '
            r'ownership and require its updates plus proportionate verification before acceptance',
            completion)
        insufficient = re.search(
            r'If inspection is partial or inconclusive, gather bounded missing evidence '
            r'or leave impact unresolved and withhold acceptance until either supported '
            r'`documentation-sync` synchronization completes or sufficient no-impact '
            r'evidence exists', completion)
        no_impact = re.search(
            r'If sufficient evidence supports no impact, record a concise no-impact '
            r'conclusion without token edits or an unnecessary worker', completion)
        owner = re.search(
            r'assign the appropriate documentation owner to apply '
            r'`documentation-sync`', completion)
        reconcile = re.search(
            r'After the owner.s published documentation report, only Cortex '
            r'report/public-evidence reads may compare its cited documentation hashes'
            r'/check results before acceptance', completion)
        delegated = re.search(
            r'delegate any additional project inspection, file read/hash or command '
            r'verification to an appropriate worker; return evidence in a task-bound '
            r'report; the coordinator must not rerun project checks', completion)
        accept = completion.index('Then accept the observed outcome')
        assert positive and owner and reconcile and delegated and insufficient and no_impact
        assert positive.start() < owner.start() < reconcile.start() < delegated.start() < insufficient.start() < no_impact.start() < accept
        assert 'missing inspection is not evidence of no impact' in completion
        assert "The coordinator does not read another role's `SKILL.md`" in completion
        assert re.search(
            r'Keep `cortex:knowledge-harvest` explicit-only: this review is not a broad census',
            completion)
        assert 'model-owned/advisory decisions are not runtime gates' in completion
        assert not re.search(
            r'\b(?:for every ordinary task|regardless of impact|without regard to impact|always)\b'
            r'.*?`documentation-sync`',
            completion,
            re.IGNORECASE)

    source = (PLUGIN/'skills/orchestrator/SKILL.md').read_text()
    completion = completion_text(source)
    assert_contract(completion)

    # Branch polarity fixtures: these mutations must be rejected by the contract.
    unconditional = completion.replace(
        'If impact is supported, assign the appropriate documentation owner to apply',
        'For every ordinary task, apply `documentation-sync`. '
        'If impact is supported, assign the appropriate documentation owner to apply',
        1)
    with pytest.raises(AssertionError):
        assert_contract(unconditional)
    weakened_hold = completion.replace(
        'withhold acceptance until either supported `documentation-sync` '
        'synchronization completes or sufficient no-impact evidence exists',
        'continue to acceptance without documentation evidence', 1)
    with pytest.raises(AssertionError):
        assert_contract(weakened_hold)
    direct_reconciliation = completion.replace(
        'only Cortex report/public-evidence reads may compare its cited documentation hashes'
        '/check results',
        'read project files and rerun project checks', 1)
    with pytest.raises(AssertionError):
        assert_contract(direct_reconciliation)
    direct_verification = completion.replace(
        'delegate any additional project inspection, file read/hash or command verification '
        'to an appropriate worker; return evidence in a task-bound report; the coordinator '
        'must not rerun project checks',
        'may inspect files and rerun project checks directly', 1)
    with pytest.raises(AssertionError):
        assert_contract(direct_verification)
    automatic_harvest = completion.replace(
        'Keep `cortex:knowledge-harvest` explicit-only: this review is not a broad census',
        '`cortex:knowledge-harvest` runs after every ordinary task', 1)
    with pytest.raises(AssertionError):
        assert_contract(automatic_harvest)

    documentation_sync = ' '.join((PLUGIN/'skills/documentation-sync/SKILL.md').read_text().split())
    assert 'evidence-backed impact review' in documentation_sync
    assert 'If inspection is partial or inconclusive' in documentation_sync
    assert 'acceptance waits for synchronization or sufficient no-impact evidence' in documentation_sync
    assert 'post-owner reconciliation' in documentation_sync
    assert 'the coordinator must not rerun project checks itself' in documentation_sync



def test_shared_worker_protocol_routes_rare_interactive_procedure():
    protocol=(PLUGIN/'agent-sources/worker-protocol.md').read_text()
    reference=(PLUGIN/'agent-sources/references/interactive-resources.md').read_text()
    assert '[interactive resources](references/interactive-resources.md)' in protocol
    assert 'Never inspect\nglobal processes, ports, tabs or devices' in reference
    assert 'select, create or reuse only owned resources' in reference
    assert 'Stop or close only sessions created by this assignment.' in reference
    assert 'Accessibility element numbers belong only to the snapshot' not in protocol



def test_native_instruction_boundaries_cover_observed_live_failures():
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
    # Found live: the coordinator was told not to use its engine's own
    # subagents, workers never were, and the runtime can only report such
    # a call after the fact -- it cannot prevent it. Every role gets the
    # rule, which is why it lives in the skill both roles always load.
    assert 'Your engine offers subagent and spawn tools of its own. Never use them.' in discipline
    assert 'A worker delegates nothing at all' in discipline
    assert 'Use the draft reader only after compaction, restart' in publication
    assert 'Correct deterministic edit or argument errors once' in publication
    assert 'successful publication is the final tool action' in publication
    assert len(discipline) < 4_000



def test_marketplace_skills_deliver_profiles_and_progressive_references():
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
    assert 'Any subagent assigned to edit prompts or skills must use only `gpt-5.6-sol` with' in instructions
    assert '`medium` reasoning effort' in instructions
    protocol = (PLUGIN / 'agent-sources/worker-protocol.md').read_text()
    assert 'directly declared Markdown reference leaves may then be read' in protocol
    assert 'never count as the initial\nworker-skill receipt' in protocol



def test_write_report_description_matches_artifact_schema():
    source=(ROOT/'src/cortex_runtime/contracts.py').read_text()
    assert 'Metadata arrays use the advertised item schemas' in source
    assert 'Never put Markdown bodies or shell interpolation' in source
    assert 'writer metadata, arrays, chunks' not in source
