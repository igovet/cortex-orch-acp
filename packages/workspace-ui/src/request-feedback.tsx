import { createSignal, type JSX } from "solid-js";
import type { WorkspaceCommand } from "@cortex/workspace-domain";
import { REQUESTING } from "./copy";
import { useWs } from "./ctx";

export { REQUESTING };

/** The status stays mounted for polite announcement and reserves the same space when empty. */
export function RequestStatus(props: { text?: string }) {
  return <div class="ws-request-status" role="status">{props.text}</div>;
}

/** Button labels share one grid box. Both contribute to sizing; only the current one is exposed. */
export function RequestLabel(props: { pending: boolean; text: string; children: JSX.Element }) {
  return <span class="ws-request-label">
    <span data-hidden={props.pending ? "" : undefined}>{props.children}</span>
    <span aria-hidden="true" class="ws-request-size" data-label={props.text} />
    <span role="status">{props.pending ? props.text : undefined}</span>
  </span>;
}

/** A composer or confirm dialog follows its exact submitted payload even if its fields change. */
export function createRequestFeedback() {
  const ws = useWs();
  const [command, setCommand] = createSignal<WorkspaceCommand>();
  return {
    run(command: WorkspaceCommand) { setCommand(command); return ws.run(command); },
    text: () => {
      const c = command();
      return c && ws.store.pendingRequest(c) ? REQUESTING[c.type as keyof typeof REQUESTING] : undefined;
    },
  };
}
