# Changelog

## 1.0.6 (unreleased)

Tasks working directly on their target branch no longer offer a merge from that branch to itself. Recorded file-write hashes identify work already committed, including older records without a Git baseline and files edited again later by another task. Pending recorded files offer a confirmed task-only commit that preserves unrelated staging; unknown ownership explains the limit and offers explicit closure. Existing Ready records update on refresh without being rerun. Separate-worktree merges and last-fetched push information keep their behavior.

Workspace switches turn with a mouse click again wherever they have no label around them: a model's On and Auto in Settings → Models, MCP servers in Tools, fallback consent, notifications, Autosave and remote access. The painted track lay over the hidden checkbox and took the click, and a checkbox does not stretch to `inset: 0`, so only a surrounding label or the keyboard reached it. The track now lets clicks through and the checkbox fills it. `focus.measure.py` now clicks every switch on every screen with real X input, centre and edge, in both themes; it failed on the 56 model switches before the fix and passes all 92 switches after it.

A started task whose engine has not opened yet no longer shows "Process state unknown" with Stop disabled. Until the runtime records the task, the projection called the execution contradictory, so the launch was invisible, normal Stop was off, and Code's Stop the task first failed with "This session no longer owns the task". A start whose session and owner agree now projects as starting; each engine-launch retry shows the runtime's own notice as the task status, and a give-up shows as needing the owner. Stop cancels such a launch through the runtime's task stop, and a stopped launch no longer keeps the checkout busy. Any other missing or disagreeing fact still projects as unknown, and a restarted service still doubts an open launch. `tests/test_workspace_launch_state.py` and `launch-state.test.ts` cover the projection, Stop and the status line.

Clicks are no longer sometimes lost, and controls no longer redraw under a resting pointer, when the machine is read again. Four places built their rows with a helper called inside the page, so a change elsewhere rebuilt them even when they were the same: the worker cards of the Workers tab at every change to the running task, the machine links of the Settings sidebar at every machine read, the Inspector's Open task at every change to the selected task, and a project's Limits and fallback rows at every machine read. A control replaced between press and release swallows the click, which is how opening a worker sometimes did nothing; a replaced field also lost its focus while the owner was typing. They are now components, which keep their elements until what they show really changes. jsdom regressions keep each of them in place through such a read and still move a worker that finishes into Finished.

A project activated before the dev store moved to `.cortex-dev` now starts again by itself after the upgrade. The earlier build wrote its own ownership marker, so the new service found no `.cortex-dev-owner.json` and refused every automatic restart with `dev_project_refused`; the project stayed stopped after the upgrade, a unit restart and a login. Activation now reserves such a project when it has no `.cortex-dev` yet, since there is nothing to adopt. An existing unreserved `.cortex-dev` is still refused, older markers and `.cortex` data stay untouched, and the desktop's refusal text no longer blames the released `.cortex` store. `packaging/container/check_systemd.py --prior-archive` now activates a project under the prior archive and requires it ready after the apt upgrade, a unit restart and a login, and Rust regressions cover both outcomes.

Resume right after Stop no longer fails with "This task changed on the machine". Stop answered as soon as it cancelled the running turn, and the turn then recorded its end, which moved the task's version past the one the window had just read. Stop now answers after the interrupted turn has settled, at most 10 seconds later, without blocking other requests, so Resume carries the current version. The stale check itself is unchanged. A service regression resumes with the version read after Stop, and the installed journey now waits for the exact Stop and Resume buttons instead of any "Stop" text.

Workspace no longer freezes when a project's runtime is away while its saved sprint plan names tasks. The shared task index looked up every sprint task as if it were loaded, so when an upgrade restart left the project's runtime stopped, every page change threw and no link, Settings or sidebar toggle responded. Tasks that are not loaded are now skipped, and the sprint's Run button waits with "This sprint's tasks are not loaded yet". `packaging/container/check_real_input.py` clicks the installed window with real X server input in a fresh home and in that state, and jsdom regressions cover the index and the Run button.

