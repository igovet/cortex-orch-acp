from types import SimpleNamespace

import pytest

from cortex_runtime.delegation import candidate_checks
from cortex_runtime.delegation.candidate_checks import CandidateCheckError, run_candidate_check
from cortex_runtime.delegation.candidates import CANDIDATE_SCHEMA
from cortex_runtime.delegation.check_evidence import list_check_evidence
from cortex_runtime.store import Store


def _fixture(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=str(project))
    stage = project / ".cortex" / "candidates" / "c1" / "stage"
    stage.mkdir(parents=True)
    with store.connection() as db:
        db.execute("INSERT INTO tasks(id,project_root,created_at) VALUES(?,?,?)", ("task", str(project), "now"))
        db.executescript(CANDIDATE_SCHEMA)
        db.execute("INSERT INTO candidate_staging(candidate_id,task_id,project_root,stage_path,attempt_ids_json,source_bindings_json,base_identity_json,project_base_manifest_json,project_base_digest,content_digest,status,created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                    ("c1", "task", str(project), str(stage), "[]", "[]", "[]", "{}", "base", "digest", "ready", "now"))
    return store, project, stage


def _patch_runner(monkeypatch, stage, project):
    monkeypatch.setattr(candidate_checks, "_manifest", lambda root: {"candidate": "tree"})
    monkeypatch.setattr(candidate_checks, "_digest_manifest", lambda manifest: "digest")
    monkeypatch.setattr(candidate_checks, "_execute_bounded",
                        lambda *args, **kwargs: {"status": "completed", "exit_code": 0,
                                                 "output_digest": "out", "output_bytes": 3,
                                                 "output_truncated": False})


def test_runner_captures_observation_without_inventing_assessment(tmp_path, monkeypatch):
    store, project, stage = _fixture(tmp_path)
    _patch_runner(monkeypatch, stage, project)
    result = run_candidate_check(store, "task", "attempt", "c1", "digest", ["pytest", "-q"])
    assert result["observed_result"]["exit_code"] == 0
    assert result["environment"]["execution"] == "direct-user-v2"
    assert result["environment"]["variables"]["PYTHONDONTWRITEBYTECODE"] == "1"
    assert result["observed_result"]["output_digest"]
    assert "evidence" not in result
    assert list_check_evidence(store, "task") == []


def test_python_check_does_not_change_candidate_with_bytecode(tmp_path):
    store, project, stage = _fixture(tmp_path)
    source = stage / "test_math.py"
    source.write_text(
        "import unittest\n\n"
        "class MathTest(unittest.TestCase):\n"
        "    def test_sum(self):\n"
        "        self.assertEqual(sum((2, 3)), 5)\n"
    )
    # The fixture's fixed digest is intentional for the unit-level runner;
    # use the real manifest identity for the subprocess integration check.
    manifest = candidate_checks._manifest(str(stage))
    digest = candidate_checks._digest_manifest(manifest)
    with store.connection() as db:
        db.execute("UPDATE candidate_staging SET content_digest=? WHERE candidate_id=?", (digest, "c1"))
    result = run_candidate_check(
        store, "task", "attempt", "c1", digest,
        ["python3", "-m", "unittest", "test_math.py"],
    )
    assert result["observed_result"]["exit_code"] == 0
    assert result["environment"]["variables"]["PYTHONDONTWRITEBYTECODE"] == "1"
    assert not list(stage.rglob("__pycache__"))


def test_candidate_check_error_exposes_only_bounded_code():
    assert CandidateCheckError("candidate_changed_during_check").code == "candidate_changed_during_check"
    assert CandidateCheckError("unsafe /secret").code == "candidate_check_failed"


def test_runner_persists_only_explicit_semantic_assessment(tmp_path, monkeypatch):
    store, project, stage = _fixture(tmp_path)
    _patch_runner(monkeypatch, stage, project)
    result = run_candidate_check(
        store, "task", "attempt", "c1", "digest", ["pytest", "-q"],
        assessment={"status": "pass", "reason": "criterion evaluated"},
        idempotency_key="check-1",
    )
    assert result["evidence"]["observed_result"]["exit_code"] == 0
    assert result["evidence"]["assessment"]["status"] == "pass"


def test_runner_rejects_shell_and_digest_mismatch(tmp_path, monkeypatch):
    store, project, stage = _fixture(tmp_path)
    _patch_runner(monkeypatch, stage, project)
    with pytest.raises(CandidateCheckError, match="check_shell_command_forbidden"):
        run_candidate_check(store, "task", "attempt", "c1", "digest", ["sh", "-c", "true"])
    with pytest.raises(CandidateCheckError, match="candidate_digest_mismatch"):
        run_candidate_check(store, "task", "attempt", "c1", "other", ["pytest"])


def test_runner_rejects_candidate_drift_and_non_successful_pass(tmp_path, monkeypatch):
    store, project, stage = _fixture(tmp_path)
    _patch_runner(monkeypatch, stage, project)
    manifests = iter([{"candidate": "tree"}, {"candidate": "changed"}])
    monkeypatch.setattr(candidate_checks, "_manifest", lambda root: next(manifests))
    monkeypatch.setattr(candidate_checks, "_digest_manifest",
                        lambda manifest: "digest" if manifest["candidate"] == "tree" else "changed")
    with pytest.raises(CandidateCheckError, match="candidate_changed_during_check"):
        run_candidate_check(store, "task", "attempt", "c1", "digest", ["pytest"])

    _patch_runner(monkeypatch, stage, project)
    monkeypatch.setattr(candidate_checks, "_execute_bounded",
                        lambda *args, **kwargs: {"status": "timeout", "exit_code": None,
                                                 "output_digest": "out", "output_bytes": 0,
                                                 "output_truncated": False})
    with pytest.raises(CandidateCheckError, match="check_observation_not_successful"):
        run_candidate_check(store, "task", "attempt", "c1", "digest", ["pytest"],
                            assessment={"status": "pass"})


def test_output_preview_is_bounded_and_redacts_secrets_and_paths():
    preview = candidate_checks._safe_preview(
        b"token=very-secret-value file=/home/user/private.txt\n" + b"x" * 10000
    )
    assert "very-secret-value" not in preview
    assert "/home/user/private.txt" not in preview
    assert "<redacted>" in preview
    assert "<path>" in preview
    assert len(preview) <= candidate_checks.MAX_OUTPUT_PREVIEW_CHARS
