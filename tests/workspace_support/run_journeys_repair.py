"""No-paid packaged desktop journeys, in a disposable no-network container."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--package", required=True)
    p.add_argument("--evidence", required=True)
    p.add_argument("--stage", required=True)
    p.add_argument("--finish", action="store_true", help="run the remaining-journey fixture")
    args = p.parse_args()
    package, evidence = Path(args.package).resolve(), Path(args.evidence).resolve()
    evidence.parent.mkdir(parents=True, exist_ok=True)
    checkout = evidence.parent / "runtime"
    if not checkout.exists():
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
                "python3", "-B", "/checkout/tests/workspace_support/" + ("journeys_finish_native.py" if args.finish else "journeys_repair_native.py"),
                "--evidence", "/evidence/" + evidence.name, "--stage", args.stage]
    return subprocess.run(command).returncode


if __name__ == "__main__":
    raise SystemExit(main())
