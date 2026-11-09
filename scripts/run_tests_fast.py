#!/usr/bin/env python3
"""Bounded no-paid Python suite; no arguments or writes to tracked files needed."""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BATCH_SIZE = 32
WORKERS = 8
TEST_SECONDS = 60
BATCH_SECONDS = 180
BUILD_SECONDS = 300
# Every exclusion needs a concrete reason and the --slow command below.
SLOW_FILES: dict[str, str] = {}


def test_files(root: Path) -> list[str]:
    return sorted(str(p.relative_to(root)) for p in (root / "tests").rglob("*.py")
                  if p.name.startswith("test_") or p.name.endswith("_test.py"))


def selection(root: Path, slow: bool = False) -> list[str]:
    files = test_files(root)
    stale = set(SLOW_FILES) - set(files)
    if stale or any(not reason.strip() for reason in SLOW_FILES.values()):
        raise ValueError(f"Invalid slow-file inventory: {sorted(stale)}")
    return [p for p in files if (p in SLOW_FILES) == slow]


def batches(files: list[str]) -> list[list[str]]:
    return [files[i:i + BATCH_SIZE] for i in range(0, len(files), BATCH_SIZE)]


def pytest_command(root: Path, files: list[str], workers: int = WORKERS) -> list[str]:
    if not files or len(files) > BATCH_SIZE:
        raise ValueError("pytest requires an explicit small file batch")
    return ["timeout", "--kill-after=5", str(BATCH_SECONDS),
            str(root / ".venv/bin/python"), "-B", "-m", "pytest", "-q",
            "-n", str(workers), "--dist=loadgroup", "--max-worker-restart=0",
            f"--timeout={TEST_SECONDS}", "--timeout-method=thread",
            "-p", "no:cacheprovider", "--durations=5", *files]


def cargo_program(env: dict[str, str]) -> str | None:
    """rustup installs below the account home, which many launchers leave off PATH."""
    found = shutil.which("cargo", path=env.get("PATH"))
    if found:
        return found
    home = env.get("CARGO_HOME") or str(Path(env.get("HOME", "/nonexistent")) / ".cargo")
    return shutil.which(str(Path(home) / "bin/cargo"))


def build_service(root: Path, env: dict[str, str]) -> Path | None:
    """Bring the service the suite launches up to date with the current sources.

    Service tests start this real binary. An older build runs the old code and
    fails or passes for the wrong reason, so the suite never reuses one unbuilt.
    Run before the account roots change: cargo needs the real toolchain home. The
    target directory and channel are pinned so an inherited setting cannot select
    a different binary or the release store.
    """
    cargo = cargo_program(env)
    if cargo is None:
        print("Requires cargo to build the workspace service under test.", file=sys.stderr)
        return None
    build_env = dict(env, CARGO_TARGET_DIR=str(root / "target"), CORTEX_WORKSPACE_CHANNEL="dev")
    build_env["PATH"] = os.pathsep.join(filter(None, (str(Path(cargo).parent), env.get("PATH"))))
    build_env.pop("CORTEX_WORKSPACE_RELEASE_AUTHORIZED", None)
    built = subprocess.run(["timeout", "--kill-after=5", str(BUILD_SECONDS), cargo, "build", "--locked",
                            "-p", "workspace-service"], cwd=root, env=build_env)
    binary = root / "target/debug/workspace-service"
    if built.returncode or not binary.is_file():
        print(f"The workspace service did not build (exit {built.returncode}).", file=sys.stderr)
        return None
    return binary


def run_batches(root: Path, files: list[str], env: dict[str, str]) -> int:
    failed = False
    start = time.monotonic()
    for index, batch in enumerate(batches(files), 1):
        print(f"Batch {index}: {', '.join(batch)}", flush=True)
        tick = time.monotonic()
        result = subprocess.run(pytest_command(root, batch), cwd=root, env=env)
        print(f"Batch {index}: exit={result.returncode}, {time.monotonic() - tick:.2f}s",
              flush=True)
        failed |= result.returncode != 0
    print(f"Checked {len(files)} files in {time.monotonic() - start:.2f}s", flush=True)
    return int(failed)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--slow", action="store_true", help="run only explicitly listed slow files")
    args = parser.parse_args()
    python = ROOT / ".venv/bin/python"
    if not python.is_file() or not shutil.which("timeout"):
        print("Requires .venv/bin/python and GNU timeout.", file=sys.stderr)
        return 2
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", PYTEST_ADDOPTS="")
    # Neither inherited plugin-disable flags nor optional user addopts may silently
    # turn off the deadline or change the selection in this fixed entry point.
    env.pop("PYTEST_DISABLE_PLUGIN_AUTOLOAD", None)
    check = subprocess.run(["timeout", "--kill-after=5", "15", str(python), "-B", "-c",
                            "import pytest, xdist, pytest_timeout"], cwd=ROOT, env=env)
    if check.returncode:
        print("Install checkout-local test dependencies first: "
              "uv pip install --python .venv/bin/python pytest-xdist==3.8.0 "
              "pytest-timeout==2.4.0", file=sys.stderr)
        return 2
    files = selection(ROOT, args.slow)
    if not files:
        print("No explicitly listed slow files." if args.slow else "No test files found.")
        return 0 if args.slow else 2
    service = build_service(ROOT, env)
    if service is None:
        return 2
    env["WORKSPACE_TEST_SERVICE_BINARY"] = str(service)
    # Fresh account roots outside Git, also usable when tracked files are read-only.
    # conftest owns each worker's short socket-safe TMPDIR below /tmp.
    with tempfile.TemporaryDirectory(prefix="cortex-fast-", dir="/tmp") as scratch:
        for key, name in (("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                          ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache")):
            path = Path(scratch) / name
            path.mkdir(mode=0o700)
            env[key] = str(path)
        env["TMPDIR"] = "/tmp"
        return run_batches(ROOT, files, env)


if __name__ == "__main__":
    raise SystemExit(main())
