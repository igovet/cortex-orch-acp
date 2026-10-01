#!/usr/bin/env python3
"""Create a read-consistent persistent dev clone of a Paseo home."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import sys
import tempfile
import time


RUNTIME_NAMES = {"daemon.pid", "daemon.lock", "daemon.sock", "paseo.sock"}
RUNTIME_SUFFIXES = {".pid", ".lock", ".sock"}
# Per-home daemon identity (Paseo 0.10.2 loads or creates both on start). Copying
# production's would give the dev daemon the same serverId and relay keypair.
IDENTITY_NAMES = {"server-id", "daemon-keypair.json"}


def resolved(value: str) -> Path:
    path = Path(value).expanduser()
    if not path.is_absolute():
        raise ValueError("paths must be absolute")
    return path.resolve(strict=False)


def skip(path: Path) -> bool:
    return (
        path.name in RUNTIME_NAMES
        or path.suffix in RUNTIME_SUFFIXES
        or stat.S_ISSOCK(path.lstat().st_mode)
    )


def is_sqlite(path: Path) -> bool:
    try:
        return path.read_bytes()[:16] == b"SQLite format 3\x00"
    except OSError:
        return False


def copy_sqlite(source: Path, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    source_uri = f"{source.as_uri()}?mode=ro"
    with sqlite3.connect(source_uri, uri=True) as origin, sqlite3.connect(target) as clone:
        origin.backup(clone)


def clone_home(source: Path, target: Path) -> None:
    stage = Path(tempfile.mkdtemp(prefix=f".{target.name}.refresh-", dir=target.parent))
    try:
        for root, directories, files in os.walk(source):
            root_path = Path(root)
            relative = root_path.relative_to(source)
            destination_root = stage / relative
            destination_root.mkdir(parents=True, exist_ok=True)
            retained_directories = []
            for name in directories:
                candidate = root_path / name
                if not skip(candidate):
                    retained_directories.append(name)
                    (destination_root / name).mkdir(exist_ok=True)
            directories[:] = retained_directories
            for name in files:
                candidate = root_path / name
                if skip(candidate) or (relative == Path(".") and name in IDENTITY_NAMES):
                    continue
                destination = destination_root / name
                if is_sqlite(candidate):
                    copy_sqlite(candidate, destination)
                else:
                    shutil.copy2(candidate, destination, follow_symlinks=False)
        # Keep the dev home's own identity; never adopt production's.
        for name in IDENTITY_NAMES:
            own = target / name
            if (own.is_file() and not own.is_symlink()
                    and not ((source / name).is_file() and own.read_bytes() == (source / name).read_bytes())):
                shutil.copy2(own, stage / name, follow_symlinks=False)
        if target.exists():
            previous = target.with_name(f"{target.name}.previous-{int(time.time())}")
            target.rename(previous)
        stage.rename(target)
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", default=str(Path.home() / ".paseo"))
    parser.add_argument("--target", default=str(Path.home() / ".paseo-dev"))
    parser.add_argument("--check", action="store_true", help="validate paths without copying")
    args = parser.parse_args(argv)
    try:
        source = resolved(args.source)
        target = resolved(args.target)
    except ValueError as error:
        print(f"cortex_paseo_clone: {error}", file=sys.stderr)
        return 2
    production = (Path.home() / ".paseo").resolve(strict=False)
    default_dev = (Path.home() / ".paseo-dev").resolve(strict=False)
    if source != production:
        print("cortex_paseo_clone: source must be the production Paseo home", file=sys.stderr)
        return 2
    if target != default_dev:
        print("cortex_paseo_clone: target must be ~/.paseo-dev", file=sys.stderr)
        return 2
    if not source.is_dir():
        print("cortex_paseo_clone: production Paseo home is missing", file=sys.stderr)
        return 2
    if args.check:
        print(f'{{"source":"{source}","target":"{target}"}}')
        return 0
    try:
        clone_home(source, target)
    except (OSError, RuntimeError, sqlite3.Error) as error:
        print(f"cortex_paseo_clone: {error}", file=sys.stderr)
        return 1
    print(f'{{"source":"{source}","target":"{target}"}}')
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
