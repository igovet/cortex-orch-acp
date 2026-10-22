"""routing.auto_route.user_model_request: a flagship route is granted only by the user's own
recorded request in the task -- never by model-authored text, and never by a bare mention."""
from __future__ import annotations

import json

import pytest

from cortex_runtime.acp import questions_store
from cortex_runtime.bootstrap import append_source_event, bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import auto_route
from cortex_runtime.portable.errors import PortableError
from model_policy_fixtures import routing_fixture
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
    # A Russian subjunctive after "чтобы"/"что бы" asks.
    ("Я хочу что бы ты переработал дизайн и сделал аудит дизайна через Фейбл.", "fable", "Фейбл"),
    ("Хочу, чтобы ты использовал Астру", "astra", "Астру"),
    ("Нужно, чтобы ревью провели через Астру", "astra", "Астру"),
    ("Все задачи связанные с UI/UX передавай только Фейблу", "fable", "Фейблу"),
    ("Поручи аудит Астре", "astra", "Астре"),
    # A refusal.
    ("Не используй астру.", "astra", None),
    ("Please don't use Astra here", "astra", None),
    ("I can't use Astra today", "astra", None),
    ("Do it without Fable", "fable", None),
    ("Use Astra, not Fable", "fable", None),
    ("Не хочу чтобы это делала Астра", "astra", None),
    ("Я не хочу, чтобы ты переработал дизайн и сделал аудит дизайна через Фейбл.", "fable", None),
    ("Хочу, чтобы ты не использовал Астру", "astra", None),
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
    ("Мы сделали аудит через Фейбл", "fable", None),
    ("Чтобы ты знал, я сделал аудит через Фейбл", "fable", None),
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
    # Labels are fixture/owner data, not derived from model names.
    labels = {"astra": ["astra", "астр"], "fable": ["fable", "фейбл", "фэйбл"],
              "mythos": ["mythos", "мифос", "майтос", "митос"]}
    exact = {"astra": "codex-acp/gpt-6-astra", "fable": "claude-agent-acp/claude-fable-5-1[1m]",
             "mythos": "claude-agent-acp/claude-mythos-1"}[family]
    if text == "run it on codex-acp/gpt-6-astra please":
        expected = "codex-acp/gpt-6-astra"
    assert auto_route.requests_family(text, exact, labels[family]) == expected


def test_family_of_takes_the_family_from_an_exact_route_only():
    assert auto_route.family_of("codex-acp/gpt-6-astra") == "codex-acp/gpt-6-astra"
    assert auto_route.family_of("claude-agent-acp/claude-fable-5-1[1m]") == "claude-agent-acp/claude-fable-5-1[1m]"
    assert auto_route.family_of("claude-agent-acp/claude-mythos-1") == "claude-agent-acp/claude-mythos-1"
    assert auto_route.family_of("codex-acp") is None
    assert auto_route.family_of("auto") is None


@pytest.fixture
def task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    created = bootstrap_task(store, SourceEvent(origin="test", event_id="one"), "Implement the feature", "owner")
    routing = routing_fixture({"models": {"claude-agent-acp": {"policy": {
        "claude-mythos-1": {"request_names": ["mythos", "мифос", "майтос", "митос"]}}}}})
    (tmp_path / ".cortex" / "routing.json").write_text(json.dumps(routing))
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
        source_ref=f"{added.report_id}:-1", family="codex-acp/gpt-6-astra", matched="астры")
    assert auto_route.user_model_request(store, task_id, "claude-agent-acp/claude-mythos-1") == dict(
        source_ref=f"{added.report_id}:-1", family="claude-agent-acp/claude-mythos-1", matched="майтоса")


def test_a_later_user_message_grants_with_its_source_ref(task):
    store, task_id = task
    assert auto_route.user_model_request(store, task_id, "codex-acp/gpt-6-astra") is None
    added = append_source_event(store, SourceEvent(origin="test", event_id="two"),
                                "Реализацию сделай на Астре", task_id)
    granted = auto_route.user_model_request(store, task_id, "codex-acp/gpt-6-astra")
    assert granted == dict(source_ref=f"{added.report_id}:-1", family="codex-acp/gpt-6-astra", matched="Астре")
    # The grant names one family: another flagship family stays ungranted.
    assert auto_route.user_model_request(store, task_id, "claude-agent-acp/claude-fable-5-1[1m]") is None
    # Engine-only hints never carry a family.
    assert auto_route.user_model_request(store, task_id, "codex-acp") is None


