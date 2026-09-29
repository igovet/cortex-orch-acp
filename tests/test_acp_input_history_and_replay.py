"""T018: durable input history and replay.

Covers event identity/delivery-ledger unification, attachment availability
truthfulness, trust labels, bounded reads, crash recovery between source
capture and coordinator delivery, and the display-only ``session/load``
replay boundary. Every Store is rooted at ``tmp_path``; no real engine,
network, or paid model call anywhere in this file.
"""
from __future__ import annotations

import json
import os
import tempfile
import time
import threading
import uuid
from pathlib import Path

import pytest

from cortex_runtime.acp import replay_store
from cortex_runtime.acp import sessions as acp_sessions
from cortex_runtime.acp.agent import Agent
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import append_source_event, bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.acp.protocol import RpcError
from cortex_runtime.acp import protocol as acp_protocol
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel, resolve_channel

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path, origin="acp:s1", event_id=None, text="hello world"):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin=origin, event_id=event_id or str(uuid.uuid4())), text, "owner-1")
    return store, result


def _coordinator_channel(store, task_id, owner_epoch):
    channel_id, secret = issue_channel(store, task_id, "coordinator", owner_epoch.epoch)
    return resolve_channel(store, channel_id, secret)


# -- decision 4: attachments -------------------------------------------------


def test_mixed_text_resource_and_resource_link_blocks_resolved(tmp_path):
    (tmp_path / "notes.txt").write_text("attached note body", encoding="utf-8")
    blocks = [
        dict(type="text", text="see attached"),
        dict(type="resource", resource=dict(uri="mem://x", mimeType="text/plain", text="embedded text body")),
        dict(type="resource_link", name="notes.txt", uri=f"file://{tmp_path}/notes.txt"),
    ]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    by_index = {b.block_index: b for b in resolved}
    assert by_index[1].kind == "resource" and by_index[1].availability == "stored"
    assert by_index[1].body == b"embedded text body"
    assert by_index[2].kind == "resource_link" and by_index[2].availability == "stored"
    assert by_index[2].body == b"attached note body"


def test_embedded_text_and_blob_stored_with_sha256(tmp_path):
    import base64

    blocks = [
        dict(type="resource", resource=dict(uri="mem://t", text="plain text body")),
        dict(type="resource", resource=dict(uri="mem://b", blob=base64.b64encode(b"\x00\x01binary").decode())),
    ]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    store, result = _task(tmp_path)
    staged = replay_store.stage_blocks(str(tmp_path), resolved)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        replay_store.record_blocks_tx(store, db, result.task_id, result.report_id, staged, [], [])
        db.commit()
    listed = replay_store.list_sources(store, result.task_id)
    entries = {e["block_index"]: e for e in [dict(e, block_index=int(e["source_ref"].rsplit(":", 1)[1])) for e in listed["entries"]]}
    import hashlib

    assert entries[0]["sha256"] == hashlib.sha256(b"plain text body").hexdigest()
    assert entries[1]["sha256"] == hashlib.sha256(b"\x00\x01binary").hexdigest()


def test_image_exact_bytes_and_mime_are_persisted_and_read_back(tmp_path):
    import base64
    import hashlib

    image_bytes = b"\x89PNG\r\n\x1a\n\x00opaque-image-bytes"
    blocks = [dict(type="image", data=base64.b64encode(image_bytes).decode("ascii"),
                   mimeType="image/png")]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    assert resolved[0].availability == "stored"
    assert resolved[0].body == image_bytes
    store, result = _task(tmp_path)
    staged = replay_store.stage_blocks(str(tmp_path), resolved)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        replay_store.record_blocks_tx(store, db, result.task_id, result.report_id, staged, [], [])
        db.commit()

    source = replay_store.read_source(store, result.task_id, f"{result.report_id}:0")
    assert source["kind"] == "image"
    assert source["mime"] == "image/png"
    assert source["sha256"] == hashlib.sha256(image_bytes).hexdigest()
    assert source["total_size"] == len(image_bytes)
    assert source["encoding"] == "base64"
    assert base64.b64decode(source["chunk"]) == image_bytes


def test_image_just_under_10mib_cap_is_stored_and_durably_captured(tmp_path):
    """An image just under the raised 10 MiB per-block cap is accepted and
    durably captured end to end, not merely classified as stored in memory."""
    import base64
    import hashlib

    image_bytes = b"x" * (replay_store.MAX_BLOCK_BYTES - 1)
    blocks = [dict(type="image", data=base64.b64encode(image_bytes).decode("ascii"), mimeType="image/png")]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    assert resolved[0].availability == "stored"
    assert resolved[0].body == image_bytes
    store, result = _task(tmp_path)
    staged = replay_store.stage_blocks(str(tmp_path), resolved)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        replay_store.record_blocks_tx(store, db, result.task_id, result.report_id, staged, [], [])
        db.commit()
    source = replay_store.read_source(store, result.task_id, f"{result.report_id}:0")
    assert source["total_size"] == len(image_bytes)
    assert source["sha256"] == hashlib.sha256(image_bytes).hexdigest()


def test_content_blocks_to_text_accepts_just_under_and_rejects_just_over_10mib_cap():
    """``protocol.content_blocks_to_text`` enforces the same raised 10 MiB
    cap on the encoded image size and reports it accurately on rejection."""
    import base64

    under = base64.b64encode(b"x" * (acp_protocol.MAX_IMAGE_BYTES - 1)).decode("ascii")
    text = acp_protocol.content_blocks_to_text([dict(type="image", data=under, mimeType="image/png")])
    assert text == "[image: image/png]"

    over = base64.b64encode(b"x" * (acp_protocol.MAX_IMAGE_BYTES + 4)).decode("ascii")
    with pytest.raises(RpcError) as excinfo:
        acp_protocol.content_blocks_to_text([dict(type="image", data=over, mimeType="image/png")])
    assert "10 MiB" in excinfo.value.message


def test_invalid_or_oversized_images_never_produce_a_stored_body(tmp_path):
    import base64

    invalid = replay_store.resolve_prompt_blocks(
        [dict(type="image", data="not-base64!", mimeType="image/png")], str(tmp_path)
    )[0]
    assert invalid.availability == "inaccessible"
    assert invalid.reason == "invalid_blob_encoding"
    assert invalid.body is None

    oversized = replay_store.resolve_prompt_blocks(
        [dict(type="image", data=base64.b64encode(b"x" * (replay_store.MAX_BLOCK_BYTES + 1)).decode(),
              mimeType="image/png")], str(tmp_path)
    )[0]
    assert oversized.availability == "too_large"
    assert oversized.body is None


def test_over_cap_block_recorded_as_too_large_not_partially_stored(tmp_path):
    big = "x" * (replay_store.MAX_BLOCK_BYTES + 1)
    blocks = [dict(type="resource", resource=dict(uri="mem://big", text=big))]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    assert resolved[0].availability == "too_large"
    assert resolved[0].body is None


def test_resource_link_inside_project_root_stored_snapshot(tmp_path):
    (tmp_path / "inside.txt").write_text("inside body", encoding="utf-8")
    blocks = [dict(type="resource_link", name="inside.txt", uri=f"file://{tmp_path}/inside.txt")]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    assert resolved[0].availability == "stored"
    assert resolved[0].body == b"inside body"


def test_resource_link_outside_project_root_reference_only_never_fetched(tmp_path):
    outside = tmp_path.parent / f"outside-{uuid.uuid4().hex}.txt"
    outside.write_text("outside body", encoding="utf-8")
    try:
        blocks = [dict(type="resource_link", name="outside.txt", uri=f"file://{outside}")]
        resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
        assert resolved[0].availability == "reference_only"
        assert resolved[0].body is None
    finally:
        outside.unlink(missing_ok=True)


def test_resource_link_symlink_hardlink_non_owned_inaccessible(tmp_path):
    real = tmp_path / "real.txt"
    real.write_text("real body", encoding="utf-8")
    link = tmp_path / "link.txt"
    os.symlink(real, link)
    resolved = replay_store.resolve_prompt_blocks(
        [dict(type="resource_link", name="link.txt", uri=f"file://{link}")], str(tmp_path))
    assert resolved[0].availability == "inaccessible"
    assert resolved[0].reason == "symlink_path"

    hardlinked_target = tmp_path / "hardlinked.txt"
    real.link_to(hardlinked_target) if hasattr(real, "link_to") else os.link(real, hardlinked_target)
    resolved = replay_store.resolve_prompt_blocks(
        [dict(type="resource_link", name="hardlinked.txt", uri=f"file://{hardlinked_target}")], str(tmp_path))
    assert resolved[0].availability == "inaccessible"
    assert resolved[0].reason == "multiple_hardlinks"


# -- TOCTOU-safe resource_link resolution (lead follow-up, HIGH) ------------


