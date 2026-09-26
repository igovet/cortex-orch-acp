"""T020: a small, explicitly bounded message catalog for the Cortex
system lines the runtime itself authors (never a worker report, never the
coordinator's own free-form text -- those already carry whatever language
the coordinator chose and are passed through verbatim).

Selection (see ``select_language``):

1. An explicit session/task ``language`` option (``"en"`` or ``"ru"``) --
   see ``acp.commands.CORTEX_LANGUAGE`` -- always wins.
2. Otherwise ``"auto"``: the script of the most recent ``user_direct``
   source text (Cyrillic letters >= 30% of all letters -> ``"ru"``, else
   ``"en"``). This is a *limited heuristic* covering only these Cortex
   system lines -- the coordinator model itself decides everything else it
   says (see docs/acp/steering-and-questions.md's "language policy and
   heuristic limits" section).

Never emits a worker's raw report/chunk text (that would defeat this
module's entire purpose) -- callers pass only a message *key* and bounded
parameters, never worker-authored free text, through this catalog.
"""
from __future__ import annotations

from typing import Mapping

DEFAULT_LANGUAGE = "en"
SUPPORTED_LANGUAGES = ("en", "ru")

_CATALOG: dict[str, dict[str, str]] = {
    # The one entry here that is not shown to the user: it is prepended to
    # the coordinator's own prompt when the user picked a language
    # explicitly, because nothing else ever told the model which language
    # to answer in -- it simply followed the language of whatever it was
    # prompted with, so a Russian selection still produced English replies
    # for an English-worded request. Rendered in the target language, which
    # is itself the strongest instruction.
    "native_subagent_refused": {
        "en": "Refused: the engine's own subagent was used instead of delegation. "
              "It runs in the coordinator's own session and workspace, with no isolated "
              "copy, no checked integration and no audit. Delegate this work instead.",
        "ru": "Отклонено: вместо делегирования использован собственный подагент движка. "
              "Он работает в сессии и рабочей области координатора — без изолированной "
              "копии, без проверенной интеграции и без аудита. Поручите эту работу воркеру.",
    },
    "harvest_requested": {
        "en": "The user explicitly requested a knowledge harvest{scope}. Read `knowledge-harvest` "
              "with `read_skill` and carry it out for this task; this is the explicit request that "
              "skill requires, not an inference from anything said earlier.",
        "ru": "Пользователь явно запросил сбор знаний{scope}. Прочитай `knowledge-harvest` через "
              "`read_skill` и выполни его для этой задачи: это и есть явный запрос, которого требует "
              "этот скилл, а не вывод из сказанного ранее.",
    },
    "harvest_refresh_requested": {
        "en": "The user explicitly requested a documentation refresh{scope}. Read `knowledge-harvest` with `read_skill` and update the project's source-backed documentation for this task; this is an explicit request, not an inference.",
        "ru": "Пользователь явно запросил обновление документации{scope}. Прочитай `knowledge-harvest` через `read_skill` и обнови подтверждённую источниками документацию проекта для этой задачи: это явный запрос, а не вывод.",
    },
    "worker_interrupted_notice": {
        "en": "Interrupted when the host connection closed; no result yet. "
              "The next message reconciles this worker from saved state.",
        "ru": "Прервано закрытием соединения с хостом; результата ещё нет. "
              "Следующее сообщение восстановит этого исполнителя из сохранённого состояния.",
    },
    "delivered_but_not_accepted": {
        "en": "This turn ended with a worker's change integrated into the project and the task "
              "still not accepted: acceptance is a recorded decision, and none was made.",
        "ru": "Ход завершён: изменение исполнителя интегрировано в проект, но задача так и не "
              "принята — приёмка это записанное решение, и его не было.",
    },
    "answer_in_language": {
        # The second sentence is not decoration. Told only to answer in
        # Russian, the coordinator wrote its workers' briefs in Russian
        # too, and a worker's whole world -- brief, acceptance, report --
        # is English by contract. The language chosen here is the language
        # of this conversation, and of nothing else.
        "en": "Always write your replies to the user in English, whatever language this prompt is in. "
              "This is about talking to the user only: everything you send to a worker -- title, brief, "
              "acceptance criteria, clarifications -- stays in English, always.",
        "ru": "Всегда пиши свои ответы пользователю по-русски, независимо от того, на каком языке этот запрос. "
              "Это касается только общения с пользователем: всё, что ты отправляешь исполнителю — заголовок, "
              "бриф, критерии приёмки, уточнения — всегда остаётся на английском.",
    },
    "blocked_no_progress": {
        "en": "Blocked: no new worker result, question, or user input arrived after repeated waiting.",
        "ru": "Заблокировано: после многократного ожидания не появилось ни нового результата исполнителя, ни вопроса, ни ввода пользователя.",
    },
    "blocked_limit": {
        "en": "Blocked on a {kind} limit with no unfinished action or new event.",
        "ru": "Заблокировано из-за лимита {kind}: нет незавершённых действий или новых событий.",
    },
    "coordinator_turn_limit": {
        "en": "Coordinator turn ended: {kind} limit reached.",
        "ru": "Ход координатора завершён: достигнут лимит {kind}.",
    },
    "engine_failure": {
        "en": "Coordinator engine turn failed ({kind}).",
        "ru": "Ход координатора завершился ошибкой движка ({kind}).",
    },
    "coordinator_replaced_by_another_client": {
        "en": "This thread was taken over by another client, so this turn ended here. "
              "The task itself is fine and is continuing there; nothing was lost.",
        "ru": "Этот тред перехватил другой клиент, поэтому ход здесь завершён. "
              "С самой задачей всё в порядке, она продолжается там; ничего не потеряно.",
    },
    "coordinator_turn_mutated_workspace": {
        "en": "The project changed during this turn, but Cortex could not attribute the "
              "change to this task's recorded work. This does not establish who wrote it. "
              "The task remains open while its evidence is reconciled.",
        "ru": "Во время хода проект изменился, но Cortex не смог связать изменение "
              "с зафиксированной работой этой задачи. Автор записи не установлен. "
              "Задача остаётся открытой до сверки свидетельств.",
    },
    "coordinator_mutation_refused": {
        "en": "This mutating action was refused: the coordinator is a route, not a worker, and "
              "this runtime never lets it edit, delete, move, or run a destructive command "
              "against the project. Delegate this work to a worker instead of attempting it "
              "yourself.",
        "ru": "Это изменяющее действие отклонено: координатор — это маршрут, а не исполнитель, "
              "и этот рантайм никогда не позволяет ему редактировать, удалять, перемещать "
              "файлы или выполнять деструктивные команды в проекте. Делегируйте эту работу "
              "исполнителю, а не пытайтесь выполнить её сами.",
    },
    "max_total_continuations": {
        "en": "Stopping: exceeded the maximum number of coordination continuations for one prompt.",
        "ru": "Остановка: превышено максимальное число продолжений координации для одного запроса.",
    },
    "acceptance_refused_newer_event": {
        "en": "Acceptance refused: a newer {event_type} event (id {event_id}) arrived after this accept decision (id {decision_id}).",
        "ru": "Принятие отклонено: после этого решения о принятии (id {decision_id}) поступило более новое событие {event_type} (id {event_id}).",
    },
    "acceptance_refused_running": {
        "en": "Acceptance refused: assignment(s) still running/queued for this task.",
        "ru": "Принятие отклонено: по этой задаче ещё выполняются или ожидают назначения.",
    },
    "acceptance_refused_gate": {
        "en": "Acceptance refused: {reasons}",
        "ru": "Принятие отклонено: {reasons}",
    },
    "final_digest_withheld": {
        "en": "Cortex withheld a digest absent from the published pipeline. Read that report for the verified value.",
        "ru": "Cortex скрыл хеш, которого нет в опубликованном pipeline.md. Точное проверенное значение приведено в этом отчёте.",
    },
    "accepted_status_correction": {
        "en": "Cortex status: the task is accepted. The preceding wait/status text was stale.",
        "ru": "Статус Cortex: задача принята. Предыдущий текст ожидания или статуса был устаревшим.",
    },
    "resume_context": {
        "en": "Resuming after a prior wait ({wait_reason}): {explanation}",
        "ru": "Возобновление после ожидания ({wait_reason}): {explanation}",
    },
    "steering_ack": {
        "en": "Noted your new guidance (revision {revision}); the coordinator will apply it at the next checkpoint.",
        "ru": "Новое указание принято (ревизия {revision}); координатор применит его на ближайшей контрольной точке.",
    },
    "task_stopped": {
        "en": "This task was stopped and will not resume. Start a new task with /cortex-new-task <text> to continue.",
        "ru": "Эта задача остановлена и не будет возобновлена. Начните новую задачу командой /cortex-new-task <текст>.",
    },
    "question_ambiguous": {
        "en": "There is more than one open question for this task; answer with /cortex-answer <question-number> <text>.\n{questions}",
        "ru": "По этой задаче открыто более одного вопроса; ответьте командой /cortex-answer <номер вопроса> <текст>.\n{questions}",
    },
    "question_answer_unknown_index": {
        "en": "No open question with that number. Use /cortex-status or check the open questions listed above.",
        "ru": "Нет открытого вопроса с таким номером. Используйте /cortex-status или посмотрите список открытых вопросов выше.",
    },
    "question_answer_none_open": {
        "en": "There is no open question to answer right now.",
        "ru": "Сейчас нет открытого вопроса для ответа.",
    },
    "question_already_answered": {
        "en": "That question was answered concurrently; your reply was recorded but not applied. Send it again if it is new guidance.",
        "ru": "На этот вопрос уже ответили параллельно; ваш ответ сохранён, но не применён. Отправьте его снова, если это новое указание.",
    },
    "question_card_title": {
        "en": "Choose an answer",
        "ru": "Выберите вариант ответа",
    },
    "question_options_label": {
        "en": "Options:",
        "ru": "Варианты ответа:",
    },
    "stop_question": {
        "en": "The task stopped: {reason}.{details}\n\nWhat should happen next? You can also reply in your own words.",
        "ru": "Задача остановилась: {reason}.{details}\n\nЧто делать дальше? Можно ответить своими словами.",
    },
    "stop_option_continue": {
        "en": "Continue: let the coordinator decide how to proceed",
        "ru": "Продолжить: пусть координатор решит, как быть дальше",
    },
    "stop_option_finish": {
        "en": "Finish: accept the current results and close the task",
        "ru": "Завершить: принять текущие результаты и закрыть задачу",
    },
    "stop_reason.worker_failed": {
        "en": "a worker finished, but not all of its checks passed",
        "ru": "исполнитель закончил работу, но не все его проверки пройдены",
    },
    "stop_reason.verification_failed": {
        "en": "the final verification commands failed",
        "ru": "финальные проверочные команды не прошли",
    },
    "stop_reason.review_cycle_limit": {
        "en": "the review requested changes too many times",
        "ru": "ревью слишком много раз запросило доработки",
    },
    "stop_reason.review_blocked": {
        "en": "the reviewer blocked the change",
        "ru": "ревьюер заблокировал изменение",
    },
    "stop_reason.structured_proposal_missing": {
        "en": "the coordinator did not produce a plan or an answer",
        "ru": "координатор не выдал ни плана, ни ответа",
    },
    "stop_reason.coordinator_turn_failed": {
        "en": "the coordinator turn ended abnormally ({detail})",
        "ru": "ход координатора завершился нештатно ({detail})",
    },
    "stop_reason.engine_failure": {
        "en": "the coordinator engine failed ({detail})",
        "ru": "движок координатора упал ({detail})",
    },
    "stop_reason.scheduler_capacity_timeout": {
        "en": "no worker capacity became free in time",
        "ru": "не освободились слоты для исполнителей",
    },
    "stop_reason.workers_still_running": {
        "en": "workers are still running",
        "ru": "исполнители ещё работают",
    },
    "stop_reason.replanning_worker_termination_unconfirmed": {
        "en": "old workers could not be confirmed stopped",
        "ru": "не удалось подтвердить остановку прежних исполнителей",
    },
    "stop_reason.worker_retry_limit": {
        "en": "a worker failed to start or run three times in a row ({detail})",
        "ru": "исполнитель трижды подряд не смог запуститься или отработать ({detail})",
    },
    "stop_reason.worker_failure_not_retryable": {
        "en": "a worker failed in a way retrying cannot fix ({detail})",
        "ru": "исполнитель упал так, что повтор не поможет ({detail})",
    },
    "stop_reason.lost_worker_action_outcome_unknown": {
        "en": "a lost worker may have left an action half-done",
        "ru": "потерянный исполнитель мог оставить действие незавершённым",
    },
    "stop_reason.review_revision_unavailable": {
        "en": "the project state could not be captured for review",
        "ru": "не удалось зафиксировать состояние проекта для ревью",
    },
    "stop_reason.no_runnable_nodes": {
        "en": "no remaining task in the plan can run",
        "ru": "ни одна оставшаяся задача плана не может запуститься",
    },
    "stop_reason.verification_revision_unavailable": {
        "en": "the project state could not be captured for verification",
        "ru": "не удалось зафиксировать состояние проекта для проверки",
    },
    "stop_reason.verification_review_revision_changed": {
        "en": "the project changed after review",
        "ru": "проект изменился после ревью",
    },
    "worker_failed_details": {
        "en": "\n\n{node} ({report_ref}){unpassed}\n\n{summary}",
        "ru": "\n\n{node} ({report_ref}){unpassed}\n\n{summary}",
    },
    "unpassed_checks_label": {
        "en": "Unpassed checks",
        "ru": "Непройденные проверки",
    },
    "new_task_created": {
        "en": "Started a new, separate task ({task_id}); the active task is unaffected.",
        "ru": "Начата новая, отдельная задача ({task_id}); активная задача не затронута.",
    },
    "stop_confirmed": {
        "en": "Stopped this task: every assignment, queued open, and pending question was cancelled.",
        "ru": "Задача остановлена: все назначения, ожидающие запуски и открытые вопросы отменены.",
    },
    "coordinator_recovery_new_session": {
        "en": "Reconnected this task with a new coordinator session, rebuilt from recorded task state (this is a new session, not a resumed one).",
        "ru": "Задача переподключена с новой сессией координатора, восстановленной из сохранённого состояния задачи (это новая сессия, а не возобновлённая).",
    },
    "connection_closed_notice": {
        "en": "Cortex restarted after the host connection closed. Any connection error concerns the transport only; the saved task state is authoritative: {state}.",
        "ru": "Cortex перезапущен после закрытия соединения с хостом. Ошибка соединения, если она показана, касается только транспорта; достоверно сохранённое состояние задачи: {state}.",
    },
    "connection_closed_unchanged": {
        "en": " The task result is unchanged.",
        "ru": " Результат задачи не изменился.",
    },
    "connection_closed_interrupted": {
        "en": " Interrupted workers: {count}. Send a message to continue; the runtime reconciles them from saved state.",
        "ru": " Прерванных воркеров: {count}. Отправьте сообщение, чтобы продолжить; runtime сверит их по сохранённому состоянию.",
    },
    "connection_closed_unknown_actions": {
        "en": " Actions with an unknown outcome: {count}. They are not re-run automatically and need explicit reconciliation.",
        "ru": " Действий с неизвестным исходом: {count}. Они не повторяются автоматически и требуют явной сверки.",
    },
    "coordinator_recovery_new_engine": {
        "en": "Reconnected this task on a newly authorized engine/route ({engine}), with a new session rebuilt from recorded task state (this is a new session, never a native resume of the prior engine).",
        "ru": "Задача переподключена на новом разрешённом движке/маршруте ({engine}) с новой сессией, восстановленной из сохранённого состояния задачи (это новая сессия, а не нативное возобновление прежнего движка).",
    },
    "assignment_delegation_failed": {
        "en": "Assignment {assignment_ref} was NOT delegated ({error}); this work was refused before reaching a worker. If the coordinator proceeds itself, that is a fallback, not a delegation.",
        "ru": "Назначение {assignment_ref} НЕ было делегировано ({error}); эта работа была отклонена ещё до передачи исполнителю. Если координатор выполнит её сам, это резервный вариант, а не делегирование.",
    },
    "coordinator_recovery_worker_lost": {
        "en": "Assignment {assignment_ref} could not be safely resumed after recovery and was marked failed ({reason}); the coordinator may re-delegate it.",
        "ru": "Назначение {assignment_ref} не удалось безопасно возобновить после восстановления и оно помечено как неудачное ({reason}); координатор может делегировать его заново.",
    },
    "budget_overflow": {
        "en": "Context overflow on {limit_name} ({context_window_tokens} tokens, {context_window_basis}): {message} No content or constraint was silently dropped.",
        "ru": "Переполнение контекста по лимиту {limit_name} ({context_window_tokens} токенов, {context_window_basis}): {message} Ни одно содержимое или ограничение не было незаметно отброшено.",
    },
    "budget_overflow_retry": {
        "en": "Context overflow on {limit_name} ({context_window_tokens} tokens, {context_window_basis}): {message} This is not yet a permanent failure -- will retry packaging (attempt {retry_count} of {max_retries}); the engine will NOT be prompted without this mandatory context in the meantime.",
        "ru": "Переполнение контекста по лимиту {limit_name} ({context_window_tokens} токенов, {context_window_basis}): {message} Это ещё не окончательный отказ -- будет предпринята повторная попытка упаковки (попытка {retry_count} из {max_retries}); до тех пор движку НЕ будет отправлен запрос без этого обязательного контекста.",
    },
    "budget_window_unverified": {
        "en": "Cortex did not send the recovery context because its estimated {mandatory_tokens}-token mandatory package exceeds the {context_window_tokens}-token default and this route has not reported its real context window. No content was dropped; delivery will retry only after an evidenced window or a smaller package is available.",
        "ru": "Cortex не отправил контекст восстановления: обязательный пакет оценивается в {mandatory_tokens} токенов, что больше значения по умолчанию {context_window_tokens}, а маршрут ещё не сообщил реальный размер контекстного окна. Ничего не отброшено; доставка будет повторена только после появления подтверждённого размера окна или меньшего пакета.",
    },
    "budget_window_unverified_retry": {
        "en": "Cortex did not send the recovery context because this route has not reported its real context window and the estimated {mandatory_tokens}-token mandatory package exceeds the {context_window_tokens}-token default. No content was dropped; it will retry packaging (attempt {retry_count} of {max_retries}) only after an evidenced window or a smaller package is available.",
        "ru": "Cortex не отправил контекст восстановления: маршрут ещё не сообщил реальный размер контекстного окна, а обязательный пакет оценивается в {mandatory_tokens} токенов, что больше значения по умолчанию {context_window_tokens}. Ничего не отброшено; упаковка будет повторена (попытка {retry_count} из {max_retries}) только после появления подтверждённого размера окна или меньшего пакета.",
    },
    "compaction_blocked": {
        "en": "Compaction stopped ({reason}): {message}",
        "ru": "Сжатие контекста остановлено ({reason}): {message}",
    },
    "summarizer_fallback": {
        "en": "Using the deterministic recovery capsule instead of a model-authored summary ({reason}); no requirement or status was invented.",
        "ru": "Вместо резюме, составленного моделью, используется детерминированная капсула восстановления ({reason}); ни одно требование или статус не были придуманы.",
    },
}


