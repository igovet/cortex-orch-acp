# Desktop maintenance and command controls

**Status: implemented local source, not installed-release qualification.**
The signed release update path has local fixture evidence, not a published
release or privileged installation result.

## Updates

Settings → Updates is application-local. Dev explicitly says that updates come
from local builds and cannot query or install the release feed. Release checks
the stable latest GitHub release of `igovet/cortex-orch-acp` on **Check for
updates**. It compares the tag's Debian version with `dpkg-query`, using
`dpkg --compare-versions`; equal/older versions and prereleases are not offered.
Release notes are bounded plain text, never rendered HTML.

A newer release must supply the exact architecture-specific `.deb`,
`.deb.sha256` and `.deb.sha256.sig`. Before offering the release, Workspace
verifies the detached Ed25519 signature over the exact checksum bytes with the
public key embedded at build time. The signed checksum must name exactly that
archive. An unsigned build (no embedded key), missing signature or any mismatch
fails closed: verification failed, nothing installed.

**Download and verify** retrieves the archive (up to 1 GiB, with progress and
cancellation), uses the authenticated digest, and checks its Debian Package,
Version and Architecture before offering installation. GitHub HTTPS still
delivers the feed and assets, but cannot substitute for the signing key.

The release workflow starts only through GitHub Actions → Workspace desktop
release → Run workflow. The selected branch determines the channel: `main`
publishes `cortex-workspace` as `v<debian-version>` and explicitly marks it Latest;
`dev` publishes `cortex-workspace-dev` as `dev-v<debian-version>`, a prerelease
explicitly excluded from Latest. Other refs are refused. Both appear in the same
repository release list, not separate branch lists. Dev installs manually from
the `.deb` and has no in-app updates.

