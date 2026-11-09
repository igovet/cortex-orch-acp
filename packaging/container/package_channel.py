#!/usr/bin/env python3
"""Channel-derived self-contained Debian layout and user-service lifecycle."""
import argparse
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from runtime_payload import payload_files
from debian_version import debian_version
from bundle_engines import validate_tree, validate_lock, digest
from bundle_voice import validate_tree as validate_voice

ICON_SIZES = (32, 48, 64, 128, 256, 512)


def channel(root, name):
    return json.loads((root / "crates/workspace-protocol/channels.json").read_text())[name]


def overlay(root, name):
    c = channel(root, name)
    return {"productName": c["title"], "identifier": c["identifier"]}


def package_version(identity, stamp, name):
    version = debian_version(identity, stamp)
    if name == "release":
        authorization = os.environ.get("CORTEX_WORKSPACE_RELEASE_AUTHORIZED")
        if authorization not in ("github-main-clean", "local-owner-test"):
            raise ValueError("release package requires explicit authorization")
        if authorization == "local-owner-test":
            # The unchanged GitHub publisher rejects this non-official version.
            version += ".local"
    return version


def package(root, target, output, name, depends=None, engines=None, *, stamp):
    # Installed modes must not follow the builder's umask: under 0077 dpkg-deb refuses
    # the 0700 control directory, and root-owned 0700 trees would be unreadable anyway.
    previous = os.umask(0o022)
    try:
        build_package(root, target, output, name, depends, engines, stamp=stamp)
    finally:
        os.umask(previous)


def build_package(root, target, output, name, depends, engines, *, stamp):
    c = channel(root, name)
    identity = json.loads((root / "package-identity.json").read_text())["version"]
    version = package_version(identity, stamp, name)
    dependencies = (depends or "libwebkit2gtk-4.1-0, libgtk-3-0, libc6, libgcc-s1")
    if "\n" in dependencies or re.search(r"\b[\w.+-]+-dev\b|\b(?:nodejs|npm)\b", dependencies):
        raise ValueError("invalid runtime dependencies")
    engines = engines or target.parent / "engines"
    validate_lock(root)
    manifest = validate_tree(engines)
    if manifest["lock_sha256"] != digest(root / "packaging/engines/package-lock.json"):
        raise ValueError("staged engine lock drift")
    voice = engines.parent / "voice"
    validate_voice(voice, root / "packaging/voice")
    output.mkdir(exist_ok=True)
    for source, binary in (("cortex-workspace-desktop", c["desktop_binary"]),
                            ("workspace-service", c["service_binary"])):
        shutil.copyfile(target / "release" / source, output / binary)
        (output / binary).chmod(0o755)
    with tempfile.TemporaryDirectory(prefix="channel-deb-") as temporary:
        stage = Path(temporary)
        stage.chmod(0o755)
        binaries = stage / "usr/bin"
        binaries.mkdir(parents=True)
        for name in (c["desktop_binary"], c["service_binary"]):
            shutil.copyfile(output / name, binaries / name)
            (binaries / name).chmod(0o755)
        desktop = stage / "usr/share/applications"
        desktop.mkdir(parents=True)
        # GNOME matches a Wayland window by its app id, which GTK takes from the identifier, so the
        # dock shows this icon and not a generic one. X11 matches the entry by the binary's name.
        (desktop / (c["package"] + ".desktop")).write_text(
            "[Desktop Entry]\nType=Application\nName=" + c["title"] +
            "\nExec=" + c["desktop_binary"] + "\nIcon=" + c["package"] +
            "\nStartupWMClass=" + c["identifier"] + "\nTerminal=false\nCategories=Development;\n")
        for size in ICON_SIZES:
            icons = stage / "usr/share/icons/hicolor" / (str(size) + "x" + str(size)) / "apps"
            icons.mkdir(parents=True)
            shutil.copyfile(root / "packaging/icons" / ("cortex-" + str(size) + ".png"), icons / (c["package"] + ".png"))
        units = stage / "usr/lib/systemd/user"
        units.mkdir(parents=True)
        shutil.copyfile(root / "packaging/systemd/user" / c["unit"], units / c["unit"])
        wants = units / "default.target.wants"
        wants.mkdir()
        (wants / c["unit"]).symlink_to("../" + c["unit"])
        runtime = stage / "usr/lib" / c["package"] / "runtime"
        for relative, source in payload_files(root):
            destination = runtime / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
            destination.chmod(0o755 if source.stat().st_mode & 0o111 else 0o644)
        voiced = runtime.parent / "voice"
        shutil.copytree(voice, voiced, symlinks=False)
        bundled = runtime.parent / "engines"
        shutil.copytree(engines, bundled, symlinks=True)
        for path in (voiced, *voiced.rglob("*"), bundled, *bundled.rglob("*")):
            if not path.is_symlink():
                path.chmod(0o755 if path.is_dir() or path.stat().st_mode & 0o111 else 0o644)
        data = stage / "usr/share" / c["package"]
        data.mkdir(parents=True)
        shutil.copyfile(root / "src/cortex_runtime/workspace_contract/schema.json", data / "schema.json")
        shutil.copyfile(root / "package-identity.json", data / "package-identity.json")
        # The fonts are compiled into the desktop binary; their OFL text ships beside it, readable.
        notices = stage / "usr/share/doc" / c["package"]
        notices.mkdir(parents=True)
        shutil.copyfile(root / "apps/cortex-workspace-desktop/public/licenses/fonts.html", notices / "fonts.html")
        control = stage / "DEBIAN"
        control.mkdir()
        for script in ("postinst", "prerm"):
            body = (root / "packaging/debian" / script).read_text().replace("@UNIT@", c["unit"])
            (control / script).write_text(body)
            (control / script).chmod(0o755)
        architecture = subprocess.check_output(["dpkg", "--print-architecture"], text=True).strip()
        (control / "control").write_text(
            "Package: " + c["package"] + "\nVersion: " + version + "\nArchitecture: " + architecture +
            "\nMaintainer: Cortex\nDepends: " + dependencies + ", python3 (>= 3.11), git, ca-certificates, bubblewrap, systemd, dbus-user-session, pulseaudio-utils, bzip2\n"
            "Description: " + c["title"] + " local desktop\n")
        subprocess.run(["dpkg-deb", "--root-owner-group", "--build", str(stage),
                        str(output / (c["package"] + "_" + version + "_" + architecture + ".deb"))],
                       env=dict(os.environ, SOURCE_DATE_EPOCH="0"), check=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("overlay", "package"))
    parser.add_argument("channel", choices=("dev", "release"))
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--target", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--depends-file", type=Path)
    parser.add_argument("--engines", type=Path)
    parser.add_argument("--build-stamp", help="explicit UTC YYYYMMDDhhmmssffffff ordering input")
    args = parser.parse_args()
    if args.action == "overlay":
        print(json.dumps(overlay(args.root, args.channel)))
    else:
        if args.build_stamp is None:
            parser.error("package requires --build-stamp")
        package(args.root, args.target, args.output, args.channel,
                args.depends_file.read_text().strip().removeprefix("shlibs:Depends=") if args.depends_file else None, engines=args.engines, stamp=args.build_stamp)


if __name__ == "__main__":
    main()
