"""Cross-layer usage: native plan windows and complete token totals, no paid calls."""
from __future__ import annotations

from dataclasses import replace
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import sqlite3
from types import SimpleNamespace
import time
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.connections import onboarding, quota
from cortex_runtime.connections.model import AuthMode
from cortex_runtime.observability.usage import usage_summary
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def test_native_windows_are_reported_per_provider_and_expired_values_are_unknown(tmp_path):
    future = time.time() + 3600
    day = tmp_path / "native" / "sessions" / "2026" / "09" / "23"
    day.mkdir(parents=True)
    (day / "rollout.jsonl").write_text(json.dumps({
        "payload": {"rate_limits": {
            "primary": {"used_percent": 25, "resets_at": future},
            "secondary": {"used_percent": 80, "resets_at": future - 7200},
        }},
    }) + "\n")
    codex = SimpleNamespace(engine="codex-acp", credential_ref=SimpleNamespace(path=str(tmp_path / "native")))
    assert quota.visible_windows(codex, now=time.time()) == [
        {"name": "primary", "usedPercent": 25.0, "resetsAt": future},
    ]

    environ = {"HOME": str(tmp_path), "XDG_STATE_HOME": str(tmp_path / "state")}
    connection_id = "cn_" + "a" * 12
    quota.observe(connection_id, {
        "rateLimitType": "five_hour", "utilization": 0.4, "resetsAt": future,
    }, environ)
    quota.observe(connection_id, {
        "rateLimitType": "overage", "utilization": 0.9, "resetsAt": future,
    }, environ)
    claude = SimpleNamespace(engine="claude-agent-acp", id=connection_id)
    assert quota.visible_windows(claude, environ, now=time.time()) == [
        {"name": "five_hour", "usedPercent": 40.0, "resetsAt": future},
    ]
    assert quota.visible_windows(claude, environ, now=future + 1) == []


def _now(text):
    return datetime.strptime(text, "%Y-%m-%d %H:%M").replace(tzinfo=timezone.utc).timestamp()


def _usage_result(result_text, *, is_error=False, num_turns=0, total_cost_usd=0):
    return SimpleNamespace(stdout=json.dumps({
        "is_error": is_error, "num_turns": num_turns, "total_cost_usd": total_cost_usd, "result": result_text,
    }))


_USAGE_TEXT = (
    "You are currently using your subscription to power your Claude Code usage\n\n"
    "Current session: 4% used · resets Sep 26, 1:30pm (UTC)\n"
    "Current week (all models): 50% used · resets Sep 29, 6am (UTC)\n"
    "Current week (Fable): 2% used · resets Sep 29, 6am (UTC)\n\n"
    "What's contributing to your limits usage?\n"
    "Last 24h · 8598 requests · 158 sessions\n"
)


def test_claude_usage_cli_text_parses_session_and_week_lines():
    now = _now("2026-09-20 00:00")
    windows = quota._parse_usage_text(_USAGE_TEXT, now)
    assert windows == {
        "five_hour": {"used": 4.0, "resets_at": _now("2026-09-26 13:30")},
        "seven_day": {"used": 50.0, "resets_at": _now("2026-09-29 06:00")},
        "seven_day_fable": {"used": 2.0, "resets_at": _now("2026-09-29 06:00")},
    }


def test_claude_usage_cli_reset_time_rolls_over_into_next_year():
    now = _now("2026-12-31 23:00")
    assert quota._parse_reset_time("Jan 2, 3pm", now) == datetime(
        2027, 1, 2, 15, 0, tzinfo=timezone.utc
    ).timestamp()


def test_claude_usage_cli_unparseable_reset_leaves_window_used_but_no_reset():
    now = _now("2026-09-20 00:00")
    windows = quota._parse_usage_text(
        "Current session: 7% used · resets whenever (UTC)\n", now,
    )
    assert windows == {"five_hour": {"used": 7.0, "resets_at": None}}


