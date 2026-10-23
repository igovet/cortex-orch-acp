# Changelog

## 1.0.0 (unreleased)

Orchestration audit fixes tighten path reservations, dropped-check certification,
worker-question continuation and consultant access. Model routing now requires
recorded user consent; storage inspection remains non-mutating and recovery
failures stay visible. Role guidance and generated profiles are synchronized,
and cancellation and installed-Paseo regression checks are corrected. These
changes do not establish live qualification or release readiness.

The default install preset now matches the refreshed production routing values:
Sol's `preferred_when` is `null`, so its configured work-kind preferences no longer
need an exact route hint and remain active on quality retries. At equal quota
without a provider preference, standard generic design/frontend work now selects
Sol instead of Sonnet. Affinity-bound frontend design still prefers eligible Opus,
and quota pressure still applies. The portable Sol coordinator now uses medium
effort from the saved `default` profile, as confirmed by the owner; Russian language
and the schema envelope are unchanged. No local identities or session bindings
are copied, existing installations are not overwritten, and no live qualification
is claimed.

The built-in default preset (`policy/model-settings-default.json`) was previously synced from the
owner's edited production `routing.json`: `claude-agent-acp/opus[1m]` drops the `standard` tier
entirely (now only `flagship` and `deep`) and its `preferred_for` regains `implementation_backend`
and `implementation_frontend` alongside `design`, `planning`, `implementation_frontend_design`,
`debugging` and `analysis`; `codex-acp/gpt-6.1-sol`'s `preferred_for` gains `testing`,
`investigation` and `writing`. Because Opus no longer configures the `standard` tier, standard-tier
design and implementation work -- hinted or not -- no longer reaches Opus (an explicit Opus hint on
standard-tier work is now ignored on tier mismatch rather than ranked against the owner's
preference) and falls to the owner's Sonnet/Sol preference instead; Opus still serves those kinds
once work is eligible for its `deep`/`flagship` tiers. The complete routing values
matched production at that time; no other preset field changed in that sync. No live or paid qualification is
claimed for this change.

The built-in default preset was previously synced from the owner's edited production `routing.json`:
`claude-agent-acp/claude-sonnet-5` also served the
`deep` tier alongside `standard`, and `claude-agent-acp/opus[1m]`'s `preferred_for` narrowed to
`design`, `planning`, `implementation_frontend_design`, `debugging` and `analysis`, dropping
`implementation_backend`, `implementation_frontend` and `review`; an explicit Opus hint on plain
backend/frontend/review work was no longer honoured and fell back to the owner's Sonnet/Sol
preference. Matching production routing values at that time; no other preset field differed. No live
or paid qualification was claimed for that change.

Coordinator-submitted final answers now show the structured “Not verified”
disclosure only once: the runtime removes a duplicate coordinator-authored
section with the English or Russian heading before appending its canonical list.
Both synthesis and planned-work answer submissions use this path; runtime-composed
synthesis is unaffected.

Paseo now keeps shell output when command input arrives in a later update, while
native reads remain metadata-only. Native Edit/Write fields are aliased for
presentation only, and worker-card projections preserve the first terminal
status across contradictory updates and replay. Qualification assertions now
require exact identity-bound confirmations and complete execution/output
metadata (not output-body verification); steering continuation stays pending
until matching capture and same-attempt progress are observed. Owner preferences
continue to suppress unrelated usage ranking when preferred routes disappear
from the catalog. Removed temporary qualification inspection helpers that could
print unrestricted payload text. No live or rendered Desktop qualification is
claimed by these no-paid changes.

A worker that steering left running no longer ends as Canceled in Paseo while it keeps
working. Paseo sends `session/cancel` before each follow-up prompt; the cancelled prompt's ACP
feed silently dropped every later update of the worker cards it had created, so the worker's
real outcome (and a lone Stop's cancellation) never reached the card and was never journaled.
`worker:` card updates now outlive their turn's cancellation; the turn's other updates still
stop with it. The Paseo 0.11.4 bridge settles a cancelled turn's open tool calls as `canceled`
(0.9.1 used `failed`); the provider's worker-card fence now withholds that settlement too.
Coverage is no-paid unit/provider tests plus a provider run against the bridge bundled with the
installed Paseo 0.11.4; no live, paid or rendered Desktop check is claimed.

The built-in default preset (`policy/model-settings-default.json`) is synced again from the
owner's slightly edited production `routing.json`: `codex-acp/gpt-6-astra`'s `preferred_for`
is `["design"]` once more (the generic design/architecture work kind), and `claude-agent-acp/opus[1m]`'s
configured `tiers` list is reordered strongest-first (`flagship`, `deep`, `standard`), matching
production byte-for-byte. Astra's tier membership stays flagship-only (never a deep-tier automatic
candidate) and `implementation_frontend_design` remains Opus-only; restoring Astra's generic `design`
preference does not change either rule. No other preset field differs from production. No live or
paid qualification is claimed for this change.