A crashed workspace service no longer leaves a project's Cortex running. Linux sends the parent-death SIGTERM again each time the killed service hands the child to another of its exiting threads, and the handler's `Event.set` took a lock the interrupted first run still held, so the child sometimes deadlocked in its own handler and stayed alive for ever. The handler now only assigns a flag the main thread polls, in the service child and in `cortex_acp` alike. A regression re-enters the handler at every line of its first run.

`scripts/run_tests_fast.py` now rebuilds the workspace service before the first batch and stops with exit 2 when it cannot, so the suite never launches an older build. Direct pytest runs refuse a service binary older than its sources and name the build command. Owner pause and resume are listed as covered public writers, with a regression for their invalidations and rollback. Debian packaging no longer depends on the builder's umask: under 0077 it failed at `dpkg-deb`, and voice files could have kept private modes. Test cleanup also removes the mode-000 work directory an overlay leaves, which had turned a passing batch into a failure on hosts with working bubblewrap.

Workspace follows one set of interface rules on every screen. Route, connection, report and build identifiers are gone outside Diagnostics, and a version reads as 1.0.6. Labels no longer name the program, and hints are English only, so the help language setting is gone. Titles and names are edited by clicking them. Zero counters are hidden. Left and right gaps are equal everywhere: scrollers keep their track, because WebKitGTK reserved no gutter and rows touched the right edge. Modified now says in words what changed, Tiers read from Light to Flagship, and the Routing table fits its Work kind and Tier columns. Text can be selected and copied with Ctrl+C, and anything cut shows whole in its tooltip. Moving with Tab or an arrow key draws one quiet 1 px line around the focused control, and a mouse click draws nothing and clears it. `packages/workspace-ui/test/sweep.measure.py` checks these rules on every screen in both themes, and `focus.measure.py` checks the line.

Workspace task and board flows follow the owner's review. Several open questions show as numbered tabs and a settled question reads as question and answer. The conversation reads as messages and folded steps, with no machine identifiers or display updates. Plans list titles with Read more, All work groups tasks by sprint, tasks waiting for the owner stand out, and a card crosses to its new column unless reduced motion is on. Every action shows its loader on the pressed control and blocks a second press, and an identical command already under way is never sent twice. A read that says nothing new no longer redraws anything, which ends the plan blinking under a resting pointer. Live data no longer shows a Confirmed stamp, the Inspector names checks that did not pass, and a check row and a checkbox no longer share a shape. Sprint planning keeps its questions across refreshes.

The Workspace rendered journey now records how the desktop process, its web process and the compositor really ended, and it no longer stops when the page replaces an accessibility node it was holding. A real death of any of them stays a hard failure, and `packaging/container/journey_series.py` repeats the journey one run at a time.

Workspace Code is now an editor. CodeMirror 6 with pinned versions edits project and task checkout files, with tabs, a dirty marker, find and replace, Save and an Autosave setting. A save writes only over the revision the owner read, so a change from a worker or on disk becomes a conflict with Reload from disk or Keep mine. Saving waits until the runtime shows the checkout's task stopped, and unsaved text survives navigation and asks before a tab or the window closes. Open in VS Code now works from Code and the worktree menu at the exact file and line, falls back to the default application, and Open logs says what to install when no opener exists.

Workspace adds projects the way Paseo does: the empty state, sidebar and palette open one flow with Search for folder, Browse and New folder. The search runs on the selected machine as you type, through a new bounded and cancellable `directories.search` service operation, and marks Git repositories. Choosing a folder adds it, named after the folder. `projects.register` gains `newFolder`. Clone from GitHub now shows its icon instead of a solid block.

Workspace refreshes configuration after first activation and background setup, retains revision-bound dirty drafts for explicit conflict review, and avoids writing local defaults on a refused save. Git commits and amend use the visible message immediately, and disabled Pull names its prerequisite. Sprint planning refuses missing or unavailable models before creating work and keeps the description. Worktree reset, tooltip and example share the service-reported channel default without moving existing checkouts. Empty-HOME helper/UI and fake-engine service journeys cover these source fixes; installed-package/native-window and real-account qualification remain outstanding.

