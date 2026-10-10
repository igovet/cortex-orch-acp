# Markdown reports

Cortex stores immutable Markdown reports under the project-local `.cortex`
store. A worker can submit complete Markdown inline with its structured result;
the runtime publishes it through the same owned-draft immutable-report path.
Reviewers default to this inline route, retaining the shared decision brief and
explicit verdict. Alternatively, a caller creates a server-owned draft, edits
only the returned path and publishes it through the current attempt. Publication is idempotent and
retains the draft on failure for recovery.

The coordinator owns `pipeline.md`, which keeps the newest edition first and
records requirements, assignments, candidates, checks, decisions and selected
evidence. Before task-level acceptance, the coordinator publishes a current
edition with its rationale, report references, artifact and check evidence,
remaining work and decision basis. A correcting edition follows only if
the actual decision changes. Reports are evidence; publication does not mean
semantic acceptance.

The pre-accept edition describes a lasting decision basis and actual unresolved
deliverable or verification work, without describing acceptance as the next
step or claiming it is pending. The task decision event and stored state give
the recorded outcome after the edition is published.

Routine reports open with a decision brief, then record the result, changes,
checks, limitations and relevant evidence links. The implementation, verification
and pipeline templates leave incident decisions out by default. For a production
incident, the coordinator records an incident decision in the current pipeline;
the owner can include it in the final handoff when relevant.

Report reads use bounded cursors and exact report identities. Workers receive
selected predecessor reports through context packets; they do not inspect
private store paths. The runtime records source revisions, candidate digests,
requirement revisions and attempt identity so stale evidence can be invalidated
without rewriting the original report.

The public operation contracts are in [`src/cortex_runtime/contracts.py`](../../../src/cortex_runtime/contracts.py).
The focused lifecycle tests are `tests/test_source_reports.py` and
`tests/test_pipeline_bootstrap.py`.
