# Markdown reports

Cortex stores immutable Markdown reports in the project's [private home store](../../project/storage.md#where-the-database-lives-and-how-it-moves). A worker can submit complete Markdown inline with its structured result;
the runtime publishes it through the same owned-draft immutable-report path.
Reviewers default to this inline route, retaining the shared decision brief and
explicit verdict. Alternatively, a caller creates a server-owned draft, edits
only the returned path and publishes it through the current attempt. Publication is idempotent and
retains the draft on failure for recovery.

The runtime owns each task's one `pipeline.md`, which keeps the newest edition
first with older editions below. No model creates or edits it: `create_draft`
does not offer the pipeline template (the store still refuses it with
`runtime_owned_operation`). The runtime
publishes the first edition ("Initial pipeline", the pipeline template filled
with the owner's request and the coordinator route) before the first
coordinator turn, and a new "Runtime <STATE>" edition at each driver boundary
where the plan revision, task state, review cycle or number of open questions
changed. A runtime edition records the state, plan revision, original objective,
each assignment's status, dependencies and report reference, open questions, a
review waiver, the runtime verification outcome, the synthesis and a blocker.
Editions are derived from durable state, so a crash between a state change and
its edition is repaired at the next boundary. The coordinator reads the current
pipeline with `read_report` without arguments. Reports are evidence;
publication does not mean semantic acceptance, and the task decision event and
stored state give the recorded outcome. A task created before the runtime
published pipelines gets its first edition at its next boundary; earlier
history is not back-filled. See
[native incoming ACP](../../project/native-incoming-acp.md) for the exact rules.

Routine reports open with a decision brief, then record the result, changes,
checks, limitations and relevant evidence links. The implementation, verification
and pipeline templates leave incident decisions out by default.

## Report structure

The runtime, never the model, chooses a worker report's template
(`cortex_core::assignment_brief`): the profile's `report_template` in
`profiles.json` for an execution assignment, and for a review the `general`
template with three review sections. The assignment brief's `report` object
carries that skeleton: every `##` heading in order, one line on what belongs under
it and whether it is required. An inline report is published under that template.

| Template (roles) | Required sections |
| --- | --- |
| implementation (accessibility-fixer, backend-dev, data-engineer, devops-engineer, frontend-dev, fullstack-dev, mobile-dev, refactorer) | Changes; Verification; Risks and remaining work |
| investigation (debugger, explorer) | Evidence examined; Findings; Gaps and next steps |
| verification (accessibility-auditor, build-verification, performance-engineer, qa-engineer, security-auditor) | Executed checks; Failures and gaps; Assessment |
| planning (architect, database-architect, planner, ux-designer) | Evidence reviewed; Proposed work graph; Risks and open questions |
| documentation (technical-writer) | Documentation changes; Validation; Limits and follow-up |
| general (general, senior-consultant) | Evidence and work; Verification; Risks and remaining work |
| review (every review assignment; general template) | Verdict; Findings; Coverage (optional: Verification; Risks and remaining work) |

The other headings of each template are listed in the skeleton as optional.
For report structure, `result_submit` looks for each required heading: a level-two
heading with the exact English text, letter case and spacing free, outside fenced
code. It does not judge the content. A missing section is a correctable tool error
(`report_sections_missing`, `field` `report`, `missing` naming each heading);
nothing is recorded, and a corrected resubmission, with the same request key or a
new one, is a new payload. After two such refusals in one attempt the report is
accepted as it is and the result carries `report_structure`
`{"status":"REPORT_STRUCTURE_INCOMPLETE","missing":[...]}`, which the reviewer's
`review_packet`, a successor's `predecessor_reports` and the coordinator's capsule
show. A report template's own guidance comment line is refused
(`draft_guidance_remaining`); any other HTML comment is ordinary content. For a production
incident, the coordinator records an incident decision in the current pipeline;
the owner can include it in the final handoff when relevant.

The additive Rust native host serves owned draft creation/replacement, immutable
publication and bounded report/draft reads through authenticated MCP channels.
Worker results can reference their own published report, not another principal's
report. Discovery and remaining limits are described in
[native incoming ACP](../../project/native-incoming-acp.md).

## Required inputs for an assignment

A plan node can require selected immutable reports through `required_reports`.
A `from_node` names a direct dependency in the same plan. The runtime resolves
that dependency's accepted result in the current revision to its report ID and
digest. An already published report in the same task can be named by its exact
`report_id` and `sha256`. The assignment brief shows the resolved materials.

The whole report is required by default. A `section` selects a half-open range
of zero-based Unicode scalar values. Use `read_report` with the report ID and
the section's `start`, then continue with each returned cursor as far as the
exclusive end. For the whole report, start at the beginning and follow the
cursors through the final page. Each direct read produces an exact receipt.
An oversized page requests a smaller limit. Reads on another channel, skipped
page chains and incomplete ranges do not satisfy the requirement.

`result_submit` returns `required_reports_unread` with the outstanding
materials before publishing a result. The same attempt can read the missing
material and submit again. A replacement attempt reads it on its own channel.
The runtime rechecks the immutable bytes on submission. If an exact input is
unavailable, the worker can report the blocker through its question route.
Other reports remain available for selective reading.

## Result submission arguments

`result_submit` rejects unknown fields such as `artifacts`, which belongs to
`write_report`. The gateway identifies the offending field path without echoing
its value. Where checks support command evidence, copy `tool_call_ref` verbatim
from the completed command response; do not add or replace a prefix.

## Report reads

Report reads use bounded cursors and exact report identities. Workers receive
selected predecessor reports through context packets; they do not inspect
private store paths. The runtime records source revisions, candidate digests,
requirement revisions and attempt identity so stale evidence can be invalidated
without rewriting the original report.

The public operation contracts are the advertised tool schemas in `crates/cortex-engine/src/schema.rs`.
The focused lifecycle tests are in `crates/cortex-core/tests` (`publication.rs`, `publication_parity.rs`).
