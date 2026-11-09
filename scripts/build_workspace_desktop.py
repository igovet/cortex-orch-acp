#!/usr/bin/env python3
"""Build the locked Ubuntu desktop without host development packages or credentials."""
import argparse
from datetime import datetime, timedelta, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "packaging/container"))
from debian_version import build_stamp
IMAGE = "cortex-workspace-builder:ubuntu26.04-rust1.99.1-node24.14.1"
TREES = (
    "apps/cortex-workspace-desktop", "crates/workspace-protocol",
    "crates/workspace-service", "packages/workspace-ui",
    "packages/workspace-domain", "packages/workspace-contract",
    "prototypes/cortex-workspace/desktop",
)
SUFFIXES = {".rs", ".toml", ".lock", ".json", ".ts", ".tsx", ".js", ".mjs", ".css", ".html", ".py", ".png", ".ico"}
SKIP = {"node_modules", "target", "dist", "__pycache__"}


def positive(value):
    number = int(value)
    if not 1 <= number <= 16:
        raise argparse.ArgumentTypeError("jobs must be between 1 and 16")
    return number


def build_id(value):
    if not re.fullmatch(r"build-[a-z0-9_]{8}", value):
        raise argparse.ArgumentTypeError("expected a local build ID")
    return value


def stamp_argument(value):
    try:
        return build_stamp(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError(str(error)) from None


def select_stamp(output_root, output, requested=None):
    """Reserve increasing inputs, including cache resumes; explicit inputs can replay."""
    if requested is not None:
        build_stamp(requested)
    saved = output / "build-stamp.txt"
    if saved.is_symlink():
        raise SystemExit("build stamp must not be a symlink")
    if saved.exists():
        prior = build_stamp(saved.read_text().strip())
        if requested == prior:
            return prior
    state = output_root / "last-build-stamp.txt"
    descriptor = os.open(state, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "r+") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        previous = stream.read().strip()
        if previous:
            build_stamp(previous)
        stamp = requested or datetime.now(timezone.utc).strftime("%Y%m%d%H%M%S%f")
        if previous and stamp <= previous:
            if requested is not None:
                raise SystemExit("new build stamp must exceed the previous build")
            stamp = (datetime.strptime(previous, "%Y%m%d%H%M%S%f") +
                     timedelta(microseconds=1)).strftime("%Y%m%d%H%M%S%f")
        stream.seek(0)
        stream.truncate()
        stream.write(stamp + "\n")
        stream.flush()
        os.fsync(stream.fileno())
        saved.write_text(stamp + "\n")
    return stamp


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--build-stamp", type=stamp_argument, help="UTC YYYYMMDDhhmmssffffff; explicit input, also for reproducible cache replay")
    channel = parser.add_mutually_exclusive_group()
    channel.add_argument("--release", action="store_true", help="GitHub manual workflow on clean main only")
    channel.add_argument("--local-release", action="store_true", help="non-official release channel for local owner testing only")
    channel.add_argument("--ci-dev", action="store_true", help="GitHub manual workflow on clean dev only")
    parser.add_argument("--engine", choices=("docker", "podman"), default="docker")
    parser.add_argument("--jobs", type=positive, default=2)
    parser.add_argument("--test-workspace", action="store_true", help="also run locked Cargo workspace tests in the container")
    parser.add_argument("--check-install", action="store_true", help="no-paid clean install/upgrade/removal smoke in a disposable container")
    parser.add_argument("--check-installed", action="store_true", help="required before owner handoff: real-input installed Debian acceptance on X11 and Wayland")
    parser.add_argument("--prior-archive", type=Path, help="exact earlier owner .deb for --check-installed plain apt upgrade")
    parser.add_argument("--resume", type=build_id, help="reuse a previous local build cache")
    args = parser.parse_args(argv)
    if args.check_installed and (args.prior_archive is None or args.engine != "docker" or args.release or args.local_release):
        parser.error("--check-installed currently requires Docker, dev channel and --prior-archive")
    if args.prior_archive is not None and not args.check_installed:
        parser.error("--prior-archive requires --check-installed")
    return args


def require_release(root, environ, channel="release"):
    branch_name = {"release": "main", "dev": "dev"}.get(channel)
    if (branch_name is None or environ.get("GITHUB_ACTIONS") != "true"
            or environ.get("GITHUB_REF") != "refs/heads/" + branch_name
            or environ.get("GITHUB_EVENT_NAME") != "workflow_dispatch"):
        raise SystemExit(f"{channel} requires a GitHub Actions workflow_dispatch on {branch_name}")
    branch = subprocess.check_output(["git", "-C", str(root), "branch", "--show-current"], text=True).strip()
    if branch != branch_name:
        raise SystemExit(f"{channel} requires current branch {branch_name}")
    if subprocess.check_output(["git", "-C", str(root), "status", "--porcelain"], text=True).strip():
        raise SystemExit(f"{channel} requires a clean tree")


def linked(root, path):
    return any(parent.is_symlink() for parent in (path, *path.parents)
               if parent != root and root in parent.parents)


def snapshot(root, destination):
    """Copy only build sources; never mount the checkout, home, Git or local config."""
    destination.mkdir(parents=True, exist_ok=True)
    for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml",
                 "src/cortex_runtime/workspace_contract/schema.json",
                 "packaging/container/package_channel.py", "packaging/container/runtime_payload.py",
                 "packaging/container/debian_version.py",
                 "packaging/container/bundle_engines.py", "packaging/container/bundle_voice.py",
                 "packaging/voice/package.json", "packaging/voice/package-lock.json",
                 "packaging/voice/voice.cjs", "packaging/voice/model.json",
                 "packaging/engines/package.json", "packaging/engines/package-lock.json",
                 "packaging/engines/node-checksums.json",
                 "packaging/debian/postinst", "packaging/debian/prerm",
                 "scripts/cortex_package.py", "scripts/generate_agent_profiles.py", "scripts/paseo_integrity.py",
                 "package-identity.json",
                 "packaging/systemd/user/cortex-workspace.service",
                 "packaging/systemd/user/cortex-workspace-dev.service",
                 *(f"packaging/icons/cortex-{size}.png" for size in (32, 48, 64, 128, 256, 512))):
        source = root / name
        if source.is_file() and not linked(root, source):
            target = destination / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
    for name in TREES:
        source = root / name
        if linked(root, source):
            continue
        for directory, dirs, files in os.walk(source, followlinks=False):
            dirs[:] = sorted(d for d in dirs if d not in SKIP and not d.startswith(".")
                             and not (Path(directory) / d).is_symlink())
            for file in sorted(files):
                path = Path(directory) / file
                if file.startswith(".") or path.is_symlink() or not path.is_file() or path.suffix not in SUFFIXES:
                    continue
                target = destination / path.relative_to(root)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, target)

    # Reuse the ACP installer\'s payload enumeration, including extensionless assets.
    if (root / "runtime-payload.json").is_file():
        import sys
        sys.path.insert(0, str(ROOT / "packaging/container"))
        from runtime_payload import payload_files
        for relative, path in payload_files(root):
            if linked(root, path):
                raise ValueError("linked runtime payload")
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)


