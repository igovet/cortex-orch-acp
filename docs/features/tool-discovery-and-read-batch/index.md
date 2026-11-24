# Tool discovery and read batches

Source: the advertised tool schemas in `crates/cortex-engine/src/schema.rs`, the brokered gateway in `crates/cortex-engine/src/gateway.rs`, and the native handlers in `crates/workspace-service/src/native_documents.rs` (discovery) and `native_batch.rs` with `CorePolicy::batch` in `native_sessions.rs` (batches).
Focused coverage: the `cortex-engine` gateway tests, the `workspace-service` `native_batch` unit test and the `native_sessions` gateway scenario `recovery_project_operations_and_structured_tool_errors`.

## Additive, not an evaluator

`tools_discover` returns a bounded names-first catalogue or one exact effective
Cortex `tools/list` record, including current role/stage narrowing and engine
metadata. Unknown and unoffered names have the same value-free error. Discovery
never inspects external MCP servers or grants permission to call a tool.
For external tools, consult the engine's session tool list or search if offered.
Use the name, description and schema advertised there, not the Cortex namespace
or a tool's display title. An absent code-index tool calls for the applicable
project-read fallback, not a guessed alias.
Existing individual tools, all 23 profiles and ordinary engine loading remain.
This is not a Cortex-owned Code Mode sandbox or a two-tool replacement for MCP.

A direct gateway call with an unexpected property in a closed argument object
returns `-32602` (`invalid_arguments`), with reason `unexpected_property`
and the offending path in `error.data.field`, for example `arguments.artifacts`
or `arguments.checks[0].closed_by`. The unknown key segment is bounded and
quoted when needed; its value is not echoed.

`read_batch` is offered to every role whose catalogue lists it (every
coordinator, worker and consultant catalogue), and discovery entries mark the
batchable reads with `read_batch_supported: true`.

`read_batch` groups explicitly selected independent read-only reads:
`project_read` (files and directory listings), `project_git` (status and diff),
`read_skill`, `list_reports`, `read_report`, `read_draft`, `list_sources`,
`read_source`, `read_large_output`, `read_capsule`, `tools_discover`,
`command_status` (workers) and `task_get_context` and `list_route_options`
(coordinator), and only those the calling channel is itself offered. Each
catalogue advertises its own item enum (the consultant's lists no project, Git or
command tool). The advertised item schema refuses any other tool name (`-32602`
`invalid_arguments` at `arguments.calls[i].tool`, reason `invalid_enum`), and the
runtime refuses the whole batch before any member executes, as a tool error
naming the exact location, when the request exceeds its byte or depth bound
(`request_too_large`, `invalid_arguments` at `calls`), an id repeats
(`calls[i].id`), a member is not offered to the channel (`tool_not_available` at
`calls[i].tool`) or a member's arguments do not match that tool's own schema
(`calls[i].arguments`). Each member then runs, one at a time and in order,
through exactly the handler a direct call of that tool uses, so it has the same
guards, path rules, paging and size bounds. An inline `project_read` adds channel
read coverage just like a direct read; `project_git` retains its snapshot under
its `request_key`. A refusal of a member, such as a private path, is that item's
error in the standard shape.
Arguments are declarative JSON, not expressions or references to prior results.
Writes, command launches, questions, submissions, access/scope/escalation
requests, availability probes (`check_preflight`), external calls, publishing,
lifecycle actions and recursive batching are not batchable.

Images returned by `read_source` or `project_read` require an individual MCP call; a batch returns
an explicit `batch_multimodal_unsupported` item error instead of flattening an
image block into apparent text evidence. A rejected image adds no project
read coverage. Text/base64 source metadata is preserved.

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
  65,536 bytes for the structured batch envelope (above it the batch is the
  `result_too_large` tool error). Larger item results are retained whole as
  owner-private output and the item is `deferred` with its `large_output_id`,
  `byte_count` and `result_sha256`, readable with `read_large_output`. Nothing is
  silently cut to fit. An oversized `read_large_output` page returns `batch_result_limit`
  instead of recursively spilling; request a smaller original page.
- Five seconds is a monotonic admission budget, not an interruptible read timeout.

Execution is sequential and **not a snapshot or transaction**. Each native read
retains its task/owner binding, authorization and cursor semantics. A native
`next_cursor` or `next_offset`, failure, encoding or truncation flag remains
inside that item's result unchanged. Caller-selected small pages avoid waste.

Every member retains its id/tool and `ok`, `error`, `deferred` or `not_run`
status; an `error` item carries the member's structured error (`code`, `field`
when known, `hint`). The envelope reports executed, returned and deferred counts
and complete/incomplete. A `deferred` item ran and is retained under its
`large_output_id`: a normal outcome that keeps the batch complete and the MCP
response not an error, so a real report above the inline bound is read through
`read_large_output` without the batch looking failed. Only an `error` or
`not_run` item makes the batch incomplete and the outer MCP response an error
(`isError: true` with the full envelope); successful siblings are
still usable evidence, never proof of the failed item. Complete describes batch
delivery, not complete document coverage or successful underlying command output.
A deferred `project_read` adds no coverage and omits the coverage fields from
the retained page, which also carries `coverage_recorded: false`. A batched read of a binary file
(returned by identity only) records hash-bound coverage like a direct one. Fetching that page with `read_large_output` does not change
coverage. For a write precondition or review coverage, use `project_read`
directly or request smaller inline pages.

