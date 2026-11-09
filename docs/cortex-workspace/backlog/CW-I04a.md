# CW-I04a: Desktop CodeMirror 6 editor with tabs and find/replace

**Status:** partly built: editor, tabs, find and replace and save. Compare and save-as in the conflict choices, and a measurement on the reference machine, remain planned · **Phase:** 3+ · **Size:** M

## Goal
Provide the desktop editor in the Tauri webview, built on CodeMirror 6, with tabs, find and replace, and save through the CW-I03 contract. Unsaved buffers are held by the frontend independently of language-server state, so a language-server failure cannot discard edits. Language-server features are in CW-I04b and CW-I04c.

## Requirements
- CW-R10: the editor is bounded, path-checked and protected against concurrent changes.
- CW-R14: the keystroke-to-glyph budget is measured.

See [requirements](../requirements.md).

## Inputs
- [IDE and Git](../ide-and-git.md): CodeMirror 6 for the desktop editor; unsaved buffers survive language-server restarts.
- [Performance](../performance.md): keystroke to glyph under 50 ms; scroll and drag at 16.7 ms per frame.
- The CodeMirror fixture from [CW-F03](CW-F03.md) and the save contract from [CW-I03](CW-I03.md).
- [Screens](../SCREENS.md): desktop file tree, editor and search.

## Deliverables
- Editor component with tabs, dirty markers, and find and replace through the CodeMirror search extension.
- Save action calling the CW-I03 contract, with a conflict dialog offering compare, reload and save-as.
- Buffer store keyed by path and base hash. It is retained across language-server state changes.
- Per-tab undo history.

## Acceptance
Positive:
- Keystroke-to-glyph is measured under 50 ms on the reference Ubuntu machine by instrumented input-to-paint. The method and hardware are recorded.
- Replace-all on a 5,000-line fixture reports the exact occurrence count.
- A tab clears its dirty state only after the service confirms the new hash.

Negative:
- Closing a dirty tab requires explicit confirmation. Edits are never discarded silently.
- A save conflict never overwrites. The conflict dialog is shown.
- When the save response is lost, the tab shows "save not confirmed", never clean.
- Buffer content survives the language-server transport closing. This is checked with a transport stub here; real crash cases are in CW-I04b and CW-I04c.

## Dependencies
- [CW-F03](CW-F03.md) (CodeMirror and xterm fixture), [CW-I03](CW-I03.md) (hash-checked save).
- Gate records: CW-D05 (Gate D).

## Validation
- Rendered run in the Ubuntu Tauri shell at 1280×800 and 1920×1080. Capture screenshots of tabs, find and replace, and the conflict dialog.
- Keystroke timing from an instrumented build.
- Automated find and replace counts on the 5,000-line fixture.
- Record exact commands, environment and outcomes.

## Documentation impact
- [Performance](../performance.md): measured keystroke result, only after execution.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
M: one editor surface with save integration, bounded by the keystroke budget.
