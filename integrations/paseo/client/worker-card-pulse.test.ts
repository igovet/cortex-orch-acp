import assert from "node:assert/strict";
import test from "node:test";
import {
  isWorkerExecutingStatus, setWorkerCardPulse, workerCardPulseActive,
  WORKER_PULSE_ATTRIBUTE, WORKER_PULSE_MIN_OPACITY, WORKER_PULSE_PERIOD_MS, WORKER_PULSE_STYLE_ID,
} from "./worker-card-pulse.ts";

function pulseFixture() {
  const styles = new Map<string, { id: string; textContent: string | null }>();
  const document = {
    getElementById(id: string) { return styles.get(id) ?? null; },
    createElement() { return { id: "", textContent: null as string | null }; },
    head: { appendChild(style: { id: string; textContent: string | null }) { styles.set(style.id, style); } },
  } as unknown as Document;
  const attributes = new Map<string, string>();
  const element = {
    ownerDocument: document,
    setAttribute(name: string, value: string) { attributes.set(name, value); },
    removeAttribute(name: string) { attributes.delete(name); },
  } as unknown as HTMLElement;
  return { styles, attributes, element, document };
}

test("an executing indicator keeps its CSS pulse through a hidden panel and return without a status change", () => {
  const { styles, attributes, element } = pulseFixture();
  const panel = { display: "block" };
  setWorkerCardPulse(element, true);
  assert.equal(attributes.get(WORKER_PULSE_ATTRIBUTE), "true");
  const style = styles.get(WORKER_PULSE_STYLE_ID);
  assert.ok(style?.textContent?.includes("animation: cortex-worker-indicator-pulse 1.6s ease-in-out infinite"));

  panel.display = "none";
  panel.display = "block";
  assert.equal(panel.display, "block");
  assert.equal(attributes.get(WORKER_PULSE_ATTRIBUTE), "true");
  assert.equal(styles.size, 1);

  setWorkerCardPulse(element, false);
  assert.equal(attributes.has(WORKER_PULSE_ATTRIBUTE), false);
  setWorkerCardPulse(element, true);
  assert.equal(attributes.get(WORKER_PULSE_ATTRIBUTE), "true");
  assert.equal(styles.size, 1, "one shared style element, never one per card or per cycle");
});

test("the pulse is gentle, slow and static under reduced motion", () => {
  const { styles, element } = pulseFixture();
  setWorkerCardPulse(element, true);
  const css = styles.get(WORKER_PULSE_STYLE_ID)!.textContent!;
  // Well under three flashes a second, and never fully invisible.
  assert.ok(1000 / WORKER_PULSE_PERIOD_MS < 1);
  assert.ok(WORKER_PULSE_MIN_OPACITY >= 0.3 && WORKER_PULSE_MIN_OPACITY < 1);
  assert.match(css, /50% \{ opacity: 0\.35; \}/);
  assert.match(css, /@media \(prefers-reduced-motion: reduce\) \{\s*\[data-cortex-worker-pulse="true"\] \{ animation: none; \}/);
  // Only opacity animates: no transform, colour, border or outline change.
  assert.doesNotMatch(css, /transform|color|border|outline|scale/);
});

test("an inactive indicator never injects the style or the attribute", () => {
  const { styles, attributes, element } = pulseFixture();
  setWorkerCardPulse(element, false);
  assert.equal(styles.size, 0);
  assert.equal(attributes.size, 0);
});

test("only an executing status pulses", () => {
  for (const status of ["running", "RUNNING", "in_progress", "in-progress", "working", "started"]) {
    assert.equal(isWorkerExecutingStatus(status), true, status);
  }
  for (const status of ["planned", "pending", "queued", "provisioning", "ready", "starting", "waiting", "interrupted",
    "completed", "completed_with_issues", "failed", "cancelled", "canceled", "", null, undefined]) {
    assert.equal(isWorkerExecutingStatus(status), false, String(status));
  }
});

test("a coordinator card pulses while executing and stops once settled or waiting", () => {
  // Host says running, compact read not landed yet: executing.
  assert.equal(workerCardPulseActive("running", undefined), true);
  assert.equal(workerCardPulseActive("running", "running"), true);
  // The durable attempt is still queued/starting: not yet working.
  assert.equal(workerCardPulseActive("running", "pending"), false);
  assert.equal(workerCardPulseActive("running", "provisioning"), false);
  // Settled or paused cards never pulse, whatever the durable row says.
  for (const settled of ["completed", "completed_with_issues", "failed", "cancelled", "canceled", "interrupted"]) {
    assert.equal(workerCardPulseActive(settled, "running"), false, settled);
    assert.equal(workerCardPulseActive(settled, undefined), false, settled);
  }
});
