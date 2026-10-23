# Tool discovery and read batches

Source: [tool_read_api.py](../../../src/cortex_runtime/tool_read_api.py),
[advertised schemas](../../../src/cortex_runtime/delegation/contracts.py),
[server dispatch](../../../src/cortex_runtime/server.py).
Focused coverage: `tests/test_tool_read_api.py`.

## Additive, not an evaluator

`tools_discover` returns a bounded names-first catalogue or one exact effective
Cortex `tools/list` record, including current role/stage narrowing and engine
metadata. Unknown and unoffered names have the same value-free error. Discovery
never inspects external MCP servers or grants permission to call a tool.
Existing individual tools, all 23 profiles and ordinary engine loading remain.
This is not a Cortex-owned Code Mode sandbox or a two-tool replacement for MCP.

`read_batch` groups explicitly selected independent evidence pages:
`list_reports`, `read_report`, `read_draft`, `list_sources`, `read_source`
and `read_large_output`. All envelope fields, unique identities, offered names
and current native argument schemas are checked before any member executes.
Arguments are declarative JSON, not expressions or references to prior results.
Commands, external calls, publishing, lifecycle actions, project/skill reads,
recursive discovery/batching and mutations are prohibited.

Images returned by `read_source` require its individual MCP call; a batch returns
an explicit `batch_multimodal_unsupported` item error instead of flattening an
image block into apparent text evidence. Text/base64 source metadata is preserved.

## Bounds and delivery

Argument contracts and units live in the advertised schemas. Current limits:

- Discovery previews: at most 50 entries and 16,384 compact UTF-8 bytes per page;
  descriptions preview at most 160 Unicode characters with an explicit flag.
  Cursors bind channel/owner and catalogue digest; changed offers invalidate them.
- Exact schemas: at most 32,768 compact UTF-8 bytes inline. Larger complete
  records defer through existing owner-private retained output; above 1 MiB fails.
- Batch: 1–8 members, unique case-sensitive item identities; at most 32,768
  compact UTF-8 request bytes and 16 container levels.
- Native data objects: at most 6,144 compact UTF-8 bytes inline per item and
  65,536 bytes for the structured batch envelope. Larger results defer as exact
  JSON up to 1 MiB, with retained id, byte count and SHA-256. Nothing is silently
  cut to fit. An oversized `read_large_output` page returns `batch_result_limit`
  instead of recursively spilling; request a smaller original page.
- Five seconds is a monotonic admission budget, not an interruptible read timeout.

Execution is sequential and **not a snapshot or transaction**. Each native read
retains its task/owner binding, authorization and cursor semantics. A native
`next_cursor` or `next_offset`, failure, encoding or truncation flag remains
inside that item's result unchanged. Caller-selected small pages avoid waste.

Every member retains its id/tool and `ok`, `error`, `deferred` or `not_run`
status. The envelope reports executed/returned counts and complete/incomplete.
Any non-ok item makes the outer MCP response an error; successful siblings are
still usable evidence, never proof of the failed item. Complete describes batch
delivery, not complete document coverage or successful underlying command output.

## Authority, cancellation and evidence

These two tools require current channel/owner authority and an active runtime
task; a worker channel additionally needs a resolved running assignment with a
role and stage (`tool_read_api.effective_catalogue`), while a coordinator channel
needs none. Authority and effective catalogue are
rechecked between reads and before delivery, even when the server's original
channel context is cached. Loss of authority or changed role/stage offers aborts
the wrapper and withholds all accumulated bodies and references. Existing
individual historical reads retain their original behavior.

The synchronous stdio transport cannot consume a cancellation notification while
an inner read runs. Cancellation is cooperative through observed authority changes
and engine/process teardown, not preemption. Disconnect/crash can leave executed
reads or retained outputs without a delivered response. Retry means a fresh read,
not replay or batch resumption.

Read-only describes the requested operation: normal runtime bookkeeping, audit
events and retained-output writes still occur. Inline report pages get a
runtime-derived, typed outer-request/item observation at final delivery.
Deferred/error/withheld pages do not establish a successful report handoff read.
Reading a deferred JSON blob alone does not establish the original report read.
Batch observations are not extra native MCP requests or command check receipts.
Display history keeps metadata/status rather than returned discovery/batch bodies.

## Examples and measurement

First recover the current schema, then submit only independent, already-known
references. The names below are literal fixtures, not actual retained identities:

```json
{"name":"read_report"}
```

```json
{"calls":[{"id":"report","tool":"read_report","arguments":{"report_id":"<report-id-from-list_reports>","limit":1000}},{"id":"sources","tool":"list_sources","arguments":{"limit":1}}]}
```

The first object is for `tools_discover`, the second for `read_batch`.
Discovering references and reading them are dependent steps: do not invent a
substitution language inside one batch.

Run `python3 -B scripts/cortex_read_api_benchmark.py` for deterministic current
catalogue sizes and fixture request bytes. A serial-only client can replace three
independent read round trips with one batch, or two calls with initial schema
discovery. Native Codex exec already combines independent calls in one model
turn. The ordinary bootstrap catalogue is unchanged and two new tools add
startup payload. JSON bytes and illustrative round trips are **not measured
live token, latency or cost reductions**.

## Live CLI qualification

Only after an owner-requested CLI check, follow the [isolated dev guide](../../project/dev-environment.md).
Use a fresh disposable project, exact stamped package identity and fresh
host/ACP/task/owner/assignment/attempt/session bindings. Verify the installed
Paseo help and bridge before launch; no production plugin or account changes.

