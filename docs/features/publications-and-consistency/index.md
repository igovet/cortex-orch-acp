# Publications and consistency

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


There is one project-local `.cortex/cortex.sqlite3` metadata index. Published
Markdown bodies live alongside it under `.cortex/`. Each task has one real
`pipeline.md`, newest edition first, with older editions below. The runtime alone
publishes pipeline editions. Worker reports are immutable independent Markdown
files, selected using bounded catalogue and content cursors.

Workers submit a result with its report as one required top-level plain string,
never an object. Inline: `result_submit.report` carries the complete Markdown
(titled by the optional `report_title`, else by its first heading), and the
runtime publishes it through the same `create_draft`/`write_report` path as the
worker's own draft before recording the result. By reference: the worker creates
an owned draft, fills it through `draft_write`, publishes using `write_report`,
and submits only the exact returned `report_id` as `result_submit.report`.
`invalid_arguments` on `arguments.report` identifies a missing report, an object
or array value, a whitespace-only string, or an identifier-shaped string that is
not an exact report id (a draft id, a receipt, a malformed report id), all
refused before anything is published. Decision-brief items (conclusion, evidence,
checks, contradictions, limits, next action) are Markdown sections inside the
report, never separate fields. The channel binds the task, owner epoch and
attempt.

The report is a string because Claude engines emit a top-level string argument
verbatim but must hand-write JSON for object and array values. With the former
`report: {title, markdown}` object, Sonnet 5 workers broke the escaping of the
nested Markdown (raw newlines, invalid escapes) in about a quarter of inline
reports of 3 KB or more across 2026-10-04..07 transcripts, and Claude Code refused
the call ("could not be parsed as JSON") before Cortex received it; the same
workers' multi-kilobyte top-level `project_write`/`draft_write` strings never
failed. Every other string a `result_submit` object or array carries is bounded
at 4096 characters or fewer.

The advertised `result_submit` JSON schema lists `report` in its plain `required`
list, carries both forms in that one string, and states
the check-field combinations: `not_run`/`unverified` require `blocker`, while
`passed`/`failed` omit `blocker`; an optional `command` is descriptive only and
never replaces receipt evidence. A new result needs a fresh
`request_key`; retry an uncertain delivery only with identical arguments.
The worker copies `write_report.report_id` alone into `result_submit.report`, not
the draft id or a command receipt. New rejected `result_submit` calls attempt
to retain only their error code and safe field path in `submission_rejection`
observability events, never the rejected arguments or report body. That
includes `situational_skill_required`: a worker whose observed changes pair
attributable documentation with behavior files (own declared scope or successful
channel writes, excluding siblings' reserved paths) without a retained `read_skill` of
`documentation-sync` is refused before any request, report or result is
recorded, and may retry with the same key after reading it (see
[roles and delegation](../roles-and-delegation/index.md)).

An inline report is bound to the result's request key: its draft and publication
use store delivery keys derived from the channel and that key, and the retained
request stores the Markdown's SHA-256 instead of a second copy. The report is
published before the result request records its `report_ref`, and the result is
published and recorded only after that. A crash at any point leaves at most an
unpublished draft or a published report without a result; retrying the same
request replays both deliveries and reuses that report, never publishing a second
one or completing the node early. Recovery accepts an interrupted inline result only
when its `report_ref` is the one its own keyed `write_report` delivery produced. A
changed report under the same request key is a `request_key_conflict`. A report from another worker,
a fabricated reference or a changed retry payload is rejected. `read_report` takes
that same exact identifier in its `report_id` field. Tool argument contracts live
in schemas, not profile prose. There is no legacy `publish_result` operation.

Before any proposal or result, inspect the live schema, including conditional
requirements, and measure bounded text in its stated counting unit. Plan task
objectives advertise their Unicode code-point bounds in the property description
as well as JSON Schema, through the shared Cortex MCP surface for Codex and
Claude. An overlong objective is rejected before plan persistence; rewrite it
concisely without dropping requirements, never silently truncate it. Correct a
deterministic rejected input only as the live declaration permits; uncertain
delivery still requires the identical key and payload. This is prevention
guidance, not proof that a live model follows it.

Before the first submission, prepare the complete report. A summary and verdict
are not a report. If the engine reports that a call could not be parsed, nothing
reached Cortex: resend the same arguments with valid serialization and the same
request key. Moving the report to a top-level string removes the observed
nested-escaping failure class; it does not repair arbitrary model output, and no
paid live run has yet established Sonnet 5 compliance with the new form.

Publication retains exact task/report/attempt identity. Runtime projection can
recover an interrupted result submission from its durable publication event; absent
or ambiguous evidence remains unresolved. A result closes a node only when its
owned report exists and its digest is retained. A passing check stays passed only
when it refers to an actual complete, successful, nontrivial gateway command
receipt of the same attempt; any other passing claim is recorded unverified (or
failed, for a cited nonzero exit) with its `evidence_gap`, as described in
[check evidence](../orchestration/index.md#check-evidence). A failed check stays failed
in the published result; only an exact later rerun of the same command closes it
(`closed_by`, see [check history](../orchestration/index.md#check-history-and-current-outcome)),
and publication and recovery preserve that closure. Exact argv/cwd reruns may cross between `command_run` and `outside_command` in either direction, without erasing either receipt or granting execution permission; same-operation profile mismatches remain nonmatching. An eligible own-probe syntax, regex or statically proven literal-expectation error is kept as `retracted` history with `retracted_by`, never as a pass, and publication and recovery preserve that as well. Literal-expectation corrections change only one expected literal in a bounded self-contained data probe, never product assertions. Project test/lint/build failures and runtime verification cannot be retracted. Before recording a new
submission, an open failure returns `open_failed_check` with the check, receipt,
a bounded command identity (never the argv or script), the relevant rerun and its
evidence gap, and says when no exact rerun can close it. A failed receipt an
earlier submission of the attempt presented must be cited again, or the result
is refused with `omitted_failed_check` ([check history](../orchestration/index.md#check-history-and-current-outcome)).
No result request, inline report or result
is published, and the channel stays active. Retain the failed check and cite a
verified later rerun of exactly that command, or explicitly submit
`assignment_outcome: "failed"` for an intentional failed assignment. Changing
argv does not close it. The refusal leaves its key reusable. Accepted receipts
include `node_status` (`completed` or `failed`), not task acceptance; the runtime
still re-derives check outcomes on recording and recovery. A not_run or unverified
check must include its concrete blocker in the structured result; publication
and recovery preserve that blocker. Chat text cannot publish
or accept a task. Review additionally binds the project digest and an explicit
APPROVE, REQUEST_CHANGES or BLOCK verdict, with optional structured findings that
the runtime checks before it confines fixes to the named plan tasks.

Runtime state, graph changes and events commit transactionally. Gateway request
receipts are retained before external effects. An uncertain effect blocks replay;
idempotency never turns an unobserved outcome into success. Worker draft/report
request keys are scoped to the authenticated channel so different workers may use
the same local key without receiving each other's receipts.

The completion gate requires all current nodes, immutable report digests, the latest
approving review, complete passing runtime verification on the same project revision,
synthesis, and observed absence of all worker engines. File drift, truncated output,
missing publication or unknown process liveness blocks completion.

Format-11 migration under the exclusive lock retains a private backup; inspection
opens do not mutate. Historical storage relations can be inspected but do not
reactivate model-owned delegation, integration or acceptance routes.
