"""Regression: representative operations never create a second project root.

Runs Store create_task/draft/write (through the real MCP dispatch, matching
``storage_fixture`` patterns), hook activations from both the main checkout
and a linked worktree, ``cortex_clear``'s retention cleanup, and
``bind_workspace`` for the worktree -- then walks the whole ``tmp_path`` tree
to assert exactly one ``cortex.sqlite3`` exists (under the main project's
``.cortex``), no ``.cortex`` is created inside the worktree, and no
``.codex/cortex`` is created anywhere.
"""
import io
import json
import os
from pathlib import Path
import subprocess
import sys

import pytest

sys.path.insert(0, os.path.dirname(__file__))
from test_project_storage import NativeIndex, configured_server, create_task, dispatch, successful, identifier

import cortex_clear
from cortex_runtime.hooks import main as hook_main
from cortex_runtime.project_storage import bind_workspace


@pytest.fixture(autouse=True)
def _restore_process_umask():
    """Undo the process-wide ``os.umask`` this file's real calls leave set.

    ``Server.__init__`` (server.py) and ``cortex_clear.main()`` both call
    ``os.umask(0o077)`` and never restore it; that is pre-existing,
    out-of-scope behavior in files this task does not own. Left unguarded, a
    test file that happens to sort before ``tests/test_package.py`` in
    pytest's default collection order would leak that umask forward and
    break unrelated tests there. This file exercises both call paths, so it
    restores the umask this process had on entry after every test.
    """
    original = os.umask(0o022)
    os.umask(original)
    yield
    os.umask(original)


def _git(cwd, home, *args):
    env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
               HOME=str(home))
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", *args],
                    cwd=cwd, env=env, check=True, capture_output=True)


def _run_hook(payload):
    output, errors = io.StringIO(), io.StringIO()
    code = hook_main(io.StringIO(json.dumps(payload)), output, errors)
    return code, json.loads(output.getvalue()) if output.getvalue() else None


def _walk_sqlite_and_cortex(root):
    sqlite_files, cortex_dirs, legacy_dirs = [], [], []
    for base, directories, files in os.walk(root):
        for name in files:
            if name == "cortex.sqlite3":
                sqlite_files.append(Path(base) / name)
        for name in directories:
            if name == ".cortex":
                cortex_dirs.append(Path(base) / name)
            if Path(base).name == ".codex" and name == "cortex":
                legacy_dirs.append(Path(base) / name)
    return sqlite_files, cortex_dirs, legacy_dirs


def test_representative_operations_create_exactly_one_project_store(tmp_path, monkeypatch):
    index = NativeIndex(tmp_path, monkeypatch)
    home = tmp_path / "git-home"; home.mkdir()
    project = tmp_path / "project"; project.mkdir()
    _git(project, home, "init", "-q")
    (project / "f.txt").write_text("hi")
    _git(project, home, "add", "f.txt")
    _git(project, home, "commit", "-q", "-m", "init")
    worktree = tmp_path / "worktree"
    _git(project, home, "worktree", "add", "-q", str(worktree), "-b", "feature")

    coordinator, worker = identifier(), identifier()
    index.add(coordinator, project)
    index.add(worker, project, coordinator)
    server = configured_server("Original native request")

    created = create_task(server, project, coordinator)
    draft = successful(dispatch(server, "create_draft", {
        "template": "general", "request_key": "alt-root-draft"}, worker, coordinator))
    path = Path(draft["draft_path"])
    path.write_text(path.read_text().split("\n\n", 1)[0] + "\n\n# Evidence\n")
    successful(dispatch(server, "write_report", {
        "title": "Evidence", "summary": "Stored evidence.", "author": "worker",
        "draft_id": draft["draft_id"], "request_key": "alt-root-report"}, worker, coordinator))

    code, _ = _run_hook({"hook_event_name": "UserPromptSubmit", "session_id": coordinator,
                          "cwd": str(project), "turn_id": "turn-alt-root",
                          "prompt": "representative hook activation"})
    assert code == 0

    # A hook invoked from the linked worktree resolves to the main project's
    # own store (hooks.py's bootstrap now routes cwd through
    # logical_project_root); it is active and uses that single store, and no
    # alternate storage root is created for the worktree itself.
    code, _ = _run_hook({"hook_event_name": "UserPromptSubmit", "session_id": coordinator,
                          "cwd": str(worktree), "turn_id": "turn-alt-root-worktree",
                          "prompt": "hook from the linked worktree"})
    assert code == 0
    assert not (worktree / ".cortex").exists()
    with next(iter(server.stores.values())).connection() as database:
        assert database.execute(
            "SELECT COUNT(*) FROM hook_pending_sources").fetchone()[0] == 2

    with next(iter(server.stores.values())).connection() as database:
        task_id = database.execute("SELECT id FROM tasks").fetchone()[0]
    binding = bind_workspace(project, task_id, worktree)
    assert binding.workspace_root == str(worktree.resolve())

    argv = ["cortex_clear.py", "--project-root", str(project), "--days", "0"]
    old_argv = sys.argv
    sys.argv = argv
    try:
        cortex_clear.main()
    finally:
        sys.argv = old_argv

    sqlite_files, cortex_dirs, legacy_dirs = _walk_sqlite_and_cortex(tmp_path)
    assert sqlite_files == [project / ".cortex" / "cortex.sqlite3"]
    assert cortex_dirs == [project / ".cortex"]
    assert legacy_dirs == []
    assert not (worktree / ".cortex").exists()
    assert not (project / ".codex").exists()