Both channels attach the archive and its SHA-256 sidecar. Only `main` also
attaches the `.deb.sha256.sig` required for in-app updates. Tags use the **actual
Debian archive version**, including the ordered build stamp, not the shorter
runtime content ID. Older unsigned releases fail closed in the release updater.
No release is created by the app. See [owner release steps](../project/packaging.md#manual-workspace-releases).

Installation needs a separate consent checkbox and a system privilege prompt.
The fixed command is `/usr/bin/pkexec /usr/bin/apt-get install --no-remove -y
<verified-archive>`, never sudo, a shell or a supplied executable. The installed
version, signature and archive digest are checked again before invocation.
Activity is read from
each registered project's runtime store before starting and immediately before
the prompt: active or unconfirmed work refuses the operation. New task starts
are blocked while an installer is running or unconfirmed. Stop work before
consenting; this is not a general machine-wide scheduling/lease implementation.

A separate `systemd-run --user` transient unit owns the installer so the package's
service restart does not kill its own installer with the Workspace service
cgroup. A missing user manager or privilege agent is an explicit failure/unknown
state, not permission to try another privilege route. The installer is never
cancelled or timed out by Workspace. Success requires apt exit 0 and the expected
installed version. Restart the app explicitly afterward; prompts are not replayed.

Rollback is a distinct consented action. It works only when apt retained the exact
previous release archive; the app copies it privately before update, records its
digest and rechecks it before an explicit `--allow-downgrades` install. Missing or
changed previous bytes refuse rollback. It does not restore project databases or
guarantee schema compatibility; never use it across an incompatible storage
migration. There is no automatic rollback or invented successful health check.

The machine maintenance provider is native Rust. Clone/download workers use a trusted same-binary entry point; installers retain the separate systemd user unit. A private exclusive worker claim prevents replay of an already-launched identity, including after a crash. HTTPS transfer uses installed `/usr/bin/curl` with TLS-only redirects, size/deadline bounds and no caller-supplied executable; a missing executable refuses the operation. Native desktop Start and work-restarting controls share the maintenance admission fence with installation jobs; unknown installers continue to hold work until the explicit owner acknowledgement.

Operation identities and progress survive navigation/window reopening. A lost
reply is inspected by the same identity, never automatically resubmitted.
Unconfirmed installation stays unconfirmed; inspect the system package manager
before any retry. After manually checking that no install is running, a separate
checkbox and Allow work action can remove the task-start hold without claiming
installation success. Private operation metadata and downloaded archives live under
the channel state directory's `maintenance/`; no raw subprocess output is saved.
This implementation does not yet prune old download archives automatically.

## Engine connections and facts

Settings → Engine connections → Add connection offers Native account or API key
for Codex and Claude. OpenCode retains its separate explicit Enable flow.

- API keys use a password field and the existing Cortex onboarding API. Bytes
  cross the authenticated local machine helper's stdin, not argv, chat, command
  receipts or logs, and enter the canonical owner-only 0600 secret file. The
  field clears on submission; browser storage contains no key. Creation checks
  the registry revision inside its existing transaction, not a nested lock.
  A failed/unknown response requires refreshing before retrying.
- Native account opens the verified bundled native executable in
  `/usr/bin/gnome-terminal -- <executable> …`: `codex login` or
  `claude auth login`. Those commands exist in the retained bundled versions.
  Finish in that terminal, then use Recheck native login and connect. Recheck runs
  the native CLI status command without retaining its output before onboarding. A terminal
  launch is not proof of authentication. HOME stays the owner's native HOME;
  credentials are never copied. Missing adapter/terminal/login is reported.
- Models come from the observed route catalogue. Refresh models is explicit, but
  the service also observes a connection's models on its own, without a model
  prompt, whenever a settings read finds a configured connection with no saved list
  (or one older than 24 hours, which stays served meanwhile). Each connection is
  observed once in the background, side by side, under its refresh lock, with the
  startup engine launch shape; a connection observed (or failed) is not started
  again by a read for ten minutes. `CORTEX_WORKSPACE_CATALOG_AUTOREFRESH=off`
  disables it.
  Model details show a context window only when cached engine telemetry reported
  it for that exact connection/model. No model-name-based size is inferred.
- Weekly usage comes only from the provider's reported weekly windows. Connected
  engines remain visible in Connections, the sidebar and inspector when usage is
  unknown, explicitly marked Unavailable rather than zero. OpenCode has no
  plan-usage meter and therefore shows Unavailable. Claude shows two weekly
  windows side by side: the general window (all models) and the Fable window,
  each with its own percentage and reset time, and each "Not reported" when the
  provider did not report it.
- Limits refresh in the background while settings are read (the app reads them
  every five minutes). A read never waits: the first one starts a service loop
  that makes a pass at most once a minute and stops after 15 minutes without a
  read; each connection is read at most once per five minutes, failures included,
  one connection after another. Both reads are free and start no model turn:
  Claude Code's local `/usage` (trusted only when its JSON shows
  `local_command: "usage"`, zero turns, zero cost and zero tokens) and the Codex
  app server's `account/rateLimits/read` account request. Each runs the verified
  native CLI of the qualified adapter with the owner's real `HOME` and the
  connection's own `CLAUDE_CONFIG_DIR`/`CODEX_HOME`; credentials are never copied
  and nothing logs in. A Codex rollout newer than the last read still wins. Every
  value shows when it was observed ("As of 14:05"); one older than 15 minutes is
  marked stale. `CORTEX_WORKSPACE_USAGE_REFRESH=off` disables the refresh.
- Coordinator defaults and exact-route pickers exclude models denied by Use
  engine or model policy. The service route catalogue marks denied routes
  unselectable; connection readiness remains separate so Models can re-enable
  observed models. New sprint validation uses the same allowed choices as the
  composer. Disabling an engine does not rewrite an existing task's route.

## Voice and titles

Settings → Voice input switches the shipped sherpa-onnx CPU recognizer between
Russian (RU, the default) and English (EN). The machine saves the choice for
the next recording. Each model displays its own missing, verified or failed
integrity status. Download fetches and verifies only the selected model without
opening the microphone (about 105 MiB for Russian or 107 MiB for English).
Test microphone uses the system default PulseAudio/PipeWire source and the same
worker as a composer, downloading the selected model first if needed. Cancel
discards the test transcript. Audio never leaves the machine.

Automatic naming is wired from `tasks.create` through the service's bounded
TitleJobs pool to the configured real engine route. No engine, invalid output,
error or deadline leaves the description fallback with a failed naming state;
hovering the title explains the fallback. Owner renames win. Naming never starts
the saved task. No paid engine generation is established by fixture tests.

