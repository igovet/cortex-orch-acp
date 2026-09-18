"""A draft's path must be one the session that was handed it can write.

`create_draft` returns "an absolute server-created project path to edit with
native file tools", under `<project>/.cortex/pipeline-drafts`. A coordinator
whose engine runs in an isolated workspace cannot write an absolute path
outside that workspace -- its sandbox refuses it. Observed live, turn after
turn: "the file is outside the working directory I may write to, so ordinary
editing hit the sandbox limit".

The workspace carries the project's real `.cortex` (one directory, reached by
a second name), so the same file has a reachable spelling. This covers that
the rewrite happens, that it is the same file, and that it never happens when
it would not be.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "plugins" / "cortex" / "scripts"))

from cortex_runtime.server import _reachable_draft_path  # noqa: E402


def _project_with_workspace(tmp_path):
    project = tmp_path / "project"
    (project / ".cortex" / "pipeline-drafts").mkdir(parents=True)
    work = tmp_path / "work"
    work.mkdir()
    os.symlink(project / ".cortex", work / ".cortex")
    return project, work


def test_the_path_is_rewritten_through_the_workspace(tmp_path, monkeypatch):
    project, work = _project_with_workspace(tmp_path)
    draft = project / ".cortex" / "pipeline-drafts" / "d1.md"
    draft.write_text("# plan\n")
    monkeypatch.chdir(work)

    result = _reachable_draft_path({"draft_path": str(draft), "draft_id": "d1"}, str(project))

    assert result["draft_path"] == str(work / ".cortex" / "pipeline-drafts" / "d1.md")
    assert result["draft_id"] == "d1", "the rest of the receipt is untouched"
    # The same file, not a copy: writing through the returned path is the write.
    with open(result["draft_path"], "w") as handle:
        handle.write("# filled\n")
    assert draft.read_text() == "# filled\n"


def test_nothing_is_rewritten_when_the_working_directory_is_the_project(tmp_path, monkeypatch):
    project, _work = _project_with_workspace(tmp_path)
    draft = project / ".cortex" / "pipeline-drafts" / "d1.md"
    draft.write_text("# plan\n")
    monkeypatch.chdir(project)
    result = _reachable_draft_path({"draft_path": str(draft)}, str(project))
    assert result["draft_path"] == str(draft)


def test_nothing_is_rewritten_when_the_working_directory_holds_a_different_store(tmp_path, monkeypatch):
    """The rewrite is only ever safe because it is checked. A working
    directory with an unrelated `.cortex` must be left alone."""
    project, _work = _project_with_workspace(tmp_path)
    draft = project / ".cortex" / "pipeline-drafts" / "d1.md"
    draft.write_text("# plan\n")
    other = tmp_path / "other"
    (other / ".cortex").mkdir(parents=True)
    monkeypatch.chdir(other)
    result = _reachable_draft_path({"draft_path": str(draft)}, str(project))
    assert result["draft_path"] == str(draft)


def test_a_result_without_a_draft_path_is_returned_unchanged(tmp_path, monkeypatch):
    project, work = _project_with_workspace(tmp_path)
    monkeypatch.chdir(work)
    assert _reachable_draft_path({"ok": True}, str(project)) == {"ok": True}
    assert _reachable_draft_path("not a dict", str(project)) == "not a dict"
