# Human edit and Git concurrency

**Status: decided; implemented for local Code saves and same-project structured file effects.**

Decision: Require a runtime-confirmed file lease and a matching disk revision for Code saves. Refuse conflicting managed file effects or admit them only once the lease is gone. Conflicts remain explicit; never rely on a visual pause or auto-overwrite.

Consequences: This is cooperative exclusion. Git and other external writers do not participate. See [Code editing and write leases](../ide-and-git.md#code-editing-and-write-leases) for the exact boundary.
