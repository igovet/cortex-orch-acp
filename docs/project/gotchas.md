# Gotchas

## Partial reads must never authorize a whole-file overwrite (2026-10-08)

- **The incident.** Production session b91db2c4 (another project, 2026-10-08)
  grew a research file correctly through about fifteen appends by twelve
  workers, from 15,153 to 61,472 characters. Then two workers each read one
  *page* of the file with `project_read` offset/limit, added their entry to
  that page and passed the page as the complete `project_write` content. The
  page's `sha256` was the digest of the whole file, so the optimistic-concurrency
  check passed. The file shrank to 2,909 characters (about 59,500 lost), and a
  later write lost about 5,000 more. The receipts reported no shrink. The
  root cause is not recorded in this document; the pre-loss text is recoverable only
  from worker transcripts, and this change does not restore it.
- **Root cause.** A partial page's `sha256` identified the whole file, and
  `project_write` treated any matching digest as authority to replace the file.
  Nothing compared the new size with the old one, and no read had to cover the
  file. Claude sessions have no native `Read`, `Edit` or `Write` alternative
  (`CLAUDE_GATEWAY_ALTERNATIVES` in `src/cortex_runtime/engine/claude_adapter_acp.py`),
  so the Cortex file tools are the only write path there.
- **Current guards** (`src/cortex_runtime/orchestration/project_files.py`,
  `src/cortex_runtime/orchestration/gateway.py`):
  - Text reads page by 1-based line (`offset`, `limit` up to 2000 lines,
    `line_numbers` on by default). A result stays under 24,000 bytes. A line
    longer than 2000 characters is truncated and flagged, and a truncated line
    never counts as read.
  - Coverage belongs to one channel and one digest. Ranges of non-truncated
    lines accumulate across pages only while the digest is unchanged, and only
    digests and ranges are stored, never file bodies. `full_content_sha256`
    appears only after complete coverage; a partial page's `sha256` is identity
    only.
  - `project_write` of an existing file needs that complete coverage and the
    matching digest. A hash supplied by hand, a partial page's hash, or
    `absent` for an existing file is refused. Creation still uses `absent`.
    A shrink is shown in the receipt (`drastic_shrink`, previous and new bytes
    and lines) and is not refused on its own.
  - `project_edit` applies 1 to 64 ordered exact `old_string`/`new_string`
    replacements atomically. Any prior text read on the channel with an unchanged
    digest is enough. Each `old_string` must match uniquely unless `replace_all`
    is true. An append replaces a unique tail anchor with itself plus the added
    text. If any edit fails, nothing is written. The receipt carries a bounded
    diff.
  - Every successful write or edit replaces that path's coverage on the channel
    with the bytes it just wrote, so the channel can edit its own result again
    without re-reading. That coverage is complete only after a full write or an
    edit of a completely read file; anyone else's later change is still a stale
    read.
  - The write path fsyncs the temporary file before the rename and the parent
    directory after it. `src/cortex_runtime/engine/fs_bridge.py` does the same
    for engine-side file writes.
- **Rules for writers.** Use `project_edit` for any partial change. Before a
  `project_write`, read every page without truncated lines. Never reuse a digest
  from a partial page, from another channel, or from a locally computed hash.
- **Known limits.**
  - A file with a line over 2000 characters cannot reach complete coverage, so
    `project_write` cannot replace it and `project_edit` is the only route. The
    current examples are `README.md` (one line of 2,081 characters) and
    `docs/features/roles-and-delegation/index.md` (two lines over 2,000 characters). Split
    such a line before a whole-file rewrite is needed.
  - The session that wrote this note ran an earlier build: its `project_read`
    still paged by character offset, and its advertised tools did not include
    `project_edit`. The guards above are verified against source and the
    focused tests in `tests/test_project_file_tools.py`. No live run on the
    current build has been recorded.

## Nested user namespaces are not a lifecycle-test dependency

