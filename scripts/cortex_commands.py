#!/usr/bin/env python3
"""Local owner control of running gateway commands; never launches a model."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys
import sqlite3
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from cortex_runtime.contracts import StoreError
from cortex_runtime.store import Store
from cortex_runtime.orchestration.command_control import list_commands, request_stop


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("list", "stop"):
        sub = commands.add_parser(name)
        sub.add_argument("--project-root", type=Path, required=True)
        sub.add_argument("--task-id", required=True)
        if name == "list":
            sub.add_argument("--assignment-ref")
        else:
            sub.add_argument("--tool-call-ref", required=True)
            sub.add_argument("--reason", required=True)
            sub.add_argument("--request-key", default=None)
    args = parser.parse_args(argv)
    project = args.project_root.resolve()
    try:
        # Inspection must not initialize or implicitly migrate an owner's store.
        store = Store(project / ".cortex", project_root=project, initialize=False)
        if args.command == "list":
            result = dict(commands=list_commands(store, args.task_id, assignment_ref=args.assignment_ref))
        else:
            result = request_stop(store, args.task_id, args.tool_call_ref, args.reason,
                                  args.request_key or str(uuid.uuid4()), origin="owner_cli")
    except (StoreError, OSError, sqlite3.Error) as error:
        print(json.dumps(dict(error=str(error) if isinstance(error, StoreError) else "command_control_storage_error",
                              message=error.correction if isinstance(error, StoreError) else "Project storage could not be accessed.")))
        return 1
    print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