def test_the_users_typed_answer_grants_with_its_answer_ref(task):
    store, task_id = task
    question = questions_store.create_if_absent(store, task_id, "q-model", "Which model should review?")
    assert questions_store.answer(store, question.id, "Use Fable for the review", "text")
    granted = auto_route.user_model_request(store, task_id, "claude-agent-acp/claude-fable-5-1[1m]")
    assert granted == dict(source_ref=questions_store.answer_ref(question.id), family="claude-agent-acp/claude-fable-5-1[1m]", matched="Fable")


def test_a_grant_never_crosses_tasks(task):
    store, task_id = task
    other = bootstrap_task(store, SourceEvent(origin="test", event_id="other"), "Use Astra everywhere", "owner")
    assert auto_route.user_model_request(store, other.task_id, "codex-acp/gpt-6-astra") is not None
    assert auto_route.user_model_request(store, task_id, "codex-acp/gpt-6-astra") is None


@pytest.mark.parametrize('engine,model', [
    ('claude-agent-acp','claude-sonnet-5'), ('codex-acp','gpt-6.1-sol'),
    ('codex-acp','gpt-6-luna'), ('claude-agent-acp','opus[1m]'),
])
@pytest.mark.parametrize('suffix', ['', '.', ',', '!', '?', ';', ':'])
def test_unique_enabled_bare_model_request_binds_exact_source(tmp_path,engine,model,suffix):
    from test_settings_only_routing import install, settings
    data=settings(model)
    data['enabled']={engine:[model]}
    data['models']={engine:data['models']['codex-acp']}
    install(tmp_path,data)
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='test',event_id='bare'),
                        f'Use {model}{suffix}', 'owner')
    grant=auto_route.user_model_request(store,task.task_id,f'{engine}/{model}')
    assert grant == dict(source_ref=f'{task.report_id}:-1',family=f'{engine}/{model}',matched=model)
    assert auto_route.ambiguous_user_model_request(store,task.task_id,f'{engine}/{model}') is None


@pytest.mark.parametrize('engine,model', [
    ('codex-acp','gpt-6.1-sol'), ('claude-agent-acp','opus[1m]'),
    ('opencode','vendor/Unit,7[wide]'),
])
def test_full_model_request_accepts_terminal_punctuation(tmp_path,engine,model):
    from test_settings_only_routing import install, settings
    data=settings(model)
    data['enabled']={engine:[model]}
    data['models']={engine:data['models']['codex-acp']}
    install(tmp_path,data)
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    identity=f'{engine}/{model}'
    task=bootstrap_task(store,SourceEvent(origin='test',event_id='full'),f'Use {identity}.','owner')
    assert auto_route.user_model_request(store,task.task_id,identity) == dict(
        source_ref=f'{task.report_id}:-1',family=identity,matched=identity)


@pytest.mark.parametrize('suffix',['','.'])
def test_ambiguous_bare_id_never_grants_either_engine(tmp_path,suffix):
    from test_settings_only_routing import install, settings
    data=settings('shared')
    data['enabled']['claude-agent-acp']=['shared']
    data['models']['claude-agent-acp']={'auto':['shared'],'tiers':{'shared':'standard'}}
    install(tmp_path,data)
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='test',event_id='ambiguous'),f'Use shared{suffix}','owner')
    for identity in ('codex-acp/shared','claude-agent-acp/shared'):
        assert auto_route.user_model_request(store,task.task_id,identity) is None
        assert auto_route.ambiguous_user_model_request(store,task.task_id,identity) == dict(
            source_ref=f'{task.report_id}:-1',matched='shared')
    append_source_event(store,SourceEvent(origin='test',event_id='qualified'),
                        'Use claude-agent-acp/shared.',task.task_id)
    assert auto_route.user_model_request(store,task.task_id,'claude-agent-acp/shared')['matched'] == 'claude-agent-acp/shared'
    assert auto_route.user_model_request(store,task.task_id,'codex-acp/shared') is None


