# Paseo history recovery

Paseo 0.9.1 loads the entire ACP session before applying its timeline cursors.
Those cursors cannot page the initial ACP `session/load`. The repository uses
the installed plugin SDK's connector, `session.opened`, `session.ready` and
`request.failed` interfaces; it does not require a new Paseo API.

The ACP runtime reads one ordered SQLite SELECT snapshot. It validates saved
rows before emitting history. Only adjacent text chunks with the same explicit
message identity and no attachments coalesce (up to 64 KiB of UTF-8 text).
Only adjacent updates for the same card coalesce; a message or another card
ends that run. Ordered source sequence membership and text byte boundaries
travel in ACP `_meta.cortexReplay`. Legacy rows without message IDs get stable
identities based on their stored sequence. Saved rows are never rewritten.

The compatibility relay stages matching load notifications in an unlinked,
owner-only temporary file. The temporary directory is removed immediately;
normal close and process death release the unlinked descriptor. Limits are
1,000,000 events, 512 MiB of serialized UTF-8 frame bodies per staging file,
and 16 MiB per frame. Four-byte record lengths and newlines are excluded from
the aggregate body budget. These are resource ceilings, not truncation targets.
Exhaustion, malformed/truncated frames, corrupt staging or incomplete replay
fails the load explicitly.

After the ACP response, the relay waits for the provider's session-open
acknowledgement. Configuration responses remain available during this handshake.
Replay advances in windows of at most 64 events or approximately 1 MiB (a single
frame may exceed that window threshold, up to the frame limit). Each window
ends with a private, per-load barrier reduced by Paseo's ordered notification
lane. The connector checks count, actual UTF-8 bytes and SHA-256; the gate also
checks delivery of the expected text/card timeline items. The final manifest
must match before `session.ready` reaches Paseo. Barriers never enter displayed
history. User messages retain their original stable identity through the bridge.

Prompts and configuration changes submitted through the provider are refused
while restoration is incomplete. Cancellation, process loss and timeouts return
an explicit incomplete-history failure. A replay acknowledgement has a 30-second
deadline; a provider restore has a 120-second deadline. Reload retries start a
fresh transcript projection with the same durable message and card identities.

Paseo still owns the hydrated timeline in memory. This implementation bounds
relay staging and the bridge's outstanding replay queue, not the total memory
required to display an arbitrarily large history. Paseo's text timeline schema
does not expose arbitrary source metadata; source sequence membership remains
in the ACP projection and original store, while display IDs stay stable.

Run the no-paid integration checks:

```sh
npm --prefix integrations/paseo run test:replay
npm --prefix integrations/paseo run typecheck
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_paseo_load_shim.py tests/test_acp_input_history_and_replay.py
```

The integration test drives the installed SDK against a scripted child, covering
4,097 distinct messages/cards, identity-preserving reload, configuration during
open, corrupt cards and child exits before/during/after the ACP load response.
It also checks staging count/byte boundaries and UTF-8 fragmentation. This is
transport/bridge evidence, not GUI evidence. Actual Paseo GUI reopening is
**NOT RUN** by this implementation worker.
