# Desktop maintenance and command controls

**Status: implemented local source, not installed-release qualification.**
The release update path has fixture evidence, not a real release or privileged
installation result. The broader signed-update requirement in CW-R38 remains
open: the current workflow publishes SHA-256 sidecars, not signatures.

## Updates

Settings → Updates is application-local. Dev explicitly says that updates come
from local builds and cannot query or install the release feed. Release checks
the stable latest GitHub release of `igovet/cortex-orch-acp` on **Check for
updates**. It compares the tag's Debian version with `dpkg-query`, using
`dpkg --compare-versions`; equal/older versions and prereleases are not offered.
Release notes are bounded plain text, never rendered HTML.

Download and verify fetches the exact architecture-specific release package and
its `.deb.sha256` sidecar. The checksum must name exactly that archive. Download
is bounded to 1 GiB, reports bytes received, supports cancellation and verifies
the digest plus the archive's Package, Version and Architecture before offering
installation. A corrupt archive is never handed to the package manager.
Transport trusts GitHub HTTPS and the repository's release publisher; a checksum
from that same publisher is **not** an independent signature or attestation.

The release workflow starts only through GitHub Actions → Workspace desktop
release → Run workflow. The selected branch determines the channel: `main`
publishes `cortex-workspace` as `v<debian-version>` and explicitly marks it Latest;
`dev` publishes `cortex-workspace-dev` as `dev-v<debian-version>`, a prerelease
explicitly excluded from Latest. Other refs are refused. Both appear in the same
repository release list, not separate branch lists. Dev installs manually from
the `.deb` and has no in-app updates.

Both publish the archive and its SHA-256 sidecar together, named
`<package>_<version>_<arch>.deb` and `.deb.sha256`. Tags use the **actual Debian
archive version**, including the ordered build stamp, not the shorter runtime
content id. Older releases lacking the sidecar fail closed. No release is created
by the app. See [owner release steps](../project/packaging.md#manual-workspace-releases).

Installation needs a separate consent checkbox and a system privilege prompt.
The fixed command is `/usr/bin/pkexec /usr/bin/apt-get install --no-remove -y
<verified-archive>`, never sudo, a shell or a supplied executable. The installed
version and digest are checked again before invocation. Activity is read from
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
- Models come from the observed route catalogue. Refresh models is explicit.
  Model details show a context window only when cached engine telemetry reported
  it for that exact connection/model. No model-name-based size is inferred.
- Weekly usage comes only from the provider's retained weekly window. Unknown
  percentages are hidden in Connections. OpenCode has no plan-usage meter.

## Voice and titles

Settings → Voice input describes the shipped sherpa-onnx CPU recognizer,
`small-zipformer-ru-2024-09-18`, Russian only, and the system default
PulseAudio/PipeWire source. It checks the real local model-file hashes. Test
microphone / retry model download uses the same local dictation worker as a
composer: first use downloads about 105 MiB, verifies it, then records. Cancel
discards the test transcript. No unsupported provider/language selectors remain.

Automatic naming is wired from `tasks.create` through the service's bounded
TitleJobs pool to the configured real engine route. No engine, invalid output,
error or deadline leaves the description fallback with a failed naming state;
hovering the title explains the fallback. Owner renames win. Naming never starts
the saved task. No paid engine generation is established by fixture tests.

## Command-to-control map

Source inventory: `src/cortex_runtime/acp/commands.py::AVAILABLE_COMMANDS`
(14 entries) and `scripts/cortex_commands.py` (two owner subcommands). These
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
| cortex-answer | Each pending question card, bound to its decision/execution/batch; Other answer supports multiline text and attachments |
| cortex-language | Task composer Language; Orchestration → Coordinator sets new-task language |
| harvest | Project → Knowledge → Collect knowledge |
| harvest-refresh | Project → Knowledge → Refresh knowledge |
| skill-management | Project → Knowledge → New skill; describe creation, revision or import |

The three Knowledge controls ask the service to build the same explicit workflow
instruction used by the ACP command, then save it as a normal task. Task
descriptions intentionally do **not** dispatch slash commands. Start is still
explicit, and normal runtime permissions/review apply.

| Owner CLI | Desktop control |
| --- | --- |
| cortex_commands.py list | Task → Workers → selected worker activity and retained command rows |
| cortex_commands.py stop | Stop command on that exact row, with a reason; native engine shell actions without a Cortex receipt remain unavailable |

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
checkout-bound PTY. New pending questions notify through the Linux desktop session bus when their task or sprint is not open (or the document is hidden). Appearance can disable notifications; they are on by default in a new profile. Notification text names the work, not the question body. A missing notification daemon produces an explicit delivery error; Inbox and sidebar remain available. Duplicate refreshes do not resend the same question. Browser-only builds disable the native notification switch.

## Diagnostics and local desktop actions

Diagnostics lists the individual checks, their observed availability, a readable failure reason and a recovery step. Copy report includes those details rather than only the overall degraded status. Older saved runs without details ask for a fresh run; missing npm in the desktop service does not imply it is absent from an interactive terminal. Diagnostics never installs tools or changes service configuration.

The task menu opens the task’s main checkout or separate worktree using the native editor action. The same menu links directly to machine Diagnostics. An unavailable opener or missing worktree has a visible reason, never an unexplained grey item. VS Code discovery checks the process search path, usual system locations and per-user `.local/bin` and `.local/share/code/bin`; the default application remains the existing fallback. No editor is launched as a side effect of discovery.

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
