"""A model requirement in the project's own instruction file (AGENTS.md or
CLAUDE.md at the project root) counts as the owner's request for the purpose
it names: ahead of the coordinator's hint, pins, owner preferred models and
usage, still below hard owner policy, and never granted by a prohibition."""
from __future__ import annotations

import json
import os
import time
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import append_source_event, bootstrap_task
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import auto_route
from test_user_model_request import HISTORY, _two_engines

OPUS = "claude-agent-acp/opus[1m]"
SONNET = "claude-agent-acp/claude-sonnet-5"
# The model-requirement lines of the project instruction file in the observed
# session (2026-10-23): a re-audit the coordinator hinted to Opus ran on Sonnet.
OBSERVED = ("# Project agent rules\n\n"
            "- All audits and re-audits must be owned and performed by **Claude Opus**. Record the actual "
            "auditor/model and evidence; never relabel another model's work as an Opus audit. Implementation "
            "workers may publish repair/test reports, not substitute them for the independent Opus audit.\n")
AUDITORS = ("worker-security-auditor", "worker-code-reviewer")
OTHERS = ("worker-backend-dev", "worker-technical-writer", "worker-debugger", "worker-devops-engineer")
# A question about a requirement is not one: these granted Opus as the user's
# request once the passive grammar matched them (review, 2026-10-23).
QUESTIONS = ("Does the audit have to be performed by Opus?", "Do reviews need to be done by Opus?",
             "Почему аудит должен проводить Opus?", "The audit has to be performed by Opus? Fine.")


def _project(tmp_path, rules, name="AGENTS.md", message="Сделай аудит текущей версии"):
    store = _two_engines(tmp_path)
    if rules is not None:
        (tmp_path / name).write_text(rules, encoding="utf-8")
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="rules"), message, "owner")
    return store, task


def test_the_observed_rule_requires_opus_for_audit_roles_only(tmp_path):
    store, task = _project(tmp_path, OBSERVED)
    for role in AUDITORS:
        grant, revocation = auto_route.project_instruction_route(store, task.task_id, role)
        assert revocation is None
        assert grant == dict(source="project_instruction", path="AGENTS.md", line=3,
                             source_ref="project_instruction:AGENTS.md:3", family=OPUS, matched="Opus")
    for role in OTHERS:
        assert auto_route.project_instruction_route(store, task.task_id, role) == (None, None)
    # The file is not a user message: user consent stays as it was.
    assert auto_route.user_model_request(store, task.task_id, OPUS) is None


def test_the_passive_requirement_is_also_a_user_request():
    pattern = auto_route.family_pattern(OPUS, ("opus", "опус"))
    for text in ("All audits and re-audits must be owned and performed by Claude Opus.",
                 "Reviews should always be done by Opus.", "Аудит должен проводить Opus.",
                 "Аудиты должны выполнять только Claude Opus."):
        assert auto_route.assigns_family(text, pattern) == "Opus"
    for text in ("The previous audit was performed by Opus.", "Audits must not be performed by Opus.",
                 "Аудит не должен проводить Opus.", "Opus performs the security audit.", *QUESTIONS,
                 "Why does the audit have to be performed by Opus?", "Разве аудит должен проводить Opus?",
                 "Does the audit have to be performed by Opus", "Аудит ли должен проводить Opus"):
        assert auto_route.assigns_family(text, pattern) is None


@pytest.mark.parametrize("question", QUESTIONS)
def test_a_question_is_never_a_request(tmp_path, question):
    store, task = _project(tmp_path, question, message=question)
    assert auto_route.user_model_request(store, task.task_id, OPUS) is None
    for role in AUDITORS:
        assert auto_route.user_purpose_model_request(store, task.task_id, role) is None
        assert auto_route.project_instruction_route(store, task.task_id, role) == (None, None)


def test_claude_md_counts_and_other_files_never_do(tmp_path):
    store, task = _project(tmp_path, "Audits must be performed by Opus.\n", name="CLAUDE.md")
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor")[0]["path"] == "CLAUDE.md"
    other = tmp_path / "other"
    other.mkdir()
    store, task = _project(other, None)
    (other / "docs").mkdir()
    (other / "docs" / "AGENTS.md").write_text("Audits must be performed by Opus.\n", encoding="utf-8")
    (other / "README.md").write_text("Audits must be performed by Opus.\n", encoding="utf-8")
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor") == (None, None)
    os.symlink(other / "README.md", other / "AGENTS.md")
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor") == (None, None)