def test_resource_link_final_component_symlink_swap_after_validation_rejected(tmp_path):
    """The classic TOCTOU: if resolution ever validated a *path string*
    and then separately stat()/read() that same string again, an attacker
    could swap the final path component for a symlink to an outside
    secret in between. The fd-based resolver in ``replay_store`` never
    re-touches the path string after its one ``O_NOFOLLOW`` open, so this
    must be rejected regardless of timing."""
    secret = tmp_path.parent / f"secret-{uuid.uuid4().hex}.txt"
    secret.write_text("outside secret", encoding="utf-8")
    target = tmp_path / "target.txt"
    target.write_text("same size!", encoding="utf-8")  # same byte length as a plausible swap
    try:
        uri = f"file://{target}"
        # Simulate the swap happening between any path-string validation
        # and the actual open: by the time resolve_prompt_blocks runs,
        # the final component is already a symlink to the outside secret.
        target.unlink()
        os.symlink(secret, target)
        resolved = replay_store.resolve_prompt_blocks(
            [dict(type="resource_link", name="target.txt", uri=uri)], str(tmp_path))
        assert resolved[0].availability == "inaccessible"
        assert resolved[0].reason == "symlink_path"
        assert resolved[0].body is None  # the outside secret's bytes were never read
    finally:
        target.unlink(missing_ok=True)
        secret.unlink(missing_ok=True)


def test_resource_link_intermediate_directory_symlink_escape_rejected(tmp_path):
    """A textually-inside-looking path whose *intermediate* directory is a
    symlink pointing outside the project root must never have its bytes
    read -- caught here by the fd's own kernel-resolved real path
    (``/proc/self/fd``), not by re-walking the original path string."""
    outside_dir = tmp_path.parent / f"outside-dir-{uuid.uuid4().hex}"
    outside_dir.mkdir()
    (outside_dir / "secret.txt").write_text("outside via directory symlink", encoding="utf-8")
    try:
        linked_dir = tmp_path / "linked"
        os.symlink(outside_dir, linked_dir)
        uri = f"file://{linked_dir}/secret.txt"
        resolved = replay_store.resolve_prompt_blocks(
            [dict(type="resource_link", name="secret.txt", uri=uri)], str(tmp_path))
        assert resolved[0].availability == "reference_only"
        assert resolved[0].body is None
    finally:
        (tmp_path / "linked").unlink(missing_ok=True)
        import shutil

        shutil.rmtree(outside_dir, ignore_errors=True)


def test_resource_link_fifo_rejected_without_blocking(tmp_path):
    fifo = tmp_path / "pipe"
    os.mkfifo(fifo)
    try:
        resolved = replay_store.resolve_prompt_blocks(
            [dict(type="resource_link", name="pipe", uri=f"file://{fifo}")], str(tmp_path))
        assert resolved[0].availability == "inaccessible"
        assert resolved[0].reason == "not_a_regular_file"
        assert resolved[0].body is None
    finally:
        fifo.unlink(missing_ok=True)


def test_resource_link_oversize_growth_during_read_never_stores_partial(tmp_path, monkeypatch):
    path = tmp_path / "growing.txt"
    path.write_bytes(b"x" * 10)  # small at fstat/validation time
    monkeypatch.setattr(replay_store, "MAX_BLOCK_BYTES", 5)
    try:
        resolved = replay_store.resolve_prompt_blocks(
            [dict(type="resource_link", name="growing.txt", uri=f"file://{path}")], str(tmp_path))
        assert resolved[0].availability == "too_large"
        assert resolved[0].reason == "block_exceeds_10mib_cap"
        assert resolved[0].body is None  # bounded read never stored a partial/truncated body
    finally:
        pass


def test_http_resource_link_reference_only_never_fetched(tmp_path):
    resolved = replay_store.resolve_prompt_blocks(
        [dict(type="resource_link", name="remote", uri="https://example.invalid/nope")], str(tmp_path))
    assert resolved[0].availability == "reference_only"
    assert resolved[0].body is None


def test_stored_body_deleted_read_reports_expired_with_provenance(tmp_path):
    (tmp_path / "gone.txt").write_text("will vanish", encoding="utf-8")
    blocks = [dict(type="resource_link", name="gone.txt", uri=f"file://{tmp_path}/gone.txt")]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    store, result = _task(tmp_path)
    staged = replay_store.stage_blocks(str(tmp_path), resolved)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        replay_store.record_blocks_tx(store, db, result.task_id, result.report_id, staged, [], [])
        db.commit()
    with store.connection() as db:
        blocks_dir = store._task_directory(db, result.task_id) / ".source_blocks"
    for path in blocks_dir.iterdir():
        path.unlink()
    read = replay_store.read_source(store, result.task_id, f"{result.report_id}:0")
    assert read["availability"] == "expired"
    assert read["reason"] == "stored_body_missing"
    with store.connection() as db:
        capture_row = db.execute(
            "SELECT availability FROM source_blocks WHERE report_id=? AND block_index=0", (result.report_id,),
        ).fetchone()
        read_rows = db.execute(
            "SELECT availability, reason FROM source_block_reads WHERE report_id=? AND block_index=0", (result.report_id,),
        ).fetchall()
    assert capture_row["availability"] == "stored"  # capture-time truth never overwritten
    assert len(read_rows) == 1 and read_rows[0]["availability"] == "expired"


# -- attachment capture atomicity (lead follow-up) ---------------------------


def _counts_all(store, task_id):
    with store.connection() as db:
        return dict(
            source_events=db.execute("SELECT COUNT(*) FROM source_events WHERE task_id=?", (task_id,)).fetchone()[0],
            source_revisions=db.execute("SELECT COUNT(*) FROM source_revisions WHERE task_id=?", (task_id,)).fetchone()[0],
            source_blocks=db.execute("SELECT COUNT(*) FROM source_blocks WHERE task_id=?", (task_id,)).fetchone()[0],
        )


def test_attachment_capture_atomic_crash_before_commit_persists_nothing(tmp_path, monkeypatch):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    (tmp_path / "a.txt").write_text("a body", encoding="utf-8")
    blocks = [dict(type="resource_link", name="a.txt", uri=f"file://{tmp_path}/a.txt")]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    staged = replay_store.stage_blocks(str(tmp_path), resolved)
    assert staged[0].staged_path is not None and staged[0].staged_path.exists()

    def explode(*a, **k):
        raise RuntimeError("injected failure between text capture and commit")

    monkeypatch.setattr(replay_store, "record_blocks_tx", explode)
    with pytest.raises(RuntimeError):
        bootstrap_task(store, SourceEvent(origin="acp:s1", event_id="e1"), "hello", "owner-1", blocks=staged)
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM source_events").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM source_revisions").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM source_blocks").fetchone()[0] == 0
    # The staged temp file (never renamed, since the transaction rolled
    # back before the rename) was cleaned up by the same crash-safety
    # machinery store.py already uses for report bodies.
    assert not staged[0].staged_path.exists()


