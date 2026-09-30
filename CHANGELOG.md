# Changelog

## 1.0.0 (unreleased)

Persisted Cortex sessions in Paseo now reopen without reapplying cached model,
mode, effort or settings. A removed or denied saved model no longer prevents
read-only history loading; new and explicit configuration choices remain validated.

Coordinator live-stream redaction now preserves path-embedded script names, including
digit-only, hyphenated and underscored names, while continuing to redact credentials
split across stream chunks.

Cortex is an incoming ACP stdio agent with a project-local runtime and
outgoing Codex and Claude ACP engines. The 1.0.0 release resets the package
identity to a content-stamped ACP payload.

This revision adds durable requirements, assignments and attempts; isolated
worker snapshots; exact staged candidates; candidate-bound checks; recoverable
integration; and explicit revision continuation. Instruction and context packets
record their sources and actual delivery, including session replacement. Report
publication remains separate from worker completion and coordinator decision.

Claude workers work again: resolving the installed claude-agent-acp version
(0.81.0, which has no delivery registration of its own) made every Claude worker
refuse its first prompt while the coordinator kept working; an unregistered
installed version now resolves to the pinned one as before. A failed worker is
recorded with its error code and source line instead of a bare class name, and
a coordinator can read its own role through `read_skill`.

A long chat reopens again. Replay sent every update of a worker card, and each
update carried the card's whole log, so one long worker made 17.7 MB of history.
That overflowed Paseo's 16 MiB history buffer, and Paseo showed "Couldn't
refresh agent history" with no way to continue. Replay now sends each card's
final state once (0.36 MB for the same chat). The Paseo relay also exits when
Cortex exits, so a failed load reports an error instead of hanging.

Paseo development checks now use the persistent isolated `~/.paseo-dev` home
and `python3 -B scripts/cortex_paseo_dev.py` CLI wrapper. CLI log retrieval
validates only what the isolated daemon returns; it is not rendered GUI evidence,
and incomplete history must remain unverified. The observed clone cannot resolve
agent `6956c56` (`Unknown sessionId`) because that archived provider session is
absent from the clone's Cortex store. Long-history completeness, binding of the
`Completed with issues` status to the right worker card, and Codex shell-command
expansion remain unverified in a rendered client. Replayed history may still
show raw model identifiers such as `gpt-5.6-luna`.

The model-specific `gpt-5.6-sol` skill-editing rule was removed; model routing,
including prompt and skill work, is decided by the orchestration runtime.