@pytest.mark.parametrize("payload", [
    {"is_error": True, "num_turns": 0, "total_cost_usd": 0, "result": _USAGE_TEXT},
    {"is_error": False, "num_turns": 1, "total_cost_usd": 0, "result": _USAGE_TEXT},
    {"is_error": False, "num_turns": 0, "total_cost_usd": 0.01, "result": _USAGE_TEXT},
    {"is_error": False, "num_turns": 0, "result": _USAGE_TEXT},  # total_cost_usd missing
    {"num_turns": 0, "total_cost_usd": 0, "result": _USAGE_TEXT},  # is_error missing
])
def test_claude_usage_cli_guard_rejects_anything_but_a_free_zero_turn_local_result(payload):
    completed = SimpleNamespace(stdout=json.dumps(payload))
    assert quota._parse_claude_usage_result(completed, time.time()) is None


def test_claude_usage_cli_guard_rejects_non_json_and_non_string_stdout():
    now = time.time()
    assert quota._parse_claude_usage_result(SimpleNamespace(stdout="not json"), now) is None
    assert quota._parse_claude_usage_result(SimpleNamespace(stdout=None), now) is None
    assert quota._parse_claude_usage_result(SimpleNamespace(stdout=json.dumps([1, 2])), now) is None


def _environ(tmp_path):
    return {"HOME": str(tmp_path / "home"), "XDG_STATE_HOME": str(tmp_path / "state"),
            "XDG_CONFIG_HOME": str(tmp_path / "config"), "PATH": "/usr/bin:/bin"}


def _claude_connection(environ, *, name="claude-usage"):
    """A real, registry-verified connection -- required now that the CLI's
    subprocess environment goes through
    ``connections.environment.build_process_env``, which re-verifies the
    connection against the on-disk registry rather than trusting a
    caller-held ``credential_ref``."""
    return onboarding.create_connection(
        name=name, engine="claude-agent-acp", provider="anthropic",
        auth_mode=AuthMode.NATIVE_ACCOUNT, environ=environ)


def _fake_claude_binary(monkeypatch):
    """``shutil.which`` cannot see a real ``claude`` on the test's PATH
    without pointing at the real installed binary, so the *resolution* step
    is faked here (a plain string), while ``_run_claude_usage_subprocess``
    is separately faked per test -- the real binary is never invoked."""
    monkeypatch.setattr(quota.shutil, "which", lambda *args, **kwargs: "/fake/bin/claude")


def test_claude_usage_cli_refresh_populates_windows_and_respects_ttl(tmp_path, monkeypatch):
    _fake_claude_binary(monkeypatch)
    environ = _environ(tmp_path)
    record = _claude_connection(environ)
    calls = []

    def fake_runner(argv, env, cwd, timeout):
        assert env["CLAUDE_CONFIG_DIR"] == str(
            quota.registry.connection_state_dir(record.id, record.engine, environ))
        assert env["TZ"] == "UTC"
        calls.append(argv)
        return _usage_result(_USAGE_TEXT)

    monkeypatch.setattr(quota, "_run_claude_usage_subprocess", fake_runner)
    now = _now("2026-09-20 00:00")
    assert quota.used_percent(record, None, environ, now=now) == 50.0
    assert quota.resets_at(record, None, environ, now=now) == _now("2026-09-29 06:00")
    assert len(calls) == 1

    # Still inside the TTL: no second spawn, even from a different accessor.
    assert quota.visible_windows(record, environ, now=now + 60) == [
        {"name": "five_hour", "usedPercent": 4.0, "resetsAt": _now("2026-09-26 13:30")},
        {"name": "seven_day", "usedPercent": 50.0, "resetsAt": _now("2026-09-29 06:00")},
        {"name": "seven_day_fable", "usedPercent": 2.0, "resetsAt": _now("2026-09-29 06:00")},
    ]
    assert len(calls) == 1

    # Past the TTL: refreshed again.
    quota.used_percent(record, None, environ, now=now + 301)
    assert len(calls) == 2