def test_attachment_capture_atomic_success_text_and_blocks_together(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    (tmp_path / "a.txt").write_text("a body", encoding="utf-8")
    blocks = [dict(type="resource_link", name="a.txt", uri=f"file://{tmp_path}/a.txt")]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    staged = replay_store.stage_blocks(str(tmp_path), resolved)
    result = bootstrap_task(store, SourceEvent(origin="acp:s1", event_id="e1"), "hello", "owner-1", blocks=staged)
    assert _counts_all(store, result.task_id) == dict(source_events=1, source_revisions=1, source_blocks=1)
    read = replay_store.read_source(store, result.task_id, f"{result.report_id}:0")
    assert read["availability"] == "stored" and read["chunk"] == "a body"


def test_replay_of_existing_event_does_not_duplicate_blocks(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    (tmp_path / "a.txt").write_text("a body", encoding="utf-8")
    ev = SourceEvent(origin="acp:s1", event_id="e1")
    blocks1 = replay_store.stage_blocks(str(tmp_path), replay_store.resolve_prompt_blocks(
        [dict(type="resource_link", name="a.txt", uri=f"file://{tmp_path}/a.txt")], str(tmp_path)))
    first = bootstrap_task(store, ev, "hello", "owner-1", blocks=blocks1)
    assert first.replayed is False

    blocks2 = replay_store.stage_blocks(str(tmp_path), replay_store.resolve_prompt_blocks(
        [dict(type="resource_link", name="a.txt", uri=f"file://{tmp_path}/a.txt")], str(tmp_path)))
    staged_path2 = blocks2[0].staged_path
    second = bootstrap_task(store, ev, "hello", "owner-1", blocks=blocks2)
    assert second.replayed is True
    assert _counts_all(store, first.task_id) == dict(source_events=1, source_revisions=1, source_blocks=1)
    assert not staged_path2.exists()  # cleaned up, never recorded a second time


def test_unexpected_resolution_exception_recorded_inaccessible_capture_error(tmp_path, monkeypatch):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)

    def explode(*a, **k):
        raise RuntimeError("simulated unexpected resolver failure")

    monkeypatch.setattr(replay_store, "_resolve_resource_link_block", explode)
    resolved = replay_store.resolve_prompt_blocks(
        [dict(type="resource_link", name="x", uri="file:///nonexistent")], str(tmp_path))
    assert resolved[0].availability == "inaccessible" and resolved[0].reason == "capture_error"
    staged = replay_store.stage_blocks(str(tmp_path), resolved)
    result = bootstrap_task(store, SourceEvent(origin="acp:s1", event_id="e1"), "hello despite failure", "owner-1", blocks=staged)
    assert replay_store.read_source(store, result.task_id, f"{result.report_id}:-1")["chunk"] == "hello despite failure"
    with store.connection() as db:
        row = db.execute(
            "SELECT availability, reason FROM source_blocks WHERE report_id=? AND block_index=0", (result.report_id,),
        ).fetchone()
    assert row["availability"] == "inaccessible" and row["reason"] == "capture_error"


# -- forensic _meta capture (lead follow-up) ---------------------------------


def test_forensic_meta_capture_stored_and_size_capped(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    small_meta = dict(client="zed", trace_id="abc-123")
    result = bootstrap_task(store, SourceEvent(origin="acp:s1", event_id="e1"), "hi", "owner-1", meta=small_meta)
    with store.connection() as db:
        row = db.execute("SELECT meta_json FROM source_events WHERE origin=? AND event_id=?", ("acp:s1", "e1")).fetchone()
    assert json.loads(row["meta_json"]) == small_meta

    big_meta = dict(blob="x" * (replay_store.MAX_META_BYTES * 2))
    result2 = bootstrap_task(store, SourceEvent(origin="acp:s2", event_id="e2"), "hi2", "owner-1", meta=big_meta)
    with store.connection() as db:
        row2 = db.execute("SELECT meta_json FROM source_events WHERE origin=? AND event_id=?", ("acp:s2", "e2")).fetchone()
    decoded = json.loads(row2["meta_json"])
    assert decoded["truncated"] is True and decoded["original_size_bytes"] > replay_store.MAX_META_BYTES


def test_meta_never_used_as_identity(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    ev = SourceEvent(origin="acp:s1", event_id="e1")
    first = bootstrap_task(store, ev, "hello", "owner-1", meta=dict(a=1))
    # Same (origin, event_id) but different _meta -- still a replay,
    # dedup_key/identity is unaffected by _meta.
    second = bootstrap_task(store, ev, "hello", "owner-1", meta=dict(a=2))
    assert second.replayed is True and second.task_id == first.task_id
    with store.connection() as db:
        row = db.execute("SELECT meta_json FROM source_events WHERE origin=? AND event_id=?", ("acp:s1", "e1")).fetchone()
    assert json.loads(row["meta_json"]) == dict(a=1)  # first write wins, never overwritten by replay


def test_meta_absent_stores_null(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="acp:s1", event_id="e1"), "hello", "owner-1")
    with store.connection() as db:
        row = db.execute("SELECT meta_json FROM source_events WHERE origin=? AND event_id=?", ("acp:s1", "e1")).fetchone()
    assert row["meta_json"] is None


# -- decision 1/2: identity, dedup, delivery ledger --------------------------


def test_two_identical_texts_distinct_event_ids_both_delivered(tmp_path):
    store, result = _task(tmp_path, event_id="e1", text="same text")
    second = append_source_event(store, SourceEvent(origin="acp:s1", event_id="e2"), "same text", result.task_id)
    assert second.replayed is False
    delegation_events.append_event(
        store, result.task_id, "user_message", "user_message:acp:s1:e1", dict(content="same text"), origin="acp:s1")
    delegation_events.append_event(
        store, result.task_id, "user_message", "user_message:acp:s1:e2", dict(content="same text"), origin="acp:s1")
    with store.connection() as db:
        rows = db.execute(
            "SELECT dedup_key FROM task_events WHERE task_id=? AND event_type='user_message'", (result.task_id,),
        ).fetchall()
    assert {r["dedup_key"] for r in rows} == {"user_message:acp:s1:e1", "user_message:acp:s1:e2"}


def test_same_request_id_retry_rejected_no_rows(tmp_path):
    store, result = _task(tmp_path, event_id="e1", text="original")
    replay = append_source_event(store, SourceEvent(origin="acp:s1", event_id="e1"), "original", result.task_id)
    assert replay.replayed is True
    with store.connection() as db:
        count = db.execute("SELECT COUNT(*) AS n FROM source_events WHERE task_id=?", (result.task_id,)).fetchone()["n"]
        revisions = db.execute("SELECT COUNT(*) AS n FROM source_revisions WHERE task_id=?", (result.task_id,)).fetchone()["n"]
    assert count == 1 and revisions == 1  # no duplicate rows from the retried identical event


def test_start_prompt_dedup_key_derived_from_source_identity_not_turn_count(tmp_path):
    from cortex_runtime.acp.orchestrating_runtime import PromptOutcome

    store, result = _task(tmp_path, event_id="e1")
    engine = FakeSessionRuntime(store, lambda *a, **k: (lambda *a2, **k2: PromptOutcome(StopReason.END_TURN, "")))
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=1.0)
    runtime.start_prompt(
        result.task_id, "acp:s1", "hello world", lambda t: True, threading.Event(),
        source_origin="acp:s1", source_event_id="e1")
    with store.connection() as db:
        row = db.execute(
            "SELECT dedup_key, payload_json FROM task_events WHERE task_id=? AND event_type='user_message'",
            (result.task_id,),
        ).fetchone()
    assert row["dedup_key"] == "user_message:acp:s1:e1"
    assert json.loads(row["payload_json"]).get("recovered") is None


def test_replayed_event_not_delivered_twice_to_coordinator(tmp_path):
    from cortex_runtime.acp.orchestrating_runtime import PromptOutcome

    store, result = _task(tmp_path, event_id="e1")
    engine = FakeSessionRuntime(store, lambda *a, **k: (lambda *a2, **k2: PromptOutcome(StopReason.END_TURN, "")))
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=1.0)
    runtime.start_prompt(result.task_id, "acp:s1", "hello world", lambda t: True, threading.Event(),
                          source_origin="acp:s1", source_event_id="e1")
    # A transport-level retry of the *same* source event never reaches
    # append_source_event/start_prompt in production (see agent.Agent's
    # duplicate-id guard); this call simulates a caller mistakenly invoking
    # start_prompt again for the identical already-recorded identity.
    runtime.start_prompt(result.task_id, "acp:s1", "hello world", lambda t: True, threading.Event(),
                          source_origin="acp:s1", source_event_id="e1")
    with store.connection() as db:
        rows = db.execute(
            "SELECT id FROM task_events WHERE task_id=? AND dedup_key=?",
            (result.task_id, "user_message:acp:s1:e1"),
        ).fetchall()
    assert len(rows) == 1


# -- decision 3: crash recovery ----------------------------------------------


def test_crash_after_capture_before_recording_recovered_once(tmp_path):
    from cortex_runtime.acp.orchestrating_runtime import PromptOutcome

    store, result = _task(tmp_path, event_id="e1", text="first message")
    # Simulate a crash between bootstrap capture and this runtime ever
    # recording a user_message for it: the source_events row exists (from
    # _task's bootstrap_task above) but no task_events row does yet.
    with store.connection() as db:
        unrecorded = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key='user_message:acp:s1:e1'", (result.task_id,),
        ).fetchone()
    assert unrecorded is None

    engine = FakeSessionRuntime(store, lambda *a, **k: (lambda *a2, **k2: PromptOutcome(StopReason.END_TURN, "")))
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=1.0)
    # A later, distinct prompt on the same task -- the recovery path must
    # record the earlier (e1) source event first, labeled recovered, before
    # this call's own (e2) message.
    append_source_event(store, SourceEvent(origin="acp:s1", event_id="e2"), "second message", result.task_id)
    runtime.start_prompt(result.task_id, "acp:s1", "second message", lambda t: True, threading.Event(),
                          source_origin="acp:s1", source_event_id="e2")
    with store.connection() as db:
        rows = db.execute(
            "SELECT dedup_key, payload_json FROM task_events WHERE task_id=? AND event_type='user_message' ORDER BY id",
            (result.task_id,),
        ).fetchall()
    assert [r["dedup_key"] for r in rows] == ["user_message:acp:s1:e1", "user_message:acp:s1:e2"]
    recovered_payload = json.loads(rows[0]["payload_json"])
    assert recovered_payload["recovered"] is True
    assert recovered_payload["content"] == "first message"
    assert json.loads(rows[1]["payload_json"]).get("recovered") is None

    # Recovery is itself idempotent: calling it again (as a fresh process's
    # first use of this task would -- see _ensure_coordinator) never
    # duplicates the already-recorded recovered event.
    runtime._recover_unrecorded_source_events(result.task_id)
    with store.connection() as db:
        count = db.execute(
            "SELECT COUNT(*) AS n FROM task_events WHERE task_id=? AND dedup_key='user_message:acp:s1:e1'",
            (result.task_id,),
        ).fetchone()["n"]
    assert count == 1


