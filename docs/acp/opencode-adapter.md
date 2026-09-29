# OpenCode v2 outgoing adapter (optional)

Cortex can run [OpenCode v2](https://opencode.ai/v2/docs/cli/acp/) (`opencode acp`) as a third outgoing ACP engine, next to Codex and Claude. It is **optional and fail-closed**:

- startup never discovers it or creates a connection for it;
- you onboard it with one explicit setup step;
- automatic worker routing never picks it unless `routing.json` opts in;
- only OpenCode **2.0.19** is accepted, and it runs only inside the engine jail with process isolation.

Source: `engine/opencode_install.py` (binary trust and version), `engine/opencode_adapter_acp.py` (configuration, prompt, option mapping), plus OpenCode branches in `engine/process.py`, `engine/session_manager.py`, `orchestration/sandbox.py`, `delegation/live_adapters.py`, `catalog/refresh.py`, `routing/auto_route.py` and `scripts/cortex_acp_setup.py`.

## Onboarding

1. Install OpenCode with its own installer (the binary normally lands in `~/.opencode/bin/opencode`) and log in once in a terminal: `opencode auth login`. The login is stored in `~/.local/share/opencode/auth.json`.
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
- its version, read by `opencode --version` inside a network-less sandbox with a throwaway HOME, must be exactly a qualified version.

The connection references your own OpenCode login in place: it is never copied, and no test login is ever made. The step prints the connection ID, its state and the next command:

- `opencode auth login` when the login is missing;
- the `chmod` when the directory is not private;
- `refresh-catalog --connection <id>` when the connection is ready.

`python3 -B scripts/cortex_acp_setup.py status` also reports a found OpenCode binary under `discovered_engines.opencode`, marked `optional` and `qualified`.

The catalog fills the way it does for the other engines:

- on each session open, from the `configOptions` OpenCode reports;
- on demand, with `refresh-catalog`, which opens a prompt-free session inside a read-only jail.

Efforts are read per model, because OpenCode offers different effort values per model and none at all for some.

Registering the connection is not enough by itself: OpenCode contributes no models to the selector until its catalog holds at least one successful refresh (there is no seed for it, unlike the built-in engines). A `configured` connection with no catalog snapshot yet is exactly as invisible in the model selector as an `invalid` one. After a `configured` state, run `refresh-catalog` (above) or open a new Cortex session, which also triggers a background refresh; only then does a new session’s model list include OpenCode.

## What Cortex configures

Cortex builds OpenCode's whole configuration itself. It passes that configuration in `OPENCODE_CONFIG_CONTENT` (see `build_opencode_config`):

- **One agent, `cortex`.** It is the default and the only agent offered: `build`, `plan` and the `title` helper are disabled. OpenCode exposes agents as its `mode` option, so the Cortex mode floor for both coordinator and worker is `cortex`.
- **Permissions.** `"*": "ask"`: every native tool call becomes an ACP `session/request_permission` that Cortex's permission bridge answers. `task` (its `subagent` tool), `question` and `skill` are denied, which removes those tools.
- **Nothing else.** `autoupdate` is off and `share` is disabled. No provider, model or MCP server is configured. Providers come from your login. The model is set per session through `session/set_config_option`, confirm-or-fail. MCP servers are passed only on `session/new`.

OpenCode also reads your `~/.config/opencode` and the project's `opencode.json`/`.opencode/`. The jail hides the first behind an empty private tmpfs. `OPENCODE_DISABLE_PROJECT_CONFIG=1` turns off the second. `OPENCODE_DISABLE_AUTOUPDATE=1` is always set.

Each OpenCode process gets a private, disposable `XDG_STATE_HOME` in its jail scratch directory, because OpenCode does not start when it cannot write there. `XDG_DATA_HOME` stays your own, so logins and session history live where they always do.

## Role instructions

OpenCode 2.0.19 ignores `session/new` `_meta.systemPrompt` and has no other per-session instruction channel. The role prompt is therefore the prompt of the `cortex` agent: `{file:<path>}` pointing at a file the jail mounts **read-only from memory** (`engine_jail(data_files=)`). The host keeps only an empty mount point, so no other process can change the prompt.

The prompt contains, in order:

1. the unified Cortex base prompt (unless `CORTEX_UNIFIED_BASE_PROMPT=0`);
2. a note on how MCP tools are reached in this engine;
3. the role instructions;
4. the project's `AGENTS.md`/`CLAUDE.md` (OpenCode stops loading them once project configuration is off);
5. the attached-MCP-servers note.

OpenCode then appends its own environment block and tool catalog. Instruction delivery is registered as QUALIFIED for exactly `("opencode", "2.0.19")`; any other version resolves to UNSUPPORTED. The instruction ledger records the prompt as `opencodeAgentPrompt`.

## MCP: the Cortex gateway through Code Mode

In 2.0.19, a session's MCP servers are **not** separate model tools. They are reached only inside OpenCode's `execute` (Code Mode) tool as `tools.<server>.<tool>(input)`, for example `tools.cortex.read_report(...)`.

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
- **Routing.** Every `opencode` route is excluded from automatic selection (`routing.auto_route.DEFAULT_AUTO_DENY_ENGINES`). The model selector, a `pin` or an exact `auto_allow` entry opts one in; model IDs contain the provider, so the entry names both (for example `"opencode/anthropic/claude-sonnet-4-5"`). Plan node hints cannot name an exact OpenCode route, because the hint pattern allows no `/` inside a model ID; an engine-level `opencode` hint does not lift the exclusion and is ignored.
- **Access modes.** Guarded and Auto both work at the bridge. OpenCode has no more permissive mode to switch to, so Auto only answers the permission cards.

## Recovery, compaction and usage

- **Recovery.** Cortex never calls `session/load`, `resume`, `fork`, `close` or `delete` on an outgoing engine. Recovery opens a fresh session and process.
- **Compaction.** OpenCode documents `/compact`, but no compaction signal was observed over ACP. The tier is `controlled_boundary`: Cortex refreshes context only at its own boundaries.
- **Usage.** `usage_update` (`used`, `size`, `cost`) and the prompt result's `usage` are recorded like the other engines. OpenCode reports no plan windows, so plan usage is unknown, never zero.

## Can all current Cortex functionality work through OpenCode v2?

| Area | Status on 2.0.19 |
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
| Owner's custom providers from `~/.config/opencode` | Not loaded (that directory is hidden); only logged-in built-in providers |
| OpenCode desktop browser tools | Unavailable in ACP |

## Evidence and limits

- `tests/test_engine_opencode.py` (default suite, no binary needed) covers these paths:
  - registration and qualification tables;
  - configuration content and prompt assembly;
  - option mapping;
  - environment pins and `XDG_DATA_HOME`;
  - install trust, sandboxed version probe and onboarding states;
  - launch refusals;
  - jail isolation, masking and the read-only prompt file;
  - sibling-jail `/proc/<pid>/environ` denial;
  - automatic-routing exclusion and refusal continuation.
- `tests/test_engine_opencode_qualification.py` runs the real binary through `EngineSessionManager`, the engine jail, the brokered Cortex MCP server and the permission bridge. It uses a loopback fake OpenAI-compatible provider, which is the only test seam: it is added to Cortex's configuration because production names no provider. Opt in with `CORTEX_PINNED_OPENCODE_BINARY=<absolute path>`.
- On 2026-09-29, against 2.0.19, the qualification test failed once and then passed 10 consecutive runs. The failed run was the first after the MCP settle wait was added: the test provider's model was missing from `session/new`'s options. Twelve raw ACP launches outside the jail did not reproduce it, and the cause was not isolated. In production, a route model that is not offered fails configuration closed.
- No paid model, real provider, Zed or Paseo run was made. Live qualification was not run because the owner did not request a paid live check.
- A future OpenCode release may change any surface above, so every other version is refused until it is qualified again.
