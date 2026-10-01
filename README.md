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

After a coordinator question commits, Cortex suppresses further text received
in that turn, including when tool lifecycle callbacks arrive late. Details
delivered only after the commit can also be omitted; see the
[question display boundary](docs/acp/orchestration.md#plan-submission-and-coordinator-polling).


## What Cortex is

ACP route/effort selection retains the owner and project ceilings and any
task-local narrowing. Automatic workers and reviewers use only eligible
observed routes within that ceiling; a denied pin is refused, not replaced.
See [route authorization](docs/acp/routing-and-fallback.md#pre-dispatch-enforcement).

Initial host model/provider echoes preserve the configured native effort/mode;
they are not parameter resets. [Bound task inspection](docs/acp/observability.md#bound-task-inspection)
provides paged, read-only lifecycle metadata and native confirmation references.
The [dev binding exporter](docs/acp/dev-paseo-environment.md#read-only-dev-binding) can observe one explicitly owned host agent and its guarded session/task/current-owner binding; it never starts a daemon or engine. It does not qualify a live run or replace independent review and verification.
Targeted tests can use the [fresh-process isolated launcher](docs/project/verification.md#fresh-process-isolated-targeted-launcher) with an explicitly authorized existing dependency path.

For `result_submit`, send exactly one of an inline `report` or the `report_id`
returned by this worker's `write_report` as `report_ref`. The advertised schema
specifies check fields and receipt sources; rejected result submissions attempt to retain
only a code and safe field path for diagnostics, not the submitted content.
Review workers must send a verdict (`APPROVE`, `REQUEST_CHANGES` or `BLOCK`;
lowercase forms normalize). A supplied `command` on a passed or failed check is
descriptive only; it never replaces the same-attempt `tool_call_ref` evidence.
A worker whose own changes pair documentation (`docs/**.md`, `README.md`,
`SECURITY.md`) with source or other behavior files must have read
`documentation-sync` through Cortex `read_skill` first; otherwise the result is
refused with `situational_skill_required` and nothing is recorded.

Cortex is an ACP v1 stdio agent. Its runtime coordinates Codex and Claude ACP workers in the project you opened, and owns task scheduling, review, verification, and completion. Task records and Markdown reports are stored in that project’s `.cortex/` directory. Worker briefs keep objectives, scope and dependency report references complete; long task goals and review evidence use marked previews with links to the full source. Plan verification commands are literal argv from documented project commands or scripts; shell syntax is rejected before work starts. The runtime runs them once after review, each until it exits, so the project's whole suite belongs there; workers run only the tests covering their own change. A worker check is marked passed only when its exact same-attempt `command_run` or user-confirmed `outside_command` receipt proves a complete, nontrivial command exited 0. Engine shell calls, reads and incomplete output cannot verify a check; older reports remain intact and weak historic evidence is marked separately when read.

To close a task immediately at your own decision, send `/cortex-finish` with no arguments, even while a Paseo turn is active. Cortex stops its owned work and records the closure as terminal `CANCELLED`, **not** verified `COMPLETED`; unfinished review, checks, and synthesis are not claimed as passed. An explicit close-as-is answer to a task-closure question has the same effect. See [completion and lifecycle](docs/acp/completion-and-lifecycle.md).

Paseo questions without preset choices still show a card: type the answer under
"Other..." or reply in chat. The instructional card action does not answer the
question. See [questions and steering](docs/acp/steering-and-questions.md).

ACP timeline updates use bounded display projections to avoid repeatedly sending growing worker transcripts. Worker cards keep a 20-action, 4 KiB tail for generic clients; per-update and field size limits and a fixed per-card event budget (forward-only status transitions plus at most 8 progress updates, one per 30 s, so at most 13 updates per worker whatever its size) bound host event volume. Coordinator cards send changed fields only, suppress no-op updates, omit an identical MCP text mirror, and reference repeated identical outputs; unchanged context polls return a compact receipt. Coordinator and engine tool cards also have a per-task total budget: the first 150 distinct calls (or 4 MiB of card updates) get their own card with at most 10 updates each; later calls are counted on one aggregate card, sent at most 20 times per task; and up to 50 failed calls past the budget keep their own cards. The budget persists across turns and restarts, and `session/load` replays older journals within the same bound. In the scale test, 1,500 coordinator calls over four turns and a restart sent 324 card updates and 187,517 bytes, against 312 updates for 500 calls. In the recorded scale test, repeated polls were 481–483 bytes, and each Paseo worker card sent 3 updates and 16,959 bytes for 10-, 60- and 300-tool workloads. These limits reduce emitted event volume; they do not cap durable task history or guarantee constant total daemon memory. See [ACP observability and resource limits](docs/acp/observability.md#bounded-acp-event-projections).

Automatic routing uses only explicit version-2 owner settings plus currently observed engine catalogs; no model identity receives a built-in tier, effort policy, or automatic membership by name. The coordinator receives route options from that same effective settings/catalog view. `list_route_options` preserves named policy migration/validation diagnostics and their corrective action, rather than reporting a storage failure. New or renamed model IDs remain ineligible until configured. Preferences rank only within already eligible routes and do not grant eligibility. See [configuration](docs/configuration.md#settings-only-model-policy) and [routing and fallback](docs/acp/routing-and-fallback.md#settings-only-routing).

Explicit user requests must name the complete case-sensitive `engine/model`
identity or an owner-configured request label. Keep an exact identity
whitespace-delimited or quote it (`Use "engine/unit".`); attached unquoted
punctuation is ambiguous identity data, never authorization for a shorter ID.
Plan hints alone cannot promote a flagship. See the routing guidance above.

New route audits use the neutral `stretch` policy key and `stretch:<tier>` effort basis for any configured stretch model; stored historical audits remain readable and are not rewritten.

Owner profiles now capture the complete supported set of user-level settings—not just routing—including defaults and owner ceiling, MCP usage policy and connection session limits. Save edits explicitly to the selected profile or save them as a new profile; activating one materializes its captured settings into the existing user configuration files. Project settings, MCP installation commands/arguments/environment and connection credentials are excluded. Project and `$CORTEX_ROUTING` files still shadow user routing. See [user routing profiles](docs/configuration.md#user-routing-profiles).

Within an eligible routing tier, owner `preferred_for` model choices now rank ahead of plan hints, plan-usage heuristics and retry diversification. Preferences do not grant a route: pins, deny rules, automatic model membership, providers, tier eligibility, availability and quota safety still apply.

The current Codex adapter is codex-acp 2.0.1 with Codex CLI 0.159.1, with a separately observed offline catalog. The installed Claude adapter is claude-agent-acp 0.84.0. Codex 1.12.0 and 1.13.0 remain supported fallbacks; later Codex adapters still need explicit compatibility support. This one-time update does not change the existing background installer. See [adapter compatibility](docs/acp/codex-acp-adapter.md).

OpenCode v2 (`opencode acp`, any `2.x.x` release) is an optional third engine.
It uses your native OpenCode logins and your configured custom providers
together, with Cortex's own `cortex` agent; no alternate database or extracted
login is used. Cortex removes OpenCode's built-in `build`/`plan` agent plugins so
that ACP offers `cortex`; see [native logins](docs/acp/opencode-adapter.md#native-logins-and-the-2020-catalog-snapshot).
It is never discovered at startup and runs only after the explicit
`python3 -B scripts/cortex_acp_setup.py add-opencode-connection` step. Automatic
worker routing skips it unless `routing.json` opts in. It runs only inside the
engine jail, in its own user/PID/IPC namespaces, with Cortex's own configuration
(one `cortex` agent that asks permission for every native tool) and the Cortex
gateway reached through its `execute` tool. Catalog refresh refuses observations
without the required `cortex` mode and preserves the previous snapshot. Imported
provider substitutions stay in read-only jail files, not process environment values. The
[OpenCode adapter page](docs/acp/opencode-adapter.md) lists what works, what
does not, and the no-paid evidence; no paid or live OpenCode run was made.

Claude Cortex sessions disallow native shell, file/search/edit, notebook edit and
Skill alternatives through adapter options, leaving the scoped Cortex MCP tools
and granted code-index MCP tools for that work. This does not change Codex's
native tool policy. Effective behavior in Claude 0.81.2 remains subject to a
fresh owner-authorized live check.

The unified role prompt keeps everyday Cortex procedures inlined. Conditional
procedures are named with action triggers in the role and `read_skill` tool
descriptions; the model is told to call Cortex MCP `read_skill` only when a
trigger arises, before that action. Documentation-impact planning as well as
affected edits trigger `documentation-sync`. The prompt distinguishes Cortex MCP
reports, skills, checks and submissions from external code-index MCP queries and
native workspace tools; argument contracts remain in the live tool schemas.
This is an instruction and advertised
contract, not proof that either model complies in a live turn. See the
[role delivery contract](docs/acp/roles-and-delegation.md#instruction-delivery).

## Install

### Paseo

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

Git installs run their declared preparation hook. Local directory installs use the checkout directly; use the setup helper for a managed local Paseo installation when needed. See [Paseo onboarding and diagnostics](docs/acp/packaging-and-onboarding.md).

The Paseo plugin shows per-worker context and token usage on worker cards. It does not add a separate provider plan-usage button.

### Other ACP clients

Register Cortex as an ACP stdio agent with this command, adapting the absolute path to your checkout:

```text
python3 -B /absolute/path/to/cortex-orch-acp/src/cortex_acp.py
```

Use the client’s ACP agent registration settings. See [the ACP documentation index](docs/acp/index.md).

## Configuration

See the [configuration guide](docs/configuration.md) for JSON configuration files and their locations, MCP server setup, engine connections, routing rules, and explicit version-2 per-model eligibility and policy settings. The plugin implements a Cortex Settings screen for supported owner and project configuration, including a Models section that switches individual models on or off for Cortex (OpenCode models start off) and refreshes every engine's model list without a prompt, model tiers, automatic-ranking membership, work-kind preferences and user routing profiles. A live Paseo 0.10.0 check did not find it in the rendered Settings navigation or command palette; do not assume it is reachable there. Cortex files and the project SQLite configuration row remain authoritative, and fields the typed form cannot safely represent are read-only rather than rewritten. See [the configuration guide](docs/configuration.md#manage-settings-in-paseo) for details and limitations. SSH from workers uses your own ssh-agent and `~/.ssh/config`; see [ssh from workers](docs/configuration.md#ssh-from-workers).

### Project skills

Codex can discover repository skills under `.agents/skills`; Claude Code discovers project skills under `.claude/skills`. Cortex's [`/skill-management <request>` workflow](docs/acp/skill-management.md) stores each skill in `.agents/skills/<name>/` and can expose it to Claude through a guarded relative symlink. Proactive skill proposals require an explicit user choice from runtime-validated approval and decline labels; reversed or ambiguous labels cannot authorize a change. Availability does not guarantee an engine will select or use a skill; that depends on the engine, its session policy, the model, and the task.

## Further documentation

- [Packaging and onboarding](docs/acp/packaging-and-onboarding.md): install, first-run setup, diagnostics
- [Configuration guide](docs/configuration.md): connections, routing, MCP servers, ssh
- [Codex adapter](docs/acp/codex-acp-adapter.md), [Claude adapter](docs/acp/claude-agent-acp-adapter.md) and the optional [OpenCode adapter](docs/acp/opencode-adapter.md): how Cortex runs each engine
- [Routing and fallback](docs/acp/routing-and-fallback.md): worker routing and multi-provider fallback
- [Project storage and recovery](docs/project/storage.md): task storage, format upgrades, recovery
- [Security](SECURITY.md): the sandbox and permission boundary
- [Project skill management](docs/acp/skill-management.md): the explicit `/skill-management <request>` command, user-approved skill proposals from repeated work, and guarded Codex/Claude skill layout
- [Verification guidance](docs/project/verification.md): check evidence rules and test commands
- [Orchestration and worker briefs](docs/acp/orchestration.md): scheduling, review and bounded assignment context
- [ACP documentation index](docs/acp/index.md): full reference


Quota-window model applicability is also settings-only: configure exact per-engine reported labels under `quota_windows`; missing/unmapped labels conservatively remain shared, never inferred from a model family. Typed controls and independent CLI migration are described in [configuration](docs/configuration.md#exact-quota-window-associations). Automated component/API tests do not establish installed settings-screen reachability or production adoption.

A crashed brokered Cortex MCP server is now detected separately from engine completion; an indeterminate channel loss cannot authorize route fallback or a same-session publication follow-up through closed tools. A replacement relay discards old readiness/tool evidence and cannot clear the known loss until its own complete `tools/list` is observed; delayed old replies and exits are fenced. Non-JSON replies and non-UTF-8 filenames no longer terminate stdio response serialization. These are implemented protections with no-paid regression coverage, not attribution of a particular production incident or proof of deployment. See [failure boundaries](docs/acp/failures-and-cancellation.md).

Paseo's native composer queue is distinct from Cortex's captured steering. With inspected 0.10.2 capabilities, the owner-selected workaround is default `steer` mode plus Ctrl/Cmd+Enter, native Edit, and Edit then clear text/attachments to cancel. Edit overwrites an existing draft; native enqueue trims surrounding whitespace; attachment restoration and reload persistence are not fully qualified. No Cortex Cancel/Edit controls were added; see [the ownership boundary](docs/acp/steering-and-questions.md#native-composer-queue-versus-captured-steering).