Coordinator `plan_submit` no longer refuses legitimate plans on the first attempt for the
patterns retained in real sessions. A read-only task may name a not-yet-existing literal path
when a writing task of the same plan whose scope overlaps it is ordered with it (a reviewer of
the directory its predecessor writer creates, or a task whose dependent writer creates the
target); an unordered or absent writer is still refused with `read_only_scope_not_writable`, now
listing every such path in `violations`. JSON `null` for an optional task field (`model_reason`,
`complexity`, `work_kind`) is treated as omitted. An unexpected field that the tool advertises
elsewhere (such as `verification` inside a task) gets a correction naming its location. The
advertised scope and optional-field descriptions state these rules. No live or paid
qualification is claimed for this change.

Implementation routing now distinguishes `implementation_backend`, `implementation_frontend`
(without design), and `implementation_frontend_design`. The optional plan-node `work_kind`
overrides role defaults and survives retries/fixes. Legacy `implementation` preferences,
standard allowances and affinity expand in memory without rewriting owner configuration.
The built-in preset makes `codex-acp/gpt-6-astra` flagship-only and lists frontend-with-design
only in `opus[1m]` preferences. Frontend-design automatic routing now preserves
structured preferences across quality retries, escalation and review fixes,
including eligible fallback tiers, independently of conditional hints.
Unavailable/exhausted preferred routes first fall back on the work kind's
affinity engine (Claude in the preset). Owner correction: only once that
engine has no authorized, available, non-exhausted route at all does selection
widen to the normal cross-engine candidates (Sol/Astra on Codex in the
preset); while the affinity engine still has one eligible route, no other
engine is ever chosen, including on retries, escalations and review fixes. No
route anywhere, or a neutral affinity, refuses visibly. Cross-engine pins are
always refused, never substituted, regardless of this fallback. Confirmed user
model requests still take precedence; hard authorization, quota and
tier/effort policy remain enforced in whichever engine pool applies. Rank
version 12 records the required affinity boundary and whether cross-engine
fallback occurred (`affinity.cross_engine_fallback`). Other work kinds retain
their existing conditional-hint rules. No live or paid qualification is
claimed for this change.
The Paseo plugin's Settings screen already drew its work-kind vocabulary, labels, affinity
choices and preferred-work switches from the runtime's configuration API, so it shows and edits
the three new kinds without a protocol change; its own static help text for the old
`implementation` kind is replaced with one entry per new kind, and the affinity help names them
by their exact ids. Settings edits the routing file's own raw rules, not the expanded effective
view, so saving an unrelated change keeps a stored legacy `implementation` affinity, preference or
standard allowance exactly as written. Such a stored value is shown as a checked
`implementation (legacy: …)` choice that the owner can turn off explicitly; it is never offered
for new selections.

A fix node created after a `REQUEST_CHANGES` review no longer keeps a read-only source role. The runtime gives it a writing role chosen from its effective scope: `worker-technical-writer` for documentation-only scopes, `worker-qa-engineer` for tests-only scopes and `worker-backend-dev` otherwise. A failed check of a worker's own standalone Python probe can be retracted by a later verified passing correction with the same name, operation, cwd and profile; the failure stays as `retracted` history, never as a pass, and runtime verification failures are never retracted. An `outside_command` identity now includes its profile, so an exact outside rerun closes its own failure, and a later unrelated recorded run no longer makes that rerun ambiguous. No live or paid qualification is claimed for these changes.

The 2026-10-14 orchestration audit's owner decisions are recorded in
[the decisions log](docs/project/decisions.md). Read-only roles, reviewers and
automatic runtime verification now run commands in one read-only host profile (receipts name it `read_only`) in every access
mode: HOME is read-only except an explicit tool-cache allowlist, scratch is private, and the
command has its own PID namespace. Inside a verification check only the project's git-ignored
directories are writable. Writing workers keep the access-mode profile. A reviewer's read-only
pass closes a writer's declared gap again in every mode. A mode switch no longer stops a read-only command or a verification check; only a host-writable
writing-worker command is stopped. The deletion classifier is lexical:
it recognises more commands than `rm`, and its tool text says so. No paid or live
qualification is claimed for these changes.