Workspace no longer reports a healthy machine as offline. Any refused request used to mark the machine disconnected until restart, so one wrong path disabled the whole app with "result not known". Now only the status check decides the connection, and it recovers by itself. Projects can live anywhere the owner can write, including other disks, and the Add project dialog gained the system folder chooser, name-from-folder and "Create folder and register". Every refusal shows its reason in the dialog. The service supervises each project's Cortex: a crashed child restarts with backoff, a slow one is no longer abandoned, a missing folder no longer stops the service, and activation no longer freezes it. A project that cannot run names its reason on its own page with Try again. Starting a task with no engine ready is refused with the reason and leaves the task startable, where it used to wedge the project.

Workspace Debian versions now use `1.0.6+z<UTC-microsecond-build-stamp>.sha256.<content-hash>`, above every legacy hash-only version. New builds reserve increasing explicit ordering inputs; cache reuse gets a new stamp unless an identical-input replay is explicitly requested. Runtime content identity remains embedded and unchanged. Reproducible archive checks and Debian ordering comparisons do not constitute a new installed-package qualification.

Workspace dialogs show command refusals inside the modal, retain unsaved settings after a failed save or profile switch, and restore focus when closed. The native journey now waits for review and worker-control dialogs to close instead of treating a new runtime result as the submit acknowledgement; worker-message input still uses real native focus and typing.

Workspace adds local Russian voice input with Ctrl+D/mic start and stop, live composer previews, Cancel, Insert and explicit Insert and send. The package includes locked sherpa-onnx-node and uses bundled Node; a first-use verified model download shows progress. Default-source PulseAudio capture and local inference keep audio on-device; stopping or losing a recording never sends it.

Workspace adds a service-owned local project terminal (Ctrl+J), pinned portable-pty/xterm.js, bounded output/input and explicit shell/job cleanup. Window closure detaches without stopping the shell; service restart never replays terminal input. Closed native/service messages cannot choose executable, cwd or environment. The owner shell intentionally has same-user authority, not a sandbox. No-paid Rust socket round-trips and a no-image WebKitGTK check cover input/output, resize, window reattachment and theme/font behavior; package installation, remote terminals and task/worktree-specific shells are not qualified.

Workspace wire 1.3 adds bounded attachment bytes to task messages and multiline Other answers. Picker, drop and pasted-image input preserve files through queue Edit; the runtime retains private prompt resources instead of filename-only hints. The Linux window keeps webview drops enabled and handles WebKitGTK empty clipboard/URI-only drop behavior through closed native byte snapshots. No-paid resource journeys and a fresh WebKitGTK 2.52.6 binary check cover clipboard paste, an external GTK file drop, queue/Edit and fake-engine resource receipt without screenshots or image inspection. This is not package-install or paid-engine qualification.

Workspace wire 1.2 saves per-task configuration and checkout choices. Task worktrees run against the shared project store and supply their own diff. Integration adds target-branch squash and GitHub CLI actions with verification, dirty-target and ambiguous-PR guards, plus retained source conflicts and owner/Cortex resolution controls. Archive deletion now removes runtime rows through journaled cleanup and attempts space reclamation. Automatic sprint advancement runs in the service child, not the window. Project routing/ceiling overrides and project removal are connected; the project menu supports a native VS Code action. LLM titles remain unavailable. Pre-save MCP listing now connects the draft to a bounded sandboxed listing without saving or calling tools. Open logs opens private rotating status categories, never raw stderr. Large/non-text conflicts expose manual Git instructions, clean task worktrees can be removed immediately after merge, and source updates require fresh result evidence. Sprint routes/targets are inherited at task creation, duplicate-connection routes remain selectable, partial setting failures are explicit, and storage shows failed space-reclamation counts. These source changes are not newly installed-package or rendered-binary qualification.

Workspace conversation now reads durable Cortex history through wire 1.1, including plans and tool activity; owner messages, unsent queue and review drafts survive reopening. Worker prose is retained at tool boundaries without expanding native worker cards. Direct worker messages reach the next successful Cortex tool response; worker and gateway-command stops use existing runtime control paths. Review feedback returns to Cortex, clears stale acceptance and produces a new plain result ordinal instead of displaying the runtime entity version. Check reruns record a mandatory request, not a fabricated pass. Stop remains non-resumable. No-paid fake-engine checks cover the real service, screen and native binary; this does not qualify paid engines or a newly installed package.

