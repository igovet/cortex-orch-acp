/** Keep a live worker card breathing across host tab hide/show cycles. */
export const WORKER_PULSE_STYLE_ID = "cortex-worker-card-pulse-style";
export const WORKER_PULSE_ATTRIBUTE = "data-cortex-worker-pulse";

const WORKER_PULSE_CSS = `
@keyframes cortex-worker-card-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.58; }
}
[${WORKER_PULSE_ATTRIBUTE}="true"] {
  animation: cortex-worker-card-pulse 1.7s ease-in-out infinite;
}
@media (prefers-reduced-motion: reduce) {
  [${WORKER_PULSE_ATTRIBUTE}="true"] { animation: none; }
}
`;

/** CSS animations resume when a hidden host panel becomes visible again. */
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
