"""Recovery never treats speculative diagnostic events as verification receipts."""
import threading

import pytest

from cortex_runtime.orchestration import controller
from test_speculative_verification import _candidate, _events, _submit


@pytest.mark.parametrize('exit_code', [0, 7])
def test_recovered_verifier_reruns_commands_instead_of_replaying_candidate(tmp_path, monkeypatch, exit_code):
    runtime, task, machine, reviewer, assignment, digest = _candidate(tmp_path)
    context = machine.context(task.task_id)
    verification = [['python3', '-B', '-c', f'raise SystemExit({exit_code})']]
    context['plan'] = dict(tasks=[], verification=verification)
    from cortex_runtime.orchestration.state import encoded
    with runtime._store.connection() as db:
        db.execute('UPDATE runtime_tasks SET plan_json=? WHERE task_id=?',
                   (encoded(context['plan']), task.task_id))
        db.commit()
    calls = []
    original = controller._verification_observation
    def observe(*args):
        calls.append(args[3])
        return original(*args)
    monkeypatch.setattr(controller, '_verification_observation', observe)
    old = controller._SpeculativeVerification(runtime, task.task_id, None)
    try:
        old.start(context, assignment, digest)
        old.thread.join(15)
        assert not old.thread.is_alive()
        assert any(t == 'SPECULATIVE_VERIFICATION_FINISHED' for t, _ in _events(runtime._store, task.task_id))
        _submit(runtime._store, reviewer, verdict='APPROVE')
        machine.move(task.task_id, 'VERIFYING')
        # A new turn/runtime instance owns no candidate, even though the old
        # diagnostics and complete command output remain in the store.
        recovered = controller._SpeculativeVerification(runtime, task.task_id, None)
        assert recovered.credit(machine.context(task.task_id), machine) is False
        controller._verification(runtime, task.task_id, machine, None, threading.Event())
        assert calls == [digest, digest]
        authoritative = [(t, p) for t, p in _events(runtime._store, task.task_id)
                         if t in {'VERIFICATION_PASSED', 'VERIFICATION_FAILED'}]
        assert len(authoritative) == 1
        assert authoritative[0][0] == ('VERIFICATION_PASSED' if exit_code == 0 else 'VERIFICATION_FAILED')
        assert authoritative[0][1]['checks'][0]['exit_code'] == exit_code
        assert machine.get_state(task.task_id) == ('RECONCILING' if exit_code == 0 else 'PLANNING')
    finally:
        old.close('old_turn_lost')
