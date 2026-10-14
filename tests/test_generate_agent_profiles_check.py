"""Generator hardening: assert exactly 23 profiles, and fail on a dropped preamble.

Scope: ``scripts/generate_agent_profiles.py``. These are the two L1/L2 fixes
from the consolidated audit (r_9731f0fc6d64 C-36 L1, L2): ``sections()``
silently dropped any text between a profile's title and its first ``##``
heading, and ``check()`` never asserted the profile count it reports.
"""
from __future__ import annotations

import json
from pathlib import Path
import shutil

import pytest

from generate_agent_profiles import PLUGIN, check, sections


@pytest.fixture
def catalogue(tmp_path):
    shutil.copytree(PLUGIN / "agent-sources", tmp_path / "agent-sources")
    (tmp_path / "skills").mkdir()
    profiles = json.loads((PLUGIN / "profiles.json").read_text())["profiles"]
    return tmp_path, profiles


def test_sections_rejects_text_between_title_and_first_heading():
    markdown = (
        "# Some Title\n\n"
        "This sentence sits between the title and the first heading.\n\n"
        "## Role and responsibility\n\nBody.\n"
    )
    with pytest.raises(ValueError, match="text between the title and its first section"):
        sections(markdown)


def test_sections_accepts_a_title_directly_followed_by_a_heading():
    markdown = (
        (PLUGIN / "agent-sources" / "general.md").read_text()
    )
    # The shipped source has no preamble; this must not raise.
    sections(markdown)


def test_build_verification_source_has_no_preamble_after_the_fix():
    markdown = (PLUGIN / "agent-sources" / "build-verification.md").read_text()
    sections(markdown)


def test_check_asserts_exactly_23_profiles(catalogue, tmp_path):
    root, profiles = catalogue
    (root / "profiles.json").write_text(json.dumps({"profiles": profiles[:-1]}))
    with pytest.raises(ValueError, match="expected exactly 23 worker profiles, found 22"):
        check(root)


def test_check_passes_with_the_real_catalogue():
    # Exercises the actual shipped sources end to end, including the new
    # exact-count assertion.
    check(PLUGIN)
