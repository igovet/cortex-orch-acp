#!/usr/bin/env python3
"""Delete this project's tasks older than a user-selected activity retention."""
import argparse
import json
import os
from datetime import datetime, timedelta, timezone
from cortex_runtime.cleanup import clear_tasks
from cortex_runtime.project_storage import canonical_project, project_store_directory
from cortex_runtime.store import Store


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--project-root',required=True)
    p.add_argument('--days',type=int,required=True)
    p.add_argument('--keep-thread',action='append',default=[],help='Protect tasks linked to known active native threads.')
    p.add_argument('--force',action='store_true',help='Assume stale processes are dead; mark live-state records released, but never override --keep-thread. CLI already ignores active protection. Does not kill OS processes.')
    p.add_argument('--dry-run',action='store_true',help='Preview counts without deletion or VACUUM.')
    args=p.parse_args()
    os.umask(0o077)
    try:
        if not 0<=args.days<=36500:raise ValueError('invalid retention')
        project=canonical_project(args.project_root)
        directory=project_store_directory(project)
        result=(clear_tasks(Store(directory,initialize=False,project_root=project,runtime_access=True),project,args.days,args.keep_thread,force=args.force,dry_run=args.dry_run)
                if (directory/'cortex.sqlite3').exists()
                else dict(deleted_tasks=0,retention_days=args.days,skipped_protected=0,
                          cutoff=(datetime.now(timezone.utc)-timedelta(days=args.days)).isoformat(timespec='microseconds'),
                          force=args.force,deleted_orphan_sessions=0,vacuum_status='not_needed',reclaimed_bytes=0,
                          **(dict(would_delete_tasks=0,would_delete_orphan_sessions=0) if args.dry_run else {})))
        print(json.dumps(result))
    except Exception:
        raise SystemExit('Cortex retention cleanup failed; no private details are displayed.') from None


if __name__=='__main__':main()
