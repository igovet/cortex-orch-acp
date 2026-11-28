import { createRoot, createSignal } from "solid-js";
import { scrollIntoFeed } from "./follow";

/**
 * A header action (Answer, See what is needed) that brings a card of a task's conversation into view. When the card is not
 * on the page yet, because that conversation is still on its first read, the request waits for it: bound to one machine,
 * task and execution, it is kept until the conversation shows the card, and is applied there after the conversation's own
 * first pin to its end, so that pin never undoes it. A newer request replaces it; another task or execution never takes it.
 * One request for the window: the header and the conversation see the store through different per-tab views of it.
 */
export type RevealIntent = { key: string; selectors: readonly string[]; focus?: boolean };
const [intent, setIntent] = createRoot(() => createSignal<RevealIntent | null>(null));

/** The conversation a request is for: machine, task and execution. */
export const revealKey = (machine: string, task: string, execution: string | null) => `${machine}|${task}|${execution ?? ""}`;

/** The first card any of `selectors` finds under `root`, in their order. */
export const firstCard = (root: ParentNode, selectors: readonly string[]) => {
  for (const selector of selectors) {
    const card = [...root.querySelectorAll(selector)].find(node => !node.closest("[hidden]"));
    if (card) return card;
  }
  return null;
};

/** Moves keyboard focus to the existing decision without opening a second editor. */
export function revealDecision(card: Element, focus = false) {
  const controls = 'button[role="radio"], textarea, button.btn';
  const target = focus && [...card.querySelectorAll<HTMLElement>(controls)].find(node =>
    !node.matches(':disabled, [aria-disabled="true"]') && !node.closest("[hidden]"));
  if (target) target.focus({ preventScroll: true });
  scrollIntoFeed(target || card);
}

/** Brings the card into view now when `root` shows it; otherwise keeps the request for that conversation. */
export function revealCard(key: string, root: ParentNode, selectors: readonly string[], focus = false) {
  const card = firstCard(root, selectors);
  if (card) { setIntent(null); revealDecision(card, focus); return; }
  setIntent({ key, selectors, focus });
}

/** The request waiting for this conversation, if any (reactive). */
export const revealIntent = (key: string) => { const asked = intent(); return asked?.key === key ? asked : null; };

/** The request was applied, or its conversation is in without that card: it no longer waits. */
export const settleReveal = (asked: RevealIntent) => { if (intent() === asked) setIntent(null); };