def _cyrillic_ratio(text: str) -> float:
    letters = [ch for ch in text if ch.isalpha()]
    if not letters:
        return 0.0
    cyrillic = sum(1 for ch in letters if "Ѐ" <= ch <= "ӿ")
    return cyrillic / len(letters)


def detect_script(text: str) -> str:
    """"ru" when Cyrillic letters are >= 30% of all letters in ``text``,
    else "en". Empty/letterless text is "en" (no signal to override the
    default)."""
    return "ru" if _cyrillic_ratio(text) >= 0.30 else "en"


def select_language(explicit: str | None, recent_user_direct_texts) -> str:
    """``explicit`` (a stored ``"en"``/``"ru"``/``"auto"``/``None`` option
    value) wins when it names a supported language; ``"auto"`` or ``None``
    falls back to the script of the most recent (last in iteration order)
    non-empty ``user_direct`` source text, defaulting to English when there
    is none yet."""
    if explicit in SUPPORTED_LANGUAGES:
        return explicit
    last_text = None
    for text in recent_user_direct_texts:
        if text:
            last_text = text
    if last_text is None:
        return DEFAULT_LANGUAGE
    return detect_script(last_text)


def render(key: str, language: str, **params) -> str:
    entry = _CATALOG.get(key)
    if entry is None:
        raise KeyError(f"unknown message key {key!r}")
    template = entry.get(language, entry.get(DEFAULT_LANGUAGE))
    return template.format(**params)


class Dedup:
    """Per-task in-memory guard against repeating the exact same runtime
    status line back to back (same message key + params) -- never persisted
    (a fresh runtime instance/process starts with no memory, which only
    means a boundary case emits one extra identical line, never a lost
    one)."""

    def __init__(self) -> None:
        self._last: dict[str, tuple[str, tuple]] = {}

    def should_emit(self, task_id: str, key: str, params: Mapping[str, object]) -> bool:
        signature = (key, tuple(sorted(params.items())))
        if self._last.get(task_id) == signature:
            return False
        self._last[task_id] = signature
        return True
