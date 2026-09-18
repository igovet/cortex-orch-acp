# Portable role instructions and delegation provisioning (T015)

Scope: preserve the T001 specialization inventory from a portable canonical
source per role, expose a small internal delegation interface, and provision
its technical execution context (workspace/channel/config/context/engine
session) atomically and recoverably. Production wiring of this service into
a real incoming ACP session and a real outgoing engine is T016/T017 -- this
task's code is only exercised here against fakes/stubs.

## Canonical role sources

The actually-loaded specialization mechanism, per the T001 baseline, is the
packaged `skills/` directory (the native-plugin `.codex-plugin/plugin.json`
manifest that once declared it was removed at T033; ACP engine adapters
read these files directly): 23 `skills/worker-*/
SKILL.md`, `skills/orchestrator/SKILL.md`, and 7 shared/coordinator skills
(`cortex-control`, `coordinator-communication`, `content-safety`,
`tool-discipline`, `knowledge-harvest`, `documentation-sync`,
`context-compaction`). The 22 non-consultant worker skills and their Codex
`agents/*.toml` renderings are both generated, byte-identically, from one
shared template (`plugins/cortex/agent-sources/worker-protocol.md` +
`consultant-protocol.md` + a per-role `agent-sources/<role>.md`) by
`scripts/generate_agent_profiles.py` (`check`/`write`); the coordinator and
shared skills are hand-maintained directly under `skills/`. `portable.roles`
loads its `RoleInstructionSet` from the shipped `skills/*/SKILL.md` files
(the real loaded artifact), not by re-running the generator.

## Native markers

Codex-only tool names, environment variables, and install paths are wrapped
in the canonical sources with:

```
<!--cortex:native codex-->
... Codex-only text ...
<!--/cortex:native-->
```

`portable.roles.load_role` strips these spans from `body_portable`. The
generator and the shipped `skills/*/SKILL.md`/`agents/*.toml` files keep the
marker comments in place (there is no separate portable-only artifact
generated); this is the simplest of the two options the task allowed and is
justified because HTML comments are inert to Codex's own rendering of these
files today, while `body_portable` (the only thing ever handed to a
non-Codex engine) removes them. `tests/test_instruction_sources_portable.py`
scans `agent-sources/**` and the 8 shared/coordinator skill directories
(recursively, including `references/*.md`) for a fixed denylist
(`codex_app`, `mcp__codex`, `apply_patch`, `spawn_agent`, `CODEX_HOME`,
`~/.codex`, `.codex-plugin`, `.codex/cortex`) outside marker spans.

`skills/orchestrator/references/worker-routing.md`'s native-subagent-tracking
paragraph is marked the same way, and `worker-protocol.md`'s two native
sentences (`codex_app.send_message_to_thread`, `apply_patch`) are marked in
every one of the 22 generated worker sources.

### Critical rules

