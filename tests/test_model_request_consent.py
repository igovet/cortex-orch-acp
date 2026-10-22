"""Explicit user model requests: a refusal is never consent, and a newer
user refusal revokes an earlier grant (latest message wins)."""
from __future__ import annotations

import pytest

from cortex_runtime.acp import questions_store
from cortex_runtime.bootstrap import append_source_event, bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import auto_route
from cortex_runtime.store import Store
from test_model_settings_template import entry
from test_settings_only_routing import fake_catalog, install

ASTRA = "codex-acp/gpt-astra"
LUNA = "codex-acp/gpt-6-luna"
OPUS = "claude-agent-acp/opus"


def _settings(labels=None):
    policy = {"gpt-astra": {"request_names": labels}} if labels else {}
    return {"model_policy_version": 2,
            "enabled": {"codex-acp": ["gpt-astra", "gpt-6-luna"], "claude-agent-acp": ["opus"]},
            "models": {"codex-acp": {"auto": ["gpt-astra", "gpt-6-luna"],
                                     "tiers": {"gpt-astra": "flagship", "gpt-6-luna": "standard"},
                                     "policy": policy},
                       "claude-agent-acp": {"auto": ["opus"], "tiers": {"opus": "deep"}}}}


@pytest.fixture
def project(tmp_path):
    env = install(tmp_path, _settings(labels=["astra", "астр"]))
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    return tmp_path, env, store


def _task(store, text, event_id="first"):
    return bootstrap_task(store, SourceEvent(origin="test", event_id=event_id), text, "owner")


def _add(store, task_id, text, event_id):
    return append_source_event(store, SourceEvent(origin="test", event_id=event_id), text, task_id)


# --- refusals, unavailability and substitution are never consent --------

# The six probe messages of the routing audit, plus EN/RU variants.
@pytest.mark.parametrize("text,granted", [
    ("codex-acp/gpt-astra must never be used; use codex-acp/gpt-6-luna.", LUNA),
    ("Use the default route. codex-acp/gpt-astra is unavailable.", None),
    ("codex-acp/gpt-astra слишком дорогая, не используй её. Используй codex-acp/gpt-6-luna.", LUNA),
    ("Use codex-acp/gpt-6-luna, codex-acp/gpt-astra is too expensive.", LUNA),
    # "luna" names gpt-6-luna: an enabled model's unique last id word is a
    # request label even without configured names (owner decision 2026-10-22).
    ("Please review whether gpt-astra output was correct and use luna.", LUNA),
    ("Do not use codex-acp/gpt-astra.", None),
    # Negation after the mention, in its own clause.
    ("codex-acp/gpt-astra should not be used here.", None),
    ("codex-acp/gpt-astra is too costly; use the default route.", None),
    ("codex-acp/gpt-astra нельзя использовать.", None),
    ("codex-acp/gpt-astra никогда не используй.", None),
    ("codex-acp/gpt-astra недоступна, работай как обычно.", None),
    # Conservative: "and" before a non-predicate keeps one segment, so both
    # mentions share the refusal (a missed request only keeps the ordinary tier).
    ("Use codex-acp/gpt-astra and codex-acp/gpt-6-luna is too expensive.", None),
    # Negation, avoidance and substitution before the mention.
    ("Never use codex-acp/gpt-astra.", None),
    ("Avoid codex-acp/gpt-astra; use codex-acp/gpt-6-luna.", LUNA),
    ("Use codex-acp/gpt-6-luna instead of codex-acp/gpt-astra.", LUNA),
    ("Stop using codex-acp/gpt-astra.", None),
    ("Replace codex-acp/gpt-astra with the default route.", None),
    ("Use codex-acp/gpt-6-luna to replace codex-acp/gpt-astra.", LUNA),
    ("Никогда не используй codex-acp/gpt-astra.", None),
    ("Используй codex-acp/gpt-6-luna вместо codex-acp/gpt-astra.", LUNA),
    ("Сделай это без codex-acp/gpt-astra.", None),
    ("Не хочу, чтобы ты использовал codex-acp/gpt-astra.", None),
    # A request verb governing another clause never reaches the mention.
    ("Use codex-acp/gpt-6-luna, codex-acp/gpt-astra wrote the last review.", LUNA),
    ("Используй codex-acp/gpt-6-luna, codex-acp/gpt-astra писала прошлый отчёт.", LUNA),
])
def test_refused_mentions_never_grant(project, text, granted):
    _root, _env, store = project
    task = _task(store, text)
    assert auto_route.user_model_request(store, task.task_id, ASTRA) is None
    luna = auto_route.user_model_request(store, task.task_id, LUNA)
    assert (luna and luna["family"]) == granted


