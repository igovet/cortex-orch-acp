---
name: cortex-control
description: Cortex retention command and optional report-writing examples. Load only for an explicitly assigned clear command or when a worker is explicitly asked to consult a report example.
---

# Cortex control

## Loading boundary

Read this skill with `read_skill` only for one of the
declared purposes. Workers receive their role instructions at engine-session
creation; they do not load this skill. Its named examples are available through
`read_skill` when relevant. Do not
read agent TOML, manifests, databases or server internals, or enumerate the installation.

## Explicit retention maintenance

When the coordinator assigns the user's taskless `clear N days` command, run only
the bundled [retention command](../../src/cortex_clear.py). Obtain its current
CLI syntax with `--help`; do not read the script implementation. Supply the canonical
project, exact retention and any protected active session or thread IDs established by the
coordinator. This route creates no task, governance choice, pipeline, or report.

The command removes matching old task records and project task directories. It also
removes only project draft files whose stored task relationship and SHA-256 make the
association unambiguous. Never remove the whole project `draft-reports` directory or
unknown drafts. Return only deletion and protection counts.

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