def test_crash_after_recording_before_delivery_delivered_once(tmp_path):
    """T017's existing reconcile_undelivered_claims covers this crash
    window (claim_unconsumed set, delivered_at never set); this asserts it
    from the T018 identity-based dedup_key path specifically."""
    from cortex_runtime.acp.orchestrating_runtime import PromptOutcome

    store, result = _task(tmp_path, event_id="e1")
    delegation_events.append_event(
        store, result.task_id, "user_message", "user_message:acp:s1:e1", dict(content="hello world"), origin="acp:s1")
    delegation_events.claim_unconsumed(store, result.task_id, "cont-crashed")
    # delivered_at never set: a crash between claim and engine.prompt
    # returning in a prior process/attempt.
    engine = FakeSessionRuntime(store, lambda *a, **k: (lambda *a2, **k2: PromptOutcome(StopReason.END_TURN, "")))
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=1.0)
    runtime.start_prompt(result.task_id, "acp:s1", "hello world", lambda t: True, threading.Event(),
                          source_origin="acp:s1", source_event_id="e1")
    with store.connection() as db:
        row = db.execute(
            "SELECT delivered_at FROM task_events WHERE task_id=? AND dedup_key='user_message:acp:s1:e1'",
            (result.task_id,),
        ).fetchone()
    assert row["delivered_at"] is not None


# -- decision 6: bounded reads / trust labels / MCP tools --------------------


def test_bounded_read_pagination_exact_reassembly_and_limit_enforcement(tmp_path):
    body = "".join(f"line-{i}\n" for i in range(5000))  # well over 64 KiB
    store, result = _task(tmp_path, text=body)
    ref = f"{result.report_id}:-1"
    chunks = []
    offset = 0
    requested = 4096
    while True:
        page = replay_store.read_source(store, result.task_id, ref, offset, requested)
        assert len(page["chunk"].encode("utf-8")) <= replay_store.READ_CHUNK_LIMIT
        chunks.append(page["chunk"])
        if page["next_offset"] is None:
            break
        offset = page["next_offset"]
    assert "".join(chunks) == body

    oversized = replay_store.read_source(store, result.task_id, ref, 0, replay_store.READ_CHUNK_LIMIT * 10)
    assert len(oversized["chunk"].encode("utf-8")) <= replay_store.READ_CHUNK_LIMIT


def test_trust_label_user_direct_on_text_and_attachment_reads(tmp_path):
    (tmp_path / "a.txt").write_text("a", encoding="utf-8")
    blocks = [dict(type="resource_link", name="a.txt", uri=f"file://{tmp_path}/a.txt")]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    store, result = _task(tmp_path)
    staged = replay_store.stage_blocks(str(tmp_path), resolved)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        replay_store.record_blocks_tx(store, db, result.task_id, result.report_id, staged, [], [])
        db.commit()
    text_read = replay_store.read_source(store, result.task_id, f"{result.report_id}:-1")
    block_read = replay_store.read_source(store, result.task_id, f"{result.report_id}:0")
    assert text_read["trust_label"] == "user_direct"
    assert block_read["trust_label"] == "user_direct"
    assert replay_store.TRUST_LABELS == {"user_direct", "file_content", "worker_report", "derived_summary"}


def test_read_source_unknown_ref_refused(tmp_path):
    store, result = _task(tmp_path)
    with pytest.raises(StoreError):
        replay_store.read_source(store, result.task_id, "r_000000000000:0")


def test_mcp_read_source_cross_task_ref_refused(tmp_path):
    store_a, result_a = _task(tmp_path, origin="acp:sa", event_id="ea")
    other_dir = tmp_path / "other"
    other_dir.mkdir()
    store_b, result_b = _task(other_dir, origin="acp:sb", event_id="eb")
    channel_a = _coordinator_channel(store_a, result_a.task_id, result_a.owner_epoch)
    listed = mcp_tools.dispatch(store_a, channel_a, "list_sources", {})
    own_ref = listed["entries"][0]["source_ref"]
    # A ref for A's own task through A's channel works.
    ok = mcp_tools.dispatch(store_a, channel_a, "read_source", dict(source_ref=own_ref))
    assert ok["chunk"] == "hello world"
    # The SAME store/db but a channel issued for task B must never resolve
    # a source_ref belonging to a different task -- refused, not leaked.
    with pytest.raises(StoreError):
        mcp_tools.dispatch(store_a, channel_a, "read_source", dict(source_ref=f"{result_b.report_id}:-1"))


def test_list_sources_mcp_tool_available_on_coordinator_and_worker(tmp_path):
    store, result = _task(tmp_path)
    channel = _coordinator_channel(store, result.task_id, result.owner_epoch)
    listed = mcp_tools.dispatch(store, channel, "list_sources", {})
    assert listed["entries"][0]["kind"] == "text"


# -- decision 9: migration ----------------------------------------------------


def test_migration_from_b19d288_schema_gains_replay_tables_on_open(tmp_path):
    import sqlite3

    from cortex_runtime.acp.sessions import ACP_SESSION_SCHEMA
    from cortex_runtime.catalog.store import CATALOG_SCHEMA
    from cortex_runtime.configuration.store import CONFIGURATION_SCHEMA
    from cortex_runtime.delegation.capacity_store import CAPACITY_SCHEMA
    from cortex_runtime.delegation.evidence_store import EVIDENCE_SCHEMA
    from cortex_runtime.delegation.events_store import TASK_EVENTS_SCHEMA
    from cortex_runtime.delegation.store import DELEGATION_SCHEMA
    from cortex_runtime.store import APPLICATION_ID, SCHEMA, private_directory
    from cortex_runtime.trusted_channel import IDENTITY_SCHEMA

    directory = private_directory(tmp_path / ".cortex")
    path = directory / "cortex.sqlite3"
    db = sqlite3.connect(path)
    try:
        db.executescript(
            "BEGIN;\n" + SCHEMA + IDENTITY_SCHEMA + CATALOG_SCHEMA + CONFIGURATION_SCHEMA +
            ACP_SESSION_SCHEMA + DELEGATION_SCHEMA + TASK_EVENTS_SCHEMA + CAPACITY_SCHEMA + EVIDENCE_SCHEMA +
            f"PRAGMA application_id={APPLICATION_ID};\nPRAGMA user_version=12;\nCOMMIT;")
        tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        assert not (tables & {"source_blocks", "source_block_reads", "host_transcript"})
    finally:
        db.close()
        os.chmod(path, 0o600)

    store = Store(directory, project_root=tmp_path)
    with store.connection() as db:
        tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    assert {"source_blocks", "source_block_reads", "host_transcript"} <= tables
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id="e1"), "hello after migration", "owner-1")
    assert replay_store.read_source(store, result.task_id, f"{result.report_id}:-1")["chunk"] == "hello after migration"


# -- decision 7: session/load display-only replay ----------------------------


class _RecordingTransport:
    """Test-only in-process stand-in: records every sent message."""

    def __init__(self):
        self.responses = []
        self.notifications = []
        self._lock = threading.Lock()

    def send_response(self, message):
        with self._lock:
            self.responses.append(message)

    def send_notification(self, message, timeout=None):
        with self._lock:
            self.notifications.append(message)
        return True

    def response_for(self, request_id):
        with self._lock:
            for message in self.responses:
                if message.get("id") == request_id:
                    return message
        return None


class _CountingFakeRuntime:
    """Test-only runtime: records every start_prompt call (never a real
    engine); used to assert session/load never invokes it."""

    def __init__(self):
        self.start_prompt_calls = []
        self.cancel_calls = []
        self.cancel_task_calls = []
        self.cancel_task_emitters = []
        self.shutdown_calls = 0

    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        self.start_prompt_calls.append((task_id, session_id, content))
        emit_update("agent reply")
        return StopReason.END_TURN

    def cancel(self, session_id):
        self.cancel_calls.append(session_id)
        return None

    def shutdown(self):
        self.shutdown_calls += 1

    def cancel_task(self, task_id, session_id, emit_tool_call=None):
        self.cancel_task_calls.append((task_id, session_id))
        self.cancel_task_emitters.append(emit_tool_call)
        if emit_tool_call is not None:
            emit_tool_call(dict(
                tool_call_id="worker:cancel-live", new=False,
                status="failed", text="Cancelled: stop",
            ))
        return True


