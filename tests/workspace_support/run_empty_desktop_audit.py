"""Run the empty-first-use audit against an already extracted dev package.

No host install, credentials, network, seeded records, screenshots or model calls.
The package has its exact installed paths only inside a disposable container.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", required=True)
    parser.add_argument("--evidence", required=True)
    parser.add_argument("--fake", action="store_true")
    parser.add_argument("--stage", default="scan")
    args = parser.parse_args()
    package = Path(args.package).resolve()
    evidence = Path(args.evidence).resolve()
    evidence.parent.mkdir(parents=True, exist_ok=True)
    checkout = evidence.parent / "checkout"
    if not checkout.exists():
        shutil.copytree(package / "usr/lib/cortex-workspace-dev/runtime", checkout)
        shutil.copytree(ROOT / "tests", checkout / "tests", ignore=shutil.ignore_patterns("__pycache__"))
        (checkout / "packaging/container").mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / "packaging/container/check_rendered.py", checkout / "packaging/container/check_rendered.py")
    # Mount just sanitized runtime/test sources, never the worktree or its stores.
    command = ["docker", "run", "--rm", "--network=none", "--shm-size=512m",
               "--user", f"{os.getuid()}:{os.getgid()}", "--cap-drop=ALL",
               "--security-opt=seccomp=unconfined", "--security-opt=apparmor=unconfined"]
    for source, target, readonly in (
        (checkout, "/checkout", True),
        (package / "usr/lib/cortex-workspace-dev", "/usr/lib/cortex-workspace-dev", True),
        (package / "usr/bin/cortex-workspace-dev", "/usr/bin/cortex-workspace-dev", True),
        (package / "usr/bin/cortex-workspace-service-dev", "/usr/bin/cortex-workspace-service-dev", True),
        (ROOT / "tests/workspace_support/empty_desktop_audit.py", "/checkout/tests/workspace_support/empty_desktop_audit.py", True),
        (ROOT / "tests/workspace_support/discovery_only_mcp.py", "/checkout/tests/workspace_support/discovery_only_mcp.py", True),
        (evidence.parent, "/evidence", False),
    ):
        command += ["--mount", f"type=bind,source={source},target={target}" + (",readonly" if readonly else "")]
    command += ["--env", "HOME=/tmp", "cortex-workspace-rendered:ubuntu26.04",
                "python3", "-B", "/checkout/tests/workspace_support/empty_desktop_audit.py",
                "--package", "/", "--evidence", "/evidence/" + evidence.name,
                "--stage", args.stage, *(["--fake"] if args.fake else [])]
    return subprocess.run(command).returncode


if __name__ == "__main__":
    raise SystemExit(main())