@pytest.mark.parametrize("rules", [
    "Heavy models are prohibited in routine work.",
    "Opus is prohibited for audits.",
    "Audits must not be performed by Opus.",
    "Never use Opus. Audits must be performed by Opus.",
    "Opus is prohibited for audits. Audits must be performed by Opus.",
    "Do not run an Opus review. Audits must be performed by Opus.",
    "Opus reviews are prohibited. Audits must be performed by Opus.",
    "Use only Opus for prompt/skill work.",          # names no role purpose
    "The previous audit was performed by Opus.",     # a statement, not a requirement
    "Opus wrote the last audit.",
    "Reviews must be done by Sonnet. Audits must be performed by Opus.",  # two models for one purpose
    # The model a sentence requires for another purpose is not this role's.
    "Implementation must be performed by Sonnet and reviewed by Opus.",
    "Implementation must be performed by Sonnet, reviews by Opus.",
    "Use Sonnet for implementation and Opus for reviews.",
    "Use Sonnet for implementation, and reviews by Opus.",
])
def test_a_prohibition_or_remark_never_grants(tmp_path, rules):
    store, task = _project(tmp_path, rules)
    for role in AUDITORS:
        assert auto_route.project_instruction_route(store, task.task_id, role) == (None, None)


def test_a_requirement_covers_only_the_purpose_it_names(tmp_path):
    store, task = _project(tmp_path, "Use Opus for design. Audits must be performed by Sonnet.")
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor")[0]["family"] == SONNET
    assert auto_route.project_instruction_route(store, task.task_id, "worker-ux-designer")[0]["family"] == OPUS
    assert auto_route.project_instruction_route(store, task.task_id, "worker-backend-dev") == (None, None)


def test_a_mixed_sentence_binds_the_model_to_its_own_subject(tmp_path):
    store, task = _project(tmp_path, "Security audits must be performed by Opus and implementation by Sonnet.",
                           message="Implementation must be performed by Sonnet and reviewed by Opus.")
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor")[0]["family"] == OPUS
    assert auto_route.project_instruction_route(store, task.task_id, "worker-code-reviewer") == (None, None)
    # The user's own sentence is scoped the same way.
    for role in AUDITORS:
        assert auto_route.user_purpose_model_request(store, task.task_id, role) is None
    append_source_event(store, SourceEvent(origin="test", event_id="mixed"),
                        "Use Sonnet for implementation and use Opus for reviews.", task.task_id)
    for role in AUDITORS:
        assert auto_route.user_purpose_model_request(store, task.task_id, role) == OPUS
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-backend-dev") is None


@pytest.mark.parametrize("message", ["Проведи ревью на Opus и обнови документацию.", "Сделай ревью кода через Opus и запусти тесты.",
                                     "Run the audit on Opus and fix the tests."])
def test_the_work_before_the_model_stays_its_purpose(tmp_path, message):
    """Other work named after the model never displaces the work named before
    it (review, 2026-10-23): these granted the reviewer no model."""
    store, task = _project(tmp_path, None, message=message)
    for role in AUDITORS:
        assert auto_route.user_purpose_model_request(store, task.task_id, role) == OPUS


def test_a_newer_request_for_the_purpose_outranks_an_older_one(tmp_path):
    store, task = _project(tmp_path, None, message="Для аудита используй Sonnet.")
    append_source_event(store, SourceEvent(origin="test", event_id="newer"),
                        "Сделай аудит через Opus и потом тесты.", task.task_id)
    for role in AUDITORS:
        assert auto_route.user_purpose_model_request(store, task.task_id, role) == OPUS


def test_a_phrase_between_two_models_is_the_one_it_plainly_belongs_to(tmp_path):
    store, task = _project(tmp_path, None, message="Do the review on Opus and run the tests on Sonnet.")
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-code-reviewer") == OPUS
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-qa-engineer") == SONNET
    append_source_event(store, SourceEvent(origin="test", event_id="purpose"),
                        "Use Sonnet for tests and use Opus for reviews.", task.task_id)
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-code-reviewer") == OPUS
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-qa-engineer") == SONNET


def test_a_rule_naming_its_work_before_the_model_grants(tmp_path):
    store, task = _project(tmp_path, "Run all audits on Opus before deploying.\n")
    for role in AUDITORS:
        assert auto_route.project_instruction_route(store, task.task_id, role)[0]["family"] == OPUS


@pytest.mark.parametrize("text", ["Audits must be performed by Opus or Sonnet.", "Аудит должен проводить Opus или Sonnet.",
                                  "Audits must be performed by Opus, Sonnet or Sol.",
                                  # A repeated preposition or a slash lists alternatives too.
                                  "Audits must be performed by Opus or by Sonnet.", "Audits must be performed by Opus/Sonnet.",
                                  "Audits must be performed by Opus и/или Sonnet."])
def test_a_list_of_alternatives_permits_a_choice_and_demands_none(tmp_path, text):
    """A rule or request listing models lets routing choose among them: it
    never forces the first one as the owner's request (review, 2026-10-23)."""
    store, task = _project(tmp_path, text, message=text)
    assert auto_route.assigns_family(text, auto_route.family_pattern(OPUS, ("opus",)),
                                     [auto_route.family_pattern(SONNET, ("sonnet",))]) is None
    assert auto_route.user_model_request(store, task.task_id, OPUS) is None
    for role in AUDITORS:
        assert auto_route.user_purpose_model_request(store, task.task_id, role) is None
        assert auto_route.project_instruction_route(store, task.task_id, role) == (None, None)


