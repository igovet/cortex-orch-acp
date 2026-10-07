"""T029: the single redaction choke point (observability.redact), and the
real sink that routes through it (engine.logs.EngineLogs). The other former
sink, the Codex-plugin-harness-era gateway.diagnostics.safe_value, was
removed with the rest of the gateway at T033.

Mirrors tests/test_connections_no_secrets.py's style: plant a fake secret
in every place this task's own risk list names (an engine's own
environment/argv, report/source-shaped free text, and simulated engine
stderr) and assert it never reaches a written log file, while ids/status/
error-category/count fields survive unredacted.
"""
from __future__ import annotations

import json

import pytest

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


def test_scrub_text_preserves_ordinary_hyphenated_and_underscored_names():
    """Names and paths must remain usable in coordinator-facing prose."""
    names = (
        "acf-blocks-v2-iframe-compatibility",
        "acf_blocks_v2_iframe_compatibility",
        "includes/acf-blocks-v2-iframe-compat.php",
        "js/admin/acf-blocks-v2-iframe-compatibility.js",
    )
    for value in names:
        assert redact.scrub_text(value) == value


def test_scrub_text_labels_an_uncontextualized_opaque_secret():
    secret = "abcde12345fghij67890klmno"
    scrubbed = redact.scrub_text(secret)
    assert secret not in scrubbed
    assert scrubbed.startswith("[HIDDEN_SECRET:")


def test_scrub_text_never_redacts_the_ordinary_word_token_followed_by_prose():
    """H2: a bare ``token`` followed by an ordinary word is not a credential
    shape (live false positive, 2026-10-07: 'whitespace token handling' ->
    'whitespace [REDACTED_TOKEN]'). Only the real forms -- key=value/
    key: value assignment, an explicit "token is/was X" claim, or an
    authorization header -- are redacted."""
    text = "Normalize whitespace token handling and collapse repeated separators."
    assert redact.scrub_text(text) == text


def test_scrub_text_still_redacts_a_token_assignment_or_claim():
    """The bare-scheme rule's narrowing does not weaken the other two
    token-shaped redaction rules this module already has: key=value/key: value
    assignments (_KEY_VALUE_SECRET) and an explicit \"token is/was X\" claim
    (_SECRET_CLAIM) still catch a real token named exactly that."""
    assigned = redact.scrub_text(f"token={FAKE_SECRET}")
    assert FAKE_SECRET not in assigned and "[REDACTED]" in assigned
    claimed = redact.scrub_text(f"the token is {FAKE_SECRET}")
    assert FAKE_SECRET not in claimed and "[REDACTED]" in claimed


# -- H2 regression fix: authorization-scheme, assignment, claim and
# known-token coverage (fix spec r_84e20c5ca0a6) --------------------------

AUTH_HEADER_CASES = [
    ("Authorization: token dummy123456", "Authorization: [REDACTED]"),
    ("authorization: Token dummy123456 rejected", "authorization: [REDACTED] rejected"),
    ("Proxy-Authorization: token dummy123456", "Proxy-Authorization: [REDACTED]"),
    ("Authorization: Basic ZHVtbXk6ZHVtbXlwYXNz", "Authorization: [REDACTED]"),
    ("Proxy-Authorization: Basic ZHVtbXk6ZHVtbXlwYXNz", "Proxy-Authorization: [REDACTED]"),
    ("Authorization: Bearer dummy123456", "Authorization: [REDACTED]"),
    (
        'Authorization: Digest username="u", realm="r", response="dummy123456"',
        "Authorization: [REDACTED]",
    ),
    ("Authorization: Custom dummy123456", "Authorization: [REDACTED]"),
    ("Authorization : token dummy123456", "Authorization : [REDACTED]"),
    ("Authorization=token dummy123456", "Authorization=[REDACTED]"),
    (
        '-H "Authorization: token dummy123456" -d x',
        '-H "Authorization: [REDACTED]" -d x',
    ),
]


@pytest.mark.parametrize("scrub", [redact.scrub_text, redact.scrub_display_text])
@pytest.mark.parametrize("text, expected", AUTH_HEADER_CASES)
def test_t1_authorization_header_redaction_any_scheme(scrub, text, expected):
    scrubbed = scrub(text)
    assert scrubbed == expected
    assert "dummy123456" not in scrubbed
    assert "ZHVtbXk6ZHVtbXlwYXNz" not in scrubbed


@pytest.mark.parametrize("scrub", [redact.scrub_text, redact.scrub_display_text])
def test_t1_authorization_header_never_crosses_a_newline(scrub):
    text = "line1\nAuthorization: token dummy123456\nline3 token handling"
    scrubbed = scrub(text)
    assert scrubbed == "line1\nAuthorization: [REDACTED]\nline3 token handling"
    assert "dummy123456" not in scrubbed


