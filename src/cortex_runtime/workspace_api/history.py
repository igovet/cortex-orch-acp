"""Read-only, bounded projection of the runtime's durable display feed.

The shared transcript/update sequence is authoritative for display order. Raw
protocol envelopes, read bodies, thoughts and private reports never enter this projection.
Selected command/output, file targets and edit pairs are retained as bounded display text.
"""
import json
import shlex

STEP_PREFIX = "Workspace step: "
HIDDEN_STEPS = frozenset({
    "agent_thought_chunk", "thought", "thinking", "reasoning", "wait", "wait_agent",
    "task_notification", "usage_update", "current_mode_update", "config_option_update",
    "available_commands_update", "session_info_update", "turn_started", "turn_complete",
    "result_submit", "create_draft", "draft_write", "write_report", "read_skill",
    "tools_discover", "read_capsule", "task_get_context", "toolsearch",
})


def hidden_step(title):
    name = str(title or "").strip().rsplit("__", 1)[-1].rsplit(".", 1)[-1].lower()
    return name in HIDDEN_STEPS


def activity_item(kind, text, sequence, recorded_at, detail=None, status=None):
    """The same selected activity shape consumed by worker detail, never raw envelopes."""
    from datetime import datetime
    try:
        at = int(datetime.fromisoformat(recorded_at.replace("Z", "+00:00")).timestamp() * 1000)
    except (AttributeError, TypeError, ValueError):
        at = None
    raw = text.encode("utf-8")
    preview = text
    if len(raw) > 8192:
        prefix = raw[:8000].decode("utf-8", "ignore")
        preview = prefix + f"\n[Content clipped: first {len(prefix.encode('utf-8'))} of {len(raw)} UTF-8 bytes]"
    item = dict(sequence=sequence, kind="message" if kind == "user" else "note",
                text=preview, at=at)
    if detail is None:
        return item
    item.update(kind="tool", text=detail.get("title") or "Tool activity",
                args=json.dumps(detail, ensure_ascii=False), error=status == "failed")
    if detail.get("callId"):
        item["toolCallRef"] = detail["callId"]
    if detail.get("command"):
        item.update(kind="command", text=detail["command"], command=dict(
            state="native", exit=None, took=None, tail=detail.get("output", ""), reason=None))
    elif detail.get("kind") == "edit" or "newText" in detail:
        old, new = detail.get("oldText", "").splitlines(), detail.get("newText", "").splitlines()
        item.update(kind="edit", diff=dict(add=len(new), **{"del": len(old)},
                    lines=["-" + line for line in old] + ["+" + line for line in new]))
    return item


def merge_tool_update(previous, update):
    """Sparse updates enrich an input; an empty placeholder cannot erase its command."""
    merged = {**previous, **{k: v for k, v in update.items() if v is not None}}
    def enrich(old, new):
        return {**old, **{k: enrich(old.get(k), v) if isinstance(old.get(k), dict) and isinstance(v, dict) else v
                         for k, v in new.items()}}
    for key in ("rawInput", "raw_input", "input"):
        if previous.get(key) and not update.get(key):
            merged[key] = previous[key]
        elif isinstance(previous.get(key), dict) and isinstance(update.get(key), dict):
            merged[key] = enrich(previous[key], update[key])
    return merged