@pytest.mark.parametrize("text", [
    "Не используй Астру.", "Астра слишком дорогая.", "Astra must never run this.",
    "Use Fable, Astra is broken.", "Вместо Астры используй обычную модель.",
    "Do it without Astra.",
])
def test_refused_labels_never_grant(project, text):
    _root, _env, store = project
    task = _task(store, text)
    assert auto_route.user_model_request(store, task.task_id, ASTRA) is None


@pytest.mark.parametrize("text,matched", [
    ("Use codex-acp/gpt-astra.", ASTRA),
    ("Используй codex-acp/gpt-astra", ASTRA),
    ("Use codex-acp/gpt-astra instead of codex-acp/gpt-6-luna.", ASTRA),
    ("Prefer codex-acp/gpt-astra over codex-acp/gpt-6-luna", ASTRA),
    ("Используй codex-acp/gpt-astra вместо codex-acp/gpt-6-luna", ASTRA),
    ("Use codex-acp/gpt-6-luna, not codex-acp/gpt-6-luna-x. Then use codex-acp/gpt-astra.", ASTRA),
    ("Не забудь про тесты и используй codex-acp/gpt-astra", ASTRA),
    ("Don't touch the tests and use codex-acp/gpt-astra.", ASTRA),
    ("Use codex-acp/gpt-astra because codex-acp/gpt-6-luna is too expensive.", ASTRA),
    ("Use codex-acp/gpt-astra, codex-acp/gpt-6-luna is unavailable.", ASTRA),
    ("Use: codex-acp/gpt-astra", ASTRA),
    ("Используй именно указанные модели.\nЧасть 1: codex-acp/gpt-astra", ASTRA),
    ("Используй Астру", "Астру"),
    ("Реализуй на Астре, а ревью сделай через codex-acp/gpt-6-luna", "Астре"),
])
def test_positive_requests_still_grant(project, text, matched):
    _root, _env, store = project
    task = _task(store, text)
    grant = auto_route.user_model_request(store, task.task_id, ASTRA)
    assert (grant and grant["matched"]) == matched


@pytest.mark.parametrize("text", [
    "Для аудита использовать ТОЛЬКО opus.",
    "На opus ты делать должен только аудит, а сам dev paseo запускать на дешевых моделях",
    "Use opus for the audit.",
])
def test_owner_opus_requests_still_grant(project, text):
    _root, _env, store = project
    task = _task(store, text)
    grant = auto_route.user_model_request(store, task.task_id, OPUS)
    assert grant is not None and grant["matched"] == "opus"


def test_owner_opus_label_request_still_grants():
    labels = ["opus"]
    assert auto_route.requests_family("Для аудита использовать ТОЛЬКО opus.", OPUS, labels) == "opus"
    assert auto_route.requests_family("Opus не нужен, используй luna", OPUS, labels) is None
    assert auto_route.refuses_family("Opus не нужен, используй luna", OPUS, labels) == "Opus"


@pytest.mark.parametrize("text,refused", [
    ("Don't use codex-acp/gpt-astra", True),
    ("codex-acp/gpt-astra слишком дорогая", True),
    ("Stop using codex-acp/gpt-astra", True),
    ("Use codex-acp/gpt-6-luna instead of codex-acp/gpt-astra", True),
    # Comparisons and remarks decline a grant but do not revoke one.
    ("Compare codex-acp/gpt-astra with codex-acp/gpt-6-luna", False),
    ("Based on codex-acp/gpt-astra output, fix the tests", False),
    ("Use codex-acp/gpt-astra", False),
    ("Use codex-acp/gpt-astra, not codex-acp/gpt-6-luna", False),
])
def test_strict_refusal_detection(text, refused):
    assert (auto_route.refuses_family(text, ASTRA) is not None) == refused


# --- a newer refusing user message revokes an earlier grant ------------

@pytest.mark.parametrize("refusal", [
    "Stop using codex-acp/gpt-astra, it is too expensive.",
    "Don't use codex-acp/gpt-astra anymore.",
    "codex-acp/gpt-astra must not be used from now on.",
    "Больше не используй codex-acp/gpt-astra.",
    "codex-acp/gpt-astra слишком дорогая.",
    "Используй codex-acp/gpt-6-luna вместо codex-acp/gpt-astra.",
    "Астра не нужна.",
])
def test_newer_refusal_revokes_earlier_grant(project, refusal):
    _root, _env, store = project
    task = _task(store, "Use codex-acp/gpt-astra for the implementation.")
    first = f"{task.report_id}:-1"
    assert auto_route.user_model_request(store, task.task_id, ASTRA)["source_ref"] == first
    assert auto_route.user_model_revocation(store, task.task_id, ASTRA) is None
    added = _add(store, task.task_id, refusal, "refusal")
    assert auto_route.user_model_request(store, task.task_id, ASTRA) is None
    revocation = auto_route.user_model_revocation(store, task.task_id, ASTRA)
    assert revocation["reason"] == "revoked_by_user"
    assert revocation["source_ref"] == f"{added.report_id}:-1"
    assert revocation["revoked_source_ref"] == first
    assert revocation["family"] == ASTRA


