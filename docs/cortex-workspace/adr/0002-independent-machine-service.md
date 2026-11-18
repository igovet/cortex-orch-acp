# Independent machine service

**Status: decided and implemented for the local machine; checked by no-paid automated checks only. Remote machines are not implemented.**

Decision: A separately running Rust service owns local child sessions and machine operations; closing a client is not cancellation. A UI-owned ACP process would couple execution to window lifetime.

Consequences: implementation and qualification are pending; revisit if feasibility or security validation fails.
