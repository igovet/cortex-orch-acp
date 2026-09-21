# Publications and consistency

There is one project-local `.cortex/cortex.sqlite3` metadata index. Published
Markdown bodies live alongside it under `.cortex/`. Each task has one real
`pipeline.md`, newest edition first, with older editions below. The runtime alone
publishes pipeline editions. Worker reports are immutable independent Markdown
files, selected using bounded catalogue and content cursors.

Workers create owned report drafts, fill them through `draft_write`, publish using
`write_report`, and submit the exact returned `report_id` as `result_submit.report_ref`.
The channel binds the task, owner epoch and attempt. A report from another worker,
a fabricated reference or a changed retry payload is rejected. `read_report` takes
that same exact identifier in its `report_id` field. Tool argument contracts live
in schemas, not profile prose. There is no legacy `publish_result` operation.

Publication retains exact task/report/attempt identity. Runtime projection can
recover an interrupted result submission from its durable publication event; absent
or ambiguous evidence remains unresolved. A result closes a node only when its
owned report exists, its digest is retained and its declared passing checks refer
to actual complete successful gateway command receipts. Chat text cannot publish
or accept a task. Review additionally binds the project digest and an explicit
APPROVE, REQUEST_CHANGES or BLOCK verdict.

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
