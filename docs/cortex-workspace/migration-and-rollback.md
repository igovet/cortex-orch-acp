# Migration and rollback

**Status: planned; no implementation or qualification is implied.**

The new client is not a replacement of the existing runtime store. Map stable product task IDs to existing execution identities with an idempotent import and explicit ownership; do not assume isolation or import by parsing prose. Before any migration specify source and destination versions, backups, validation, reversible changes and rollback point. Preserve existing project data and avoid modifying production Paseo. On failed import, retain originals, report partial progress and allow safe retry. Never claim rollback succeeded without rehearsal. Runtime storage format migration remains governed by existing project documentation.