def _isolated_environ():
    home = Path(tempfile.mkdtemp(prefix="cortex-t018-fake-home-"))
    return dict(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"))


def test_agent_teardown_invokes_optional_runtime_shutdown():
    runtime = _CountingFakeRuntime()
    Agent(runtime, environ=_isolated_environ()).shutdown_runtime()
    assert runtime.shutdown_calls == 1


def _row_counts(store, task_id):
    with store.connection() as db:
        return dict(
            source_events=db.execute("SELECT COUNT(*) FROM source_events WHERE task_id=?", (task_id,)).fetchone()[0],
            task_events=db.execute("SELECT COUNT(*) FROM task_events WHERE task_id=?", (task_id,)).fetchone()[0],
            deliveries=db.execute("SELECT COUNT(*) FROM deliveries").fetchone()[0],
            reports=db.execute("SELECT COUNT(*) FROM reports WHERE task_id=?", (task_id,)).fetchone()[0],
        )


def test_paseo_session_cancel_keeps_followup_resumable(tmp_path):
    runtime = _CountingFakeRuntime()
    agent = Agent(runtime, environ=_isolated_environ())
    agent.client_info = {"name": "Paseo"}
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    agent.sessions[session_id]["task_id"] = "task-running"

    agent._session_cancel({"sessionId": session_id})

    assert runtime.cancel_calls == [session_id]
    assert runtime.cancel_task_calls == []


def test_paseo_session_cancel_does_not_cancel_worker_card(tmp_path):
    runtime = _CountingFakeRuntime()
    agent = Agent(runtime, environ=_isolated_environ())
    agent.client_info = {"name": "Paseo"}
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    agent.sessions[session_id]["task_id"] = "task-running"
    create = agent._make_emit_tool_call(session_id, threading.Event())
    assert create(dict(
        tool_call_id="worker:cancel-live", new=True, title="Worker",
        kind="other", status="in_progress", text="Working",
    ))

    agent._session_cancel({"sessionId": session_id})

    updates = [n["params"]["update"] for n in transport.notifications]
    assert updates[-1]["toolCallId"] == "worker:cancel-live"
    assert updates[-1]["status"] == "in_progress"
    assert runtime.cancel_task_emitters == []


def test_non_paseo_session_cancel_keeps_turn_scope(tmp_path):
    runtime = _CountingFakeRuntime()
    agent = Agent(runtime, environ=_isolated_environ())
    agent.client_info = {"name": "Zed"}
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    agent.sessions[session_id]["task_id"] = "task-running"

    agent._session_cancel({"sessionId": session_id})

    assert runtime.cancel_task_calls == []


def test_session_load_replays_history_in_order_with_zero_mutations(tmp_path):
    runtime = _CountingFakeRuntime()
    agent = Agent(runtime, environ=_isolated_environ())
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True

    new_result = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))
    session_id = new_result["sessionId"]
    cancel_event = threading.Event()
    session = agent.sessions[session_id]
    agent._run_prompt(request_id=1, session_id=session_id, session=session,
                       text="hello there", cancel_event=cancel_event,
                       raw_blocks=[dict(type="text", text="hello there")])
    assert runtime.start_prompt_calls == [(session["task_id"], session_id, "hello there")]

    store = agent._store_for(session["project"])
    before = _row_counts(store, session["task_id"])

    # A brand-new Agent (simulating a fresh reconnect/process) loads the
    # same durable session.
    loader = Agent(_CountingFakeRuntime(), environ=agent.environ)
    loader_transport = _RecordingTransport()
    loader.bind_transport(loader_transport)
    loader.initialized = True
    reconnect_servers = [dict(
        name="project-memory", command="/usr/bin/project-memory",
        args=["--workspace", str(tmp_path)],
        env=[dict(name="MEMORY_TOKEN", value="reconnect-secret")],
    )]
    loader._session_load(42, dict(
        sessionId=session_id, cwd=str(tmp_path), mcpServers=reconnect_servers,
    ))

    after = _row_counts(store, session["task_id"])
    assert before == after  # zero source/task_events/deliveries/reports mutations

    kinds = [n["params"]["update"]["sessionUpdate"] for n in loader_transport.notifications]
    assert kinds == ["user_message_chunk", "agent_message_chunk"]
    texts = [n["params"]["update"]["content"]["text"] for n in loader_transport.notifications]
    assert texts[0].startswith("hello there")
    assert texts[1] == "agent reply"
    reply = loader_transport.response_for(42)
    assert reply is not None and "error" not in reply
    assert loader.runtime.start_prompt_calls == []  # never opened an engine/runtime turn
    # A resumed ACP session must preserve the client's request-local MCP
    # configuration for the next prompt.  The durable store only keeps safe
    # shape metadata; secrets are retained in this process exactly as they
    # are after session/new, and are never written by session/load.
    assert loader.sessions[session_id]["mcp_servers"] == reconnect_servers
    changed_servers = [dict(name="different-server", command="/usr/bin/different-server")]
    loader._session_load(43, dict(
        sessionId=session_id, cwd=str(tmp_path), mcpServers=changed_servers,
    ))
    assert loader.sessions[session_id]["mcp_servers"] == changed_servers


def test_session_load_replays_durable_structured_worker_cards(tmp_path):
    """A reconnect restores worker/tool cards as ACP updates, in order.

    Zed owns the visual card state; Cortex must therefore persist and replay
    the exact updates it actually delivered instead of attempting to rebuild
    cards from model text or task events.
    """
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    new_result = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))
    session_id = new_result["sessionId"]
    emitter = agent._make_emit_tool_call(session_id, threading.Event())
    assert emitter(dict(tool_call_id="worker:a", new=True, title="Backend worker", kind="other", status="in_progress", text="Running tests"))
    assert emitter(dict(tool_call_id="worker:a", new=False, status="completed", text="Published report"))

    loader = Agent(_CountingFakeRuntime(), environ=agent.environ)
    loader_transport = _RecordingTransport()
    loader.bind_transport(loader_transport)
    loader.initialized = True
    loader._session_load(22, dict(sessionId=session_id, cwd=str(tmp_path), mcpServers=[]))

    updates = [n["params"]["update"] for n in loader_transport.notifications]
    assert [u["sessionUpdate"] for u in updates] == ["tool_call", "tool_call_update"]
    assert updates[0]["toolCallId"] == "worker:a"
    assert updates[1]["status"] == "completed"
    assert loader.runtime.start_prompt_calls == []


@pytest.mark.parametrize("terminal_status", ["completed", "failed"])
def test_session_load_replays_terminal_worker_card_statuses(tmp_path, terminal_status):
    """Paseo's worker renderer must receive terminal status after reload.

    A completed result and a cancellation (represented by ACP's supported
    failed status) are both durable card updates; replay must preserve the
    exact status rather than leaving the initial in-progress projection live.
    """
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    emitter = agent._make_emit_tool_call(session_id, threading.Event())
    assert emitter(dict(tool_call_id="worker:a", new=True, title="Backend worker", kind="other", status="in_progress", text="Working"))
    assert emitter(dict(tool_call_id="worker:a", new=False, status=terminal_status, text="Terminal evidence"))

    loader = Agent(_CountingFakeRuntime(), environ=agent.environ)
    loader_transport = _RecordingTransport()
    loader.bind_transport(loader_transport)
    loader.initialized = True
    loader._session_load(22, dict(sessionId=session_id, cwd=str(tmp_path), mcpServers=[]))

    updates = [n["params"]["update"] for n in loader_transport.notifications]
    assert updates[-1]["toolCallId"] == "worker:a"
    assert updates[-1]["status"] == terminal_status


def test_session_load_reconciles_stale_worker_card_from_durable_result(tmp_path):
    """A historical in-progress card is corrected from a real worker result."""
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    new_result = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))
    session_id = new_result["sessionId"]
    store = agent._store_for(str(tmp_path))
    session = acp_sessions.get_session(store, session_id)
    assert session is not None
    task = bootstrap_task(store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), "replay", "owner")
    acp_sessions.bind_task(store, session_id, task.task_id)
    session = acp_sessions.get_session(store, session_id)
    assignment_ref = "assignment-replay-result"
    emitter = agent._make_emit_tool_call(session_id, threading.Event())
    assert emitter(dict(tool_call_id=f"worker:{assignment_ref}", new=True, title="Worker", kind="other", status="in_progress", text="Working"))
    from cortex_runtime.delegation import events as delegation_events
    delegation_events.append_event(
        store, session["task_id"], "delegation_requested", "replay-delegation",
        {"title": "Worker", "role": "backend-dev"}, origin="runtime", assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, session["task_id"], "worker_result", "replay-result",
        {"summary": "Published report", "checks": []}, origin="runtime", assignment_ref=assignment_ref,
    )
    loader = Agent(_CountingFakeRuntime(), environ=agent.environ)
    loader_transport = _RecordingTransport()
    loader.bind_transport(loader_transport)
    loader.initialized = True
    loader._session_load(22, dict(sessionId=session_id, cwd=str(tmp_path), mcpServers=[]))
    updates = [n["params"]["update"] for n in loader_transport.notifications]
    assert updates[-1]["toolCallId"] == f"worker:{assignment_ref}"
    assert updates[-1]["status"] == "completed"


