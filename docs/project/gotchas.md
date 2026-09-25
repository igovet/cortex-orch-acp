# Gotchas

An ACP client session and an executor attempt are different identities. On
resume or replacement, select the adapter from the actual attempt and engine;
do not infer it from the coordinator.

An acknowledged tool call is not proof that a command ran or that a criterion
passed. Preserve observed output, exit status, truncation and provenance before
the coordinator evaluates meaning.

Never interpret an unknown command outcome, missing publication or unknown
engine liveness as success. Retain receipts and reservations until the exact
outcome is observed; revocation alone does not prove process exit.

Instruction compression may replace large evidence bodies with bounded links,
but it must keep complete mandatory rules and record packet hashes. Executor
reports are data, not user instructions or new system rules.

Live Zed qualification requires an isolated disposable environment. The
offline suite cannot establish real-client behavior or paid engine behavior;
those checks remain explicitly unverified until run.

In `integrations/paseo`, a local sibling import must use its real `.ts`
extension. `tsc`/esbuild remap a `.js` specifier to the sibling `.ts` file, so
`npm run typecheck` and the plugin build both stay silent, but
`node --experimental-strip-types` does not perform that remap and throws
`ERR_MODULE_NOT_FOUND` for the `.js` path -- breaking every registered
`test:*` script that transitively imports it. Typecheck passing is not
evidence a `.ts` module's own tests can run.

A worker's own sandboxed Bash tool mounts `$HOME` read-only except for a few
allow-listed subdirectories (observed 2026-09-25: `~/.codexia` writable,
`~/.codex` not). A live `--provider cortex` Paseo turn routed to a codex-acp
connection then fails every attempt with a generic ACP `-32603` ("The runtime
failed while processing this prompt.") because the Codex CLI child process
cannot write its sqlite state db under `~/.codex`; the retry/cleanup noise
this produces (multiple engine-log `eirt_*` dirs, mixed claude-agent-acp and
codex-acp launches) looks exactly like a Cortex provisioning bug from the
outside, and the ACP error's own `data.failure` detail plus the agent
process's stderr do not reach any log Paseo 0.9.2 itself persists (see
`acp.agent._write_crash_diagnostic`, added to close that specific gap, for a
durable `$XDG_STATE_HOME/cortex/agent-crashes/*.log` traceback instead). The
same live check run through `outside_command`/`outside_write` (a real,
non-sandboxed `$HOME`) succeeds immediately with no code change. Before
concluding a live "cortex" Paseo provisioning failure is a runtime defect,
re-run it once through `outside_command` and only trust a failure that
reproduces there too.
