"""Display only confirmed runtime routes; never consult current machine defaults."""
import json


def _object(value):
    try:
        result = json.loads(value or "{}")
    except (TypeError, ValueError):
        return {}
    return result if isinstance(result, dict) else {}


def _label(model, detail, parameters):
    if not isinstance(model, str) or not model or model == "unknown":
        return None
    name = detail.get("model_name")
    model_label = name if isinstance(name, str) and name else model + " (alias)"
    effort = next((parameters[k] for k in ("effort", "reasoning_effort", "thought_level")
                   if isinstance(parameters.get(k), str) and parameters[k]), None)
    if effort == "engine_default":
        effort = None
    labels = {"low": "Low", "medium": "Medium", "high": "High", "xhigh": "Extra high",
              "minimal": "Minimal", "max": "Max", "none": "None"}
    return dict(model=model_label, effort=labels.get(effort, effort.replace("_", " ") if effort else ""))


def observed_route(db, runtime, *, attempt_id=None, assignment_ref=None):
    tables = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    audit, detail = None, {}
    if attempt_id and "attempt_route_audit" in tables:
        audit = db.execute("SELECT * FROM attempt_route_audit WHERE task_id=? AND attempt_id=? "
                           "ORDER BY recorded_at DESC,rowid DESC LIMIT 1", (runtime, attempt_id)).fetchone()
        detail = _object(audit["decision_detail_json"]) if audit else {}
    if assignment_ref is not None:
        if not attempt_id:
            return None
        row = db.execute("SELECT payload_json FROM task_events WHERE task_id=? AND (assignment_ref=? OR assignment_ref IS NULL) "
            "AND event_type='progress' AND json_valid(payload_json) "
            "AND json_extract(payload_json,'$.kind')='worker_configuration_confirmed' "
            "AND json_extract(payload_json,'$.attempt_id')=? ORDER BY id DESC LIMIT 1",
            (runtime, assignment_ref, attempt_id)).fetchone()
        confirmed = _object(row[0]) if row else {}
        if not confirmed.get("confirmation_evidence_ref"):
            return None
        # Dispatch effort is intent. The engine may have narrowed it at configure time.
        if not audit or audit["model"] != confirmed.get("model"):
            detail = {}
        return _label(confirmed.get("model"), detail, confirmed.get("parameters") or {})
    if not {"configuration_revisions", "configuration_route_extras"} <= tables:
        return None
    row = db.execute("SELECT c.*,x.route_model FROM configuration_revisions c "
        "JOIN configuration_route_extras x ON x.revision_id=c.id AND x.task_id=c.task_id "
        "WHERE c.task_id=? AND c.state='active' AND c.confirmation_evidence_ref IS NOT NULL "
        "ORDER BY c.revision DESC LIMIT 1", (runtime,)).fetchone()
    if not row or not row["confirmation_evidence_ref"]:
        return None
    if "attempt_route_audit" in tables:
        audit = db.execute("SELECT * FROM attempt_route_audit WHERE task_id=? AND role='coordinator' "
            "AND config_revision_id=? AND model=? ORDER BY recorded_at DESC,rowid DESC LIMIT 1",
            (runtime, row["id"], row["route_model"])).fetchone()
        detail = _object(audit["decision_detail_json"]) if audit else {}
    uncertain = db.execute("SELECT 1 FROM configuration_revisions WHERE task_id=? AND revision>? "
        "AND state IN ('pending_next_use','indeterminate') LIMIT 1", (runtime, row["revision"])).fetchone()
    if uncertain:
        return None
    if "task_orchestration" in tables:
        current = db.execute("SELECT coordinator_attempt_ref FROM task_orchestration WHERE task_id=?", (runtime,)).fetchone()
        if current and current[0] and "attempt_route_audit" in tables:
            audit = db.execute("SELECT * FROM attempt_route_audit WHERE task_id=? AND attempt_id=? "
                "ORDER BY recorded_at DESC,rowid DESC LIMIT 1", (runtime, current[0])).fetchone()
            if (not audit or audit["model"] != row["route_model"] or audit["engine"] != row["route_engine"]
                    or audit["connection_id"] != row["route_connection_id"]
                    or audit["config_revision_id"] != row["id"]):
                return None
            detail = _object(audit["decision_detail_json"])
    return _label(row["route_model"], detail, _object(row["parameters"]))


def worker_route_attempts(db, runtime, attempt_id, assignment_ref):
    """Follow only the audited predecessor chain of this provisioned engine attempt."""
    columns = {r[1] for r in db.execute("PRAGMA table_info(attempt_route_audit)")}
    chain, seen = [], set()
    current = attempt_id
    while current:
        if current in seen or len(seen) >= 50:
            # An incomplete/cyclic binding cannot assert a route for another attempt.
            return []
        seen.add(current)
        chain.append(current)
        if "predecessor_attempt_id" not in columns:
            break
        row = db.execute("SELECT predecessor_attempt_id FROM attempt_route_audit WHERE task_id=? AND attempt_id=? "
                         "ORDER BY recorded_at DESC,rowid DESC LIMIT 1", (runtime, current)).fetchone()
        current = row[0] if row else None
    return [dict(n=n, route=observed_route(db, runtime, attempt_id=attempt, assignment_ref=assignment_ref))
            for n, attempt in enumerate(reversed(chain), 1)]