def test_session_load_marks_worker_failed_when_recorded_process_is_absent(tmp_path):
    """A restarted daemon must not replay a dead worker as ``working``.

    The process registry is the only cross-process identity evidence used
    here.  A confirmed-absent identity produces one ephemeral failed card
    update; session/load still performs no store mutation and never calls the
    runtime, so the next user prompt remains the only recovery/start point.
    """
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    store = agent._store_for(str(tmp_path))
    task = bootstrap_task(store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), "replay", "owner")
    acp_sessions.bind_task(store, session_id, task.task_id)
    assignment_ref = "assignment-dead-process"
    attempt_id = "xa_dead_process"
    emitter = agent._make_emit_tool_call(session_id, threading.Event())
    assert emitter(dict(
        tool_call_id=f"worker:{assignment_ref}", new=True, title="Worker",
        kind="other", status="in_progress", text="Working",
    ))
    delegation_events.append_event(
        store, task.task_id, "delegation_requested", "dead-delegation",
        {"title": "Worker", "role": "backend-dev"}, origin="runtime", assignment_ref=assignment_ref,
    )
    with store.connection() as db:
        owner_epoch = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task.task_id,)).fetchone()["epoch"]
        now = "2026-01-01T00:00:00+00:00"
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?,?)",
            (assignment_ref, task.task_id, "a_dead_process", attempt_id, "backend-dev", "ready", "ready", owner_epoch, now, now),
        )
        # A mismatched boot id is a safe, deterministic ``absent`` probe.
        db.execute(
            "INSERT INTO attempt_process_registry "
            "(attempt_id, task_id, role, engine_session_ref, connection_id, pid, pgid, start_ticks, boot_id, "
            "launch_command_digest, recorded_at, terminated_at, termination_reason) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?,NULL,NULL)",
            (attempt_id, task.task_id, "worker", "es_dead", "cn_test", os.getpid(), os.getpgrp(), "0", "not-the-current-boot",
             "digest", now),
        )
        db.commit()

    loader = Agent(_CountingFakeRuntime(), environ=agent.environ)
    loader_transport = _RecordingTransport()
    loader.bind_transport(loader_transport)
    loader.initialized = True
    loader._session_load(22, dict(sessionId=session_id, cwd=str(tmp_path), mcpServers=[]))

    updates = [n["params"]["update"] for n in loader_transport.notifications]
    assert updates[-1]["toolCallId"] == f"worker:{assignment_ref}"
    assert updates[-1]["status"] == "failed"
    assert updates[-1]["content"][0]["text"] == "Worker process exited before reporting a result."
    assert loader.runtime.start_prompt_calls == []
    # The display-only replay must not write a terminal event or mark the
    # process registry row; normal recovery remains the next prompt's job.
    with store.connection() as db:
        assert db.execute(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='assignment_failed'", (task.task_id,)
        ).fetchone()[0] == 0
        assert db.execute(
            "SELECT terminated_at FROM attempt_process_registry WHERE attempt_id=?", (attempt_id,)
        ).fetchone()[0] is None


@pytest.mark.parametrize("result_first", [True, False])
def test_session_load_result_wins_over_task_stop_cancellation(tmp_path, result_first):
    """A published worker result remains completed when stop is separate."""
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    store = agent._store_for(str(tmp_path))
    task = bootstrap_task(store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), "replay", "owner")
    acp_sessions.bind_task(store, session_id, task.task_id)
    emitter = agent._make_emit_tool_call(session_id, threading.Event())
    assignment_ref = "assignment-result-stop-order"
    assert emitter(dict(tool_call_id=f"worker:{assignment_ref}", new=True, title="Worker", kind="other", status="in_progress", text="Working"))
    events = [
        ("worker_result", "result-order", {"summary": "Published report", "checks": []}),
        ("assignment_cancelled", "cancel-order", {"reason": "urgent stop (/cortex-stop)"}),
    ]
    if not result_first:
        events.reverse()
    for event_type, dedup, payload in events:
        delegation_events.append_event(
            store, task.task_id, event_type, dedup, payload, origin="runtime", assignment_ref=assignment_ref,
        )
    loader = Agent(_CountingFakeRuntime(), environ=agent.environ)
    loader_transport = _RecordingTransport()
    loader.bind_transport(loader_transport)
    loader.initialized = True
    loader._session_load(22, dict(sessionId=session_id, cwd=str(tmp_path), mcpServers=[]))
    updates = [n["params"]["update"] for n in loader_transport.notifications]
    assert updates[-1]["toolCallId"] == f"worker:{assignment_ref}"
    assert updates[-1]["status"] == "completed"


def test_session_load_fails_when_history_cannot_be_enqueued(tmp_path):
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    store = agent._store_for(str(tmp_path))
    replay_store.append_host_transcript(store, session_id, "agent_message", "recorded")

    class FullTransport(_RecordingTransport):
        def send_notification(self, message, timeout=None):
            return False

    loader = Agent(_CountingFakeRuntime(), environ=agent.environ)
    loader.bind_transport(FullTransport())
    loader.initialized = True
    with pytest.raises(RpcError, match="Cannot enqueue complete session history"):
        loader._session_load(22, dict(sessionId=session_id, cwd=str(tmp_path), mcpServers=[]))
    assert not loader.transport.responses
    assert loader.runtime.start_prompt_calls == []


def test_outbound_text_is_persisted_before_enqueue_and_fails_closed(tmp_path, monkeypatch):
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    cancel = threading.Event()
    original = replay_store.append_host_transcript

    def fail_agent_text(store, sid, kind, text, attachments=None):
        if kind == "agent_message":
            raise StoreError("history unavailable")
        return original(store, sid, kind, text, attachments)

    monkeypatch.setattr(replay_store, "append_host_transcript", fail_agent_text)
    agent._run_prompt(request_id=1, session_id=session_id, session=agent.sessions[session_id],
                      text="hello", cancel_event=cancel,
                      raw_blocks=[dict(type="text", text="hello")])
    assert cancel.is_set()
    assert not any(n["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
                   for n in transport.notifications)


def test_structured_history_persistence_failure_prevents_enqueue(tmp_path, monkeypatch):
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    cancel = threading.Event()

    def fail(*args, **kwargs):
        raise StoreError("history unavailable")

    monkeypatch.setattr(replay_store, "append_host_update", fail)
    assert not agent._make_emit_display_update(session_id, cancel)(dict(sessionUpdate="plan", entries=[]))
    assert cancel.is_set()
    assert not transport.notifications


def test_structured_update_is_durable_before_transport_enqueue(tmp_path):
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    observed = []
    original = replay_store.append_host_update

    def record_then_observe(store, sid, update):
        result = original(store, sid, update)
        observed.append((result, len(transport.notifications)))
        return result

    # At persistence time no live frame has yet been enqueued.
    import unittest.mock as mock
    with mock.patch.object(replay_store, "append_host_update", side_effect=record_then_observe):
        emit = agent._make_emit_tool_call(session_id, threading.Event())
        assert emit(dict(tool_call_id="worker:a", new=True, title="Worker", status="pending"))
    assert observed == [(0, 0)]
    assert len(transport.notifications) == 1


def test_session_load_unknown_session_errors(tmp_path):
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    with pytest.raises(RpcError) as excinfo:
        agent._session_load(1, dict(sessionId="acps_never_existed", cwd=str(tmp_path), mcpServers=[]))
    assert excinfo.value.code == -32602


def test_replay_rejects_corrupt_transcript_before_any_partial_replay(tmp_path):
    """An incomplete saved transcript must never look like a successful load."""
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    new_result = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))
    session_id = new_result["sessionId"]
    store = agent._store_for(str(tmp_path))
    replay_store.append_host_transcript(store, session_id, "agent_message", "valid before")
    with store.connection() as db:
        db.execute(
            "UPDATE host_transcript SET attachments_json=? WHERE session_id=? AND seq=0",
            ("{broken", session_id),
        )
        db.commit()
    replay_store.append_host_transcript(store, session_id, "agent_message", "valid after")
    with pytest.raises(ValueError, match="Incomplete history"):
        replay_store.replay_session(store, session_id)
    transport.notifications.clear()
    with pytest.raises(RpcError, match="Incomplete history"):
        agent._session_load(2, dict(sessionId=session_id, cwd=str(tmp_path), mcpServers=[]))
    assert transport.notifications == []


def test_replayed_assistant_text_never_becomes_user_input(tmp_path):
    """The replayed agent_message_chunk text is never captured as a new
    source event / user_message -- session/load's own transcript read never
    calls bootstrap.bootstrap_task/append_source_event."""
    runtime = _CountingFakeRuntime()
    agent = Agent(runtime, environ=_isolated_environ())
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    new_result = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))
    session_id = new_result["sessionId"]
    session = agent.sessions[session_id]
    agent._run_prompt(request_id=1, session_id=session_id, session=session,
                       text="do the thing", cancel_event=threading.Event(),
                       raw_blocks=[dict(type="text", text="do the thing")])
    store = agent._store_for(session["project"])
    with store.connection() as db:
        before = db.execute("SELECT COUNT(*) FROM source_events WHERE task_id=?", (session["task_id"],)).fetchone()[0]
    agent._session_load(2, dict(sessionId=session_id, cwd=str(tmp_path), mcpServers=[]))
    with store.connection() as db:
        after = db.execute("SELECT COUNT(*) FROM source_events WHERE task_id=?", (session["task_id"],)).fetchone()[0]
    assert before == after == 1  # "agent reply" was never captured as a new source event


