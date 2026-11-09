# CW-I01: Canonical file tree, read and search in the Rust service

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Implement the canonical file tree, bounded read and bounded search in the Rust/tokio machine service. They operate on the project root registered through CW-H02. Paths are canonicalized against that root, listings use opaque cursors, and no file content leaves the service except for an authorized, size-limited read.

## Requirements
- CW-R10: bounded file tree, read and search that are path-checked.
- CW-R16: project paths and device identities are protected; traversal and forbidden links are rejected.

See [requirements](../requirements.md).

## Inputs
- [Security](../security.md): restrict project paths and defend against traversal, symlink escapes and private-path access.
- [IDE and Git](../ide-and-git.md): files are bounded and path-checked.
- [Architecture](../architecture.md): the service owns files; clients are views.
- CW-F04 (workspace paths, to be validated against the current runtime source) and CW-H02 (project registration).

## Deliverables
- Path resolver that canonicalizes against the registered root and rejects anything that escapes it.
- Listing with a page-size limit, an opaque cursor and a stable order.
- Read with a byte limit that returns content and its SHA-256.
- Search with result and time limits. It excludes service-private paths (the service's state and credential directories) and project paths excluded by policy.
- Fixture project containing symlinks, nested directories, a private directory and an oversize file.

## Acceptance
Positive:
- Listing a 10,000-entry fixture directory returns pages whose concatenation equals the full ordered listing. An unchanged directory yields identical cursors.
- A read within the byte limit returns content whose SHA-256 matches the file.

Negative:
- Path traversal (`../`), absolute paths outside the root, percent-encoded traversal and NUL bytes are each rejected with a typed error and no content.
- A symlink whose target is outside the root, or inside service-private state, is rejected for read, list and search. It is never followed.
- A path that belongs to another registered project is rejected.
- A read above the byte limit is refused. It is never silently truncated.
- Search returns no matches from private paths.

## Dependencies
- [CW-H02](CW-H02.md) (project registration), [CW-F04](CW-F04.md) (workspace paths, to be validated).
- Gate records: CW-D05 (Gate D).

## Validation
- `cargo test` on the service crate, including the rejection matrix and the 10,000-entry listing test, on Ubuntu.
- Path-resolver fuzz-style cases over the fixture tree.
- Record exact commands, toolchain version and outcomes. The item has no rendered check of its own; rendered use is exercised by CW-U01 and CW-I04a.

## Documentation impact
- [Protocol](../protocol.md): file command shapes and typed errors once implemented.
- [Security](../security.md): the path rules that are implemented.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
L: canonical path rules, cursors, bounded reads and search, with a security-focused rejection matrix.
