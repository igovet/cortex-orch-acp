import { useWs } from "./ctx";
import { I } from "./kit";
import type { TaskView } from "./selectors";

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
      ws.openTaskMenu(id, trigger, e.button === 2 || e.clientX || e.clientY ? { x: e.clientX, y: e.clientY } : undefined);
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
