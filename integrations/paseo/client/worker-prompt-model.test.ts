import assert from "node:assert/strict";
import { promptSections, promptView } from "./worker-prompt-model.ts";
import { groupWorkerEvents, withoutShownResult, type WorkerEvent } from "./worker-panel-events.ts";
import { attemptResultView } from "./worker-attempt-result.ts";
import { cardFrameStyle } from "./worker-card-model.ts";

// --- Assignment prompt of the selected attempt -------------------------------

const brief = JSON.stringify({
  objective: "Fix **the** card.\n\n- keep `paging`",
  scope: ["."],
  dependency_reports: [{ node: "investigate", report_ref: "r_0123456789ab" }],
  original_goal: "Когда вставляется воркер, он разрывает текст",
  review_evidence: [],
});

{
  const view = promptView({ state: "sent", sentAt: "2026-10-21T18:47:05Z", brief, acceptance: "Publish a truthful report." });
  assert.ok(view);
  assert.equal(view.status, "Sent to this attempt · 2026-10-21T18:47:05Z");
  assert.equal(view.tone, "normal");
  assert.deepEqual(view.sections?.map((section) => section.heading), ["Objective", "Scope", "Dependency reports", "Original goal", "Acceptance"]);
  // Markdown stays the original text, untouched.
  assert.deepEqual(view.sections?.[0], { kind: "markdown", heading: "Objective", text: "Fix **the** card.\n\n- keep `paging`" });
  assert.deepEqual(view.sections?.[1], { kind: "list", heading: "Scope", items: ["."] });
  assert.deepEqual(view.sections?.[2], { kind: "list", heading: "Dependency reports", items: ["investigate · r_0123456789ab"] });
  assert.equal(view.preview, "Fix **the** card.");
  assert.equal(view.truncatedNote, null);
}

// A title the runtime prefixed (because the brief did not already contain
// it) is shown first; a server display clip is stated.
{
  const view = promptView({ state: "sent", title: "Separate title", brief, acceptance: "A", truncated: true });
  assert.equal(view?.sections?.[0].heading, "Title");
  assert.match(view?.truncatedNote ?? "", /clipped for display; the worker received the complete text/);
}

// Not yet recorded as sent, or older history: content is the dispatched
// assignment and says so; never presented as confirmed delivery.
{
  const queued = promptView({ state: "queued", brief, acceptance: "A" });
  assert.match(queued!.status, /^Queued: not yet recorded as sent/);
  assert.equal(queued!.tone, "warning");
  assert.ok(queued!.sections);
  const unrecorded = promptView({ state: "unrecorded", brief, acceptance: "A" });
  assert.match(unrecorded!.status, /not recorded for this attempt/);
  assert.ok(unrecorded!.sections);
}

// Mismatched or unavailable: no content, a truthful reason; no prompt data
// at all (catalogue read, older server) renders nothing.
for (const state of ["mismatch", "unavailable"] as const) {
  const view = promptView({ state, brief, acceptance: "A" });
  assert.equal(view?.sections, null);
  assert.equal(view?.tone, "warning");
}
assert.equal(promptView(undefined), null);

// A legacy plain-text brief and a goal shortened in delivery.
assert.deepEqual(promptSections("Do the thing\nin Markdown"), [{ kind: "markdown", heading: "Assignment", text: "Do the thing\nin Markdown" }]);
{
  const sections = promptSections(JSON.stringify({ objective: "O", scope: ["src"], dependency_reports: [],
    original_goal: { truncated: true, original_characters: 900, shown_characters: 300, preview: "goal start" },
    review_evidence: { truncated: true, original_characters: 50, shown_characters: 10, preview: "{\"kind\":" }, extra: 1 }));
  assert.deepEqual(sections.map((section) => section.heading), [
    "Objective", "Scope", "Original goal (shortened in delivery: 300 of 900 characters)",
    "Review evidence (shortened in delivery: 10 of 50 characters)", "Other fields"]);
}

// --- Result shown once -------------------------------------------------------

const event = (id: number, changes: Partial<WorkerEvent>): WorkerEvent => ({ id, type: "worker_activity", createdAt: String(id), text: `event ${id}`, ...changes });
{
  const summary = "Reconciled the original requests.\nNo files were changed.";
  const attempt = { status: "completed", summary, checks: [] };
  const shown = attemptResultView(attempt)?.summary;
  const events = [
    event(1, { activity_ref: "tool:1", tool_kind: "read", title: "Read a.ts", text: "Read a.ts" }),
    event(2, { type: "worker_result", text: "Reconciled the original requests. No files were changed." }),
    // A replayed/refreshed identical row is the same Result, not new prose.
    event(3, { type: "worker_result", text: summary }),
    event(4, { activity_ref: "final-response", text: "[final response] Published acceptance report." }),
    // Similar prose in another event type is the worker's own and stays.
    event(5, { text: summary }),
    // A different result text is not the shown summary and stays.
    event(6, { type: "worker_result", text: "An earlier, different result." }),
  ];
  const kept = withoutShownResult(events, shown);
  assert.deepEqual(kept.map((item) => item.id), [1, 4, 5, 6]);
  assert.equal(groupWorkerEvents(kept).filter((row) => row.event.type === "worker_result").length, 1);
  // Nothing is removed when no summary is shown, and the input is untouched.
  assert.equal(withoutShownResult(events, null).length, events.length);
  assert.equal(withoutShownResult(events, "  ").length, events.length);
  assert.equal(events.length, 6);
}

// --- Coordinator card frame --------------------------------------------------

{
  const options = { disabled: false, web: true, compact: false, border: "#333", surface: "#111", hoverSurface: "#222" };
  const states = [{ pressed: false }, { pressed: false, hovered: true }, { pressed: false, focused: true }, { pressed: true }];
  for (const state of states) {
    const style = cardFrameStyle(state, options);
    // The same neutral boundary in every state; no outline/perimeter colour.
    assert.equal(style.borderWidth, 1);
    assert.equal(style.borderColor, "#333");
    assert.equal(style.outlineWidth, 0);
    assert.equal(style.outlineStyle, "none");
    assert.ok(!("outlineColor" in style) && !("outlineOffset" in style) && !("borderLeftWidth" in style));
  }
  assert.equal(cardFrameStyle({ pressed: false }, options).backgroundColor, "#111");
  assert.equal(cardFrameStyle({ pressed: true }, options).backgroundColor, "#222");
  assert.equal(cardFrameStyle({ pressed: true }, { ...options, disabled: true }).backgroundColor, "#111");
  assert.ok(!("outlineWidth" in cardFrameStyle({ pressed: false }, { ...options, web: false })));
  // Keyboard focus shows as the hover fill, never as a perimeter.
  assert.equal(cardFrameStyle({ pressed: false, focused: true }, options).backgroundColor, "#222");
  assert.equal(cardFrameStyle({ pressed: false, focused: true }, { ...options, disabled: true }).backgroundColor, "#111");
}

console.log("worker prompt, shown-result and card frame tests passed");
