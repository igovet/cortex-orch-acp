"""Paseo worker history/pipeline resolve the store a worktree session used.

The runtime keys its one project store by the logical project root of the
session cwd, so a session opened in a linked ``git worktree`` writes to the
main repository's ``.cortex``. Paseo reports the worktree directory itself;
before the fix the plugin looked for ``<worktree>/.cortex/cortex.sqlite3``
and answered "No Cortex store is present in this workspace." All git and
agent state lives under ``tmp_path`` with an isolated git config and HOME.
"""
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess

import pytest

from acp_fakes import spawn_fake_agent
from cortex_runtime.contracts import StoreError
from cortex_runtime.project_storage import logical_project_root


ROOT = Path(__file__).resolve().parents[1]
SOURCE = (ROOT / "integrations/paseo/server/worker-history.ts").read_text()
PIPELINE_SOURCE = (ROOT / "integrations/paseo/server/pipeline.ts").read_text()


def _embedded(source, name):
    match = re.search(rf"const {name} = String\.raw`\n(.*?)\n`;", source, re.S)
    assert match, f"embedded {name} was not found"
    helpers = re.search(r"export const STORE_PYTHON_HELPERS = String\.raw`(.*?)`;", SOURCE, re.S)
    return match.group(1).replace("${STORE_PYTHON_HELPERS}", helpers.group(1))


def _paseo_project_root(directory):
    result = subprocess.run(["python3", "-B", "-c", _embedded(SOURCE, "PROJECT_ROOT_PYTHON"), str(directory)],
                            capture_output=True, text=True)
    return result.stdout[:-1] if result.returncode == 0 else None


def _runtime_project_root(directory):
    try:
        return logical_project_root(directory)
    except StoreError:
        return None


def _git(cwd, home, *args):
    env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", HOME=str(home))
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", *args],
                   cwd=cwd, env=env, check=True, capture_output=True)


def _repo_with_worktree(tmp_path, name="main", worktree_name="wt"):
    home = tmp_path / "home"; home.mkdir(exist_ok=True)
    repo = tmp_path / name; repo.mkdir()
    _git(repo, home, "init", "-q")
    (repo / "f.txt").write_text("hi")
    _git(repo, home, "add", "f.txt")
    _git(repo, home, "commit", "-q", "-m", "init")
    worktree = tmp_path / worktree_name
    _git(repo, home, "worktree", "add", "-q", str(worktree), "-b", f"feature-{worktree_name}")
    return repo, worktree


def _open_worktree_session(worktree):
    """Drive the real ACP agent: session/new in the worktree, then one prompt."""
    agent = spawn_fake_agent()
    try:
        def call(request_id, method, params):
            agent.stdin.write(json.dumps(dict(jsonrpc="2.0", id=request_id, method=method, params=params)) + "\n")
            agent.stdin.flush()
            while True:
                message = json.loads(agent.stdout.readline())
                if message.get("id") == request_id:
                    assert "result" in message, message
                    return message["result"]
        call(1, "initialize", dict(protocolVersion=1, clientCapabilities={}))
        session_id = call(2, "session/new", dict(cwd=str(worktree), mcpServers=[]))["sessionId"]
        call(3, "session/prompt", dict(sessionId=session_id, prompt=[dict(type="text", text="hi")]))
    finally:
        agent.stdin.close()
        agent.wait(timeout=10)
    return session_id


def test_worktree_session_history_and_pipeline_resolve_the_runtime_store(tmp_path):
    repo, worktree = _repo_with_worktree(tmp_path)
    session_id = _open_worktree_session(worktree)

    # The runtime wrote exactly one store, under the main repository; the
    # directory Paseo reports for the worktree workspace has none.
    assert not (worktree / ".cortex").exists()
    assert (repo / ".cortex" / "cortex.sqlite3").is_file()

    project_root = _paseo_project_root(worktree)
    assert project_root == str(repo)
    db = sqlite3.connect(f"file:{project_root}/.cortex/cortex.sqlite3?mode=ro", uri=True)
    row = db.execute("select task_id from acp_host_sessions where session_id=? and project_root=?",
                     (session_id, project_root)).fetchone()
    db.close()
    assert row and row[0]
    task_id = row[0]

    history = subprocess.run(["python3", "-B", "-c", _embedded(SOURCE, "PYTHON"), project_root, task_id, "", "0", "200", "", ""],
                             capture_output=True, text=True, check=True)
    assert json.loads(history.stdout)["available"] is True
    pipeline = subprocess.run(["python3", "-B", "-c", _embedded(PIPELINE_SOURCE, "PYTHON"), project_root, task_id],
                              capture_output=True, text=True, check=True)
    pipeline_output = json.loads(pipeline.stdout)
    assert pipeline_output["available"] is True
    assert pipeline_output["task"]["id"] == task_id