Workspace Debian packages now include locked Codex/Claude adapters, their dependencies and checksum-verified Node 24.14.1. First activation needs no npm or adapter download; system Node/npm are no longer package dependencies. Discovery prefers the channel bundle and can relocate an existing same-version connection without changing account references. A no-network Ubuntu check installs the package and initializes both real adapters through Cortex without a prompt. On 2026-11-06 the owner reviewed Anthropic's published terms and chose public distribution with native-account login unchanged, accepting the unresolved authentication interpretation risk. This is not Anthropic permission or a legal conclusion; see the [licence inventory](docs/project/bundled-engine-licenses.md).

The real Workspace app now shows a task the way the accepted design does. The title band and plan bar name the real state and runtime phase, the Inspector and the worker pane are on, workers appear in the conversation, and Runtime verification reports the result's own checks. The Queue no longer sends straight away: a queued message waits in the app, Edit puts it back in the field, and Send now delivers it. Questions asked as a batch can be answered one by one and go to the machine together. A new task shows where it works and where it merges, and can start at once. A message to a worker goes through Cortex, and the field says so. Stop says it ends the run, because a run cannot be resumed yet. A task that has no route of its own shows what its machine sets for every task.

The Workspace desktop app now opens every screen of the accepted design on real data instead of "Not connected in this version". Code and Git read the real repository, with stage, commit, fetch, pull, push, branches and worktree removal. Workers come from the project store. Orchestration, Connections and Files edit real Cortex settings through its configuration API, with revision checks. Tasks can be stopped, steered, renamed, accepted, merged into the open branch, closed, archived, restored and deleted from the Archive. Sprints run a Cortex planning task, create their tasks from the plan and can run them in order. The Cortex logo is now the window, dock and launcher icon. Still off, with the reason on the control: the terminal, voice input, attachments, remote machines, per-task model choice and per-task worktrees.

The Workspace desktop window now looks like the accepted design. It has one title band with working minimize, maximize and close, a drag region and edge resize, instead of an OS frame above a fake button group. The page fills the window with the theme surface from the first paint, the split button no longer stretches across the band, the setup notice no longer pushes the band down, and Roboto and Cousine ship with the app. A new rendered audit measures all of this in the real binary on X11 and Wayland and fails on regressions.

Workspace Debian packages now bundle the complete Cortex runtime and host for both isolated channels. Units no longer need a checkout or environment file. Login startup, bounded native app-driven service start/retry, best-effort upgrade restart and removal stop replace manual service setup. First activation runs the existing native-account onboarding, and the desktop shows engine setup/login diagnostics separately from service connectivity. The Docker installer smoke checks normal-user activation, dependency installation, legacy archive upgrade and data-preserving removal; systemd-manager and sandbox gaps are not treated as passes.

Cortex Workspace, a desktop application with a user-level machine service, has its local vertical slice built and checked without paid calls: a real Ubuntu window in a container build, a closed local service with a `_cortex/workspace` adapter in Cortex, identified question answers, explicit starts and read-only results. The slice is not released. Isolated dev and release channels, a dev settings clone and a refusal rule for unowned project stores are included; manual GitHub Actions runs on `main` publish production as Latest (`v<debian-version>`), while `dev` runs publish a separate dev prerelease (`dev-v<debian-version>`) explicitly excluded from Latest. Both share one release list; dev installs manually and has no in-app updates.

Paseo 0.15.0 is qualified for the CLI runtime and protocol behavior exercised by the owner-requested paid `cli` checks, through the isolated dev Paseo daemon and disposable projects only, on `codex-acp/gpt-6-luna:high`. A lifecycle run with mandatory independent review and verification passed; a model remark on a Git-initialized fixture passed with the unchanged-project review waiver; and a user question whose answer resumed the run passed. A first model-remark run used a non-Git fixture that did not satisfy the review-or-waiver baseline; that failure is retained in the dev environment guide and is not attributed to Paseo 0.15.0. This release changes no runtime behavior and records the CLI qualification and the package version bump. It does not establish rendered desktop/GUI behavior, release readiness or qualification of other Paseo versions.

## 1.0.5 (unreleased)

