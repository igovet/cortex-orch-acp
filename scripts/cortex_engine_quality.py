#!/usr/bin/env python3
"""Offline aggregate-only analysis of a project's Cortex engine quality metrics."""
import argparse
from collections import Counter, defaultdict
from pathlib import Path
import json
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
from cortex_runtime.storage.inspection import _snapshot

PROCESS_TOOLS = {"replan_request", "scope_request", "ask_question", "read_skill"}
PROCESS_WORDS = {
    "assignment", "attempt", "blocker", "check", "coordinator", "evidence",
    "receipt", "report", "review", "route", "runtime", "scope", "verification",
}
TOKEN = re.compile(r"[A-Za-z][A-Za-z0-9_-]*")


def _tables(db):
    return {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}


def _json_object(value):
    try:
        result = json.loads(value or "{}")
        return result if isinstance(result, dict) else {}
    except (TypeError, ValueError):
        return {}


def _clean_model(value):
    if not isinstance(value, str) or not value or len(value) > 128:
        return "unknown"
    if any(ord(char) < 32 for char in value):
        return "unknown"
    return value


def aggregate(db):
    """Read only allowlisted metadata and derive aggregate counters."""
    tables = _tables(db)
    if not {"runtime_tasks", "runtime_nodes", "delegation_provisioning", "attempt_route_audit"} <= tables:
        return {"models": {}, "engines": {}, "review_pairs": {}, "review_pairs_by_engine": {},
                "coordinator_replies": {"count": 0, "mean_words": None, "process_word_density": None},
                "coordinator_replies_by_engine": {"count": 0, "mean_words": None, "process_word_density": None},
                "native_instructions_by_engine": {}, "sessions_with_unresolved_catalog_entries": 0,
                "availability": "required_tables_missing"}

    audits = defaultdict(list)
    for row in db.execute(
        "SELECT attempt_id, role, model, engine, recorded_at FROM attempt_route_audit ORDER BY recorded_at"
    ):
        audits[row["attempt_id"]].append((row["role"], _clean_model(row["model"]), _clean_model(row["engine"])))

    provisions = {}
    for row in db.execute(
        "SELECT idempotency_key, attempt_id, task_id FROM delegation_provisioning"
    ):
        if row["attempt_id"]:
            provisions[row["attempt_id"]] = (row["idempotency_key"], row["task_id"])

    # Each execution node contributes once, attributed to its latest audited
    # model, and in parallel to its latest audited engine
    # (claude-agent-acp/codex-acp/opencode), so every metric below can be
    # compared per engine as well as per model.
    models = defaultdict(Counter)
    engines = defaultdict(Counter)
    attempt_model = {}
    attempt_engine = {}
    execution_attempts = set()
    for attempt_id, rows in audits.items():
        attempt_model[attempt_id] = rows[-1][1]
        attempt_engine[attempt_id] = rows[-1][2]
    for row in db.execute(
        "SELECT task_id, assignment_ref, stage, status, result_json FROM runtime_nodes"
    ):
        if row["stage"] not in {"execution", "review"}:
            continue
        provision = next((a for a, (ref, task) in provisions.items()
                          if task == row["task_id"] and ref == row["assignment_ref"]), None)
        model = attempt_model.get(provision, "unknown")
        engine = attempt_engine.get(provision, "unknown")
        if row["stage"] == "execution":
            if provision is not None:
                execution_attempts.add(provision)
            status = row["status"]
            outcome = "completed" if status in {"completed", "approved"} else (
                "failed" if status in {"failed", "cancelled"} else "no_result")
            models[model][outcome] += 1
            engines[engine][outcome] += 1
        elif row["stage"] == "review":
            continue

    # Failed task rows with an explicit latest successful final verification.
    if "runtime_events" in tables:
        failed_green = Counter()
        failed_green_engine = Counter()
        tasks = {r["task_id"]: r["state"] for r in db.execute("SELECT task_id,state FROM runtime_tasks")}
        final_status = {}
        task_plans = {
            row["task_id"]: (row["revision"], _json_object(row["plan_json"]).get("verification"))
            for row in db.execute("SELECT task_id,revision,plan_json FROM runtime_tasks")
        }
        for row in db.execute(
            "SELECT task_id,type,actor,payload_json FROM runtime_events "
            "WHERE type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id"
        ):
            # Count only an explicit runtime-owned final pass receipt whose
            # checks bind to the current plan revision and all passed.
            passed = False
            if row["type"] == "VERIFICATION_PASSED" and row["actor"] == "runtime":
                payload = _json_object(row["payload_json"])
                revision, commands = task_plans.get(row["task_id"], (None, None))
                checks = payload.get("checks")
                digest = payload.get("revision_digest")
                passed = (
                    payload.get("revision") == revision
                    and isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{64}", digest)
                    and isinstance(commands, list) and len(commands) > 0
                    and isinstance(checks, list) and len(checks) == len(commands)
                    and all(
                        isinstance(check, dict) and check.get("argv") == command
                        and type(check.get("exit_code")) is int and check["exit_code"] == 0
                        and check.get("complete") is True and check.get("revision_digest") == digest
                        and isinstance(check.get("command_ref"), str)
                        for check, command in zip(checks, commands)
                    )
                )
            final_status[row["task_id"]] = passed
        for row in db.execute(
            "SELECT task_id,assignment_ref FROM runtime_nodes "
            "WHERE stage='execution' AND status='failed'"
        ):
            if tasks.get(row["task_id"]) != "FAILED" or not final_status.get(row["task_id"]):
                continue
            attempt_id = next((a for a, (ref, task) in provisions.items()
                               if task == row["task_id"] and ref == row["assignment_ref"]), None)
            failed_green[attempt_model.get(attempt_id, "unknown")] += 1
            failed_green_engine[attempt_engine.get(attempt_id, "unknown")] += 1
        for model, count in failed_green.items():
            models[model]["failed_with_passing_final_verification"] += count
        for engine, count in failed_green_engine.items():
            engines[engine]["failed_with_passing_final_verification"] += count

    # Count calls only from the execution attempts included in the outcome
    # population above, not reviewers or coordinators using the same model.
    # Operation names are allowlisted; never inspect or emit argument payloads.
    if {"runtime_events", "trusted_channels"} <= tables:
        for row in db.execute(
            "SELECT c.attempt_id, json_extract(e.payload_json,'$.operation') AS operation "
            "FROM runtime_events e JOIN trusted_channels c "
            "ON c.channel_id=e.actor AND c.task_id=e.task_id "
            "WHERE e.type='TOOL_AUTHORIZED' AND c.attempt_id IS NOT NULL"
        ):
            if row["attempt_id"] not in execution_attempts:
                continue
            model = attempt_model.get(row["attempt_id"], "unknown")
            engine = attempt_engine.get(row["attempt_id"], "unknown")
            operation = row["operation"]
            if operation in PROCESS_TOOLS:
                models[model]["process_tool_calls"] += 1
                models[model][operation] += 1
                engines[engine]["process_tool_calls"] += 1
                engines[engine][operation] += 1
            elif operation == "command_run":
                models[model]["command_run_calls"] += 1
                engines[engine]["command_run_calls"] += 1

    # Approval rate uses only review-stage node outcome/status and route audit.
    review_totals = defaultdict(Counter)
    review_totals_by_engine = defaultdict(Counter)
    latest_nodes = db.execute(
        "SELECT task_id,revision,assignment_ref,status,result_json FROM runtime_nodes "
        "WHERE stage='review' ORDER BY revision,rowid"
    )
    for row in latest_nodes:
        attempt_id = next((a for a, (ref, task) in provisions.items()
                           if task == row["task_id"] and ref == row["assignment_ref"]), None)
        reviewer_model = attempt_model.get(attempt_id, "unknown")
        reviewer_engine = attempt_engine.get(attempt_id, "unknown")
        reviewer_role = next((role for role, _, _ in audits.get(attempt_id, [])), "")
        if "review" not in reviewer_role.casefold():
            continue
        implementer = "unknown"
        implementer_engine = "unknown"
        if "runtime_nodes" in tables:
            # Prefer the execution node sharing this review's own revision
            # (same review cycle) as the better signal; only when no
            # same-revision execution node exists do we fall back to the
            # latest execution node for the task overall. Tasks whose
            # single revision carries multiple execution nodes can still be
            # misattributed under that fallback.
            predecessor = db.execute(
                "SELECT assignment_ref FROM runtime_nodes WHERE task_id=? AND stage='execution' "
                "AND revision=? ORDER BY rowid DESC LIMIT 1", (row["task_id"], row["revision"])
            ).fetchone()
            if predecessor is None:
                predecessor = db.execute(
                    "SELECT assignment_ref FROM runtime_nodes WHERE task_id=? AND stage='execution' "
                    "ORDER BY revision DESC,rowid DESC LIMIT 1", (row["task_id"],)
                ).fetchone()
            if predecessor:
                implementer_attempt = next((a for a, (ref, task) in provisions.items()
                                            if task == row["task_id"] and ref == predecessor["assignment_ref"]), None)
                implementer = attempt_model.get(implementer_attempt, "unknown")
                implementer_engine = attempt_engine.get(implementer_attempt, "unknown")
        result = _json_object(row["result_json"])
        approved = result.get("verdict") == "APPROVE" or row["status"] == "approved"
        pair = (implementer, reviewer_model)
        review_totals[pair]["reviews"] += 1
        review_totals[pair]["approved"] += int(approved)
        engine_pair = (implementer_engine, reviewer_engine)
        review_totals_by_engine[engine_pair]["reviews"] += 1
        review_totals_by_engine[engine_pair]["approved"] += int(approved)

    # Coordinator transcript text is consumed only in memory to calculate
    # length/density; no text, message IDs, or report contents are returned.
    reply_stats = defaultdict(lambda: {"count": 0, "word_total": 0, "process_word_total": 0})
    reply_stats_by_engine = defaultdict(lambda: {"count": 0, "word_total": 0, "process_word_total": 0})
    if {"host_transcript", "acp_host_sessions", "trusted_channels"} <= tables:
        session_models = {}
        session_engines = {}
        for row in db.execute(
            "SELECT s.session_id,c.attempt_id FROM acp_host_sessions s "
            "JOIN trusted_channels c ON c.task_id=s.task_id "
            "WHERE c.role='coordinator' AND s.task_id IN "
            "(SELECT task_id FROM runtime_tasks)"
        ):
            session_models[row["session_id"]] = attempt_model.get(row["attempt_id"], "unknown")
            session_engines[row["session_id"]] = attempt_engine.get(row["attempt_id"], "unknown")
        for session_id, model in session_models.items():
            engine = session_engines.get(session_id, "unknown")
            for row in db.execute(
                "SELECT text FROM host_transcript WHERE session_id=? "
                "AND kind='agent_message'", (session_id,)
            ):
                words = TOKEN.findall(row["text"] or "")
                process_words = sum(1 for word in words if word.casefold() in PROCESS_WORDS)
                stats = reply_stats[model]
                stats["count"] += 1
                stats["word_total"] += len(words)
                stats["process_word_total"] += process_words
                engine_stats = reply_stats_by_engine[engine]
                engine_stats["count"] += 1
                engine_stats["word_total"] += len(words)
                engine_stats["process_word_total"] += process_words

    # Aggregate-only, secret-free read of the existing per-session
    # base_prompt observability event (observability.events/metrics._base_
    # prompt): "native" = this session's engine kept its own native
    # system-prompt/instructions channel (Cortex text appended alongside
    # it); "unified" = that native channel was replaced outright by
    # Cortex's own base-prompt text. unresolved_catalog_count (added to
    # the same event's detail) is a session whose bundle named a
    # situational `read_skill` catalog entry that would currently fail to
    # load -- advisory only, never blocking, and never raw skill text.
    native_instructions = defaultdict(Counter)
    sessions_with_unresolved_catalog_entries = 0
    if "observability_events" in tables:
        for row in db.execute(
            "SELECT route_engine, outcome, detail_json FROM observability_events WHERE category='base_prompt'"
        ):
            engine = _clean_model(row["route_engine"]) if row["route_engine"] else "unknown"
            mode = row["outcome"] if isinstance(row["outcome"], str) and row["outcome"] else "unknown"
            disposition = {"native": "kept", "unified": "replaced"}.get(mode, "unknown")
            native_instructions[engine][disposition] += 1
            if _json_object(row["detail_json"]).get("unresolved_catalog_count"):
                sessions_with_unresolved_catalog_entries += 1

    def _finalize_counts(grouped):
        """Render one {key: Counter} mapping into the shared outcome-share
        shape, reused identically for the model-keyed and engine-keyed
        groupings so every metric stays available split either way."""
        out = {}
        for key, counts in sorted(grouped.items()):
            total = sum(counts[k] for k in ("completed", "failed", "no_result"))
            out[key] = {
                "attempts": total,
                "completed": counts["completed"],
                "failed": counts["failed"],
                "no_result": counts["no_result"],
                "completed_share": counts["completed"] / total if total else None,
                "failed_share": counts["failed"] / total if total else None,
                "no_result_share": counts["no_result"] / total if total else None,
                "failed_with_passing_final_verification": counts["failed_with_passing_final_verification"],
                "process_tool_calls": counts["process_tool_calls"],
                "command_run_calls": counts["command_run_calls"],
                # Execution-only call totals divided by the same key's
                # execution-node attempt count. None if division is undefined.
                "process_tool_calls_per_attempt": counts["process_tool_calls"] / total if total else None,
                "command_run_calls_per_attempt": counts["command_run_calls"] / total if total else None,
                **{name: counts[name] for name in sorted(PROCESS_TOOLS)},
            }
        return out

    def _review_pairs_out(grouped):
        return {
            f"{implementer} -> {reviewer}": {
                "reviews": counts["reviews"], "approved": counts["approved"],
                "approval_rate": counts["approved"] / counts["reviews"] if counts["reviews"] else None,
            }
            for (implementer, reviewer), counts in sorted(grouped.items())
        }

    def _coordinator_replies_out(grouped):
        return {
            key: {
                "count": stats["count"],
                "mean_words": stats["word_total"] / stats["count"] if stats["count"] else None,
                "process_word_density": (
                    stats["process_word_total"] / stats["word_total"]
                    if stats["word_total"] else None
                ),
            }
            for key, stats in sorted(grouped.items())
        }

    return {
        "availability": "available",
        "models": _finalize_counts(models),
        "engines": _finalize_counts(engines),
        "review_pairs": _review_pairs_out(review_totals),
        "review_pairs_by_engine": _review_pairs_out(review_totals_by_engine),
        "coordinator_replies": _coordinator_replies_out(reply_stats),
        "coordinator_replies_by_engine": _coordinator_replies_out(reply_stats_by_engine),
        "native_instructions_by_engine": {
            engine: dict(counts) for engine, counts in sorted(native_instructions.items())
        },
        "sessions_with_unresolved_catalog_entries": sessions_with_unresolved_catalog_entries,
    }


def read_metrics(project):
    with _snapshot(str(Path(project).absolute())) as (_, db, _store):
        return aggregate(db)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", default=".", help="project root containing .cortex/cortex.sqlite3")
    args = parser.parse_args(argv)
    try:
        result = read_metrics(args.project)
    except Exception as exc:
        # Only stable exception codes are suitable for CLI display.
        code = getattr(exc, "code", None)
        if not isinstance(code, str):
            code = str(exc) if str(exc).replace("_", "").isalnum() else "analysis_unavailable"
        print(json.dumps({"availability": "unavailable", "reason": code}, sort_keys=True))
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
