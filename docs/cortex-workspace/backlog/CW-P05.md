# CW-P05: Optional UnifiedPush receiver support for owners running their own distributor; strictly additive/opt-in

**Status:** planned · **Phase:** Post-v1 · **Size:** M

## Goal
Add optional UnifiedPush receiver support for owners who install an external distributor. It is additive, opt-in and never replaces FCM.

## Requirements
CW-R09 (push with an explicit unavailable state) and CW-R16 (protect credentials and device identities). See [requirements](../requirements.md).

## Inputs
- The design report's fast-follow position: UnifiedPush is not a v1 commitment because the default-user path would still need FCM.
- CW-P02 (receiver and schema) and CW-P03 (dispatch).
- [Security design](../security.md): push is an opaque hint.

## Deliverables
- A UnifiedPush connector in the Android app, behind an explicit opt-in setting, off by default.
- Endpoint registration through the authenticated desktop channel. The UnifiedPush endpoint URL is treated as a capability secret: it is stored in Keystore-backed storage, never logged, and rotated on re-registration.
- A documented precedence rule for when both FCM and UnifiedPush are available.
- Dispatch to a UnifiedPush endpoint uses the same metadata-only schema (CW-P01).

## Acceptance
**Positive**
- With opt-in enabled and a distributor installed, messages are handled exactly as in CW-P02: same schema and on-open fetch.
- The documented precedence rule is applied and shown in the UI.

**Negative**
- Without opt-in, UnifiedPush is inactive, and no endpoint is registered.
- With opt-in but no distributor, the state is `unavailable`. The app does not fall back to any third-party service.
- The endpoint URL does not appear in logs or diagnostics.
- A message that does not match the schema is rejected.
- Turning opt-in off unregisters the endpoint and stops dispatch to it.
- Reject revoked or expired credentials: a revoked device's endpoint is removed by CW-N04 revocation.

## Dependencies
CW-P02. Post-v1. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively.

## Validation
- Physical Android with an owner-chosen distributor. Not run; it is planned post-v1 work.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): optional distributor path.
- [Platform policy](../platforms.md): the optional Android push path.
- [Security design](../security.md): endpoint-URL secrecy.
