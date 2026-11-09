"""Attachment bytes and atomic identified answers; no model or image viewing."""
import base64
import copy
import hashlib
import stat

import pytest

from cortex_runtime.acp.replay_store import list_sources, read_source
from cortex_runtime.workspace_api import WorkspaceError
from cortex_runtime.workspace_api.attachments import MAX_BYTES, checked
from cortex_runtime.workspace_contract import ContractError
from test_workspace_decisions import asking, command, decisions


def attachment(body=b"fixture resource", name="notes.txt", mime="text/plain"):
    return dict(name=name, mimeType=mime, data=base64.b64encode(body).decode())


@pytest.mark.parametrize("items", [
    [attachment(name="../outside")], [attachment(name="a\\b")],
    [dict(attachment(), data="invalid!")], [dict(attachment(), data="Zh==")],
    [attachment(mime="text/plain\nsecret")], [attachment()] * 9,
    [attachment(b"x" * (MAX_BYTES + 1))],
    [attachment(b"x" * (MAX_BYTES // 2 + 1))] * 2,
])
def test_reject_invalid_or_excessive_bytes(items):
    with pytest.raises((WorkspaceError, ContractError)):
        checked(items)


def test_answer_resources_commit_once_with_answer_and_are_private(tmp_path):
    observed = []
    api, execution, _ = asking(tmp_path, dispatch=observed.append)
    cmd = command(decisions(api, execution), texts=["first\nsecond"])
    cmd["payload"]["answers"][0]["attachments"] = [attachment(), attachment(b"\x89PNG\r\nfixture", "paste.png", "image/png")]
    receipt = api.handle(cmd)
    assert receipt["outcome"] == "recorded"
    assert api.handle(cmd) == receipt
    assert len(observed) == 1
    sources = [s for s in list_sources(api.store, observed[0]["task_id"])["entries"] if s["kind"] != "text"]
    assert len(sources) == 2
    assert sources[0]["name"] == "notes.txt"
    assert read_source(api.store, observed[0]["task_id"], sources[0]["source_ref"])["chunk"] == "fixture resource"
    assert sources[1]["sha256"] == hashlib.sha256(b"\x89PNG\r\nfixture").hexdigest()
    with api.store.connection() as db:
        row = db.execute("SELECT answer_text FROM pending_questions").fetchone()
        assert row[0] == "first\nsecond"
    bodies = list((tmp_path / ".cortex").rglob("*.bin"))
    assert len(bodies) == 2
    assert all(stat.S_IMODE(p.stat().st_mode) == 0o600 for p in bodies)
    changed = copy.deepcopy(cmd)
    changed["payload"]["answers"][0]["attachments"][0] = attachment(b"changed")
    assert api.handle(changed)["outcome"] == "payload_conflict"


def test_stale_answer_retains_wait_and_captures_no_resource(tmp_path):
    api, execution, _ = asking(tmp_path)
    cmd = command(decisions(api, execution))
    cmd["expectedVersion"] = "999"
    cmd["payload"]["answers"][0]["attachments"] = [attachment()]
    assert api.handle(cmd)["outcome"] == "stale_version"
    assert all(d["status"] == "pending" for d in decisions(api, execution))
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM source_blocks").fetchone()[0] == 0
    assert not list((tmp_path / ".cortex/.source_blocks_staging").iterdir())


def test_answer_capture_failure_never_answers(tmp_path, monkeypatch):
    from cortex_runtime.workspace_api import attachments
    api, execution, _ = asking(tmp_path)
    cmd = command(decisions(api, execution))
    cmd["payload"]["answers"][0]["attachments"] = [attachment()]
    monkeypatch.setattr(attachments, "answer_source_tx", lambda *a: (_ for _ in ()).throw(OSError("fixture disk failure")))
    with pytest.raises(OSError):
        api.handle(cmd)
    assert all(d["status"] == "pending" for d in decisions(api, execution))
