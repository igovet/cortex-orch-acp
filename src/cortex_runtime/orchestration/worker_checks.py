"""Execute retained worker checks, without asking a model to imitate a receipt."""
import hashlib
import json
import threading
import time

from ..contracts import StoreError

RERUN_TIMEOUT_SECONDS = 60


def _hash(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def active_checks(db, task_id, assignment_ref, checks):
    """Use runtime evidence, not reviewer-only current_outcome presentation fields."""
    from ..delegation.evidence import current_outcomes
    provision = db.execute(
        "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
        (task_id, assignment_ref)).fetchone()
    current = current_outcomes(db, task_id, provision["attempt_id"], checks) if provision else None
    for index, check in enumerate(checks):
        if current is not None:
            withdrawn = (current[index]["outcome"] == "retracted"
                         and check.get("retracted_by") == current[index].get("retracted_by"))
        else:
            withdrawn = False  # Missing receipts/provisioning cannot prove a withdrawal.
        if not withdrawn:
            yield index, check


def targets(db, task_id, revision):
    """Literal gaps or receipt-backed failures; never trust a descriptive argv."""
    from ..delegation.evidence import assess_check
    from ..task_workspace import task_root
    from pathlib import Path
    root = str(task_root(db, task_id))
    found = []
    projection_index = 0
    for node in db.execute(
            "SELECT assignment_ref,result_json FROM runtime_nodes WHERE task_id=? AND revision=? "
            "AND result_json IS NOT NULL ORDER BY rowid", (task_id, revision)):
        result = json.loads(node["result_json"])
        provision = db.execute(
            "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
            (task_id, node["assignment_ref"])).fetchone()
        for index, check in active_checks(db, task_id, node["assignment_ref"], result.get("checks", [])):
            public_index = projection_index
            projection_index += 1
            outcome = check.get("outcome")
            argv = check.get("command") if outcome in ("not_run", "unverified") else None
            order = 0
            cwd = root
            if outcome in ("failed", "passed") and provision:
                assessment = assess_check(db, task_id, provision["attempt_id"], outcome, check.get("tool_call_ref"))
                identity = assessment.get("identity")
                if (assessment["strength"] == "verified" and identity
                        and Path(identity[2]).resolve().is_relative_to(Path(root).resolve())):
                    argv, order = list(identity[1]), assessment.get("order", 0)
                    cwd = identity[2]
            if not isinstance(argv, list) or not argv or not all(isinstance(w, str) and w and "\0" not in w for w in argv):
                if outcome not in ("not_run", "unverified"):
                    continue
                argv = []
            original = {k: v for k, v in check.items() if k != "closed_by"}
            found.append(dict(assignment=node["assignment_ref"], index=index, check_hash=_hash(original),
                              name=check["name"], outcome=outcome, argv=argv, cwd=cwd, order=order,
                              projection_index=public_index, policy_blocker=_policy_blocker(dict(argv=argv), check),
                              kind="policy" if _policy_blocker(dict(argv=argv), check) else "automated" if argv else "manual",
                              detail=check.get("blocker", "")))
    task = db.execute("SELECT plan_json FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()
    plan = json.loads(task[0] or "{}") if task else {}
    latest = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' "
                        "AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
                        (task_id,)).fetchone()
    verification = json.loads(latest[0]) if latest else {}
    for index, argv in enumerate(plan.get("verification", [])):
        checks = verification.get("checks", []) if verification.get("revision") == revision else []
        check = checks[index] if index < len(checks) else {}
        found.append(dict(assignment="runtime-verification", index=index,
                          check_hash=_hash(dict(revision=revision, argv=argv)), name="Runtime verification " + str(index + 1),
                          outcome="passed" if check.get("complete") and check.get("exit_code") == 0 else "unverified",
                          argv=argv, cwd=root, order=check.get("started_event_id", 0),
                          projection_index=projection_index + index, policy_blocker=_policy_blocker(dict(argv=argv), {}),
                          kind="policy" if _policy_blocker(dict(argv=argv), {}) else "automated"))
    return found


def _identity(target):
    return {k: target[k] for k in ("assignment", "index", "check_hash", "argv", "cwd")}


def _policy_blocker(target, check):
    # A disclosure cannot authorize a paid probe or override an explicit consent
    # requirement. Unknown technical problems are established by the real runner.
    blocker = check.get("blocker", "")
    text = blocker.casefold()
    if ("paid" in text or "authoriz" in text or "approval" in text or
            "policy" in text or "consent" in text or "permission" in text or
            "denied" in text or "refus" in text or "prohibit" in text or
            any("cortex_codex_probe.py" in w or "cortex_claude_probe.py" in w or
                w == "--i-authorize-paid-call" for w in target["argv"])):
        return blocker or "Owner authorization required"
    return None


def execute(runtime, task_id, target, request_key, *, cancel=None, emit_tool_call=None, input_digest=None):
    """Reserve once before launch; interrupted/ambiguous reservations never replay."""
    if not target["argv"] or target.get("kind") == "manual":
        return dict(outcome="unsupported", message="Check manually: no command was recorded. This item is not an automated check.")
    if target.get("policy_blocker"):
        return dict(outcome="unsupported", message="Not run: owner authorization is required for this check.")
    from .state import TaskMachine, event
    from ..delegation.revision import capture_revision
    observed = capture_revision(runtime._task_workspace_root(task_id))
    digest = observed.get("dirty_digest") if observed.get("kind") == "observed" else None
    if not digest:
        raise StoreError("check_revision_unavailable")
    context = TaskMachine(runtime._store).context(task_id)
    with runtime._store.connection() as db:
        owner_epoch = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()[0]
    binding = dict(revision=context["revision"], digest=digest, owner_epoch=owner_epoch, target=_identity(target),
                   input_digest=input_digest)
    def reserve(db):
        prior = db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='WORKER_CHECK_REQUESTED' "
            "AND json_extract(payload_json,'$.request_key')=?", (task_id, request_key)).fetchone()
        if prior:
            if json.loads(prior[0])["binding"] != binding:
                raise StoreError("conflicting_check_request")
            return False
        current = db.execute("SELECT r.revision,o.epoch FROM runtime_tasks r JOIN task_owners o USING(task_id) WHERE r.task_id=?", (task_id,)).fetchone()
        matches = [t for t in targets(db, task_id, context["revision"]) if _identity(t) == _identity(target)]
        if not current or current[0] != context["revision"] or current[1] != owner_epoch or len(matches) != 1:
            raise StoreError("stale_check")
        event(db, task_id, "WORKER_CHECK_REQUESTED", dict(request_key=request_key, binding=binding))
        return True
    if not runtime._store.immediate_write(reserve):
        return dict(outcome="recorded")
    try:
        if not target["argv"]:
            raise StoreError("verification_command_unavailable")
        if target.get("policy_blocker"):
            raise StoreError("check_requires_owner_authorization")
        from .controller import _verification_observation
        run_context = dict(context, plan=dict(context["plan"] or {}, verification=[target["argv"]]))
        result = _verification_observation(runtime, task_id, run_context, digest, emit_tool_call,
                                           cancel or threading.Event(), time.monotonic_ns(),
                                           start_index=target["index"] if target["assignment"] == "runtime-verification" else 0,
                                           timeout_seconds=RERUN_TIMEOUT_SECONDS, cwd=target["cwd"])
    except Exception as error:
        reason = "verification_command_unavailable" if str(error) == "verification_command_unavailable" else "verification_execution_error:" + type(error).__name__
        result = dict(blocker=target.get("policy_blocker") or reason, stale=False)
    after = capture_revision(runtime._task_workspace_root(task_id))
    def finish(db):
        current = db.execute("SELECT r.revision,o.epoch FROM runtime_tasks r JOIN task_owners o USING(task_id) WHERE r.task_id=?",
                             (task_id,)).fetchone()
        if (not current or current[0] != binding["revision"] or current[1] != owner_epoch
                or after.get("dirty_digest") != digest):
            result.update(stale=True, blocker="check_binding_changed")
        event(db, task_id, "WORKER_CHECK_FINISHED", dict(request_key=request_key, binding=binding, result=result))
        if target["assignment"] == "runtime-verification":
            _refresh_final(db, task_id, binding, result)
    runtime._store.immediate_write(finish)
    if result.get("blocker"):
        message = ("The recorded check tool is not available on this machine." if result["blocker"] == "verification_executable_unavailable"
                   else "No executable command was recorded for this check." if result["blocker"] == "verification_command_unavailable"
                   else "The check could not start because a required tool, permission or service is unavailable.")
        return dict(outcome="unsupported", message=message + " Restore it and rerun, or use Accept with the acknowledgement checkbox for this technical blocker.")
    return dict(outcome="recorded")


