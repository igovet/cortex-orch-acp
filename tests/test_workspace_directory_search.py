"""The machine's folder search, driven as the service drives it: one JSON request on stdin."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

import pytest

HELPER = Path(__file__).resolve().parents[1] / "crates/workspace-service/directory_search.py"
sys.path.insert(0, str(HELPER.parent))
import directory_search  # noqa: E402


def run(home, query, **extra):
    request = {"query": query, "home": str(home), "roots": [str(home)], **extra}
    done = subprocess.run([sys.executable, "-I", "-B", str(HELPER)], input=json.dumps(request).encode(), capture_output=True, timeout=20, cwd="/")
    assert done.returncode == 0, done.stderr
    return json.loads(done.stdout.decode().splitlines()[0])


def paths(reply):
    return [row["path"] for row in reply["entries"]]


@pytest.fixture
def home(tmp_path):
    root = tmp_path / "home"
    for name in ("Code/cedar-notes", "Code/cedar-app", "Code/maple", "Documents/cedar", "Documents/deep/deeper/thing"):
        (root / name).mkdir(parents=True)
    (root / "Code/cedar-notes/.git").mkdir()
    (root / "Code/cedar-notes/.git/config").write_text("not read\n")
    (root / "file.txt").write_text("not a folder\n")
    return root


def test_a_fragment_ranks_an_exact_name_first_then_prefixes_then_the_rest(home):
    found = paths(run(home, "cedar"))
    assert found[0] == str(home / "Documents/cedar")
    assert found[1:] == [str(home / "Code/cedar-app"), str(home / "Code/cedar-notes")]


def test_fragments_are_case_blind_and_match_across_the_path(home):
    assert paths(run(home, "CEDAR-N")) == [str(home / "Code/cedar-notes")]
    assert str(home / "Documents/deep/deeper/thing") in paths(run(home, "deep/deeper"))


def test_a_scattered_fragment_still_finds_the_folder(home):
    assert paths(run(home, "cdrnts")) == [str(home / "Code/cedar-notes")]


def test_git_repositories_are_marked_and_nothing_else_is(home):
    rows = {row["path"]: row["git"] for row in run(home, "cedar")["entries"]}
    assert rows[str(home / "Code/cedar-notes")] is True
    assert rows[str(home / "Code/cedar-app")] is False


def test_a_blank_query_walks_nothing(home):
    reply = run(home, "   ")
    assert reply["entries"] == [] and reply["scanned"] == 0 and reply["complete"] is True


@pytest.mark.parametrize("query", ["~", "~/", "."])
def test_the_home_is_listed_by_its_own_name(home, query):
    assert paths(run(home, query)) == [str(home), str(home / "Code"), str(home / "Documents")]


def test_a_tilde_path_anchors_under_its_parent(home):
    assert paths(run(home, "~/Code/ced")) == [str(home / "Code/cedar-app"), str(home / "Code/cedar-notes")]


def test_a_trailing_slash_lists_that_folder_with_itself_first(home):
    assert paths(run(home, "~/Code/")) == [str(home / "Code"), str(home / "Code/cedar-app"), str(home / "Code/cedar-notes"), str(home / "Code/maple")]


def test_an_absolute_path_can_be_outside_the_home(home, tmp_path):
    (tmp_path / "disk/shared/repo").mkdir(parents=True)
    assert paths(run(home, str(tmp_path / "disk/sha"))) == [str(tmp_path / "disk/shared"), str(tmp_path / "disk/shared/repo")]


def test_a_mistyped_parent_falls_back_to_the_whole_home(home):
    assert str(home / "Code/maple") in paths(run(home, "~/Cod/maple"))


def test_a_named_path_that_is_not_a_folder_finds_no_folder(home):
    assert paths(run(home, "~/file.txt")) == []
    assert paths(run(home, "/definitely/not/here")) == []


def test_hidden_folders_stay_out_until_they_are_typed(home):
    (home / ".config/cedar-hidden").mkdir(parents=True)
    assert paths(run(home, "cedar-hidden")) == []
    assert paths(run(home, "~/.conf"))[0] == str(home / ".config")
    assert str(home / ".config/cedar-hidden") in paths(run(home, "~/.config/"))


def test_build_output_is_never_walked(home):
    for name in ("node_modules", "target", "dist", "__pycache__"):
        (home / "Code/maple" / name / "cedar-buried").mkdir(parents=True)
    assert not [p for p in paths(run(home, "cedar-buried"))]


def test_private_stores_are_never_offered(home, tmp_path):
    (home / "Code/maple/.cortex/inner").mkdir(parents=True)
    private = home / "Code/state-private"
    private.mkdir()
    assert paths(run(home, "~/Code/maple/.cortex/")) == []
    assert paths(run(home, "~/Code/maple/.cor")) == []
    assert str(private) not in paths(run(home, "state-private", excluded=[str(private)]))
    assert paths(run(home, str(private) + "/", excluded=[str(private)])) == []


def test_nothing_is_offered_outside_the_allowed_root(home, tmp_path):
    allowed = home / "Code"
    outside = tmp_path / "elsewhere"
    outside.mkdir()
    assert paths(run(home, str(outside) + "/", allowedRoot=str(allowed))) == []
    assert paths(run(home, "cedar", allowedRoot=str(allowed), roots=[str(allowed)])) == [str(allowed / "cedar-app"), str(allowed / "cedar-notes")]


def test_a_link_loop_ends_and_a_link_is_offered_once(home):
    (home / "Code/maple/back").symlink_to(home)
    (home / "Code/maple/self").symlink_to(home / "Code/maple/self")
    (home / "Code/maple/broken").symlink_to(home / "missing")
    started = time.monotonic()
    reply = run(home, "cedar")
    assert reply["complete"] is True and time.monotonic() - started < 5
    assert len(paths(reply)) == len(set(paths(reply))) == 3


def test_a_link_out_of_the_tree_is_offered_but_not_walked(home, tmp_path):
    (tmp_path / "away/cedar-inside").mkdir(parents=True)
    (home / "Code/away").symlink_to(tmp_path / "away")
    assert str(home / "Code/away") in paths(run(home, "away"))
    assert paths(run(home, "cedar-inside")) == []


@pytest.mark.skipif(os.geteuid() == 0, reason="root reads every folder")
def test_an_unreadable_folder_is_skipped_without_a_word(home):
    locked = home / "Code/locked"
    (locked / "cedar-secret").mkdir(parents=True)
    locked.chmod(0)
    try:
        reply = run(home, "cedar")
        assert reply["complete"] is True and "error" not in reply
        assert str(locked / "cedar-secret") not in paths(reply)
    finally:
        locked.chmod(0o700)


@pytest.mark.skipif(os.geteuid() == 0, reason="root reads every folder")
def test_a_path_can_pass_through_a_folder_that_cannot_be_listed(home):
    gate = home / "gate"
    repo = gate / "inner/repo"
    repo.mkdir(parents=True)
    (repo / ".git").mkdir()
    gate.chmod(0o311)
    try:
        assert paths(run(home, "~/gate/")) == [str(gate)]
        assert paths(run(home, "~/gate/inner/")) == [str(gate / "inner"), str(repo)]
        reply = run(home, "~/gate/inner/rep")
        assert reply["entries"] == [{"path": str(repo), "git": True}]
        assert paths(run(home, str(repo))) == [str(repo)]
    finally:
        gate.chmod(0o700)


def test_the_result_count_is_bounded(home):
    for index in range(40):
        (home / f"Many/cedar-{index:02}").mkdir(parents=True)
    assert len(paths(run(home, "cedar"))) == directory_search.DEFAULT_LIMIT
    assert len(paths(run(home, "cedar", limit=5))) == 5
    assert len(paths(run(home, "cedar", limit=100000))) == 43


def test_depth_is_bounded(home):
    deep = home / "/".join(f"d{n}" for n in range(16))
    (deep / "cedar-deepest").mkdir(parents=True)
    assert paths(run(home, "cedar-deepest", maxDepth=6)) == []
    assert paths(run(home, "cedar-deepest", maxDepth=20)) == [str(deep / "cedar-deepest")]


def test_the_folder_budget_stops_the_walk_and_says_so(home):
    for index in range(30):
        (home / f"Wide/n{index:02}").mkdir(parents=True)
    reply = run(home, "nothing-like-this", maxFolders=10)
    assert reply["stopped"] == "limit" and reply["complete"] is False and reply["scanned"] == 10


def test_one_huge_folder_ends_its_own_listing_not_the_search(home):
    for index in range(30):
        (home / f"Code/huge-{index:02}").mkdir(parents=True)
    reply = run(home, "thing", maxDirectoryEntries=8)
    assert reply["stopped"] == "limit" and reply["complete"] is False
    assert str(home / "Documents/deep/deeper/thing") in paths(reply)


def test_a_confident_match_ends_a_long_walk_early(home):
    for index in range(30):
        (home / f"Wide/n{index:02}").mkdir(parents=True)
    reply = run(home, "cedar", confidentAfter=5)
    assert reply["stopped"] == "confident" and reply["complete"] is True and paths(reply)


def test_the_time_budget_stops_the_walk_with_what_it_found(home):
    for index in range(300):
        (home / f"Wide/n{index:03}/cedar-{index:03}").mkdir(parents=True)
    reply = run(home, "cedar", deadlineMs=1)
    assert reply["stopped"] == "deadline" and reply["complete"] is False


def test_a_read_that_never_returns_cannot_hold_the_answer(home, tmp_path):
    script = tmp_path / "stuck.py"
    script.write_text(f"""
