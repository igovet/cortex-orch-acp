---
name: cortex-control
description: Cortex retention command, coordinator-only. Load only for an explicitly assigned clear command.
---

# Cortex control

## Loading boundary

Read this skill with `read_skill` only for the coordinator's declared clear-command
purpose. Only the coordinator can read this skill; a worker's or the consultant's
`read_skill` never advertises or accepts this id. A worker or the consultant reaches
the optional report-example catalogue and its named examples through the separate
`index` id (physically under `references/` here), not by reading this skill. Do not
read manifests, databases or server internals, or enumerate the installation.

## Explicit retention maintenance

Tell the owner to type `/cortex-clear` in the current project's ACP chat.
The runtime handles this command itself, without a coordinator turn or model
call. Do not execute deletion through `command_run` or any other model tool.
`/clear` is host-reserved; do not offer it as a Cortex alias.

For example, preview tasks with last activity before 1 October 2026 at 00:00 UTC:

```
/cortex-clear before 2026-10-01
```

The runtime preview reports deletion/protection counts and cutoff. Manual CLI JSON includes `deleted_tasks`, `retention_days`, `skipped_protected`, `cutoff` (ISO-8601 UTC), `force`, `deleted_orphan_sessions`, `vacuum_status`, and `reclaimed_bytes`; after actual reclamation it also includes `db_bytes_before` and `db_bytes_after` (and `vacuum_error` on failure). Dry runs add `would_delete_tasks` and `would_delete_orphan_sessions` instead of deleting or vacuuming. The runtime also gives the exact command to confirm:

```
/cortex-clear before 2026-10-01 confirm
```

Retention days are also accepted: `/cortex-clear 7`, `/cortex-clear 7d`, or
`/cortex-clear days 7`; integers must be 0..36500. A bare `YYYY-MM-DD` works
as an absolute cutoff. Malformed, invalid and future dates are refused.
Without trailing `confirm` nothing is deleted, even with `force`. Chat cleanup
normally protects loaded-session tasks, non-terminal tasks and live/ambiguous
coordinator/worker activity. To delete stale protected tasks, the owner can use:

```
/cortex-clear days 1 force
/cortex-clear days 1 force confirm
```

Force also works with dates and bare days. It always keeps the invoking session
and its task/kept threads; it assumes other processes are already dead, marks
stale reservation/process rows released/terminated, and never kills OS processes.
Never recommend force for genuinely running work. Busy-store access and pending
intents conflicting with remaining keeps are still refused.

Cleanup also removes old unbound/orphan host sessions and their transcript/update
rows, excluding the invoking session and (without force) other loaded sessions.
Their last activity includes creation, close and latest transcript/update times.
Preview reports `would_delete_orphan_sessions`; confirmation reports
`deleted_orphan_sessions`, `reclaimed_bytes`, and before/after DB sizes after
SQLite `VACUUM`. A vacuum failure leaves deletion committed and is stated separately.

The bundled [CLI](../../src/cortex_clear.py) remains an alternative the owner
runs manually in their shell, not something the coordinator executes:

```
python3 -B src/cortex_clear.py --project-root <path> --days N [--keep-thread <id>]
```

`--project-root` is the canonical project directory; `--days` is 0..36500.
`--keep-thread <id>` is repeatable and protects the exact known thread.
The CLI retains its manual retention/keep-thread contract: it is not the
chat preview/confirm workflow and does not automatically protect active tasks.
Optional `--force` explicitly marks stale live-state rows released/terminated;
the CLI already ignores active protection without it. `--dry-run` previews only.
Use it only when the project runtime is idle and name every thread to keep.

Cleanup removes matching old task records and project task directories, plus
only drafts whose stored task relationship and accepted SHA-256 make the
association unambiguous. Never remove whole draft directories or unknown files.
No output contains task contents or private reports. Committed deletion intents
remain recoverable after interrupted filesystem cleanup.

## Optional report examples

The [report example catalogue](references/index.md) routes optional examples.
Read a relevant declared example only when the assignment asks for one. Otherwise use the
headings and guidance returned by the live draft creator. Examples guide content
and never replace the profile's requirements,
live writer schema, observable evidence, or honest unrun checks.

All worker reports remain complete English Markdown files, either submitted
inline or allocated by the live draft creator, filled through the owned-draft
gateway and published through the live writer, per the live tool descriptions.
Preserve server-issued registration metadata and complete Markdown. Never edit
published documents or the runtime database directly.
