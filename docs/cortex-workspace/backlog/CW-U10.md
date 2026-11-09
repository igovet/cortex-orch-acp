# CW-U10: Task composer and session controls

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** L

## Goal

Provide the task composer and the task header with Cortex's core working controls: main model (provider and model), effort, access mode, language, visible context occupancy, and token and plan usage. These are UI controls, not typed commands (CW-R23). Each control sends an intent to the runtime. The value shown as current is the configuration revision the runtime confirms, and unknown values stay unknown.

## Requirements

- CW-R26: the core controls and indicators listed above are preserved in the desktop app.
- CW-R02: controls express intent; they never rewrite a runtime fact.
- CW-R04: configuration changes carry request identity and the expected revision; the server authorizes them.
- CW-R07: unknown context and usage values remain unknown in snapshots and events.
- CW-R15: Cortex-specific configuration uses the public structured adapter, not the generic ACP option list alone.
- CW-R34: selecting a value changes no font weight or size and shifts no layout.
- CW-R36: each connected Codex and Claude engine shows its weekly limit usage (percentage used, window, reset time); OpenCode is excluded.

See [requirements](../requirements.md).

## Inputs

- Current source, verified 2026-11-04:
  - Session configuration options: `provider`, `model`, `thought_level` (effort), `language` and `access_mode` (category `mode`), built in `src/cortex_runtime/acp/config_options.py`.
  - Changing the provider or model always resets `thought_level` (`src/cortex_runtime/acp/agent.py`, configuration-change path).
  - Access modes Development, Guarded and Auto, with their descriptions, in `src/cortex_runtime/configuration/access_mode.py`. An unknown stored value is shown as Guarded.
  - Language `auto`, `en` or `ru` for Cortex's own status, question and stop lines (`src/cortex_runtime/acp/config_options.py`, `src/cortex_runtime/acp/messages.py`).
  - Configuration is a per-task, append-only numbered sequence of `configuration_revisions` with states (`src/cortex_runtime/configuration/store.py`).
  - Context occupancy is the `usage_update` used/size pair and is not a token total. Usage basis values are `observed`, `engine_reported`, `estimated` and `unknown` (`src/cortex_runtime/portable/engine_contract.py`, [observability](../../features/observability/index.md) section 2).
  - Plan usage shown on worker cards is the retained route-selection value, and a missing context snapshot is shown as unavailable, never as zero ([token and plan usage](../../features/token-and-plan-usage/index.md)).
  - Route refusals are visible and never substitute another model ([routing and fallback](../../features/routing-and-fallback/index.md)).
- [CW-U02](CW-U02.md) (task header and discussion), [CW-C01](CW-C01.md) (versioned contract) and [CW-C04](CW-C04.md) (typed read projections).
- [CW-U07](CW-U07.md) supplies machine-level defaults for new tasks.

## Deliverables

- Composer controls: model picker (provider and model from the runtime's current route catalog); effort picker scoped to the selected model; access mode selector with the three mode descriptions; language selector (Auto, English, Russian).
- The create-task form (CW-U01) reuses these controls for its optional settings.
- Typed configuration command through the public contract: set model, effort, access mode or language, with request identity, the expected configuration revision and an owner-epoch check where the contract requires it. Typed outcomes: accepted, already applied, superseded, refused (unavailable route, unsupported effort, revoked device, unauthorized) and unknown.
- Header read-back line: effective provider, model, effort, access mode and language, with the configuration revision they come from.
- Context meter: last reported occupancy and its age. With no snapshot the meter shows no value: no placeholder text and no dash.
- Usage panel: token counts, each labelled with its usage basis; plan usage labelled with its basis (at route selection, or a provider window).
- Engine limit usage: for each connected Codex and Claude engine, the weekly limit percentage used, its window and its reset time, with the snapshot time. OpenCode is excluded because it spans multiple providers. Shown in the usage panel and in the engines and connections settings of the machine.
- Loading, unavailable, stale, permission-denied, conflict and error states for every control, as listed in [SCREENS](../SCREENS.md).

## Acceptance

Positive:

- Selecting a model shows the effort list for that model. Because a model change resets effort, the reset is shown as a visible change to the effort control.
- A changed access mode, model, effort or language shows as current only after the runtime confirms the new configuration revision.
- With a context snapshot, the meter shows the value and its age; without one, it shows no value, and no dash or placeholder.
- Language changes are labelled as applying to Cortex's own system text.
- An engine limit with percentage, window and reset time shows all three with its snapshot age. If only the reset time is missing, that field shows no value and the other fields still show.

Negative:

- A model or effort that the route does not support is shown as unavailable with its reason; it is not silently replaced.
- A stored unknown access mode is displayed as Guarded, never as Auto.
- Context is never shown as zero, or estimated from cumulative tokens, when the snapshot is missing.
- A usage value with basis `unknown` is never styled as a measured total.
- A configuration command with a stale expected revision is refused with a typed reason; the control returns to the confirmed value and the user's pending choice is shown as not applied.
- A configuration command from a revoked device is refused and changes nothing on the task.
- The same request identity with a different payload is rejected.
- An engine limit with no snapshot, or with an expired snapshot, shows no value and never 0%. The engine row carries one state line, "Limit usage unavailable".
- No absent value is rendered as a dash (—) in the composer, the header, the usage panel or the engine rows.
- OpenCode never shows a limit row.
- Selecting a model, effort, access mode or language changes the font weight and size of no control or label and moves no neighbouring element.

## Dependencies

- [CW-U02](CW-U02.md), [CW-C01](CW-C01.md), [CW-C04](CW-C04.md).
- Gate records: CW-D05 (Gate D) must be recorded before implementation.

## Validation

- Unit tests of the view model: model-to-effort reset, unknown access mode mapped to Guarded, absent context versus zero, unknown usage basis; engine limit mapping for a missing snapshot, a missing reset time and OpenCode exclusion. A DOM geometry check confirms that selecting a picker value leaves the bounding boxes of neighbouring controls unchanged at 1280×800 and 1920×1080.
- Contract tests for the configuration command against the CW-C01 harness: stale revision, duplicate request identity, changed payload under the same identity, unknown field, revoked device.
- Rendered run of the composer and header in the Ubuntu Tauri 2 shell (CW-F01) at 1280×800 and 1920×1080, with fixture configurations covering confirmed, unknown and unsupported values. Capture screenshots.
- Keyboard-only path to each control.
- Record exact commands, environment and outcomes. A check that was not run is recorded as not run, not passed.

## Documentation impact

- [SCREENS](../SCREENS.md): core session controls and their states.
- [Protocol](../protocol.md): the configuration command shape and its typed outcomes, once implemented.
- Feature pages under `docs/features/` only after a shipped behavior change; this planned item does not change them.

## Size

L: a composer and header surface, a typed configuration command, and two indicators with honest unknown states. Split before implementation if effort exceeds L.