def test_claude_usage_cli_failure_is_negatively_cached_and_never_falls_back_to_a_stale_value(tmp_path, monkeypatch):
    _fake_claude_binary(monkeypatch)
    environ = _environ(tmp_path)
    record = _claude_connection(environ)
    calls = []

    def failing_runner(argv, env, cwd, timeout):
        calls.append(argv)
        raise TimeoutError("simulated CLI timeout")

    monkeypatch.setattr(quota, "_run_claude_usage_subprocess", failing_runner)
    now = _now("2026-09-20 00:00")
    assert quota.used_percent(record, None, environ, now=now) is None
    assert len(calls) == 1
    # Still within the TTL: the failure is cached, not retried.
    assert quota.used_percent(record, None, environ, now=now + 10) is None
    assert len(calls) == 1
    # Past the TTL: retried, and still unknown (no invented stale fallback).
    assert quota.used_percent(record, None, environ, now=now + 301) is None
    assert len(calls) == 2


def test_claude_usage_cli_never_overrides_a_newer_rate_limit_event(tmp_path, monkeypatch):
    _fake_claude_binary(monkeypatch)
    environ = _environ(tmp_path)
    record = _claude_connection(environ)
    connection_id = record.id
    future = time.time() + 3600
    # observe() stamps this window's observed_at with the real wall clock.
    quota.observe(connection_id, {"rateLimitType": "five_hour", "utilization": 0.9, "resetsAt": future}, environ)

    monkeypatch.setattr(quota, "_run_claude_usage_subprocess",
                        lambda *a, **k: _usage_result(_USAGE_TEXT))
    # A CLI refresh timestamped well before the event above must not
    # overwrite it, even though its own parsed value differs.
    stale_now = time.time() - 10_000
    assert quota.used_percent(record, None, environ, now=stale_now) == 90.0


def test_claude_usage_cli_refresh_is_skipped_when_the_registry_rejects_the_connection(tmp_path, monkeypatch):
    """``build_process_env`` re-verifies the connection against the on-disk
    registry before this module trusts its ``credential_ref``; a rejection
    must leave the window unknown and must never spawn the CLI runner."""
    _fake_claude_binary(monkeypatch)

    def guard(*args, **kwargs):
        raise AssertionError("must not spawn without a registry-verified connection")
    monkeypatch.setattr(quota, "_run_claude_usage_subprocess", guard)
    environ = _environ(tmp_path)

    # Never registered in the connections registry at all.
    no_ref = SimpleNamespace(engine="claude-agent-acp", id="cn_" + "g" * 12, credential_ref=None)
    assert quota.used_percent(no_ref, None, environ) is None

    # Registered, but its credential_ref has been tampered to a path that
    # is no longer the id-derived canonical one.
    from cortex_runtime.connections.model import EngineNativeDir

    record = _claude_connection(environ, name="claude-tampered")
    tampered = replace(record, credential_ref=EngineNativeDir(path=str(tmp_path / "does-not-exist")))
    assert quota.used_percent(tampered, None, environ) is None


def test_summary_includes_calls_after_public_list_limit(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(
        store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Count calls", "owner"
    )
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.executemany(
            "INSERT INTO usage_records "
            "(id, task_id, attempt_id, engine_session_ref, call_sequence, basis, tokens_json, "
            "stop_reason, failed, created_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            (
                (f"use_{i}", task.task_id, "co_test", "session", i + 1, "engine_reported",
                 '{"inputTokens":1,"totalTokens":1}', "end_turn", 0, "now")
                for i in range(2001)
            ),
        )
        db.commit()
    summary = usage_summary(store, task.task_id)
    assert summary["total_calls"] == 2001
    assert summary["by_connection"]["unattributed"]["tokens_reported"]["totalTokens"] == 2001


