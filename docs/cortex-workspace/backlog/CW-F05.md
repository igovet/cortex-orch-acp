# CW-F05: WebKitGTK GPU/session startup mitigation layer: detect NVIDIA-proprietary-driver + X11/Wayland combinations and apply the documented environment workaround before webview creation, with an owner-facing override and a logged diagnostic

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal

Add a startup layer to the desktop shell that detects the GPU vendor, driver and session type on Linux, then sets the documented WebKitGTK environment workaround before the webview is created. The layer shows the active rendering mode in an in-app diagnostics panel, and an explicit override flag can force any mode. Every override is logged.

Candidate mapping to verify on the owner's installed webkit2gtk before it is adopted:

- NVIDIA proprietary driver under Wayland: `__NV_DISABLE_EXPLICIT_SYNC=1`, for the launch-time protocol error ("Error 71").
- NVIDIA proprietary driver under X11: `WEBKIT_DISABLE_DMABUF_RENDERER=1`, for the blank window.
- Broader fallbacks (`WEBKIT_DISABLE_COMPOSITING_MODE=1`, or `GDK_BACKEND=x11`) are off by default and reserved for an explicit override. They cost rendering performance.

## Requirements

CW-R12 (Ubuntu qualification, including WebKitGTK behavior) and CW-R14 (measured budgets). See [requirements](../requirements.md).

## Inputs

- [platforms](../platforms.md): WebKitGTK GPU and session behavior must be measured on representative hardware.
- [risks](../risks-and-open-decisions.md): WebKitGTK GPU behavior is an open risk.
- [CW-F01](CW-F01.md): the baseline record of GPU vendor, driver and session type.
- [performance](../performance.md): measurement method and hardware must be recorded.

## Deliverables

- A pure decision function: inputs are GPU vendor, driver family, session type and webkit2gtk version; output is the rendering mode and the environment variables to set. Unknown or undetectable inputs produce an explicit `unknown` mode.
- Startup hook that applies the environment before the webview is created.
- Diagnostics panel row showing the active mode, the detected inputs and whether the mode came from detection or from an override.
- Override flag that forces any mode, with the forced mode written to the diagnostic log.

## Acceptance

Positive:

- On NVIDIA with Wayland, the decision sets `__NV_DISABLE_EXPLICIT_SYNC=1`, and launch does not show the Error 71 crash on the owner's hardware.
- On NVIDIA with X11, the decision sets `WEBKIT_DISABLE_DMABUF_RENDERER=1`, and the window renders content (not blank) on the owner's hardware.
- The diagnostics panel shows the active mode. The override flag forces the chosen mode and logs it.

Negative:

- On hardware without NVIDIA proprietary drivers, no NVIDIA-specific variable is set.
- An undetectable GPU or driver yields the default mode and a diagnostic reporting "undetected". It is never reported as mitigated.
- A detection command that is missing or fails (for example a missing hardware-listing tool) yields `unknown` and does not crash the app.
- The environment must be set before webview creation. A test that sets it afterwards must fail.
- An override to a mode that the detected hardware does not need is allowed, but it is logged and the diagnostic does not hide the detected inputs.

If the owner's machine is not NVIDIA, the NVIDIA rendered checks are not run and are recorded as blocked by hardware, not as passes.

## Dependencies

CW-F01.

## Validation

- `mode-decision-matrix`: unit tests over the full input matrix (vendor, driver, session, webkit version, unknown values), including the no-NVIDIA and unknown cases.
- `env-before-webview`: ordering test that fails if the environment is applied after webview creation.
- `nvidia-wayland-launch` and `nvidia-x11-render`: rendered observations on NVIDIA hardware, with the webkit2gtk version and driver recorded.
- `override-logged`: forced mode and log line check.

## Documentation impact

- [platforms](../platforms.md) and [risks](../risks-and-open-decisions.md): record the observed behavior per GPU and session type.
- [performance](../performance.md): note any performance cost of the fallback modes.
- No README change until a shell with this layer ships.
