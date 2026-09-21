# Paseo 0.8.0 production-client observations, 2026-09-20

The user explicitly requested testing in the already-open production Paseo
application, using the existing `test-ai` project. Codex and Claude native
accounts were not isolated or copied. This record distinguishes a completed
observation from a later source change that still needs a new GUI run.

## Completed read-only task before the stream/final-answer changes

- Paseo's Cortex diagnostic showed ACP spawn, initialize, session/new and
  cleanup as ready. The combined Cortex picker exposed five Codex and five
  Claude route choices; `gpt-5.6-luna · codex` was selectable.
- The read-only `test.txt` task was `t_56e2399537d8`. One Codex worker was
  delegated, published a result, and the coordinator accepted both the
  assignment and overall task. The project file was not changed.
- The current published pipeline and independent command observation agreed
  on zero newline-terminated lines and SHA-256
  `b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9`.
- The final user-facing answer printed a shorter, incorrect hash. **This is
  a failed final-answer-integrity observation**, even though the durable task
  state was `accepted` and the pipeline value was correct.
- Paseo showed a worker lifecycle card and tool cards. This observation predates
  the current single-worker-card aggregation and is retained as historical
  evidence; the current aggregation's GUI rendering remains unverified.

## Source correction and current qualification boundary

The source now checks fingerprint-shaped strings in an accepted final answer
against exact SHA-256 values in the current published pipeline edition.
Unmatched strings are withheld with a warning; exact matches remain visible.
Focused fake-engine tests cover both the shorter observed typo and a
same-length wrong digest, plus credential redaction. The source forwards
coordinator Cortex calls and aggregates each worker's observable actions and
messages into one worker card; the structured ACP updates remain journaled for
`session/load` replay. Codex `wait` polling is omitted from the display.

A fresh Paseo task was prepared with the `test-ai` project and visible
`gpt-5.6-luna · codex` selection, but the new-chat composer did not accept
keyboard or paste input during this observation. No new prompt was submitted
and no worker/card/final-response result exists for the changed source.
The fresh GUI regression is **NOT RUN**, not inferred from offline tests or
the prior task. The open Paseo client was left running.
