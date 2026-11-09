# CW-I04b: Python language server integration (completion, diagnostics, definition)

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Connect a pinned Python language server, run by the Rust service, to the desktop CodeMirror client (@codemirror/lsp-client). Provide completion, diagnostics and go-to-definition for the fixture Python project. This item also introduces the service-side language-server supervisor, which CW-I04c reuses. The server candidate and its exact version are selected and recorded during CW-F03.

## Requirements
- CW-R10: language-server features operate on the bounded, path-checked project.
- CW-R16: the server root and any opened definitions stay inside the registered project.

See [requirements](../requirements.md).

## Inputs
- [IDE and Git](../ide-and-git.md): language servers provide diagnostics and completion; unsaved buffers survive language-server restarts.
- The lsp-client fixture from [CW-F03](CW-F03.md).
- The editor and buffer store from [CW-I04a](CW-I04a.md).
- Explicit project activation from [CW-H02](CW-H02.md) and the path rules in [security](../security.md).

## Deliverables
- Service-side supervisor: starts the server with the project root as working directory, restarts with bounded backoff, and stops on project deactivation.
- Document synchronization from buffer state: open, change and close. Documents are resynchronized after a restart.
- Completion, diagnostic display and go-to-definition navigation.
- Python fixture project with known symbols and one injected error.

## Acceptance
Positive:
- Completion on the fixture returns the expected symbol. The injected diagnostic appears within a recorded bound. Go-to-definition opens the target file at the expected line.
- After a restart, the server receives the current unsaved buffer content.

Negative:
- Killing the server process during unsaved edits leaves the buffer text and dirty state unchanged.
- A definition outside the project root is not opened.
- No server starts before explicit project activation.
- A server that exits during shutdown is not restarted.

## Dependencies
- [CW-I04a](CW-I04a.md), [CW-F03](CW-F03.md), [CW-H02](CW-H02.md).
- Gate records: CW-D05 (Gate D).

## Validation
- Integration test that drives the pinned server binary against the fixture project.
- SIGKILL crash test comparing buffer bytes before and after the restart.
- Rendered check of completion and diagnostics in the Tauri shell on Ubuntu.
- Record the server version, commands and outcomes.

## Documentation impact
- [Architecture](../architecture.md): language-server supervision in the service.
- [IDE and Git](../ide-and-git.md) and [roadmap](../roadmap.md) when qualification changes.

## Size
L: the supervisor, document synchronization, three language features and a crash-safe buffer contract.
