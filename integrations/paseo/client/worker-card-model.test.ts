import assert from "node:assert/strict";
import test from "node:test";
import { cardAccessibilityLabel, cardAttention, cardIssueCount, cardStatusAnnouncement, cardStatusKey, cardUsageSnapshot, sameCardUsage, shouldAnnounceCardStatus, CARD_ANNOUNCEMENT_TITLE_MAX_CHARS } from "./worker-card-model.ts";
import { resolveDurableWorkerCardStatus, workerCardIdentity, workerStatusPresentation } from "./worker-display.ts";
import { openIssuesOfResult, workerCardOutcome } from "../shared/worker-card-outcome.ts";
import { attemptResultView } from "./worker-attempt-result.ts";

test("card identity separates role, actual route and title, without route-selection provenance", () => {
  const identity = workerCardIdentity("Worker: technical-writer · claude · Opus 5.5 · high · flagship · by user_request · usage 25% — Rewrite the audit");
  assert.deepEqual(identity, { title: "Rewrite the audit", role: "technical-writer", route: "claude · Opus 5.5 · high" });
  const crossTier = workerCardIdentity("Worker: debugger · codex · gpt (alias) · light→standard · hint ignored · by usage · usage unknown — Fix · Interrupted");
  assert.equal(crossTier.route, "codex · gpt (alias)");
  assert.equal(crossTier.title, "Fix");
  assert.deepEqual(workerCardIdentity("Worker: explorer — Look"), { title: "Look", role: "explorer", route: null });
  // A very long title is kept whole (the card clips visually, the accessible name keeps it).
  const long = "x".repeat(3000);
  assert.equal(workerCardIdentity(`Worker: explorer — ${long}`).title, long);
});

test("the open-issue count reduces current, legacy and raw results, never guessing", () => {
  assert.equal(openIssuesOfResult({ summary: "s", checks: [
    { name: "a", outcome: "passed" }, { name: "b", outcome: "failed", closedBy: "gateway:1" },
    { name: "c", outcome: "failed", closed_by: "gateway:2" }, { name: "d", outcome: "not_run" }, { name: "e", outcome: "unverified" },
  ] }), 2);
  assert.equal(openIssuesOfResult({ summary: 3, checks: [] }), undefined);
  assert.equal(openIssuesOfResult({ summary: null, checks: [{ outcome: "passed" }] }), undefined);
  assert.equal(openIssuesOfResult("plain failure text"), undefined);
  assert.deepEqual(workerCardOutcome({ cortexWorkerOutcome: { openIssues: 3 } }), { openIssues: 3 });
  assert.deepEqual(workerCardOutcome({ cortexWorkerResult: { summary: "x", checks: [{ name: "a", outcome: "failed" }] } }), { openIssues: 1 });
  assert.deepEqual(workerCardOutcome(undefined, { summary: null, checks: [] }), { openIssues: 0 });
  assert.equal(workerCardOutcome({ cortexWorkerOutcome: { openIssues: -1 } }), undefined);
  assert.equal(workerCardOutcome(null), undefined);
});

test("the card keeps a compact usage snapshot and an unchanged poll keeps the same state", () => {
  const attempt = { status: "running", context: { used: 10, size: 100 }, tokens: { input: 1, output: 2, cachedRead: 0, cachedWrite: 0, total: 3, knownCalls: 1, unknownCalls: 0 },
    summary: "BODY", checks: [{ name: "c", outcome: "passed" }], events: [], failureReason: null, id: "a", attemptNo: 1 };
  const snapshot = cardUsageSnapshot(attempt as never);
  assert.deepEqual(Object.keys(snapshot).sort(), ["context", "status", "tokens"]);
  assert.equal(JSON.stringify(snapshot).includes("BODY"), false);
  assert.ok(sameCardUsage(snapshot, cardUsageSnapshot(attempt as never)));
  assert.ok(!sameCardUsage(snapshot, cardUsageSnapshot({ ...attempt, context: { used: 11, size: 100 } } as never)));
  assert.ok(!sameCardUsage(snapshot, cardUsageSnapshot({ ...attempt, status: "completed" } as never)));
  assert.ok(!sameCardUsage(snapshot, cardUsageSnapshot({ ...attempt, openIssues: 1 } as never)));
  assert.ok(!sameCardUsage(undefined, snapshot));
});

