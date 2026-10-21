import { workerStatusLabel } from "./worker-display.ts";

/**
 * The executing worker's indicator pulse: only the small worker/status
 * marker breathes, never the card, its text or its perimeter. One gentle
 * 1.6 s opacity cycle (well under three flashes a second), static under a
 * reduced-motion preference.
 */
export const WORKER_PULSE_STYLE_ID = "cortex-worker-card-pulse-style";
export const WORKER_PULSE_ATTRIBUTE = "data-cortex-worker-pulse";
export const WORKER_PULSE_PERIOD_MS = 1600;
export const WORKER_PULSE_MIN_OPACITY = 0.35;

const WORKER_PULSE_CSS = `
@keyframes cortex-worker-indicator-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: ${WORKER_PULSE_MIN_OPACITY}; }
}
[${WORKER_PULSE_ATTRIBUTE}="true"] {
  animation: cortex-worker-indicator-pulse ${WORKER_PULSE_PERIOD_MS / 1000}s ease-in-out infinite;
}
@media (prefers-reduced-motion: reduce) {
  [${WORKER_PULSE_ATTRIBUTE}="true"] { animation: none; }
}
`;

/** Attempt statuses in which a worker is actually executing. Queued,
 * planned, provisioning, ready and starting are waits, not work; terminal
 * and interrupted statuses never pulse. */
const EXECUTING = new Set(["running", "in_progress", "in-progress", "working", "started"]);

export function isWorkerExecutingStatus(status: string | null | undefined): boolean {
  return EXECUTING.has((status ?? "").trim().toLowerCase());
}

/**
 * Whether a coordinator card's indicator pulses: the card must read
 * Working, and the durable attempt status, once the compact read has one,
 * must be an executing status (the host's own `running` stands in only
 * until that read lands). A settled card never pulses.
 */
export function workerCardPulseActive(resolvedStatus: string | null | undefined, durableStatus: string | null | undefined): boolean {
  if (workerStatusLabel(resolvedStatus) !== "working") return false;
  const durable = (durableStatus ?? "").trim();
  return isWorkerExecutingStatus(durable || resolvedStatus);
}

/** Web: a CSS animation keyed on one attribute, so no timer or per-frame
 * state runs in JavaScript and a hidden host panel resumes the pulse when it
 * becomes visible again. Inactive removes the attribute, stopping it. */
export function setWorkerCardPulse(element: HTMLElement, active: boolean): void {
  if (!active) {
    element.removeAttribute(WORKER_PULSE_ATTRIBUTE);
    return;
  }
  const document = element.ownerDocument;
  if (!document.getElementById(WORKER_PULSE_STYLE_ID)) {
    const style = document.createElement("style");
    style.id = WORKER_PULSE_STYLE_ID;
    style.textContent = WORKER_PULSE_CSS;
    document.head.appendChild(style);
  }
  element.setAttribute(WORKER_PULSE_ATTRIBUTE, "true");
}
