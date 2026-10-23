"""Bounded read-boundary fingerprints, not bodies or command check receipts.

Canonical JSON is UTF-8, sorted keys, no whitespace, no NaN. Observations mean
prepared replies, not transport acknowledgement, report handoff or acceptance.
"""
from __future__ import annotations

import base64
import hashlib
import json

from .contracts import StoreError
from .tool_read_api import BATCH_READS, encoded

OPERATIONS = BATCH_READS | {"tools_discover", "read_batch"}


def fingerprint(value):
    raw = encoded(value)
    return dict(sha256=hashlib.sha256(raw).hexdigest(), bytes=len(raw))


def request_hash(request_id):
    if type(request_id) not in (str, int):
        return None
    return fingerprint([type(request_id).__name__, request_id])["sha256"]


def summary(data):
    """Only hashes, lengths and native completeness flags; no arbitrary keys."""
    result = fingerprint(data)
    if not isinstance(data, dict):
        return result
    for key in ("complete", "partial", "truncated"):
        if type(data.get(key)) is bool:
            result[key] = data[key]
    result["paged"] = any(data.get(key) is not None for key in ("next_cursor", "next_offset"))
    body = None
    if isinstance(data.get("markdown"), str):
        body = data["markdown"].encode("utf-8")
    elif isinstance(data.get("chunk"), str):
        if data.get("encoding") == "utf-8":
            body = data["chunk"].encode("utf-8")
        elif data.get("encoding") == "base64":
            body = base64.b64decode(data["chunk"], validate=True)
    if body is not None:
        result.update(content_sha256=hashlib.sha256(body).hexdigest(), content_bytes=len(body))
    return result


def project(detail):
    """Inspector allowlist; malformed metadata is unavailable, never repaired."""
    def digest(value):
        return isinstance(value, str) and len(value) == 64 and all(c in "0123456789abcdef" for c in value)

    def shape(value):
        if not isinstance(value, dict) or not digest(value.get("sha256")):
            raise ValueError("digest")
        size = value.get("bytes")
        if type(size) is not int or not 0 <= size <= 1048576:
            raise ValueError("size")
        result = dict(sha256=value["sha256"], bytes=size)
        for key in ("complete", "partial", "truncated", "paged"):
            if key in value:
                if type(value[key]) is not bool:
                    raise ValueError("flag")
                result[key] = value[key]
        if "content_sha256" in value:
            if not digest(value["content_sha256"]) or type(value.get("content_bytes")) is not int or not 0 <= value["content_bytes"] <= 1048576:
                raise ValueError("content")
            result.update(content_sha256=value["content_sha256"], content_bytes=value["content_bytes"])
        return result

    if not isinstance(detail, dict):
        return dict(available=False)
    try:
        if (detail.get("version") != 1 or detail.get("available") is not True
                or detail.get("operation") not in OPERATIONS
                or type(detail.get("owner_epoch")) is not int
                or not digest(detail.get("request_hash"))
                or not isinstance(detail.get("channel_hash"), str)
                or len(detail["channel_hash"]) != 32
                or not digest(detail["channel_hash"] * 2)
                or type(detail.get("is_error")) is not bool):
            raise ValueError("identity")
        result = {key: detail[key] for key in (
            "version", "operation", "owner_epoch", "request_hash", "channel_hash", "is_error")}
        result.update(available=True, arguments=shape(detail["arguments"]), response=shape(detail["response"]))
        if "members" in detail:
            if not isinstance(detail["members"], list) or len(detail["members"]) > 8:
                raise ValueError("members")
            members = []
            for index, item in enumerate(detail["members"]):
                if (item.get("index") != index or not digest(item.get("item_hash"))
                        or item.get("operation") not in BATCH_READS
                        or item.get("status") not in ("ok", "error", "deferred", "not_run")):
                    raise ValueError("member")
                member = {key: item[key] for key in ("index", "item_hash", "operation", "status")}
                member["arguments"] = shape(item["arguments"])
                if "response" in item:
                    member["response"] = shape(item["response"])
                if "large_output_id" in item:
                    import re
                    if not isinstance(item["large_output_id"], str) or not re.fullmatch(r"lo_[0-9a-f]{12}", item["large_output_id"]):
                        raise ValueError("retained_identity")
                    member["large_output_id"] = item["large_output_id"]
                if "delivery_request" in item:
                    if not isinstance(item["delivery_request"], str) or not item["delivery_request"].startswith("batch:") or not digest(item["delivery_request"][6:]):
                        raise ValueError("delivery_identity")
                    member["delivery_request"] = item["delivery_request"]
                members.append(member)
            result["members"] = members
        return result
    except (ValueError, TypeError, KeyError):
        return dict(available=False)