def _refresh_final(db, task_id, binding, observed):
    """Refresh evidence, not lifecycle, only on the same reviewed source."""
    from .state import event, reviewed_digest
    if observed.get("stale") or reviewed_digest(db, task_id, binding["revision"]) != binding["digest"]:
        return
    latest = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' "
                        "AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
                        (task_id,)).fetchone()
    previous = json.loads(latest[0]) if latest else {}
    if previous.get("revision") != binding["revision"] or previous.get("revision_digest") != binding["digest"]:
        return
    checks = list(previous.get("checks", []))
    index = binding["target"]["index"]
    if index >= len(checks) or len(observed.get("checks", [])) != 1:
        return
    checks[index] = observed["checks"][0]
    task = db.execute("SELECT plan_json FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()
    commands = json.loads(task[0])["verification"]
    passed = len(checks) == len(commands) and all(
        c.get("argv") == argv and c.get("complete") is True and type(c.get("exit_code")) is int
        and c["exit_code"] == 0 and c.get("revision_digest") == binding["digest"]
        for c, argv in zip(checks, commands))
    event(db, task_id, "VERIFICATION_PASSED" if passed else "VERIFICATION_FAILED",
          dict(revision=binding["revision"], revision_digest=binding["digest"], checks=checks))


def automatic(runtime, task_id, cancel, emit_tool_call):
    """One attempt per exact gap and observed revision, after ordinary verification."""
    from .state import TaskMachine
    context = TaskMachine(runtime._store).context(task_id)
    with runtime._store.connection() as db:
        candidates = targets(db, task_id, context["revision"])
        from .check_closure import runtime_gap_closures
        already_closed = runtime_gap_closures(db, task_id, context["revision"])
    for target in candidates:
        if cancel.is_set():
            return
        if target.get("kind") in ("manual", "policy") or target["outcome"] not in ("not_run", "unverified") or (target["assignment"], target["index"]) in already_closed:
            continue
        from ..delegation.revision import capture_revision
        observation = capture_revision(runtime._task_workspace_root(task_id))
        key = "automatic-check:" + _hash(dict(target=_identity(target), digest=observation.get("dirty_digest")))
        execute(runtime, task_id, target, key, cancel=cancel, emit_tool_call=emit_tool_call)


def latest_attempts(db, task_id, revision):
    latest = {}
    for row in db.execute(
            "SELECT id,type,payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' "
            "AND type IN ('WORKER_CHECK_REQUESTED','WORKER_CHECK_FINISHED') ORDER BY id", (task_id,)):
        payload = json.loads(row["payload_json"])
        binding = payload["binding"]
        if binding["revision"] == revision:
            target = binding["target"]
            latest[(target["assignment"], target["index"])] = (row, payload)
    return latest


def reconcile_closures(db, task_id, revision, base):
    """A later failed, pending or ambiguous rerun supersedes an earlier pass."""
    result = dict(base)
    for key, (row, _) in latest_attempts(db, task_id, revision).items():
        ref = result.get(key)
        if ref:
            start = db.execute("SELECT max(id) FROM runtime_events WHERE task_id=? "
                               "AND type='CHECK_EXECUTION_STARTED' AND json_extract(payload_json,'$.command_ref')=?",
                               (task_id, ref)).fetchone()[0]
            if not start or row["id"] > start:
                result.pop(key, None)
    result.update(closures(db, task_id, revision))
    return result


def unresolved(db, task_id, revision):
    """Only a real technical launch/policy blocker permits owner acknowledgement."""
    result = {}
    for key, (row, payload) in latest_attempts(db, task_id, revision).items():
        observed = payload.get("result", {})
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
        reason = observed.get("blocker")
        if not owner or payload["binding"].get("owner_epoch") != owner[0]:
            # A new settled-session owner does not cure a missing command/tool.
            # Carry only these technical non-passes, never permissions, passes or stale runs.
            from ..delegation.revision import capture_revision
            from ..task_workspace import task_root
            binding = payload["binding"]
            unchanged = (owner and row["type"] == "WORKER_CHECK_FINISHED" and not observed.get("stale")
                         and reason in ("verification_command_unavailable", "verification_executable_unavailable")
                         and any(_identity(t) == binding.get("target") for t in targets(db, task_id, revision))
                         and capture_revision(task_root(db, task_id)).get("dirty_digest") == binding.get("digest"))
            if not unchanged:
                result[key] = dict(outcome="unverified")
                continue
        technical = reason in ("verification_command_unavailable", "verification_executable_unavailable") or (isinstance(reason, str) and reason.startswith("verification_execution_error:") and not reason.endswith("StoreError"))
        if row["type"] == "WORKER_CHECK_FINISHED" and technical and not observed.get("stale"):
            result[key] = dict(outcome="not_run", technicalReason=reason[:1000])
        elif observed.get("checks"):
            check = observed["checks"][0]
            code = check.get("exit_code")
            if check.get("failure") == "timeout" or type(code) is int and code != 0 and check.get("complete") is True:
                result[key] = dict(outcome="failed")
            else:
                result[key] = dict(outcome="unverified")
        else:
            result[key] = dict(outcome="unverified")
    return result


def closures(db, task_id, revision):
    """Revalidate the latest attempt, full output, source publication and current review."""
    from .check_closure import _retained_output_complete
    from .state import reviewed_digest
    from ..delegation.revision import capture_revision
    from ..task_workspace import task_root
    latest = latest_attempts(db, task_id, revision)
    if not latest:
        return {}
    digest = reviewed_digest(db, task_id, revision)
    current = capture_revision(task_root(db, task_id))
    if not digest or current.get("kind") != "observed" or current.get("dirty_digest") != digest:
        return {}
    closed = {}
    for target in targets(db, task_id, revision):
        if target["assignment"] == "runtime-verification":
            continue
        key = (target["assignment"], target["index"])
        if key not in latest:
            continue
        row, payload = latest[key]
        binding, result = payload["binding"], payload.get("result", {})
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
        if not owner or binding.get("owner_epoch") != owner[0]:
            continue
        if (row["type"] != "WORKER_CHECK_FINISHED" or binding["target"] != _identity(target)
                or binding["digest"] != digest or result.get("digest") != digest
                or result.get("stale") or result.get("blocker") or len(result.get("checks", [])) != 1):
            continue
        check = result["checks"][0]
        if (check.get("argv") != target["argv"] or check.get("complete") is not True
                or type(check.get("exit_code")) is not int or check["exit_code"] != 0
                or check.get("revision_digest") != digest or check.get("failure") or check.get("truncated")
                or check.get("started_event_id", 0) <= target["order"]):
            continue
        ref = check.get("command_ref")
        starts = db.execute("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' "
                            "AND type='CHECK_EXECUTION_STARTED' AND json_extract(payload_json,'$.command_ref')=?",
                            (task_id, ref)).fetchall()
        expected = dict(command_ref=ref, argv=target["argv"], revision=revision, revision_digest=digest,
                        index=0, profile=check.get("profile"))
        if check.get("cwd", str(task_root(db, task_id))) != target["cwd"]:
            continue
        if check.get("cwd"):
            expected["cwd"] = check["cwd"]
        if (len(starts) != 1 or starts[0]["id"] != check["started_event_id"]
                or json.loads(starts[0]["payload_json"]) != expected):
            continue
        requests = db.execute("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' "
                              "AND type='WORKER_CHECK_REQUESTED' AND json_extract(payload_json,'$.request_key')=?",
                              (task_id, payload["request_key"])).fetchall()
        finishes = db.execute("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' "
                              "AND type='CHECK_EXECUTION_FINISHED' AND json_extract(payload_json,'$.command_ref')=?",
                              (task_id, ref)).fetchall()
        if (len(requests) != 1 or json.loads(requests[0]["payload_json"])["binding"] != binding
                or not requests[0]["id"] < starts[0]["id"] < row["id"] or len(finishes) != 1
                or not starts[0]["id"] < finishes[0]["id"] < row["id"]
                or any(json.loads(finishes[0]["payload_json"]).get(k) != v for k, v in check.items() if k != "output_tail")):
            continue
        outputs = db.execute("SELECT * FROM large_outputs WHERE task_id=? AND source_ref=?", (task_id, ref)).fetchall()
        if (len(outputs) != 1 or outputs[0]["id"] != check.get("large_output_id")
                or outputs[0]["owner_scope"] != "coordinator" or outputs[0]["source_kind"] != "terminal"
                or outputs[0]["exit_code"] != 0 or outputs[0]["failure"] or outputs[0]["signal"]
                or outputs[0]["truncated"] or not _retained_output_complete(db, task_id, outputs[0])):
            continue
        # A successful rerun does not certify changed source or replace review.
        provision = db.execute("SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                               (task_id, target["assignment"])).fetchone()
        node = db.execute("SELECT result_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                          (task_id, target["assignment"])).fetchone()
        report = json.loads(node[0]).get("report_ref")
        publication = db.execute("SELECT kind,dirty_digest FROM attempt_revisions WHERE attempt_id=? AND phase=?",
                                 (provision[0], "worker_publish:" + str(report))).fetchall() if provision else []
        if len(publication) == 1 and publication[0]["kind"] == "observed" and publication[0]["dirty_digest"] == digest:
            closed[key] = ref
    return closed
