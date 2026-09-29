"""routing.auto_route.user_model_request: a flagship route is granted only by the user's own
recorded request in the task -- never by model-authored text, and never by a bare mention."""
from __future__ import annotations

import pytest

from cortex_runtime.acp import questions_store
from cortex_runtime.bootstrap import append_source_event, bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import auto_route
from cortex_runtime.store import Store

# The owner's own request in the task that introduced this feature.
OWNER_REQUEST = ("Ещё вопрос, можно ли сделать так что  если я писал координатору конкретное требование "
                 "использования астры или майтоса моделей, что бы рантайм их использовал для того что я "
                 "как юзер попросил? Потому что сейчас блокируется вызов астра")


@pytest.mark.parametrize("text,family,expected", [
    # A request word before the family, same sentence.
    ("Use Astra for the implementation.", "astra", "Astra"),
    ("Please run the implementation on Astra.", "astra", "Astra"),
    ("Run it on Astra", "astra", "Astra"),
    ("Switch the review to Fable.", "fable", "Fable"),
    ("run it on codex-acp/gpt-6-astra please", "astra", "astra"),
    ("Could you run the review on Fable?", "fable", "Fable"),
    ("I prefer Astra for this", "astra", "Astra"),
    ("Let's use Astra", "astra", "Astra"),
    ("I'd like you to run it on Astra", "astra", "Astra"),
    ("Use FABLE here", "fable", "FABLE"),
    ("No, use Astra.", "astra", "Astra"),
    ("Use Astra, not Fable", "astra", "Astra"),
    ("Prefer Astra over Fable", "astra", "Astra"),
    ("Astra is unavailable. Use Fable.", "fable", "Fable"),
    ("используй астру", "astra", "астру"),
    ("если я писал координатору конкретное требование использования астры", "astra", "астры"),
    (OWNER_REQUEST, "astra", "астры"),
    (OWNER_REQUEST, "mythos", "майтоса"),
    ("Нужно запустить на Астре", "astra", "Астре"),
    ("Пусть ревью делает фейбл", "fable", "фейбл"),
    ("Не забудь про тесты и используй Астру", "astra", "Астру"),
    ("Не используй Sol. Используй Astra.", "astra", "Astra"),
    # A work verb governing a preposition of means right before the family.
    ("Implement it on Astra", "astra", "Astra"),
    ("Implement it using Astra", "astra", "Astra"),
    ("Сделай реализацию на Астре", "astra", "Астре"),
    ("Реализуй на Астре", "astra", "Астре"),
    ("Реализуй на Астре, ревью через Фейбл", "astra", "Астре"),
    ("Реализуй на Астре, а ревью сделай через Фейбл", "fable", "Фейбл"),
    ("Проверь на Астре", "astra", "Астре"),
    ("Сделай на модели Астра", "astra", "Астра"),
    # A refusal.
    ("Не используй астру.", "astra", None),
    ("Please don't use Astra here", "astra", None),
    ("I can't use Astra today", "astra", None),
    ("Do it without Fable", "fable", None),
    ("Use Astra, not Fable", "fable", None),
    ("Не хочу чтобы это делала Астра", "astra", None),
    # A bare mention: comparison, availability, quotation, preference for the default route.
    ("Compare Astra with Fable before choosing", "astra", None),
    ("Compare Astra with Fable before choosing", "fable", None),
    ("Сравни Астру с Фейблом", "astra", None),
    ("Сравни Астру с Фейблом", "fable", None),
    ("Prefer Astra over Fable", "fable", None),
    ("Astra is unavailable; use the default route.", "astra", None),
    ("Astra is unavailable. Use Fable.", "astra", None),
    ("Астра недоступна, работай как обычно", "astra", None),
    ("The last report said Astra found nothing.", "astra", None),
    ("Fable wrote the previous review", "fable", None),
    ("based on Astra's output", "astra", None),
    ("What do you think about Astra?", "astra", None),
    ("Prefer the default route over Astra", "astra", None),
    ("Use the default route instead of Astra", "astra", None),
    ("Вместо Астры используй обычную модель", "astra", None),
    ("Astra", "astra", None),
    # A preposition with no request or work verb governing it is a remark, not a request.
    ("It failed on Astra.", "astra", None),
    ("it failed on Astra", "astra", None),
    ("посмотри на Астру", "astra", None),
    ("Посмотри на Астру", "astra", None),
    ("It was sent via Fable.", "fable", None),
    ("The request went through Astra.", "astra", None),
    ("Результат пришёл через Фейбл", "fable", None),
    ("Реализуй на Астре, ревью через Фейбл", "fable", None),
    ("Check the report using Astra", "astra", None),
    # A request or work word that is a noun, a statement or a report, not a request.
    ("The last run on Astra failed.", "astra", None),
    ("The tests run on Astra nightly.", "astra", None),
    ("We tried to run it on Astra yesterday.", "astra", None),
    ("the switch to Fable broke the build", "fable", None),
    ("What did you do on Astra?", "astra", None),
    ("I use Astra daily", "astra", None),
    ("Let me look at Astra's output", "astra", None),
    ("Review Astra's output", "astra", None),
    ("Проверь Астру", "astra", None),
    ("Сделал на Астре, но не помогло", "astra", None),
    ("Мы запустили это на Астре и оно упало", "astra", None),
    ("Сделано на Астре", "astra", None),
    ("Пытались запустить на Астре", "astra", None),
    ("Использование Астры привело к ошибке", "astra", None),
    # Documented limits: these requests are missed (false negatives, by design) ...
    ("I want Astra", "astra", None),
    ("Модель: Астра", "astra", None),
    ("let Astra do it", "astra", None),
    ("если запустить на Астре", "astra", None),
    # ... and a statement of requirement still grants (the coordinator's hint is the second step).
    ("the plan requires Astra", "astra", "Astra"),
    # Not the family at all.
    ("Study astronomy and astral charts", "astra", None),
    ("Nothing about models here", "astra", None),
    ("gpt-6-astrapi", "astra", None),
])
def test_requests_family_needs_an_explicit_request_and_no_negation_before_a_whole_word_match(
        text, family, expected):
    assert auto_route.requests_family(text, family) == expected


