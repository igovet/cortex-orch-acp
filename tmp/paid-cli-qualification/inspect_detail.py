"""Read-only metadata detail: attempt mapping, worker result checks, review verdict,
reviewer route decision, configuration revisions. No message bodies."""
import json
import sqlite3
import sys

c = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
c.row_factory = sqlite3.Row


def j(text):
    try:
        return json.loads(text) if text else None
    except ValueError:
        return None


print("== provisioning map")
for r in c.execute("select assignment_id, attempt_id, role_id, step, status, engine_session_ref, instruction_delivery, "
                   "error_code, config_revision from delegation_provisioning order by created_at"):
    print(" ", dict(r))

print("== node results")
for r in c.execute("select node_id, result_json from runtime_nodes order by node_id"):
    res = j(r["result_json"]) or {}
    checks = res.get("checks") or []
    print(" ", r["node_id"], {k: res.get(k) for k in ("verdict", "report_ref", "report_id", "status", "outcome",
                                                     "assignment_outcome") if k in res},
          "keys=", sorted(res)[:20])
    for ch in checks:
        print("     check", {k: ch.get(k) for k in ("name", "outcome", "tool_call_ref", "closed_by", "blocker")})

print("== worker_result events")
for r in c.execute("select id, assignment_ref, payload_json from task_events where event_type='worker_result' order by id"):
    p = j(r["payload_json"]) or {}
    print(" ", r["id"], (r["assignment_ref"] or "")[:16], {k: p.get(k) for k in ("verdict", "report_ref", "attempt_id",
                                                                              "outcome", "accepted") if k in p},
          "keys=", sorted(p)[:20])
    for ch in p.get("checks") or []:
        print("     check", {k: ch.get(k) for k in ("name", "outcome", "closed_by")})

print("== reviewer route decision")
for r in c.execute("select attempt_id, decision_detail_json from attempt_route_audit where role='worker-code-reviewer'"):
    rd = (j(r["decision_detail_json"]) or {}).get("route_decision") or {}
    slim = {k: v for k, v in rd.items() if k not in ("candidates",)}
    print(" ", r["attempt_id"], json.dumps(slim, ensure_ascii=False)[:4000])
    for cand in rd.get("candidates") or []:
        print("    cand", json.dumps(cand, ensure_ascii=False)[:800])

print("== design route decision (full)")
for r in c.execute("select attempt_id, decision_detail_json from attempt_route_audit where model='opus[1m]'"):
    rd = (j(r["decision_detail_json"]) or {}).get("route_decision") or {}
    print(" ", r["attempt_id"], json.dumps(rd, ensure_ascii=False)[:5000])

print("== effort per attempt")
for r in c.execute("select attempt_id, role, engine, model, decision_detail_json from attempt_route_audit order by recorded_at"):
    d = j(r["decision_detail_json"]) or {}
    print(" ", r["attempt_id"], r["role"], r["engine"], r["model"], "effort=", d.get("effort"),
          "effort_source=", d.get("effort_source"), "route_basis=", d.get("route_basis"))

print("== configuration_revisions")
cols = [x[1] for x in c.execute("pragma table_info(configuration_revisions)")]
print("  cols", cols)
for r in c.execute("select * from configuration_revisions"):
    row = dict(r)
    print(" ", {k: row[k] for k in row if k in ("revision_id", "revision", "route_engine", "route_model", "parameters_json",
                                                "state", "source", "created_at", "model", "engine")})

print("== runtime events 72 payload")
for r in c.execute("select id,type,payload_json from runtime_events where type in "
                   "('SYNTHESIS_RUNTIME_DECLINED','VERIFICATION_PASSED','SPECULATIVE_VERIFICATION_CREDITED',"
                   "'CHECK_EXECUTION_FINISHED','REVIEW_STARTED','PLAN_ACCEPTED')"):
    p = j(r["payload_json"]) or {}
    print(" ", r["id"], r["type"], json.dumps(p, ensure_ascii=False)[:1200])