def test_disabled_duplicate_does_not_make_bare_id_ambiguous(tmp_path):
    from test_settings_only_routing import install, settings
    data=settings('shared')
    data['models']['claude-agent-acp']={'auto':[],'tiers':{'shared':'standard'}}
    install(tmp_path,data)
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='test',event_id='enabled'),'Use shared','owner')
    assert auto_route.user_model_request(store,task.task_id,'codex-acp/shared') is not None
    assert auto_route.user_model_request(store,task.task_id,'claude-agent-acp/shared') is None


@pytest.mark.parametrize('text', [
    'gpt-6-luna', 'Do not use gpt-6-luna.',
    'Use gpt-6-luna-extended', 'Use gpt-6-luna.v2', 'Use gpt-6-luna[wide]',
    'Use vendor/gpt-6-luna', 'Use GPT-6-LUNA', 'Use gpt-6-luna,next',
])
def test_bare_ids_keep_exact_boundaries_and_request_grammar(tmp_path,text):
    from test_settings_only_routing import install, settings
    install(tmp_path,settings('gpt-6-luna'))
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='test',event_id='negative'),text,'owner')
    assert auto_route.user_model_request(store,task.task_id,'codex-acp/gpt-6-luna') is None


@pytest.mark.parametrize('qualified',[False,True])
def test_terminal_dot_that_is_a_configured_id_is_not_stripped(tmp_path,qualified):
    from test_settings_only_routing import install, settings
    data=settings('unit')
    data['enabled']['codex-acp'].append('unit.')
    data['models']['codex-acp']['tiers']['unit.']='standard'
    install(tmp_path,data)
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    spelling='codex-acp/unit.' if qualified else 'unit.'
    task=bootstrap_task(store,SourceEvent(origin='test',event_id='opaque'),f'Use {spelling}','owner')
    assert auto_route.user_model_request(store,task.task_id,'codex-acp/unit') is None
    assert auto_route.user_model_request(store,task.task_id,'codex-acp/unit.')['matched'] == spelling


def test_bare_model_consent_uses_only_recorded_user_answer(task):
    store,task_id=task
    _publish_model_report(store,task_id,'Use gpt-6-luna.')
    question=questions_store.create_if_absent(store,task_id,'q-bare','Use gpt-6-luna?')
    assert auto_route.user_model_request(store,task_id,'codex-acp/gpt-6-luna') is None
    assert questions_store.answer(store,question.id,'Use gpt-6-luna.','text')
    assert auto_route.user_model_request(store,task_id,'codex-acp/gpt-6-luna') == dict(
        source_ref=questions_store.answer_ref(question.id),family='codex-acp/gpt-6-luna',matched='gpt-6-luna')


@pytest.mark.parametrize('qualified',[False,True])
@pytest.mark.parametrize('template', [
    'Часть 2 на {model}: исправь подсчёт',
    'Part 2 on {model}: fix counting',
    'Step 2 via {model}: fix counting',
    'Part 2 with {model}: fix counting',
    'на {model}', 'on {model}', 'via {model}', 'with {model}',
])
def test_exact_enabled_identity_after_means_preposition_is_an_owner_request(tmp_path,qualified,template):
    from test_settings_only_routing import install, settings
    install(tmp_path,settings('gpt-6-luna'))
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    model='codex-acp/gpt-6-luna' if qualified else 'gpt-6-luna'
    task=bootstrap_task(store,SourceEvent(origin='test',event_id='means'),template.format(model=model),'owner')
    assert auto_route.user_model_request(store,task.task_id,'codex-acp/gpt-6-luna') == dict(
        source_ref=f'{task.report_id}:-1',family='codex-acp/gpt-6-luna',matched=model)


@pytest.mark.parametrize('request_text', ['Используй именно указанные модели.', 'Use exactly the named models.'])
@pytest.mark.parametrize('position',['before','after'])
def test_message_wide_request_governs_every_exact_enabled_identity(tmp_path,request_text,position):
    from test_settings_only_routing import install, settings
    data=settings('gpt-6-luna')
    data['enabled']['codex-acp'].append('gpt-6.1-sol')
    data['models']['codex-acp']['tiers']['gpt-6.1-sol']='standard'
    data['enabled']['claude-agent-acp']=['claude-sonnet-5']
    data['models']['claude-agent-acp']={'auto':['claude-sonnet-5'],'tiers':{'claude-sonnet-5':'standard'}}
    install(tmp_path,data)
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    listing='Часть 1: claude-agent-acp/claude-sonnet-5\nЧасть 2: gpt-6-luna\nЧасть 3: gpt-6.1-sol'
    text=request_text+'\n'+listing if position=='before' else listing+'\n'+request_text
    task=bootstrap_task(store,SourceEvent(origin='test',event_id='message'),text,'owner')
    for identity in ['claude-agent-acp/claude-sonnet-5','codex-acp/gpt-6-luna','codex-acp/gpt-6.1-sol']:
        assert auto_route.user_model_request(store,task.task_id,identity)['source_ref']==f'{task.report_id}:-1'


