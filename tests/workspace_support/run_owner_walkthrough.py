"""Run the owner-scale no-paid walkthrough in the packaged desktop's container."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--package", required=True)
    p.add_argument("--evidence", required=True)
    p.add_argument("--clipboard", action="store_true", help="exercise native text selection and Ctrl+C")
    a = p.parse_args()
    package, evidence = Path(a.package).resolve(), Path(a.evidence).resolve()
    evidence.parent.mkdir(parents=True, exist_ok=True)
    # An exact command rerun gets a new observation directory, never overwrites
    # prior evidence, and always reads the currently selected package's runtime.
    requested = evidence
    number = 1
    while evidence.exists():
        evidence = requested.with_name(requested.name + f"-rerun-{number}")
        number += 1
    checkout = Path(tempfile.mkdtemp(prefix="runtime-", dir=evidence.parent)) / "checkout"
    shutil.copytree(package / "usr/lib/cortex-workspace-dev/runtime", checkout)
    (checkout / "packaging/container").mkdir(parents=True)
    shutil.copy2(ROOT / "packaging/container/check_rendered.py", checkout / "packaging/container/check_rendered.py")
    (checkout / "tests").mkdir()
    command = ["docker", "run", "--rm", "--network=none", "--shm-size=512m",
               "--user", f"{os.getuid()}:{os.getgid()}", "--cap-drop=ALL",
               "--security-opt=seccomp=unconfined", "--security-opt=apparmor=unconfined"]
    for source, target, readonly in (
        (checkout, "/checkout", True), (ROOT / "tests", "/checkout/tests", True),
        (package / "usr/lib/cortex-workspace-dev", "/usr/lib/cortex-workspace-dev", True),
        (package / "usr/bin/cortex-workspace-dev", "/usr/bin/cortex-workspace-dev", True),
        (package / "usr/bin/cortex-workspace-service-dev", "/usr/bin/cortex-workspace-service-dev", True),
        (evidence.parent, "/evidence", False),
    ):
        command += ["--mount", f"type=bind,source={source},target={target}" + (",readonly" if readonly else "")]
    command += ["--env", "HOME=/tmp", "cortex-workspace-rendered:ubuntu26.04",
                "python3", "-B", "/checkout/tests/workspace_support/" + ("clipboard_native.py" if a.clipboard else "owner_walkthrough.py"),
                "--evidence", "/evidence/" + evidence.name]
    return subprocess.run(command).returncode


if __name__ == "__main__":
    raise SystemExit(main())
