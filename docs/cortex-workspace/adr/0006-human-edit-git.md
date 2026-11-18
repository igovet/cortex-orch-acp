# Human edit and Git concurrency

**Status: decided; partly implemented. Hash-checked save and Git actions exist; the human and agent write lease does not.**

Decision: Require runtime-confirmed quiescence and hash/version checks. Conflicts remain explicit; never rely on a visual pause or auto-overwrite.

Consequences: implementation and qualification are pending; revisit if feasibility or security validation fails.
