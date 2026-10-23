"""Additive channel-scoped schema discovery and serial evidence reads.

No evaluator, external dispatch, snapshot or mutation batching. Each member
re-enters the existing trusted read boundary with fresh runtime authority.
"""
from __future__ import annotations

import base64
import hashlib
import json
import re
import time

from .contracts import StoreError

BATCH_READS = frozenset({
    "list_reports", "read_report", "read_draft",
    "list_sources", "read_source", "read_large_output",
})
REQUEST_BYTES = 32768
ITEM_BYTES = 6144
RESULT_BYTES = 65536
CAPTURE_BYTES = 1048576
DISCOVERY_BYTES = 16384
SCHEMA_BYTES = 32768
ADMISSION_SECONDS = 5.0


def encoded(value, maximum=CAPTURE_BYTES):
    """Bound compact UTF-8 serialization without constructing an unbounded blob."""
    chunks = []
    size = 0
    for chunk in json.JSONEncoder(ensure_ascii=False, sort_keys=True,
                                  separators=(",", ":"), allow_nan=False).iterencode(value):
        raw = chunk.encode("utf-8")
        size += len(raw)
        if size > maximum:
            raise StoreError("result_too_large")
        chunks.append(raw)
    return b"".join(chunks)


def _request_bytes(args):
    # Check containers iteratively before recursive schema walking/JSON encoding.
    stack = [(args, 0)]
    nodes = 0
    while stack:
        value, depth = stack.pop()
        nodes += 1
        if nodes > REQUEST_BYTES:
            raise StoreError("request_too_large")
        if isinstance(value, (dict, list)):
            if depth >= 16 or len(value) > REQUEST_BYTES:
                raise StoreError("invalid_arguments", "arguments",
                                 expected="at most 16 container levels")
            stack.extend((item, depth + 1) for item in
                         (value.values() if isinstance(value, dict) else value))
    try:
        return len(encoded(args, REQUEST_BYTES))
    except StoreError:
        raise StoreError("request_too_large") from None
    except (ValueError, TypeError, UnicodeError, RecursionError):
        raise StoreError("invalid_arguments", "arguments") from None


def effective_catalogue(store, channel, *, active=False):
    """The same effective records as tools/list, with optional fresh authority."""
    from .channeled_tools import channeled_base_tools
    from .delegation.mcp_tools import assignment_profile, tool_definitions
    from .engine.mcp_bridge import always_load
    from .orchestration.gateway import _authority

    if active:
        with store.connection() as db:
            _authority(db, channel)
            if channel.role == "worker":
                row = db.execute(
                    "SELECT n.role,n.stage FROM runtime_nodes n "
                    "JOIN runtime_tasks t ON n.task_id=t.task_id AND n.revision=t.revision "
                    "JOIN delegation_provisioning p ON n.task_id=p.task_id "
                    "AND n.assignment_ref=p.idempotency_key "
                    "WHERE n.task_id=? AND p.attempt_id=? AND n.status='running'",
                    (channel.task_id, channel.attempt_id),
                ).fetchone()
                if row is None or not row["role"] or not row["stage"]:
                    raise StoreError("tool_profile_unavailable")
                role_id, stage = row["role"], row["stage"]
            else:
                role_id, stage = None, None
    else:
        role_id, stage = assignment_profile(store, channel)
    return sorted(
        [always_load(tool) for tool in list(channeled_base_tools()) +
         tool_definitions(channel.role, role_id=role_id, stage=stage)],
        key=lambda tool: tool["name"],
    )


def _revision(catalogue):
    digest = hashlib.sha256()
    for chunk in json.JSONEncoder(ensure_ascii=False, sort_keys=True,
                                  separators=(",", ":"), allow_nan=False).iterencode(catalogue):
        digest.update(chunk.encode("utf-8"))
    return digest.hexdigest()


def _binding(channel):
    return hashlib.sha256(encoded([
        channel.channel_id, channel.owner_epoch, channel.attempt_id,
    ])).hexdigest()[:32]


def _cursor(channel, revision, index):
    return "td1." + base64.urlsafe_b64encode(
        encoded([_binding(channel), revision, index])
    ).decode().rstrip("=")


def _index(cursor, channel, revision, count):
    try:
        if not cursor.startswith("td1."):
            raise ValueError
        payload = cursor[4:]
        binding, digest, index = json.loads(base64.b64decode(
            payload + "=" * (-len(payload) % 4), altchars=b"-_", validate=True))
        if binding != _binding(channel) or type(index) is not int or not 0 <= index < count:
            raise ValueError
        if digest != revision:
            raise StoreError("discovery_cursor_stale")
        return index
    except (ValueError, TypeError, UnicodeError):
        raise StoreError("invalid_cursor") from None


def _fresh(store, channel, revision):
    if _revision(effective_catalogue(store, channel, active=True)) != revision:
        raise StoreError("tool_catalogue_changed")


