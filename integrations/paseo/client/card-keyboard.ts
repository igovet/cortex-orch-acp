/**
 * Space activation for the coordinator worker card.
 *
 * The card is one whole-card button. Enter and a click open the worker
 * through the button's native activation, but in a recorded Paseo 0.12.0
 * Desktop run a focused card ignored Space; the installed bundle binds a
 * bare Space outside text fields to a voice-mute shortcut handled by a
 * window capture listener, ahead of the button. A
 * button must activate on Space, so the card handles exactly that key
 * itself, for its own focused element only: keydown is claimed (no page
 * scroll) and the card activates on keyup, as a native button does. Keyup is
 * prevented and stopped, so neither a native button click nor
 * react-native-web's own key handling can open the worker a second time;
 * the shared activation guard additionally absorbs any duplicate activation
 * within one gesture. Enter, mouse and touch keep their native paths.
 */

export type CardKeyEvent = {
  key: string;
  target: unknown;
  repeat?: boolean;
  altKey?: boolean;
  ctrlKey?: boolean;
  metaKey?: boolean;
  preventDefault(): void;
  stopPropagation(): void;
};

export function isSpaceActivationKey(event: Pick<CardKeyEvent, "key" | "altKey" | "ctrlKey" | "metaKey">): boolean {
  return (event.key === " " || event.key === "Spacebar") && !event.altKey && !event.ctrlKey && !event.metaKey;
}

/** One activation per gesture: a second call within `windowMs` of the last
 * accepted one is ignored, so two input paths can never navigate twice. */
export function createActivationGuard(activate: () => void, windowMs = 400, now: () => number = Date.now): () => boolean {
  let last = -Infinity;
  return () => {
    const at = now();
    if (at - last < windowMs) return false;
    last = at;
    activate();
    return true;
  };
}

/** Keydown/keyup handlers (window, capture phase) that turn Space on the
 * card's own element into one activation on release. */
export function createSpaceActivation(input: { isCard(target: unknown): boolean; activate(): void }) {
  let armed = false;
  return {
    keydown(event: CardKeyEvent): void {
      if (!isSpaceActivationKey(event) || !input.isCard(event.target)) return;
      event.preventDefault();
      if (!event.repeat) armed = true;
    },
    keyup(event: CardKeyEvent): void {
      if (!isSpaceActivationKey(event)) return;
      const fire = armed && input.isCard(event.target);
      armed = false;
      if (!fire) return;
      event.preventDefault();
      event.stopPropagation();
      input.activate();
    },
    /** Focus left the card mid-press: a later release elsewhere does nothing. */
    cancel(): void {
      armed = false;
    },
  };
}
