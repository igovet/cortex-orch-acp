"""T024: recovery.large_output_store -- owner-scoped persistence for
oversized tool/terminal output, with exit status/truncation/failure kept
intact across a file round-trip and chunked reads."""
from __future__ import annotations

import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery.large_output_store import (
    LargeOutputError, persist_output, persist_output_stream, read_output,
)
from cortex_runtime.store import Store


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def test_persist_and_read_round_trips_bytes_exactly(tmp_path):
    store, task_id = _task(tmp_path)
    data = b"line one\nline two\n" * 5000
    record = persist_output(
        store, task_id=task_id, owner_scope="coordinator", source_kind="terminal", data=data,
        source_ref="term_1", exit_code=0, signal=None, failure=None, truncated=False,
    )
    result = read_output(store, task_id=task_id, owner_scope="coordinator", record_id=record["id"], chunk_size=1_000_000)
    assert result["encoding"] == "utf-8"
    assert result["chunk"] == data.decode("utf-8")
    assert result["byte_count"] == len(data)
    assert result["exit_status"] == dict(exit_code=0, signal=None)
    assert result["truncated"] is False
    assert result["failure"] is None
    assert result["next_offset"] is None


def test_truncation_and_failure_and_exit_status_survive_persistence(tmp_path):
    store, task_id = _task(tmp_path)
    record = persist_output(
        store, task_id=task_id, owner_scope="a_worker1", source_kind="terminal", data=b"partial output",
        source_ref="term_2", exit_code=None, signal="SIGKILL", failure="killed_grace_expired", truncated=True,
    )
    result = read_output(store, task_id=task_id, owner_scope="a_worker1", record_id=record["id"])
    assert result["truncated"] is True
    assert result["exit_status"] == dict(exit_code=None, signal="SIGKILL")
    assert result["failure"] == "killed_grace_expired"


def test_chunked_reads_cover_the_whole_output(tmp_path):
    import base64
    store, task_id = _task(tmp_path)
    data = bytes(range(256)) * 500  # not valid UTF-8 -> exercises the base64 fallback
    record = persist_output(
        store, task_id=task_id, owner_scope="coordinator", source_kind="tool_result", data=data,
    )
    collected = b""
    offset = 0
    while True:
        chunk_result = read_output(store, task_id=task_id, owner_scope="coordinator", record_id=record["id"], offset=offset, chunk_size=997)
        assert chunk_result["encoding"] == "base64"
        collected += base64.b64decode(chunk_result["chunk"])
        # Every chunk still carries the full record's metadata, not only the first/last.
        assert chunk_result["byte_count"] == len(data)
        if chunk_result["next_offset"] is None:
            break
        offset = chunk_result["next_offset"]
    assert collected == data


def test_read_with_wrong_owner_scope_is_refused_as_unknown(tmp_path):
    store, task_id = _task(tmp_path)
    record = persist_output(
        store, task_id=task_id, owner_scope="a_worker1", source_kind="terminal", data=b"secret worker output",
    )
    with pytest.raises(LargeOutputError, match="unknown_large_output_for_scope"):
        read_output(store, task_id=task_id, owner_scope="a_worker2", record_id=record["id"])
    with pytest.raises(LargeOutputError, match="unknown_large_output_for_scope"):
        read_output(store, task_id=task_id, owner_scope="coordinator", record_id=record["id"])


def test_persisted_file_is_owner_only_mode(tmp_path):
    store, task_id = _task(tmp_path)
    record = persist_output(store, task_id=task_id, owner_scope="coordinator", source_kind="terminal", data=b"x")
    with store.connection() as db:
        directory = store._task_directory(db, task_id) / "large_outputs"
    path = directory / record["filename"]
    import stat
    mode = path.stat().st_mode
    assert stat.S_IMODE(mode) == 0o600

@pytest.mark.parametrize(
    "data",
    [
        pytest.param(b"x" * (2 * 1024 * 1024), id="multi-megabyte"),
        pytest.param(bytes(range(256)) * 8192, id="non-utf8"),
    ],
)
def test_stream_persistence_is_byte_exact(tmp_path, data):
    store, task_id = _task(tmp_path)
    source_path = tmp_path / "verification-output.bin"
    source_path.write_bytes(data)

    with source_path.open("rb") as source:
        record = persist_output_stream(
            store, task_id=task_id, owner_scope="coordinator",
            source_kind="terminal", source_ref="verification:test", stream=source,
            exit_code=0,
        )
        assert source.tell() == len(data)

    with store.connection() as db:
        directory = store._task_directory(db, task_id) / "large_outputs"
    persisted = (directory / record["filename"]).read_bytes()
    assert persisted == data
    assert record["byte_count"] == len(data)


def test_stream_read_failure_removes_partial_output_file(tmp_path):
    store, task_id = _task(tmp_path)

    class FailingStream:
        def __init__(self):
            self.calls = 0

        def read(self, _size):
            self.calls += 1
            if self.calls == 1:
                return b"partial output"
            raise OSError("simulated source failure")

    with pytest.raises(OSError, match="simulated source failure"):
        persist_output_stream(
            store, task_id=task_id, owner_scope="coordinator",
            source_kind="terminal", stream=FailingStream(),
        )

    with store.connection() as db:
        directory = store._task_directory(db, task_id) / "large_outputs"
        rows = db.execute(
            "SELECT COUNT(*) FROM large_outputs WHERE task_id=?", (task_id,),
        ).fetchone()[0]
    assert list(directory.iterdir()) == []
    assert rows == 0
