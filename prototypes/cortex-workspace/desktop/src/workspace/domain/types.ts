import type { Activity, Check, CheckOutcome, ConfigFault, Connection, Delivery, PlanItem, SurfaceKey, Task, WorkspaceCommand as SharedCommand, WorkspaceState as SharedState } from "@cortex/workspace-domain";

export type * from "@cortex/workspace-domain";

/** What only the simulation can do: nothing here is an owner action, and none of it reaches the shared packages. */
export type SimulationCommand =
  | { type: "add_remote"; id: string; name: string }
  | { type: "set_connection"; machineId: string; connection: Connection; now?: number }
  | { type: "config_changed"; machineId: string; surface?: SurfaceKey }
  | { type: "set_config_fault"; machineId: string; fault: ConfigFault; on: boolean }
  | { type: "task_issue"; taskId: string; reason: string }
  | { type: "publish_result"; taskId: string; digest: string; checks: Check[]; summary?: string }
  | { type: "finish_check"; taskId: string; digest: string; checkIndex: number; outcome: CheckOutcome; detail: string; technicalReason?: string }
  | { type: "workspace_changed"; taskId: string; digest: string }
  | { type: "finish_resolution"; taskId: string; failed: boolean; now?: number }
  | { type: "plan_note"; sprintId: string; text: string; now?: number }
  | { type: "plan_question"; sprintId: string; question: string; options?: string[]; now?: number }
  | { type: "publish_plan"; sprintId: string; items: PlanItem[]; now?: number }
  | { type: "archive_due" | "purge_due"; now: number }
  | { type: "set_safety"; taskId: string; process: Task["process"]; unexpectedChanges: boolean }
  | { type: "simulate_delivery"; messageId: string; delivery: Delivery; now?: number; failure?: string }
  | { type: "ask_decision"; taskId: string; workerId: string | null; text: string; options: { label: string; sub?: string }[]; now?: number }
  | { type: "worker_activity"; workerId: string; text: string; entry?: Partial<Omit<Activity, "sequence" | "text" | "mock">>; now?: number }
  | { type: "finish_worker"; workerId: string; outcome: "completed" | "failed"; summary: string; checks: Check[]; now?: number }
  | { type: "settle_command"; workerId: string; sequence: number; state: "stopped" | "passed" | "failed"; exit: number | null }
  | { type: "set_conflict" | "advance_resolution"; taskId: string; now?: number }
export type WorkspaceCommand = SharedCommand | SimulationCommand;
/** The mock hands out its own ids. A real machine owns identity, so this counter stops here. */
export type WorkspaceState = SharedState & { nextId: number };
export type MockReceipt = { ok: boolean; mock: true; message: string; entityId?: string };
export interface WorkspacePort {
  readonly mock: true;
  snapshot(): WorkspaceState;
  dispatch(command: WorkspaceCommand): MockReceipt;
}
