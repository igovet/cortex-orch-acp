# OpenCode v2 outgoing adapter (optional)

Cortex can run [OpenCode v2](https://opencode.ai/v2/docs/cli/acp/) (`opencode acp`) as a third outgoing ACP engine, next to Codex and Claude. It is **optional and fail-closed**:

- startup never discovers it or creates a connection for it;
- you onboard it with one explicit setup step;
- automatic worker routing never picks it unless `routing.json` opts in (a `models` selection list or an exact `auto_allow` entry);
- only OpenCode **2.x.x** is accepted (1.x and 3.x are refused), and it runs only inside the engine jail with process isolation.

Source: `engine/opencode_install.py` (binary trust and version), `engine/opencode_adapter_acp.py` (configuration, prompt, option mapping), plus OpenCode branches in `engine/process.py`, `engine/session_manager.py`, `orchestration/sandbox.py`, `delegation/live_adapters.py`, `catalog/refresh.py`, `routing/auto_route.py` and `scripts/cortex_acp_setup.py`.

## Onboarding

1. Install OpenCode with its own installer (the binary normally lands in `~/.opencode/bin/opencode`) and log in once in a terminal: `opencode auth login`. The login is stored in `~/.local/share/opencode`: OpenCode 2.x keeps it in `opencode.db` there, while the connection state still checks for the `auth.json` marker that OpenCode 1.x wrote.
2. Cortex requires that directory to be private. If `status` or the step below reports the connection `invalid`, run `chmod 700 ~/.local/share/opencode`. Cortex never changes it for you.
3. Register the connection only after explicitly choosing to enable this optional engine. The plugin implements a [Paseo Cortex Settings screen](../configuration.md#manage-settings-in-paseo) for discovery and onboarding, but a live Paseo 0.10.0 check did not find that screen in Settings or the command palette. If it is not available in your host, use the setup CLI below. Onboarding requires an explicit opt-in, never copies the existing owner login, and does not change routing. When the Settings panel is reachable, a successful “Enable OpenCode v2” shows a banner naming the connection’s exact next step (for example, making the login directory private) directly from the onboard response; it never shows the credential directory’s path, since Cortex never surfaces credential paths to the plugin layer.

```bash
python3 -B scripts/cortex_acp_setup.py add-opencode-connection
# or, for a specific binary:
python3 -B scripts/cortex_acp_setup.py add-opencode-connection --binary /absolute/path/to/opencode
```

The CLI step looks at `--binary`, `$CORTEX_OPENCODE_BINARY`, `~/.opencode/bin/opencode` and `opencode` on `PATH`, in that order. The Paseo API deliberately does not accept an arbitrary binary path. Each candidate is resolved to its real file, then checked:

- the path must be absolute and have no symlink component;
- the file must be owned by you or root and writable by nobody else;
- its version, read by `opencode --version` inside a network-less sandbox with a throwaway HOME, must be in the `2.x.x` range.

The connection references your own OpenCode login in place: it is never copied, and no test login is ever made. The step prints the connection ID, its state and the next command:

- `opencode auth login` when the login is missing;
- the `chmod` when the directory is not private;
- `refresh-catalog --connection <id>` when the connection is ready.

`python3 -B scripts/cortex_acp_setup.py status` also reports a found OpenCode binary under `discovered_engines.opencode`, marked `optional` and `qualified`.

The catalog fills the way it does for the other engines:

- on each session open, from the `configOptions` OpenCode reports;
- on demand, with `refresh-catalog`, which opens a prompt-free session inside a read-only jail.

Efforts are read per model, because OpenCode offers different effort values per model and none at all for some.

OpenCode observations must also offer `cortex` in the `mode` selector before
either refresh or session capture saves a snapshot. Otherwise refresh reports
`opencode_cortex_mode_not_advertised` and leaves the previous snapshot untouched;
session capture likewise keeps the prior catalog. An advertised model list alone
does not establish usable Cortex routes.

Registering the connection is not enough by itself: OpenCode contributes no models to the selector until its catalog holds at least one successful refresh (there is no seed for it, unlike the built-in engines). A `configured` connection with no catalog snapshot yet is exactly as invisible in the model selector as an `invalid` one. After a `configured` state, run `refresh-catalog` (above) or open a new Cortex session, which also triggers a background refresh; only then does a new session’s model list include OpenCode.

## What Cortex configures

Cortex builds OpenCode's whole configuration itself. It passes that configuration in `OPENCODE_CONFIG_CONTENT` (see `build_opencode_config`):

- **One agent, `cortex`.** It is configured as the default and only agent: `build`, `plan` and the `title` helper are disabled. OpenCode exposes agents as its `mode` option, so the Cortex mode floor for both coordinator and worker is `cortex`.
- **Built-in agent plugins removed.** `plugin` holds only removals: `-opencode.agent` and `-opencode.plan`. Those built-in plugins create `build`, `plan` and OpenCode's helper agents (`general`, `explore`, `compaction`, `title`, `summary`). No plugin is loaded. See [native logins](#native-logins-and-the-2020-catalog-snapshot) for why.
- **Permissions.** `"*": "ask"`: every native tool call becomes an ACP `session/request_permission` that Cortex's permission bridge answers. `task` (its `subagent` tool), `question` and `skill` are denied, which removes those tools.
- **Your providers, nothing else of yours.** `autoupdate` is off and `share` is disabled. No model or MCP server is configured. Provider definitions come from your configuration; OpenCode reads your native logins itself (below). The model is set per session through `session/set_config_option`, confirm-or-fail. MCP servers are passed only on `session/new`.

OpenCode also reads your `~/.config/opencode` and the project's `opencode.json`/`.opencode/`. The jail hides the first behind an empty private tmpfs. `OPENCODE_DISABLE_PROJECT_CONFIG=1` turns off the second. `OPENCODE_DISABLE_AUTOUPDATE=1` is always set.

A custom provider (for example an OpenAI-compatible endpoint with its own `baseURL` and model list) is defined only in that hidden directory, so Cortex carries it over itself (`owner_provider_config`):

- **What is taken.** Exactly the `provider` definitions and the `enabled_providers`/`disabled_providers` lists of `config.json`, `opencode.json` and `opencode.jsonc` (JSONC allowed), merged in that order as OpenCode does. No agent, permission, MCP server, plugin, command or instruction. A provider Cortex itself names wins.
- **What is left out.** A provider whose `npm` package is not an `@ai-sdk/` package (that would be code loaded on your behalf), one with an unreadable `{file:…}` reference or a substitution that cannot be resolved, and everything once the imported part exceeds 64 KiB.
- **Credentials and substitutions.** Any API-key, token, secret, password, authorization or cookie field and every `headers` value is never put in the environment. Cortex resolves `{env:NAME}` from its own environment and `{file:path}` (`~/`, absolute, or relative to that directory), mounts each expanded string read-only from memory into the jail and names it `{file:<path>}`, exactly like the role prompt. This applies regardless of field name, including `accessKeyId`, `privateKey`, nested values and public fields such as `baseURL`. Empty credential or substituted values are dropped. Unknown literal fields cannot be classified as credentials; use substitutions for provider-specific credential fields.
- **Observable.** The engine observation `opencode_owner_providers` lists imported provider names (`imported`) and skip reasons, never values.

### Native logins and the 2.0.20 catalog snapshot

OpenCode opens the owner's existing `opencode.db` in its normal data directory
(`~/.local/share/opencode`). Cortex does not read credential rows, extract keys,
copy logins, change the data directory or set `OPENCODE_DB`. Native OpenCode
session state and logins share that database. Configured provider definitions
still use the containment import above; arbitrary owner agents, MCP and plugins
are not loaded.

The former alternate-database workaround was removed at the owner's request.
If an earlier build created `~/.local/share/opencode/cortex-opencode.db` (and
`-wal`/`-shm`), those files are no longer used. Cortex does not remove them; they
may contain previous Cortex/OpenCode session history.

`opencode acp` 2.0.20 is a thin client of a private `opencode serve` child. It
builds each directory's `session/new` catalog once, as soon as the server lists
any enabled model and any primary agent, and memoizes it for the process. Later
`session/set_mode`, `session/set_config_option`, `session/new` and
`session/load` validate against that same snapshot. A native login makes its
provider's models exist when the directory boots. Configuration (the `cortex`
agent, custom providers) is applied by OpenCode's own configuration plugins
about 150-200 ms later. The built-in `build` agent used to exist at once, so the
snapshot held `build`/`plan` and only logged-in providers.

Cortex therefore removes the two built-in plugins that create `build` and `plan`
(`plugin: ["-opencode.agent", "-opencode.plan"]` in its own
`OPENCODE_CONFIG_CONTENT`). The server's plugin supervisor reads configured
plugin removals before it starts any plugin, so no built-in primary agent ever
exists. The ACP catalog waits (up to OpenCode's own 5 s) for the configured
`cortex` agent; by then the owner's configured providers have been applied too.
If `cortex` never appears, `session/new` fails and the catalog and mode guards
still refuse the observation; Cortex never runs as a built-in agent.

A merged configuration alone does not work. The owner's `opencode.jsonc` merged
with Cortex's agent and delivered through every channel at once (global
`opencode.jsonc`, `OPENCODE_CONFIG`, `OPENCODE_CONFIG_CONTENT`, a project
`opencode.json` and `.opencode/agent/cortex.md`) still produced `build`/`plan`
and no custom provider (3 of 3 prompt-free probes). All of those sources feed
the same configuration plugins, which settle after the login's models exist.

Removing `opencode.agent` also removes OpenCode's hidden `compaction` and
`summary` helper agents and its `general`/`explore` subagents (subagents are
already denied). A manual `/compact` turn still completes (qualification test).

Each process still gets a private disposable `XDG_STATE_HOME` for startup.
`XDG_DATA_HOME` stays the owner's own; no separate database is created.

## Role instructions

The tested OpenCode 2.0.19 ignores `session/new` `_meta.systemPrompt` and has no other per-session instruction channel. The role prompt is therefore the prompt of the `cortex` agent: `{file:<path>}` pointing at a file the jail mounts **read-only from memory** (`engine_jail(data_files=)`). The host keeps only an empty mount point, so no other process can change the prompt.

The prompt contains, in order:

1. the unified Cortex base prompt (unless `CORTEX_UNIFIED_BASE_PROMPT=0`);
2. a note on how MCP tools are reached in this engine;
3. the role instructions;
4. the project's `AGENTS.md`/`CLAUDE.md` (OpenCode stops loading them once project configuration is off);
5. the attached-MCP-servers note.

OpenCode then appends its own environment block and tool catalog. Cortex uses this delivery channel for every `2.x.x` release; 1.x, 3.x and malformed versions resolve to UNSUPPORTED and are refused at launch. The instruction ledger records the prompt as `opencodeAgentPrompt`.

## MCP: the Cortex gateway through Code Mode

In the tested 2.0.19 build, a session's MCP servers are **not** separate model tools. They are reached only inside OpenCode's `execute` (Code Mode) tool as `tools.<server>.<tool>(input)`, for example `tools.cortex.read_report(...)`.

`execute` is therefore kept enabled. Observed behaviour:

- **Permission requests.** One `execute` call raises one permission request (kind `other`, title `execute`, `rawInput.code`). The calls inside it are not asked one by one. Every Cortex MCP call is still re-checked by the channel-bound gateway.
- **Catalog.** Code Mode's catalog holds the session's MCP servers, `opencode` (session move/rename, model search, MCP resources) and `browser` (fails without OpenCode's desktop app). It has no shell or file tools, and `fetch` is available.
- **Timing.** OpenCode connects MCP servers as `session/new` returns and fixes the session's tool catalog at its first prompt. A prompt sent within about 0.1 s of the servers' `tools/list` never sees them, in that turn or any later one. So after `session/new` Cortex waits until the broker observes OpenCode's own `tools/list` on the Cortex gateway, then another 1.0 s (`OPENCODE_MCP_SETTLE_SECONDS`). A gateway that never becomes ready fails the session (`opencode_mcp_not_ready`) instead of running without it. No ACP signal marks the registration, so this margin is a measured heuristic, not a guarantee.

## Process isolation (control-server credential)

Each `opencode acp` starts a private `opencode serve` child on `127.0.0.1` and keeps its Basic-auth password in its environment. The OpenCode jail runs in its own user, PID and IPC namespaces (`engine_jail(isolate_processes=True)`).

- Every other bubblewrap sandbox — other engine jails, `command_run` — lives in a sibling user namespace. The kernel refuses it `/proc/<pid>/environ` of the OpenCode processes: reading it needs `CAP_SYS_PTRACE` in the target's user namespace. This was checked on this host for a plain and an isolated target, and in the qualification test for the real `serve` child.
- Processes in the parent user namespace can still read it. That means the Cortex runtime itself and your own shell, both already inside your trust boundary.
- This relies on unprivileged bubblewrap giving every sandbox its own user namespace. A setuid `bwrap` is refused (`opencode_process_isolation_unavailable`).
- Cortex never reads, uses or relays the password.
- Inside its own jail, the model's shell could reach its own control server, but that grants nothing its shell cannot already do in that jail.

## Launch and version checks

OpenCode is launched only:

- from a verified install (`opencode_install_unverified`);
- inside the engine jail (`opencode_requires_engine_jail`);
- when the running engine's `initialize` `agentInfo.version` is qualified (`opencode_version_unqualified`, also on catalog refresh).

The data directory's `bin` (helper executables OpenCode downloads and later runs in your own sessions) is read-only in the jail. Pinned dependencies are not involved: OpenCode is a standalone binary with its own updater, which Cortex turns off for its processes.

## Permissions and deletion

Native tool requests reach the same permission bridge as the other engines:

- Shell commands arrive as `kind: execute` with `rawInput.command`, so a declared `rm` is recognised lexically and asks you in Guarded mode, exactly as for Codex.
- Code Mode cannot call shell or file tools. A deletion through Cortex's own `project_delete` asks you inside the gateway.
- When a permission is refused, OpenCode ends the **whole turn** with `stopReason: cancelled`, where the other engines only fail that one tool call. Cortex records its own refusal. The turn then continues in the same session with a runtime notice that the call was refused, at most three times per prompt. A user cancellation is never continued.

## Options, routing and access mode

- **Options.** `model` is always set, since OpenCode's own default is a free OpenCode Zen model. `effort` is set when the route carries one; it is pre-checked only when the route's model is already current, otherwise `configure()` re-checks it after the model switch. `mode` is always `cortex`.
- **Routing.** By default every `opencode` route is excluded from automatic selection (`routing.model_policy.BUILTIN_AUTO_DENY_ENGINES`). A `routing.json` `models.opencode.auto` list makes exactly the listed models eligible (with `models.opencode.tiers` and `policy` placing them; unlisted OpenCode models get a tier by list position), and an exact `auto_allow` entry lifts the exclusion for one route; the model selector or a `pin` runs any offered route. Model IDs contain the provider, so an entry names both (`models.opencode.auto: ["anthropic/claude-sonnet-4-5"]`, or `"opencode/anthropic/claude-sonnet-4-5"` in `auto_allow`). A plan node hint may name an exact OpenCode route (`opencode/anthropic/claude-sonnet-4-5`) copied from `list_route_options`; it is honoured only when that route is eligible for automatic routing, and an engine-level `opencode` hint does not lift the exclusion. See [owner-configured models](routing-and-fallback.md#owner-configured-models-selection-tiers-and-policy).
- **Access modes.** Guarded and Auto both work at the bridge. OpenCode has no more permissive mode to switch to, so Auto only answers the permission cards.

## Recovery, compaction and usage

- **Recovery.** Cortex never calls `session/load`, `resume`, `fork`, `close` or `delete` on an outgoing engine. Recovery opens a fresh session and process.
- **Compaction.** OpenCode documents `/compact`, but no compaction signal was observed over ACP. The tier is `controlled_boundary`: Cortex refreshes context only at its own boundaries. A manual `/compact` completes without OpenCode's removed `compaction` helper agent.
- **Usage.** `usage_update` (`used`, `size`, `cost`) and the prompt result's `usage` are recorded like the other engines. OpenCode reports no plan windows, so plan usage is unknown, never zero.

## Can all current Cortex functionality work through OpenCode v2?

The table records the no-paid qualification observations.

| Area | No-paid evidence on 2.0.19 (not a guarantee for every 2.x release) |
| --- | --- |
| Runtime-owned lifecycle, plans, review, verification | Engine-independent; works |
| Jail scopes, out-of-scope write refusal | Works (qualification test) |
| Cortex MCP gateway | Works through Code Mode `execute`, after the MCP settle wait |
| External MCP servers from the client/registry | Passed on `session/new` like the others; not separately exercised |
| Role instructions, project rules | Launch-time agent prompt; works |
| Model / effort / mode confirm-or-fail | Works; efforts are per model |
| Native subagent ban | Tool removed by configuration |
| Guarded deletion confirmation | Works for declared shell `rm` |
| Usage, cancellation | Works |
| Recovery | Fresh session (the same as the other engines) |
| Compaction signal | Not observable; controlled boundary only |
| Plan-window quota | Not reported by OpenCode |
| Structured auth/quota failure classification | Not observed; such failures surface as ordinary engine errors |
| Owner's custom providers and native logins | Works on installed 2.0.20 with the native `opencode.db` once the built-in agent plugins are removed (qualification test and owner-account catalog refresh); no alternate database |
| OpenCode desktop browser tools | Unavailable in ACP |

## Evidence and limits

- `tests/test_engine_opencode.py` (default suite, no binary needed) covers these paths:
  - registration and qualification tables;
  - configuration content and prompt assembly;
  - option mapping;
  - environment pins (every `OPENCODE_DB` override is refused) and native `XDG_DATA_HOME`;
  - owner provider definitions as jail files, no database credential extraction, and a jailed launch using the native database;
  - install trust, sandboxed version probe and onboarding states;
  - launch refusals;
  - jail isolation, masking and the read-only prompt file;
  - sibling-jail `/proc/<pid>/environ` denial;
  - automatic-routing exclusion and refusal continuation.
- `tests/test_engine_opencode_qualification.py` runs the real binary through `EngineSessionManager`, the engine jail, the brokered Cortex MCP server and the permission bridge. It uses a loopback fake OpenAI-compatible provider, which is the only test seam: it is added to Cortex's configuration. The disposable owner configuration also defines a provider on the same endpoint with a stand-in key: it must be offered, a turn on it must authenticate with that key read from its jail file, and no OpenCode process environment may hold the key. The disposable data directory is created by the binary itself (one prompt-free `session/new`, which fetches the public models.dev catalog) and holds stand-in logins for `nvidia` and for a third owner-configured provider: `session/new` must still offer only `cortex`, list `nvidia` models, and a turn on the third provider must authenticate with its login; native credential rows must stay unchanged while session state uses the same database. Opt in with `CORTEX_PINNED_OPENCODE_BINARY=<absolute path>`.
- On 2026-09-29, against 2.0.19, the qualification test failed once and then passed 10 consecutive runs. The failed run was the first after the MCP settle wait was added: the test provider's model was missing from `session/new`'s options. Twelve raw ACP launches outside the jail did not reproduce it, and the cause was not isolated. In production, a route model that is not offered fails configuration closed.
- The original 2026-09-29 qualification made no paid model, real-provider, Zed or Paseo run. On 2026-09-30, the same no-paid installed-binary qualification passed against OpenCode 2.0.20 with a loopback fake provider. This does not establish the owner's BytePlus model in ACP or a live user-route outcome.
- **Native-login qualification passes on 2.0.20** (2026-09-30):
  `tests/test_engine_opencode_qualification.py` passes (6 consecutive runs),
  including a manual `/compact` turn. With the plugin removal switched off, it
  fails with `['build', 'plan'] == ['cortex']` before any prompt.
- Prompt-free probes with a stand-in `nvidia` login: without the removal, 2 of 2
  offered `build`/`plan` and only `nvidia`. With it, 26 of 26 offered only
  `cortex` together with `nvidia`, `opencode` and both configured custom providers
  (one keyed only by a database login). On a directly launched `opencode serve`,
  the `cortex` agent appeared about 150-170 ms after start and the custom
  providers about 25 ms later. This ordering is not an OpenCode guarantee. A
  catalog that ever misses a configured provider only fails that route closed
  (`opencode_acp_option_not_offered:model`) until the next refresh.
- Earlier failed probes (a delayed `session/set_mode`/`session/set_config_option`,
  another `session/new` or `session/load`, a standard global config or agent
  file, a different cwd spelling) all hit the memoized catalog. `opencode acp`
  has no `--agent`, attach or server option: it always spawns its own
  `serve --stdio --port 0` child.
- **Fresh owner-account dev refresh, native database** (2026-09-30): catalog
  snapshots `cs_01fd91b8145a` and `cs_3199ac0a6401`, 148 routes each, every
  route in mode `cortex`: `byteplus-plan` 10 (including `glm-5.3-flash`),
  `minimax-coding-plan` 8, `moonshotai` 4, `nvidia` 57, `opencode` 40,
  `opencode-go` 29. No new database file appeared and the credential-row
  metadata was unchanged. The earlier failing refresh (build/plan) counted 79
  `opencode` models; that snapshot predates OpenCode's account configuration,
  which is fetched for a native `opencode` login and applied with the other
  plugins. This was a prompt-free setup refresh with the dev wrapper's isolated
  Cortex XDG environment and a disposable project, not a Paseo turn or a paid
  model call.
- Cortex now permits every OpenCode 2.x patch/minor release without a per-release behavior test. This relaxes **release-specific compatibility qualification**, not binary path trust, sandboxed version probing, jail/process isolation, permission-bridge policy or launch-time `agentInfo.version` major checking. The 2.0.19 behavior observations above may differ on a later 2.x release; a session still fails closed when a requested model, effort or mode is not offered or confirmed by its ACP options.

`opencode models` is not Cortex's route catalog. Cortex derives offered routes from an onboarded connection's prompt-free ACP `session/new` `configOptions`, then applies the owner's `models.opencode` selection. The Paseo host's separate `provider models opencode` catalogue is not the Cortex `route_catalog` settings surface. A missing model in that host catalogue does not prove a Cortex route is missing; nor does a CLI-only `opencode models` listing prove ACP can select it. Refresh a configured connection and inspect its `route_catalog` before claiming the exact route is usable.