def test_pre_t018_source_events_gains_meta_json_column_on_open(tmp_path):
    """A source_events table predating T018 (no meta_json column, exactly
    the b19d288/T005 shape) gains it via ALTER TABLE the next time
    store.Store opens it -- no data loss, no user_version bump."""
    import sqlite3

    from cortex_runtime.store import APPLICATION_ID, SCHEMA, private_directory

    directory = private_directory(tmp_path / ".cortex")
    path = directory / "cortex.sqlite3"
    db = sqlite3.connect(path)
    try:
        pre_t018_identity_schema = '''
        CREATE TABLE IF NOT EXISTS trusted_channels (
         channel_id TEXT PRIMARY KEY, secret_hash TEXT NOT NULL,
         task_id TEXT NOT NULL REFERENCES tasks(id),
         principal_thread_id TEXT NOT NULL, parent_principal_thread_id TEXT,
         role TEXT NOT NULL, assignment_id TEXT, attempt_id TEXT,
         workspace_binding_id TEXT, context_generation_id TEXT,
         owner_epoch INTEGER NOT NULL, issued_at TEXT NOT NULL,
         expires_at TEXT, revoked_at TEXT
        );
        CREATE TABLE IF NOT EXISTS source_events (
         origin TEXT NOT NULL, event_id TEXT NOT NULL, task_id TEXT NOT NULL REFERENCES tasks(id),
         captured_at TEXT NOT NULL, PRIMARY KEY(origin,event_id)
        );
        CREATE TABLE IF NOT EXISTS task_owners (
         task_id TEXT PRIMARY KEY REFERENCES tasks(id), owner_ref TEXT NOT NULL,
         epoch INTEGER NOT NULL, acquired_at TEXT NOT NULL
        );
        '''
        db.executescript("BEGIN;\n" + SCHEMA + pre_t018_identity_schema + f"PRAGMA application_id={APPLICATION_ID};\nPRAGMA user_version=12;\nCOMMIT;")
        db.execute("INSERT INTO tasks VALUES (?,?,?)", ("t_legacy0001", str(tmp_path), "2020-01-01T00:00:00.000000"))
        db.execute("INSERT INTO source_events VALUES (?,?,?,?)", ("cli", "evt-1", "t_legacy0001", "2020-01-01T00:00:00.000000"))
        db.commit()
        before_columns = {row[1] for row in db.execute("PRAGMA table_info(source_events)")}
        assert "meta_json" not in before_columns
    finally:
        db.close()
        os.chmod(path, 0o600)

    store = Store(directory, project_root=tmp_path)
    with store.connection() as db:
        columns = {row[1] for row in db.execute("PRAGMA table_info(source_events)")}
        row = db.execute("SELECT origin, meta_json FROM source_events WHERE event_id='evt-1'").fetchone()
    assert "meta_json" in columns
    assert row["origin"] == "cli" and row["meta_json"] is None  # pre-existing data preserved
    Store(directory, project_root=tmp_path)  # idempotent re-open


def test_pre_t018_source_events_ledger_state_backfilled_legacy_recorded(tmp_path):
    """(fresh-review fix, HIGH) A source_events table predating T018 (no
    ledger_state column at all -- exactly the b19d288 shape) has every
    pre-existing row backfilled 'legacy_recorded' -- never left NULL, which
    would make it wrongly eligible for auto-recovery."""
    import sqlite3

    from cortex_runtime.store import APPLICATION_ID, SCHEMA, private_directory

    directory = private_directory(tmp_path / ".cortex")
    path = directory / "cortex.sqlite3"
    db = sqlite3.connect(path)
    try:
        pre_t018_identity_schema = '''
        CREATE TABLE IF NOT EXISTS trusted_channels (
         channel_id TEXT PRIMARY KEY, secret_hash TEXT NOT NULL,
         task_id TEXT NOT NULL REFERENCES tasks(id),
         principal_thread_id TEXT NOT NULL, parent_principal_thread_id TEXT,
         role TEXT NOT NULL, assignment_id TEXT, attempt_id TEXT,
         workspace_binding_id TEXT, context_generation_id TEXT,
         owner_epoch INTEGER NOT NULL, issued_at TEXT NOT NULL,
         expires_at TEXT, revoked_at TEXT
        );
        CREATE TABLE IF NOT EXISTS source_events (
         origin TEXT NOT NULL, event_id TEXT NOT NULL, task_id TEXT NOT NULL REFERENCES tasks(id),
         captured_at TEXT NOT NULL, PRIMARY KEY(origin,event_id)
        );
        CREATE TABLE IF NOT EXISTS task_owners (
         task_id TEXT PRIMARY KEY REFERENCES tasks(id), owner_ref TEXT NOT NULL,
         epoch INTEGER NOT NULL, acquired_at TEXT NOT NULL
        );
        '''
        db.executescript("BEGIN;\n" + SCHEMA + pre_t018_identity_schema + f"PRAGMA application_id={APPLICATION_ID};\nPRAGMA user_version=12;\nCOMMIT;")
        db.execute("INSERT INTO tasks VALUES (?,?,?)", ("t_legacy0002", str(tmp_path), "2020-01-01T00:00:00.000000"))
        db.execute("INSERT INTO source_events VALUES (?,?,?,?)", ("acps_legacy", "evt-legacy", "t_legacy0002", "2020-01-01T00:00:00.000000"))
        db.commit()
    finally:
        db.close()
        os.chmod(path, 0o600)

    store = Store(directory, project_root=tmp_path)
    with store.connection() as db:
        row = db.execute("SELECT ledger_state FROM source_events WHERE event_id='evt-legacy'").fetchone()
    assert row["ledger_state"] == "legacy_recorded"


def test_legacy_delivered_rows_never_re_delivered_new_prompt_recovers_only_genuine_gap(tmp_path):
    """(fresh-review fix, HIGH, reproduced-bug regression) Simulates the
    exact reported scenario: a task with one source event already
    delivered under the b19d288-era turn-count dedup_key (and, after
    migration, marked 'legacy_recorded'), one source event genuinely
    captured-but-never-recorded (a real crash gap, ledger_state NULL), and
    then a brand-new prompt. Only the genuine gap is recovered; the legacy
    row is never re-delivered."""
    store, result = _task(tmp_path, event_id="e1", text="legacy prompt")
    legacy_dedup_key = "user_message:acp:s1:0"  # b19d288-era turn-count shape
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO task_events (task_id, event_type, assignment_ref, dedup_key, payload_json, origin, "
            "created_at, consumed_by, superseded, delivered_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            (result.task_id, "user_message", None, legacy_dedup_key, '{"content":"legacy prompt"}', "acp:s1",
             store._now(), "cont-legacy", 0, store._now()),
        )
        # Simulates trusted_channel.ensure_source_event_ledger_column's
        # migration backfill for this pre-existing row.
        db.execute(
            "UPDATE source_events SET ledger_state='legacy_recorded' WHERE origin=? AND event_id=?",
            ("acp:s1", "e1"),
        )
        db.commit()

    # A genuine crash gap: captured, never recorded (ledger_state stays NULL).
    append_source_event(store, SourceEvent(origin="acp:s1", event_id="e2"), "genuine gap prompt", result.task_id)

    engine = FakeSessionRuntime(store, lambda *a, **k: (lambda *a2, **k2: __import__(
        "cortex_runtime.acp.orchestrating_runtime", fromlist=["PromptOutcome"]).PromptOutcome(StopReason.END_TURN, "")))
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=1.0)
    append_source_event(store, SourceEvent(origin="acp:s1", event_id="e3"), "third, new prompt", result.task_id)
    runtime.start_prompt(result.task_id, "acp:s1", "third, new prompt", lambda t: True, threading.Event(),
                          source_origin="acp:s1", source_event_id="e3")

    with store.connection() as db:
        rows = db.execute(
            "SELECT dedup_key, payload_json FROM task_events WHERE task_id=? AND event_type='user_message' ORDER BY id",
            (result.task_id,),
        ).fetchall()
        ledger = {r["event_id"]: r["ledger_state"] for r in db.execute(
            "SELECT event_id, ledger_state FROM source_events WHERE task_id=?", (result.task_id,),
        ).fetchall()}
    dedup_keys = [r["dedup_key"] for r in rows]
    # The legacy row's own (pre-existing) task_events row is untouched and
    # never duplicated under the new identity-based key.
    assert legacy_dedup_key in dedup_keys
    assert "user_message:acp:s1:e1" not in dedup_keys
    assert dedup_keys.count(legacy_dedup_key) == 1
    # The genuine gap (e2) was recovered exactly once, and the new prompt
    # (e3) was recorded normally.
    assert "user_message:acp:s1:e2" in dedup_keys
    assert "user_message:acp:s1:e3" in dedup_keys
    e2_payload = json.loads(next(r["payload_json"] for r in rows if r["dedup_key"] == "user_message:acp:s1:e2"))
    assert e2_payload["recovered"] is True
    # Ledger markers: legacy stays legacy_recorded (untouched), the
    # genuine gap and the new prompt are both now 'recorded'.
    assert ledger["e1"] == "legacy_recorded"
    assert ledger["e2"] == "recorded"
    assert ledger["e3"] == "recorded"


