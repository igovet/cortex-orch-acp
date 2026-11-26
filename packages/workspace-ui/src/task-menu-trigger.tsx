import { useWs } from "./ctx";
import { I } from "./kit";
import type { TaskView } from "./selectors";

/** Runs `open` once the pointer press under way has ended (or was cancelled, or the window lost it), after that event. */
function afterRelease(open: () => void) {
  const events = ["pointerup", "mouseup", "pointercancel", "blur"] as const;
  let done = false;
  const end = () => {
    if (done) return;
    done = true;
    for (const name of events) window.removeEventListener(name, end, true);
    // After the release has been handled, so the popover's light dismiss of this press no longer applies to it.
    setTimeout(open, 0);
  };
  for (const name of events) window.addEventListener(name, end, true);
}

/** Keep secondary clicks and context keys separate from link activation and selection. */
export function taskContextMenu(taskId: () => string | undefined) {
  const ws = useWs();
  return {
    onContextMenu(e: MouseEvent & { currentTarget: HTMLElement }) {
      const id = taskId();
      if (!id) return;
      e.preventDefault();
      const target = e.target instanceof Element ? e.target.closest<HTMLElement>("a,button") : null;
      const trigger = target && e.currentTarget.contains(target) ? target : e.currentTarget.querySelector<HTMLElement>("a") ?? e.currentTarget;
      const point = e.button === 2 || e.clientX || e.clientY ? { x: e.clientX, y: e.clientY } : undefined;
      // Where the context menu comes on the press (Linux), the release that follows lands outside the menu and its popover
      // light dismiss closed it at once. With the button still held, the menu opens once the press has ended.
      if (e.buttons & 2) afterRelease(() => ws.openTaskMenu(id, trigger, point));
      else ws.openTaskMenu(id, trigger, point);
    },
    onKeyDown(e: KeyboardEvent & { currentTarget: HTMLElement }) {
      const id = taskId();
      if (!id || e.defaultPrevented) return;
      if (e.key !== "ContextMenu" && !(e.shiftKey && e.key === "F10")) return;
      e.preventDefault();
      ws.openTaskMenu(id, e.target instanceof HTMLElement ? e.target : e.currentTarget);
    },
  };
}

export function TaskMenuButton(props: { view: TaskView; label?: string }) {
  const ws = useWs();
  return <button type="button" data-tooltip="Actions for this task" class="ib" aria-haspopup="menu" aria-label={props.label ?? `Actions for ${props.view.task.title}`}
    onClick={e => ws.openTaskMenu(props.view.id, e.currentTarget)}><I n="more" s={14} /></button>;
}
