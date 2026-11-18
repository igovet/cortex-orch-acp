# Public contract and event stream

**Status: decided; partly implemented. The local Workspace wire contract is versioned and generated, but the public contract for other clients is not complete.**

Decision: Use a versioned typed adapter, atomic snapshot/cursor and durable outbox, rather than client access to internal SQLite or Markdown parsing.

Consequences: implementation and qualification are pending; revisit if feasibility or security validation fails.
