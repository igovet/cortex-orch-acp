#!/usr/bin/env python3
"""Run the rendered journey several times in a row and report each run in one line.

Runs are strictly one at a time. Every run's complete output stays in --logs; only
step names, process evidence and counts are printed, never message bodies.
"""
import argparse
import json
import re
from pathlib import Path
import subprocess
import sys
import time

CHECK = Path(__file__).resolve().parent / "check_rendered.py"


def summarize(text, exit_code):
    """Accept only one complete, consistent run; summaries cannot erase failures."""
    steps = []
    errors = []
    for line in text.splitlines():
        try:
            step = json.loads(line)
        except ValueError:
            if line.lstrip().startswith(("{", "[")):
                errors.append("malformed_json_evidence")
            continue
        if isinstance(step, dict) and "step" in step:
            steps.append(step)
    last = lambda name: next((step for step in reversed(steps) if step["step"] == name), {})
    summary, incomplete = last("visual_audit_summary"), last("journey_incomplete")
    # Exception evidence is never a successful terminal record, even if a later
    # run_evidence or passing summary claims otherwise. Keep it in diagnostics.
    evidence = last("native_process_evidence") or last("run_evidence")
    passed = [step for step in steps if step.get("status") == "passed"]
    failures = [step["step"] for step in steps if step.get("status") == "failed"]
    required = ("bundled_native_window", "run_evidence", "visual_audit_summary", "owned_process_cleanup")
    for name in required:
        if sum(step["step"] == name for step in steps) != 1:
            errors.append("missing_or_duplicate:" + name)
    if any(step.get("status") not in ("passed", "diagnostic") for step in steps):
        errors.append("unsuccessful_step")
    if incomplete or last("native_process_evidence"):
        errors.append("journey_incomplete")
    if [step["step"] for step in steps[-3:]] != list(required[1:]):
        errors.append("invalid_terminal_order")
    binary = last("bundled_native_window")
    digest = binary.get("binary_sha256")
    if binary.get("status") != "passed" or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        errors.append("invalid_binary_identity")
    if not any(step["step"] == "visual_audit" and step.get("status") == "passed" for step in steps):
        errors.append("missing_visual_audit")
    if summary.get("status") != "passed" or type(summary.get("failures")) is not int or summary["failures"] != 0:
        errors.append("invalid_visual_summary")
    if last("owned_process_cleanup").get("status") != "passed":
        errors.append("cleanup_not_passed")
    # The common journey closes the app before final evidence, restarts the
    # service, and leaves the outer Wayland compositor running (absent on X11).
    expected = {"status": "diagnostic", "failure_class": None, "app": None,
                "app_processes": [], "page_answers": None,
                "web_process_terminations": [], "cores": [], "service": {"state": "running"}}
    if any(key not in evidence or evidence[key] != value for key, value in expected.items()):
        errors.append("invalid_native_evidence")
    if "compositor" not in evidence or evidence["compositor"] not in (None, {"state": "running"}):
        errors.append("invalid_compositor_evidence")
    if type(evidence.get("app_launches")) is not int or evidence["app_launches"] < 1:
        errors.append("missing_app_launches")
    complete = exit_code == 0 and not errors
    return {
        "complete": complete, "exit_code": exit_code, "steps_passed": len(passed), "last_step": passed[-1]["step"] if passed else None,
        "screens_audited": sum(step["step"] == "visual_audit" and step.get("status") == "passed" for step in steps),
        "failures": failures, "evidence_errors": errors, "reason": incomplete.get("reason"), "at": incomplete.get("at"),
        "failure_class": evidence.get("failure_class"), "app": evidence.get("app"), "page_answers": evidence.get("page_answers"), "app_processes": evidence.get("app_processes"),
        "web_process_terminations": evidence.get("web_process_terminations"), "compositor": evidence.get("compositor"), "service": evidence.get("service"),
        "app_stderr": evidence.get("app_stderr"), "compositor_stderr": evidence.get("compositor_stderr"), "app_launches": evidence.get("app_launches"),
        "largest_app_stderr_bytes": evidence.get("largest_app_stderr_bytes"), "cores": evidence.get("cores"), "core_limit": evidence.get("core_limit"),
        "stale_nodes_tolerated": sum(step["step"] == "accessibility_node_stale" for step in steps), "binary_sha256": last("bundled_native_window").get("binary_sha256"),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", required=True)
    parser.add_argument("--logs", required=True, help="directory that keeps each run's complete output")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--display", choices=("wayland", "x11"), default="wayland")
    parser.add_argument("--expect-sha256", help="fail any run that did not observe this exact desktop binary")
    args = parser.parse_args()
    logs = Path(args.logs)
    logs.mkdir(parents=True, exist_ok=True)
    command = [sys.executable, "-B", str(CHECK), "--artifacts", args.artifacts, "--container", *(["--journey-wayland"] if args.display == "wayland" else [])]
    bad = 0
    for number in range(1, args.runs + 1):
        started = time.monotonic()
        log = logs / f"{args.display}-{number}.jsonl"
        with log.open("wb") as sink:
            exit_code = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=sink, stderr=subprocess.STDOUT).returncode
        result = summarize(log.read_text(errors="replace"), exit_code)
        if args.expect_sha256 and result["binary_sha256"] != args.expect_sha256:
            result["complete"] = False
        bad += not result["complete"]
        print(json.dumps({"run": number, "display": args.display, "seconds": round(time.monotonic() - started), **result}), flush=True)
    print(json.dumps({"series": args.display, "runs": args.runs, "complete": args.runs - bad, "incomplete": bad}), flush=True)
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main())