Commit, squash and Push drafts use the same [short model text route](local-task-execution.md#short-model-texts) as naming. Commit and squash use changed files and bounded diffs for a subject and detailed body. Push summarizes outgoing commit subjects, bodies and change counts from local tracking information. If Git or the model cannot provide a draft, the dialog shows why and leaves the text editable. A confirmed Push summary can prefill the PR description, not the title. It is kept whole up to 8,000 UTF-8 bytes; a longer one disables Push with the byte count shown and is never shortened.

## Command-to-control map

Source inventory: the ACP command list in `crates/cortex-acp/src/commands.rs`
(14 entries when this table was written) and the two owner subcommands of the former `cortex_commands` script (removed). These
are intent mappings, not an arbitrary command console.

| ACP command | Desktop control / owning path |
| --- | --- |
| cortex-clear | Settings → Storage and archive → Clear old tasks: days or UTC date, exact preview and Delete confirmation; service rechecks age and eligibility and uses the existing journaled purge |
| cortex-status | Task header/inspector and composer configuration show runtime state, model, effort and access; Connections shows engine state |
| cortex-model | Task composer model/connection selector; Orchestration → Coordinator for new-task defaults |
| cortex-effort | Task composer Effort selector; machine defaults remain separate |
| cortex-setup | Settings → Engine connections: install, native login/recheck, API-key add, Check, Refresh models |
| cortex-new-task | New task / Ctrl+N: required description, optional route and checkout, then explicit Start |
| cortex-kill | Worker activity → Stop command, with a reason and retained Cortex tool-call identity |
| cortex-stop | Close task → Cancel task cancels runtime work; the ordinary Stop task button is deliberately resumable Pause, not this irreversible command |
| cortex-finish | Close task → Close as is: runtime cancellation plus explicit not-verified outcome; never fabricated review/check completion |
| cortex-answer | Each pending question card, bound to its decision/execution/batch; its answer composer, inside the card, supports multiline text and attachments; choosing an option only prepares the answer and the plane sends it |
| cortex-language | Task composer Language; Orchestration → Coordinator sets new-task language |
| harvest | Project → Knowledge → Collect knowledge |
| harvest-refresh | Project → Knowledge → Refresh knowledge |
| skill-management | Project → Knowledge → New skill; describe creation, revision or import |

The three Knowledge controls ask the service to build the same explicit workflow
instruction used by the ACP command, then save it as a normal task. Task
descriptions intentionally do **not** dispatch slash commands. Start is still
explicit, and normal runtime permissions/review apply.

| Former owner CLI (removed) | Desktop control |
| --- | --- |
| `cortex_commands list` | Task → Workers → selected worker activity and retained command rows |
| `cortex_commands stop` | Stop command on that exact row, with a reason; native engine shell actions without a Cortex receipt remain unavailable |

Desktop Clear old tasks is intentionally narrower than legacy cortex-clear:
only shown finished tasks without unmerged work are eligible; archived tasks use
Archive deletion. Live/unknown runtime protection is never bypassed, including
when the legacy CLI's stale-protection force option exists. A partial batch says
how many were acknowledged and requires refreshing; no hidden replay occurs.
Setup/packaging utilities that install Paseo, clone settings or edit host client
registration are administrative tools, not Workspace task commands.

Local clone sources may also be absolute bare Git repository paths. Private settings and runtime roots are refused; local clones use separate object files rather than hard links. The helper still disables global Git configuration and interactive credential prompts. Existing destinations are never overwritten.

Saved profile rows project their actual routing, defaults/ceiling, MCP usage and
connection-session policy sections. Executable commands, arguments, environment
and credentials are not included in those profile sections.

The native PTY is a registered-project shell, even when opened from a task. A
worktree-local terminal action is disabled because the service does not offer a
checkout-bound PTY. The machine service notifies through the Linux desktop session bus when something needs the owner: a question or permission, the runtime's stop question, a result to review, a failed task, a sprint planning question and a sprint that paused itself, each outside the task or sprint on view in the focused window, also while no window is open. Appearance can disable notifications and, below that switch, each kind; all are on by default in a new profile, and the window passes the choice to the service, which keeps it in its state folder (`notifications.json`, 0600, with the newest 1000 item states already announced). Notification text names the work and, for a stopped task or a paused sprint, the runtime's one-line reason; never a question's text. Each task or sprint has one notification, replaced by a newer state and withdrawn once resolved. The service's first start announces nothing already open, a relaunch of the window announces nothing again, and an item that arrived while the window was closed is announced when it arrives. A machine without a session bus raises nothing and `service.status` says why; Inbox and sidebar remain available. A service that does not raise notifications leaves the window's own question notifications, where a missing notification daemon produces an explicit delivery error. Browser-only builds disable the native notification switch. Clicking a notification raises the window, or starts the app, and opens the place it is about; see [notifications](SCREENS.md#notifications-desktop) and [GNOME and Wayland](platforms.md#desktop-notifications-on-gnome-and-wayland).

## Diagnostics and local desktop actions

Diagnostics lists the individual checks, their observed availability, a readable failure reason and a recovery step. Copy report includes those details rather than only the overall degraded status. Older saved runs without details ask for a fresh run. Git, Node, npm and bubblewrap are looked up on the tool search path below, the same one worker commands and the verifier use, and each found tool names its location and whether it came from your shell, a usual tool folder, a folder you added or the system folders; Docker and `python3` are optional items that never degrade the result. A **Tool search path** item says whether your shell was read or why the fallback is used. Diagnostics never installs tools or changes service configuration.

### Tool search path

The local service is started by your session manager, not your terminal, so its own `PATH` lacks what your shell startup files add (nvm, `~/.local/bin`, `~/.cargo/bin` and so on). The Diagnostics page's **Tool search path** section (local desktop only) shows the folders commands search:

- **Detected**: the `PATH` read from your login shell (bash, zsh, fish or a POSIX shell), run once as an interactive login shell without a terminal, with a 5-second limit, at service start and when you choose **Re-read from my shell**. The time it was read is shown. If the shell cannot be read, the reason is shown and the service's own `PATH` plus the usual per-user tool folders that exist (`~/.local/bin`, `~/.cargo/bin`, `~/bin`, nvm's default version, `~/.volta/bin`, `~/.bun/bin`, `~/.deno/bin`, `~/go/bin`, pnpm's home, pyenv and asdf shims) are used instead. Entries that were ignored are listed with the reason.
- **Extra folders**: folders you add, searched after the detected ones and saved in this machine's settings. Only absolute existing folders are accepted; a folder inside a project, in Cortex's private state, or writable by anyone (without the sticky bit) is refused.
- **Usual tool folders**: after the detected and extra folders, the usual per-user tool folders listed above (plus fnm's default Node, `~/.local/share/fnm/aliases/default/bin` or `~/.fnm/aliases/default/bin`) that exist and are not already listed are searched too, before `/usr/local/bin`, `/usr/bin` and `/bin`, and shown as "a usual tool folder". They only fill gaps: a program your shell's path already finds is found there first. A shell whose startup files leave out `~/.cargo/bin` or nvm's Node therefore no longer makes worker commands and checks fail with `cargo: not found` or `execvp npm`. An incoming ACP agent (started by Zed or Paseo, which never reads your shell) uses its client's `PATH` followed by the same usual folders and the system folders.

The project's own `.venv/bin`, `venv/bin` and `node_modules/.bin` still come first for its commands. A change applies to running tasks from their next command or check; nothing restarts. Engines keep their own bundled Node and environment. See [SECURITY.md](../../SECURITY.md) for what the shell read runs and what is refused.

The task menu opens the task’s project folder or separate worktree using the native editor action. The same menu links directly to machine Diagnostics. An unavailable opener or missing worktree has a visible reason, never an unexplained grey item. VS Code discovery checks the process search path, usual system locations and per-user `.local/bin` and `.local/share/code/bin`; the default application remains the existing fallback. No editor is launched as a side effect of discovery.

Worktree settings save only the new default root. A failed save retains the draft and a persistent readable refusal; retry or discard remains explicit. A writable external folder with a `.cortex` name is allowed, while project checkouts are not. Existing worktrees remain where they are.

## Remote machines and verification limits

Remote pairing remains unavailable until the owner selects a relay server.
Real mode shows that reason rather than simulated codes, fingerprints or server
health. No relay, credentials or host settings are changed by these controls.

Focused no-paid checks cover local HTTP release fixtures (good archive, corrupt
checksum, downgrade and network failure), real cloning from a local bare Git
repository, cancellation, isolated secret-file storage and stale revisions,
installer/rollback argv through recorder stubs, bundled native login help, and
DOM control flows. They do not establish privileged Ubuntu installation,
successful real-account login, microphone recognition, a rendered native
maintenance journey, or release readiness.
