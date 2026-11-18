# Mobile concepts

**Status: decided on 2026-11-17. The owner approved the static phone mockups in `prototypes/cortex-workspace/mobile/` (`index.html`, `mobile.css`) and chose one design direction. The phone interface is built as the shell described below; it has not been checked on a physical device, and no observation of it on a phone is recorded.**

The earlier plan was two clickable mock-only navigation concepts (a Now-first and a Task-first one) and a provisional React Native prototype. The owner decided against that on 2026-11-17:

- The Android client is the **same adaptive interface as the desktop**, built from the shared Solid packages and run in a Tauri 2 Android app. It is not a separate React Native application, and there are not two competing navigation concepts.
- Android connects **only to remote machines** paired from a desktop. The phone has no local machine ([remote access](../remote-access.md)).
- There is **no tab strip** on the phone. Navigation is the desktop's, with the sidebar as a drawer.
- **Larger touch controls on the phone only.** The desktop's control sizes do not change.
- The first version has **read-only Code and no terminal**.
- **App lock is an optional setting, off by default.** It is not built.

What the phone shell does is described in [screens](../SCREENS.md#phone-shell). The approved mockups are static HTML and CSS: they are a design record, not the implementation, and they use no production backend or credentials. The earlier requirements to exercise 360 x 800 and 412 x 915 viewports, large text, touch targets and keyboard navigation, and to record screenshots and actual results including failures, still apply to the built shell and are not done. The rendered layout gates for the phone markup have not run. Record any observation at [Gate M](mobile-approval.md).
