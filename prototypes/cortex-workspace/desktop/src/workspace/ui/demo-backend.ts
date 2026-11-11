import type { AttachmentInput, CommandReceipt, PortCapabilities, PortUpdate, ProjectionUpdate, QueryOperation, QueryParameters, QueryResults, WorkspacePort as Port } from "@cortex/workspace-domain";
import type { MockReceipt, WorkspaceCommand, WorkspacePort as MockPort, WorkspaceState } from "../domain";

const EVERYTHING: PortCapabilities = { everything: true, operations: [] };

/**
 * The old mock port behind the production seam. Every command changes memory first and then publishes a confirmed
 * projection, in the same turn, so the demo keeps the timing the prototype was accepted with. Nothing here can be reached
 * from a real entry.
 */
export function createDemoBackend(createPort: () => MockPort, now: number) {
  let mock = createPort();
  let current = mock.snapshot();
  const savedFiles = new Map<string, AttachmentInput[]>();
  let version = 1;
  const opened = Date.now();
  const listeners = new Set<(update: PortUpdate) => void>();
  const projection = (): ProjectionUpdate => ({ kind: "projection", version: String(version), scope: "workspace", state: current });
  const publish = () => {
    version++;
    current = mock.snapshot();
    const update = projection();
    for (const listener of listeners) listener(update);
  };
  const apply = (command: WorkspaceCommand): MockReceipt => {
    const receipt = mock.dispatch(command);
    if (receipt.ok) {
      if (command.type === "create_task" && receipt.entityId) savedFiles.set(receipt.entityId, structuredClone(command.attachments ?? []));
      if (command.type === "edit_task") savedFiles.set(command.taskId, structuredClone(command.attachments));
      publish();
    }
    return receipt;
  };
  const port: Port = {
    mode: "demo",
    query<O extends QueryOperation>(operation: O, _parameters: QueryParameters[O]): Promise<QueryResults[O]> {
      const answers: QueryResults = { "capabilities.get": EVERYTHING, "project.snapshot": [projection()], "project.facts": [], "machine.facts": [] };
      return Promise.resolve(answers[operation]);
    },
    command(envelope) {
      const command = envelope.payload;
      const config = command.type === "save_surface" ? current.machines.find(m => m.id === command.machineId)?.config : undefined;
      const conflict = command.type === "save_surface" && config && command.revision !== (command.projectId ? config.projects[command.projectId]?.revisions?.[command.surface] ?? 0 : config.revisions[command.surface]);
      const { ok, message, entityId } = apply(command as WorkspaceCommand);
      const receipt: CommandReceipt = { requestId: envelope.requestId, ok, outcome: ok ? "recorded" : conflict ? "revision_conflict" : "unsupported", message, entityId };
      return Promise.resolve(receipt);
    },
    readTaskAttachments: async taskId => {
      const files = savedFiles.get(taskId) ?? [];
      if (files.some(f => typeof f === "string")) throw new Error("Saved attachment bytes are unavailable in this simulation");
      return structuredClone(files.filter(f => typeof f !== "string"));
    },
    subscribe(listener) { listeners.add(listener); return () => { listeners.delete(listener); }; },
  };
  return {
    port,
    initial: () => ({ state: current, version: String(version), capabilities: EVERYTHING }),
    state: (): WorkspaceState => current,
    /** The fixture clock plus the real time since the app opened, so stamps and elapsed figures move on. */
    clock: () => now + (Date.now() - opened),
    /** What the simulation does on its own. The owner's commands come through the port. */
    dispatch: (command: WorkspaceCommand, _options: { quiet?: boolean } = {}) => apply(command),
    reset() { mock = createPort(); savedFiles.clear(); publish(); },
  };
}
export type DemoBackend = ReturnType<typeof createDemoBackend>;
