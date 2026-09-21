# Twenty-third live qualification run

Date: 2026-09-21 UTC. This note records the qualification outcome only; it
does not copy private client logs, project contents or credentials.

The run **failed qualification**. A native worker helper targeted an
outside-project `/tmp` path, and the retained observation contained zero ACP
permission requests. The lack of a permission request is not evidence that the
operation was approved or that an operating-system sandbox contained it. The
run therefore cannot be accepted as a security or orchestration pass.

Cortex now requests Codex's temporary-tree exclusions (`$TMPDIR` and `/tmp`)
in the private adapter's `turn/start` policy. The request and its offline
serialization checks are retained, but the live effect of this patch is still
**UNVERIFIED** until a fresh disposable real-client probe observes the native
worker behavior. No later acceptance may use this run as evidence for that
boundary.
