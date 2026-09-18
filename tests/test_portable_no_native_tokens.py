"""Guard against native/host/vendor tokens leaking into the portable package.

The portable package (src/cortex_runtime/portable/) must
stay engine-agnostic: it must never reference a specific native engine
protocol detail, host environment variable, or vendor name.  This scans the
actual source text of every portable/*.py file for a documented denylist of
forbidden tokens, matched on word boundaries (so, for example, "codex" does
not accidentally match as a substring of an unrelated identifier, but does
match "Codex", "CODEX_HOME", or "codex_acp").
"""
from __future__ import annotations

from pathlib import Path
import re

import pytest

PORTABLE_DIR = Path(__file__).resolve().parents[1] / "src/cortex_runtime/portable"

# Documented forbidden-token list (word-boundary, case-insensitive for
# vendor/product names as noted):
#   - "x-codex-turn-metadata": a Codex-specific ACP _meta header.
#   - "CODEX_HOME": a Codex-specific host environment variable.
#   - "codex": the Codex vendor/product name (case-insensitive).
#   - "claude": the Claude vendor/product name (case-insensitive).
#   - "thread_id" / "parent_thread_id": Cortex/Codex host thread-binding
#     concepts, not portable engine concepts.
#   - "apply_patch" / "exec_command": native Codex tool names.
#   - "spawn_agent" / "send_message_to_thread": native collaboration tool
#     names.
#   - "_meta": ACP/JSON-RPC native metadata channel, opaque host data.
#   - "jetbrains": a specific IDE vendor name (case-insensitive).
#   - "mcp__": a native MCP tool-name prefix convention.
FORBIDDEN_TOKENS = (
    "x-codex-turn-metadata",
    "CODEX_HOME",
    "codex",
    "claude",
    "thread_id",
    "parent_thread_id",
    "apply_patch",
    "exec_command",
    "spawn_agent",
    "send_message_to_thread",
    "_meta",
    "jetbrains",
    "mcp__",
)


def _pattern_for(token: str) -> re.Pattern:
    # Word-boundary matching: \b treats underscores as word characters, so
    # an exact \b...\b match on "codex" will not fire inside an unrelated
    # identifier like "codexterous" containing a different word boundary,
    # while still matching "CODEX_HOME", "codex_acp", and "Codex".
    escaped = re.escape(token)
    return re.compile(rf"\b{escaped}\b", re.IGNORECASE)


def portable_source_files():
    return sorted(PORTABLE_DIR.glob("*.py"))


@pytest.mark.parametrize("path", portable_source_files(), ids=lambda path: path.name)
def test_portable_source_has_no_forbidden_native_tokens(path):
    text = path.read_text(encoding="utf-8")
    hits = []
    for token in FORBIDDEN_TOKENS:
        pattern = _pattern_for(token)
        for match in pattern.finditer(text):
            line_number = text.count("\n", 0, match.start()) + 1
            hits.append(f"{token!r} at {path.name}:{line_number}")
    assert not hits, f"forbidden native/host/vendor tokens found: {hits}"


def test_portable_directory_is_not_empty():
    assert portable_source_files(), "expected portable/*.py files to scan"