The runtime stop question's Finish now force-finishes the task: it ends `CANCELLED` with
`USER_FORCE_FINISHED`. Continue resumes the flow and never releases the stop. The stop question is recognised by a
structured runtime marker, not by its wording, so a coordinator question worded the same way never
force-finishes the task. A direct answer
that would close planned work which did not pass review and verification ends `FAILED` with
`answer_not_certified:<gap>`, never `COMPLETED`. Only an explicit `deny-write: <path>` creates
a write denial; a natural-language request such as "do not modify X" creates none, and a
wildcard value is not stored. A denial that an earlier runtime stored with a wildcard still guards
writes, matched as a glob. Denials match after path normalization, so a `./` spelling no longer
bypasses one. The outside-write refusal also resolves real paths and protects a worktree's git
metadata directories.

Automatic routing honours a user's model revocation in live dispatch (`revoked_by_user`).
Infrastructure failures (engine and transport failures), and workers lost to a runtime restart or host disconnect (`worker_lost`), retry without escalating the model
tier. Rank version 11 ranks a known usage at or above 95% behind every non-hot route, known or
unknown. Expired quota windows are pruned or expire, and a stretch exclusion no longer removes a
dual-tier model. A pinned effort that the policy does not permit is applied at the nearest
permitted step and recorded as `pin_effort_clamped`.

The senior consultant is reached only through `escalation_request`. The runtime builds its
packet from the reports the escalation names and the escalated revision's reports, each with its
SHA-256 and artifact versions. The consultant receives no `ask_question` or `access_request`, and
a plan that names it is refused.

In Paseo, a `session/cancel` followed by a `session/prompt` on the same session is steering: the
cancelled coordinator turn ends, the task is not paused, and running workers continue. A lone
cancel still pauses the task after a 5-second grace window; the pause notice appears on the next turn, and a failed deferred pause is recorded as a progress event. The Paseo plugin answers the bridge's
`session/close` locally, and the dev wrapper strips inherited `PASEO_*` variables.

The brokered MCP relay's handshake token is no longer in process argv or its environment. The
broker writes it to a 0600 `relay.token` file in its private socket directory, and the relay reads
it there without following symlinks. Claude's `--mcp-config` had carried the previous environment
entry on its command line.

A worker's first or continuation prompt carries its assignment objective exactly once. The
recovery capsule keeps the full brief verbatim and points to it from its other copies. Role
instructions delivered to engines omit generated skill frontmatter and footer comments. The
prompt generator asserts 23 profiles and fails on a dropped preamble. Skills no longer copy tool
argument contracts; the live tool schema is authoritative.

Paseo's plugin dependency `shell-quote` moved to 1.12.0, which fixes a critical advisory. Fifteen
high and five moderate advisories remain, transitively through React Native, and are recorded in
the dependency audit. Compact argv launches fall back to an unlinked temporary file where
`os.memfd_create` is unavailable.

Project file tools now follow the read-before-edit discipline of native editors
and refuse the partial-read overwrite that truncated a shared research file. A
text `project_read` pages by 1-based line (`offset` and `limit`, at most 2000
lines each; `line_numbers` defaults to true), keeps each result under 24,000
bytes and marks lines longer than 2000 characters as truncated. Offsets are
line numbers now, not character offsets, so callers that passed character
offsets must change. A truncated line never counts as read. Complete,
non-truncated coverage of a file is kept per channel as digests and line
ranges only, and `full_content_sha256` is returned only after that coverage.
`project_write` replaces an existing file only with that complete-coverage
digest: the `sha256` of a partial page identifies the page and never authorizes
an overwrite, and a supplied hash alone does not either. Creation still uses
`absent`. Receipts report previous and new bytes and lines plus a
`drastic_shrink` flag; a shrink is visible, not refused. The new `project_edit`
applies one to 64 ordered exact `old_string`/`new_string` replacements
atomically: each must match uniquely unless `replace_all` is true, an append
replaces a unique tail anchor, any prior text read on the channel with an
unchanged digest is enough, and the receipt carries a bounded diff. Own read
coverage is cleared after every mutation. The write path fsyncs the temporary
file before the rename and the parent directory after it. Focused gateway tests
are in `tests/test_project_file_tools.py`; no live or rendered qualification is
claimed.

Paseo's live chat echo for an answered Cortex question now reads
`Question: <question> → Answer: <answer>`, with the batch topic label in front
for a labelled batch member. Single, free-text ("Other...") and batch answers
each get their own line. Only the echo text changed: the recorded answer and
the durable replay event are unchanged. Unit tests cover the text; no rendered
GUI check was run.

