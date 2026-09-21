# Historical production Zed sandbox refusal (2026-09-19)

Status: **historical failure record**. This incident documents the old
mandatory-containment path and is not a current installation or repair
requirement.

The user reported that a prompt in their production Zed session failed with
`SandboxUnavailable at sandbox.py:141`. This line is the fail-closed
`bubblewrap` availability check before an engine launch. It is not an engine
authentication result or a failed project edit.

Read-only kernel audit inspection found matching AppArmor denials at
22:48:15 local time: the `unprivileged_userns` profile denied `setpcap` and
write access to a process's `uid_map`. The PID and path were redacted from
this record. In a separate no-paid probe through a disposable Zed profile on
virtual display `:92`, the same minimal `bwrap` command exited 0. A shell
probe also exited 0. These observations show that sandbox availability is
specific to the launched process context; they do not prove the production
Zed context is now fixed.

The then-current Cortex change classified a future denial as
`sandbox_userns_denied`, gave repair guidance in the ACP error, retried only
transient probe failures once, and refused unsandboxed execution. No
production Zed process, credential, AppArmor policy, or user project was
changed during this investigation. The original production prompt remains
failed. The current path no longer performs this OS containment probe and
records the incident as a historical diagnostic; a fresh production Zed/Cortex observation
is required before marking the current direct path qualified.
