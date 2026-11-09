# CW-P03: Wire desktop event-outbox writes to push dispatch (new decision/result/question → push gateway call with recipient's stored FCM token)

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Dispatch a push for each new decision, question or result event from the desktop's public outbox. Send it to every paired, non-revoked device that holds a registered FCM token.

## Requirements
CW-R09 (privacy-preserving push with an explicit unavailable state) and CW-R16 (reject revoked devices; protect credentials). See [requirements](../requirements.md).

## Inputs
- [Consistency and recovery](../consistency-and-recovery.md): at-least-once delivery must not produce duplicate effects.
- [Security design](../security.md): push is an opaque hint.
- CW-C02 (public outbox and cursor), CW-P01 (gateway schema), CW-P02 (token registration), CW-N04 (revocation).

## Deliverables
- An outbox consumer in the desktop service. It maps the closed set of event types (`decision_requested`, `question_asked`, `result_ready`) to schema-valid dispatches carrying the persisted event-scoped `task_ref` defined in CW-P01; retries and restart reuse it, while distinct events on one task use distinct references. The outbox cursor advances only after the dispatch outcome is recorded.
- Per-device dispatch: one dispatch per paired device with a registered token. Revoked devices are skipped.
- Bounded retry with exponential backoff and a maximum attempt count. A `token_invalid` answer clears the stored token.
- A dispatch status record (`sent`, `failed`, `token_invalid`, `unavailable`) for diagnostics. It holds no content.

## Acceptance
**Positive**
- A new decision event yields exactly one gateway request per paired device that holds a token. Idempotency is keyed on event ID and device.
- Restarting the desktop mid-dispatch loses no event, because the outbox replays it. After a recorded success, the event is not re-sent by the desktop. FCM can still duplicate a delivery; CW-P02 deduplicates by `task_ref` on the phone within its supported window.

**Negative**
- After revocation, a revoked device receives zero requests.
- A device with no token is recorded as `unavailable` and produces no request.
- With the gateway unreachable, retries stop at the bound and record `failed`. No content is queued.
- An event of unknown type is not dispatched.
- The dispatch body equals the CW-P01 schema, checked by a test.
- Reject revoked or expired credentials: CW-N04 revocation stops dispatch immediately.

## Dependencies
CW-P01, CW-C02, CW-P02, CW-N04. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively.

## Validation
- Desktop outbox tests with a fixture gateway. They cover stable event references across replay and retry, distinct events on one task, the retry bound and the revoke skip.
- End-to-end delivery through the live gateway and FCM only after explicit owner authorization. Not run here.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): dispatch behavior and the unavailable state.
- [Consistency and recovery](../consistency-and-recovery.md): at-least-once push semantics.
- [Public contract](../protocol.md): outbox consumers for push.