@pytest.mark.parametrize("text", ["Run the review on Opus or Sonnet.", "Run audits on Opus or on Sonnet.",
                                  "Аудит делай на Opus или на Sonnet."])
def test_a_requested_list_of_alternatives_is_no_purpose_request(tmp_path, text):
    """An imperative naming alternatives requests no model for the purpose
    and the file's copy assigns none; the request grammar still lets the
    coordinator's hint of a listed model through as task-wide consent."""
    store, task = _project(tmp_path, text, message=text)
    assert auto_route.assigns_family(text, auto_route.family_pattern(OPUS, ("opus",)),
                                     [auto_route.family_pattern(SONNET, ("sonnet",))]) is None
    for role in AUDITORS:
        assert auto_route.user_purpose_model_request(store, task.task_id, role) is None
        assert auto_route.project_instruction_route(store, task.task_id, role) == (None, None)


def test_the_owners_own_words_outrank_the_file(tmp_path):
    store, task = _project(tmp_path, OBSERVED, message="Не используй Opus для аудита.")
    grant, revocation = auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor")
    assert grant is None
    assert revocation["reason"] == "revoked_by_user" and revocation["revoked_source_ref"] == "project_instruction:AGENTS.md:3"
    (tmp_path / "own").mkdir()
    store, task = _project(tmp_path / "own", OBSERVED, message="Для аудита используй sonnet.")
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor") == (None, None)
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-security-auditor") == SONNET


def test_imported_history_never_hides_the_file(tmp_path):
    store, task = _project(tmp_path, OBSERVED, message=HISTORY)
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor")[0]["family"] == OPUS


@pytest.mark.parametrize("message", ["Не используй Opus для аудита.", "Для аудита используй sonnet."])
def test_imported_history_never_hides_the_owners_older_words(tmp_path, message):
    """A newer imported history message neither grants nor revokes: an older
    direct refusal of the file's model, or an older request naming another
    model for the purpose, still outranks the file."""
    store, task = _project(tmp_path, OBSERVED, message=message)
    append_source_event(store, SourceEvent(origin="test", event_id="history"), HISTORY, task.task_id)
    grant, revocation = auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor")
    assert grant is None
    if message.startswith("Не"):
        assert revocation["reason"] == "revoked_by_user"
        assert revocation["revoked_source_ref"] == "project_instruction:AGENTS.md:3"
    else:
        assert revocation is None


def test_an_instruction_file_changed_during_the_task_never_grants(tmp_path):
    """The task's own writing workers may edit AGENTS.md/CLAUDE.md, so a file
    created or changed after the task started grants nothing for that task."""
    store, task = _project(tmp_path, None)
    time.sleep(0.05)
    (tmp_path / "AGENTS.md").write_text(OBSERVED, encoding="utf-8")
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor") == (None, None)
    (tmp_path / "own").mkdir()
    store, task = _project(tmp_path / "own", OBSERVED)
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor")[0]["family"] == OPUS
    time.sleep(0.05)
    with open(tmp_path / "own" / "AGENTS.md", "a", encoding="utf-8") as stream:
        stream.write("- Reviews should be done by Opus.\n")
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor") == (None, None)
    # The lapse is visible, never consent.
    assert auto_route.project_instruction_change(store, task.task_id, "worker-security-auditor") == dict(
        status="changed_during_task", path="AGENTS.md", line=3, route=OPUS)
    assert auto_route.project_instruction_change(store, task.task_id, "worker-backend-dev") is None
    # The next task reads the file as it stands when that task starts.
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="next"), "Сделай аудит", "owner")
    assert auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor")[0]["family"] == OPUS
    assert auto_route.project_instruction_change(store, task.task_id, "worker-security-auditor") is None


def test_model_text_naming_the_file_never_grants(tmp_path):
    """Without the file, the coordinator's reason, a report or a user remark
    quoting AGENTS.md grants nothing."""
    store, task = _project(tmp_path, None, message="Посмотри, что написано в AGENTS.md про аудит.")
    (tmp_path / "docs").mkdir()
    (tmp_path / "docs" / "report.md").write_text("AGENTS.md requires Opus: audits must be performed by Opus.\n")
    for role in AUDITORS:
        assert auto_route.project_instruction_route(store, task.task_id, role) == (None, None)


def _review_preferred(monkeypatch, tmp_path):
    from test_model_settings_template import entry
    from test_settings_only_routing import fake_catalog
    fake_catalog(monkeypatch, [entry("claude-agent-acp", "claude-sonnet-5"), entry("claude-agent-acp", "opus[1m]"),
                               entry("codex-acp", "gpt-6.1-sol"), entry("codex-acp", "gpt-6-luna")])
    store = _two_engines(tmp_path)
    path = tmp_path / ".cortex" / "routing.json"
    routing = json.loads(path.read_text())
    routing["models"]["claude-agent-acp"]["tiers"]["claude-sonnet-5"] = ["deep", "standard"]
    routing["models"]["claude-agent-acp"]["policy"]["claude-sonnet-5"] = {"preferred_for": ["review"]}
    path.write_text(json.dumps(routing))
    return store


