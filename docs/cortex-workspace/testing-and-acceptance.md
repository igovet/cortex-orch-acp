# Testing and acceptance

**Status: the no-paid checks below run and pass for the implemented desktop. Not verified live: paid model runs, a real Ubuntu desktop session and GPU, a real microphone and a published GitHub release. No release qualification is implied.**

Gate D passed on 2026-11-05 for the clickable browser prototype, recorded in [desktop approval](design/desktop-approval.md). Desktop implementation proceeds through the local vertical slice in the [roadmap](roadmap.md), and no desktop item waits for mobile. Gate M follows Gate D and requires mobile concepts, owner selection and recorded accessibility and offline scenarios. It gates only mobile and Android implementation. Mock evidence and the browser prototype are not production qualification.

Use test-first development for domain and lifecycle rules, sprint dependencies and queue, archive retention and purge, workspace and merge guards, runtime-to-Workspace mapping and mock-adapter contracts. Cover positive and negative acceptance cases, unknown and stale states, races and injected-time boundaries. Use real Git only in isolated disposable fixtures. Visual design is not adequately established by TDD: use rendered walkthroughs, accessibility checks and owner review. Current installed-package acceptance is text-only, without screenshots. The rendered layout geometry gate (`packages/workspace-ui/test/layout-fit.mjs`) proves box geometry in WebKitGTK on fake data at the owner's widths and the window minimum; it gives no visual verdict and exercises no pointer or keyboard input. The scroll gate (`packages/workspace-ui/test/scroll-perf.mjs`) scrolls every long surface of the same fake data in WebKitGTK with compositing on, by scrollbar drag and the real wheel, and fails on dropped or long frames, scroll-time script and DOM changes; its limits and known shortfalls are in [Scrolling, recorded](performance.md#scrolling-recorded), and it is software rendering, not the owner's GPU. Mock evidence is not production qualification.

