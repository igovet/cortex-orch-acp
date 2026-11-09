#!/usr/bin/env python3
"""Build the on-device recognizer from its locked npm tree, never at first launch."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate_tree(tree, source):
    manifest = json.loads((tree / "bundle-manifest.json").read_text())
    if manifest["lock_sha256"] != digest(source / "package-lock.json"):
        raise ValueError("voice lock drift")
    for name, expected in manifest["files"].items():
        file = tree / name
        if file.is_symlink() or not file.is_file() or digest(file) != expected:
            raise ValueError("incomplete voice bundle")
    for name in ("voice.cjs", "model.json", "package.json"):
        if manifest["files"].get(name) != digest(source / name):
            raise ValueError("staged voice source drift")
    for name in ("voice.cjs", "model.json", "node_modules/sherpa-onnx-node/sherpa-onnx.js"):
        if name not in manifest["files"]:
            raise ValueError("missing voice runtime")
    for name in ("sherpa-onnx.node", "libonnxruntime.so", "libsherpa-onnx-c-api.so"):
        if not any(p.endswith("/" + name) for p in manifest["files"]):
            raise ValueError("missing native voice runtime")
    return manifest


def build(root, output):
    source = root / "packaging/voice"
    lock = json.loads((source / "package-lock.json").read_text())
    if lock["packages"][""]["dependencies"] != {"sherpa-onnx-node": "2.0.1"}:
        raise ValueError("voice version drift")
    for name, item in lock["packages"].items():
        if name and (not item.get("integrity", "").startswith("sha512-")
                     or not item.get("resolved", "").startswith("https://registry.npmjs.org/")):
            raise ValueError("unlocked voice dependency")
    output.mkdir(parents=True, exist_ok=False)
    for name in ("package.json", "package-lock.json", "voice.cjs", "model.json", "NOTICE.md", "LICENSE-APACHE-2.0", "LICENSE-ONNXRUNTIME"):
        shutil.copyfile(source / name, output / name)
    subprocess.run(["npm", "ci", "--prefix", str(output), "--omit=dev", "--ignore-scripts",
                    "--no-audit", "--no-fund"], check=True, timeout=180)
    subprocess.run(["node", "-e", "require('sherpa-onnx-node')"], cwd=output, check=True, timeout=30)
    manifest = {"lock_sha256": digest(source / "package-lock.json"),
                "files": {str(p.relative_to(output)): digest(p) for p in sorted(output.rglob("*")) if p.is_file()}}
    (output / "bundle-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    validate_tree(output, source)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    build(args.root, args.output)