def evaluate_manifest(rows, binding, manifest):
    """Compare independent expectations, never manufacture them from observations.

    This verifies fingerprints and identity, not the origin/truth of a supplied
    expected file. The public fixture runner establishes semantic expectations.
    """
    if (not isinstance(manifest, dict) or set(manifest) != {"binding", "calls"}
            or manifest["binding"] != binding or not isinstance(manifest["calls"], list)
            or not 1 <= len(manifest["calls"]) <= 64):
        return ["read_api_expectation_binding"]
    observed = [r for r in rows if r.get("section") == "read_api_calls"]
    failures, used = [], set()
    for index, expected in enumerate(manifest["calls"]):
        safe = project(expected) if isinstance(expected, dict) else {}
        identity = (safe.get("channel_hash"), safe.get("request_hash"))
        matches = [r for r in observed if (r.get("channel_hash"), r.get("request_hash")) == identity]
        valid = (safe.get("available") is True and safe == expected
                 and identity not in used and len(matches) == 1)
        used.add(identity)
        if valid:
            row = matches[0]
            valid = (row.get("bound") is True and row.get("owner_epoch") == binding["owner_epoch"]
                     and project(row) == expected
                     and sum(r.get("id") == row.get("id") for r in observed) == 1)
            attempt_id = row.get("attempt_id")
            if attempt_id is not None:
                attempts = [a for a in rows if a.get("section") == "attempts" and a.get("attempt_id") == attempt_id]
                valid = valid and len(attempts) == 1 and attempts[0].get("owner_epoch") == binding["owner_epoch"]
                if valid:
                    nodes = [n for n in rows if n.get("section") == "nodes"
                             and n.get("assignment_ref") == attempts[0].get("idempotency_key")]
                    valid = len(nodes) == 1 and bool(attempts[0].get("engine_session_ref"))
        if not valid:
            failures.append("read_api_call_" + str(index))
    return failures


def decode(reply):
    if "structuredContent" in reply:
        return reply["structuredContent"]
    content = reply.get("content", [])
    if len(content) != 1 or content[0].get("type") != "text":
        raise ValueError("unsupported_reply")
    return json.loads(content[0]["text"])


def record(store, channel, operation, arguments, reply, request_id):
    """One immutable runtime event per actual call; failures never become passes.

    No new selector, caller-written observation or exporter. Missing/oversized/
    unencodable evidence remains unavailable and cannot qualify a case.
    """
    from .observability.events import append_event
    from .tool_read_api import _binding

    if channel is None or operation not in OPERATIONS:
        return
    detail = dict(version=1, operation=operation, owner_epoch=channel.owner_epoch,
                  channel_hash=_binding(channel), request_hash=request_hash(request_id))
    try:
        data = decode(reply)
        detail.update(arguments=fingerprint(arguments), response=summary(data),
                      is_error=reply.get("isError") is True)
        if operation == "read_batch" and isinstance(data, dict):
            members = []
            calls = arguments.get("calls", [])
            for index, item in enumerate(data.get("results", [])):
                call = calls[index]
                # Do not attest a misidentified/reordered item.
                if item["id"] != call["id"] or item["tool"] != call["tool"]:
                    raise ValueError("member_identity")
                member = dict(index=index, item_hash=fingerprint(item["id"])["sha256"],
                              operation=call["tool"], arguments=fingerprint(call["arguments"]),
                              status=item["status"])
                from .tool_read_api import _member_request_id
                if _member_request_id(request_id, call["id"]) is not None:
                    member["delivery_request"] = _member_request_id(request_id, call["id"])
                if item["status"] == "ok":
                    member["response"] = summary(item["result"])
                elif item["status"] == "error":
                    member["response"] = fingerprint(item["error"])
                elif item["status"] == "deferred":
                    member["response"] = dict(sha256=item["result_sha256"], bytes=item["byte_count"])
                    member["large_output_id"] = item["large_output_id"]
                members.append(member)
            detail["members"] = members
        detail["available"] = True
    except (StoreError, ValueError, TypeError, KeyError, IndexError, UnicodeError, RecursionError):
        detail = dict(detail, available=False)
        # Do not leave partial fingerprints looking complete.
        for key in ("arguments", "response", "is_error", "members"):
            detail.pop(key, None)
    append_event(store, channel.task_id, "read_api_call", "prepared", "observed",
                 attempt_id=channel.attempt_id, detail=detail)