@pytest.mark.parametrize('text', [
    'Часть 2 не на gpt-6-luna: исправь',
    'Part 2 not on gpt-6-luna: fix',
    'Use the named models. Part 2: not gpt-6-luna',
    'Do not use these models. Part 2: gpt-6-luna',
    'Не используй эти модели. Часть 2: gpt-6-luna',
    'Use these models. Without gpt-6-luna',
    'Part 2 on GPT-6-LUNA', 'Part 2 on gpt-6-luna-extended',
])
def test_expanded_identity_request_grammar_keeps_negations_and_exact_case(tmp_path,text):
    from test_settings_only_routing import install, settings
    install(tmp_path,settings('gpt-6-luna'))
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='test',event_id='negative-means'),text,'owner')
    assert auto_route.user_model_request(store,task.task_id,'codex-acp/gpt-6-luna') is None


def test_message_wide_grant_does_not_cross_user_messages_or_apply_to_labels(task):
    store,task_id=task
    append_source_event(store,SourceEvent(origin='test',event_id='request'),'Use exactly the named models.',task_id)
    append_source_event(store,SourceEvent(origin='test',event_id='listing'),'Part 2: gpt-6-luna',task_id)
    assert auto_route.user_model_request(store,task_id,'codex-acp/gpt-6-luna') is None
    append_source_event(store,SourceEvent(origin='test',event_id='label'),'Astra. Use the default model.',task_id)
    assert auto_route.user_model_request(store,task_id,'codex-acp/gpt-6-astra') is None


@pytest.mark.parametrize("text,expected", [
    # The owner's request that routed an audit to another model (2026-10-22).
    ("Добавь в AGENTS.md требование, чтобы за audit отвечал opus.", "opus"),
    ("За аудит отвечает Opus", "Opus"),
    ("Opus is responsible for the security review", "Opus"),
    ("Let Opus handle the review", "Opus"),
])
def test_a_model_made_responsible_for_a_role_or_stage_is_a_request(text, expected):
    pattern = auto_route.family_pattern("claude-agent-acp/opus[1m]", ("opus", "опус"))
    assert auto_route.assigns_family(text, pattern) == expected


@pytest.mark.parametrize("text", [
    "Opus wrote the last review.",          # a remark about the past
    "За аудит отвечал Opus.",               # a report, no "чтобы"/"пусть"
    "Opus is fast.",                         # no role or stage
    "Не нужно, чтобы за аудит отвечал Opus.",  # refused
])
def test_a_remark_about_a_model_assigns_nothing(text):
    pattern = auto_route.family_pattern("claude-agent-acp/opus[1m]", ("opus", "опус"))
    assert auto_route.assigns_family(text, pattern) is None


def _two_engines(tmp_path, opus_names=("opus", "опус")):
    from test_settings_only_routing import install
    install(tmp_path, {
        "model_policy_version": 2,
        "enabled": {"claude-agent-acp": ["claude-sonnet-5", "opus[1m]"], "codex-acp": ["gpt-6.1-sol", "gpt-6-luna"]},
        "models": {
            "claude-agent-acp": {"auto": ["claude-sonnet-5", "opus[1m]"],
                                 "tiers": {"claude-sonnet-5": "standard", "opus[1m]": "deep"},
                                 "policy": {"claude-sonnet-5": {}, "opus[1m]": {"request_names": list(opus_names)}}},
            "codex-acp": {"auto": ["gpt-6.1-sol", "gpt-6-luna"], "tiers": {"gpt-6.1-sol": "standard", "gpt-6-luna": "light"},
                          "policy": {"gpt-6.1-sol": {}, "gpt-6-luna": {}}}}})
    return Store(tmp_path / ".cortex", project_root=tmp_path)


