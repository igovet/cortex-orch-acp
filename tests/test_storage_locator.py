"""StorageLocator/logical_project_root/bind_workspace regressions.

All git operations run against real ``git`` inside ``tmp_path`` only, with an
isolated config/home so the host user's real git configuration is never
touched.
"""
import os
from pathlib import Path
import subprocess

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.project_storage import StorageLocator, bind_workspace, logical_project_root


def _git(cwd, home, *args):
    env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
               HOME=str(home))
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", *args],
                    cwd=cwd, env=env, check=True, capture_output=True)


def _init_repo(tmp_path, name):
    home = tmp_path / "home"; home.mkdir(exist_ok=True)
    repo = tmp_path / name; repo.mkdir()
    _git(repo, home, "init", "-q")
    (repo / "f.txt").write_text("hi")
    _git(repo, home, "add", "f.txt")
    _git(repo, home, "commit", "-q", "-m", "init")
    return repo, home


def _add_worktree(tmp_path, repo, home, worktree_name, branch):
    worktree = tmp_path / worktree_name
    _git(repo, home, "worktree", "add", "-q", str(worktree), "-b", branch)
    return worktree


def test_main_checkout_is_its_own_logical_root(tmp_path):
    project = tmp_path / "project"; project.mkdir()
    assert logical_project_root(project) == str(project)


def test_worktree_and_main_checkout_resolve_the_same_logical_root(tmp_path):
    repo, home = _init_repo(tmp_path, "main")
    worktree = _add_worktree(tmp_path, repo, home, "wt", "feature")

    assert logical_project_root(repo) == str(repo)
    assert logical_project_root(worktree) == str(repo)

    locator = StorageLocator(repo)
    assert locator.store_dir() == repo / ".cortex"
    assert locator.sqlite_path() == repo / ".cortex" / "cortex.sqlite3"
    assert locator.verify_workspace(worktree) == worktree.resolve()
    assert locator.verify_workspace(repo) == repo.resolve()


def test_bind_workspace_gives_distinct_ids_for_main_and_worktree(tmp_path):
    repo, home = _init_repo(tmp_path, "main")
    worktree = _add_worktree(tmp_path, repo, home, "wt", "feature")
    task_id = "t_" + "a" * 12

    main_binding = bind_workspace(repo, task_id, repo)
    worktree_binding = bind_workspace(repo, task_id, worktree)

    assert main_binding.id != worktree_binding.id
    assert main_binding.task_id == worktree_binding.task_id == task_id
    assert main_binding.workspace_root == str(repo.resolve())
    assert worktree_binding.workspace_root == str(worktree.resolve())


def test_cross_project_worktree_is_rejected(tmp_path):
    repo_a, home_a = _init_repo(tmp_path, "repo-a")
    repo_b, home_b = _init_repo(tmp_path, "repo-b")
    worktree_b = _add_worktree(tmp_path, repo_b, home_b, "wt-b", "feature-b")

    with pytest.raises(StoreError, match="project_context_conflict"):
        StorageLocator(repo_a).verify_workspace(worktree_b)


def test_forged_gitdir_pointing_at_another_repos_worktree_metadata_is_rejected(tmp_path):
    repo_a, home_a = _init_repo(tmp_path, "repo-a")
    _add_worktree(tmp_path, repo_a, home_a, "wt-a", "feature-a")
    repo_b, home_b = _init_repo(tmp_path, "repo-b")
    _add_worktree(tmp_path, repo_b, home_b, "wt-b", "feature-b")

    forged = tmp_path / "forged"; forged.mkdir()
    (forged / ".git").write_text(f"gitdir: {repo_b / '.git' / 'worktrees' / 'wt-b'}\n")

    with pytest.raises(StoreError, match="project_context_conflict"):
        logical_project_root(forged)


def test_forged_gitdir_pointing_at_arbitrary_directory_is_rejected(tmp_path):
    forged = tmp_path / "forged"; forged.mkdir()
    other = tmp_path / "just-a-dir"; other.mkdir()
    (forged / ".git").write_text(f"gitdir: {other}\n")

    with pytest.raises(StoreError):
        logical_project_root(forged)


def test_forged_gitdir_traversal_outside_is_rejected(tmp_path):
    forged = tmp_path / "forged"; forged.mkdir()
    (forged / ".git").write_text("gitdir: /etc\n")

    with pytest.raises(StoreError):
        logical_project_root(forged)


def test_forged_gitdir_with_extra_lines_is_rejected(tmp_path):
    repo, home = _init_repo(tmp_path, "main")
    worktree = _add_worktree(tmp_path, repo, home, "wt", "feature")
    marker = worktree / ".git"
    marker.write_text(marker.read_text() + "extra line\n")

    with pytest.raises(StoreError, match="project_context_conflict"):
        logical_project_root(worktree)


def test_submodule_style_gitdir_without_worktrees_parent_is_rejected(tmp_path):
    repo, home = _init_repo(tmp_path, "main")
    submodule = tmp_path / "submodule"; submodule.mkdir()
    (submodule / ".git").write_text(f"gitdir: {repo / '.git'}\n")

    with pytest.raises(StoreError, match="project_context_conflict"):
        logical_project_root(submodule)


def test_symlinked_git_marker_is_rejected(tmp_path):
    repo, home = _init_repo(tmp_path, "main")
    linked = tmp_path / "linked"; linked.mkdir()
    (linked / ".git").symlink_to(repo / ".git")

    with pytest.raises(StoreError, match="unsafe_storage"):
        logical_project_root(linked)


def test_non_git_project_is_its_own_root(tmp_path):
    project = tmp_path / "plain"; project.mkdir()
    assert logical_project_root(project) == str(project)
    locator = StorageLocator(project)
    assert locator.verify_workspace(project) == project.resolve()


def test_ambiguous_project_binding_never_falls_back(tmp_path):
    repo_a, home_a = _init_repo(tmp_path, "repo-a")
    repo_b, _home_b = _init_repo(tmp_path, "repo-b")
    with pytest.raises(StoreError, match="project_context_conflict"):
        StorageLocator(repo_a).verify_workspace(repo_b)
