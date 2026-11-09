# CW-Q04a: Ubuntu installer, service and file/RPC authorization checks

**Status:** planned · **Phase:** 3+ · **Size:** L

Split from [CW-Q04](CW-Q04.md) for the Ubuntu platform.

## Goal
On the reference Ubuntu machine, verify that the installer installs the desktop app and machine service. Verify the service lifecycle: it survives closing the UI and restarts after a crash. Verify the machine-service file/RPC authorization boundary for registered projects; do not claim ACP-child process confinement.

## Requirements
CW-R06 (the machine service survives window closure and reconciles crashes without replaying prompts), CW-R12 (Ubuntu qualification) and CW-R16 (protect project paths; reject unauthorized access). See [requirements](../requirements.md).

## Inputs
- [Architecture](../architecture.md) and [ADR 0002](../adr/0002-independent-machine-service.md): the service owns the ACP children.
- [Security design](../security.md): project-path restriction, traversal and symlink defenses, and private-path refusal.
- CW-Q03a (rendered Ubuntu evidence) and the installer format chosen in CW-F01.

## Deliverables
- Installer run on a clean Ubuntu reference install. It records the installed versions.
- Service lifecycle checks: service starts at login; closing the UI leaves the service and its ACP child running; `SIGKILL` of the service is followed by a restart and reconciliation with no prompt replay (CW-H04).
- Machine-service file/RPC checks: attempt traversal, symlink escape, unregistered-project and private-path access through the service API. Record each authorization refusal. These checks do not establish process isolation; the existing runtime's documented host reads, scratch, real `/tmp` and native engine-state access remain outside this claim (see [SECURITY.md](../../../SECURITY.md#current-acp-boundary-106)).
- Uninstall check: no orphan ACP child remains.
- Update-path checks for [CW-U16](CW-U16.md), once a `.deb` exists: install fixture version N, serve a signed fixture N+1, update through the app, then confirm the installed version, the service health and that no prompt was replayed. A tampered package is refused before any privilege prompt appears.

## Acceptance
**Positive**
- The installer completes on the clean reference install, and the service reports healthy.
- Closing the UI does not stop the ACP child.

**Negative**
- Killing the service restarts it and reconciles state. No prompt is replayed.
- A registration outside allowed roots is refused.
- A symlink escape or traversal probe is refused and recorded.
- An attempt to reach a private path is refused.
- Uninstall leaves no orphan process.
- Reject stale, unauthorized, missing or conflicting input: each probe must show its refusal, not an inferred pass.

## Dependencies
CW-Q03a. Owner gate CW-D05 is mandatory and is satisfied transitively. This item does not depend on any mobile item.

## Validation
- Clean-install run on the reference Ubuntu machine. Record OS version, package version and the commands used, with their exit codes.
- Process listing and `systemctl --user status` checks.
- Machine-service file/RPC authorization probes with recorded outcomes; no inference about ACP-child confinement.
- No check has been run for this planned item.

## Documentation impact
- [Security design](../security.md): service API path-authorization evidence and its limits.
- [Architecture](../architecture.md): installed service layout, once known.