@pytest.mark.parametrize("text,identity,matched", [
    ("Используй sonnet для ревью", "claude-agent-acp/claude-sonnet-5", "sonnet"),
    ("Run the implementation on sol.", "codex-acp/gpt-6.1-sol", "sol"),
    ("Добавь в AGENTS.md требование, чтобы за audit отвечал opus.", "claude-agent-acp/opus[1m]", "opus"),
])
def test_an_owner_naming_a_model_in_plain_words_requests_it(tmp_path, text, identity, matched):
    """Unnamed models get their unique last id word as a label, and a model
    made responsible for a role or stage is requested (owner decision
    2026-10-22, after an audit asked for Opus ran on Sol)."""
    store = _two_engines(tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="plain"), text, "owner")
    grant = auto_route.user_model_request(store, task.task_id, identity)
    assert grant is not None and grant["matched"] == matched


@pytest.mark.parametrize("text", ["Use GPT-6-LUNA", "Use vendor/gpt-6-luna", "luna-extended is fine"])
def test_a_derived_label_never_matches_inside_another_id(tmp_path, text):
    store = _two_engines(tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="inside"), text, "owner")
    assert auto_route.user_model_request(store, task.task_id, "codex-acp/gpt-6-luna") is None


@pytest.mark.parametrize("text,role,expected", [
    ("Ревью делай через sonnet.", "worker-code-reviewer", "claude-agent-acp/claude-sonnet-5"),
    ("За аудит отвечает Opus.", "worker-code-reviewer", "claude-agent-acp/opus[1m]"),
    ("Use luna for the tests.", "worker-qa-engineer", "codex-acp/gpt-6-luna"),
    ("Дизайн делай через opus.", "worker-code-reviewer", None),      # another purpose
    ("Не используй opus для ревью.", "worker-code-reviewer", None),  # refused
    ("Для ревью используй opus или sonnet.", "worker-code-reviewer", None),  # two models
    ("Ревью делай через sonnet.", "worker-backend-dev", None),        # not this role's purpose
])
def test_a_model_the_owner_named_for_a_purpose_reaches_unhinted_nodes(tmp_path, text, role, expected):
    """The runtime's review node has no model hint, so "review with Sonnet"
    never reached it and the owner's choice lost to pins (2026-10-22)."""
    store = _two_engines(tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="purpose"), text, "owner")
    found = auto_route.user_purpose_model_request(store, task.task_id, role)
    assert found == expected
    if expected:
        # It grants through the explicit-request path, which precedes pins.
        assert auto_route.user_model_request(store, task.task_id, expected) is not None


def test_the_newest_purpose_request_wins(tmp_path):
    store = _two_engines(tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="older"), "Ревью делай через sonnet.", "owner")
    append_source_event(store, SourceEvent(origin="test", event_id="newer"), "Теперь ревью делай через opus.",
                        task.task_id)
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-code-reviewer") == \
        "claude-agent-acp/opus[1m]"


def test_a_purpose_request_beats_a_pin_on_the_runtime_review_node(tmp_path, monkeypatch):
    """End to end through choose(): the owner's "review with Sonnet" wins over
    a '*' pin to Luna for the runtime-created review node."""
    from test_model_settings_template import entry
    from test_settings_only_routing import fake_catalog
    fake_catalog(monkeypatch, [entry("claude-agent-acp", "claude-sonnet-5"), entry("claude-agent-acp", "opus[1m]"),
                               entry("codex-acp", "gpt-6.1-sol"), entry("codex-acp", "gpt-6-luna")])
    store = _two_engines(tmp_path)
    routing = json.loads((tmp_path / ".cortex" / "routing.json").read_text())
    routing["pin"] = {"*": "codex-acp/gpt-6-luna"}
    (tmp_path / ".cortex" / "routing.json").write_text(json.dumps(routing))
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="pin"), "Ревью делай через sonnet.", "owner")
    hint = auto_route.user_purpose_model_request(store, task.task_id, "worker-code-reviewer")
    choice = auto_route.choose("worker-code-reviewer", tmp_path, {"HOME": str(tmp_path)}, model_hint=hint,
                               user_request=auto_route.user_model_request(store, task.task_id, hint))
    assert choice.model == "claude-sonnet-5" and choice.decision["decided_by"] == "user_request"
    pinned = auto_route.choose("worker-code-reviewer", tmp_path, {"HOME": str(tmp_path)})
    assert pinned.model == "gpt-6-luna"