A worker that published its report but ended its turn without `result_submit` is now shut down and released. Before this, the runtime recorded that report as the worker's result under the assignment key instead of the attempt id. The finished-worker shutdown then tried to stop an engine under the assignment key, matched nothing and never retried, so the worker's engine stayed live. Every later close-out stopped with `workers_still_live_or_unknown`. A live Codex worker held a finished task this way for about three and a half hours. The shutdown now resolves that key to the provisioned attempt; a key with no attempt is not stopped. That stop question now names each worker still live or unconfirmed by node id, role, engine and observed state, in English and Russian. This does not establish live qualification.

A failed automatic model discovery now says why. The per-engine first-run status and the Settings engine row keep a fixed failure code instead of only `catalog_failed`, never adapter output. A new code, `engine_home_read_only`, names an engine account directory such as `~/.codex` that the Cortex process cannot write; Codex app-server exits at startup then. A fresh dev install hit this when its daemon was started inside a Cortex command sandbox that mounts `~/.codex` read-only; seeding the default profile and disabled MCP examples did not cause it, and model discovery never passes MCP servers to an engine. The guarded dev wrapper now also refuses to start the daemon or GUI when `~/.codex` or `~/.claude` is mounted read-only. This does not establish live qualification.

Connections no longer replaces its form with a loading placeholder every three seconds. Passive refreshes keep row identity/order, draft inputs and removal confirmation, skip structurally identical data, and still update live status and newly registered connections. Failed refreshes retain visibly stale data with retry controls instead of hiding rows; save/conflict feedback is preserved. These changes do not establish rendered desktop qualification.

Fresh installations now show one selected, applied profile named `default`, not just default-looking routing values. The bundled preset also registers disabled `codebase-memory-mcp`, `playwright` and `vivaldi` examples using placeholder paths and no environment secrets. Settings shows and can edit/toggle these entries; no MCP executable is downloaded or launched by seeding. Existing settings, MCP registries and saved profiles remain untouched, including malformed files. Custom settings are not relabeled as an applied default. Inactive OpenCode model examples were removed to match the current portable owner baseline; OpenCode remains manual and disabled. This does not establish live or rendered desktop qualification.

Claude plan usage no longer uses the legacy session / 5-hour window. Weekly and scoped windows still drive quota bands, routing eligibility and reset times; older session events/caches are ignored and weekly-only CLI output is valid. Settings help and current window display follow the same policy. Missing usage remains unavailable, never inferred hot. Codex windows and actual engine-refusal handling are unchanged. This does not establish live qualification.

A coordinator that answers a simple question in plain text instead of calling `answer_submit` is now told why its reply did not count. Before this, the runtime resent an identical prompt twice and then stopped with `structured_proposal_missing`. A live Codex coordinator hit this after the user asked to "answer only the number" and "not use tools". The second and third planning or reconciling prompts now start with a repair note: the text was not recorded, so the turn must call the named structured tool. When `answer_submit` is offered, the prompt also says that even a one-word answer goes through it and that a user's no-tools request does not forbid it. This does not establish live qualification.

Plugin activation and ACP startup now run automatic first-run setup in the background: install the qualified pinned Codex/Claude adapters, connect an existing native account login when its marker exists, and load models immediately without a model prompt. Settings shows progress and failures while keeping manual actions available. Removing a connection records a persistent per-engine opt-out so startup cannot recreate it; explicit Add connection can restore it. Registering from Settings also refreshes models immediately; a discovery failure keeps the connection and previous catalogue. `CORTEX_ENGINE_UPDATE=off` disables the automatic pass. OpenCode stays manual and API keys remain command-line only. These changes do not establish live or rendered desktop qualification.

First-run setup in Paseo is more complete. Each Cortex agent start now creates a missing `routing.json` or `defaults.json` from the bundled default preset and never replaces an existing file. Paseo Settings shows each engine's status, can download Cortex's pinned `codex-acp` or `claude-agent-acp` adapter, and can add a native-account connection after an explicit opt-in; API-key connections remain setup-command only. The default-preset card applies the preset to a missing file, or resets both user files after confirmation with a private backup of each. The configuration, packaging, security and engine pages describe these paths. These changes do not establish live qualification, paid-model behavior or rendered Paseo GUI behavior.