def test_worker_usage_follows_audited_retry_chain():
    source = (Path(__file__).resolve().parents[1] / "integrations/paseo/server/worker-history.ts").read_text()
    helper = source.split("export const STORE_PYTHON_HELPERS = String.raw`", 1)[1].split("`;", 1)[0]
    namespace = {"json": json, "re": re}
    exec(helper, namespace)
    db = sqlite3.connect(":memory:")
    db.row_factory = sqlite3.Row
    db.execute("create table attempt_route_audit (id text, task_id text, attempt_id text, "
               "predecessor_attempt_id text, recorded_at text)")
    db.executemany("insert into attempt_route_audit values (?,?,?,?,?)", [
        ("r1", "task", "first", None, "1"),
        ("r2", "task", "second", "first", "2"),
        ("r3", "task", "third", "second", "3"),
        ("r4", "other", "foreign", "first", "4"),
    ])
    refs = namespace["usage_fallback_refs"](db, "task", ["third"])
    assert refs == ["first", "second", "third"]
    db.execute("create table usage_records (task_id text, attempt_id text, basis text, tokens_json text)")
    db.executemany("insert into usage_records values (?,?,?,?)", [
        ("task", "first", "engine_reported", '{"totalTokens":10}'),
        ("task", "second", "engine_reported", '{"totalTokens":20}'),
        ("task", "third", "engine_reported", '{"totalTokens":30}'),
        ("other", "foreign", "engine_reported", '{"totalTokens":999}'),
    ])
    selected = db.execute(
        "select basis,tokens_json from usage_records where task_id=? and attempt_id in (?,?,?)",
        ("task", *refs),
    ).fetchall()
    assert namespace["sum_tokens"](selected)["total"] == 60


def test_paseo_totals_keep_cache_separate_and_mark_unknown_calls():
    source = (Path(__file__).resolve().parents[1] / "integrations/paseo/server/worker-history.ts").read_text()
    helper = source.split("export const STORE_PYTHON_HELPERS = String.raw`", 1)[1].split("`;", 1)[0]
    namespace = {"json": json, "re": re}
    exec(helper, namespace)
    rows = [
        {"basis": "engine_reported", "tokens_json": json.dumps({
            "inputTokens": 657, "outputTokens": 97, "cachedReadTokens": 107000,
            "cachedWriteTokens": 50, "totalTokens": 107804,
        })},
        {"basis": "engine_reported", "tokens_json": json.dumps({
            "inputTokens": 1, "outputTokens": 2, "cachedReadTokens": 3, "cachedWriteTokens": 4,
        })},
        {"basis": "unknown", "tokens_json": "{}"},
    ]
    result = namespace["sum_tokens"](rows)
    assert result == {
        "input": 658, "output": 99, "cachedRead": 107003, "cachedWrite": 54,
        "total": 107814, "knownCalls": 2, "unknownCalls": 1,
    }


def test_sum_tokens_excludes_released_after_result_without_the_unavailable_alarm():
    """F1 fix: a released_after_result call is the runtime's own
    post-acceptance release pre-empting an already-successful call, never a
    genuinely unaccounted-for one -- sum_tokens must skip it entirely
    (neither knownCalls nor unknownCalls), so the card never renders its
    "call unavailable" alarm for it."""
    source = (Path(__file__).resolve().parents[1] / "integrations/paseo/server/worker-history.ts").read_text()
    helper = source.split("export const STORE_PYTHON_HELPERS = String.raw`", 1)[1].split("`;", 1)[0]
    namespace = {"json": json, "re": re}
    exec(helper, namespace)
    sum_tokens = namespace["sum_tokens"]

    # Every call for this attempt was released_after_result: nothing is
    # known or unknown, so the caller must render nothing at all.
    assert sum_tokens([{"basis": "unknown", "tokens_json": "{}", "failed": 0, "stop_reason": "released_after_result"}]) is None

    # Mixed with a genuinely unknown call: only the real unknown is counted.
    mixed_unknown = sum_tokens([
        {"basis": "unknown", "tokens_json": "{}", "failed": 0, "stop_reason": "released_after_result"},
        {"basis": "unknown", "tokens_json": "{}"},
    ])
    assert mixed_unknown == {
        "input": 0, "output": 0, "cachedRead": 0, "cachedWrite": 0,
        "total": 0, "knownCalls": 0, "unknownCalls": 1,
    }

    # Mixed with a known call: the released_after_result row contributes
    # nothing and raises no alarm alongside the real figures.
    mixed_known = sum_tokens([
        {"basis": "unknown", "tokens_json": "{}", "failed": 0, "stop_reason": "released_after_result"},
        {"basis": "engine_reported", "tokens_json": json.dumps({"inputTokens": 10, "outputTokens": 5})},
    ])
    assert mixed_known == {
        "input": 10, "output": 5, "cachedRead": 0, "cachedWrite": 0,
        "total": 15, "knownCalls": 1, "unknownCalls": 0,
    }