def test_the_observed_dispatch_runs_opus_with_visible_provenance(tmp_path, monkeypatch):
    """End to end through choose(): the owner preferred Sonnet for review work
    and the coordinator's Opus hint was ignored:owner_preferred (observed);
    the project instruction grant now routes Opus, recorded as its basis."""
    store = _review_preferred(monkeypatch, tmp_path)
    (tmp_path / "AGENTS.md").write_text(OBSERVED, encoding="utf-8")
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="dispatch"), "Сделай ре-аудит", "owner")
    env = {"HOME": str(tmp_path)}
    reason = "AGENTS.md requires all audits and re-audits to be owned and performed by Claude Opus."
    before = auto_route.choose("worker-security-auditor", tmp_path, env, complexity="high", model_hint=OPUS,
                               model_reason=reason, work_kind_override="review")
    assert before.model == "claude-sonnet-5"
    assert before.decision["model_hint_status"] == "ignored:owner_preferred"
    grant, _ = auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor")
    after = auto_route.choose("worker-security-auditor", tmp_path, env, complexity="high", model_hint=OPUS,
                              model_reason=reason, user_request=grant, work_kind_override="review")
    assert after.model == "opus[1m]" and after.basis == "auto:user_request"
    assert after.decision["decided_by"] == after.decision["basis"] == "project_instruction"
    assert after.decision["project_instruction"] == dict(path="AGENTS.md", line=3, matched="Opus")
    assert after.decision["user_model_request"]["status"] == "honoured"
    from cortex_runtime.routing.decision_display import route_decision_summary
    assert "by project rules" in route_decision_summary("claude-agent-acp", "opus[1m]", dict(route_decision=after.decision))


@pytest.mark.parametrize("option", ["Перепроверить историю изменений через Opus",
                                  "Перепроверить анализ коммитов через Claude Opus",
                                  "Перепроверить список изменений через Opus",
                                  "Recheck commit history through Opus"])
def test_selected_recheck_model_reaches_explorer_dispatch(tmp_path, monkeypatch, option):
    """A choice to recheck with Opus is consent before dispatch, not merely
    a hint to start a cheaper explorer and wait for another correction."""
    from cortex_runtime.acp import questions_store
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    from cortex_runtime.orchestration.state import TaskMachine
    from test_runtime_task_machine import _provision, plan

    store = _review_preferred(monkeypatch, tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="recheck"), "Inspect commit history.", "owner")
    question = questions_store.create_if_absent(
        store, task.task_id, "recheck", "Как продолжить?", options=[option, "Остановиться"])
    assert questions_store.answer(store, question.id, option, "elicitation")
    state = TaskMachine(store)
    state.create(task.task_id, "Inspect commit history.")
    channel = SimpleNamespace(task_id=task.task_id, channel_id="coordinator", role="coordinator")
    proposal = plan()
    proposal["tasks"][0].update(role="explorer", model=OPUS, complexity="low",
                                model_reason="The owner selected this recheck.")
    state.submit_plan(channel, proposal, "plan")
    state.move(task.task_id, "EXECUTING")
    [node] = state.claim_ready(task.task_id, channel.channel_id)
    _provision(state, channel, node["node_id"], "xa_recheck")
    adapter = EngineSessionManagerAdapter(lambda _: None, lambda _: str(tmp_path),
                                          environ={"HOME": str(tmp_path)}, store=store)
    launch = dict(task_id=task.task_id, assignment_id=node["assignment_ref"],
                  attempt_id="xa_recheck", role_id=node["role"])
    adapter._resolve_runtime_route(launch, str(tmp_path))
    assert launch["resolved_model"] == "opus[1m]"
    request = launch["route_decision"]["user_model_request"]
    assert request["status"] == "honoured"
    assert request["source_ref"] == questions_store.answer_ref(question.id)


@pytest.mark.parametrize("text,requested", [
    ("Перепроверь список изменений через Opus", True),
    ("Recheck commit history through Opus", True),
    ("Хочу, чтобы ты перепроверил список изменений через Opus", True),
    ("Перепроверили список изменений через Opus", False),
    ("Список изменений перепроверен через Opus", False),
    ("Не перепроверяй список изменений через Opus", False),
])
def test_recheck_request_forms_do_not_turn_past_work_into_consent(text, requested):
    assert (auto_route.requests_family(text, OPUS, ("opus", "опус")) is not None) == requested


