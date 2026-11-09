# Independent machine service

**Status: planned; no implementation or qualification is implied.**

Decision: A separately running Rust service owns local child sessions and machine operations; closing a client is not cancellation. A UI-owned ACP process would couple execution to window lifetime.

Consequences: implementation and qualification are pending; revisit if feasibility or security validation fails.