def test_release_usage_reopens_pre_fix_store_without_schema_migration(tmp_path, monkeypatch):
    """Frozen pre-fix CHECK vocabulary must accept release records on reopen."""
    from cortex_runtime.observability import store as usage_store
    from cortex_runtime.observability.usage import list_usage, record_call_usage

    # Create the table from the historical SQL, independent of the current
    # OBSERVABILITY_SCHEMA (so broadening that constant cannot hide a regression).
    legacy_usage_sql = """
CREATE TABLE IF NOT EXISTS usage_records (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 attempt_id TEXT, engine_session_ref TEXT NOT NULL,
 call_sequence INTEGER NOT NULL,
 basis TEXT NOT NULL CHECK(basis IN ('observed','engine_reported','derived','estimated','unknown')),
 tokens_json TEXT NOT NULL DEFAULT '{}',
 stop_reason TEXT,
 failed INTEGER NOT NULL DEFAULT 0 CHECK(failed IN (0,1)),
 created_at TEXT NOT NULL
);
"""
    historical_schema = re.sub(
        r"CREATE TABLE IF NOT EXISTS usage_records \([\s\S]*?\);",
        lambda _: legacy_usage_sql.strip(),
        usage_store.OBSERVABILITY_SCHEMA,
    )
    with monkeypatch.context() as patch:
        patch.setattr(usage_store, "OBSERVABILITY_SCHEMA", historical_schema)
        store = Store(tmp_path / ".cortex", project_root=tmp_path)
        task = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())),
                              "Legacy usage compatibility", "owner")
    record_call_usage(
        store, task_id=task.task_id, attempt_id="att", engine_session_ref="session",
        usage_basis="engine_reported", usage_values={"totalTokens": 12},
        stop_reason="end_turn", failed=False,
    )
    with store.connection() as db:
        original_sql = db.execute(
            "SELECT sql FROM sqlite_master WHERE name='usage_records'"
        ).fetchone()[0]
        version = db.execute("PRAGMA user_version").fetchone()[0]
    assert "'released_after_result'" not in original_sql

    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    for stop_reason, failed in [
        ("released_after_result", False), ("end_turn", False),
        ("released_after_result", True),
    ]:
        record_call_usage(
            store, task_id=task.task_id, attempt_id="att", engine_session_ref="session",
            usage_basis="unknown", usage_values={"totalTokens": 999},
            stop_reason=stop_reason, failed=failed,
        )
    with store.connection() as db:
        assert db.execute("SELECT sql FROM sqlite_master WHERE name='usage_records'").fetchone()[0] == original_sql
        assert db.execute("PRAGMA user_version").fetchone()[0] == version
        rows = db.execute("SELECT basis,tokens_json,failed,stop_reason FROM usage_records ORDER BY call_sequence").fetchall()
        # The real Paseo helper consumes the actual rows from the legacy store.
        source = (Path(__file__).resolve().parents[1] / "integrations/paseo/server/worker-history.ts").read_text()
        helper = source.split("export const STORE_PYTHON_HELPERS = String.raw`", 1)[1].split("`;", 1)[0]
        namespace = {"json": json, "re": re}
        exec(helper, namespace)
        assert namespace["sum_tokens"]([rows[1]]) is None
        assert namespace["sum_tokens"](rows) == {
            "input": 0, "output": 0, "cachedRead": 0, "cachedWrite": 0,
            "total": 12, "knownCalls": 1, "unknownCalls": 2,
        }
    assert [r["call_sequence"] for r in list_usage(store, task.task_id)] == [1, 2, 3, 4]
    assert all(r["tokens"] == {} for r in list_usage(store, task.task_id)[1:])
    summary = usage_summary(store, task.task_id)
    assert summary["total_calls"] == 4
    assert summary["released_after_result_calls"] == 1
    assert summary["unknown_basis_calls"] == 2
    assert summary["failed_calls"] == 1
    assert summary["by_connection"]["unattributed"]["tokens_reported"] == {"totalTokens": 12}
    assert not (tmp_path / ".cortex" / ".migration-backups").exists()