def test_latest_message_wins_and_a_new_request_regrants(project):
    _root, _env, store = project
    task = _task(store, "Используй codex-acp/gpt-astra")
    _add(store, task.task_id, "Не используй codex-acp/gpt-astra.", "refusal")
    assert auto_route.user_model_request(store, task.task_id, ASTRA) is None
    again = _add(store, task.task_id, "Use codex-acp/gpt-astra again.", "again")
    assert auto_route.user_model_request(store, task.task_id, ASTRA)["source_ref"] == f"{again.report_id}:-1"
    assert auto_route.user_model_revocation(store, task.task_id, ASTRA) is None


def test_a_newer_comparison_or_unrelated_message_does_not_revoke(project):
    _root, _env, store = project
    task = _task(store, "Use codex-acp/gpt-astra.")
    _add(store, task.task_id, "Compare codex-acp/gpt-astra with codex-acp/gpt-6-luna later.", "compare")
    _add(store, task.task_id, "Do not use codex-acp/gpt-6-luna.", "other")
    assert auto_route.user_model_request(store, task.task_id, ASTRA)["source_ref"] == f"{task.report_id}:-1"
    assert auto_route.user_model_revocation(store, task.task_id, ASTRA) is None


def test_refusal_without_an_earlier_grant_is_not_a_revocation(project):
    _root, _env, store = project
    task = _task(store, "Never use codex-acp/gpt-astra.")
    assert auto_route.user_model_request(store, task.task_id, ASTRA) is None
    assert auto_route.user_model_revocation(store, task.task_id, ASTRA) is None


def test_an_older_refusal_does_not_revoke_a_newer_grant(project):
    _root, _env, store = project
    task = _task(store, "Never use codex-acp/gpt-astra.")
    added = _add(store, task.task_id, "Now use codex-acp/gpt-astra.", "grant")
    assert auto_route.user_model_request(store, task.task_id, ASTRA)["source_ref"] == f"{added.report_id}:-1"


def test_refusal_in_the_same_message_outweighs_its_grant(project):
    _root, _env, store = project
    task = _task(store, "Use codex-acp/gpt-astra for the backend. Do not use codex-acp/gpt-astra for UI.")
    assert auto_route.user_model_request(store, task.task_id, ASTRA) is None


def test_answers_and_messages_are_ordered_by_time(project):
    _root, _env, store = project
    task = _task(store, "Use codex-acp/gpt-astra.")
    question = questions_store.create_if_absent(store, task.task_id, "q-model", "Which model?")
    assert questions_store.answer(store, question.id, "Не используй codex-acp/gpt-astra.", "text")
    assert auto_route.user_model_request(store, task.task_id, ASTRA) is None
    assert auto_route.user_model_revocation(store, task.task_id, ASTRA)["source_ref"] == questions_store.answer_ref(question.id)
    # A newer user message, after the answer, grants again.
    again = _add(store, task.task_id, "Use codex-acp/gpt-astra after all.", "again")
    assert auto_route.user_model_request(store, task.task_id, ASTRA)["source_ref"] == f"{again.report_id}:-1"


def test_revocation_is_recorded_in_the_routing_decision(project, monkeypatch):
    root, env, store = project
    fake_catalog(monkeypatch, [entry("codex-acp", "gpt-astra"), entry("codex-acp", "gpt-6-luna"),
                               entry("claude-agent-acp", "opus")])
    task = _task(store, "Use codex-acp/gpt-astra.")
    _add(store, task.task_id, "codex-acp/gpt-astra is too expensive, stop.", "refusal")
    grant = auto_route.user_model_request(store, task.task_id, ASTRA)
    revocation = auto_route.user_model_revocation(store, task.task_id, ASTRA)
    assert grant is None and revocation is not None
    choice = auto_route.choose("worker-backend-dev", root, env, model_hint=ASTRA,
                               user_request=grant, user_revocation=revocation)
    assert choice.basis == "auto" and choice.model != "gpt-astra"
    assert choice.decision["user_model_request"]["status"] == "revoked_by_user"
    assert choice.decision["user_model_request"]["reason"] == "revoked_by_user"
    assert choice.decision["user_model_request"]["source_ref"] == revocation["source_ref"]
