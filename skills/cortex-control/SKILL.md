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
read agent TOML, manifests, databases or server internals, or enumerate the installation.

## Explicit retention maintenance

Tell the owner to type `/cortex-clear` in the current project's ACP chat.
The runtime handles this command itself, without a coordinator turn or model
call. Do not execute deletion through `command_run` or any other model tool.
`/clear` is host-reserved; do not offer it as a Cortex alias.

For example, preview tasks with last activity before 1 October 2026 at 00:00 UTC:

```
/cortex-clear before 2026-10-01
```

The runtime preview reports deletion/protection counts and cutoff. CLI JSON reports exact `deleted_tasks`, `retention_days`, and `skipped_protected` counts/settings plus `cutoff` as an ISO-8601 UTC timestamp. The runtime also gives the exact command to confirm:

```
/cortex-clear before 2026-10-01 confirm
```

Retention days are also accepted: `/cortex-clear 7`, `/cortex-clear 7d`, or
`/cortex-clear days 7`; integers must be 0..36500. A bare `YYYY-MM-DD` works
as an absolute cutoff. Malformed, invalid and future dates are refused.
Without trailing `confirm` nothing is deleted. Chat cleanup protects tasks
bound to this agent's sessions, non-terminal tasks and live or ambiguously
live coordinator/worker activity. It refuses busy-store and protected pending
intent races. Ask the owner to retry from an idle session, not to bypass guards.

The bundled [CLI](../../src/cortex_clear.py) remains an alternative the owner
runs manually in their shell, not something the coordinator executes:

```
python3 -B src/cortex_clear.py --project-root <path> --days N [--keep-thread <id>]
```

`--project-root` is the canonical project directory; `--days` is 0..36500.
`--keep-thread <id>` is repeatable and protects the exact known thread.
The CLI retains its manual retention/keep-thread contract: it is not the
chat preview/confirm workflow and does not automatically protect active tasks.
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

All worker reports remain complete English Markdown files first allocated by the
live draft creator, filled through the owned-draft gateway and published through
the live writer. Preserve server-issued registration metadata and complete Markdown.
Never edit published documents or the runtime database directly.