@pytest.mark.parametrize("option", [
    "Перепроверить список коммитов Opus",       # no means: the model's commits
    "Не перепроверять список коммитов через Opus",
    "Перепроверили список коммитов через Opus", # past work is not a request
    "Список коммитов перепроверен через Opus",
    "Перепроверить список коммитов через Opus или Sonnet",
])
def test_recheck_remarks_and_ambiguous_options_never_grant(tmp_path, option):
    from cortex_runtime.acp import questions_store

    store, task = _project(tmp_path, None, message="Inspect commit history.")
    question = questions_store.create_if_absent(
        store, task.task_id, "recheck", "Как продолжить?", options=[option, "Остановиться"])
    assert auto_route.user_model_request(store, task.task_id, OPUS) is None  # not selected
    assert questions_store.answer(store, question.id, option, "elicitation")
    assert auto_route.user_model_request(store, task.task_id, OPUS) is None
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-code-reviewer") is None


@pytest.mark.parametrize("denied", [False, True])
def test_unhinted_mandatory_review_resolves_project_rule_before_engine_open(tmp_path, monkeypatch, denied):
    """A mandatory reviewer has no plan hint: the project rule must beat the
    opposite-provider preference, not leave the wrong model to request a replan."""
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    from cortex_runtime.orchestration import state as runtime_state
    from test_runtime_task_machine import _provision, plan

    store = _review_preferred(monkeypatch, tmp_path)
    (tmp_path / "AGENTS.md").write_text(OBSERVED, encoding="utf-8")
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="mandatory"), "Inspect the changes.", "owner")
    state = runtime_state.TaskMachine(store)
    state.create(task.task_id, "Inspect the changes.")
    channel = SimpleNamespace(task_id=task.task_id, channel_id="coordinator", role="coordinator")
    state.submit_plan(channel, plan(), "plan")
    state.move(task.task_id, "EXECUTING")
    state.claim_ready(task.task_id, channel.channel_id)
    (tmp_path / "changed.txt").write_text("implementation change\n", encoding="utf-8")
    # Seed completed execution; this test exercises the subsequent runtime
    # review creation and dispatch routing, not result publication.
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=?", (task.task_id,))
        db.commit()
    state.add_review(task.task_id)
    [review] = state.claim_ready(task.task_id, channel.channel_id)
    assert review["stage"] == "review" and review["node_id"] == "_review_1"
    _provision(state, channel, review["node_id"], "xa_mandatory")
    # Exactly the preference produced after a Claude execution worker.
    monkeypatch.setattr(runtime_state, "review_provider_preference",
                        lambda *_: ("codex-acp", "prefer_codex_after_claude_execution"))
    if denied:
        path = tmp_path / ".cortex" / "routing.json"
        routing = json.loads(path.read_text())
        routing["deny"] = [OPUS]
        path.write_text(json.dumps(routing))
    adapter = EngineSessionManagerAdapter(lambda _: None, lambda _: str(tmp_path),
                                          environ={"HOME": str(tmp_path)}, store=store)
    launch = dict(task_id=task.task_id, assignment_id=review["assignment_ref"],
                  attempt_id="xa_mandatory", role_id=review["role"])
    # open() invokes this resolver before capacity reservation or manager
    # creation. No engine is started by this no-paid dispatch-boundary check.
    if denied:
        with pytest.raises(PortableError, match="user_requested_route_unavailable"):
            adapter._resolve_runtime_route(launch, str(tmp_path))
        assert "resolved_connection_id" not in launch and "resolved_model" not in launch
    else:
        adapter._resolve_runtime_route(launch, str(tmp_path))
        assert launch["resolved_model"] == "opus[1m]"
        assert launch["route_decision"]["decided_by"] == "project_instruction"
        assert launch["route_decision"]["cross_provider_review"] == "prefer_codex_after_claude_execution"
        assert launch["route_decision"]["project_instruction"] == dict(
            path="AGENTS.md", line=3, matched="Opus", superseded_hint="auto")


def test_hard_owner_policy_still_refuses_without_substitution(tmp_path, monkeypatch):
    store = _review_preferred(monkeypatch, tmp_path)
    (tmp_path / "AGENTS.md").write_text(OBSERVED, encoding="utf-8")
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="deny"), "Сделай аудит", "owner")
    grant, _ = auto_route.project_instruction_route(store, task.task_id, "worker-security-auditor")
    path = tmp_path / ".cortex" / "routing.json"
    routing = json.loads(path.read_text())
    routing["deny"] = [OPUS]
    path.write_text(json.dumps(routing))
    with pytest.raises(PortableError, match="user_requested_route_unavailable"):
        auto_route.choose("worker-security-auditor", tmp_path, {"HOME": str(tmp_path)}, model_hint=OPUS,
                          user_request=grant)