def tool_detail(update):
    """Select display fields only; never expose a raw protocol envelope or read body."""
    raw = update.get("rawInput", update.get("raw_input", update.get("input", {})))
    if isinstance(raw, str):
        try:
            raw = json.loads(raw)
        except ValueError:
            raw = {}
    if not isinstance(raw, dict):
        raw = {}
    if isinstance(raw.get("arguments"), dict) and raw.get("server") == "cortex":
        raw = raw["arguments"]
    elif isinstance(raw.get("input"), dict):
        raw = raw["input"]
    detail = {"title": str(update.get("title") or "Tool activity"),
              "callId": str(update.get("toolCallId") or update.get("activity_ref") or ""),
              "status": update.get("status"), "kind": update.get("tool_kind", update.get("kind"))}
    def put(name, *keys):
        for key in keys:
            if isinstance(raw.get(key), str):
                detail[name] = raw[key]
                return
    put("path", "file_path", "filePath", "path", "absolutePath")
    if "path" not in detail:
        locations = update.get("locations")
        if isinstance(locations, list) and locations and isinstance(locations[0], dict):
            path = locations[0].get("path")
            if isinstance(path, str):
                detail["path"] = path
    put("command", "command", "cmd")
    if isinstance(raw.get("command"), list) and all(isinstance(a, str) for a in raw["command"]):
        detail["command"] = shlex.join(raw["command"])
    if "command" not in detail and isinstance(raw.get("argv"), list) and all(isinstance(a, str) for a in raw["argv"]):
        detail["command"] = shlex.join(raw["argv"])
    put("query", "query", "pattern", "q")
    operation = detail["title"].rsplit("__", 1)[-1].rsplit(".", 1)[-1].lower()
    is_edit = detail.get("kind") == "edit" or operation in {
        "project_edit", "project_write", "outside_write", "edit", "write", "multiedit", "apply_patch"}
    if is_edit:
        put("oldText", "old_string", "oldString", "oldText")
        put("newText", "new_string", "newString", "newText", "content")
    edits = raw.get("edits")
    if is_edit and isinstance(edits, list):
        pairs = [e for e in edits if isinstance(e, dict)]
        detail["oldText"] = "\n".join(str(e.get("old_string", "")) for e in pairs)
        detail["newText"] = "\n".join(str(e.get("new_string", "")) for e in pairs)
    # ACP diff blocks are authoritative when no native edit input is present.
    for block in (update.get("content") if isinstance(update.get("content"), list) else []):
        if isinstance(block, dict) and block.get("type") == "diff":
            for target, key in (("path", "path"), ("oldText", "oldText"), ("newText", "newText")):
                if isinstance(block.get(key), str):
                    detail[target] = block[key]
    def output(value):
        if isinstance(value, str):
            return value
        if isinstance(value, list):
            return "\n".join(filter(None, (output(v) for v in value)))
        if isinstance(value, dict):
            for key in ("output", "text", "stdout", "formatted_output", "content"):
                if key in value:
                    return output(value[key])
        return ""
    if detail.get("command"):
        detail["output"] = output(update.get("rawOutput", update.get("raw_output", update.get("output")))) or output(update.get("content"))
    detail = {k: v for k, v in detail.items() if v is not None}
    if update.get("projection_partial"):
        detail["notice"] = "Only the last 100 updates of this call are available in this view."
    if update.get("payload_offloaded"):
        detail["notice"] = "Only a clipped preview of this step was retained in this view."
    # Keep structured text valid JSON inside both the history and worker byte budgets.
    while len(json.dumps(detail, ensure_ascii=False).encode("utf-8")) > 18000:
        key = max((k for k, v in detail.items() if isinstance(v, str)), key=lambda k: len(detail[k]))
        detail[key] = detail[key][:len(detail[key]) // 2] + "\n[Content clipped]"
    return detail

from ..workspace_contract import ContractError, validate
from .storage import WorkspaceError, digest


def display_update(update):
    kind = update.get("sessionUpdate")
    if kind == "plan":
        entries = update.get("entries", [])
        return "system", "Plan\n" + "\n".join(
            str(e.get("content", "")) + " · " + str(e.get("status", "unknown"))
            for e in entries if isinstance(e, dict)), None
    if kind in ("tool_call", "tool_call_update"):
        status = update.get("status")
        title = update.get("title") or "Tool activity"
        detail = tool_detail(update)
        if not any(key in detail for key in ("path", "command", "query", "newText")) and not update.get("toolCallId"):
            return "tool", str(title) + (" · " + status if isinstance(status, str) else ""), status
        if str(title).startswith("Worker"):
            return "tool", str(title) + (" · " + status if isinstance(status, str) else ""), status
        if not update.get("title") and kind == "tool_call_update":
            detail.pop("title", None)
        else:
            detail["title"] = str(title)
        return "tool", STEP_PREFIX + json.dumps(detail, ensure_ascii=False), status
    return "system", "Display update: " + str(kind or "unknown"), None


def page_tx(db, project_id, task_id, *, limit=100, cursor=None, byte_budget=65536):
    if not 1 <= limit <= 100:
        raise WorkspaceError("invalid_request")
    execution = db.execute(
        "SELECT e.* FROM workspace_executions e JOIN workspace_tasks t USING(task_id) "
        "WHERE t.project_id=? AND t.task_id=?", (project_id, task_id)).fetchone()
    if execution is None:
        raise WorkspaceError("not_found")
    session = execution["acp_session_id"]
    binding = digest([project_id, task_id, session, limit])[:24]
    sql = ("SELECT seq,kind,text,NULL AS update_json,created_at FROM host_transcript WHERE session_id=? "
           "UNION ALL SELECT seq,'update',NULL,update_json,created_at FROM host_updates WHERE session_id=?")
    maximum = db.execute("SELECT coalesce(max(seq),-1) FROM (" + sql + ")", (session, session)).fetchone()[0]
    after, high = -1, maximum
    if cursor is not None:
        parts = cursor.split(".") if isinstance(cursor, str) else []
        try:
            if len(parts) != 4 or parts[:2] != ["wh1", binding]:
                raise ValueError
            high, after = map(int, parts[2:])
            if str(high) != parts[2] or str(after) != parts[3] or not 0 <= after <= high <= maximum:
                raise ValueError
        except (ValueError, OverflowError):
            raise WorkspaceError("invalid_cursor") from None
    rows = db.execute("SELECT * FROM (" + sql + ") WHERE seq>? AND seq<=? ORDER BY seq LIMIT ?",
                      (session, session, after, high, limit + 1)).fetchall()
    items, consumed = [], after
    for row in rows[:limit]:
        detail = None
        kind, text, status = ("user" if row["kind"] == "user_message" else "assistant"), row["text"], None
        if row["kind"] == "update":
            try:
                update = json.loads(row["update_json"])
                ref = update.get("toolCallId")
                if ref and update.get("sessionUpdate") in ("tool_call", "tool_call_update"):
                    prior = db.execute("SELECT update_json FROM host_updates WHERE session_id=? AND seq<=? "
                                       "AND json_extract(update_json,'$.toolCallId')=? ORDER BY seq DESC LIMIT 100",
                                       (session, row["seq"], ref)).fetchall()
                    merged = {}
                    for saved in reversed(prior):
                        merged = merge_tool_update(merged, json.loads(saved["update_json"]))
                    if len(prior) >= 100:
                        initial = db.execute("SELECT update_json FROM host_updates WHERE session_id=? AND seq<=? "
                                             "AND json_extract(update_json,'$.toolCallId')=? ORDER BY seq LIMIT 1",
                                             (session, row["seq"], ref)).fetchone()
                        merged = merge_tool_update(json.loads(initial["update_json"]), merged)
                        merged["projection_partial"] = True
                    update = merged
                if (update.get("sessionUpdate") not in ("plan", "tool_call", "tool_call_update")
                        or hidden_step(update.get("title"))):
                    consumed = row["seq"]
                    continue
                if update.get("sessionUpdate") in ("tool_call", "tool_call_update"):
                    detail = tool_detail(update)
                kind, text, status = display_update(update)
            except (ValueError, TypeError, AttributeError):
                kind, text = "unknown", None
        item = dict(historyId="wh_" + digest([session, row["seq"]])[:32], sequence=str(row["seq"] + 1),
                    revision="1", kind=kind, text=text, toolStatus=status,
                    recordedAt=row["created_at"], availability="available" if text is not None else "unavailable")
        if text is not None and len(text) > 32000:
            item.update(text=None, availability="unavailable", reason="content_exceeds_wire_limit")
        if item["availability"] == "available":
            item["activity"] = activity_item(kind, text, row["seq"] + 1, row["created_at"], detail, status)
        candidate = dict(items=items + [item], nextCursor=None, highWatermark=str(high + 1))
        try:
            validate("HistoryPage", candidate)
            if len(json.dumps(candidate, ensure_ascii=False).encode()) > byte_budget - 256:
                raise ContractError("page_too_large")
        except ContractError as error:
            if str(error) != "page_too_large":
                raise
            if items:
                break
            # The convenience activity duplicates selected display text. Do not
            # lose an otherwise readable row merely because that duplicate is large.
            item.pop("activity", None)
            try:
                smaller = dict(items=[item], nextCursor=None, highWatermark=str(high + 1))
                validate("HistoryPage", smaller)
                if len(json.dumps(smaller, ensure_ascii=False).encode()) > byte_budget - 256:
                    raise ContractError("page_too_large")
            except ContractError:
                item.update(text=None, availability="unavailable", reason="content_exceeds_page_limit")
        items.append(item)
        consumed = row["seq"]
    last = consumed
    more = bool(rows) and last < high
    return validate("HistoryPage", dict(items=items,
        nextCursor=f"wh1.{binding}.{high}.{last}" if more else None, highWatermark=str(high + 1)))
