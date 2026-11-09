#!/usr/bin/env python3
"""Check reviewed Rust/CLI inputs without building native code or using the network.

After an intentional Cargo update, review Cargo.lock and update its SHA-256 in
workspace.metadata.cortex-pins alongside any changed direct dependency pins.
"""
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib

VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
DESKTOP = Path("apps/cortex-workspace-desktop")
APP = (DESKTOP / "src-tauri").as_posix()
DESKTOPS = (DESKTOP, Path("prototypes/cortex-workspace/desktop"))
EDITOR = Path("packages/workspace-ui")
EDITOR_SCOPES = ("@codemirror/", "@lezer/")


def read_toml(path):
    return tomllib.loads(path.read_text())


def dependency_tables(manifest):
    for key in ("dependencies", "dev-dependencies", "build-dependencies"):
        yield manifest.get(key, {})
    for target in manifest.get("target", {}).values():
        yield from dependency_tables(target)


def check_editor(root):
    """The editor's direct packages are exactly the reviewed versions, in the manifest and in the lock."""
    errors = []
    reviewed = json.loads((root / "pinned-dependencies.json").read_text())["workspace_editor"]
    package = json.loads((root / EDITOR / "package.json").read_text())["dependencies"]
    lock = json.loads((root / EDITOR / "package-lock.json").read_text())["packages"]
    direct = {name: version for name, version in package.items() if name.startswith(EDITOR_SCOPES)}
    if not reviewed or direct != reviewed:
        errors.append(f"{EDITOR}: editor packages differ from the reviewed exact pins")
    for name, version in direct.items():
        if not VERSION.fullmatch(version):
            errors.append(f"{name}: editor package requires an exact x.y.z pin")
        if lock[""]["dependencies"].get(name) != version or lock.get("node_modules/" + name, {}).get("version") != version:
            errors.append(f"{name}: editor package-lock drift")
    return errors


def check(root):
    """Return actionable, value-free diagnostics for missing or drifting inputs."""
    errors = []
    try:
        workspace = read_toml(root / "Cargo.toml")["workspace"]
        pins = workspace["metadata"]["cortex-pins"]
        channel = read_toml(root / "rust-toolchain.toml")["toolchain"]["channel"]
        if not VERSION.fullmatch(channel) or channel != pins["rust"]:
            errors.append("Rust channel must be the reviewed exact stable version")
        if workspace["package"]["rust-version"] != channel:
            errors.append("workspace rust-version differs from the toolchain")
        lock_bytes = (root / "Cargo.lock").read_bytes()
        lock = tomllib.loads(lock_bytes.decode())
        if hashlib.sha256(lock_bytes).hexdigest() != pins["lock-sha256"]:
            errors.append("Cargo.lock digest drift; review and refresh cortex-pins.lock-sha256")
        locked = {(p["name"], p["version"]) for p in lock["package"]}
        members = workspace["members"]
        defaults = workspace["default-members"]
        if APP not in members or not defaults or APP in defaults:
            errors.append("Tauri must be a workspace member but not a default member")
        if any(member not in members or not member.startswith("crates/") for member in defaults):
            errors.append("default members must explicitly select headless crates")
        manifests = [(root / "Cargo.toml", {"dependencies": workspace.get("dependencies", {})})]
        for member in members:
            manifests.append((root / member / "Cargo.toml", read_toml(root / member / "Cargo.toml")))
        for path, manifest in manifests:
            for dependencies in dependency_tables(manifest):
                for name, spec in dependencies.items():
                    if isinstance(spec, dict) and (spec.get("workspace") or "path" in spec):
                        continue
                    version = spec if isinstance(spec, str) else spec.get("version", "")
                    if not version.startswith("=") or not VERSION.fullmatch(version[1:]):
                        errors.append(f"{path.relative_to(root)}: {name} requires an exact =x.y.z crate pin")
                        continue
                    package = spec.get("package", name) if isinstance(spec, dict) else name
                    if (package, version[1:]) not in locked:
                        errors.append(f"{package}: exact manifest pin missing from Cargo.lock")
                    if package in pins and version[1:] != pins[package]:
                        errors.append(f"{package}: manifest differs from reviewed pin")
        for desktop in DESKTOPS:
            package = json.loads((root / desktop / "package.json").read_text())
            npm_lock = json.loads((root / desktop / "package-lock.json").read_text())["packages"]
            cli = package["devDependencies"]["@tauri-apps/cli"]
            if not VERSION.fullmatch(cli) or cli != pins["tauri-cli"]:
                errors.append(f"{desktop}: Tauri CLI differs from the reviewed exact pin")
            if (npm_lock[""]["devDependencies"]["@tauri-apps/cli"] != cli
                    or npm_lock["node_modules/@tauri-apps/cli"]["version"] != cli):
                errors.append(f"{desktop}: Tauri CLI package-lock drift")
        for package in ("tauri", "tauri-build"):
            if (package, pins[package]) not in locked:
                errors.append(f"{package}: reviewed pin missing from Cargo.lock")
        errors += check_editor(root)
    except (OSError, ValueError, KeyError, TypeError) as error:
        errors.append(f"missing or malformed pin input ({type(error).__name__})")
    return errors


def main():
    errors = check(Path(__file__).resolve().parents[1])
    for error in errors:
        print(error, file=sys.stderr)
    if not errors:
        print("Workspace pins OK")
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