The former owner-scale walkthrough is historical. The supported installed-package input gate is documented below and in [verification](../project/verification.md#installed-desktop-real-input-gate).

For the local slice, the no-paid checks use fake engines and the scripted protocol peer. A browser run does not qualify the WebKitGTK window. Native checks run in the container build ([platforms](platforms.md)); a native check that cannot run is recorded as blocked rather than passed. Rendered Ubuntu observations are recorded as structured text outcomes, without screenshots.

The no-paid `worktree-bound-probe` case in `crates/workspace-service/tests/native_desktop.rs`
uses real disposable Git checkouts and a fake ACP peer. It exercises overlapping
writes to the same relative path in two worktrees, checkout-specific Changes,
Code, review and verification, and clean removal versus dirty retention after
squash. Run it on a host with Landlock enabled:

```bash
umask 022
CORTEX_REQUIRE_NAMESPACE_FIXTURES=1 timeout --kill-after=5 900 "$HOME/.cargo/bin/cargo" test --locked --offline -p workspace-service --test native_desktop -- worktree-bound-probe
```

This is source-level coverage, not installed-archive, rendered-desktop or
live-engine qualification.

A control is shown to work only when a real pointer clicks it. `scripts/cortex-xtask.sh check-real-input [--channel dev|release] [--prefix /usr] [--app <desktop-binary>]` (the in-container stage of the installed gate; the host side needs Docker) clicks the installed window with X server input (XTEST through xdotool) on a private Xvfb display with a disposable HOME, runtime directory and session bus, and leaves nothing running. AT-SPI is only read, to find a control and to see what the screen shows next; nothing is dispatched into the page. It clicks only once the machine reads Connected, in a fresh home and in a home whose project runtime is away while its saved sprint plan names tasks the runtime has not listed. Checks that drive the page through DOM events, a preloaded probe or accessible actions skip hit-testing, and any check covers only the states its fixtures create, so neither establishes that a person can use the window. A build that is not installed is checked by passing its desktop binary as `--app`, because the service accepts only its installed runtime or a source checkout.

Before any desktop archive is handed to an owner, require the [installed real-input gate](../project/verification.md#installed-desktop-real-input-gate) on that exact archive. It installs through apt in disposable systemd containers, launches the installed desktop entry on private X11 and Wayland screens, uses real OS input and closes its own window. Extracted-binary, browser, preload and accessible-action checks cannot replace it. Keep complete output and the archive-bound result; an incomplete or failed gate blocks handoff.

Paid live testing of the standalone Cortex Workspace desktop is a separate,
opt-in check, not an installed-gate substitute. Default checks remain no-paid.
Use the [standalone desktop procedure](../project/verification.md#standalone-cortex-workspace-desktop)
only with an explicit owner request for this app in `desktop` mode and a
recorded approval reference. The procedure covers candidate identity, private
virtual screen and service, a disposable project and app roots, native
account checks and before/after metadata comparison. Use real input and
observe the actual window with screenshots or recordings; logs alone do not
qualify a rendered case. Virtual-screen evidence does not qualify the
owner's Ubuntu session, GPU or a physical Android device.

## Extracted Dev readiness gates

Run `bash scripts/qualify-dev-deb.sh smoke --archive newest` on the exact candidate
before any separately authorized desktop scenario. Both smoke and drive run a fixed
no-task planning preflight and read-only Landlock command on a disposable Git fixture
inside the extracted service's own startup. A missing, incomplete, truncated, nonzero
or unexpected verification receipt fails the run. The launcher keeps the service's
fresh verifier reference in `nested-verifier.out`; a probe on the bare host is not
a substitute.

Both launch modes record local host facts and one bounded `gh auth status --active --hostname github.com` exit code in `host-facts.txt`. They discard GitHub CLI output and do not invoke the broad `probe` action. The login exit code is a diagnostic, not proof of repository access.

Before window input, the actual extracted desktop bridge records one test address.
The launcher checks the exact argument digest, retains it in `opener-probe.sha256`
and starts the scenario's `opener.sha256` log empty. Both View PR controls use this
same native handoff. No program is selected or run for View PR in the extracted app;
an absent, inactive or unwritable recorder cannot fall back to a browser.
`--capture-opener` is accepted for older drive commands but is no longer needed.

`drive` requires a private authenticated Xvfb and never uses the owner's display: it refuses `--real-display`, a missing Xvfb or a missing `xauth` before any archive handling or driver dispatch. The launcher creates a per-run mode-0600 X authority cookie, starts Xvfb with it and with TCP disabled, and checks that the display belongs to the run's own Xvfb process and differs from the inherited `DISPLAY`. Before every click, capture or driver dispatch it checks that the target window belongs to the extracted desktop process. `smoke --xvfb` applies the same checks; only plain `smoke` may use another display. `bash scripts/test-qualify-dev-display.sh` exercises these refusals without paid calls.
Compare each scenario record with the independently read GitHub URL. This proves
the address handoff, not browser navigation, PR creation or a rendered click by itself.

## Result acceptance when workspace observation fails

Manual acceptance, owner-approved Automatic finish, and unattended acceptance
retry a detected scan race once when the initial scan failed promptly. The
retry keeps the same observation rules and checks the native result again
after capturing the workspace. When sandboxed Git selection is unavailable,
a complete bounded tree scan can still establish the workspace witness.
If that also fails, the result stays pending acceptance with a reason and a
retry action. The runtime records the observation reason and any Git fallback
classification without paths, output, or environment values. The queue keeps
the completed result and waits for workspace observation before continuing.
When the native observer reports that its sandbox could not start, the
acceptance refusal asks the owner to restore local sandbox support and retry.

## Desktop and Android release qualification

Desktop release qualification for the Ubuntu first release covers: test schemas and negative authorization; event replay and gaps; idempotency and races between desktop clients; crash and ambiguous external outcomes; files and leases; Git conflicts and push; the fake-ACP desktop scenario (CW-Q01a); the desktop fault and race matrix (CW-Q02a); rendered Ubuntu accessibility, long histories and performance (CW-Q03a); installer, service and authorization checks (CW-Q04a); and a desktop migration and rollback rehearsal on copies (CW-Q05). The owner pilot is the Ubuntu checklist (CW-Q06a). Mobile qualification is separate and waits for Gate M: enrollment, relay, revoke and network transitions; push unavailable and delivery; the fake-ACP phone leg (CW-Q01b); the phone and device fault rows (CW-Q02b); physical Android accessibility, long histories, APK upgrade and rollback (CW-Q03b and CW-Q04b); and the Android pilot (CW-Q06b). Record exact version, device, commands, outcome and missing evidence. Do not treat mocked or CLI-only results as rendered/mobile qualification. Paid or live external operations need separate authorization.
