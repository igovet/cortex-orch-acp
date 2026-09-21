"""Read runtime-owned assignment status during provisioning cleanup.

Completion and all evidence invariants live in orchestration.state.TaskMachine.
There is no model-owned acceptance gate or compatibility completion route.
"""


def assignment_is_terminal(db, task_id, assignment_ref):
    row=db.execute('SELECT status FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
                   (task_id,assignment_ref)).fetchone()
    return row is not None and row['status'] in {'completed','failed','cancelled'}