@pytest.mark.parametrize("scrub", [redact.scrub_text, redact.scrub_display_text])
def test_t2_quoted_authorization_and_token_forms(scrub):
    assert scrub('{"Authorization": "token dummy123456", "x": 1}') == '{"Authorization": "[REDACTED]", "x": 1}'
    assert scrub("{'authorization': 'token dummy123456'}") == "{'authorization': '[REDACTED]'}"
    assert scrub('{"token": "dummy123456"}') == '{"token": "[REDACTED]"}'


@pytest.mark.parametrize("scrub", [redact.scrub_text, redact.scrub_display_text])
def test_t3_bare_scheme_redacts_only_credential_shaped_values(scrub):
    assert scrub("token dummy123456") == "[REDACTED_TOKEN]"
    assert scrub("Token dummy123456") == "[REDACTED_TOKEN]"
    scrubbed = scrub("Bearer ghp_DUMMYabcdefghijklmnopqrstuvwxyz")
    assert "ghp_DUMMYabcdefghijklmnopqrstuvwxyz" not in scrubbed


@pytest.mark.parametrize("scrub", [redact.scrub_text, redact.scrub_display_text])
def test_t4_assignments_and_claims_still_catch_a_real_token(scrub):
    for text in (
        f"token={FAKE_SECRET}",
        f"token: {FAKE_SECRET}",
        f"input_tokens: {FAKE_SECRET}",
        f"token_value: {FAKE_SECRET}",
        f"password_hash: {FAKE_SECRET}",
        f"the token is {FAKE_SECRET}",
        f"the secret was {FAKE_SECRET}",
    ):
        scrubbed = scrub(text)
        assert FAKE_SECRET not in scrubbed
        assert "[REDACTED]" in scrubbed


@pytest.mark.parametrize("scrub", [redact.scrub_text, redact.scrub_display_text])
def test_t5_known_token_formats_are_redacted_everywhere(scrub):
    known = (
        "ghp_" + "a1b2c3" * 4,
        "github_pat_" + "a1b2c3" * 4,
        "glpat-" + "a1b2c3" * 4,
        "xoxb-1234567890",
        "sk-ant-api03-" + "a1b2c3" * 4,
        "AKIA" + "0" * 16,
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
    )
    for literal in known:
        scrubbed = scrub(literal)
        assert literal not in scrubbed
        scrubbed_after_token = scrub(f"token {literal}")
        assert literal not in scrubbed_after_token


@pytest.mark.parametrize("scrub", [redact.scrub_text, redact.scrub_display_text])
def test_t5_hyphenated_sk_identifier_without_digits_is_unchanged(scrub):
    value = "sk-learn-compatibility-layer-for-blocks"
    assert scrub(value) == value


@pytest.mark.parametrize("scrub", [redact.scrub_text, redact.scrub_display_text])
def test_t6_ordinary_prose_and_paths_are_unchanged(scrub):
    unchanged = (
        "token handling",
        "the token budget is 4096 per turn",
        "Authorization header missing",
        "authorization required",
        "includes/acf-blocks-v2-iframe-compat.php",
    )
    for value in unchanged:
        assert scrub(value) == value


def test_t8_engine_logs_stderr_line_never_leaks_authorization_token_header(tmp_path):
    environ = _isolated_environ(tmp_path)
    logs = EngineLogs("eirt_" + "c" * 12, environ)
    logs.stderr_line(f"retrying with Authorization: token {FAKE_SECRET}")
    raw = logs.stderr._path.read_bytes()
    assert FAKE_SECRET.encode() not in raw


def test_t8_scrub_mapping_never_leaks_authorization_token_header():
    scrubbed = redact.scrub_mapping({"message": f"Authorization: token {FAKE_SECRET}"})
    dumped = json.dumps(scrubbed)
    assert FAKE_SECRET not in dumped


@pytest.mark.parametrize("text, _expected", AUTH_HEADER_CASES)
def test_t9_scrub_text_is_idempotent(text, _expected):
    once = redact.scrub_text(text)
    assert redact.scrub_text(once) == once


@pytest.mark.parametrize(
    "text",
    [
        f"token={FAKE_SECRET}",
        f"the token is {FAKE_SECRET}",
        '{"token": "dummy123456"}',
        "token dummy123456",
    ],
)
def test_t9_scrub_text_is_idempotent_for_assignments_and_claims(text):
    once = redact.scrub_text(text)
    assert redact.scrub_text(once) == once


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
