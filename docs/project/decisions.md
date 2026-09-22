# Decisions

- Cortex is an incoming ACP stdio agent; the Codex-plugin harness is removed.
- The runtime owns lifecycle, scheduling, routes, permissions and completion.
  Models propose structured plans and submit immutable reports.
- Keep one project-local `.cortex` store and one newest-first `pipeline.md`.
- Work directly in the selected project through the runtime gateway. Serialize
  overlapping scopes and reserve at most four workers per project store.
- Keep each engine's native tools and bound them with an OS jail around the engine
  process (scope-only writes); disable native subagents, user MCP, hooks and
  plugins. Contain commands and external MCP processes with explicit grants;
  unknown effects block replay.
- Require a fresh reviewer, bounded fix cycles and actual command verification.
  Bind review and verification to the observed project revision.
- Preserve all 23 specialist/consultant profiles and the shared report protocol.
  Runtime escalation selects a read-only consultant; models cannot choose routes.
- Compaction and session replacement reconstruct context from durable state.
  Absence of evidence never establishes success.
- Use semantic version `1.0.0` and regenerate the full payload hash after changes.
