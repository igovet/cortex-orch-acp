"""T029: the single redaction choke point (observability.redact), and the
two real sinks that route through it (engine.logs.EngineLogs,
gateway.diagnostics.safe_value).

Mirrors tests/test_connections_no_secrets.py's style: plant a fake secret
in every place this task's own risk list names (an engine's own
environment/argv, report/source-shaped free text, and simulated engine
stderr) and assert it never reaches a written log file, while ids/status/
error-category/count fields survive unredacted.
"""
from __future__ import annotations

import json

from cortex_runtime.gateway import diagnostics as gateway_diagnostics
from cortex_runtime.engine.logs import EngineLogs
from cortex_runtime.observability import redact

FAKE_SECRET = "fake-super-secret-value-QWERTY123456"
FAKE_BEARER = "Bearer sk-fakeSecretToken987654321zzzz"
FAKE_URL = "https://user:fakeSecretPass99@example.com/api"


def _isolated_environ(tmp_path):
    state = tmp_path / "state"
    home = tmp_path / "home"
    state.mkdir(); home.mkdir()
    return dict(XDG_STATE_HOME=str(state), HOME=str(home))


def test_scrub_text_redacts_url_bearer_and_long_secret_but_keeps_short_ids():
    text = f"connecting to {FAKE_URL} using {FAKE_BEARER} secret={FAKE_SECRET} task_id=t_1234567890ab status=ok"
    scrubbed = redact.scrub_text(text)
    assert FAKE_SECRET not in scrubbed
    assert "fakeSecretToken987654321zzzz" not in scrubbed
    assert "fakeSecretPass99" not in scrubbed
    # ids/status survive -- not everything is redacted.
    assert "task_id=t_1234567890ab" in scrubbed
    assert "status=ok" in scrubbed


def test_scrub_text_does_not_redact_ordinary_error_codes_or_event_names():
    """A digit-free snake_case identifier (an error code, an event name)
    must never be treated as secret-shaped -- 'keep ids, status, error
    category and counts' (task acceptance)."""
    for value in ("invalid_compaction_reasoning", "engine_session_unusable", "assignment_failed", "prompt_turn_abandoned"):
        assert redact.scrub_text(value) == value


def test_scrub_command_never_logs_full_argv():
    command = ["/usr/bin/codex-acp", "--api-key", FAKE_SECRET, "--project", "/home/user/work"]
    result = redact.scrub_command(command)
    assert result["argv0"] == "/usr/bin/codex-acp"
    assert FAKE_SECRET not in json.dumps(result)
    assert result["argc"] == 5
    assert "argv_hash" in result and isinstance(result["argv_hash"], str)


def test_scrub_mapping_redacts_by_denylisted_key_name_and_free_text_value():
    fields = dict(
        event="engine_started", exit_code=0, alive=True,
        ANTHROPIC_API_KEY=FAKE_SECRET,
        message=f"auth failed: {FAKE_BEARER}",
        command=["/usr/bin/claude-agent-acp", "--token", FAKE_SECRET],
    )
    scrubbed = redact.scrub_mapping(fields)
    dumped = json.dumps(scrubbed)
    assert FAKE_SECRET not in dumped
    assert "fakeSecretToken987654321zzzz" not in dumped
    assert scrubbed["event"] == "engine_started"
    assert scrubbed["exit_code"] == 0
    assert scrubbed["alive"] is True
    assert scrubbed["command"]["argv0"] == "/usr/bin/claude-agent-acp"


def test_engine_logs_observe_never_writes_the_planted_secret(tmp_path):
    environ = _isolated_environ(tmp_path)
    logs = EngineLogs("eirt_" + "a" * 12, environ)
    logs.observe(
        "starting",
        command=["/usr/bin/codex-acp", "--config", FAKE_SECRET],
        cwd="/home/user/project",
        credential_hint=FAKE_SECRET,
        engine_session_ref="ses_abc123456789",
    )
    raw = logs.observations._path.read_bytes()
    assert FAKE_SECRET.encode() not in raw
    row = json.loads(raw.splitlines()[-1])
    assert row["event"] == "starting"
    assert row["engine_session_ref"] == "ses_abc123456789"  # a short id, unredacted
    assert row["command"]["argv0"] == "/usr/bin/codex-acp"


def test_engine_logs_stderr_line_never_writes_planted_secrets_or_report_text(tmp_path):
    """Simulated engine stderr containing a credential and report-shaped
    free text -- never a content body is logged, only redacted lifecycle
    text (see module docstring: 'log refs + digests + sizes' for a body,
    never the body itself)."""
    environ = _isolated_environ(tmp_path)
    logs = EngineLogs("eirt_" + "b" * 12, environ)
    report_like_text = (
        f"Traceback: failed to load report containing 'user secret is {FAKE_SECRET}' "
        f"while authenticating with {FAKE_URL}"
    )
    logs.stderr_line(report_like_text)
    logs.stderr_line(f"retrying with {FAKE_BEARER}")
    raw = logs.stderr._path.read_bytes()
    assert FAKE_SECRET.encode() not in raw
    assert b"fakeSecretPass99" not in raw
    assert b"fakeSecretToken987654321zzzz" not in raw


def test_gateway_safe_value_redacts_credential_shaped_field_but_keeps_error_code():
    assert gateway_diagnostics.safe_value(FAKE_SECRET) != FAKE_SECRET
    assert FAKE_SECRET not in gateway_diagnostics.safe_value(f"key={FAKE_SECRET}")
    # A plain error code (task acceptance: "retain correct error provenance").
    assert gateway_diagnostics.safe_value("invalid_compaction_reasoning") == "invalid_compaction_reasoning"
    assert gateway_diagnostics.safe_value(200) == "200"
