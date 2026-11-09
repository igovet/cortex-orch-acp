import { applyCommand, createInitialWorkspace, DomainError, type WorkspacePort, type WorkspaceState } from "../domain";

/** In-memory simulation only. No ACP, network, engine, filesystem or Git operations. */
export function createMockWorkspace(initial: WorkspaceState = createInitialWorkspace()): WorkspacePort {
  let state = structuredClone(initial);
  return {
    mock: true,
    snapshot: () => structuredClone(state),
    dispatch(command) {
      const candidate = structuredClone(state);
      try {
        const entityId = applyCommand(candidate, command);
        state = candidate;
        return { ok: true, mock: true, message: "Mock " + command.type + ": changed memory only", entityId };
      } catch (error) {
        if (!(error instanceof DomainError)) throw error;
        return { ok: false, mock: true, message: "Mock action blocked: " + error.message };
      }
    },
  };
}
