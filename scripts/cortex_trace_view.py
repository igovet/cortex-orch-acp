#!/usr/bin/env python3
"""Read existing dev wire JSONL; no Store, daemon, login or write operations."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import stat
import sys


def records(directory):
    if directory.resolve() != directory or any((p / ".git").exists() or p.name == ".cortex"
                                                for p in (directory, *directory.parents)):
        raise ValueError("unsafe_trace_directory")
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        info = os.fstat(fd)
        if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
            raise ValueError("unsafe_trace_directory")
        # Oldest surviving rotation first. A concurrent rotation may yield
        # a missing/duplicated generation; stop the writer for exact analysis.
        for name in ("trace.3.jsonl", "trace.2.jsonl", "trace.1.jsonl", "trace.jsonl"):
            try:
                child = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
                                | os.O_CLOEXEC, dir_fd=fd)
            except FileNotFoundError:
                continue
            with os.fdopen(child, "r", encoding="utf-8") as stream:
                info = os.fstat(stream.fileno())
                if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                        or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600):
                    raise ValueError("unsafe_trace_file")
                for line in stream:
                    record = json.loads(line)
                    if not isinstance(record, dict):
                        raise ValueError("invalid_trace_record")
                    yield record
    finally:
        os.close(fd)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=Path.home() / ".paseo-dev/cortex-xdg/state/cortex/wire-traces")
    parser.add_argument("--task")
    parser.add_argument("--assignment")
    parser.add_argument("--direction", choices=("in", "out", "internal"))
    parser.add_argument("--channel", choices=("upstream_acp", "downstream_acp", "mcp", "verification", "lifecycle"))
    parser.add_argument("--pretty", action="store_true")
    args = parser.parse_args(argv)
    try:
        for row in records(args.directory):
            if any(value is not None and row.get(key) != value for key, value in (
                    ("task_id", args.task), ("assignment_ref", args.assignment),
                    ("direction", args.direction), ("channel", args.channel))):
                continue
            print(json.dumps(row, ensure_ascii=True, indent=2 if args.pretty else None))
    except (OSError, ValueError) as error:
        print(f"cortex_trace_view: cannot read complete trace ({type(error).__name__})", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
