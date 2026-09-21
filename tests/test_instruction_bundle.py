from cortex_runtime.portable.instruction_bundle import (
    InstructionBundleLedger, InstructionSource, build_instruction_bundle,
    capture_instruction_bundle,
)
import sqlite3
from cortex_runtime.portable.roles import load_role


def test_bundle_is_deterministic_and_records_source_provenance():
    role = load_role("worker-backend-dev")
    source = InstructionSource.from_content("project:AGENTS.md", "project", "Keep changes scoped", version="v3")
    kwargs = dict(task_id="task-1", assignment_ref="as-1", attempt_id="at-1",
                  role_set=role, engine="codex-acp", engine_version="1.12.0",
                  project_sources=(source,), skill_catalog=(
                      InstructionSource("ref:testing", "reference", "1", "abc", "catalog"),
                  ), routes={"read": "runtime"}, capabilities={"mcp": True})
    first = build_instruction_bundle(**kwargs)
    second = build_instruction_bundle(**kwargs)
    assert first.bundle_id == second.bundle_id
    assert first.assembled_sha256 == second.assembled_sha256
    assert first.source_manifest_sha256
    assert "Keep changes scoped" in first.text
    assert first.sent_representation_sha256 is None


def test_binding_is_attempt_and_engine_specific_and_idempotent():
    role = load_role("worker-backend-dev")
    bundle = build_instruction_bundle(task_id="t", assignment_ref="a", attempt_id="x",
                                      role_set=role, engine="codex-acp", engine_version="1")
    bound = bundle.bind("wire text", engine="codex-acp", engine_version="1", attempt_id="x", session_ref="s")
    assert bound.delivery_bound
    assert bound.bind("wire text", engine="codex-acp", engine_version="1", attempt_id="x", session_ref="s") == bound
    import pytest
    with pytest.raises(ValueError, match="delivery_conflict"):
        bound.bind("wire text", engine="codex-acp", engine_version="1", attempt_id="x", session_ref="s2")
    with pytest.raises(ValueError, match="delivery_conflict"):
        bound.bind("different", engine="codex-acp", engine_version="1", attempt_id="x")
    with pytest.raises(ValueError, match="attempt_mismatch"):
        bundle.bind("wire text", engine="claude", engine_version="1", attempt_id="x")


def test_bundle_round_trip_rejects_tampered_assembled_text():
    role = load_role("worker-backend-dev")
    bundle = build_instruction_bundle(task_id="t", assignment_ref="a", attempt_id="x",
                                      role_set=role, engine="codex-acp", engine_version="1")
    payload = bundle.to_dict()
    payload["text"] += " tampered"
    import pytest
    with pytest.raises(ValueError, match="assembled_hash_mismatch"):
        type(bundle).from_dict(payload)


def test_capture_wraps_exact_text_without_reassembly():
    source = InstructionSource.from_content("role", "role", "canonical role", version="r2")
    text = "already assembled adapter text\nwith its exact boundaries"
    bundle = capture_instruction_bundle(
        task_id="t", assignment_ref="a", attempt_id="x", role_id="worker-backend-dev",
        role_kind="worker", engine="codex-acp", engine_version="1.12", assembled_text=text,
        sources=(source,),
    )
    assert bundle.text == text
    assert bundle.assembled_sha256
    assert bundle.sources[0].version == "r2"


class _SQLiteStore:
    def __init__(self, path):
        self.path = path

    def connection(self):
        return sqlite3.connect(self.path)


def test_ledger_persists_intended_and_sent_representation(tmp_path):
    role = load_role("worker-backend-dev")
    bundle = capture_instruction_bundle(
        task_id="t", assignment_ref="a", attempt_id="attempt-1", role_id=role.role_id,
        role_kind=role.kind, engine="codex-acp", engine_version="1.12", assembled_text="wire",
        sources=(InstructionSource(role.role_id, "role", "r1", role.sha256, role.source_path),),
    )
    ledger = InstructionBundleLedger(_SQLiteStore(str(tmp_path / "ledger.sqlite3")))
    ledger.save_intended(bundle)
    sent = ledger.record_sent(bundle, b"wire", engine="codex-acp", engine_version="1.12",
                              attempt_id="attempt-1", session_ref="session-1")
    assert sent.session_ref == "session-1"
    loaded = ledger.get(bundle.bundle_id)
    assert loaded is not None
    assert loaded.sent_representation_sha256 == sent.sent_representation_sha256
    assert loaded.sent_attempt_id == "attempt-1"
    assert loaded.sent_engine == "codex-acp"
    replacement = ledger.record_sent(bundle, b"wire", engine="codex-acp", engine_version="1.12",
                                     attempt_id="attempt-1", session_ref="session-2")
    assert replacement.session_ref == "session-2"
    assert ledger.get(bundle.bundle_id).session_ref == "session-2"
