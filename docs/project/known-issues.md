# Known orchestration issues (open)

Recorded 2026-11-16 from a read-only audit of the native Rust path
(`crates/cortex-core`, `crates/workspace-service/src/native_*.rs`,
`crates/workspace-service/src/incoming*.rs`, `crates/cortex-acp`). The Python
legacy runtime was not audited. Fixed entries have been removed from this page.

Each entry states how it was established:

- **Confirmed (code)** — the failure path was traced in source by hand.
- **Confirmed (test)** — an existing automated test fails on it.
- **Reported** — found during the audit and traced to file/line, but not
  re-traced independently; re-verify before fixing.
- **Plausible** — a likely defect whose full path was not verified.

Line numbers refer to the working tree on the recording date and will drift.
When an entry is fixed, remove it from this page and describe the new behavior
in the affected feature page.

## Failing automated tests

On 2026-11-16, after the fixes, two consecutive full runs of
`cargo test --offline --workspace --exclude cortex-workspace-desktop --exclude cortex-workspace-prototype --no-fail-fast`
passed. Every test listed here earlier now passes. On 2026-11-17 the intermittent
`config_engine_install_tests::npm_is_killed_at_its_bound_and_leaves_staging_for_the_next_pass`
failure was traced to its own lock check: a sibling test thread forking while the install held
the lock briefly shares the lock's open file description until its child execs. The check now
waits up to 5 s for that window; four consecutive `workspace-service` lib runs passed.

## Open limitations (2026-11-16)

### KI-25 An answer turn can show a short preamble and then the certified answer

**Confirmed (live, Paseo desktop).** In a direct-answer task (no workers) the
coordinator may stream a short sentence during its turn, then submit the answer;
the runtime publishes the certified answer as its own message (`synthesis:` id).
Clients show both. The streamed text reaches the client before the turn's outcome is
known, so it cannot be withheld or retracted afterwards; hiding the certified answer
instead would lose the result. Display only; no text is lost or duplicated in storage.

### KI-26 A first prompt right after connections are replaced can be refused

**Confirmed (live, dev Paseo, 2026-11-17).** When `connections.json` is replaced from
outside with connection ids that have no retained catalogue yet, a prompt sent before
the background catalogue refresh for those ids finishes is refused with
`orchestration_unavailable` (the coordinator route cannot resolve). Retrying after the
refresh succeeds. Connections created through Settings refresh their catalogue when
they are created, so this needs an external file swap. The native reason is not in the
ACP error (`prompt_error` in `crates/cortex-acp/src/python_protocol.rs`).

## Not audited

- The Python legacy runtime under `src/`.
- `lifecycle/probe_correction.rs`.
- Outbox atomicity (`store/outbox.rs`) and `core_host.rs`.
- Python-session history reload (`incoming_history.rs`).
- Paseo cancel-hold timing.
- Live or paid checks (`cli`/`desktop`). These were not run because the owner
  did not request a paid live check.
