"""Read-only, metadata-only inspection of a disposable project's Cortex store.
Prints structured identities and routing fields; never message bodies."""
import collections
import json
import sqlite3
import sys

db = sys.argv[1]
c = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
c.row_factory = sqlite3.Row


def j(text):
    try:
        return json.loads(text) if text else None
    except ValueError:
        return {"_unparsed": True}


print("== tasks")
for r in c.execute("select t.id, t.created_at, rt.state, rt.revision, rt.review_cycle, rt.blocker, rt.updated_at, rt.plan_json "
                   "from tasks t left join runtime_tasks rt on rt.task_id=t.id"):
    print(dict(task_id=r["id"], created=r["created_at"], state=r["state"], revision=r["revision"],
               review_cycle=r["review_cycle"], blocker=(r["blocker"] or "")[:200], updated=r["updated_at"]))
    plan = j(r["plan_json"]) or {}
    for node in (plan.get("tasks") or []):
        print("  plan-node", {k: node.get(k) for k in ("id", "role", "work_kind", "model", "model_reason",
                                                      "complexity", "depends_on", "scope")})

print("== runtime_nodes")
for r in c.execute("select revision,node_id,role,stage,status,assignment_ref from runtime_nodes order by revision,node_id"):
    print(" ", dict(r))

print("== attempts")
for r in c.execute("select a.attempt_id,a.assignment_id,a.attempt_no,a.predecessor_attempt_id,a.status,a.failure_reason,"
                   "a.created_at from work_assignment_attempts a join work_assignments w using(assignment_id) "
                   "order by a.created_at"):
    print(" ", dict(r))

print("== route audit")
for r in c.execute("select * from attempt_route_audit order by id"):
    d = j(r["decision_detail_json"]) or {}
    rd = d.get("route_decision") or {}
    purpose = rd.get("purpose") or {}
    aff = rd.get("affinity") or {}
    print(" ", dict(id=r["id"], attempt=r["attempt_id"], role=r["role"], engine=r["engine"], model=r["model"],
                    decision=r["decision"], predecessor=r["predecessor_attempt_id"], at=r["recorded_at"]))
    print("    detail:", json.dumps({
        "rank_version": rd.get("rank_version"), "work_kind": purpose.get("work_kind") or rd.get("work_kind"),
        "purpose_status": purpose.get("status"), "purpose_preferred": purpose.get("preferred"),
        "role_tier": rd.get("role_tier"), "tier": rd.get("tier"), "effort": rd.get("effort"),
        "effort_basis": rd.get("effort_basis"), "decided_by": rd.get("decided_by"),
        "model_hint": rd.get("model_hint"), "model_hint_status": rd.get("model_hint_status"),
        "affinity": aff, "diversify_from": rd.get("diversify_from"),
        "candidates": [(x.get("route") or x.get("label") or f"{x.get('engine')}/{x.get('model')}", x.get("preferred"),
                        x.get("tier")) for x in (rd.get("candidates") or [])][:12],
        "detail_keys": sorted(d)}, ensure_ascii=False))

print("== runtime_requests")
for r in c.execute("select rowid, actor, operation, request_key, result_json from runtime_requests order by rowid"):
    res = j(r["result_json"]) or {}
    err = res.get("error") if isinstance(res, dict) else None
    print(" ", dict(rowid=r["rowid"], actor=r["actor"], op=r["operation"], key=r["request_key"][:40],
                    ok=(res.get("ok") if isinstance(res, dict) else None),
                    error=(err.get("code") if isinstance(err, dict) else err),
                    keys=sorted(res)[:12] if isinstance(res, dict) else None))

print("== runtime_events")
for r in c.execute("select id,type,actor,created_at,payload_json from runtime_events order by id"):
    p = j(r["payload_json"]) or {}
    print(" ", r["id"], r["type"], r["actor"], r["created_at"],
          {k: p.get(k) for k in ("node_id", "state", "status", "verdict", "code", "error", "reason", "outcome", "stage") if k in p})

print("== task_events types")
print(" ", dict(collections.Counter(r[0] for r in c.execute("select event_type from task_events"))))
for r in c.execute("select id,event_type,assignment_ref,created_at,payload_json from task_events where event_type like '%refus%' "
                   "or event_type like '%reject%' or event_type like '%plan%' or event_type like '%review%' "
                   "or event_type like '%verif%' or event_type like '%complet%' or event_type like '%fail%' order by id"):
    p = j(r["payload_json"]) or {}
    print(" ", r["id"], r["event_type"], (r["assignment_ref"] or "")[:16], r["created_at"],
          {k: p.get(k) for k in ("code", "error", "reason", "verdict", "outcome", "state", "status", "node_id")
           if isinstance(p, dict) and k in p})

print("== reports")
for r in c.execute("select id,kind,filename,created_at from reports order by created_at"):
    print(" ", dict(r))

print("== route selection")
for r in c.execute("select sequence,route_id,label_at_selection,selected_at from task_route_selections"):
    print(" ", dict(r))

print("== tool_result_observations")
for r in c.execute("select attempt_id,kind,title,status,count(*) n from tool_result_observations "
                   "group by attempt_id,kind,title,status order by min(id)"):
    print(" ", dict(r))

print("== delegation_provisioning cols")
print(" ", [r[1] for r in c.execute("pragma table_info(delegation_provisioning)")])

print("== usage_records")
for r in c.execute("select attempt_id,engine_session_ref,call_sequence,basis,failed,created_at from usage_records order by id"):
    print(" ", dict(r))
