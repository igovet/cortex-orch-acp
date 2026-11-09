# CW-U03: Desktop decision queue with typed conflicts

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Build the desktop queue of open decisions across projects. Questions, permissions and result acceptance are separate kinds. Each decision detail states its effect, scope and rejection consequence. Submissions carry a request ID and the expected entity version. Racing submissions resolve to one winner, and every losing or stale outcome is shown with a typed reason.

## Requirements
- CW-R02: a decision shows the wait reason it resolves.
- CW-R03: questions, permissions and result acceptance are distinct; there is no approve-all.
- CW-R04: version checks, idempotent requests and server-side authorization.

See [requirements](../requirements.md).

## Inputs
- [Screens](../SCREENS.md): decision detail must identify effect, scope and rejection consequences.
- [Protocol](../protocol.md): request ID, expected version, one winner for concurrent distinct decisions, explicit stale and revoked failures.
- [Consistency and recovery](../consistency-and-recovery.md): device races need executable tests.
- [ADR 0007](../adr/0007-result-acceptance.md): acceptance is bound to a version and separate from Git.
- The decision handler and its tests in CW-C03.

## Deliverables
- Queue with three separated groups (question, permission, result acceptance). No bulk-select and no approve-all control.
- Decision detail: what is requested and why, what happens on approve or reject, what remains blocked, and the entity version the decision applies to.
- Submit path with a generated request ID kept for retries. Typed outcomes: accepted, already resolved (with the winning actor or device), superseded, expired, revoked, unauthorized, unknown.
- Unknown outcome shows "not confirmed" and offers a re-read of the decision. It never offers a silent resubmit.
- The free-text answer ("Other…") on a question card takes line breaks and attachments, as the main composer does: Enter sends, Shift+Enter inserts a line break. The answer is submitted as one decision with its attachments. Files can be dropped into the field, and a copied screenshot can be pasted into it ([CW-R39](../requirements.md)).

## Acceptance
Positive:
- Two submissions with different request IDs for the same decision: exactly one is accepted. The other shows "already resolved" with the winning actor.
- Re-sending the same request ID and payload returns the original receipt and produces no second effect.
- An approval against the current entity version is accepted. The item leaves the queue only after the projection confirms it.
- A free-text answer with line breaks and an attachment is delivered unchanged as one answer.

Negative:
- No approve-all control exists. A result-acceptance decision never renders as a single permission-style button without its evidence link.
- A decision whose version has moved on is shown as stale with the reason. It is not dropped from the queue.
- A decision submitted from a revoked device is refused with an explanation. The decision stays open for authorized devices.
- A lost response leaves the decision in "outcome unknown" until a read confirms its state. No automatic resubmission.
- The same request ID with a different payload is rejected.
- A free-text answer whose attachment fails to upload is not submitted, and the text stays in the field.

## Dependencies
- [CW-U01](CW-U01.md) (project context), CW-C03 (decision CAS, idempotency and revocation handler).
- Gate records: CW-D05 (Gate D).

## Validation
- Automated race tests against the real CW-C03 handler in a test service, with two request IDs for one decision. A UI mock does not count as evidence for this.
- Rendered run of the queue at 1280×800 and 1920×1080 with stale, conflict and revoked fixtures. Capture screenshots.
- Keyboard-only path from queue to decision detail and back.
- Record exact commands, environment and outcomes.

## Documentation impact
- [Screens](../SCREENS.md): queue and decision-detail states.
- [Protocol](../protocol.md): names of typed conflict outcomes once implemented.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
M: one queue, one decision detail and one typed submit path, all tied to CW-C03 behaviour.
