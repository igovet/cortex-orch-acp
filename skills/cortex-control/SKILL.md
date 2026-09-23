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

The bundled [retention command](../../src/cortex_clear.py) is a user-run
maintenance tool, not something the coordinator executes. When the user's
taskless `clear N days` request reaches the coordinator, explain the exact
command for the user to run by hand in their own shell:

```
python3 src/cortex_clear.py --project-root <path> --days N [--keep-thread <id>]
```

`--project-root` is the canonical project directory and `--days` the
retention window (a nonnegative integer). `--keep-thread <id>` is repeatable
and protects any task bound to that exact thread ID; omit it when the user
names no thread to keep. Do not invent other flags. This route creates no
task, governance choice, pipeline, or report, and the coordinator never runs
it with `command_run` or any other tool on the user's behalf.

The command removes matching old task records and project task directories. It also
removes only project draft files whose stored task relationship and SHA-256 make the
association unambiguous. Never remove the whole project `draft-reports` directory or
unknown drafts. Its JSON output reports only deletion and protection counts.

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