## Authority, cancellation and evidence

These two tools require current channel/owner authority. The channel's exact
identity (task, attempt, owner epoch, role, profile, stage) is rechecked before
and after every member; a channel that is no longer current aborts the batch as
a named `-32001` refusal and withholds all accumulated bodies and references. An
authority refusal of a member does the same. A member's ordinary failure is that
item's error only.

Cancellation is cooperative through observed authority changes and
engine/process teardown, not preemption. Disconnect/crash can leave executed
reads or retained outputs without a delivered response. Retry means a fresh read,
not replay or batch resumption.

Read-only describes the requested operation: deferred items write retained
output. The native runtime keeps no per-read delivery observation for reports,
for a batch or a direct call, and emits no batch audit events. Display history
keeps metadata/status rather than returned discovery/batch bodies.

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

The former benchmark script (`cortex_read_api_benchmark`) was removed. A serial-only client can replace three
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

The no-paid real-stdio fixture, the live assertion CLI and the outcome-assertion script (`cortex_read_api_fixture`, `cortex_read_api_live_assert`, `cortex_qualification_assert`) were Python tools; they were removed on 2026-11-16 without replacement. The matrix below can still be exercised by hand in an owner-requested live CLI case, but nothing shipped asserts its result, so each row is read from the retained evidence and a row that cannot be read is reported as unverified, never as passed.

For a live CLI case, use one writing worker in a new disposable project. First make the public expected report file as `required_first_line + "\n\n" + "# Public fixture\n" + "λ" * 3600 + "\n"` **before** publishing/reading it. Preserve it as `public-read-api-report.md` directly in the disposable project, outside private storage. Use the advertised tool schemas and the following small matrix; every call needs a unique actual MCP request identity:

| Assertion | Calls / independent expected value |
| --- | --- |
| Exact schema and catalogue | Discover `read_report` by name; catalogue with limit 50. Expected effective records derive from the stamped checkout and the exact worker role/stage. |
| Malformed discovery | Name `read_report` plus limit 1. Expect the exact incompatible-limit field/expected/correction, not merely an error flag. |
| Native and successful batch | Through `command_run`, run `printf 'Cortex public fixture v1: λ\n'`. Retain its real receipt/output ID. Read that ID individually with limit 1000 and in one batch item `public`. Expected literal UTF-8 bytes are fixed independently. |
| Mixed / rejected | Add item `missing`, reading `lo_000000000000` with limit 100; expect a native item error beside the unchanged public result. Separately add item `forbidden` with tool `command_run` and empty arguments; expect schema rejection and zero member executions. |
| Deferred and retrieved | Read the public report individually with limit 4000; then as batch item `deferred`. Compare the native canonical hash/bytes and public file content. Retrieve the actual deferred ID individually with limit 16000; compare the exact canonical JSON bytes and require no batch report-delivery observation. |
| Real check / mandatory review | Make a tiny real project edit (for example change disposable `qualification.txt` from `pending\n` to `qualified\n` after reading it). Run an exact byte assertion through `command_run` and attach that same attempt's returned receipt to its passed check. The runtime, not execution QA, supplies the independent review. |

Expected schemas/catalogue come from the selected worker role/stage, never the first matching reply. Authenticated coordinator/reviewer/other-attempt reads in the same snapshot are other-actor observations, not worker evidence. Fresh repeated idempotent worker reads are allowed, not deduplicated: every selected occurrence must independently satisfy exact binding and expected semantics. Duplicate observations, reused request correlation, foreign/unbound identities or one wrong/error/withheld/truncated/incomplete occurrence block the matrix even beside a correct reply. Missing required shapes also fail. Checkout schema parity is not live process build attestation; establish current-build adoption separately before the case.

A missing client-dispatched malformed call, unsupported native cancellation interleaving, absent binding, failed/ambiguous/replayed/truncated/pending evidence or an unexpected catalogue/page size is a concrete blocker. Do not relabel it passed or spend repeated paid calls trying to manufacture the signal. This matrix does not qualify GUI rendering, costs, latency or unobservable host actions.

Codex and Claude use the same model-facing `tools_discover` call to find Cortex tools and inspect an unfamiliar input schema. The engine's MCP client still obtains its initial registrations through the protocol handshake. An invalid argument response names the schema field and the failed constraint. For an atomic edit that fails to match, the field identifies the replacement in the ordered array.

A supervised `command_run` that continues beyond five seconds after its launch handshake returns its reference and a pending state. The original channel can use `command_status` while continuing other work. An identical request key refers to the same command. Only the settled record supplies the final exit or failure.