The no-paid suite can run inside Cortex's verification sandbox without making
lifecycle tests depend on nested user namespaces. Test-only launch seams keep
real command outcomes and retention; actual bubblewrap tests probe once and
skip known namespace denial explicitly. Production containment stays fail-closed
with no unsandboxed fallback.

Claude ACP now has an exact 0.88.0 instruction-delivery registration for its bundled SDK 0.3.293. Discovery prefers the newest registered adapter and pruning preserves registered installs, so a background engine auto-update that installs a newer adapter without a registration no longer displaces the usable registered version. Critical dispatch rejects unsupported or non-retryably unqualified engine/version pairs rather than retrying another model on that adapter. Qualification is static/source and no-paid protocol evidence, plus one owner-requested paid `desktop` check on 2026-10-08 (`claude-agent-acp/claude-sonnet-5`, adapter 0.88.0): its resumed scenario completed without `instruction_delivery_unqualified`. The first canary attempt was operator-cancelled, one version diagnostic assertion remains an open failed check, and the native agentInfo version is unverified. Paid CLI mode was not run (not requested by the owner). This is bounded evidence, not clean or full qualification, paid native-tool enforcement or release readiness.

A worker that published its result and ended while a scheduler pass was
running is no longer mistaken for a lost worker. The pass read the plan nodes
before the live worker set, so a reviewer finishing in between looked
"running" with no live worker; `retry_lost` then failed with
`lost_worker_result_changed` and returned an approved task to the coordinator
for an extra plan revision (the intermittent composed-runtime test failures
under load, where an escalation's consultant node vanished from the final
revision). The live worker set is now read first. No paid/live qualification
is claimed.

Paseo's model Refresh now refreshes the Claude catalog too. Paseo reads models
from a Cortex ACP process it stops about a second after `session/new`; the
refresh started there ran on a thread of that process, and a Claude refresh
(one effort probe per model, about 12 s) was killed every time, so no Claude
snapshot was ever saved by Refresh. The refresh now runs in its own process
session (`catalog/detached_refresh.py`, one per connection at a time) and
saves its snapshot after the requester exits. The isolated dev profile's
stale routing (bare `haiku` denied) was updated to the current preset after a
private backup; its picker now lists Haiku 5.5 at Luna parity. The owner's
production runtime predates the `+sdk` build ordering and repoints Claude to
the upstream SDK 0.3.287 install at each start, where `haiku` is Haiku 4.5; it
needs the current runtime deployed. No paid/live qualification is claimed.

