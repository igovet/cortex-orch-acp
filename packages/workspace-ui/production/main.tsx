import { render } from "solid-js/web";
import { createRealPort, unavailableTransport, Workspace, type RealTransport } from "../src";

/**
 * The production entry. The native bridge supplies the transport; until it does, every call fails and the screen says so.
 * There is no other backend in this bundle to fall back to.
 */
export function mountWorkspace(root: HTMLElement, transport: RealTransport = unavailableTransport()) {
  return render(() => <Workspace port={createRealPort({ transport, actorId: "local-peer" })} />, root);
}
mountWorkspace(document.getElementById("root")!);