def command_environment(output):
    home = output / "client-home"
    home.mkdir(parents=True, exist_ok=True)
    config = home / "docker"
    config.mkdir(exist_ok=True)
    # Do not inherit daemon endpoints, proxies, credential helpers or engine tokens.
    return {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(home), "DOCKER_CONFIG": str(config)}


def run_command(engine, output, jobs, workspace_tests=False, channel="dev", *, stamp, local_release=False):
    return [
        engine, "run", "--rm", "--cidfile", str(output / "container.cid"), "--user", f"{os.getuid()}:{os.getgid()}",
        "--cap-drop=ALL", "--security-opt=no-new-privileges",
        "--mount", f"type=bind,source={output / 'input'},target=/source,readonly",
        "--mount", f"type=bind,source={output},target=/build",
        "--env", f"CORTEX_WORKSPACE_BUILD_STAMP={build_stamp(stamp)}",
        "--env", "HOME=/build/home", "--env", f"CORTEX_WORKSPACE_CHANNEL={channel}",
        * (["--env", "CORTEX_WORKSPACE_RELEASE_AUTHORIZED=" + ("local-owner-test" if local_release else "github-main-clean")] if channel == "release" else []),
        IMAGE, str(jobs),
    ] + (["test"] if workspace_tests else [])


def main(argv=None):
    args = parse_args(argv)
    channel = "release" if args.release or args.local_release else "dev"
    if args.local_release and os.environ.get("GITHUB_ACTIONS") == "true":
        raise SystemExit("--local-release is for local owner testing, not GitHub Actions")
    if args.release or args.ci_dev:
        require_release(ROOT, os.environ, channel)
    output_root = ROOT / ".workspace-build"
    if output_root.is_symlink():
        raise SystemExit("build output must not be a symlink")
    output_root.mkdir(exist_ok=True)
    output = output_root / args.resume if args.resume else Path(tempfile.mkdtemp(prefix="build-", dir=output_root))
    if output.is_symlink() or not output.is_dir():
        raise SystemExit("build directory must be a local non-symlink directory")
    stamp = select_stamp(output_root, output, args.build_stamp)
    for name in ("manifest.json", "container.cid"):
        if (output / name).exists():
            (output / name).unlink()
    if (output / "artifacts").exists():
        shutil.rmtree(output / "artifacts")
    if (output / "input").exists():
        shutil.rmtree(output / "input")
    snapshot(ROOT, output / "input")
    # Only the reviewed recipe and Node checksums form the image build context.
    context = output / "context"
    context.mkdir(exist_ok=True)
    for name in ("Dockerfile", "build.sh", "node-checksums.json"):
        source = ROOT / ("packaging/engines" if name == "node-checksums.json" else "packaging/container") / name
        if source.is_symlink():
            raise SystemExit("container recipe must not be a symlink")
        shutil.copyfile(source, context / name)
    rust = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", rust):
        raise SystemExit("an exact Rust toolchain is required")
    env = command_environment(output)
    subprocess.run([args.engine, "build", "--build-arg", f"RUST_VERSION={rust}",
                    "--tag", IMAGE, str(context)], env=env, check=True)
    subprocess.run(run_command(args.engine, output, args.jobs, args.test_workspace, channel, stamp=stamp, local_release=args.local_release), env=env, check=True)
    if args.check_install:
        subprocess.run(["python3", "-B", str(ROOT / "packaging/container/check_install.py"),
                        "--engine", args.engine, "--artifacts", str(output / "artifacts"),
                        "--channel", channel], env=env, check=True)
    installed_acceptance = None
    if args.check_installed:
        archive, = (output / "artifacts").glob("*.deb")
        evidence = Path(tempfile.mkdtemp(prefix="installed-", dir=output))
        evidence.rmdir()  # The acceptance runner requires a new evidence path.
        subprocess.run(["python3", "-B", str(ROOT / "packaging/container/check_installed.py"),
                        "--archive", str(archive), "--prior-archive", str(args.prior_archive.resolve(strict=True)),
                        "--evidence", str(evidence)], env=env, check=True)
        installed_acceptance = json.loads((evidence / "result.json").read_text())
        if not installed_acceptance.get("accepted") or installed_acceptance.get("archive_sha256") != hashlib.sha256(archive.read_bytes()).hexdigest():
            raise SystemExit("installed acceptance did not bind to this archive")
    image = subprocess.check_output([args.engine, "image", "inspect", IMAGE, "--format", "{{.Id}}"],
                                    env=env, text=True).strip()
    artifacts = []
    for path in sorted((output / "artifacts").iterdir()):
        artifacts.append({"file": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                          "bytes": path.stat().st_size})
    manifest = {"build_stamp": stamp, "channel": channel, "image": image, "rust": rust, "artifacts": artifacts,
                "local_release": args.local_release,
                "owner_handoff_eligible": installed_acceptance is not None and not args.local_release,
                "installed_acceptance": installed_acceptance}
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"output": str(output.relative_to(ROOT)), **manifest}, indent=2))


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        raise SystemExit(f"container build failed (exit {error.returncode})") from None
