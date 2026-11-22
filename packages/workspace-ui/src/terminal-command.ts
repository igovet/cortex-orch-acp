import { createSignal } from "solid-js";

/**
 * A command the owner asked to run in a project's shell (an owner action's sign-in, say). It is typed into that project's
 * terminal once its shell runs, then forgotten. Only one waits per project; a newer one replaces it. It never holds a secret:
 * a sign-in command asks for its own input in the shell.
 */
const [queued, setQueued] = createSignal<ReadonlyMap<string, string>>(new Map());

/** Queue `text` (one line, no newline) for the terminal of `projectId`. */
export function queueTerminalCommand(projectId: string, text: string) {
  if (!projectId || !text || /[\r\n\0]/.test(text)) return;
  setQueued(prev => new Map(prev).set(projectId, text));
}
/** The command waiting for `projectId`, tracked. */
export const queuedTerminalCommand = (projectId: string): string | undefined => queued().get(projectId);
/** Takes the command waiting for `projectId`, once. */
export function takeTerminalCommand(projectId: string): string | undefined {
  const text = queued().get(projectId);
  if (text !== undefined) setQueued(prev => { const next = new Map(prev); next.delete(projectId); return next; });
  return text;
}
