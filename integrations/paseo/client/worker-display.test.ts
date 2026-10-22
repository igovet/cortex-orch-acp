import { strict as assert } from "node:assert";
import {
  contextSummary, resolveDurableWorkerCardStatus, routeSelectionText, resolveWorkerCardStatus, tokensAccessibilityLabel, tokensSummaryLabel,
  workerAssignmentLine, workerCardMetadata, workerDisplayName, workerModelDisplay, workerStatusPresentation,
} from "./worker-display.ts";

assert.equal(workerDisplayName({ role: "explorer", title: "delegate:b2c3d4e5-f6a7-4b8c-9d0e-234567890abc", nodeKey: "worker" }), "explorer");
assert.equal(workerDisplayName({ role: "worker-explorer", title: "Explore the project", nodeKey: "worker" }), "explorer");
assert.equal(workerDisplayName({ title: "delegate:b2c3d4e5-f6a7-4b8c-9d0e-234567890abc", nodeKey: "explorer" }), "explorer");
assert.equal(workerDisplayName({ title: "Explore the project", nodeKey: "worker" }), "Explore the project");
// The Workers popover shows exactly the chat row's name and grey line. A
// route with no engine-observed model evidence (T035) keeps its routing id
// but the display marks it explicitly as an alias, never presenting a bare
// id/alias as though it were the concrete model.
const chat = workerCardMetadata("Worker: debugger · codex · gpt-5.6-sol (alias) · high — Map the trailing-stop lifecycle", "running");
assert.deepEqual(chat, { workerName: "Map the trailing-stop lifecycle", subtitle: "debugger · codex · gpt-5.6-sol (alias) · high · working" });
assert.deepEqual(workerAssignmentLine({
  id: "a", nodeKey: "code_path_audit", role: "worker-debugger", title: "Map the trailing-stop lifecycle",
  route: { provider: "codex", model: "gpt-5.6-sol", effort: "high" }, attempts: [],
} as never, "running"), chat);
assert.equal(workerCardMetadata("Worker: debugger — Audit", "canceled").subtitle, "debugger · cancelled");
assert.deepEqual(workerCardMetadata("Worker: debugger · codex · Luna · medium · Completed with issues — Audit", "completed"), { workerName: "Audit", subtitle: "debugger · codex · Luna · medium · completed with issues" });

// A card rendered with zero live pipeline data (an embedded result, or the
// batched fallback -- no `pipeline.workers` projection either way) recovers
// "completed with issues" and "cancelled" from this same portable title
// suffix; ACP's own status carries neither (no "completed_with_issues"
// value at all, and a cancelled assignment is rendered "failed" -- see
// orchestrating_runtime.py's module docstring near
// `_TOOL_CALL_STATUS_FOR_CANCELLED`).
assert.equal(resolveWorkerCardStatus("Worker: debugger — Audit", "completed"), "completed");
assert.equal(resolveWorkerCardStatus("Worker: debugger · Completed with issues — Audit", "completed"), "completed_with_issues");
assert.equal(resolveWorkerCardStatus("Worker: debugger · Cancelled — Audit", "failed"), "cancelled");
assert.deepEqual(workerCardMetadata("Worker: debugger · Cancelled — Audit", "failed"), { workerName: "Audit", subtitle: "debugger · cancelled" });

// The runtime appends the suffix to the whole title (replay_store.py's
// interrupted correction, orchestrating_runtime.py's cancelled/interrupted
// cards). Live (r_b6091be1cd02) that shape read as a generic "failed".
const runtimeTitle = "Worker: general · claude · Sonnet 5 · medium — Run sleep 40";
assert.equal(resolveWorkerCardStatus(`${runtimeTitle} · Interrupted`, "failed"), "interrupted");
assert.equal(resolveWorkerCardStatus(`${runtimeTitle} · Cancelled`, "failed"), "cancelled");
assert.equal(resolveWorkerCardStatus(`${runtimeTitle} · Completed with issues`, "completed"), "completed_with_issues");
assert.deepEqual(workerCardMetadata(`${runtimeTitle} · Interrupted`, "failed"),
  { workerName: "Run sleep 40", subtitle: "general · claude · Sonnet 5 · medium · interrupted" });
