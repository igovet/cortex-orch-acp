# VS Code is not the base

**Status: decided and implemented; the product is a standalone application, and an optional Open in editor action exists in the desktop client.**

Decision: A standalone task-first UI and independent machine service are the product boundary. VS Code extension or fork could provide views, but imposes its host layout and distribution tradeoffs; optional Open in VS Code remains possible.

Consequences: implementation and qualification are pending; revisit if feasibility or security validation fails.
