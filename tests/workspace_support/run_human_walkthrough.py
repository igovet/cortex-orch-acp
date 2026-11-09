"""Install the exact dev archive offline, then run the screenshot walkthrough."""
import argparse
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--archive", type=Path, required=True)
    p.add_argument("--evidence", type=Path, required=True)
    a = p.parse_args()
    archive, evidence = a.archive.resolve(strict=True), a.evidence.resolve()
    evidence.mkdir(parents=True, exist_ok=False)
    # Only disposable container state is installed/changed; no host home or display.
    command = ["docker", "run", "--rm", "--network=none", "--shm-size=512m",
               "--cap-drop=ALL", "--cap-add=CHOWN", "--cap-add=DAC_OVERRIDE",
               "--cap-add=SETUID", "--cap-add=SETGID", "--cap-add=FOWNER",
               "--security-opt=seccomp=unconfined", "--security-opt=apparmor=unconfined"]
    for source, target, ro in (
        (archive, "/candidate.deb", True),
        (ROOT / "tests", "/fixture-tests", True),
        (ROOT / "packaging/container/check_rendered.py", "/render-probe.py", True),
        (evidence, "/evidence", False),
    ):
        command += ["--mount", f"type=bind,source={source},target={target}" + (",readonly" if ro else "")]
    script = """set -eu
export PATH=/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
apt install -y /candidate.deb
cp -a /usr/lib/cortex-workspace-dev/runtime /checkout
cp -a /fixture-tests /checkout/tests
mkdir -p /checkout/packaging/container
cp /render-probe.py /checkout/packaging/container/check_rendered.py
python3 -c 'from pathlib import Path; p=Path("/usr/bin/code"); assert not p.exists(); p.write_text("#!/usr/bin/python3\\nimport json,sys\\nfrom pathlib import Path\\nPath(\\"/tmp/editor-called.json\\").write_text(json.dumps(sys.argv[1:]))\\n"); p.chmod(0o755)'
useradd --uid 1001 --create-home --shell /bin/sh walker
chown -R 1001:1001 /checkout/tests /checkout/packaging /evidence
runuser -u walker -- env HOME=/home/walker python3 -B /checkout/tests/workspace_support/human_walkthrough.py --evidence /evidence/screens
"""
    command += ["cortex-workspace-rendered:ubuntu26.04", "/bin/sh", "-c", script]
    return subprocess.run(command, timeout=900).returncode


if __name__ == "__main__":
    raise SystemExit(main())
