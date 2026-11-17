import { validate } from "@cortex/workspace-contract";
import type { NativeTaskDelta } from "@cortex/workspace-domain";

const record = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
export function nativeVersion(v: unknown): v is string {
  return typeof v === "string" && /^(0|[1-9][0-9]*)$/.test(v) && v.length <= 19 && BigInt(v) <= 9223372036854775807n;
}
/** The Rust bridge and JS transport independently check selected DTOs; raw runtime events are never accepted. */
export function nativeDelta(raw: unknown): NativeTaskDelta | null {
  try {
    if (!record(raw) || typeof raw.projectId !== "string" || !record(raw.batch)) return null;
    validate("Identity", raw.projectId);
    const b = raw.batch;
    if (typeof b.generation !== "string" || !b.generation || !record(b.streamKey) || b.streamKey.scope !== "project" || b.streamKey.project_id !== raw.projectId
      || !nativeVersion(b.fromSequence) || b.fromSequence === "0" || !nativeVersion(b.throughSequence) || BigInt(b.throughSequence) < BigInt(b.fromSequence)
      || !Array.isArray(b.changes) || !b.changes.length || new TextEncoder().encode(JSON.stringify(b)).length > 65536) return null;
    for (const c of b.changes) {
      if (!record(c) || !["task", "project"].includes(String(c.kind)) || typeof c.id !== "string" || !nativeVersion(c.baseVersion) || !nativeVersion(c.newVersion) || BigInt(c.newVersion) <= BigInt(c.baseVersion) || !record(c.operation)) return null;
      validate("Identity", c.id);
      if (c.kind === "project" && (c.id !== raw.projectId || c.operation.type !== "invalidate")) return null;
      if (c.operation.type === "upsert") {
        const entity = c.operation.data;
        if (c.kind !== "task" || !record(entity) || entity.kind !== "task_projection" || !record(entity.data)) return null;
        const task = validate("Task", entity.data);
        if (task.taskId !== c.id || task.version !== c.newVersion) return null;
      } else if (!["remove", "invalidate"].includes(String(c.operation.type))) return null;
    }
    return raw as unknown as NativeTaskDelta;
  } catch { return null; }
}
