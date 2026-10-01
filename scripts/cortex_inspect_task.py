#!/usr/bin/env python3
"""Read one bounded, explicitly bound task metadata page; never initialize."""
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))
from cortex_runtime.storage.inspection import discover_binding, read_binding_page
from cortex_runtime.contracts import StoreError


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', required=True)
    parser.add_argument('--session', required=True)
    parser.add_argument('--discover', action='store_true', help='discover exact session/task/current owner metadata only')
    parser.add_argument('--task')
    parser.add_argument('--owner-epoch', type=int)
    parser.add_argument('--cursor')
    parser.add_argument('--limit', type=int, default=100)
    args = parser.parse_args()
    if args.discover:
        if args.task is not None or args.owner_epoch is not None or args.cursor is not None or args.limit != 100:
            parser.error('--discover cannot be combined with page arguments')
    elif args.task is None or args.owner_epoch is None:
        parser.error('--task and --owner-epoch are required for explicit page reads')
    try:
        result = (discover_binding(args.project, args.session) if args.discover else
                  read_binding_page(args.project, args.session, args.task, args.owner_epoch,
                                    cursor=args.cursor, limit=args.limit))
    except Exception as error:
        # No paths, exception text, private content or host logs on refusal.
        safe_codes = {'inspection_invalid_binding', 'inspection_binding_mismatch', 'unsafe_storage',
                      'inspection_uncheckpointed_wal', 'legacy_storage_migration_required', 'unsupported_storage',
                      'inspection_unsafe_parameter', 'inspection_evidence_overflow', 'inspection_snapshot_changed',
                      'inspection_invalid_cursor'}
        code = error.args[0] if isinstance(error, StoreError) and error.args else None
        if isinstance(error, BlockingIOError):
            code = 'inspection_busy'
        print(json.dumps({'available': False, 'error': code if code in safe_codes or code == 'inspection_busy' else 'inspection_refused'}))
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
