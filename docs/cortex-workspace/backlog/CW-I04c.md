# CW-I04c: TypeScript language server integration

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Apply the same language-server integration to a pinned TypeScript language server and a TypeScript fixture project. Reuse the supervisor and document synchronization from CW-I04b, and add detection of the project configuration (tsconfig).

## Requirements
- CW-R10: language-server features operate on the bounded, path-checked project.
- CW-R16: the server root and any opened definitions stay inside the registered project.

See [requirements](../requirements.md).

## Inputs
- The supervisor and document synchronization from [CW-I04b](CW-I04b.md).
- The lsp-client fixture from [CW-F03](CW-F03.md) and the editor from [CW-I04a](CW-I04a.md).
- [IDE and Git](../ide-and-git.md) and the path rules in [security](../security.md).

## Deliverables
- TypeScript fixture with a tsconfig and a multi-file import graph.
- Pinned TypeScript server version, recorded with its configuration.
- Project-configuration detection: the server root is the directory that holds the tsconfig.
- Go-to-definition across imports.

## Acceptance
Positive:
- Completion, diagnostics and go-to-definition across an import work on the TypeScript fixture.
- After a restart, the server receives the current buffer state.

Negative:
- A project without a tsconfig shows an explicit "no project configuration" state. It does not silently degrade.
- A definition that resolves outside the project root, or to an excluded dependency directory, is shown as external and is not opened.
- Killing the server keeps the unsaved buffer intact.

## Dependencies
- [CW-I04b](CW-I04b.md) (supervisor and document synchronization).
- Gate records: CW-D05 (Gate D).

## Validation
- Integration tests against the pinned server and the TypeScript fixture.
- SIGKILL crash test comparing buffer bytes before and after the restart.
- Rendered check in the Tauri shell on Ubuntu.
- Record the server version, commands and outcomes.

## Documentation impact
- [Architecture](../architecture.md) only if the supervisor design changes from CW-I04b.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
M: a second language configuration on the supervisor from CW-I04b.
