"""Keep the documented Desktop qualification protocol executable and explicit."""

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def test_agents_desktop_protocol_binds_expected_result_before_plain_send():
    text = (ROOT / "AGENTS.md").read_text()

    start = text.index("Real Desktop uses `scripts/cortex-desktop-dev`")
    protocol = text[start:]

    assert "--prompt-file" in protocol
    assert "--expected-result-path README.md" in protocol
    assert "--expected-result-sha256 EXACT_RESULT_SHA256" in protocol
    assert "Before launch" in protocol
    assert "plain `send` with no `--prompt-file`" in protocol
    assert "final\nworker report" in protocol
    assert "report/public-evidence reconciliation" in protocol
    assert "audit exit code" in protocol
    assert "run plain `stop`" in protocol
    assert protocol.index("--expected-result-path") < protocol.index("plain `send`")
    assert protocol.index("report/public-evidence reconciliation") < protocol.index("running `audit`")
    assert protocol.index("audit exit code") < protocol.index("run plain `stop`")


def test_the_readme_no_longer_documents_the_desktop_procedure():
    """The README is the setup guide for an ACP agent now.

    It used to carry the Codex Desktop qualification procedure verbatim, and
    a test pinned that copy word for word. `scripts/cortex-desktop-dev` is
    kept but never run (see docs/project/host-compatibility.md), so the
    procedure is history, not instructions -- and history belongs where it is
    already recorded, in AGENTS.md, which the test above still guards. A
    first-time reader opening the README should find how to configure and run
    Cortex today, not a rehearsal for a host that was removed at T033.
    """
    text = (ROOT / "README.md").read_text()
    assert "cortex-desktop-dev" not in text
    # The procedure itself is still documented and still guarded.
    assert "Real Desktop uses `scripts/cortex-desktop-dev`" in (ROOT / "AGENTS.md").read_text()
