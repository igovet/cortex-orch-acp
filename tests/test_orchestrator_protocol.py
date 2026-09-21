"""Guard the coordinator's fresh-activation bootstrap ordering contract."""

from pathlib import Path

from cortex_runtime.delegation.mcp_tools import tool_definitions
from cortex_runtime.portable.roles import load_role


ROOT = Path(__file__).resolve().parents[1]


def test_the_bound_task_is_confirmed_before_governance():
    """The ordering this has always protected -- a task binding is
    confirmed before governance is applied -- is unchanged. What changed
    (T035) is who binds: in an ACP client the session creates and binds
    the task before the coordinator's first turn, so the coordinator
    calling `create_task` itself is refused every time, and every live run
    spent its opening words explaining that refusal to the user.
    """
    text = (ROOT / "skills/orchestrator/SKILL.md").read_text()

    start = text.index("## Apply governance to decisions")
    end = text.index("\n## Durable decisions", start)
    section = text[start:end]

    assert "One session is one task" in section
    assert "never call\n`create_task`" in section
    assert "confirm the bound task" in section
    assert section.index("bound task") < section.index("`set_governance`")
    assert "never called before that binding is confirmed" in section
    assert "not a server gate or approval machine" in section


def test_harvest_remains_explicit_only_in_orchestrator_contract():
    text = (ROOT / "skills/orchestrator/SKILL.md").read_text()
    assert "Keep `cortex:knowledge-harvest` explicit-only" in text


def test_explicit_harvest_can_be_the_first_project_request():
    coordinator = (ROOT / "skills/orchestrator/SKILL.md").read_text()
    harvest = (ROOT / "skills/knowledge-harvest/SKILL.md").read_text()
    assert "Bare `/harvest` (also as a first prompt)" in coordinator
    assert "Optional text specifies scope" in coordinator
    assert "`/harvest-refresh` updates it against current source" in coordinator
    assert "Neither command requires a prior task" in coordinator
    assert "Bare\n`/harvest`, including as the first prompt" in harvest
    assert "Optional command text narrows or clarifies its scope" in harvest
    assert "`/harvest-refresh` asks Cortex to update" in harvest
    assert "durable, source-backed project documentation" in harvest


def test_small_harvest_has_one_worker_path_and_refresh_rechecks_source():
    coordinator = (ROOT / "skills/orchestrator/SKILL.md").read_text()
    harvest = (ROOT / "skills/knowledge-harvest/SKILL.md").read_text()
    census = (ROOT / "skills/knowledge-harvest/references/feature-census.md").read_text()

    assert "Small projects may use one writer for inventory, docs and checks" in coordinator
    assert "one `technical_writer` worker for the whole\nharvest or refresh" in harvest
    assert "run relevant deterministic checks" in harvest
    assert "when a concrete unresolved risk warrants it" in harvest
    assert "Audit\n  existing project and feature documents against that inventory" in harvest
    assert "Do not substitute chat or report summaries" in census
    assert "post-write pages and manifest with the fresh source inventory" in census
    assert "Every completed harvest contains all five" in harvest


def test_semantic_harvest_refresh_searches_examples_and_gets_separate_review():
    coordinator = (ROOT / "skills/orchestrator/SKILL.md").read_text()
    harvest = (ROOT / "skills/knowledge-harvest/SKILL.md").read_text()
    census = (ROOT / "skills/knowledge-harvest/references/feature-census.md").read_text()

    assert len(coordinator.encode()) <= 8600
    normalized = " ".join(coordinator.split())
    assert "Corrected behavior/default/output claims need" in coordinator
    assert "complete relevant post-write documentation/example inventory" in normalized
    assert "safe exact exit/stdout comparison and separate source/docs review even without source changes" in normalized
    assert "Low-risk factual refreshes can use one writer" in coordinator
    assert "every relevant occurrence across the target repository's documentation" in " ".join(harvest.split())
    assert "commands, flags, example inputs and expected output" in " ".join(harvest.split())
    assert "Confirm each claim in current source" in " ".join(harvest.split())
    assert "separate `code_reviewer` for these changed documentation claims" in " ".join(harvest.split())
    assert "Inventory every affected executable example" in harvest
    assert "compare exact exit status and\nstdout" in harvest
    assert "Mark examples that cannot safely\nrun" in harvest
    assert "without relying on the writer's claim that a search was\nclean" in harvest
    assert "A purely factual, low-risk refresh may stay with one" in harvest
    for text in (harvest, census):
        assert "every relevant occurrence across the target repository's" in text
        assert "even if source is unchanged" in text
        assert "safe example-result comparison" in text
        assert "separate worker's\n  source-backed review" in text