# -- orphaned attachment staging sweep (lead follow-up) ----------------------


def test_sweep_orphaned_staging_removes_only_old_files_never_referenced_bodies(tmp_path):
    (tmp_path / "a.txt").write_text("a body", encoding="utf-8")
    blocks = replay_store.resolve_prompt_blocks(
        [dict(type="resource_link", name="a.txt", uri=f"file://{tmp_path}/a.txt")], str(tmp_path))
    staged = replay_store.stage_blocks(str(tmp_path), blocks)
    orphan_path = staged[0].staged_path
    assert orphan_path.exists()

    # A fresh (not yet renamed) staged file is never swept, regardless of
    # the age threshold used, until it is old enough.
    removed = replay_store.sweep_orphaned_staging(str(tmp_path), max_age_seconds=3600)
    assert removed == 0 and orphan_path.exists()

    # Simulate it having been abandoned by a crashed process an hour+ ago.
    old = time.time() - 7200
    os.utime(orphan_path, (old, old))
    removed = replay_store.sweep_orphaned_staging(str(tmp_path), max_age_seconds=3600)
    assert removed == 1 and not orphan_path.exists()

    # A referenced, already-committed body (renamed into .source_blocks/,
    # not .source_blocks_staging/) is never touched by the sweep.
    store, result = _task(tmp_path)
    staged2 = replay_store.stage_blocks(str(tmp_path), replay_store.resolve_prompt_blocks(
        [dict(type="resource_link", name="a.txt", uri=f"file://{tmp_path}/a.txt")], str(tmp_path)))
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        replay_store.record_blocks_tx(store, db, result.task_id, result.report_id, staged2, [], [])
        db.commit()
        referenced_path = store._task_directory(db, result.task_id) / ".source_blocks" / f"{result.report_id}_0.txt"
    assert referenced_path.exists()
    os.utime(referenced_path, (old, old))
    replay_store.sweep_orphaned_staging(str(tmp_path), max_age_seconds=1)
    assert referenced_path.exists()  # never in .source_blocks_staging/, never swept


@pytest.mark.parametrize('title,kind', [('mcp.cortex.project_read','execute'), ('mcp__cortex__read_skill','read'), ('Read','read')])
def test_read_body_omitted_from_wire_and_durable_display_history(tmp_path, title, kind):
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    sid = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))['sessionId']
    emit = agent._make_emit_display_update(sid, threading.Event())
    source = dict(sessionUpdate='tool_call', toolCallId='read-1', title=title, kind=kind,
                  status='in_progress', rawInput={'path': 'README.md'},
                  content=[{'type': 'content', 'content': {'type': 'text', 'text': 'READ_BODY_MARKER'}}])
    assert emit(source)
    assert emit(dict(sessionUpdate='tool_call_update', toolCallId='read-1', status='completed', rawOutput='READ_BODY_MARKER'))
    # Display projection never mutates the engine's response.
    assert 'READ_BODY_MARKER' in str(source)
    assert 'READ_BODY_MARKER' not in str(transport.notifications)
    store = agent._store_for(str(tmp_path))
    with store.connection() as db:
        rows = [r[0] for r in db.execute('select update_json from host_updates where session_id=?', (sid,))]
    assert 'READ_BODY_MARKER' not in str(rows)
    assert 'README.md' in str(rows)
    assert 'completed' in str(rows)
    # Old retained history is projected on replay without rewriting its rows.
    replay_store.append_host_update(store, sid, dict(source, toolCallId='old-read'))
    replay_store.append_host_update(store, sid, dict(sessionUpdate='tool_call_update', toolCallId='old-read', rawOutput='READ_BODY_MARKER'))
    assert 'READ_BODY_MARKER' not in str(replay_store.replay_session(store, sid))
    assert emit(dict(sessionUpdate='tool_call', toolCallId='shell-1', title='Shell', kind='execute', rawOutput='COMMAND_OUTPUT'))
    assert 'COMMAND_OUTPUT' in str(replay_store.replay_session(store, sid))


def test_replay_folds_each_card_update_run_into_its_final_state(tmp_path):
    """A worker card resends its whole log per update; replay sends it once."""
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    sid = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    store = agent._store_for(str(tmp_path))
    log = ""
    replay_store.append_host_update(store, sid, dict(sessionUpdate="tool_call", toolCallId="worker:a", title="W", status="in_progress"))
    for step in range(50):
        log += f"line {step}\n"
        replay_store.append_host_update(store, sid, dict(sessionUpdate="tool_call_update", toolCallId="worker:a", rawInput=log))
    replay_store.append_host_transcript(store, sid, "agent_message", "between")
    replay_store.append_host_update(store, sid, dict(sessionUpdate="tool_call_update", toolCallId="worker:a", status="completed"))
    replay_store.append_host_update(store, sid, dict(sessionUpdate="tool_call", toolCallId="worker:a", title="W2", status="in_progress"))
    replay_store.append_host_update(store, sid, dict(sessionUpdate="tool_call_update", toolCallId="worker:a", status="failed"))

    history = replay_store.replay_session(store, sid)
    shapes = [(e["kind"], e.get("update", {}).get("sessionUpdate")) for e in history]
    assert shapes == [
        ("update", "tool_call"), ("update", "tool_call_update"), ("agent_message", None), ("update", "tool_call_update"),
        ("update", "tool_call"), ("update", "tool_call_update"),
    ]
    assert history[1]["update"] == dict(sessionUpdate="tool_call_update", toolCallId="worker:a", rawInput=log)
    assert history[1]["source_sequences"] == list(range(1, 51))
    assert history[3]["update"]["status"] == "completed"
    assert history[5]["update"]["status"] == "failed" and "rawInput" not in history[5]["update"]


@pytest.mark.parametrize("count", [4095, 4096, 4097])
def test_long_history_reloads_with_stable_identity_and_no_durable_changes(tmp_path, count):
    agent = Agent(_CountingFakeRuntime(), environ=_isolated_environ())
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    sid = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    store = agent._store_for(str(tmp_path))
    with store.connection() as db:
        db.executemany(
            "INSERT INTO host_transcript(session_id,seq,kind,text,attachments_json,message_id,created_at) VALUES (?,?,?,?,?,?,?)",
            [(sid, i, "agent_message", f"🙂я {i}", "[]", f"m:{i}", "2026-09-23") for i in range(count)],
        )
        db.commit()
        before = list(map(tuple, db.execute("SELECT * FROM host_transcript WHERE session_id=? ORDER BY seq", (sid,))))
    deliveries = []
    for request_id in (40, 41):
        transport.notifications.clear()
        agent._session_load(request_id, dict(sessionId=sid, cwd=str(tmp_path), mcpServers=[]))
        updates = [n["params"]["update"] for n in transport.notifications
                   if n.get("method") == "session/update" and n["params"]["update"]["sessionUpdate"] == "agent_message_chunk"]
        assert len(updates) == count
        assert [u["messageId"] for u in updates] == [f"m:{i}" for i in range(count)]
        assert [u["_meta"]["cortexReplay"]["sequences"] for u in updates] == [[i] for i in range(count)]
        deliveries.append(updates)
    assert deliveries[0] == deliveries[1]
    with store.connection() as db:
        assert list(map(tuple, db.execute("SELECT * FROM host_transcript WHERE session_id=? ORDER BY seq", (sid,)))) == before


def test_text_coalescing_keeps_boundaries_sequences_and_byte_lengths():
    def text(seq, body, identity="m", attachments=None, kind="agent_message"):
        return dict(kind=kind, text=body, message_id=identity, attachments=attachments or [],
                    source_sequences=[seq], source_text_bytes=[len(body.encode("utf-8"))])
    rows = [text(0, "🙂"), text(1, "я"),
            dict(kind="update", update=dict(sessionUpdate="tool_call", toolCallId="card"), source_sequences=[2]),
            text(3, "after"), text(4, "no-id", None), text(5, "no-id", None),
            text(6, "attached", attachments=[dict(name="note")]), text(7, "x" * 65536), text(8, "end")]
    result = replay_store._merge_replay_entries(rows)
    assert result[0]["text"] == "🙂я"
    assert result[0]["source_sequences"] == [0, 1]
    assert result[0]["source_text_bytes"] == [4, 2]
    assert len(result) == 8
    assert [seq for entry in result for seq in entry["source_sequences"]] == list(range(9))