Exercise exact discovery versus individual tools/list; allowed read parity and
native cursors; forbidden command/write/external/recursive preflight with zero
members; mixed missing-reference errors; deferred bytes/hash and owner isolation;
and observed authority loss/admission-budget behavior. A race passes only when
bound events establish its interleaving. Retain incomplete/unobserved cases as
unverified. Inspect complete paged metadata/events and native configuration
confirmation, not an opaque host id alone. CLI covers protocol/runtime behavior,
not rendered GUI behavior. Independent review and final verification remain
runtime-owned.

## Executable public evidence

First run the no-paid real-stdio fixture (new disposable store, no engine):

```bash
python3 -B scripts/cortex_read_api_fixture.py
```

It asserts named/schema and catalogue parity against tools/list, the exact
name-plus-limit diagnostic, literal UTF-8 output, native/successful/mixed/rejected
reads and exact deferred canonical JSON retrieval. Its independent expectations
come from public literal bytes, current tools/list and an individually checked
public report, not copied observation rows. It checks the immutable fingerprints
and runs the assertion CLI before deleting its owned fixture. Its output explicitly
says no paid dispatch and no acceptance. Worker-specific tests ensure that deferred
retrieval does not create report-delivery evidence.

For an owner-requested live CLI case, use one writing worker in a new disposable
project. First make the public expected report file as `required_first_line + "\n\n" +
"# Public fixture\n" + "λ" * 3600 + "\n"` **before** publishing/reading it.
Preserve it as `public-read-api-report.md` directly in the disposable project,
outside private storage. The reader requires the literal fixture structure and
its published edition digest/size, not an arbitrary observed response dump.
Use the advertised tool schemas and
the following small matrix; every call needs a unique actual MCP request identity:

| Assertion | Calls / independent expected value |
| --- | --- |
| Exact schema and catalogue | Discover `read_report` by name; catalogue with limit 50. Expected effective records derive from the stamped checkout and the exact worker role/stage. |
| Malformed discovery | Name `read_report` plus limit 1. Expect the exact incompatible-limit field/expected/correction, not merely an error flag. |
| Native and successful batch | Through `command_run`, run `python3 -B -c 'import sys;sys.stdout.write("Cortex public fixture v1: λ\n")'`. Retain its real receipt/output ID. Read that ID individually with limit 1000 and in one batch item `public`. Expected literal UTF-8 bytes are fixed independently. |
| Mixed / rejected | Add item `missing`, reading `lo_000000000000` with limit 100; expect a native item error beside the unchanged public result. Separately add item `forbidden` with tool `command_run` and empty arguments; expect schema rejection and zero member executions. |
| Deferred and retrieved | Read the public report individually with limit 4000; then as batch item `deferred`. Compare the native canonical hash/bytes and public file content. Retrieve the actual deferred ID individually with limit 16000; compare the exact canonical JSON bytes and require no batch report-delivery observation. |
| Real check / mandatory review | Make a tiny real project edit (for example change disposable `qualification.txt` from `pending\n` to `qualified\n` after reading it). Run an exact byte assertion through `command_run` and attach that same attempt's returned receipt to its passed check. The runtime, not execution QA, supplies the independent review. |

After dispatch, obtain the exact owned binding through the dev wrapper. Run from
the checkout, using only observed IDs and the public expected file:

```bash
python3 -B scripts/cortex_read_api_live_assert.py --project "$PROJECT" \
  --session "$ACP_SESSION" --task "$TASK" --owner-epoch "$OWNER_EPOCH" \
  --attempt "$ATTEMPT" --command-ref "$PUBLIC_COMMAND_RECEIPT" \
  --report-id "$PUBLIC_REPORT_ID" --public-report "$PUBLIC_EXPECTED_FILE"
python3 -B scripts/cortex_qualification_assert.py --project "$PROJECT" \
  --session "$ACP_SESSION" --task "$TASK" --owner-epoch "$OWNER_EPOCH" \
  --require-review --expect-command-ref "$BYTE_ASSERTION_RECEIPT" \
  --expect-route "$EXPECTED_WORKER_ROUTE" --expect-route "$EXPECTED_REVIEW_ROUTE"
```

The first command authenticates every page of the exact task/session/current-owner
snapshot and its durable channel/attempt bindings, then selects the worker through
its unique assignment/engine-session/role/stage joins. Expected schemas/catalogue
come from that selected role/stage, never the first matching reply. Authenticated
coordinator/reviewer/other-attempt reads in the same snapshot are other-actor
observations, not worker evidence or failures solely for having the same shape.
Fresh repeated idempotent worker reads are allowed, not deduplicated: every
selected occurrence must independently satisfy exact binding and expected semantics.
The JSON result includes safe worker occurrence counts, observation IDs and
request/channel hashes, plus other-actor counts. Duplicate observations,
reused request correlation, foreign/unbound identities or one wrong/error/withheld/
truncated/incomplete selected occurrence block the matrix even beside a correct reply.
Missing required shapes also fail. Unavailable observations whose shape the
allowlist cannot expose fail closed, rather than being assumed unrelated.
Checkout schema parity is not live process
build attestation; establish current-build adoption separately before the case.
The second checks lifecycle,
actual review and native route/command evidence. Neither command grants acceptance.
A supplied expectations file to `--read-api-expectations` is another strict
fingerprint comparison route; its origin must still be established independently.

A missing client-dispatched malformed call, unsupported native cancellation
interleaving, absent binding, failed/ambiguous/replayed/truncated/pending evidence
or an unexpected catalogue/page size is a concrete blocker. Do not relabel it
passed or spend repeated paid calls trying to manufacture the signal. This matrix
does not qualify GUI rendering, costs, latency or unobservable host actions.