def test_resolve_agent_task_maps_the_workspace_directory_before_the_store_check():
    body = SOURCE[SOURCE.index("export async function resolveAgentTask"):SOURCE.index("export async function getWorkerHistory")]
    assert "const directory = workspaceHandle.directory" in body
    assert 'directory.startsWith("/")' in body and 'directory.includes("\\0")' in body
    assert body.index("resolveProjectRoot(directory)") < body.index("existsSync(`${projectRoot}/.cortex/cortex.sqlite3`)")
    assert "'/.cortex/cortex.sqlite3',safe='/')+'?mode=ro'" in SOURCE
    assert "resolveAgentTask(input, context)" in PIPELINE_SOURCE


def _layouts(tmp_path):
    plain = tmp_path / "plain"; plain.mkdir()
    repo, worktree = _repo_with_worktree(tmp_path)
    other_repo, other_worktree = _repo_with_worktree(tmp_path, "other", "other-wt")
    subdirectory = worktree / "sub"; subdirectory.mkdir()

    forged = tmp_path / "forged"; forged.mkdir()
    (forged / ".git").write_text(f"gitdir: {other_repo / '.git' / 'worktrees' / 'other-wt'}\n")
    arbitrary = tmp_path / "arbitrary"; arbitrary.mkdir()
    (arbitrary / ".git").write_text(f"gitdir: {plain}\n")
    submodule = tmp_path / "submodule"; submodule.mkdir()
    (submodule / ".git").write_text(f"gitdir: {repo / '.git'}\n")
    extra = tmp_path / "extra"
    _git(repo, tmp_path / "home", "worktree", "add", "-q", str(extra), "-b", "feature-extra")
    (extra / ".git").write_text((extra / ".git").read_text() + "extra line\n")
    linked = tmp_path / "linked"; linked.mkdir()
    (linked / ".git").symlink_to(repo / ".git")
    alias = tmp_path / "alias"; alias.symlink_to(worktree)

    return {
        "plain": (plain, str(plain)), "main": (repo, str(repo)),
        "worktree": (worktree, str(repo)), "other_worktree": (other_worktree, str(other_repo)),
        "worktree_subdirectory": (subdirectory, str(subdirectory)),
        "forged": (forged, None), "arbitrary_gitdir": (arbitrary, None),
        "submodule_style": (submodule, None), "extra_line": (extra, None),
        "symlinked_marker": (linked, None), "non_canonical": (alias, None),
        "missing": (tmp_path / "missing", None),
    }


def test_paseo_root_resolution_matches_the_runtime_for_every_layout(tmp_path):
    for name, (directory, expected) in _layouts(tmp_path).items():
        assert _runtime_project_root(directory) == expected, name
        assert _paseo_project_root(directory) == expected, name
    relative = subprocess.run(["python3", "-B", "-c", _embedded(SOURCE, "PROJECT_ROOT_PYTHON"), "plain"],
                              cwd=tmp_path, capture_output=True, text=True)
    assert relative.returncode != 0 and relative.stdout == ""


@pytest.mark.parametrize("name", ["forged", "submodule_style", "symlinked_marker", "non_canonical"])
def test_unsafe_layout_never_falls_back_to_another_store(tmp_path, name):
    directory, _expected = _layouts(tmp_path)[name]
    (tmp_path / "main" / ".cortex").mkdir()
    (tmp_path / "main" / ".cortex" / "cortex.sqlite3").write_bytes(b"")
    assert _paseo_project_root(directory) is None