import os, sys, time
sys.path.insert(0, {str(HELPER.parent)!r})
import directory_search
real = os.scandir
def scandir(path):
    if str(path).endswith('Documents'):
        time.sleep(60)
    return real(path)
directory_search.os.scandir = scandir
raise SystemExit(directory_search.main())
""")
    request = {"query": "cedar", "home": str(home), "roots": [str(home)], "deadlineMs": 200}
    started = time.monotonic()
    done = subprocess.run([sys.executable, "-B", str(script)], input=json.dumps(request).encode(), capture_output=True, timeout=20)
    assert time.monotonic() - started < 10
    reply = json.loads(done.stdout.decode().splitlines()[0])
    assert reply["stopped"] == "deadline" and reply["complete"] is False
    assert str(home / "Code/cedar-notes") in paths(reply)


def test_a_name_json_cannot_carry_is_left_out(home):
    os.mkdir(os.fsencode(str(home)) + b"/cedar-\xff")
    assert paths(run(home, "cedar")) == [str(home / "Documents/cedar"), str(home / "Code/cedar-app"), str(home / "Code/cedar-notes")]


def test_no_file_is_ever_opened(home, monkeypatch):
    opened = []
    real = open
    monkeypatch.setattr("builtins.open", lambda *args, **kwargs: opened.append(args[0]) or real(*args, **kwargs))
    search = directory_search.Search({"query": "cedar", "home": str(home), "roots": [str(home)]})
    search.run()
    assert search.snapshot() and opened == []


def test_a_request_it_cannot_read_is_refused():
    done = subprocess.run([sys.executable, "-I", "-B", str(HELPER)], input=b"not json", capture_output=True, timeout=20)
    assert json.loads(done.stdout) == {"error": "invalid_directory_search"}
