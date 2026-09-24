import { strict as assert } from "node:assert";
import {
  contextSummary, workerAssignmentLine, workerCardMetadata, workerDisplayName,
  workerModelDisplay, workerStatusPresentation,
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
