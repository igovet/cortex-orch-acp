# Publications and consistency

There is one project-local `.cortex/cortex.sqlite3` metadata index. Published
Markdown bodies live alongside it under `.cortex/`. Each task has one real
`pipeline.md`, newest edition first, with older editions below. The runtime alone
publishes pipeline editions. Worker reports are immutable independent Markdown
files, selected using bounded catalogue and content cursors.

Workers submit a result with its report in one of two forms. Inline:
`result_submit.report` carries the title and complete Markdown, and the runtime
publishes it through the same `create_draft`/`write_report` path as the worker's own
draft before recording the result. By reference: the worker creates an owned
draft, fills it through `draft_write`, publishes using `write_report`, and submits
the exact returned `report_id` as `result_submit.report_ref`. Exactly one form is
accepted (`result_report_conflict`, `result_report_required`). The channel binds
the task, owner epoch and attempt.

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

Publication retains exact task/report/attempt identity. Runtime projection can
recover an interrupted result submission from its durable publication event; absent
or ambiguous evidence remains unresolved. A result closes a node only when its
owned report exists and its digest is retained. A passing check stays passed only
when it refers to an actual complete, successful, nontrivial gateway command
receipt of the same attempt; any other passing claim is recorded unverified (or
failed, for a cited nonzero exit) with its `evidence_gap`, as described in
[check evidence](orchestration.md#check-evidence). A failed check stays failed
in the published result; only an exact later rerun of the same command closes it
(`closed_by`, see [check history](orchestration.md#check-history-and-current-outcome)),
and publication and recovery preserve that closure. A not_run or unverified
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