test("issue count and attention stay short and category-level", () => {
  assert.equal(cardIssueCount("completed", 3), null);
  assert.equal(cardIssueCount("completed with issues", undefined, 2), 2);
  assert.equal(cardIssueCount("completed with issues", 1, 2), 1, "the durable count wins over the item's");
  assert.equal(cardIssueCount("completed with issues"), null);
  assert.equal(cardAttention("working", null), null);
  assert.equal(cardAttention("completed", null), null);
  assert.equal(cardAttention("completed with issues", 2), "2 open issues · details in the worker tab");
  assert.equal(cardAttention("completed with issues", null), "Issues reported · details in the worker tab");
  // Whatever a reason says -- a check list, a blocker, a cancellation token,
  // prose in any language, one line or many, short or long -- the card shows
  // only a fixed category and where to look; the reason is worker detail.
  const reasons = [
    "Completed with issues. Runtime did not accept worker checks. HEAD identity: unverified",
    "lint: failed; typecheck: not_run (paid live check not requested by the owner)",
    "Worker crashed: engine exited 1\nTraceback ...",
    "Исполнитель не запустился: CODEX_AUTH_MISSING",
    "工作进程失败",
    "x",
    "e".repeat(5000),
  ];
  for (const reason of reasons) {
    assert.equal(cardAttention("failed", null, reason), "Failure details in the worker tab");
    assert.equal(cardAttention("cancelled", null, reason), "Cancellation details in the worker tab");
  }
  assert.equal(cardAttention("cancelled", null, "Cancelled: turn_interrupted"), "Cancellation details in the worker tab");
  assert.equal(cardAttention("failed", null), "Failure details in the worker tab");
  assert.equal(cardAttention("cancelled", null), null);
  assert.equal(cardAttention("cancelled", null, "  "), null);
  assert.equal(cardAttention("interrupted", null, "long notice"), "Interrupted · details in the worker tab");
  assert.equal(cardAttention("interrupted", null), "Interrupted · details in the worker tab");
});

test("an interrupted card whose recovery writes its attempt off never promises that the worker resumes", () => {
  // A host disconnect left the card running with the portable "Interrupted"
  // title suffix; recovery then marked the attempt lost (assignment_failed,
  // recovery_worker_lost), which the history reader projects as
  // "interrupted", while the graph row itself reads "failed". A new attempt
  // may follow: the interrupted one is not guaranteed to resume.
  const title = "Worker: debugger · codex · gpt — Fix the parser · Interrupted";
  const cases: Array<[string, string | null]> = [
    ["running", "interrupted"],
    ["running", "failed"],
    ["failed", "interrupted"],
    ["failed", "failed"],
    ["running", null],
  ];
  for (const [hostStatus, durable] of cases) {
    const label = workerStatusPresentation(resolveDurableWorkerCardStatus(title, hostStatus, durable)).label;
    assert.equal(label, "interrupted", `${hostStatus}/${durable}`);
    const attention = cardAttention(label, cardIssueCount(label, 0), "recovery_worker_lost: worker host disconnected");
    assert.equal(attention, "Interrupted · details in the worker tab");
    assert.doesNotMatch(attention ?? "", /resum|continu|saved state|recover|retry/i);
    assert.equal(attention?.includes("recovery_worker_lost"), false, "the reason stays worker detail");
  }
  // Without the title suffix, a durably interrupted attempt still settles a
  // running card to the same neutral notice.
  const plain = workerStatusPresentation(resolveDurableWorkerCardStatus("Worker: debugger — Fix", "running", "interrupted")).label;
  assert.equal(cardAttention(plain, null), "Interrupted · details in the worker tab");
});

