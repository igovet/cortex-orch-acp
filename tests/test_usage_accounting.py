"""Cross-layer usage: native plan windows and complete token totals, no paid calls."""
from __future__ import annotations

import json
from pathlib import Path
import re
import sqlite3
from types import SimpleNamespace
import time
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.connections import quota
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
