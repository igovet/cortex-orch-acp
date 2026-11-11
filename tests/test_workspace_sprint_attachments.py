"""No-paid sprint and owner-input resource persistence and planning delivery."""
import base64
import json
import sys
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.workspace_api.attachments import MAX_BYTES
from cortex_runtime.workspace_contract import LIMITS, encode_frame, decode_frame
from test_workspace_api import api_for, create_command
from test_workspace_machine_tools import machine, ok

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "crates/workspace-service"))


def file(body=b"fixture", name="screen.png"):
    return dict(name=name, mimeType="image/png", data=base64.b64encode(body).decode())


def test_full_screenshot_is_saved_reopened_and_delivered_to_planning(machine):
    import machine_tools as m
    from planning_jobs import PlanningJobs
    # A real byte budget, not a name-only or a tiny-file claim.
    files = [file(b"\x89PNG\r\n\x1a\n" + b"x" * (MAX_BYTES - 8))]
    assert MAX_BYTES == 4 * 1024 * 1024
    request = dict(action="create", description="Plan from this screenshot", attachments=files)
    assert decode_frame(encode_frame(request)) == request
    made = ok(machine("product.sprint", request))
    saved = ok(machine("product.read"))["sprints"][0]
    assert saved["id"] == made["sprintId"] and saved["attachments"] == files
    seen = []
    def generate(prompt, options, expires, cancel):
        seen.append((prompt, options))
        return '{"tasks":[{"key":"one","description":"Use the screenshot","dependsOn":[]}]}'
    jobs = PlanningJobs(machine.repo.parent / "state", generate, threading.Event())
    jobs.tick(SimpleNamespace(project_id="wp_1"))
    jobs.join(3)
    assert len(seen) == 1
    assert seen[0][1]["attachments"] == files
    assert files[0]["data"] not in seen[0][0]  # bytes never serialized as text
    assert m.product_of(jobs.state, "wp_1")["sprints"][0]["planning"] == "ready"


@pytest.mark.parametrize("files", [
    [file()] * 9, [file(b"x" * (MAX_BYTES + 1))],
    [dict(file(), data="invalid!")], [dict(file(), data="Zh==")],
    [file(name="../outside")], ["name-only.png"],
])
def test_invalid_sprint_files_never_save(machine, files):
    reply = machine("product.sprint", dict(action="create", description="Keep me", attachments=files))
    assert not reply["ok"] and reply["code"] == "invalid_attachments"
    assert "4 MiB" in reply["message"]
    assert ok(machine("product.read"))["sprints"] == []


def test_product_budget_refuses_before_saving_an_unreadable_sprint(machine):
    files = [file(b"x" * MAX_BYTES)]
    ok(machine("product.sprint", dict(action="create", description="First", attachments=files)))
    before = ok(machine("product.read"))["sprints"]
    refused = machine("product.sprint", dict(action="create", description="Second", attachments=files))
    assert refused["code"] == "product_full"
    assert ok(machine("product.read"))["sprints"] == before


def test_planning_answers_and_messages_keep_resources_and_reject_overflow_atomically(machine):
    made = ok(machine("product.sprint", dict(action="create", description="Plain old client")))
    key = made["sprintId"]
    for kind in ("answer", "message"):
        ok(machine("product.sprint", dict(action="note", sprintId=key, kind=kind, text="Use this", attachments=[file()])))
    saved = ok(machine("product.read"))["sprints"][0]
    assert [e["attachments"] for e in saved["conversation"]] == [[file()], [file()]]
    before = json.dumps(saved, sort_keys=True)
    assert not machine("product.sprint", dict(action="note", sprintId=key, text="bad", attachments=[file()] * 9))["ok"]
    assert not machine("product.sprint", dict(action="note", sprintId=key, text="over total", attachments=[file(b"x" * MAX_BYTES)]))["ok"]
    assert json.dumps(ok(machine("product.read"))["sprints"][0], sort_keys=True) == before


def test_comments_and_queue_edits_preserve_replace_and_validate_bytes(machine):
    for op, key in (("product.comment", "comment"), ("product.message", "message")):
        item = dict(id=key, taskId="task", text="Look", attachments=[file()], sent=False,
                    workerId=None, delivery="queued")
        ok(machine(op, dict(action="add", taskId="task", **{key:item})))
        identity = {"commentId" if key == "comment" else "messageId": key, "taskId":"task"}
        ok(machine(op, dict(action="edit", text="Keep attachment", **identity)))
        product = ok(machine("product.read"))
        rows = product["comments"]["task"] if key == "comment" else product["messages"]
        assert rows[0]["attachments"] == [file()]
        assert not machine(op, dict(action="edit", text="Bad", attachments=[file()] * 9, **identity))["ok"]
        ok(machine(op, dict(action="edit", text="Remove attachment", attachments=[], **identity)))
        product = ok(machine("product.read"))
        rows = product["comments"]["task"] if key == "comment" else product["messages"]
        assert rows[0]["attachments"] == []


def test_sent_message_copy_is_kept_and_gives_way_before_a_queued_message_is_refused(machine):
    big = [file(b"x" * MAX_BYTES)]
    def row(key, delivery="recorded", files=big):
        return dict(id=key, taskId="task", workerId=None, text=key, mode="queue", delivery=delivery,
                    at=1, deliveredAt=None, failure=None, attachments=files)
    ok(machine("product.message", dict(action="add", taskId="task", message=row("sent"))))
    assert ok(machine("product.read"))["messages"][0]["attachments"] == big
    # The second full file only fits if the sent copy lets go of its bytes; its name stays.
    ok(machine("product.message", dict(action="add", taskId="task", message=row("waiting", "queued"))))
    messages = ok(machine("product.read"))["messages"]
    assert messages[0]["attachments"] == ["screen.png"] and messages[1]["attachments"] == big
    assert not machine("product.message", dict(action="add", taskId="task", message=dict(row("worker"), workerId="w")))["ok"]
    assert not machine("product.message", dict(action="add", taskId="task", message=row("sending", "delivered")))["ok"]


def test_task_creation_bytes_can_be_read_without_start_or_board_payload_duplication(machine):
    api = api_for(machine.repo)
    cmd = create_command()
    cmd["payload"]["attachments"] = [file(b"x" * MAX_BYTES)]
    task = api.handle(cmd)["target"]
    assert ok(machine("product.read", dict(taskId=task, attachments=True), project=api.project_id)) == cmd["payload"]["attachments"]
    assert "attachments" not in ok(machine("product.read", project=api.project_id))["tasks"][task]
    assert not machine("product.read", dict(taskId=task, attachments=True), project="wrong")["ok"]
