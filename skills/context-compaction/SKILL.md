---
name: context-compaction
description: Restore coordinator and specialist context after compaction or restart.
---

# Context recovery

Use this skill after context loss. A recovery summary points into durable state;
it does not replace the original request, assignment or evidence. Recover only
what the summary lacks for the next action, in the order below, and stop once
you have it. Reload the next needed live tool declaration before calling it;
remembered arguments are not authoritative.

## Coordinator

1. Call `task_get_context` to recover the current task, requirements, plan
   revision, results and unresolved decisions in the same session.
2. Read the current pipeline with `read_report`, following bounded cursors only
   for the original goal, decisions and evidence the next action needs.
3. Use `list_sources` and needed source reads only for exact user requirements
   and attachment references the summary lacks. Read relevant immutable reports
   by exact reference.

Follow the runtime's per-turn language directive (English or Russian), not the
language of a recovery summary. Worker messages and briefs stay in English.
The coordinator may read bounded project files or code-index results for a direct
answer, orientation or evidence reconciliation, consistent with the orchestrator.
Harvest-specific source discovery remains worker-owned. Project edits and checks
belong to workers; the runtime owns scheduling, review and verification. Do not
recreate workers, manage engine handles, replay uncertain external effects or
infer completion from chat. Report missing or inconsistent evidence; the runtime
reconciles durable identities.

## Worker

1. Read `read_capsule` first when the summary lacks the exact assignment, role
   instructions, requirements, declared work area, retained checks, report
   references or drafts.
2. Read the assignment's relevant immutable reports and clarifications through
   bounded pages. Use `list_reports` only when a needed reference is missing.
3. If your role writes, inspect current target state before editing and preserve
   unrelated changes. Reuse retained command receipts rather than rerunning
   unchanged checks. Consultants recover only their packet and retained reports;
   do not inspect project source or perform checks.

Resume English worker communication; user-facing structured questions follow the
runtime's per-turn language directive and advertised tool contract. Recovery does not grant new scope or access.
Use the runtime's advertised routes within their live contracts for missing input
or additional project paths; do not ask the user to repair tool access. Report unavailable
sources or attachments explicitly; do not infer their contents or create a
replacement task. Retain owned draft identities and acknowledged publications.