def test_visible_ui_defaults_to_independent_review_with_bounded_exception():
    coordinator = (ROOT / "skills/orchestrator/SKILL.md").read_text()
    governance = (ROOT / "skills/orchestrator/references/governance.md").read_text()
    routing = (ROOT / "skills/orchestrator/references/worker-routing.md").read_text()

    assert "A user-visible UI implementation is nontrivial" in coordinator
    assert "unless a concrete low-risk\n   exception is recorded" in coordinator
    assert "Implementer or coordinator checks are never independent review" in coordinator
    assert "pipeline records a concrete low-risk\n  rationale tied to its acceptance boundary" in governance
    assert "Implementer and coordinator checks do not satisfy that independent\n  ownership" in governance
    assert "pipeline records a concrete low-risk\nrationale tied to its acceptance boundary" in routing
    assert "Implementer and coordinator checks are never independent review" in routing


def test_pipeline_and_final_copy_durable_route_and_evidence_facts():
    coordinator = (ROOT / "skills/orchestrator/SKILL.md").read_text()
    routing = (ROOT / "skills/orchestrator/references/worker-routing.md").read_text()
    publication = (ROOT / "skills/orchestrator/references/pipeline-publication.md").read_text()
    communication = (ROOT / "skills/coordinator-communication/SKILL.md").read_text()

    assert "observed route facts exactly from live receipts or the current capsule" in " ".join(coordinator.split())
    assert "never reconstruct them from a brief, planned route or model prose" in " ".join(coordinator.split())
    assert "route requested in `delegate` or repeated in its brief is a plan, not execution evidence" in " ".join(routing.split())
    assert "exact `observed_worker_route`" in routing
    assert "planned `delegate`\nroute is not execution evidence" in publication
    assert "omit unavailable facts instead of reconstructing planned values" in " ".join(communication.split())


def test_later_tasks_discover_harvested_docs_with_provenance_and_selective_reads():
    coordinator = (ROOT / "skills/orchestrator/SKILL.md").read_text()
    routing = (ROOT / "skills/orchestrator/references/worker-routing.md").read_text()
    for text in (coordinator, routing):
        assert "project-document catalog" in text
        assert "revision and provenance" in text
        assert "docs/project/index.md" in text
        assert "docs/features/index.md" in text
        assert "relevant" in text
    assert "page content is not automatically loaded" in coordinator
    assert "retain that diagnostic" in coordinator


def test_coordinator_can_discover_the_user_question_route():
    role_text = load_role("orchestrator").body_portable
    assert "`decide` on the `cortex` MCP" in role_text
    assert "Ask through it and leave the task open for the answer" in role_text
    assert "leave the task open for the answer" in role_text
    assert "no separate coordinator `ask_question` tool" in role_text

    coordinator_tools = {tool["name"]: tool for tool in tool_definitions("coordinator")}
    assert "decide" in coordinator_tools
    assert "ask_user" in coordinator_tools["decide"]["description"]
    assert "question" in coordinator_tools["decide"]["description"]
    assert "ask_question" not in coordinator_tools


def test_task_acceptance_has_a_prior_pipeline_edition_and_incident_is_conditional():
    text = (ROOT / "skills/orchestrator/SKILL.md").read_text()
    section = text.split("## Durable decisions", 1)[1].split("\n## Honest communication", 1)[0]
    assert "Before task acceptance via `decide`, publish the current pipeline edition" in section
    normalized = " ".join(section.split())
    assert "rationale, assignment/report refs, artifact/check evidence" in normalized
    assert "remaining work and the factual basis for your intended decision" in normalized
    assert 'Do not claim acceptance before the `decide` receipt' in normalized
    assert 'Keep lasting decision basis, not transient acceptance status' in normalized
    assert 'remaining work excludes `decide`' in normalized
    assert "Durable task decision and state show the outcome" in normalized
    assert "Correct the edition only if the decision changes" in normalized
    assert "For a production incident, keep the brief" in section


def test_pipeline_template_and_publication_guidance_survive_task_decision():
    template = (ROOT / "report-templates/pipeline.md").read_text()
    reference = (ROOT / "skills/orchestrator/references/pipeline-publication.md").read_text()
    assert template.startswith("# Pipeline decision basis\n")
    assert "Do not put a momentary task status or an acceptance step here" in template
    assert "## Decisions, alternatives, discriminating checks, and unresolved deliverable work" in template
    assert "Replace the template's objective/decision-basis marker with the objective and\ndecision basis" in reference
    assert "Remaining work means unresolved\ndeliverables or verification, not the `decide` call itself" in reference
    assert "The edition must remain true after `decide`" in reference
    assert "durable task decision and state for the recorded outcome" in reference


def test_a_running_worker_is_waited_out_silently():
    """The rule is unchanged -- say nothing until something actually
    happens. What changed (T035) is that it no longer instructs a "bounded
    native wait": the engine's own wait tool is exactly what the
    coordinator must not use, because it comes with the engine's own
    subagents, which bypass the isolated workspace, the conflict-checked
    integration and the audit. A Cortex worker's progress arrives on its
    own; there is nothing to poll.
    """
    text = (ROOT / "skills/orchestrator/SKILL.md").read_text()

    start = text.index("## Honest communication")
    end = text.index("\n## Load only relevant details", start)
    section = text[start:end]

    assert "While a worker is running, say nothing until" in section
    assert "material state transition" in section
    assert "native" not in section, "the engine's own wait must not be instructed"
