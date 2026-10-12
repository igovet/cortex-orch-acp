<table>
  <tr>
    <td width="190" align="center" valign="middle">
      <img src="./assets/logo.png" alt="Cortex logo" width="156" />
    </td>
    <td valign="middle">
      <h1>Cortex</h1>
      <p><strong>Multi-agent coordination for software work, as an ACP agent.</strong></p>
      <p>
        You talk to Cortex in your editor. Cortex decides what to delegate,
        runs other coding agents (codex-acp, claude-agent-acp) as workers in
        the exact project opened in your editor, serializes writers, checks
        what they did, and records it. Tasks,
        one Markdown pipeline per task, and reports are kept on disk next to
        your project.
      </p>
      <p>
        <img src="https://img.shields.io/badge/Cortex-1.0.0-7c3aed" alt="Cortex 1.0.0" />
        <img src="https://img.shields.io/badge/Python-3.11%2B-3776ab" alt="Python 3.11+" />
        <img src="https://img.shields.io/badge/Protocol-ACP%20v1-111827" alt="Agent Client Protocol v1" />
        <img src="https://img.shields.io/badge/Storage-Markdown%20%2B%20SQLite-0f766e" alt="Markdown files and SQLite metadata" />
      </p>
    </td>
  </tr>
</table>

On a fully closed, unrepaired first-review path, Cortex can complete with a
[runtime-composed synthesis](docs/features/task-lifecycle/index.md#runtime-composed-synthesis)
instead of another coordinator model turn. Independent review, digest-bound
verification and evidence-gap gates remain mandatory. The final text separates
runtime metadata from quoted, attributed report summaries; pending decisions,
gaps or replans retain coordinator reconciliation. Registry exit bookkeeping shares
the completion transaction instead of opening a second SQLite writer; a restart
from unfinished reconciliation retries the same gates without completing twice.
Before composition, an already-owned post-result settlement/exit cleanup gets
at most ten seconds outside the SQLite transaction to confirm absence; timeout
or enduring ineligibility retains coordinator reconciliation.

After a coordinator question commits, Cortex suppresses further text received
in that turn, including when tool lifecycle callbacks arrive late. Details
delivered only after the commit can also be omitted; see the
[question display boundary](docs/features/orchestration/index.md#plan-submission-and-coordinator-polling).


Fresh-install model settings use the saved 2026-10-12 owner default's portable
routing policy, `codex-acp/gpt-6.1-sol` coordinator at high effort, and Russian
message language. Existing owner settings are never replaced. Account/connection
identities and machine-specific MCP definitions/paths are not cloned; see
[the install snapshot boundary](docs/configuration.md#fresh-install-owner-default-snapshot).

## Clear old project tasks from chat

Preview `/cortex-clear before 2026-10-01`, then use the displayed command ending
in `confirm` to delete old unprotected tasks. Dates mean strictly before
00:00 UTC; `7`, `7d` or `days 7` instead select retention days (0..36500).
The runtime handles this without a model call and normally protects loaded-session
and active tasks. To remove stale protected tasks, preview `/cortex-clear days 1 force`,
then use `/cortex-clear days 1 force confirm`. Force always keeps the invoking
session's task; it assumes other processes are already dead and does not kill them.
Cleanup also removes old unbound/orphan chat sessions, then runs SQLite `VACUUM`
after committed deletion and reports reclaimed bytes (or an honest vacuum failure).
Invalid input and busy-store races are refused. `/clear` is host-reserved, not a Cortex alias.
See [retention maintenance](docs/project/packaging.md#retention-maintenance)
for safety details and the owner-run CLI alternative.

Claude ACP's current fallback is 0.88.0, with an exact source-backed
instruction-delivery registration and bundled SDK 0.3.293. Discovery prefers
the newest registered install and retains it across future unqualified updates;
critical-role dispatch excludes unsupported engine versions instead of retrying another model on them. A paid `desktop` check on 2026-10-08 completed its resumed scenario on this adapter with `claude-agent-acp/claude-sonnet-5`; that is bounded evidence, not full qualification. SDK 0.3.293 resolves `haiku` to Haiku 5.5 and
advertises low/medium/high/xhigh/max. The default preset and production
`default` profile enable it at Luna policy parity; old Haiku remains denied
by exact ID. No-prompt ACP controls accepted high/xhigh/max. See [adapter support and limits](docs/features/claude-engine/index.md#current-support-0880-2026-10-08).
A Cortex runtime older than this checkout repoints Claude to the upstream SDK
0.3.287 install at each start, where `haiku` is Haiku 4.5; the owner's production
runtime was in that state on 2026-10-08. The catalog refresh a new session starts
now runs in its own process, so Paseo's Refresh also saves the Claude catalog; see
[catalog refresh lifetime](docs/features/claude-engine/index.md#catalog-refresh-lifetime-and-deployed-runtimes).
This is no-paid evidence, not live model or release qualification.

Codex ACP's current fallback and exact supported release is 2.1.1.
The owner already had it installed; checkout discovery now selects it instead
of 2.0.1. No-paid bundle and CLI/jail checks passed; the production Paseo plugin
was not refreshed. See [Codex support and limits](docs/features/codex-engine/index.md#current-support-211-2026-10-07).

## What Cortex is

When a reviewer requests the plan's exact verification command on the same
project digest, Cortex returns the runtime-produced receipt instead of rerunning
it. In-flight checks are awaited for at most five seconds; mismatches, unavailable
or discarded evidence and wait expiry execute normally. Independent diff review
and mandatory final verification remain unchanged. See
[reviewer verification reuse](docs/features/orchestration/index.md#reviewer-verification-reuse).

After an accepted worker/reviewer result, Cortex banks the in-flight prompt's
token report on a separate cleanup thread for up to 15 seconds before teardown
(a turn that ends sooner settles at once).
Missing reports stay unknown; capacity and writer locks stay reserved until
confirmed exit. See [usage settlement](docs/features/token-and-plan-usage/index.md#coverage-and-interrupted-turns).

ACP route/effort selection retains the owner and project ceilings and any
task-local narrowing. Automatic workers and reviewers use only eligible
observed routes within that ceiling; a denied pin is refused, not replaced.
See [route authorization](docs/features/routing-and-fallback/index.md#pre-dispatch-enforcement).

An explicit model requested by the user is the first routing priority, but it
never overrides authorization, availability, or supported parameters. If that
exact route cannot be used, Cortex reports the refusal instead of silently
substituting another model. Recorded requests also recognize an exact bare model
ID unique across enabled configured engines, with terminal prose punctuation.
Exact enabled IDs also accept direct `на/on/via/with` phrasing (for example
`Часть 2 на gpt-6-luna:`); a request verb in the same user message governs all
exact identities, while negations and hard policy still apply.
Shared bare IDs grant neither engine and the plan preview explains the ambiguity;
see [identity rules](docs/features/routing-and-fallback/index.md#settings-only-routing).
Usage-based ranking switches only at observed usage
of at least 95%; unknown usage remains unavailable, not assumed below threshold.

Initial host model/provider echoes preserve the configured native effort/mode;
they are not parameter resets. [Bound task inspection](docs/features/observability/index.md#bound-task-inspection)
provides paged, read-only lifecycle metadata and native confirmation references.
The [dev binding exporter](docs/project/dev-environment.md#read-only-dev-binding) can observe one explicitly owned host agent and its guarded session/task/current-owner binding; it never starts a daemon or engine. It does not qualify a live run or replace independent review and verification. Failures retain bounded local stage/process diagnostics and complete allowlisted inspector reasons without returning a binding. Missing endpoint metadata, helper startup/output failures and storage refusals remain distinct; earlier generic errors cannot identify their historical cause. Zero child exit alone is not complete binding evidence.
Targeted tests can use the [fresh-process isolated launcher](docs/project/verification.md#fresh-process-isolated-targeted-launcher) with an explicitly authorized existing dependency path.

Before `plan_submit`, inspect the live schema and measure complete bounded fields.
The objective property description exposes its Unicode code-point limit even
when a host renders only descriptions; rewrite overlong text without dropping
requirements. Rejection does not accept a plan or start workers.

For `result_submit`, `report` is required and is one plain string, never an
object: the complete report Markdown (optional `report_title`; otherwise its
first heading is the title), or only the `report_id` returned by this worker's
`write_report`. Claude engines emit a top-level string verbatim but hand-escape
object values as JSON, which broke long nested reports before Cortex received
the call. The advertised schema
specifies check fields and receipt sources; rejected result submissions attempt to retain
only a code and safe field path for diagnostics, not the submitted content.
Review workers must send a verdict (`APPROVE`, `REQUEST_CHANGES` or `BLOCK`;
lowercase forms normalize). A supplied `command` on a passed or failed check is
descriptive only; it never replaces the same-attempt `tool_call_ref` evidence.
Open failed checks return `open_failed_check` before publication or channel
revocation. Keep the failure and cite an exact later passing rerun, or explicitly
submit `assignment_outcome: "failed"`; changing argv does not close it. Accepted
receipts include `node_status`, not task acceptance.
The review brief includes a bounded runtime-facts block: verified executor
receipts, planned verification argv (not results), and changed-path hashes/sizes
versus the execution baseline. Worker report access, independent review and
mandatory digest-bound verification are unchanged; see
[reviewer handoff](docs/features/orchestration/index.md#reviewer-handoff).
Prepare the complete report before the first submission; a summary or verdict
alone is insufficient. Pass structured arguments, not hand-concatenated JSON.
Reviewers default to publishing the complete report inline with `result_submit`,
including the shared decision brief and explicit verdict. Other roles may prefer
the owned-draft publication route for code blocks or larger reports; it remains
available for genuine incremental editing. Decision-
brief items (including contradictions) are Markdown sections inside `markdown`,
never separate report fields. After an inline JSON failure, correct serialization
or use that draft route. This guidance does not
guarantee model compliance; see [publication rules](docs/features/publications-and-consistency/index.md).
A worker whose own changes pair documentation (`docs/**.md`, `README.md`,
`SECURITY.md`) with source or other behavior files must have read
`documentation-sync` through Cortex `read_skill` first; otherwise the result is
refused with `situational_skill_required` and nothing is recorded. Gateway writes
provide early shared-tree skill reminders; submission attributes only this node's
reserved paths or successful channel writes, excluding sibling writers' paths.
Native edits remain checked where authorship is attributable.
If a reminder's database lookup or observation fails after a write commits,
the successful write receipt carries `skill_hint_unavailable` instead of a write error.

A text `project_read` pages by 1-based line: `offset` is a line number, `limit` is at most 2000 lines, and `line_numbers` is on by default. Each result stays under 24,000 bytes. A line longer than 2000 characters is flagged as truncated and never counts as read. Complete coverage is kept per channel. `project_write` replaces an existing file only with the `full_content_sha256` from that complete read; a partial page's `sha256` identifies those bytes and never authorizes an overwrite. The new `project_edit` applies up to 64 ordered exact replacements atomically, after any prior text read of the unchanged file. An append replaces a unique tail anchor. Each successful write or edit clears the channel's read coverage. Receipts show previous and new size and lines and a `drastic_shrink` flag. A file with a line over 2000 characters cannot be fully read, so `project_edit` is the only route to change it.

Root `.gitignore` repair reads exact full text internally, never display pages. Edit receipts describe the protected committed bytes. Diff matching is input-budgeted; oversized inputs return `diff_omitted: "input_budget"` and `diff_truncated: true` without blocking the edit.

A missing permitted `project_read` path succeeds with `sha256: "absent"` for
creation and `parent_exists`; private/link/read-error boundaries remain enforced.
Creating a file (`expected_sha256: "absent"`) also creates its missing parent
directories, no-follow; `project_mkdir` likewise creates missing parents and
returns `created: false` for an existing directory. A file or symlink in the parent
chain is refused with `gateway_parent_not_directory`; a non-`absent` hash under a
missing parent is a `gateway_write_conflict`.
Project write/edit/mkdir/delete keys may be omitted: identical keyless retries reuse
their effect receipt, but a precondition refusal before any effect is not replayed
under its derived key. After correcting the precondition, retry the same keyless
write; explicit-key refusals still need a fresh key. `project_git` is context only,
not a check receipt. Plan scopes reserve writes, not reads. Worker capsules
separate the task/coordinator configuration from the attempt's confirmed route.
See [file operations](docs/features/permissions-and-containment/index.md).

Cortex is an ACP v1 stdio agent. Its runtime coordinates Codex and Claude ACP workers in the project you opened, and owns task scheduling, review, verification, and completion. Task records and Markdown reports are stored in that project’s `.cortex/` directory. Workers can ask assignment-local questions and request additional exact paths: the runtime parks only that worker and resumes its same healthy session after a real answer or validated access decision, without a replacement plan. A recovered channel without retained session metadata keeps its ordinary tools; questions use task-global recovery instead of promising same-session continuation. Access requests retry writer conflicts automatically with bounded delays, then grant or refuse without a user question; path, owner-policy and read-only refusals are immediate runtime decisions. Worker questions are reserved for genuine user decisions and require question text plus every option in the selected chat language (English or Russian); worker briefs and reports stay English. The runtime relays those strings verbatim, without automatic translation or a guarantee of model compliance. Answers never override policy or widen the OS jail; see [orchestration](docs/features/orchestration/index.md#worker-questions-and-additional-access). Worker briefs keep objectives, scope and dependency report references complete; long task goals and review evidence use marked previews with links to the full source. Plan verification commands are literal argv from documented project commands or scripts; shell syntax is rejected before work starts. The runtime runs them once after review, each until it exits, so the project's whole suite belongs there; workers run only the tests covering their own change. A worker check is marked passed only when its exact same-attempt `command_run` or user-confirmed `outside_command` receipt proves a complete, nontrivial command exited 0. Engine shell calls, reads and incomplete output cannot verify a check; older reports remain intact and weak historic evidence is marked separately when read.

To close a task immediately at your own decision, send `/cortex-finish` with no arguments, even while a Paseo turn is active. Cortex stops its owned work and records the closure as terminal `CANCELLED`, **not** verified `COMPLETED`; unfinished review, checks, and synthesis are not claimed as passed. An explicit close-as-is answer to a task-closure question has the same effect. See [completion and lifecycle](docs/features/task-lifecycle/index.md).

Questions from coordinator, worker, or runtime fallback flows use the same structured question contract: worker cards require meaningful selectable choices, and Paseo presents an answer form with an "Other..." field. Related questions can be grouped into one paginated form and submitted together; every member must be answered. Tabs use short topics rather than ids; colliding labels gain a bounded numeric suffix, and header-keyed answers map back to exact question identities. Text fallback numbers follow the current open-question order used by `/cortex-answer`, with blank lines between questions; remaining questions renumber after an answer. Free-text answers and chat replies remain available, and dismissing a form does not answer or close the pending question. Hosts without the required structured-question UI may present the prompt and choices as chat text instead. After an answer is recorded, the Paseo chat echoes the pair as `Question: <question> → Answer: <answer>`, one line per batch member and prefixed with its topic when it has one. This is a runtime/UI contract, not proof that a model will always recognize when it should ask or that every ACP host renders a form. See [questions and steering](docs/features/steering-and-questions/index.md).

A real answer to a runtime recovery question starts a fresh bounded planning
cycle, including when answered within the same host turn. The three-prompt
limit still applies; an answer does not manufacture a missing model proposal.
A repaired recovery-counter defect had previously caused follow-up recovery
answers to reblock without another engine prompt; a focused regression now
checks that an answered recovery can continue into a scripted plan. This does
not explain or prevent a model from failing to submit a proposal within its
three prompts, and it is not evidence that every live task will complete.
A prior isolated Desktop run remains partial: its coordinator question rendered
and its answer mapped, but the task stopped at `structured_proposal_missing`;
batch and worker scenarios were not reached. See
[question recovery verification](docs/project/verification.md#question-and-submission-regressions).

When a planned step cannot start because another worker holds overlapping paths (in this chat or another chat of the same project) or the chat's worker slots are full, the chat shows a "Waiting" card naming the holder, the overlapping paths and when the wait began; it closes when the step starts. While the holder is a worker observed alive, the task keeps waiting rather than stopping on the 30-minute scheduler deadline. See [orchestration](docs/features/orchestration/index.md).

ACP timeline updates use bounded display projections to avoid repeatedly sending growing worker transcripts. Worker cards keep a 20-action, 4 KiB tail for generic clients; per-update and field size limits and a fixed per-card event budget (forward-only status transitions plus at most 8 progress updates, one per 30 s, so at most 13 updates per worker whatever its size) bound host event volume. Coordinator cards send changed fields only, suppress no-op updates, omit an identical MCP text mirror, and reference repeated identical outputs; unchanged context polls return a compact receipt. Coordinator and engine tool cards also have a per-task total budget: the first 150 distinct calls (or 4 MiB of card updates) get their own card with at most 10 updates each; later calls are counted on one aggregate card, sent at most 20 times per task; and up to 50 failed calls past the budget keep their own cards. The budget persists across turns and restarts, and `session/load` replays older journals within the same bound. In the scale test, 1,500 coordinator calls over four turns and a restart sent 324 card updates and 187,517 bytes, against 312 updates for 500 calls. In the recorded scale test, repeated polls were 481–483 bytes, and each Paseo worker card sent 3 updates and 16,959 bytes for 10-, 60- and 300-tool workloads. These limits reduce emitted event volume; they do not cap durable task history or guarantee constant total daemon memory. See [ACP observability and resource limits](docs/features/observability/index.md#bounded-acp-event-projections).

Automatic routing uses only explicit version-2 owner settings plus currently observed engine catalogs; no model identity receives a built-in tier, effort policy, or automatic membership by name. The coordinator receives route options from that same effective settings/catalog view. `list_route_options` preserves named policy migration/validation diagnostics and their corrective action, rather than reporting a storage failure. New or renamed model IDs remain ineligible until configured. Preferences rank only within already eligible routes and do not grant eligibility. See [configuration](docs/configuration.md#settings-only-model-policy) and [routing and fallback](docs/features/routing-and-fallback/index.md#settings-only-routing).

Explicit user requests must name the complete case-sensitive `engine/model`
identity or an owner-configured request label. Keep an exact identity
whitespace-delimited or quote it (`Use "engine/unit".`); attached unquoted
punctuation is ambiguous identity data, never authorization for a shorter ID.
Plan hints alone cannot promote a flagship. Confirmed requests in recorded
user messages/answers select that exact model first on covered nodes in every
tier, including retries/fixes, ahead of pins, preferences and usage ranking.
Hard policy or unavailable routes refuse visibly rather than substitute.
Usage switching engages only at observed usage >=95%; below it normal selection
applies, and unknown usage is unavailable. See the routing guidance above.

New route audits use the neutral `stretch` policy key and `stretch:<tier>` effort basis for any configured stretch model; stored historical audits remain readable and are not rewritten.

Owner profiles now capture the complete supported set of user-level settings—not just routing—including defaults and owner ceiling, MCP usage policy and connection session limits. Save edits explicitly to the selected profile or save them as a new profile; activating one materializes its captured settings into the existing user configuration files. Project settings, MCP installation commands/arguments/environment and connection credentials are excluded. Project and `$CORTEX_ROUTING` files still shadow user routing. See [user routing profiles](docs/configuration.md#user-routing-profiles).

Within an eligible routing tier, owner `preferred_for` model choices now rank ahead of plan hints, plan-usage heuristics and retry diversification. Preferences do not grant a route: pins, deny rules, automatic model membership, providers, tier eligibility, availability and quota safety still apply.

A model's preference can also carry an owner-written `preferred_when` condition (for example "frontend changes without design work"). The coordinator judges each plan node against it and names the model only where it holds; elsewhere the model is an ordinary candidate. In Settings, preferred work kinds are now chosen from the work-kind list instead of typed. See [model policy](docs/configuration.md).

The current Codex adapter is codex-acp 2.0.1 with Codex CLI 0.159.1, with a separately observed offline catalog. The supported Claude fallback is claude-agent-acp 0.88.0. Codex 1.12.0 and 1.13.0 remain supported fallbacks; later Codex adapters still need explicit compatibility support. This one-time update does not change the existing background installer. See [adapter compatibility](docs/features/codex-engine/index.md).

OpenCode v2 (`opencode acp`, any `2.x.x` release) is an optional third engine.
It uses your native OpenCode logins and your configured custom providers
together, with Cortex's own `cortex` agent; no alternate database or extracted
login is used. Cortex removes OpenCode's built-in `build`/`plan` agent plugins so
that ACP offers `cortex`; see [native logins](docs/features/opencode-engine/index.md#native-logins-and-the-2020-catalog-snapshot).
It is never discovered at startup and runs only after the explicit
`python3 -B scripts/cortex_acp_setup.py add-opencode-connection` step. Automatic
worker routing skips it unless `routing.json` opts in. It runs only inside the
engine jail, in its own user/PID/IPC namespaces, with Cortex's own configuration
(one `cortex` agent that asks permission for every native tool) and the Cortex
gateway reached through its `execute` tool. Catalog refresh refuses observations
without the required `cortex` mode and preserves the previous snapshot. Imported
provider substitutions stay in read-only jail files, not process environment values. The
[OpenCode adapter page](docs/features/opencode-engine/index.md) lists what works, what
does not, and the no-paid evidence; no paid or live OpenCode run was made.

Claude Cortex sessions disallow native shell, file/search/edit, notebook edit and
Skill alternatives through adapter options, leaving the scoped Cortex MCP tools
and granted code-index MCP tools for that work. This does not change Codex's
native tool policy. Effective behavior in Claude 0.81.2 remains subject to a
fresh owner-authorized live check.

Project/user skills listed in the session prompt are readable through Cortex
MCP `read_skill` using exactly the displayed `SKILL.md` path. The runtime
binds each coordinator or worker channel to its own observed engine session;
fallback and recovery use fresh channels and catalogs. Unlisted paths and
revoked or stale session bindings fail closed. See
[skill reads](docs/features/unified-system-prompt/index.md).

The unified role prompt keeps everyday Cortex procedures inlined. Conditional
procedures are named with action triggers in the role and `read_skill` tool
descriptions; the model is told to call Cortex MCP `read_skill` only when a
trigger arises, before that action. Documentation-impact planning as well as
affected edits trigger `documentation-sync`. The prompt distinguishes Cortex MCP
reports, skills, checks and submissions from external code-index MCP queries and
native workspace tools; argument contracts remain in the live tool schemas.
This is an instruction and advertised
contract, not proof that either model complies in a live turn. See the
[role delivery contract](docs/features/roles-and-delegation/index.md#instruction-delivery).

## Install

### Paseo

The plugin lockfile currently retains an unpatched `braces` advisory propagated
through React Native tooling (15 npm package entries, one advisory). See the
[dependency disposition](docs/project/paseo-dependency-audit.md); passing plugin tests
or listing packages as dev dependencies does not mean a clean security audit.

If rendering fails in a plugin panel, the composer popover, or a timeline card, Paseo shows a themed fallback for that surface with reopen guidance. If the thrown value cannot safely provide text, the fallback uses `unknown error`.

When reopening a persisted Cortex session, Paseo uses the configuration restored by Cortex instead of reapplying cached composer selections. New-session selections and explicit later configuration requests remain validated.

Paseo installs can prepare Cortex automatically: the plugin manifest’s supported `build` hook runs the setup command before activation.

Install the current GitHub version:

```bash
paseo plugin install github:igovet/cortex-orch-acp:integrations/paseo
```

Install from a branch, tag, or commit by adding `--ref`:

```bash
paseo plugin install github:igovet/cortex-orch-acp:integrations/paseo --ref dev
```


Apply plugin updates between active tasks until session recovery has been qualified for the version being deployed. A transport disconnect preserves task state and recovery reconciles saved Cortex state, but a host `SIGKILL` without `SIGTERM` cannot be recorded, and native engine actions do not have retained command receipts.

On stdin EOF or a failed stdout write, Cortex waits for the corresponding reader or writer disconnect callback to finish before shutting down the runtime; observing the transport event alone does not prove the disconnect record is complete. A failed stdout write does not wait for a reader blocked on still-open stdin.

Update an installed Git plugin:

```bash
paseo plugin update cortex-acp --check
paseo plugin update cortex-acp --ref dev --yes
```

For a local checkout, install the plugin directory by its absolute path and reload it after editing:

```bash
paseo plugin install /absolute/path/to/cortex-orch-acp/integrations/paseo
paseo plugin reload cortex-acp
```

Git installs run their declared preparation hook. Local directory installs use the checkout directly; use the setup helper for a managed local Paseo installation when needed. See [Paseo onboarding and diagnostics](docs/project/packaging.md).

The Paseo plugin shows per-worker context and token usage on worker cards. It does not add a separate provider plan-usage button.
Post-result releases with no reported tokens use existing usage-record columns,
so existing stores need no schema migration. Paseo omits their false token alarm;
genuine unknown/failed calls remain visible. Missing usage is not zero/free.
Read-only timeline aggregates separate token coverage by engine and role;
coordinator subtotals never fill missing worker or reviewer cost. Complete call
reporting is not complete token expenditure: last-request-only adapter totals
remain reported subtotals, not whole verified-result cost. Synthesis aggregates
count unfinished reconciliation separately and withhold its share while pending;
see [synthesis outcomes](docs/features/observability/index.md#runtime-composed-synthesis-share-and-fallback-reasons).
See [usage attribution](docs/features/observability/index.md#2-usagecost-attribution-observabilityusage).

### Other ACP clients

Register Cortex as an ACP stdio agent with this command, adapting the absolute path to your checkout:

```text
python3 -B /absolute/path/to/cortex-orch-acp/src/cortex_acp.py
```

Use the client’s ACP agent registration settings. See [the ACP documentation index](docs/project/architecture.md).

## Configuration

See the [configuration guide](docs/configuration.md) for JSON configuration files and their locations, MCP server setup, engine connections, routing rules, and explicit version-2 per-model eligibility and policy settings. The plugin implements a Cortex Settings screen for supported owner and project configuration, including a Models section that switches individual models on or off for Cortex (OpenCode models start off) and refreshes every engine's model list without a prompt, model tiers, automatic-ranking membership, work-kind preferences and user routing profiles, with per-field help and recommendations in English, Russian or both. A live Paseo 0.10.0 check did not find it in the rendered Settings navigation or command palette; do not assume it is reachable there. Cortex files and the project SQLite configuration row remain authoritative, and fields the typed form cannot safely represent are read-only rather than rewritten. See [the configuration guide](docs/configuration.md#manage-settings-in-paseo) for details and limitations. SSH from workers uses your own ssh-agent and `~/.ssh/config`; see [ssh from workers](docs/configuration.md#ssh-from-workers).

### Project skills

Codex can discover repository skills under `.agents/skills`; Claude Code discovers project skills under `.claude/skills`. Cortex's [`/skill-management <request>` workflow](docs/features/skill-management/index.md) stores each skill in `.agents/skills/<name>/` and can expose it to Claude through a guarded relative symlink. Proactive skill proposals require an explicit user choice from runtime-validated approval and decline labels; reversed or ambiguous labels cannot authorize a change. Availability does not guarantee an engine will select or use a skill; that depends on the engine, its session policy, the model, and the task.

## Further documentation

- [Packaging and onboarding](docs/project/packaging.md): install, first-run setup, diagnostics
- [Configuration guide](docs/configuration.md): connections, routing, MCP servers, ssh
- [Codex adapter](docs/features/codex-engine/index.md), [Claude adapter](docs/features/claude-engine/index.md) and the optional [OpenCode adapter](docs/features/opencode-engine/index.md): how Cortex runs each engine
- [Routing and fallback](docs/features/routing-and-fallback/index.md): worker routing and multi-provider fallback
- [Project storage and recovery](docs/project/storage.md): task storage, format upgrades, recovery
- [Security](SECURITY.md): the sandbox and permission boundary
- [Project skill management](docs/features/skill-management/index.md): the explicit `/skill-management <request>` command, user-approved skill proposals from repeated work, and guarded Codex/Claude skill layout
- [Verification guidance](docs/project/verification.md): check evidence rules and test commands
- [Orchestration and worker briefs](docs/features/orchestration/index.md): scheduling, review and bounded assignment context
- [ACP documentation index](docs/project/architecture.md): full reference


Quota-window model applicability is also settings-only: configure exact per-engine reported labels under `quota_windows`; missing/unmapped labels conservatively remain shared, never inferred from a model family. Typed controls and independent CLI migration are described in [configuration](docs/configuration.md#exact-quota-window-associations). Automated component/API tests do not establish installed settings-screen reachability or production adoption.

A crashed brokered Cortex MCP server is now detected separately from engine completion; an indeterminate channel loss cannot authorize route fallback or a same-session publication follow-up through closed tools. A replacement relay discards old readiness/tool evidence and cannot clear the known loss until its own complete `tools/list` is observed; delayed old replies and exits are fenced. Non-JSON replies and non-UTF-8 filenames no longer terminate stdio response serialization. These are implemented protections with no-paid regression coverage, not attribution of a particular production incident or proof of deployment. See [failure boundaries](docs/features/failures-and-cancellation/index.md).

Captured image steering includes a `read_source` reference and hint, including
an identical retry after source capture committed but before steering was recorded.
Retries preserve one steering event and requirement revision; they do not edit history.

Owner-uploaded Paseo files referenced by absolute path or file URI are retained
as task attachments. Workers use `list_sources`/`read_source`, not an
outside-project `project_read`. Capture is read-only and confined to the
launching Paseo home's uploads directory, with regular-file, containment and
size checks; rejected references retain diagnostics. See
[uploaded-file sources](docs/features/input-history-and-replay/index.md#paseo-uploaded-files).

Paseo's native composer queue is distinct from Cortex's captured steering. In default `steer` mode, Enter delivers immediately, so that chat bubble has no Edit icon. Use Ctrl/Cmd+Enter while Cortex works, then the **pencil on the queued chip above the composer** to edit before delivery; Edit then clear text/attachments cancels the draft. Cortex explains this after the first successful busy-turn send and refuses changed payloads reusing an already-captured message id; send a new correction instead. Edit overwrites an existing draft; native enqueue trims surrounding whitespace. Installed 0.10.2 helper tests cover edit/resubmit and consumed identities, not rendered GUI, complete attachment restoration or reload persistence. No simulated Cortex Cancel/Edit controls were added; see [the ownership boundary](docs/features/steering-and-questions/index.md#native-composer-queue-versus-captured-steering).

## Runtime store ignore protection

Root `.gitignore` writes through Cortex keep a final `/.cortex/` rule,
including after negations. Command edits are reconciled at completion and
native edits before a fresh writable worker result is published; there is no
universal native pre-action hook. See [the boundary and tracked-file limits](docs/features/permissions-and-containment/index.md#runtime-store-ignore-protection).

Independent review can start once all execution results are accepted, while
finished workers' engine turns wind down. Their MCP authority is revoked at
acceptance; capacity remains occupied until confirmed exit. Review and completion
remain bound to project digests. Verification can overlap the running reviewer;
only an unchanged, approved binding credits its result. Stale or rejected
candidates are discarded, and stops cancel their command sequence. Execution-start
metadata alone never closes a deferred check; only authoritative verification on
the approved digest can do so, with exact argv and the same gap-author and current
project digest regardless of execution order. The retained output body must also
be safely readable at its exact recorded length; metadata alone is insufficient. See
[orchestration](docs/features/orchestration/index.md#speculative-verification-during-review).
Project digest captures reuse bounded in-memory hashes of unchanged files on
recognized local filesystems without changing the manifest or drift gates;
see [project digest caching](docs/features/workspaces-and-integration/index.md).

## Orchestration timing

Use `python3 -B scripts/cortex_timeline.py --project "$PROJECT" --task "$TASK" --json`
for a read-only, metadata-only phase breakdown of a checkpointed project store.
Missing milestones stay missing; worker overlaps are not summed. Adding `--aggregate` (optionally with repeated `--task`) prints counts, ratios and decision-gate inputs in separate engine/route/effort cohorts; incomplete chains and missing approved inputs withhold verdicts rather than becoming zero. The reinvestigation gate counts dependent-attempt reads only, over all eligible worker execution time; independent repeats remain diagnostic. The owner runs it outside the worker sandbox on a checkpointed store.
See [timing observations and limits](docs/features/observability/index.md#orchestration-phase-measurement).

Developer debugging can opt in to private dev-only wire traces through the
guarded wrapper, only when starting the isolated dev daemon:

```bash
python3 -B scripts/cortex_paseo_dev.py --wire-trace -- daemon start --timeout 60 --json
python3 -B scripts/cortex_trace_view.py --task "$TASK" --pretty
```

Tracing is off by default; ordinary wrapper launches strip inherited opt-in.
The guarded switch is accepted only for `~/.paseo-dev` daemon start, and the
runtime refuses installed production payload/state. Redacted traces live in
private capped files below the dev home, outside project stores and Git; stop
the dev daemon to avoid concurrent rotation while analyzing exact history.
Records can contain sensitive project/model content even after redaction, so
treat the files as confidential and remove them only under the dev-home's
normal retention procedure. See [wire-trace boundaries and limits](docs/features/observability/index.md#developer-only-wire-tracing).