assert.equal(resolveWorkerCardStatus("Worker assignment · Interrupted", "failed"), "interrupted");
// A name that merely contains the word is not a status.
assert.equal(resolveWorkerCardStatus("Worker: general — Explain why builds get Interrupted", "running"), "running");

// After a plugin reload the host keeps the card `running` and sends nothing
// until the next prompt: the polled durable attempt status settles it. A
// durable status still running never overrides the host.
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "running", "interrupted"), "interrupted");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "running", "completed"), "completed");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "running", "running"), "running");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "running", "planned"), "running");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "running", undefined), "running");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "failed", "interrupted"), "interrupted");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "failed", "failed"), "failed");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "completed", "interrupted"), "completed");
assert.equal(resolveDurableWorkerCardStatus(`${runtimeTitle} · Cancelled`, "failed", "interrupted"), "cancelled");
// A cancellation the runtime could only send as ACP `failed` (no title
// marker reached the host) settles from the durable status; a durable
// failure stays a failure, and the host's own turn-end `canceled` guess
// yields to the durable outcome.
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "failed", "cancelled"), "cancelled");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "failed", "canceled"), "canceled");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "canceled", "completed"), "completed");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "canceled", "failed"), "failed");
assert.equal(resolveDurableWorkerCardStatus(runtimeTitle, "completed", "cancelled"), "completed");

// T035: realistic Claude and Codex configOptions shapes -- an alias
// (`opus[1m]`, `haiku`, `default`) with the engine's own resolved name
// shows that name; the same alias with no resolved-name evidence keeps the
// routing id, explicitly marked as an alias; an already-concrete id
// (`claude-sonnet-5`, `gpt-5.6-luna`) shows its own resolved name once one
// is evidenced. The routing id itself (used for pin/deny) is never lost --
// it stays `route.model` in every case.
assert.equal(workerModelDisplay({ model: "opus[1m]", modelName: "Opus 5.5 (1M)" }), "Opus 5.5 (1M)");
assert.equal(workerModelDisplay({ model: "opus[1m]" }), "opus[1m] (alias)");
assert.equal(workerModelDisplay({ model: "opus[1m]", modelName: null }), "opus[1m] (alias)");
assert.equal(workerModelDisplay({ model: "haiku" }), "haiku (alias)");
assert.equal(workerModelDisplay({ model: "default", modelName: "Opus 5.5 (1M)" }), "Opus 5.5 (1M)");
assert.equal(workerModelDisplay({ model: "claude-sonnet-5", modelName: "Sonnet 5" }), "Sonnet 5");
assert.equal(workerModelDisplay({ model: "gpt-5.6-luna", modelName: "Luna" }), "Luna");
assert.equal(workerModelDisplay({ model: "gpt-5.6-luna" }), "gpt-5.6-luna (alias)");
assert.equal(workerModelDisplay(undefined), undefined);
assert.equal(workerModelDisplay(null), undefined);

assert.deepEqual(workerAssignmentLine({
  id: "b", nodeKey: "resolved_model", role: "worker-fullstack-dev", title: "Resolve model names",
  route: { provider: "claude", model: "opus[1m]", modelName: "Opus 5.5 (1M)", effort: "medium" }, attempts: [],
} as never, "completed"), {
  workerName: "Resolve model names",
  subtitle: "fullstack-dev · claude · Opus 5.5 (1M) · medium · completed",
});
console.log("worker display tests passed");

assert.equal(contextSummary(undefined), null);
assert.deepEqual(contextSummary({ used: 84_300, size: 200_000 }), { percent: 42, detail: "84k / 200k", level: "normal" });
assert.deepEqual(contextSummary({ used: 950_000, size: 1_000_000 }), { percent: 95, detail: "950k / 1M", level: "danger" });
assert.equal(contextSummary({ used: 300_000, size: 200_000 })?.percent, 100);