def test_plan_preview_and_dispatch_agree_on_the_project_instruction(tmp_path, monkeypatch):
    from cortex_runtime.orchestration.state import TaskMachine, attempt_routing
    from test_runtime_task_machine import _provision, plan

    store = _two_engines(tmp_path)
    (tmp_path / "AGENTS.md").write_text(OBSERVED, encoding="utf-8")
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="preview"), "Сделай ре-аудит", "owner")
    state = TaskMachine(store)
    state.create(task.task_id, "Сделай ре-аудит")
    channel = SimpleNamespace(task_id=task.task_id, channel_id="coordinator", role="coordinator")
    monkeypatch.setattr(auto_route, "route_menu", lambda *_: {
        "tiers": {"flagship": [], "deep": [{"route": OPUS}, {"route": SONNET}], "standard": [{"route": SONNET}],
                  "light": []}})
    proposal = plan(2)
    proposal["tasks"][0].update(role="security-auditor", model=SONNET, model_reason="Sonnet is enough here.")
    proposal["tasks"][1].update(role="backend-dev")
    previews = state.submit_plan(channel, proposal, "plan")["model_hints"]
    assert previews[0]["preview_status"] == "pending_dispatch:project_instruction"
    assert previews[0]["project_instruction"] == dict(route=OPUS, path="AGENTS.md", line=3, matched="Opus")
    assert "instead of this hint" in previews[0]["preview_note"]
    assert previews[1]["preview_status"] == "auto"
    state.move(task.task_id, "EXECUTING")
    state.claim_ready(task.task_id, channel.channel_id)
    _provision(state, channel, "N0", "xa_rules")
    routed = attempt_routing(store, task.task_id, "xa_rules")
    assert routed.model_hint == OPUS and routed.model_reason is None
    assert routed.user_request["source"] == "project_instruction"
    assert routed.user_request["superseded_hint"] == SONNET
    # A newer user refusal of Opus revokes the file's requirement for this task.
    append_source_event(store, SourceEvent(origin="test", event_id="refuse"), "Не используй Opus.", task.task_id)
    routed = attempt_routing(store, task.task_id, "xa_rules")
    assert routed.model_hint == SONNET and routed.user_request is None
    assert routed.project_instruction_status is None


def test_a_mid_task_edit_leaves_a_visible_status_in_preview_and_dispatch(tmp_path, monkeypatch):
    """An edit to AGENTS.md during the task lapses its requirement for that
    task, visibly: the plan preview and the dispatch decision say why, while
    the route itself follows ordinary ranking (review, 2026-10-23)."""
    from cortex_runtime.orchestration.state import TaskMachine, attempt_routing
    from test_runtime_task_machine import _provision, plan

    store = _review_preferred(monkeypatch, tmp_path)
    (tmp_path / "AGENTS.md").write_text(OBSERVED, encoding="utf-8")
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="edit"), "Сделай ре-аудит", "owner")
    time.sleep(0.05)
    with open(tmp_path / "AGENTS.md", "a", encoding="utf-8") as stream:
        stream.write("- Keep the changelog current.\n")
    state = TaskMachine(store)
    state.create(task.task_id, "Сделай ре-аудит")
    channel = SimpleNamespace(task_id=task.task_id, channel_id="coordinator", role="coordinator")
    proposal = plan(2)
    proposal["tasks"][0].update(role="security-auditor")
    previews = state.submit_plan(channel, proposal, "plan")["model_hints"]
    changed = dict(status="changed_during_task", path="AGENTS.md", line=3, route=OPUS)
    assert previews[0]["preview_status"] == "auto"
    assert previews[0]["project_instruction"] == changed
    assert "changed after this task started" in previews[0]["project_instruction_note"]
    assert "project_instruction" not in previews[1]
    state.move(task.task_id, "EXECUTING")
    state.claim_ready(task.task_id, channel.channel_id)
    _provision(state, channel, "N0", "xa_edit")
    routed = attempt_routing(store, task.task_id, "xa_edit")
    assert routed.user_request is None and routed.project_instruction_status == changed
    choice = auto_route.choose("worker-security-auditor", tmp_path, {"HOME": str(tmp_path)}, complexity="high",
                               model_hint=routed.model_hint, user_request=routed.user_request,
                               work_kind_override="review",
                               project_instruction_status=routed.project_instruction_status)
    assert choice.basis != "auto:user_request"
    assert choice.decision["decided_by"] != "project_instruction"
    assert choice.decision["project_instruction"] == changed


SOL = "codex-acp/gpt-6.1-sol"
LUNA = "codex-acp/gpt-6-luna"
# The committed rule of the live CLI run (2026-10-23): the period inside
# "gpt-6.1-sol" split the sentence, so it granted nothing and the mandatory
# reviewer ran on Luna.
LIVE_RULE = "All audits and reviews must use codex-acp/gpt-6.1-sol."
DOTTED_RULES = [(LIVE_RULE, SOL), ("All audits and reviews must use `codex-acp/gpt-6.1-sol`.", SOL),
                ("All audits and reviews must use gpt-6.1-sol.", "gpt-6.1-sol"),
                ("Reviews must use codex-acp/gpt-6.1-sol", SOL),
                ("Аудит должен использовать gpt-6.1-sol.", "gpt-6.1-sol")]