test("a status announcement names the worker and fires only on a real transition", () => {
  assert.equal(cardStatusAnnouncement({ title: "Fix the parser.", role: "debugger", status: "Completed with issues", issueCount: 2 }),
    "Worker debugger, Fix the parser: Completed with issues, 2 open issues");
  assert.equal(cardStatusAnnouncement({ title: "Review", role: "reviewer", status: "Failed", issueCount: null }), "Worker reviewer, Review: Failed");
  const long = cardStatusAnnouncement({ title: "t".repeat(3000), role: "r", status: "Working", issueCount: null });
  assert.ok(long.includes("t".repeat(CARD_ANNOUNCEMENT_TITLE_MAX_CHARS - 1) + "…"));
  assert.ok(long.length < 200);

  const working = cardStatusKey("Working", null);
  const done = cardStatusKey("Completed", null);
  assert.equal(shouldAnnounceCardStatus(null, { key: working, loaded: false }), false, "mount or replay is not a transition");
  assert.equal(shouldAnnounceCardStatus({ key: working, loaded: false }, { key: working, loaded: true }), false, "first read, same status");
  assert.equal(shouldAnnounceCardStatus({ key: working, loaded: false }, { key: done, loaded: true }), false,
    "the first durable read reconciling a replayed card is not a transition");
  assert.equal(shouldAnnounceCardStatus({ key: working, loaded: true }, { key: working, loaded: true }), false, "a usage poll changes no status");
  assert.equal(shouldAnnounceCardStatus({ key: working, loaded: true }, { key: done, loaded: true }), true);
  assert.equal(shouldAnnounceCardStatus({ key: working, loaded: false }, { key: done, loaded: false }), true, "a host status update");
  assert.equal(shouldAnnounceCardStatus({ key: cardStatusKey("Completed with issues", 1), loaded: true },
    { key: cardStatusKey("Completed with issues", 2), loaded: true }), true, "a changed issue count");
});

test("the accessible name carries the full title, role, status and the action", () => {
  const label = cardAccessibilityLabel({ title: "A long task", role: "debugger", status: "Completed with issues", route: "claude · Opus", issueCount: 2, disabled: false });
  assert.equal(label, "A long task. debugger, claude · Opus. Status: Completed with issues, 2 open issues. Open worker");
  assert.match(cardAccessibilityLabel({ title: "t", role: "r", status: "Working", route: null, issueCount: null, disabled: true }), /Worker unavailable$/);
});

test("the worker tab's result view binds to the attempt and distinguishes every check outcome", () => {
  assert.equal(attemptResultView({ status: "running" }), null, "active attempt: no terminal sections");
  assert.equal(attemptResultView({ status: "failed" }), null, "failed without a result: only its failure reason");
  assert.deepEqual(attemptResultView({ status: "completed" }), { summary: null, summaryTruncated: false, checks: null, openCount: 0 },
    "absent check data reads as unavailable, not as an empty list");
  assert.deepEqual(attemptResultView({ status: "completed", summary: "Done", checks: [] })?.checks, []);
  const view = attemptResultView({ status: "completed_with_issues", summary: "S".repeat(16000), summaryTruncated: true, checks: [
    { name: "unit", outcome: "passed" },
    { name: "lint", outcome: "failed", blocker: "2 errors" },
    { name: "old", outcome: "failed", closedBy: "gateway:9" },
    { name: "live", outcome: "not_run", blocker: "paid live check not requested by the owner" },
    { name: "weak", outcome: "unverified", claimedOutcome: "passed", evidenceGap: "tool_call_ref_missing" },
  ] })!;
  assert.equal(view.summaryTruncated, true);
  assert.equal(view.summary?.length, 16000);
  assert.equal(view.openCount, 3);
  assert.deepEqual(view.checks!.map((check) => check.label), ["Passed", "Failed", "Resolved", "Not run", "Unverified"]);
  assert.deepEqual(view.checks!.map((check) => check.tone), ["success", "danger", "muted", "warning", "warning"]);
  assert.deepEqual(view.checks![1].details, ["2 errors"]);
  assert.deepEqual(view.checks![2].details, ["Closed by a later identical rerun."]);
  assert.equal(view.checks![4].weakEvidence, true);
  assert.deepEqual(view.checks![4].details, ["Evidence gap: tool_call_ref_missing", "Claimed passed; kept unverified by the runtime."]);
  assert.match(view.checks![3].accessibilityLabel, /^live\. Not run\. paid live check/);
});
