# Decisions

- Keep the ACP product independent of the removed Codex-plugin/native-harness
  model. There is no plugin manifest, gateway, lifecycle hook dispatcher or
  native profile registration in the packaged runtime.
- Keep one project-local `.cortex` store and one task `pipeline.md`; retain
  immutable report bodies and durable relations in SQLite.
- Separate requirements, assignments, attempts, candidates, checks and
  coordinator decisions. A retry or revision creates a new attempt and never
  mutates an earlier report.
- Require isolated writable workspaces for changing assignments. Failure to
  create isolation stops the operation; it never falls back to writing the
  user project.
- Verify a candidate before integration. Checks bind to candidate digest,
  requirement revision, environment and attempt; changed scope invalidates
  only affected evidence.
- Treat tool completion, observed command output and semantic acceptance as
  separate facts. The coordinator interprets evidence and records the final
  decision.
- Keep the shared worker report protocol and all generated specialist profiles,
  but deliver role instructions through one versioned `InstructionBundle`.
- Keep advisory knowledge routing and the consultant optional and model-owned;
  neither creates a runtime stage, gate or acceptance authority.
- Use semantic version `1.0.0` for this ACP release and regenerate the complete
  payload hash after installable changes.
