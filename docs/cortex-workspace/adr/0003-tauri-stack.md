# Desktop and mobile stack

**Status: planned; no implementation or qualification is implied.**

Decision: Use Tauri 2/SolidJS on desktop and provisionally React Native on Android; the Android choice is settled by the measured go/no-go in [CW-F02](../backlog/CW-F02.md) against Tauri Android. CodeMirror 6 replaces Monaco for mobile feasibility. Rejected alternatives: Electron with React as the desktop shell, and Monaco as the editor base. Avoid claiming unmeasured performance.

Consequences: implementation and qualification are pending; revisit if feasibility or security validation fails.