A correctly enforced host policy can deny a second bubblewrap namespace inside
Cortex's sandbox. A lifecycle test then used to end in WAITING_USER instead of
COMPLETED because its verification command could not launch. Test-only launch
seams now execute those checks without nesting; production still requires its
sandbox. Real bubblewrap tests probe once and skip known namespace denial
explicitly, not as a pass. Unexpected probe errors still fail. Run the
`requires_bwrap` cases outside nesting for containment evidence; see
[verification](verification.md#running-the-suite-inside-a-runtime-sandbox).

## Claude adapter auto-update can outrun delivery registration

A background engine auto-update can install a new `claude-agent-acp` adapter version before Cortex has an exact instruction-delivery registration for it. Do not assume the newest installed version is qualified: discovery now falls back to the newest registered install, and pruning preserves registered installs rather than removing them as old versions. If no registered install exists, diagnostics report the actual unsupported version and critical-role dispatch refuses it before launch. This is a delivery qualification guard, not evidence of effective tool enforcement. An owner-requested Desktop run on 0.88.0 (2026-10-08) observed a resumed scenario complete with real model turns, but that bounded, not-clean evidence does not establish native-tool enforcement (Desktop evidence). See [Claude adapter evidence](../features/claude-engine/index.md#current-support-0880-2026-10-08).

## O1/O2/W2: installed-client label humanizing, orphan question-card update, and an over-broad wire-trace .git gate (2026-10-07)

- **O1: the installed Paseo client humanizes any tool-row name lacking
  `:`/`.`/`/`/`__`, stripping a bare argv or report id's dashes and
  lowercasing it (`Shell — python3 -m pytest -q` rendered as
  `Shell python3 m pytest q`).** The plugin's own targeted label
  (`presentToolSemantics` in `integrations/paseo/shared/tool-presentation.ts`)
  built `${action} — ${target}` with an em dash; a target with no dot or
  slash (a bare shell argv, a report-style identifier) left the
  whole string without any of the host's keep-verbatim characters. The
  label now uses `${action}: ${target}`; the colon guarantees the host's
  `/[:./]/` rule matches, so every targeted label renders exactly as sent.
  Regression: `integrations/paseo/shared/tool-presentation.test.ts`
  (including a JS port of the host's own humanizing rule asserting every
  targeted label -- a bare argv, a report id -- survives verbatim),
  `integrations/paseo/server/provider-native-tool-detail.test.ts`,
  `integrations/paseo/server/replay.test.ts`.
- **O2: a generic "Tool call" row appeared right after an answered
  question/question-batch card and never went away.** The question
  card's `toolCallId` (`cortex-question:*`, `cortex-question-batch:*`) is
  only ever carried inside `session/request_permission` params -- it is
  never announced as its own `session/update` `tool_call`. `agent.py`'s
  `_PaseoQuestionElicitor.elicit`/`elicit_batch` nonetheless sent a
  best-effort `tool_call_update(status="completed")` for that same
  unannounced id once the card closed; an update for an unknown id gets
  turned into a new synthetic timeline item (title/name falls back to the
  raw id) by the ACP bridge, which the host then renders as a bare
  "Tool call" row. Both call sites now skip that update entirely -- the
  host retires the form itself once its permission is denied. Defence in
  depth: `integrations/paseo/server/provider.ts`'s `visibleTimeline` also
  drops any `tool_call` item whose `callId` starts with `cortex-question`,
  whatever produced it. Regression: `tests/test_acp_elicitation.py
  ::test_elicit_sends_no_tool_call_update_for_the_never_announced_card`
  and `::test_elicit_batch_sends_no_tool_call_update_for_the_never_announced_card`,
  `integrations/paseo/server/provider-question.test.ts`.
- **`--wire-trace` never activated when `$HOME` itself was a Git
  checkout.** `wire_trace.py`'s `_gate`/`WireTrace._directory_fd` walked
  *every* ancestor of the trace directory for a `.git`/`.cortex` marker,
  including `$HOME` and everything above it -- a developer's own checkout
  living under their home directory silently disabled tracing with no
  daemon-log trace of why (the refusal went only to the runtime's own
  stderr). The ancestor check is now scoped (`_ancestors_up_to`) to the
  subtree from the trace directory up to and including the guarded dev
  home (`~/.paseo-dev`/`CORTEX_DEV_TRACE_HOME`) itself; a `.git`/`.cortex`
  *inside* that subtree still refuses. `scripts/cortex_paseo_dev.py`'s
  `--wire-trace start` now preflights the same gate before ever launching
  Paseo, failing visibly (`--wire-trace preflight refused (...)`, exit 2)
  instead of launching a daemon whose tracing can never activate.
  Regression: `tests/test_dev_wire_trace.py
  ::test_gate_ignores_a_git_repository_above_the_dev_home`,
  the renamed `::test_gate_refuses_production_and_unsafe_optin[dev_home_git]`
  case (a `.git` inside `~/.paseo-dev` itself still refuses), and
  `::test_wrapper_wire_trace_preflight_fails_visibly_without_launching`.

## O3/W3: a mandatory `server/discover` probe and an over-broad trace-viewer `.git` gate (2026-10-07)

- **O3: Claude Code 2.1.285 sends a `server/discover` JSON-RPC request on
  every Cortex MCP channel before `initialize`, and MCP protocol revision
  2026-07-28 (`modelcontextprotocol.io/specification/2026-07-28`) makes
  implementing it mandatory for every server.** `server.py`'s
  `_dispatch` answered it with the generic -32601 "Unknown method", which
  the installed client read as legacy evidence and fell back to the
  ordinary `initialize` handshake -- functionally fine, but a protocol
  error on every connection and non-conformant with the mandatory RPC.
  Cortex still speaks only the pre-2026-07-28 (stateful, per-connection
  `initialize`) wire protocol, so the handler must never claim a
  2026-07-28+ entry in `supportedVersions` (that would tell a modern
  client it can skip `initialize` and address this server statelessly,
  which it cannot do). `server/discover` now answers a valid
  `DiscoverResult` (`resultType: "complete"`, `supportedVersions:
  ["2025-11-25"]` -- the exact version `initialize` negotiates,
  `capabilities` identical to `initialize`'s, `_meta["io.modelcontextprotocol/serverInfo"]`
  carrying the same `name`/`version` as `initialize`'s own `serverInfo`,
  and `ttlMs`/`cacheScope` caching hints): "no modern evidence", so the
  client's own fallback logic proceeds straight to `initialize` exactly as
  before, without round-tripping a protocol error. Session/auth semantics
  are unchanged: the reply carries no channel state and grants nothing by
  itself. Regression: `tests/test_stdio.py
  ::test_server_discover_answers_before_initialize_without_an_error`,
  `::test_server_discover_is_advertised_in_the_unknown_method_message`.
- **W3: `scripts/cortex_trace_view.py`'s `records()` had the same
  over-broad `.git`/`.cortex` ancestor walk W2 fixed in `wire_trace.py`
  -- it refused the real trace directory whenever `$HOME` itself was a
  Git checkout.** It now imports and reuses `wire_trace.py`'s own
  `_ancestors_up_to` helper and applies the identical scoping: when the
  requested `--directory` sits inside the guarded dev home
  (`~/.paseo-dev`), the `.git`/`.cortex` check is scoped to that subtree,
  stopping at the dev home itself; a directory outside the dev home (an
  explicit override) keeps the original full-ancestor walk, since there
  is no fixed, guarded root to stop at. Regression: `tests/test_dev_wire_trace.py
  ::test_viewer_ignores_a_git_repository_above_the_dev_home_but_refuses_one_inside_it`.

## F1/F2: schema-compatible post-result release and attributed advisory skill hints (2026-10-07)

- **A post-result release must not require recreating an existing store.**
  The first F1 patch added a `released_after_result` usage basis, which
  existing SQLite CHECK constraints reject. That approach is removed:
  `EngineSessionManagerAdapter.prompt` keeps `basis='unknown'` when no
  real engine figure arrived, and records `failed=0` plus
  `stop_reason='released_after_result'` when the exact attempt already
  published its result before teardown raised. Real `engine_reported`
  counts remain intact. Both fresh and existing stores use the original
  basis vocabulary; no table rebuild, format bump or manual recreation is
  needed. `usage_summary` derives `released_after_result_calls` separately
  from genuine unknown calls using these existing columns; token metrics
  still report unavailable when no call supplied a real figure.
  Paseo's worker-history and pipeline readers fetch both columns and skip
  only non-failed unknown rows with that exact stop reason. Release-only
  cards render no token alarm, but normal unknown calls and failed rows
  (even with the same stop reason) still count as unavailable. This is a
  presentation distinction, not evidence that the call was free.
  Historical unmarked rows remain unchanged and unknown.
  Regressions: `tests/test_usage_accounting.py
  ::test_release_usage_reopens_pre_fix_store_without_schema_migration`
  creates the table from frozen pre-fix SQL, reopens it, preserves earlier
  real usage and records the release successfully without changing the
  table SQL or format. `tests/test_observability_usage.py` exercises the
  adapter's release branch and real-reported preservation.
  `integrations/paseo/server/worker-history.test.ts` exercises the actual
  card reader plus UsageMeter label helpers, including genuine unknown and
  failed calls; `server/pipeline.test.ts` covers task/coordinator totals.
- **An advisory skill-gate hint on a `project_write`/`project_mkdir` receipt
  ignored attribution, charging a code-only worker for a sibling's
  unrelated docs edit.** `orchestration/gateway.py` calls
  `skill_gate.required_before_result_submit(..., advisory=True)` on every
  successful gateway write to build its early, non-error hint;
  `skill_gate.py`'s own `if not advisory:` guard only ever narrowed the
  observed shared-tree delta to this attempt's own paths
  (`attributable_changed_paths`) for the hard `result_submit` gate, so the
  advisory hint always used the *whole* shared-tree delta -- a parallel
  worker that only ever touched code still got hinted about
  `documentation-sync` the moment any sibling wrote `docs/**`/`README.md`/
  `SECURITY.md`, observed live on 2026-10-07 (
  `luna`/`sonnet` each made one extra `read_skill` call after `sol` wrote
  `docs/project/index.md`). `required_before_result_submit` now calls
  `attributable_changed_paths` in both modes, and advisory mode emits no
  hint at all (rather than one built from the unattributed delta) when
  attribution itself is unavailable (`no_declared_scope`) -- the
  `unobservable` observability event is still recorded only for the hard
  gate, not for a silent advisory skip. Regression:
  `tests/test_situational_skill_gate.py
  ::test_parallel_workers_do_not_inherit_each_others_documentation_gate`
  (flipped to assert no hint) and the new
  `::test_advisory_hint_is_attributed_like_the_hard_gate`. Since 2026-10-13
  the hint observes no tree at all: it uses only this attempt's own gateway
  write paths, which need no attribution (a per-write tree scan cost up to
  0.6 s on a large project); native edits count first at `result_submit`.

## G1-G8 fix-cycle low findings H1-H3, and two test-text drifts (2026-10-07)

- **A worker's first codebase-memory-mcp call failed on an unindexed
  workspace because the Cortex prompt asserted an unobserved fact.**
  `engine.unified_system_prompt.code_index_project_name` derives a plausible
  index project name from the workspace path convention alone, with no way
  for this runtime to observe whether that external server actually has the
  workspace indexed. `engine.session_manager.create_session` and
  `_opencode_prompt` used to call it and assert the guessed name as fact
  ("The index project for this workspace is `X`; use it directly"); when the
  workspace was not indexed, a worker's first `search_graph`/`get_architecture`
  call against that guessed project failed. Both call sites now always pass
  `code_index_project=None`, so the prompt instead always leads with the
  `list_projects`-first instruction (`attached_servers_section` still accepts
  an explicit, truly-confirmed name from a future caller that can observe
  one -- see its existing unit tests). Regression:
  `tests/test_unified_system_prompt.py::test_session_manager_never_guesses_the_code_index_project_from_path`.
- **H2's narrowed bare-scheme rule left every non-"bearer" authorization
  scheme leaking in full, and its own fix regressed into a worse leak.**
  The original narrowing of `observability.redact._BEARER` from
  `(?:bearer|token)\s+<8+ chars>` down to `bearer`-only stopped the "token
  handling" false positive, but a real `Authorization: token <credential>`
  header (or `Basic`/`Digest`/`Custom`/any other RFC 9110 scheme word) then
  leaked: `scrub_text`/`scrub_display_text` redacted only the scheme word
  (`Authorization=[REDACTED] <credential>`), and `StreamingRedactor` leaked
  the header completely (31 of 31 two-way chunk cuts), because its holdback
  only ever recognized the literal words `bearer`/`token`. The fix adds a
  dedicated, scheme-agnostic authorization-header rule (`_AUTH_HEADER`,
  S1 in the fix spec) that runs before every other credential rule: any
  `*authorization`-labelled key followed by `:`/`=` and either a quoted
  value or an optional scheme word plus its credential (with any
  `, param=value` continuation, e.g. `Digest`/AWS-style) is redacted as a
  whole, regardless of the scheme name -- `Authorization: Token X`,
  `Authorization: Basic X`, `Authorization: Digest a="b", c="d"` and a
  quoted `{"Authorization": "Token X"}` are all now fully redacted. The
  bare-scheme rule (`_BEARER`, S2) still narrows `token` to only a
  credential-shaped value (8+ chars containing a digit, no label needed),
  so "token handling"/"the token budget is 4096" remain untouched; a real
  `token`-named credential is still also caught by the `key=value`/
  `key: value` assignment rule (`_KEY_VALUE_SECRET`, extended with a quoted-
  key form so `{"token": "X"}` is caught too) or the "token is/was X"claim
  rule (`_SECRET_CLAIM`). A new known-token-format rule (`_KNOWN_TOKEN`)
  redacts GitHub/GitLab/Slack/OpenAI/AWS tokens and three-part JWTs
  regardless of context. `StreamingRedactor._hold_start` now also holds a
  release point back whenever a completed match of any of these rules
  straddles it, or an open header/claim/key/scheme context extends to the
  end of the current line -- the streaming result is now identical to the
  whole-text result for every tested two-way chunk cut, char-by-char
  partition, and the one-chunk case, with `preserve_paths` both on and off.
  The gated developer wire trace (`observability.wire_trace.Redactor`) had
  its own instance of the same regression for free text: its `_ASSIGNMENT`
  pattern treated the authorization header's own scheme word (`token`) as
  the "value" to remember, so every later literal "token" in the same
  payload was globally redacted too; a dedicated `_AUTH_ASSIGNMENT` pattern
  now skips an optional scheme word and remembers only the real credential.
  Regression: `tests/test_observability_redaction.py` (`test_t1_*` through
  `test_t9_*`, plus the retained
  `test_scrub_text_never_redacts_the_ordinary_word_token_followed_by_prose`
  and `test_scrub_text_still_redacts_a_token_assignment_or_claim`),
  `tests/test_streaming_redaction.py::test_t7_streaming_partition_invariance_for_h2_cases`,
  and `tests/test_dev_wire_trace.py
  ::test_redaction_authorization_token_header_free_text_preserves_ordinary_token_word`.
- **Every worker card showed "Tokens unavailable · 1 call unavailable"
  even for a fully successful attempt.** When this exact attempt had
  already published its `worker_result` (its `result_submit` was accepted)
  and the runtime then tore the engine session down mid-turn (the
  surviving in-flight `prompt()` call sees the resulting `EngineTurnFailed`,
  e.g. `released_mid_turn`/`transport_eof`), `delegation.live_adapters
  .EngineSessionManagerAdapter.prompt`'s `finally` block always recorded
  that call as `failed=True`/`basis=unknown` -- the runtime ending an
  already-successful call is not the same thing as that call failing. The
  `finally` block now checks `task_events` for a `worker_result` row
  matching this exact `attempt_id` before writing the usage row, and
  records `failed=False`/`stop_reason="end_turn"` when one is already
  there. Regression:
  `tests/test_observability_usage.py::test_release_after_accepted_result_is_not_recorded_as_failed`.
- **Two tests asserted stale text instead of the behavior a prior fix cycle
  intentionally changed.** `tests/test_error_help.py
  ::test_an_uncertain_outcome_is_never_reported_as_rejected` still asserted
  the old generic storage-failure wording ("identical retry fails the same
  way") that G2 deliberately replaced with the precise keyless-vs-keyed
  retry semantics in `contracts.error_help` (see that function's own
  docstring); the test now asserts the new wording. `tests/
  test_acp_elicitation.py::test_elicitation_not_advertised_uses_text_fallback`
  asserted the text fallback's first chunk started with the bare question
  text; G6 added the numbering the client's own `/cortex-answer` command
  uses (`"1. Which environment?"`), so the test now matches on the
  numbered prefix.

## Paseo UI: question-batch tabs and provisioning-failure cards (2026-10-07)

- **Batch question tabs show a topic, never the raw question id.** The
  advertised `user_question_request`/`ask_question` batch item schema
  (`QUESTION_FORM` in `orchestration/contracts.py`) now carries an optional
  bounded `topic` (<=24 characters); `question_batches.forms` validates it.
  It flows through the existing `QUESTION_BATCH_CREATED` event payload (no
  storage migration) to `agent.py` `_PaseoQuestionElicitor.elicit_batch`
  and into the Paseo elicitation `rawInput`. Paseo `question-batch.ts`
  (`batchForm`) uses it verbatim as each tab label (`header`); a batch
  member with no topic (an older runtime, or a model that omitted it) gets
  a client-side fallback (`fallbackTopic`) derived from the question text
  own first words, bounded and punctuation-stripped -- never the id.
- **A provisioning failure worker card showed only "role status" with no
  reason.** `orchestrating_runtime._emit_worker_card` already redacted the
  failure line (error code plus detail) into the ACP tool call `rawOutput`
  (`item.error` on the client), but Paseo `worker-tool-call-row` timeline
  transformer (`index.client.tsx`) never read it, so the collapsed chat card
  showed only the role/status subtitle (`workerCardMetadata`). The
  transformer now carries `item.error` through as `failureReason` and the
  card renders it under the subtitle for any non-running card that reports
  one (a genuine failure, a cancellation or an interruption). Separately,
  the generic `assignment_failed` branch in `_emit_assignment_tool_calls`
  now also tries `_worker_base_title` (the helper `assignment_ready` uses)
  so the title carries the resolved route when provisioning had already
  picked one before failing, e.g. during engine session open
  (`unsafe_storage`).

## Paid desktop follow-up: registry, reports and routing (2026-10-07)

- **A detached registry inode is not a hardlink.** An atomic replace between
  open and fstat can produce `st_nlink == 0`. Owner-only regular-file reads now
  reopen by path up to eight attempts, validating each replacement. Hardlinks
  (`nlink > 1`), symlinks, wrong owner and unsafe modes still fail closed.
  Registry transactions compare serialized state and skip unchanged writes;
  merely validating a connection must not churn `connections.json`.
- **Report structure belongs in Markdown.** Conclusion, evidence, checks,
  contradictions, limits and next action are sections inside `report.markdown`,
  not invented report fields. The advertised schema recommends the owned-draft
  route up front for code blocks or reports larger than about 3 KB. This reduces
  fragile inline transport; it cannot repair engine-side malformed JSON.
- **Bare model IDs require uniqueness, not guesswork.** Recorded user requests
  can name an exact bare ID only when it is unique across enabled configured
  engines. One terminal prose mark is supported unless the longer token itself
  is a configured ID. Shared bare IDs grant neither route; plan previews explain
  the missing-engine ambiguity. Exact enabled identities now also accept direct
  means prepositions and a request verb anywhere in the same user message;
  labels keep local grammar, and negations/hard routing checks remain. See [routing identity rules](../features/routing-and-fallback/index.md#settings-only-routing).
- **A missing write parent is created, not refused.** Production session
  e37b655d (2026-10-07) refused `project_write docs/project/index.md` twice with
  an opaque `gateway_io_failure` (field `operation`) because `docs/project` did
  not exist; the worker only succeeded after guessing `project_mkdir`. Creating a
  file (`expected_sha256: "absent"`) and `project_mkdir` now create missing
  parents no-follow, after the private-path/grant/deny-write checks. Remaining
  refusals are precise: `gateway_parent_not_directory` (a file or symlink in the
  chain), `gateway_write_conflict` (a hash for a file under a missing parent),
  `gateway_path_exists` (mkdir onto a file or link). Regression:
  `tests/test_project_docs_write_access.py`.

## MCP error classes from session 471edd44 (2026-10-07)

- **Missing-path probes are observations, not failed actions.** A permitted missing
  path now returns `exists: false`, `sha256: "absent"` and `parent_exists` from
  `project_read`. Pass that SHA value directly to `project_write`, which creates
  missing parents itself. Private/escaping paths, links and real OS read
  failures remain refused. Do not turn a denied path into a creation attempt.
- **Keyless project mutations have stable identities.** `project_write`,
  `project_mkdir` and `project_delete` accept an omitted `request_key`; the runtime
  hashes operation plus payload, scoped to the channel, and identical retries
  reuse the retained effect receipt before CAS or deletion confirmation. Pre-effect
  refusals retain only their rejection event, allowing corrected keyless retry. A different
  payload is a new action. To intentionally repeat the same payload after an
  intervening change, use a fresh explicit key. Unknown effects still block replay.
- **Honest failed-check history must not silently end the worker.** An open failure
  now refuses `result_submit` with `open_failed_check` before publishing a report,
  recording a request/result or revoking the channel. The correction names the
  check/receipt and its observed argv when available. Keep the failed check and
  cite an exact later passing rerun, or intentionally submit
  `assignment_outcome: "failed"`. Correcting the check command itself changes its
  identity and does not close the original failure. A mistyped command line whose
  own output shows the check never started (usage, unknown-option or
  file-not-found error) is not a failed check, though: cite the corrected
  command's receipt and describe the mistake in the report. A Codex worker on
  2026-10-07 instead cited `generate_agent_profiles.py --check` (exit 2) and failed
  an otherwise verified assignment. Accepted receipts include
  `node_status`; this is not task acceptance. No `supersedes` policy was introduced.
- **Situational skills are still gated at submission.** Successful gateway writes
  and mkdir calls give early `required_before_result_submit` skill/reason hints
  from this attempt's own gateway writes as non-error reminders; only attributable paths
  in this node's declared scope or successful channel writes can refuse a result.
  Reading the skill clears later hints;
  native edits still rely on the result gate. Missing manifests remain explicitly
  unobservable, and advisory observation failure cannot reject a committed write.
- **Keep long free text in top-level string arguments.** Claude engines emit a
  top-level string argument verbatim but hand-write JSON for object and array
  values. Sonnet 5 broke the escaping of Markdown nested in the former
  `result_submit` `report: {title, markdown}` (raw newlines, invalid escapes) in
  31 of about 121 inline reports of 3 KB or more (2026-10-04..07), and Claude Code
  refused the call before Cortex received it, while 532 top-level
  `project_write`/`draft_write` strings up to 150 KB never failed. Description-only
  advice to prefer the draft route did not stop it. `report` is now one plain
  string; nested result strings stay at 4096 characters or fewer. The retained
  inputs are truncated at 2 KB, so the exact malformed bytes are mostly unknown,
  and no paid live run has yet confirmed the new form.

See [file boundaries](../features/permissions-and-containment/index.md),
[publications](../features/publications-and-consistency/index.md) and
[check history](../features/orchestration/index.md#check-history-and-current-outcome).

Quota window labels are exact evidence, not model-family names. An unmapped exhausted window conservatively blocks all models until an explicit association distinguishes it. A scoped `models: []` is an intentional exclusion, not unknown usage. Older profiles without mappings remain untouched and use conservative semantics when activated; never fill them silently from current settings. See [quota fields and migration](../configuration.md#exact-quota-window-associations).

Owner model preferences are rank inputs, not route grants. They only decide
among routes already eligible for the role's tier; optional OpenCode models
need explicit automatic-selection opt-in, and pins, deny rules, provider
limits, availability and quota remain in force. Routing uses the first
existing project, `$CORTEX_ROUTING`, or user file rather than merging files,
so project/environment routing can shadow an active user profile. A
coordinator hint or retry diversification should no longer outrank an eligible
owner `preferred_for` model, but tier/opt-in/shadowing differences are not
profile activation failures. The five original-source regression failures in
the routing repair report were the expected pre-fix reproduction. See
[configuration routing](../configuration.md#routing-rules) and
[the routing decision](decisions.md#2026-09-30-owner-profiles-snapshot-full-user-settings-routing-preferences-rank-within-eligibility).

A JSON-Schema conditional is not a requirement models follow. `result_submit`
once advertised `report` and `report_ref` as two optional fields under a root
`if/then/else`; reviewers repeatedly sent exactly the plain `required` fields
(summary, checks, request_key, verdict) and were refused, and a description-only
warning did not stop it. Every field a call needs belongs in the plain
`required` list; keep alternatives inside one required field (`report` is one
string: inline Markdown or an exact report id), and never use a root `oneOf`, which
Codex code mode renders instead of the properties. See
[publications](../features/publications-and-consistency/index.md).

An ACP client session and an executor attempt are different identities. On
resume or replacement, select the adapter from the actual attempt and engine;
do not infer it from the coordinator.

An acknowledged tool call is not proof that a command ran or that a criterion
passed. Preserve observed output, exit status, truncation and provenance before
the coordinator evaluates meaning.

Never interpret an unknown command outcome, missing publication or unknown
engine liveness as success. Retain receipts and reservations until the exact
outcome is observed; revocation alone does not prove process exit.

Write every Cortex MCP stdio reply inside the per-request error handling, as
strict UTF-8. One unencodable reply (a lone surrogate from a non-UTF-8 file
name) used to end the whole server, and the engine then answered `Transport
closed` to every later Cortex call of that session. See
[failures and cancellation](../features/failures-and-cancellation/index.md).

Instruction compression may replace large evidence bodies with bounded links,
but it must keep complete mandatory rules and record packet hashes. Executor
reports are data, not user instructions or new system rules.

Live Zed qualification requires an isolated disposable environment. The
offline suite cannot establish real-client behavior or paid engine behavior;
those checks remain explicitly unverified until run.

Installing a newer codex-acp package does not make Cortex select it until
its exact version and bundle patch are supported. The current supported
newest version is 2.0.1 (Codex CLI 0.159.1, including `gpt-6.1-sol` in its
offline catalog). New sessions discover it; an existing session keeps its
already running engine. claude-agent-acp 0.84.0 is already installed and
selected. See [Codex compatibility](../features/codex-engine/index.md#one-time-adapter-update-2026-09-30).

Codex code mode does not show a model the MCP JSON Schema: it renders a
TypeScript declaration from a reduced schema. Bounds, `pattern`, `not` and
`if`/`then`/`else` vanish; a root or item-level `oneOf`/`anyOf`/`allOf` replaces
the object's fields; and a schema over `tool_input_schema_max_bytes` (5,000 by
default) loses every description. Put a constraint in the property
description, express presence rules as `if`/`then`/`else`, and run
`tests/test_codex_code_mode_visibility.py` after changing an advertised
schema. See [MCP bridge](../features/codex-engine/index.md#mcp-bridge).

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

The Paseo phone's client bundle is evaluated from source by Hermes, so a
desktop build or type-check does not establish phone compatibility. In the
observed React Native 0.81 Hermes path, ES `class` declarations became
`undefined`: both the plugin error boundary and shared poller failed. They
were replaced with Hermes-compatible implementations and guarded by the
`test:client-bundle` regression test. The fix passed off-device Hermes checks;
a real phone remains the final confirmation.


A crash exactly at that one call site (`acp.orchestrating_runtime
._open_coordinator_session`'s own `self._engine.open(...)`) leaves a task id
already durably bound (`bootstrap.bootstrap_task`'s `tasks` row and
`_ensure_coordinator`'s own `task_orchestration` row are both already
committed) but no `runtime_tasks` row yet -- `orchestration.state.TaskMachine
.create` only runs afterward, from `orchestration.controller.run`, itself
only reached once `_ensure_coordinator` returns successfully. That call site
now tags (never replaces) the exception with `cortex_launch_stage=
"engine_launch"` plus the task/attempt id; `acp.agent`'s except block reads
those (only present for this one failure shape) into
`_write_crash_diagnostic`'s log header alongside the *running process's own*
package version (`_agent_version()` -- not necessarily this checkout's, see
`scripts/cortex_diagnose.py`'s separate installed-vs-checkout
`package_version` check), and into the `-32603` `message` text itself
("...(stage: engine_launch)."), since a real host drops `data` (above). A
retry recovers through the pre-existing coordinator-replacement path
(`recovery.coordinator_recovery.recover`) without duplicating the task:
`task_orchestration`/`runtime_tasks` are both `task_id PRIMARY KEY`, and
`TaskMachine.create`/`_ensure_coordinator` both check for an existing row
before inserting one -- see
`tests/test_engine_launch_failure_recovery.py`. The same failure shape also
used to permanently skip publishing the initial pipeline of a task on every
retry (`_run`'s publish call was gated on the `task_orchestration` row *not*
existing yet, but that row is committed before the engine session ever
opens); it is now called unconditionally, relying on
`pipeline_bootstrap.publish_initial_pipeline`'s own idempotent `reports`-row
check instead.

Codex can present MCP tool schemas as TypeScript declarations that omit JSON
Schema constraints such as `minItems`, `maxItems`, and `pattern`, along with
array item descriptions. Put array meaning, bounds and accepted forms in the
field's own description; keep runtime validation authoritative and report all
argument violations together so one correction can address them. For
`plan_submit`, both `plan.tasks` and `plan.verification` are required (1–16
items). Verification applies even to investigation-only and trivial plans. If
project documentation has no verification command, the schema directs the model
to propose the smallest literal-argv check that would fail when the requested
result is wrong, using project files and standard tools. The Codex failure
investigation found that the schema was visible; the actual gap was conflicting
instructions that made an empty list appear to be the only compliant answer.
The current contract puts the fallback at the start of each required array's
description and repeats it in validation corrections.

A recovery capsule is not always verbatim. When its critical sections exceed
`CRITICAL_BUDGET_BYTES`, fields with `status: "referenced"` carry only a
preview. Read the full text through the named `retrieve` tool, and verify it
against `sha256`, before relying on anything past the preview. Never treat a
preview as the complete requirement. `<key>_omitted` records list entries that
were left out of a long list.

A full connection is not a failure. Seven parallel workers on a four-slot
connection once lost three of them to `capacity_timeout` →
`fallback_refused:no_untried_authorized_route` after 600 s, while the four
holders were live and still working. A queued open now keeps waiting while
every slot is held by a live session. A capacity-class fallback refusal is also
retried within `MAX_ATTEMPT_RETRIES`. A chat session can run at most four
workers across engines and connections; a connection's
`max_concurrent_sessions` can only lower that session's share. Excess ready
nodes remain pending, and queued worker opens wait for capacity. Plan size does
not raise the session cap.

A read-only role (ux-designer, architect, …) can never write its output file,
whatever scope the plan gives it: its `command_run` jail binds nothing
writable, and `project_write` refuses. `plan_submit` refuses a read-only
task whose scope names a path that does not exist yet
(`read_only_scope_not_writable`). An existing directory in its scope does not
make it writable either. Have the task return content in its report and give
the file to a writing role in a dependent task.

A review binds to an exact project digest. If another writer (the owner or
another agent session) edits the same checkout during review, the verdict is
refused and the review is re-bound to the new digest, at most
`MAX_REVIEW_REBINDS` times. Avoid editing a checkout while its task is in
review.

A prompt rule is not a guarantee. The coordinator was told not to recap after
`user_question_request`, and it still rewrote its full details after the
question in some turns. The runtime now withholds coordinator text after the
committed question (`WAITING_USER`) for the rest of that turn. A delayed first
lifecycle callback is not proof that preceding text was written before the
question; neither new nor repeated tool calls release suppressed text. This
conservative boundary can hide pre-question details delivered after the commit.
Final-only text is withheld too when the question was observed or is pending.
Text received before the commit and text after the answer in a later turn remain
visible.

A Paseo "cancel" (`session/cancel`) pauses the whole task and cancels every
running and queued worker (`turn_interrupted`). That is by design, not a
worker failure. A cancelled queued worker is reported only as cancelled.

`opencode models` and a Cortex OpenCode catalog disagreed for two separate
reasons (observed 2026-09-30 on OpenCode 2.0.20). First, a custom provider
exists only in the owner's `~/.config/opencode`, which the engine jail hides;
Cortex carries its provider definitions over
(`engine.opencode_adapter_acp.owner_provider_config`). Second, `opencode acp`
fixes each directory's `session/new` catalog as soon as any model and primary
agent exist, and a login in the database it opens makes models exist before
the supplied configuration (agents and providers) is applied, so it offered
`build`/`plan` and only logged-in providers. It is a race, not ignored
configuration: waiting does not help, because the catalog is memoized per
directory. Selecting `cortex` after a delay, another `session/new` or
`session/load` does not refresh that catalog. Merging the owner's config with
Cortex's and delivering it through every config channel did not help either:
all of them are applied by the same late configuration plugins. The fix removes
the built-in `opencode.agent`/`opencode.plan` plugins through Cortex's own
`plugin` list, which the server resolves before starting any plugin, so no
`build` agent exists and the catalog waits for `cortex`. The alternate-database
and extracted-login workaround was rejected and removed: OpenCode opens its
native `opencode.db` in place, with no copied database credentials. The catalog
guard (`opencode_cortex_mode_not_advertised`) still rejects a catalog without
`cortex`. OpenCode 2.x keeps logins in `opencode.db` (`credential` table), not
`auth.json`. See
[the OpenCode adapter](../features/opencode-engine/index.md#evidence-and-limits).

Model identities and policy are not inferred from names, versions, family labels or catalog order. A valid version-2 owner routing document must explicitly place every automatic identity; missing policy is a visible error. The packaged production-derived template is installation-only and never a runtime fallback. Existing owner/profile state is not overwritten; migration requires preview, explicit consent and digest-bound apply. Portable coordinator binding can be ambiguous across connections and must remain an error until the owner resolves it. See [settings-only model policy](../configuration.md#settings-only-model-policy).

Deleting a selected saved profile clears only its linkage; it does not reset materialized settings. The last profile may be deleted, and stale/error results must not be retried invisibly.


Paseo's native pre-delivery queue is not Cortex's captured steering ledger.
With the inspected 0.10.2 host, keep default `steer`, use Ctrl/Cmd+Enter, native
Edit, and Edit then clear text/attachments to cancel. Edit overwrites an existing
draft, enqueue trims surrounding whitespace, exact attachment filtering remains
unverified, and reload persistence is only inferred. `queue` send behavior
can turn Send now/question-card input into explicit Stop-like interruption.
Normal native drain during a Cortex background continuation now waits for
session ownership rather than cancelling the controller. No Cortex Withdraw or
separate editor was added; CLI cannot prove the rendered native interaction.
See [the native queue contract](../features/steering-and-questions/index.md#native-composer-queue-versus-captured-steering).

The compaction guard's recovery ratio needs a real pre-compaction size. The
capsule size is part of the mandatory post-compaction content, so using it as
"before" made every ratio 0 and blocked the third compaction of any session
with `consecutive_low_recovery`. Without an engine-reported `usage_update.used`
from before the compaction, the attempt is `unmeasured` and only the frequency
and immediate-refill checks apply. A task already blocked this way stays
blocked; the fix applies to later compactions. See
[the compaction-loop guard](../features/summaries-and-budgets/index.md#5-compaction-loop-guard).

A long-lived process started from a Cortex sandboxed command outlives the
command but not its scratch. The dev Paseo daemon started through
`command_run` kept the sandbox mount namespace, whose `/var/tmp` was the
command's deleted scratch subdirectory; every engine jail it launched then
failed in bubblewrap before ACP `initialize`, for Codex and Claude alike, and
the agent showed only `engine_launch`. Read the engine's own `stderr.log`
before blaming an engine version, and start daemons with `outside_command`. See
[the dev Paseo environment](dev-environment.md).


## Paid desktop rerun runtime findings (2026-10-07)

- **Auto-key refusals are not effect receipts.** Before any possible side effect,
  a keyless precondition refusal keeps `TOOL_REJECTED`, but removes its
  replayable derived-key request row. Creating the missing parent allows the
  identical keyless write to succeed. Explicit keys retain complete structured
  errors; actual/pending effects still fence identical retries.
- **Shared deltas are not worker authorship.** The skill gate excludes sibling
  writers' reserved paths unless this attempt has a successful gateway write
  for them. Unattributed or overlapping native work may yield only an advisory
  hint. Read-only/review scopes are not writers; prior plan revisions do not
  claim this revision's paths.
- **Git observations are context, not checks.** `project_git` explicitly returns
  `evidence: "context only, not a check receipt"`; its `large_output_id` is never
  a command receipt. Put inspected status/diff in the report body. Meaningful
  Git checks must use `command_run` with an exit-status assertion. F3 (2026-10-07):
  `project_git` now inlines `output` up to 64 KiB (the same bound `command_run`
  uses), with `output_truncated` marking when the retained bytes go further;
  only an oversize result needs a `read_large_output` follow-up at all.
- **Honor exact owner-named models.** `Часть 2 на gpt-6-luna:` is a request when
  that exact enabled ID is bare-unique; qualified IDs also work. A request-form
  verb in the same user message governs every exact identity, not labels.
  Negation, exact case/boundaries, source provenance and hard routing checks
  remain. See [routing](../features/routing-and-fallback/index.md#settings-only-routing).
- **Scope is a write reservation.** Do not list read-only dependencies in a
  writing node's scope: they unnecessarily serialize parallel writers.
- **Worker capsule routes have separate evidence.** `task_configuration` is the
  task/coordinator snapshot. `own_worker_route` contains only this attempt's
  dispatch audit and engine-confirmed event/reference; missing confirmation
  stays unavailable. See [capsule shape](../features/recovery-capsules/index.md#nine-sections).

These are implementation contracts, not proof of zero errors in a later paid
desktop workload. Qualification still requires real retained run evidence.
