import assert from "node:assert/strict";
import test from "node:test";
import { setWorkerCardPulse, WORKER_PULSE_ATTRIBUTE, WORKER_PULSE_STYLE_ID } from "./worker-card-pulse.ts";

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
  return { styles, attributes, element };
}

test("a Working card keeps its CSS pulse through a hidden panel and return without a status change", () => {
  const { styles, attributes, element } = pulseFixture();
  const panel = { display: "block" };
  setWorkerCardPulse(element, true);
  assert.equal(attributes.get(WORKER_PULSE_ATTRIBUTE), "true");
  const style = styles.get(WORKER_PULSE_STYLE_ID);
  assert.ok(style?.textContent?.includes("animation: cortex-worker-card-pulse 1.7s ease-in-out infinite"));
  assert.ok(style.textContent.includes("prefers-reduced-motion: reduce"));

  panel.display = "none";
  panel.display = "block";
  assert.equal(panel.display, "block");
  assert.equal(attributes.get(WORKER_PULSE_ATTRIBUTE), "true");
  assert.equal(styles.size, 1);

  setWorkerCardPulse(element, false);
  assert.equal(attributes.has(WORKER_PULSE_ATTRIBUTE), false);
  setWorkerCardPulse(element, true);
  assert.equal(attributes.get(WORKER_PULSE_ATTRIBUTE), "true");
  assert.equal(styles.size, 1);
});
