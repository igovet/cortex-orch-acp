"""Availability only, inside the check runner's real sandbox. Never a pass receipt."""
import os
import subprocess
import tempfile
from pathlib import Path

from ..contracts import StoreError

GUIDANCE = (
    "The check program is unavailable in its execution environment. Install the required tool "
    "in an isolated project-private tools directory (for example .venv), without sudo or "
    "system changes, or choose another executable check. Probe again before depending on it. "
    "If neither is possible, ask the owner now through ask_question (workers) or "
    "user_question_request (coordinator); do not defer the decision until the result."
)


def probe(root, argv):
    from .controller import _verification_command
    # Fixed shell source, literal positional arguments: never run a proposed check.
    # Only PATH affects lookup. Never launch a caller-supplied env binary or
    # apply loader/startup variables to the fixed probe process.
    remaining = list(argv)
    wrapper, path_override = "", None
    if remaining and Path(remaining[0]).name == "env":
        wrapper = remaining.pop(0)
        while remaining and "=" in remaining[0] and not remaining[0].startswith("-"):
            name, value = remaining.pop(0).split("=", 1)
            if name == "PATH":
                path_override = value
    programs = []
    while remaining and Path(remaining[0]).name == "timeout":
        programs.append(remaining.pop(0))
        while remaining and remaining[0].startswith("-"):
            flag = remaining.pop(0)
            if flag in ("-s", "--signal", "-k", "--kill-after") and remaining:
                remaining.pop(0)
        if remaining:
            remaining.pop(0)  # duration
    if not remaining:
        return dict(available=False, reason="command_missing",
                    message="No check program was declared. Supply an executable command.")
    programs.extend(remaining[:1])
    source = ('available() { case "$1" in */*) [ -f "$1" ] && [ -x "$1" ]; return;; esac; '
              'rest=$PATH; while :; do directory=${rest%%:*}; '
              'candidate=${directory:-.}/$1; [ -f "$candidate" ] && [ -x "$candidate" ] && return 0; '
              'case "$rest" in *:*) rest=${rest#*:};; *) break;; esac; done; return 1; }; '
              'wrapper=$1; override=$2; lookup_path=$3; shift 3; '
              'if [ -n "$wrapper" ]; then available "$wrapper" || exit 42; fi; '
              'if [ "$override" = yes ]; then PATH=$lookup_path; export PATH; fi; '
              'for program do available "$program" || exit 42; done')
    command = ["/bin/sh", "-c", source, "cortex-check-preflight", wrapper,
               "yes" if path_override is not None else "no", path_override or "", *programs]
    try:
        with tempfile.TemporaryDirectory(prefix="cortex-check-preflight-") as scratch:
            with _verification_command(Path(root), scratch, command) as (launch, fds):
                from .sandbox import compact_argv
                command, launch_fds = compact_argv(launch, fds)
                try:
                    done = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                          stderr=subprocess.DEVNULL, pass_fds=launch_fds, timeout=10,
                                          cwd=str(root), env={"PATH": "/usr/bin:/bin"})
                finally:
                    for fd in set(launch_fds) - set(fds):
                        os.close(fd)
        # Only the fixed lookup shell's sentinel establishes absence; a failed
        # sandbox/launcher is not evidence that the proposed program is missing.
        reason = None if done.returncode == 0 else "program_missing" if done.returncode == 42 else "probe_unavailable"
    except (OSError, StoreError, subprocess.TimeoutExpired):
        reason = "probe_unavailable"
    message = ("Check program available; the check has not run." if reason is None else
               "The verification environment could not complete the availability probe. "
               "Restore the verification sandbox and probe again; this does not establish a missing program."
               if reason == "probe_unavailable" else GUIDANCE)
    return dict(available=reason is None, reason=reason, message=message)


def require_executable(root, commands):
    for index, argv in enumerate(commands):
        result = probe(root, argv)
        if not result["available"]:
            code = "check_preflight_unavailable" if result.get("reason") == "probe_unavailable" else "check_executable_unavailable"
            raise StoreError(code, f"verification[{index}]",
                             result.get("reason"), "an executable check", result["message"])