A Git install of the Paseo plugin into a second Paseo instance with its own `XDG_CONFIG_HOME`, such as the isolated dev daemon, no longer fails in its build step. Setup now writes `paseo-plugin-launch.json` into the Cortex config root of the environment it runs in (`$XDG_CONFIG_HOME/cortex/`, or `~/.config/cortex/` without a custom `XDG_CONFIG_HOME`), so each instance has its own record and the owner's existing record is never replaced. Previously every instance shared `~/.config/cortex/paseo-plugin-launch.json`, and the build step refused to overwrite the other installation's record. The provider no longer falls back to another instance's `~/.config` record. A relative `XDG_CONFIG_HOME` is refused. The guarded dev wrapper's new `--installed-runtime` flag runs a Git-installed plugin from its staged runtime instead of the checkout, and the wrapper refuses Git installs and updates without that flag. These changes do not establish live qualification.

In Paseo, choosing a Cortex model without effort levels, such as Claude Haiku, no longer fails to start the chat with "ACP session does not expose thinking configuration" when the composer still holds a thinking choice from another model. The plugin drops a thinking choice the selected model does not offer, so Cortex keeps that model's own effort; an offered effort is applied as before. This does not establish live qualification.

Settings' **Download engine** and the background engine update now install the same adapter: the manifest pin, raised to the current qualified codex-acp 2.3.0 and claude-agent-acp 0.97.0. Previously Settings installed the stale pins 2.0.1 and 0.78.0, while the background update downloaded npm's latest, unqualified releases, which can never launch. A sandboxed engine whose Cortex-owned install lives under `/var/tmp`, as with an `XDG_DATA_HOME` there, now starts: the jail's private `/var/tmp` hid the install, so every Codex task launch failed with `EngineProtocolError` while model lists still loaded. The verified install is now mounted read-only into the jail. These changes do not establish live qualification.

Routing settings now offer each exact model's observed effort levels for effort
pins, `effort_floor`, `max_effort` and ordered `effort_ladder` values.
Unknown models use a marked global fallback, while stored unsupported values
remain visible and unchanged. These changes do not establish live qualification
or release readiness.

## 1.0.2

This release refreshes Terra routing preferences in the fresh-install default
preset, adds exact support for codex-acp 2.3.0 and claude-agent-acp 0.97.0,
and automatically follows newer approved installed adapters. Set
`CORTEX_ENGINE_FOLLOW=off` to opt out; the startup environment handling is
corrected. These changes do not establish live qualification or release
readiness.

## 1.0.1

Since 1.0.0, orchestration and recovery fixes improve steering, worker
visibility and evidence handling. In Paseo, the busy-steering acknowledgement
is now kept within its own message boundary, so it cannot consume or hide the
following worker-card update; worker cards remain visible while their workers
continue. The release also includes the routing, lifecycle, storage-inspection,
qualification and packaging safeguards detailed below. These changes do not
establish live qualification or release readiness.


