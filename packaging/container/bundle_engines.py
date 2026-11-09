#!/usr/bin/env python3
"""Build the offline engine tree from the reviewed npm lock and Node checksums."""
import argparse
import hashlib
import json
import platform
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

PACKAGES = {"codex-acp": "@agentclientprotocol/codex-acp",
            "claude-agent-acp": "@agentclientprotocol/claude-agent-acp"}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate_lock(root):
    source = root / "packaging/engines"
    lock = json.loads((source / "package-lock.json").read_text())
    package = json.loads((source / "package.json").read_text())
    pins = json.loads((root / "pinned-dependencies.json").read_text())["engines"]
    expected = {name: pins[engine]["pinned_package_version"] for engine, name in PACKAGES.items()}
    expected["@openai/codex"] = pins["codex-acp"]["bundled_dependency_version"]
    if package["dependencies"] != expected or lock["packages"][""]["dependencies"] != expected:
        raise ValueError("engine lock differs from pinned-dependencies.json")
    for name, version in expected.items():
        if lock["packages"]["node_modules/" + name]["version"] != version:
            raise ValueError("engine lock version drift")
    for name, entry in lock["packages"].items():
        if name and (not entry.get("integrity", "").startswith("sha512-")
                     or not entry.get("resolved", "").startswith("https://registry.npmjs.org/")):
            raise ValueError("unlocked engine dependency")
    return lock


def validate_tree(tree):
    """Fail closed on missing/modified Node, adapters or any installed dependency file."""
    manifest = json.loads((tree / "bundle-manifest.json").read_text())
    for relative, expected in manifest["files"].items():
        path = tree / relative
        if not path.is_file() or digest(path) != expected:
            raise ValueError("incomplete engine bundle: " + relative)
    for relative in manifest["required"]:
        if not (tree / relative).is_file():
            raise ValueError("missing required engine: " + relative)
    if not (tree / "node/bin/node").stat().st_mode & 0o111:
        raise ValueError("bundled Node is not executable")
    return manifest


def build(root, output):
    lock = validate_lock(root)
    output.mkdir(parents=True, exist_ok=False)
    for name in ("package.json", "package-lock.json"):
        shutil.copyfile(root / "packaging/engines" / name, output / name)
    subprocess.run(["npm", "ci", "--prefix", str(output), "--omit=dev", "--ignore-scripts",
                    "--no-audit", "--no-fund"], check=True, timeout=600)
    spec = json.loads((root / "packaging/engines/node-checksums.json").read_text())
    arch = {"x86_64": "x64", "aarch64": "arm64"}[platform.machine()]
    if subprocess.check_output(["node", "--version"], text=True).strip() != "v" + spec["version"]:
        raise ValueError("builder Node differs from bundled Node")
    archive_name = "node-v" + spec["version"] + "-linux-" + arch
    with tempfile.TemporaryDirectory() as temporary:
        archive = Path(temporary) / "node.tar.xz"
        urllib.request.urlretrieve("https://nodejs.org/dist/v" + spec["version"] + "/" + archive_name + ".tar.xz", archive)
        if digest(archive) != spec["sha256"][arch]:
            raise ValueError("Node checksum mismatch")
        with tarfile.open(archive) as tar:
            for relative in ("bin/node", "LICENSE"):
                member = tar.getmember(archive_name + "/" + relative)
                destination = output / "node" / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as stream:
                    destination.write_bytes(stream.read())
        (output / "node/bin/node").chmod(0o755)
    if digest(output / "node/bin/node") != digest(Path(shutil.which("node")).resolve()):
        raise ValueError("builder Node bytes differ from verified distribution")
    required = ["node/bin/node", "node/LICENSE"]
    for name in PACKAGES.values():
        package_dir = output / "node_modules" / name
        data = json.loads((package_dir / "package.json").read_text())
        entry = data["bin"]
        entry = entry if isinstance(entry, str) else next(iter(entry.values()))
        required.extend([str((package_dir / "package.json").relative_to(output)),
                         str((package_dir / entry).relative_to(output))])
    triple = {"x64": "x86_64", "arm64": "aarch64"}[arch] + "-unknown-linux-musl"
    required.append("node_modules/@openai/codex-linux-" + arch + "/vendor/" + triple + "/bin/codex")
    required.append("node_modules/@anthropic-ai/claude-agent-sdk-linux-" + arch + "/claude")
    # All installed package identities must match the lock, including optional native parts.
    installed = []
    for relative, entry in lock["packages"].items():
        if not relative:
            continue
        package_json = output / relative / "package.json"
        if not package_json.exists():
            if entry.get("optional"):
                continue
            raise ValueError("missing dependency: " + relative)
        data = json.loads(package_json.read_text())
        if data["version"] != entry["version"]:
            raise ValueError("dependency version drift: " + relative)
        installed.append(dict(name=relative.split("node_modules/")[-1], version=entry["version"],
                              license=entry.get("license", "UNDECLARED")))
    manifest = dict(node_version=spec["version"], architecture=arch, required=required,
                    packages=installed, lock_sha256=digest(output / "package-lock.json"),
                    files={str(p.relative_to(output)): digest(p) for p in sorted(output.rglob("*"))
                           if p.is_file() and not p.is_symlink()})
    (output / "bundle-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    validate_tree(output)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    build(args.root, args.output)