def _retain(store, channel, raw):
    from .orchestration.gateway import _authority
    from .recovery.large_output_store import persist_output

    with store.connection() as db:
        _, owner_scope = _authority(db, channel)
    record = persist_output(store, task_id=channel.task_id, owner_scope=owner_scope,
                            source_kind="tool_result", data=raw)
    return dict(status="deferred", large_output_id=record["id"],
                byte_count=len(raw), result_sha256=hashlib.sha256(raw).hexdigest())


def _discover(store, channel, args, catalogue, revision):
    if "name" in args:
        if "cursor" in args or "limit" in args:
            raise StoreError("invalid_arguments", "arguments")
        tool = next((tool for tool in catalogue if tool["name"] == args["name"]), None)
        if tool is None:
            raise StoreError("tool_not_available")
        data = dict(catalog_revision=revision, tool=tool)
        try:
            raw = encoded(data)
        except StoreError:
            raise StoreError("schema_too_large") from None
        if len(raw) > SCHEMA_BYTES:
            data = dict(catalog_revision=revision, **_retain(store, channel, raw))
        return data
    index = _index(args["cursor"], channel, revision, len(catalogue)) if "cursor" in args else 0
    result = dict(catalog_revision=revision, tools=[], next_cursor=None)
    while index < len(catalogue) and len(result["tools"]) < args.get("limit", 20):
        tool = catalogue[index]
        entry = dict(name=tool["name"], title=tool["title"],
                     description=tool["description"][:160],
                     description_truncated=len(tool["description"]) > 160,
                     read_batch_supported=tool["name"] in BATCH_READS)
        next_index = index + 1
        candidate = dict(result, tools=result["tools"] + [entry],
                         next_cursor=_cursor(channel, revision, next_index)
                         if next_index < len(catalogue) else None)
        if len(encoded(candidate)) > DISCOVERY_BYTES:
            break
        result = candidate
        index = next_index
    if not result["tools"] and index < len(catalogue):
        raise StoreError("schema_too_large")
    return result


def record_report_delivery(store, channel, data, request_id):
    """Shared by single reads and inline batch delivery, never deferred bodies."""
    from .delegation.report_reads import record_report_read
    record_report_read(store, task_id=channel.task_id, attempt_id=channel.attempt_id,
                       report_id=data.get("report_id"), request_id=request_id)


def _member_request_id(outer_id, item_id):
    if outer_id is None:
        return None
    return "batch:" + hashlib.sha256(encoded(
        [type(outer_id).__name__, outer_id, item_id])).hexdigest()


def _execute(store, channel, tool, arguments):
    from .delegation.mcp_tools import dispatch
    from .orchestration.gateway import _authority
    if tool in {"list_reports", "read_report", "read_draft"}:
        return store.call(
            tool, dict(arguments), channel.principal_thread_id,
            channel.parent_principal_thread_id, original_request=None,
            steering_source=None, trusted=True, authorization=lambda db: _authority(db, channel))
    return dispatch(store, channel, tool, arguments)


def _safe_error(error, field=None):
    # Do not forward received values, raw errors or corrections listing private refs.
    code = str(error)
    if not re.fullmatch(r"[a-z][a-z0-9_]{0,63}", code):
        code = "storage_error"
    result = dict(code=code, correction="Use the current tools_discover schema and an individually smaller page if needed.")
    if field:
        result["field"] = field
    return result


def _audit(store, channel, outcome, **detail):
    from .observability.events import append_event
    append_event(store, channel.task_id, "tool_read_api", outcome, "observed",
                 attempt_id=channel.attempt_id, detail=dict(
                     channel_hash=_binding(channel), **detail))