A declared `<!--cortex:critical-->...<!--/cortex:critical-->` span (bulleted
or a marked existing sentence) becomes `RoleInstructionSet.critical_rules`.
`worker-protocol.md`'s own text is within single-digit characters of the
package's compact-source ceiling shared by all 22 rendered worker profiles
(`tests/test_package.py`'s `< 12_500`-character check); adding a
per-worker marked block large enough to carry 2+ non-negotiable rules does
not fit that budget. Those two rules (restated, not new, from
`worker-protocol.md`'s existing "Assignment contract" text) are instead
defined once as `portable.roles.WORKER_CRITICAL_RULES` and applied
uniformly to every `kind="worker"` role. `orchestrator`'s and
`worker-senior-consultant`'s critical rules use an in-place marker around
existing sentences (no new prose, near-zero size cost). The 7 shared skills
have no `critical_rules` today (`()` is not an error).

## Instruction-delivery qualification

`portable.instruction_delivery.DeliveryQualification` is one of `qualified`,
`unqualified`, `unsupported`. No ACP release exposes a universal
system-prompt/skills field (T002/T013 evidence), so nothing is assumed
qualified without live proof (T035+).

| Engine | Version | Channel | Qualification |
|---|---|---|---|
| codex-acp | 1.12.0 (pinned) | nonce-tagged leading `session/prompt` text block (`engine.codex_adapter_acp.build_first_prompt_blocks`) | `qualified` -- T035 live evidence (real run, `gpt-5.6-luna`: instructions followed, a guessed closing tag rejected); see `docs/acp/codex-acp-adapter.md`'s "Instruction delivery" section |
| claude-agent-acp | any | none | `unsupported` -- no static evidence of any delivery mechanism yet |
| anything else | any | none | `unsupported` (unknown engine/version never guesses a qualification) |

`instruction_delivery.qualify_delivery(role, engine, version, user_blocks,
allow_unqualified_delivery=False)` refuses (raises `PortableError` code
`instruction_delivery_unqualified`, naming engine/version/channel, with a
concrete repair: "qualify via T035 live evidence or choose a qualified
engine") delegating a role that carries `critical_rules` unless the
delivery is `QUALIFIED`. The only escape hatch is a **constructor
parameter**, `ProvisioningService(..., allow_unqualified_delivery=True)` --
never a per-call/tool argument -- and when used it records
`instruction_delivery="unqualified"` on the provisioning row. A role with
no `critical_rules` (the 7 shared skills) may be delivered at any
qualification, since there is nothing non-negotiable to risk losing.

T035 flips `codex-acp`/1.12.0 (or `claude-agent-acp`) to `qualified` only by
adding a new, version-gated class to `instruction_delivery._REGISTRY` backed
by live evidence -- never by relaxing this module's default.

## Delegation operations

`delegation.contracts` defines two small, closed, **internal** (non-ACP)
operation sets -- never new standard ACP methods:

- coordinator: `delegate(title, brief, role, acceptance, request_key)`,
  `list_events(after, limit)`, `clarify(assignment_ref, message,
  request_key)`, `cancel(assignment_ref, reason, request_key)`.
- worker: `ask_question(question, request_key)`, `publish_result(report_ref,
  summary, request_key)`.

No schema has a task/attempt/session/channel/workspace/epoch id field;
`assignment_ref` is an opaque string returned by `delegate` and passed back
unchanged. `schemas_for(role_kind)` returns a fresh deep copy every call
(call it again on every session/generation start rather than caching a
returned schema), and `validate` raises `contracts.StoreError` with a
concrete field/received/expected/correction on any deviation, including an
attempted id field.

## Provisioning: one semantic operation, recoverable

`delegation.provisioning.ProvisioningService.delegate(channel, title, brief,
role, acceptance, request_key)` is the single semantic delegation call.
`channel` is a resolved, trusted `trusted_channel.ChannelContext` (coordinator
role only); every technical id is derived from it plus this service's own
bookkeeping -- the model supplies no id.

Provisioning steps, tracked one row per `idempotency_key =
sha256(task_id|coordinator_channel_id|request_key)` in
`delegation_provisioning`: `assignment`/`attempt` (pure local id allocation,
done with row creation) -> `workspace` -> `channel` -> `config_snapshot` ->
`context_snapshot` -> `engine_session` -> `ready`. Role instructions are
resolved and instruction delivery is qualified (see above) *before* any
external effect.

External effects (workspace creation, engine session open) are provided by
injected protocols (`WorkspaceProvisioner`, `EngineSessionOpener`,
`ConfigSnapshotSource`) -- never a real process in this task's tests. Each
external step commits an "intent" row to `delegation_effects` before the
call and an "outcome" row after it succeeds. A retry with the identical key
resumes from the first incomplete step:

- **workspace** intent-without-outcome: `WorkspaceProvisioner.lookup` ->
  `present` adopts it, `absent` retries `create`, `unknown` fails closed
  with `provisioning_indeterminate` (never retried automatically).
- **engine_session** intent-without-outcome: `EngineSessionOpener.
  lookup_owned` -> `alive` adopts it, `absent` retries `open`, `unknown`
  fails closed the same way.
- **channel**: reconciled by a direct query for a channel already bound to
  the attempt id (channel issuance is one local SQLite transaction, so the
  only uncertain window is between its own commit and this service
  recording the outcome).

Concurrent `delegate` calls under the same key (for example two callers
racing a resumed retry) converge on one live assignment via a
compare-and-swap claim on the row's `step` column: the CAS winner performs
the external effect; a loser waits (bounded) for the row to advance instead
of repeating the effect. A bounded further wait followed by a one-shot
"steal" of an apparently-abandoned claim (`claimed:<step>` with no
progress) supports resuming after a genuine crash without ever letting two
still-live callers both perform the same external effect.

Ownership epoch is checked against the channel's task at the start of
`delegate` and again immediately before the `engine_session` step; a
mismatch (task ownership transferred mid-provisioning) fails closed with
`stale_ownership_epoch`, never retried under the same key. At most one live
assignment exists per idempotency key; a **different** `request_key` with
otherwise-identical content is deliberately treated as a new, independent
delegation (no fuzzy/semantic key derivation).

## Control plane

`control_plane.ProvisioningControlPlane` is a real `ControlPlane`
implementation (`assign`/`cancel`/`status`) backed by `ProvisioningService`,
bound to one already-resolved coordinator channel; it rejects a `task_id`
that does not match the bound channel. `NotImplementedControlPlane` remains
the production default -- wiring a real channel/store/engine into this class
for an actual incoming ACP session is T016/T017 scope. `status` here
returns the raw `delegation_provisioning` rows for a task rather than a
separate cursor-paginated event log; `list_events`'s `after`/`limit`
contract exists in `delegation.contracts` for a future richer event feed
but is not yet backed by a distinct event table (documented limitation).

## Deferred to T016/T017

- Wiring `ProvisioningService` to a real project `Store`, a real
  `WorkspaceProvisioner` (actual workspace directories), a real
  `EngineSessionOpener` (actual `codex-acp`/other engine process launch),
  and a real `ConfigSnapshotSource` (actual `configuration`/`catalog`
  routing).
- Wiring `ProvisioningControlPlane` behind the incoming ACP session/agent
  surface so a real coordinator turn can call `delegate`/`cancel`/`status`.
- A distinct, cursor-paginated `list_events` backing store.
- T035's live-evidence-gated `instruction_delivery` qualification flip.