A model requirement naming a dotted identity now routes to that identity. A
period inside a token (`codex-acp/gpt-6.1-sol`, `AGENTS.md`, `1.0.0`) no longer
ends a sentence, so `All audits and reviews must use codex-acp/gpt-6.1-sol.`,
the same identity in backticks and the bare `gpt-6.1-sol` are read whole in
instruction files, user messages and chosen question options. Previously the
rule split into fragments that named no model, granted nothing, and the
mandatory reviewer ran on another model. A question about a "must use"
requirement ("Do reviews have to use Sol?", "Почему аудит должен использовать
Sol?") now requests nothing, like the passive forms; negations, alternatives
lists and remarks still grant nothing, and ordinary sentences such as "Use Opus.
Then review." still split as before, including after closing Markdown emphasis
("**Use Opus for the review.** Never use Sonnet." still requests Opus).

Selected recheck options now retain the user's model at explorer dispatch:
`recheck`/`перепроверить` and commit-list/history work objects are recognized,
without assigning a model to unrelated role purposes. Previously an explicit
choice could be dismissed as an unconfirmed hint, starting a cheaper worker
that then had to be cancelled. Negated, past-work, unselected and ambiguous
options still grant nothing. Mandatory-review regression checks also ensure
project model requirements override the opposite-provider preference before
engine open, or refuse without a substitute when hard policy denies the model.

Orchestration audit fixes tighten path reservations, dropped-check certification,
worker-question continuation and consultant access. Model routing now requires
recorded user consent; storage inspection remains non-mutating and recovery
failures stay visible. Role guidance and generated profiles are synchronized,
and cancellation and installed-Paseo regression checks are corrected. These
changes do not establish live qualification or release readiness.

A model requirement in the project root's own `AGENTS.md` or `CLAUDE.md` now counts
as the owner's model request for the role purpose it names, with the same precedence
as a recorded user request: ahead of the coordinator's hint, pins, owner preferred
models and usage, and still refused visibly by hard owner policy, unavailability or
exhausted quota. Previously such a rule reached routing only as an unconfirmed
hint, so a re-audit required to run on Opus ran on Sonnet under the owner's review
preference. Dispatch and the plan preview record it as `project_instruction` with
the file path and line; a newer user message outranks it, prohibitions never grant,
and a sentence naming no role purpose grants nothing. The request grammar also
recognizes passive requirements ("must be performed by Claude Opus", "аудит должен
проводить Opus") in user messages and instruction files alike, but not questions
about them ("Does the audit have to be performed by Opus?"). A model in a sentence
covers only its own work, so "Implementation must be performed by Sonnet and
reviewed by Opus" never gives reviewers Sonnet, while the work named before the
model stays its purpose ("Проведи ревью на Opus и обнови документацию", "Run all
audits on Opus before deploying"). A list of alternatives ("Audits must be performed
by Opus or Sonnet") permits a choice and demands none of them. Only the file as it
stood when the task started counts: a file changed during the task, including by
the task's own workers, grants nothing for it, and the dispatch decision and plan
preview show `changed_during_task` with the file path. Newer imported history no
longer hides an older user refusal of the file's model. Regression coverage also
checks an unhinted runtime-created reviewer: the project requirement beats the
opposite-provider preference, and a hard-denied required model refuses at the
dispatch boundary instead of selecting a substitute.

Recorded user model requests now also recognize restrictive selections
("Выполнить аудит только Claude Opus", "только Opus", "do the review only on Opus")
and an exact identity in parentheses after a model name. A real user answer that
is exactly one of its question's options grants the single model that option
selects, in plan preview and at dispatch alike. It covers the work the option names,
or else the purpose its question asks a model for, for unhinted review nodes; a
sentence judging a result never supplies that purpose. A bare model name, alone or
after a lone verb of choice ("Выбрать Opus", "Pick Opus"), grants only under a
question that asks which model to use (including "Какой модели поручить
аудит?"). An option that only mentions a model
("Принять результат аудита Opus", "Opus исчерпал квоту"), such a name chosen
under "Чей результат аудита принять?", "Чью проверку использовать?", "Какой
аудит выбрать?", "Какой выбрать аудит?" or "Какой в итоге выбрать аудит?", question text, unchosen options,
coordinator `model_reason`, remarks, negations and imported history still never
grant. "Какую выбрать на аудит?" and "Which one should we use on the audit?" ask
which model to use, and a parenthesized usage note ("Opus (квота 52%)") does not
stop an option from selecting its model. Quota and limit words are not refusals:
"Сделай аудит на Opus в пределах лимита" still requests Opus and revokes no earlier
grant. Hard owner deny, unavailability and exhausted quota still refuse without
substitution.

Plan usage is now only the last tie-breaker between otherwise equal routes (rank
version 15): the 10-point "meaningfully less used ranks first" step is removed, so
usage no longer outranks engine affinity or retry diversification. A route the
coordinator names that is eligible is kept whatever its usage, unless its known
usage is at least 95% while another candidate is not hot; the replacement is
recorded as `ignored:near_quota_limit`. Owner preferred models for the work kind
still outrank a hint naming another route, and the plan preview now reports that
(`ignored:owner_preferred` with the `owner_preferred` routes) instead of
`pending_dispatch`, so a legitimate choice among the owner's models is not
treated as a mismatch. Explicit user requests keep precedence. `list_route_options`
still shows quota bands and used percentages. New audits drop `usage_band` and the
usage `margin`.

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