# Questions, negations, alternatives and remarks naming the dotted identity.
DOTTED_NON_REQUESTS = [
    "Do all audits and reviews have to use codex-acp/gpt-6.1-sol?",
    "Reviews must use gpt-6.1-sol?",
    "Почему аудит должен использовать gpt-6.1-sol?",
    "All audits and reviews must not use codex-acp/gpt-6.1-sol.",
    "Аудит не должен использовать gpt-6.1-sol.",
    "All audits and reviews must use gpt-6.1-sol or gpt-6-luna.",
    "All audits and reviews must use codex-acp/gpt-6.1-sol or codex-acp/gpt-6-luna.",
    "The last review used codex-acp/gpt-6.1-sol.",
]


def _rules(rule):
    return f"# Project agent rules\n\n{rule}\n"


def test_a_period_inside_a_token_never_ends_a_sentence():
    assert auto_route._sentences("Use Opus. Then review.") == ["Use Opus.", " Then review.", ""]
    assert auto_route._sentences(f"{LIVE_RULE} Then review AGENTS.md v1.0.0.") == [
        LIVE_RULE, " Then review AGENTS.md v1.0.0.", ""]
    assert auto_route._sentences('Use "Opus." Then review') == ['Use "Opus.', '" Then review']
    assert auto_route._sentences("**Use Opus.** Never Sonnet") == ["**Use Opus.", "** Never Sonnet"]


# A period before closing Markdown emphasis still ends the sentence in a user
# message, whose emphasis is never stripped: merged with the next sentence,
# its negation or question mark dropped the request (review, 2026-10-23).
@pytest.mark.parametrize("message", ["**Use Opus for the review.** Never use Sonnet.",
                                     "**Reviews must use Opus.** Why? Cost."])
def test_a_period_before_closing_emphasis_keeps_the_users_request(tmp_path, message):
    store, task = _project(tmp_path, None, message=message)
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-code-reviewer") == OPUS


@pytest.mark.parametrize("rule,matched", DOTTED_RULES)
def test_a_dotted_identity_in_a_rule_grants_its_exact_route(tmp_path, rule, matched):
    store, task = _project(tmp_path, _rules(rule))
    for role in AUDITORS:
        assert auto_route.project_instruction_route(store, task.task_id, role) == (dict(
            source="project_instruction", path="AGENTS.md", line=3, source_ref="project_instruction:AGENTS.md:3",
            family=SOL, matched=matched), None)
    for role in OTHERS:
        assert auto_route.project_instruction_route(store, task.task_id, role) == (None, None)


@pytest.mark.parametrize("rule,matched", DOTTED_RULES)
def test_a_dotted_identity_in_a_user_message_requests_its_exact_route(tmp_path, rule, matched):
    store, task = _project(tmp_path, None, message=rule)
    assert auto_route.user_model_request(store, task.task_id, SOL)["matched"] == matched
    for role in AUDITORS:
        assert auto_route.user_purpose_model_request(store, task.task_id, role) == SOL
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-backend-dev") is None


@pytest.mark.parametrize("option", [SOL, "gpt-6.1-sol", "Use codex-acp/gpt-6.1-sol for the review"])
def test_a_chosen_option_naming_a_dotted_identity_selects_it(tmp_path, option):
    from cortex_runtime.acp import questions_store

    store, task = _project(tmp_path, None, message="Inspect the changes.")
    question = questions_store.create_if_absent(
        store, task.task_id, "model", "Which model should run the review: codex-acp/gpt-6.1-sol or gpt-6-luna?",
        options=[option, LUNA])
    assert questions_store.answer(store, question.id, option, "elicitation")
    assert auto_route.user_model_request(store, task.task_id, SOL) is not None
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-code-reviewer") == SOL


@pytest.mark.parametrize("text", DOTTED_NON_REQUESTS)
def test_a_dotted_identity_question_negation_list_or_remark_grants_nothing(tmp_path, text):
    store, task = _project(tmp_path, _rules(text), message=text)
    for role in AUDITORS:
        assert auto_route.project_instruction_route(store, task.task_id, role) == (None, None)
        assert auto_route.user_purpose_model_request(store, task.task_id, role) is None
    if " or " not in text:
        # A listed alternative may still let a coordinator hint through as
        # task-wide consent (see test_a_requested_list_of_alternatives_...).
        assert auto_route.user_model_request(store, task.task_id, SOL) is None


@pytest.mark.parametrize("text", ["Do reviews have to use Sol?", "Почему аудит должен использовать Sol?",
                                  "Why do audits need to use Sol"])
