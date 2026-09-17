"""T019 design point 2: fault injection at file/SQLite write_report boundaries.

Every case: a real ``write_report`` call (report or pipeline) is crashed
(``os._exit(137)``, no cleanup) at a named checkpoint in a child process
(see ``fault_injection_harness``), then a fresh ``Store`` is reopened over
the same project root, ``reconcile()`` is run, and the result is checked
against the acceptance criteria: no confirmed publication is ever lost or
duplicated, unconfirmed writes are invisible (quarantined or removed),
exactly one current pipeline view with ordered revisions survives, a
same-``request_key`` retry publishes exactly once, and
``PRAGMA integrity_check`` is ``ok``.
"""
from __future__ import annotations

import uuid
from pathlib import Path

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.store import Store

from fault_injection_harness import crash_write_report
from storage_fixture import call_store, thread_for
from test_markdown_store import create, write


REPORT_POINTS = ["before_temp_write", "after_temp_fsync", "after_replace_before_commit", "after_commit_before_cleanup"]
PIPELINE_POINTS = REPORT_POINTS + ["after_backup_copy"]


def _draft(store, task, template):
    return call_store(store, "create_draft", dict(task_id=task, template=template, request_key="draft-" + uuid.uuid4().hex))


def _fill(draft, body):
    path = Path(draft["draft_path"])
    marker = path.read_bytes().split(b"\n\n", 1)[0] + b"\n\n"
    path.write_bytes(marker + body.encode())
    return path


@pytest.mark.parametrize("point", REPORT_POINTS)
def test_report_write_crash_leaves_recoverable_state(tmp_path, point):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    thread = thread_for(store, task)
    draft = _draft(store, task, "general")
    _fill(draft, "First crashed attempt.")
    result = crash_write_report(
        store.directory, tmp_path, thread, draft["draft_id"], "req-" + point, injection_point=point,
    )
    assert result.returncode == 137, result.stdout + result.stderr

    fresh = Store(store.directory)
    report = fresh.reconcile(task)
    with fresh.connection() as db:
        assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        reports_before = db.execute("SELECT COUNT(*) FROM reports WHERE task_id=? AND kind='report'", (task,)).fetchone()[0]

    if point == "before_temp_write":
        assert report["deleted_temps"] == [] and report["quarantined"] == []
    elif point == "after_temp_fsync":
        assert len(report["deleted_temps"]) == 1 and report["quarantined"] == []
    elif point == "after_replace_before_commit":
        # The metadata commit never happened (crash before the outer
        # transaction's own db.commit()); the written file is a confirmed
        # orphan -- preserved, quarantined, never auto-published.
        assert len(report["quarantined"]) == 1

    # A same request_key retry (same draft, unaffected by the crash except
    # at after_commit_before_cleanup, where it was already fully committed)
    # publishes exactly once.
    retry = call_store(fresh, "write_report", dict(
        task_id=task, title="Work", summary="A concise result.", author="worker",
        draft_id=draft["draft_id"], request_key="req-" + point,
    ))
    assert retry["report_id"]
    with fresh.connection() as db:
        reports_after = db.execute("SELECT COUNT(*) FROM reports WHERE task_id=? AND kind='report'", (task,)).fetchone()[0]
        editions = db.execute("SELECT COUNT(*) FROM editions WHERE report_id=?", (retry["report_id"],)).fetchone()[0]
    assert editions == 1
    if point == "after_commit_before_cleanup":
        assert reports_after == reports_before  # already committed before the crash
    else:
        assert reports_after == reports_before + 1
    # A second identical retry (post-commit) is the same confirmed response,
    # never a duplicate.
    again = call_store(fresh, "write_report", dict(
        task_id=task, title="Work", summary="A concise result.", author="worker",
        draft_id=draft["draft_id"], request_key="req-" + point,
    ))
    assert again["report_id"] == retry["report_id"]
    with fresh.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM editions WHERE report_id=?", (retry["report_id"],)).fetchone()[0] == 1


@pytest.mark.parametrize("point", PIPELINE_POINTS)
def test_pipeline_write_crash_preserves_one_current_view(tmp_path, point):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    thread = thread_for(store, task)
    committed = write(store, task, "Committed pipeline edition.", kind="pipeline")
    with store.connection() as db:
        committed_digest = db.execute(
            "SELECT digest FROM editions WHERE report_id=? ORDER BY sequence DESC LIMIT 1", (committed["report_id"],),
        ).fetchone()[0]

    draft = _draft(store, task, "pipeline")
    _fill(draft, "Second crashed pipeline edition.")
    result = crash_write_report(
        store.directory, tmp_path, thread, draft["draft_id"], "req-" + point, injection_point=point,
    )
    assert result.returncode == 137, result.stdout + result.stderr

    fresh = Store(store.directory)
    report = fresh.reconcile(task)
    with fresh.connection() as db:
        assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        latest_digest = db.execute(
            "SELECT digest FROM editions WHERE report_id=? ORDER BY sequence DESC LIMIT 1", (committed["report_id"],),
        ).fetchone()[0]
        edition_count_before = db.execute("SELECT COUNT(*) FROM editions WHERE report_id=?", (committed["report_id"],)).fetchone()[0]
    assert report["indeterminate"] == []
    task_dir = tmp_path / ".cortex" / task
    pipeline_path = task_dir / "pipeline.md"
    # Exactly one current pipeline.md, no stray backups, and its digest
    # matches the last *committed* edition. At after_commit_before_cleanup
    # the second edition's metadata commit already happened before the
    # crash (only cleanup remained) -- that edition is a confirmed
    # publication and must survive; at every earlier checkpoint the crash
    # is entirely pre-commit, so the original first edition is still the
    # only confirmed one.
    assert not list(task_dir.glob(".backup_pipeline_*.md"))
    from cortex_runtime.store import file_digest
    if point == "after_commit_before_cleanup":
        assert latest_digest != committed_digest
    else:
        assert latest_digest == committed_digest
    assert file_digest(pipeline_path) == latest_digest

    # Matches the crashed child's exact write_report arguments (see
    # fault_injection_harness.CHILD_TEMPLATE) so a request_key retry with
    # different args never spuriously raises delivery_conflict here.
    retry = call_store(fresh, "write_report", dict(
        task_id=task, title="Work", summary="A concise result.", author="worker",
        draft_id=draft["draft_id"], request_key="req-" + point,
    ))
    with fresh.connection() as db:
        edition_count_after = db.execute("SELECT COUNT(*) FROM editions WHERE report_id=?", (committed["report_id"],)).fetchone()[0]
    if point == "after_commit_before_cleanup":
        assert edition_count_after == edition_count_before  # already committed pre-crash
    else:
        assert edition_count_after == edition_count_before + 1
    assert not list(task_dir.glob(".backup_pipeline_*.md"))
    # One current view: exactly one pipeline.md, its digest is the latest
    # edition's digest.
    with fresh.connection() as db:
        final_digest = db.execute(
            "SELECT digest FROM editions WHERE report_id=? ORDER BY sequence DESC LIMIT 1", (committed["report_id"],),
        ).fetchone()[0]
    assert file_digest(pipeline_path) == final_digest