`result_submit` now takes `report` as one plain string: the complete Markdown
(optional `report_title`, else its first heading) or only the exact `report_id`
from the worker's own `write_report`. The former `report: {title, markdown}`
object made Claude engines hand-escape long nested Markdown; Sonnet 5 broke it in
about a quarter of large inline reports and Claude Code refused those calls
before Cortex received them, which was the recurring "submit result error".
Object values and identifier-shaped strings that are not an exact report id are
refused with a corrective `invalid_arguments` before anything is published. The
advertised check contract and `open_failed_check` correction now say that a
mistyped command line that never started the check is not a failed check (cite the
corrected command's own receipt); exact-argv closure is unchanged, so a different
argv still never closes a cited failure. No paid/live qualification is claimed.

The `result_submit` schema's `checks[].outcome` field description and the
top-level `result_submit` tool description worded that mistyped-command
guidance with "unknown-option"; under Codex code mode that description text
renders verbatim inside the generated TypeScript declaration, so the literal
substring "unknown" appeared there even though every field and description
otherwise survived intact. Both descriptions now say "unrecognized-flag"
instead, with no change in meaning. No paid/live qualification is claimed.

Codex ACP now supports exact 2.1.1 private-adapter patches, instruction delivery,
compaction and discovery, with a 2.1.1 fallback. Role metadata reaches all
qualified versions, not only 1.12.0; diagnostics report the current registrations.
The owner already had 2.1.1 installed (CLI 0.159.3), so the update path made no
reinstall. No-paid bundle and CLI/jail checks passed; no production Paseo/plugin
refresh or paid/live qualification is claimed.

Claude ACP now has an exact 0.87.0 system-instruction registration and fallback,
observed/grouped model-option handling, diagnostics, and reactive compaction
recognition for its new non-AIR metadata. Unknown installed Claude versions no
longer borrow another release's qualification. The original SDK 0.3.287 only
reports Haiku 4.5; the independently inspected SDK 0.3.293 refresh now installs
side by side and exposes `haiku` = Haiku 5.5, with old Haiku separately named.
No-prompt ACP controls accepted high/xhigh/max. The default preset and owner's
active production profile enable the new alias at Luna policy parity, associate
`seven_day_haiku`, and deny old Haiku by exact ID. Owner settings were backed up;
unrelated settings and the original install were preserved. No paid/live
qualification is claimed.

Model policy gains an optional owner-written `preferred_when` condition. The
coordinator sees it in `list_route_options.preferred_when` and names the model
on the plan nodes that meet it; only there does it rank as the owner's preferred
model. Settings now choose `preferred_for` and `also_standard_for` work kinds
from the list instead of free-text entries, which the runtime always rejected.

Orchestration now records coordinator/engine timing milestones and changed
scheduler wait reasons, with a read-only phase timeline for checkpointed stores.
Review can start while accepted worker engines wind down; unchanged-file digest
hashes are reused where filesystem metadata supports it. Sandboxed verification
may overlap a running reviewer, but only an unchanged approved binding can credit
its result; stale candidates are discarded and interrupted work is rerun through
ordinary verification. Engine prewarming remains deferred pending live timing
evidence; no performance gain is claimed.

Routing now honors an explicit user-requested model before other routing
preferences, subject to hard authorization and availability checks; an unusable
requested route is refused visibly rather than replaced. Usage-based model
switching activates only at observed usage of 95% or higher; unknown usage is
unavailable. Fresh installs now receive the owner's portable default routing,
coordinator preference and language, while existing settings and machine-local
account, connection and MCP bindings are not replaced or copied.

Added opt-in developer wire tracing for the isolated dev Paseo daemon. It records
redacted ACP, MCP, verification and lifecycle frames to private, capped files
outside project storage; it is off by default and refuses installed production
state. Trace data can still include sensitive content and must be treated as
confidential. No paid/live qualification is implied by these changes.

Updated prompt and access documentation for the four-role unified base prompt,
profile `access_class`, class-selected verification guidance, skill-layer splits
and shorter shared worker report protocol. Clarified that Auto intentionally
permits outside-project writes/commands without a card while Guarded requires
accepted consent; documented accepted jail exposure of host `/tmp`, engine state
directories and `/run/user`. The consultant remains normally routed; Codex native
safe reads may reach source despite the reports-only prompt/tool boundary. Also
aligned routing-hint, code-index, review-reuse and host inactivity-nudge guidance
with current behavior.


Persisted Cortex sessions in Paseo now reopen without reapplying cached model,
mode, effort or settings. A removed or denied saved model no longer prevents
read-only history loading; new and explicit configuration choices remain validated.

Coordinator live-stream redaction now preserves path-embedded script names, including
digit-only, hyphenated and underscored names, while continuing to redact credentials
split across stream chunks.

Cortex is an incoming ACP stdio agent with a project-local runtime and
outgoing Codex and Claude ACP engines. The 1.0.0 release resets the package
identity to a content-stamped ACP payload.

This revision adds durable requirements, assignments and attempts; isolated
worker snapshots; exact staged candidates; candidate-bound checks; recoverable
integration; and explicit revision continuation. Instruction and context packets
record their sources and actual delivery, including session replacement. Report
publication remains separate from worker completion and coordinator decision.

Claude workers work again: resolving the installed claude-agent-acp version
(0.81.0, which has no delivery registration of its own) made every Claude worker
refuse its first prompt while the coordinator kept working; an unregistered
installed version now resolves to the pinned one as before. A failed worker is
recorded with its error code and source line instead of a bare class name, and
a coordinator can read its own role through `read_skill`.

A long chat reopens again. Replay sent every update of a worker card, and each
update carried the card's whole log, so one long worker made 17.7 MB of history.
That overflowed Paseo's 16 MiB history buffer, and Paseo showed "Couldn't
refresh agent history" with no way to continue. Replay now sends each card's
final state once (0.36 MB for the same chat). The Paseo relay also exits when
Cortex exits, so a failed load reports an error instead of hanging.

Paseo development checks now use the persistent isolated `~/.paseo-dev` home
and `python3 -B scripts/cortex_paseo_dev.py` CLI wrapper. CLI log retrieval
validates only what the isolated daemon returns; it is not rendered GUI evidence,
and incomplete history must remain unverified. The observed clone cannot resolve
agent `6956c56` (`Unknown sessionId`) because that archived provider session is
absent from the clone's Cortex store. Long-history completeness, binding of the
`Completed with issues` status to the right worker card, and Codex shell-command
expansion remain unverified in a rendered client. Replayed history may still
show raw model identifiers such as `gpt-5.6-luna`.

The model-specific `gpt-5.6-sol` skill-editing rule was removed; model routing,
including prompt and skill work, is decided by the orchestration runtime.