def test_a_question_about_a_must_use_requirement_grants_nothing(tmp_path, text):
    store, task = _project(tmp_path, _rules(text), message=text)
    assert auto_route.user_model_request(store, task.task_id, SOL) is None
    for role in AUDITORS:
        assert auto_route.project_instruction_route(store, task.task_id, role) == (None, None)
        assert auto_route.user_purpose_model_request(store, task.task_id, role) is None


@pytest.mark.parametrize("text", ["Reviews must use Sol.", "Must use Sol for reviews.", "Reviews have to use Sol.",
                                  "Аудит должен использовать Sol.", "Could you run the review on Sol?"])
def test_a_stated_must_use_requirement_and_a_polite_request_still_grant(tmp_path, text):
    store, task = _project(tmp_path, _rules(text), message=text)
    for role in AUDITORS:
        assert auto_route.user_purpose_model_request(store, task.task_id, role) == SOL
        if not text.startswith("Could"):
            assert auto_route.project_instruction_route(store, task.task_id, role)[0]["family"] == SOL


@pytest.mark.parametrize("rule,matched", DOTTED_RULES[:3])
def test_a_dotted_identity_rule_reaches_preview_and_dispatch(tmp_path, monkeypatch, rule, matched):
    from cortex_runtime.orchestration.state import TaskMachine, attempt_routing
    from test_runtime_task_machine import _provision, plan

    store = _two_engines(tmp_path)
    (tmp_path / "AGENTS.md").write_text(_rules(rule), encoding="utf-8")
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="dotted"), "Review the changes", "owner")
    state = TaskMachine(store)
    state.create(task.task_id, "Review the changes")
    channel = SimpleNamespace(task_id=task.task_id, channel_id="coordinator", role="coordinator")
    monkeypatch.setattr(auto_route, "route_menu", lambda *_: {
        "tiers": {"flagship": [], "deep": [], "standard": [{"route": SOL}], "light": [{"route": LUNA}]}})
    proposal = plan(2)
    proposal["tasks"][0].update(role="security-auditor", model=LUNA, model_reason="Luna is enough here.")
    proposal["tasks"][1].update(role="backend-dev")
    previews = state.submit_plan(channel, proposal, "plan")["model_hints"]
    assert previews[0]["preview_status"] == "pending_dispatch:project_instruction"
    assert previews[0]["project_instruction"] == dict(route=SOL, path="AGENTS.md", line=3, matched=matched)
    assert previews[1]["preview_status"] == "auto"
    state.move(task.task_id, "EXECUTING")
    state.claim_ready(task.task_id, channel.channel_id)
    _provision(state, channel, "N0", "xa_dotted")
    routed = attempt_routing(store, task.task_id, "xa_dotted")
    assert routed.model_hint == SOL
    assert routed.user_request["source"] == "project_instruction" and routed.user_request["superseded_hint"] == LUNA


@pytest.mark.parametrize("rule,matched", DOTTED_RULES[:3])
def test_a_dotted_identity_rule_routes_the_mandatory_reviewer(tmp_path, monkeypatch, rule, matched):
    """The live failure: the unhinted runtime reviewer must run on the exact
    route the rule names, ahead of the opposite-provider preference and the
    owner's preferred review model."""
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    from cortex_runtime.orchestration import state as runtime_state
    from test_runtime_task_machine import _provision, plan

    store = _review_preferred(monkeypatch, tmp_path)
    (tmp_path / "AGENTS.md").write_text(_rules(rule), encoding="utf-8")
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="dotted"), "Inspect the changes.", "owner")
    state = runtime_state.TaskMachine(store)
    state.create(task.task_id, "Inspect the changes.")
    channel = SimpleNamespace(task_id=task.task_id, channel_id="coordinator", role="coordinator")
    state.submit_plan(channel, plan(), "plan")
    state.move(task.task_id, "EXECUTING")
    state.claim_ready(task.task_id, channel.channel_id)
    (tmp_path / "changed.txt").write_text("implementation change\n", encoding="utf-8")
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=?", (task.task_id,))
        db.commit()
    state.add_review(task.task_id)
    [review] = state.claim_ready(task.task_id, channel.channel_id)
    assert review["node_id"] == "_review_1"
    _provision(state, channel, review["node_id"], "xa_dotted_review")
    monkeypatch.setattr(runtime_state, "review_provider_preference",
                        lambda *_: ("claude-agent-acp", "prefer_claude_after_codex_execution"))
    adapter = EngineSessionManagerAdapter(lambda _: None, lambda _: str(tmp_path),
                                          environ={"HOME": str(tmp_path)}, store=store)
    launch = dict(task_id=task.task_id, assignment_id=review["assignment_ref"],
                  attempt_id="xa_dotted_review", role_id=review["role"])
    adapter._resolve_runtime_route(launch, str(tmp_path))
    assert launch["resolved_model"] == "gpt-6.1-sol"
    assert launch["route_decision"]["decided_by"] == "project_instruction"
    assert launch["route_decision"]["project_instruction"] == dict(
        path="AGENTS.md", line=3, matched=matched, superseded_hint="auto")