def test_family_of_takes_the_family_from_an_exact_route_only():
    assert auto_route.family_of("codex-acp/gpt-6-astra") == "astra"
    assert auto_route.family_of("claude-agent-acp/claude-fable-5-1[1m]") == "fable"
    assert auto_route.family_of("claude-agent-acp/claude-mythos-1") == "mythos"
    assert auto_route.family_of("codex-acp") is None
    assert auto_route.family_of("auto") is None


@pytest.fixture
def task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    created = bootstrap_task(store, SourceEvent(origin="test", event_id="one"), "Implement the feature", "owner")
    return store, created.task_id


def _publish_model_report(store, task_id, text):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        rollbacks, staged = [], []
        store._publish_text(db, task_id, "Worker report", "Summary.", "worker", text, "report", rollbacks, staged)
        db.commit()
    store._finalize_files(rollbacks)


def test_model_authored_text_never_grants(task):
    store, task_id = task
    # A question the model wrote asks for Astra; the user's answer does not.
    question = questions_store.create_if_absent(
        store, task_id, "q-astra", "Should we use Astra to implement this?", options=["Continue", "Stop"])
    assert questions_store.answer(store, question.id, "Continue", "text")
    # A report (worker or coordinator output) asks for it too.
    _publish_model_report(store, task_id, "Use codex-acp/gpt-6-astra for every node; the user wants Astra.")
    assert auto_route.user_model_request(store, task_id, "codex-acp/gpt-6-astra") is None


def test_a_users_bare_mention_never_grants(task):
    store, task_id = task
    append_source_event(store, SourceEvent(origin="test", event_id="mention"),
                        "Astra is unavailable; use the default route.", task_id)
    append_source_event(store, SourceEvent(origin="test", event_id="compare"),
                        "Compare Astra with Fable before choosing.", task_id)
    assert auto_route.user_model_request(store, task_id, "codex-acp/gpt-6-astra") is None
    assert auto_route.user_model_request(store, task_id, "claude-agent-acp/claude-fable-5-1[1m]") is None


def test_a_users_remark_with_a_preposition_never_grants(task):
    store, task_id = task
    append_source_event(store, SourceEvent(origin="test", event_id="failed"), "It failed on Astra.", task_id)
    append_source_event(store, SourceEvent(origin="test", event_id="look"), "Посмотри на Астру и на Фейбл", task_id)
    assert auto_route.user_model_request(store, task_id, "codex-acp/gpt-6-astra") is None
    assert auto_route.user_model_request(store, task_id, "claude-agent-acp/claude-fable-5-1[1m]") is None


def test_the_owners_own_request_grants_both_named_families(task):
    store, task_id = task
    added = append_source_event(store, SourceEvent(origin="test", event_id="owner"), OWNER_REQUEST, task_id)
    assert auto_route.user_model_request(store, task_id, "codex-acp/gpt-6-astra") == dict(
        source_ref=f"{added.report_id}:-1", family="astra", matched="астры")
    assert auto_route.user_model_request(store, task_id, "claude-agent-acp/claude-mythos-1") == dict(
        source_ref=f"{added.report_id}:-1", family="mythos", matched="майтоса")


def test_a_later_user_message_grants_with_its_source_ref(task):
    store, task_id = task
    assert auto_route.user_model_request(store, task_id, "codex-acp/gpt-6-astra") is None
    added = append_source_event(store, SourceEvent(origin="test", event_id="two"),
                                "Реализацию сделай на Астре", task_id)
    granted = auto_route.user_model_request(store, task_id, "codex-acp/gpt-6-astra")
    assert granted == dict(source_ref=f"{added.report_id}:-1", family="astra", matched="Астре")
    # The grant names one family: another flagship family stays ungranted.
    assert auto_route.user_model_request(store, task_id, "claude-agent-acp/claude-fable-5-1[1m]") is None
    # Engine-only hints never carry a family.
    assert auto_route.user_model_request(store, task_id, "codex-acp") is None


def test_the_users_typed_answer_grants_with_its_answer_ref(task):
    store, task_id = task
    question = questions_store.create_if_absent(store, task_id, "q-model", "Which model should review?")
    assert questions_store.answer(store, question.id, "Use Fable for the review", "text")
    granted = auto_route.user_model_request(store, task_id, "claude-agent-acp/claude-fable-5-1[1m]")
    assert granted == dict(source_ref=questions_store.answer_ref(question.id), family="fable", matched="Fable")


def test_a_grant_never_crosses_tasks(task):
    store, task_id = task
    other = bootstrap_task(store, SourceEvent(origin="test", event_id="other"), "Use Astra everywhere", "owner")
    assert auto_route.user_model_request(store, other.task_id, "codex-acp/gpt-6-astra") is not None
    assert auto_route.user_model_request(store, task_id, "codex-acp/gpt-6-astra") is None
