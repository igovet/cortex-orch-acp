"""Run the rendered editor journey on owned source-only builds: no host settings, install, network or credentials."""
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / ".workspace-build/editor-native"
RECORDER = "#!/bin/sh\nfor word in \"$@\"; do printf '%s\\n' \"$word\"; done >> /evidence/opener-calls.txt\n"


def main():
    stage, name = sys.argv[1], sys.argv[2]
    run = OUT / ("run-" + name)
    run.mkdir(parents=True, exist_ok=False)
    stubs = run / "stubs"
    stubs.mkdir()
    (stubs / "recorder").write_text(RECORDER)
    (stubs / "recorder").chmod(0o755)
    (stubs / "absent").write_text("")
    (stubs / "absent").chmod(0o644)
    command = ["docker", "run", "--rm", "--network=none", "--shm-size=512m", "--user", f"{os.getuid()}:{os.getgid()}",
               "--cap-drop=ALL", "--security-opt=seccomp=unconfined", "--security-opt=apparmor=unconfined"]
    # A plain file with no execute bit stands in for a machine with no opener; the recorder for one that has it.
    openers = {"none": [("absent", "/usr/bin/gio")], "system": [("recorder", "/usr/bin/gio")],
               "journey": [("recorder", "/usr/bin/gio"), ("recorder", "/usr/local/bin/code")]}[stage]
    mounts = [(OUT / "work", "/checkout", True), (OUT / "pkg/usr/bin/cortex-workspace-dev", "/usr/bin/cortex-workspace-dev", True),
              (OUT / "pkg/usr/bin/cortex-workspace-service-dev", "/usr/bin/cortex-workspace-service-dev", True),
              (ROOT / "tests", "/checkout/tests", True), (run, "/evidence", False),
              *[(stubs / source, target, True) for source, target in openers]]
    for source, target, readonly in mounts:
        command += ["--mount", f"type=bind,source={source},target={target}" + (",readonly" if readonly else "")]
    command += ["--env", "HOME=/tmp", "cortex-workspace-rendered:ubuntu26.04", "python3", "-B",
                "/checkout/tests/workspace_support/editor_native.py", "--evidence", "/evidence/observations", "--stage", stage]
    return subprocess.run(command).returncode


if __name__ == "__main__":
    raise SystemExit(main())
