"""Every question can stop its task: two plain "stop" answers once failed to
stop a task because only the coordinator interpreted them (2026-10-22)."""
import pytest

from cortex_runtime.acp import messages
from cortex_runtime.orchestration.questions import is_force_finish_answer


@pytest.mark.parametrize('answer', ['стоп', 'Стоп.', 'stop', 'Stop!', 'Остановить задачу', 'stop the task',
                                    *(messages.render('question_option_stop_task', language)
                                      for language in messages.SUPPORTED_LANGUAGES)])
def test_a_stop_answer_or_the_runtime_stop_option_stops_the_task(answer):
    assert is_force_finish_answer(answer, 'Как продолжить после сбоя?')


@pytest.mark.parametrize('answer', ['Продолжить', 'stop using the old API', 'остановить сервер и продолжить'])
def test_an_ordinary_answer_is_not_a_stop(answer):
    assert not is_force_finish_answer(answer, 'Как продолжить?')
