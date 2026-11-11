"""Real repository regressions: inspection cannot execute repository programs."""
import hashlib
import os
from pathlib import Path
import subprocess

import pytest
from test_workspace_machine_tools import machine, ok


def fingerprint(repo):
    return {str(p.relative_to(repo)): (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
            for p in repo.rglob("*") if p.is_file()}


@pytest.mark.parametrize("driver", ["lfs", "custom-driver"])
@pytest.mark.parametrize("missing", [True, False])
def test_snapshot_never_runs_filters_or_changes_repo(machine, driver, missing):
    repo = machine.repo
    (repo / ".gitattributes").write_text(f"*.dat filter={driver} diff=hostile\n")
    (repo / "asset.dat").write_text("version https://git-lfs.github.com/spec/v1\noid sha256:" + "a" * 64 + "\nsize 123\n")
    machine.git("add", ".")
    machine.git("commit", "-m", "Add filtered asset")
    marker = repo / "program-ran"
    command = "cw-nonexistent-filter-987654" if missing else f"touch '{marker}'; cat"
    for key in ["clean", "smudge", "process"]:
        machine.git("config", f"filter.{driver}.{key}", command)
    machine.git("config", f"filter.{driver}.required", "true")
    machine.git("config", "diff.hostile.textconv", f"touch '{marker}'; cat")
    machine.git("config", "diff.external", f"touch '{marker}'")
    machine.git("config", "core.fsmonitor", f"touch '{marker}'")
    machine.git("config", "core.hooksPath", str(repo / "hooks"))
    # Force Git to compare instead of trusting the index's stat cache.
    (repo / "asset.dat").write_text("materialized content\n")
    (repo / "a.txt").write_text("changed\n")
    before = fingerprint(repo)
    snapshot = ok(machine("repo.snapshot"))
    files = {f["path"]: f for f in snapshot["checkouts"][0]["files"]}
    assert files["asset.dat"]["filter"] == driver
    assert files["asset.dat"]["hunks"] == []
    assert files["a.txt"]["hunks"]
    assert snapshot["commits"][0]["message"] == "Add filtered asset"
    file = ok(machine("repo.file", {"file": "asset.dat"}))
    assert ("Git LFS" if driver == "lfs" else "filter") in file["reason"]
    assert file["text"] is None
    assert fingerprint(repo) == before
    assert not marker.exists()


def test_missing_required_filter_reads_do_not_weaken_writes(machine):
    repo = machine.repo
    (repo / ".gitattributes").write_text("*.dat filter=missing\n")
    (repo / "asset.dat").write_text("original\n")
    machine.git("add", ".")
    machine.git("commit", "-m", "Filtered file")
    for key in ("clean", "smudge", "process"):
        machine.git("config", f"filter.missing.{key}", "cw-absent-filter-987654")
    machine.git("config", "filter.missing.required", "true")
    (repo / "asset.dat").write_text("modified\n")
    before = fingerprint(repo)
    snapshot = ok(machine("repo.snapshot"))
    assert any(f["path"] == "asset.dat" for f in snapshot["checkouts"][0]["files"])
    assert snapshot["commits"][0]["message"] == "Filtered file"
    assert fingerprint(repo) == before
    refused = machine("git", {"action": "stage", "path": "asset.dat", "staged": True})
    assert not refused["ok"]
    assert subprocess.check_output(["git", "config", "--get", "filter.missing.required"], cwd=repo, text=True).strip() == "true"


def test_snapshot_bounds_untracked_dependencies_but_keeps_tracked_files(machine):
    repo = machine.repo
    (repo / "node_modules").mkdir()
    (repo / "node_modules/tracked.txt").write_text("tracked\n")
    machine.git("add", ".")
    machine.git("commit", "-m", "Track explicit dependency file")
    for prefix in ["node_modules", "nested/node_modules", ".venv", "nested/__pycache__"]:
        directory = repo / prefix
        directory.mkdir(parents=True, exist_ok=True)
        for n in range(600):
            (directory / f"untracked-{n}.txt").write_text("dependency\n")
    (repo / "node_modules/tracked.txt").write_text("changed\n")
    (repo / "new.txt").write_text("user file\n")
    before = fingerprint(repo)
    snapshot = ok(machine("repo.snapshot"))
    # 2,401 untracked files exceed the bound: Git's wholly untracked folders collapse, smallest
    # expanded first; files beside tracked ones are never collapsed. Names alone exclude nothing.
    loose = {f"node_modules/untracked-{n}.txt" for n in range(600)}
    venv = {f".venv/untracked-{n}.txt" for n in range(600)}
    assert {"new.txt", "node_modules/tracked.txt"} | loose | venv <= set(snapshot["tree"])
    assert not any(path.startswith("nested") for path in snapshot["tree"])
    files = {f["path"]: f for f in snapshot["checkouts"][0]["files"]}
    assert set(files) == {"new.txt", "node_modules/tracked.txt", "nested"} | loose | venv
    assert files["nested"]["stage"] == "untracked" and files["nested"]["hunks"] == []
    assert fingerprint(repo) == before