// Each status gets a distinct tone and icon, never colour alone, and every
// terminal state (plus the still-running default) is covered.
assert.deepEqual(workerStatusPresentation("running"), { label: "working", tone: "active", icon: "Loader2" });
assert.deepEqual(workerStatusPresentation("provisioning"), { label: "working", tone: "active", icon: "Loader2" });
assert.deepEqual(workerStatusPresentation("completed"), { label: "completed", tone: "success", icon: "CircleCheck" });
assert.deepEqual(workerStatusPresentation("reported"), { label: "completed", tone: "success", icon: "CircleCheck" });
assert.deepEqual(workerStatusPresentation("completed_with_issues"), { label: "completed with issues", tone: "warning", icon: "CircleAlert" });
assert.deepEqual(workerStatusPresentation("failed"), { label: "failed", tone: "danger", icon: "CircleAlert" });
assert.deepEqual(workerStatusPresentation("error"), { label: "failed", tone: "danger", icon: "CircleAlert" });
assert.deepEqual(workerStatusPresentation("cancelled"), { label: "cancelled", tone: "muted", icon: "CircleSlash" });
assert.deepEqual(workerStatusPresentation("canceled"), { label: "cancelled", tone: "muted", icon: "CircleSlash" });
assert.deepEqual(workerStatusPresentation(null), { label: "working", tone: "active", icon: "Loader2" });
console.log("worker status presentation tests passed");


// F1 fix: a known call renders its counts with no alarm; a genuine unknown
// call still shows the "call unavailable" alarm; and an attempt whose only
// calls were released_after_result (excluded entirely by sum_tokens, see
// server/worker-history.ts) never reaches this component with tokens at
// all, so null input renders nothing -- never the "Tokens unavailable"
// alarm a genuinely unaccounted-for call shows.
assert.equal(tokensSummaryLabel(null), null);
assert.equal(tokensSummaryLabel(undefined), null);
assert.equal(
  tokensSummaryLabel({ input: 657, output: 97, cachedRead: 107000, cachedWrite: 50, knownCalls: 1, unknownCalls: 0 }),
  "\u2191 657  \u2193 97  \u00b7 107k cache read  \u00b7 50 cache write",
);
assert.equal(
  tokensSummaryLabel({ input: 10, output: 5, cachedRead: 0, cachedWrite: 0, knownCalls: 1, unknownCalls: 1 }),
  "\u2191 10  \u2193 5  \u00b7 1 call unavailable",
);
assert.equal(
  tokensSummaryLabel({ input: 0, output: 0, cachedRead: 0, cachedWrite: 0, knownCalls: 0, unknownCalls: 2 }),
  "Tokens unavailable  \u00b7 2 calls unavailable",
);
assert.equal(tokensAccessibilityLabel(null), null);
assert.equal(
  tokensAccessibilityLabel({ input: 1, output: 2, cachedRead: 3, cachedWrite: 4, knownCalls: 1, unknownCalls: 0 }),
  "1 uncached input, 2 output, 3 cache read, 4 cache write tokens",
);
assert.equal(
  tokensAccessibilityLabel({ input: 0, output: 0, cachedRead: 0, cachedWrite: 0, knownCalls: 0, unknownCalls: 1 }),
  "Token usage unavailable",
);
// The worker page's route choice: the same parts as the card's line, with
// the chosen plan's usage never shown as a bare `usage`.
assert.equal(routeSelectionText({ tier: "light→standard", hint: "ignored:pinned", decidedBy: "usage", planUsedPercent: 42.4 }),
  "light→standard · hint ignored:pinned · by usage · plan usage 42%");
assert.equal(routeSelectionText({ tier: "deep", hint: null, decidedBy: null, planUsedPercent: null }), "deep · plan usage unknown");
assert.equal(routeSelectionText(null), null);
assert.equal(routeSelectionText(undefined), null);
console.log("usage meter token label tests passed");