def _batch(store, channel, args, catalogue, revision, outer_id):
    from .orchestration.contracts import validate
    from .read_api_evidence import request_hash
    offered = {tool["name"]: tool for tool in catalogue}
    seen = set()
    for index, call in enumerate(args["calls"]):
        field = f"arguments.calls[{index}]"
        if call["id"] in seen:
            raise StoreError("invalid_arguments", field + ".id")
        seen.add(call["id"])
        if call["tool"] not in BATCH_READS or call["tool"] not in offered:
            raise StoreError("tool_not_available", field + ".tool")
        try:
            validate(offered[call["tool"]]["inputSchema"], call["arguments"])
        except StoreError:
            raise StoreError("invalid_arguments", field + ".arguments") from None

    started = time.monotonic()
    results = []
    delivered = []
    executed = 0
    for index, call in enumerate(args["calls"]):
        _fresh(store, channel, revision)
        if time.monotonic() - started >= ADMISSION_SECONDS:
            results.append(dict(id=call["id"], tool=call["tool"], status="not_run",
                                reason="batch_budget_exhausted"))
            continue
        item_started = time.monotonic()
        item = dict(id=call["id"], tool=call["tool"])
        executed += 1
        try:
            try:
                data = _execute(store, channel, call["tool"], call["arguments"])
            finally:
                _audit(store, channel, "executed", operation=call["tool"],
                       outer_hash=_member_request_id(outer_id, ""), request_hash=request_hash(outer_id), item_index=index,
                       item_hash=hashlib.sha256(call["id"].encode()).hexdigest())
            _fresh(store, channel, revision)
            if "image_base64" in data:
                raise StoreError("batch_multimodal_unsupported")
            raw = encoded(data)
            if len(raw) > ITEM_BYTES:
                if call["tool"] == "read_large_output":
                    raise StoreError("batch_result_limit")
                item.update(_retain(store, channel, raw))
            else:
                item.update(status="ok", result=data)
                if call["tool"] == "read_report":
                    delivered.append((data, _member_request_id(outer_id, call["id"])))
        except StoreError as error:
            # Authority loss is not a local item failure: withhold every body.
            _fresh(store, channel, revision)
            item.update(status="error", error=_safe_error(error))
        except Exception:
            _fresh(store, channel, revision)
            item.update(status="error", error=_safe_error(StoreError("storage_error")))
        results.append(item)
        _audit(store, channel, item["status"], catalog_revision=revision,
               outer_hash=_member_request_id(outer_id, ""), request_hash=request_hash(outer_id), item_index=index,
               item_hash=hashlib.sha256(call["id"].encode()).hexdigest(),
               operation=call["tool"], elapsed_ms=int((time.monotonic()-item_started)*1000),
               input_bytes=len(encoded(call["arguments"])), output_bytes=len(encoded(item)),
               error_code=item.get("error", {}).get("code"),
               large_output_id=item.get("large_output_id"),
               result_sha256=item.get("result_sha256"), byte_count=item.get("byte_count"))
    _fresh(store, channel, revision)
    result = dict(outcome="complete" if all(item["status"] == "ok" for item in results) else "incomplete",
                  results=results, executed_count=executed,
                  returned_count=sum(item["status"] == "ok" for item in results))
    encoded(result, RESULT_BYTES)
    return result, delivered


def dispatch(store, channel, operation, args, *, request_id=None):
    from .delegation.contracts import SHARED
    from .orchestration.contracts import validate
    from .read_api_evidence import request_hash

    started = time.monotonic()
    try:
        input_bytes = _request_bytes(args)
        catalogue = effective_catalogue(store, channel, active=True)
        revision = _revision(catalogue)
        try:
            validate(SHARED[operation], args)
        except StoreError as error:
            if operation == "tools_discover" and error.field in {
                    "arguments.cursor", "arguments.limit"}:
                field = error.field.rsplit(".", 1)[-1]
                raise StoreError(
                    "invalid_arguments", error.field,
                    received=f"{field} is present while name is supplied",
                    expected=f"{field} omitted when name is supplied",
                    correction=f"Omit {field} when name is supplied; use the advertised schema form.",
                ) from None
            raise StoreError("invalid_arguments", "arguments",
                             correction="Match the current advertised inputSchema.") from None
        delivered = []
        if operation == "tools_discover":
            data = _discover(store, channel, args, catalogue, revision)
        else:
            data, delivered = _batch(store, channel, args, catalogue, revision, request_id)
        _fresh(store, channel, revision)
        _audit(store, channel, data.get("outcome", data.get("status", "complete")),
               operation=operation, catalog_revision=revision,
               outer_hash=_member_request_id(request_id, ""), request_hash=request_hash(request_id),
               elapsed_ms=int((time.monotonic()-started)*1000),
               input_bytes=input_bytes, output_bytes=len(encoded(data)),
               executed_count=data.get("executed_count", 0),
               deferred_count=sum(item["status"] == "deferred" for item in data.get("results", [])),
               not_run_count=sum(item["status"] == "not_run" for item in data.get("results", [])))
        _fresh(store, channel, revision)
        # Delivery observations follow the final gates, never just an executed read.
        if delivered:
            from .delegation.report_reads import record_report_reads
            from .orchestration.gateway import _authority
            record_report_reads(
                store, task_id=channel.task_id, attempt_id=channel.attempt_id,
                reads=[(body["report_id"], inner_id) for body, inner_id in delivered],
                authorization=lambda db: _authority(db, channel))
        return data
    except StoreError as error:
        _audit(store, channel, "rejected", operation=operation, error_code=_safe_error(error)["code"],
               outer_hash=_member_request_id(request_id, ""), request_hash=request_hash(request_id))
        # No accumulated bodies, references, rejected values or arbitrary field names.
        safe = _safe_error(error)
        field = error.field if isinstance(error.field, str) and re.fullmatch(
            r"arguments(?:\.(?:cursor|limit)|\.calls\[[0-7]\](?:\.(?:id|tool|arguments))?)?", error.field) else None
        if field in {"arguments.cursor", "arguments.limit"} and error.expected:
            raise StoreError(safe["code"], field=field, received=error.received,
                             expected=error.expected, correction=error.correction) from None
        raise StoreError(safe["code"], field=field, correction=safe["correction"]) from None
