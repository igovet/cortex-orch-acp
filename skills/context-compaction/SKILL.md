---
name: context-compaction
description: Restore coordinator and specialist context after compaction or restart.
---

# Context recovery

Use this skill after context loss. A recovery summary points into durable state;
it does not replace the original request, assignment or evidence. Reload the
next needed live tool declaration before calling it; remembered arguments are
not authoritative.

## Coordinator

1. Call `task_get_context` to recover the current task, requirements, plan
   revision, results and unresolved decisions in the same session.
2. Read the current pipeline with `read_report`, following bounded cursors for
   the original goal, decisions and evidence needed for the next action.
3. Use `list_sources` and needed source reads to recover exact user requirements
   and attachment references. Read relevant immutable reports by exact reference.

Follow the runtime's per-turn language directive (English or Russian), not the
language of a recovery summary. Worker messages and briefs stay in English.
The coordinator may read bounded project files or code-index results for a direct
answer, orientation or evidence reconciliation, consistent with the orchestrator.
Harvest-specific source discovery remains worker-owned. Project edits and checks
belong to workers; the runtime owns scheduling, review and verification. Do not
recreate workers, manage engine handles or replay uncertain external effects.

## Worker

1. Read `read_capsule` first to recover the exact assignment, role instructions,
   requirements, declared work area, retained checks, report references and drafts.
2. Read the assignment's relevant immutable reports and clarifications through
   bounded pages. Use `list_reports` only when a needed reference is missing.
3. If your role writes, inspect current target state before editing and preserve
   unrelated changes. Reuse retained command receipts rather than rerunning
   unchanged checks. Consultants recover only their packet and retained reports;
   do not inspect project source or perform checks.

Resume English-only communication. Recovery does not grant new scope or access.
Use the advertised worker question route to the user for missing material input,
and the access-request route for additional project paths. Report unavailable
sources or attachments explicitly; do not infer their contents or create a
replacement task. Retain owned draft identities and acknowledged publications.
