import type { WorkerHistory } from "../shared/worker-history.js";

export type WorkerEvent = WorkerHistory["assignments"][number]["attempts"][number]["events"][number];
export type TranscriptEntry = { event: WorkerEvent; updates: WorkerEvent[] };

const MESSAGE_MARKER = "[worker message] ";

function internalEvent(event: WorkerEvent): boolean {
  const type = event.type.trim().toLowerCase();
  if (["maturity", "state", "maturity/state", "coordinator_decision"].includes(type)) return true;
  // `progress` is also the durable event type for worker_activity, including
  // real worker message chunks such as "maturity/state". Keep it here; the
  // server projection filters lifecycle rows by payload kind/origin.
  return false;
}

function messageChunk(event: WorkerEvent): string | null {
  if (event.tool_kind != null || event.type !== "progress") return null;
  const marker = event.text.indexOf(MESSAGE_MARKER);
  return marker < 0 ? null : event.text.slice(marker + MESSAGE_MARKER.length);
}

function groupingRef(event: WorkerEvent): string | undefined {
  // Streamed worker prose intentionally reuses the logical `message` ref.
  // It is a sequence of distinct durable events, so grouping it by ref would
  // retain only the latest chunk and leave fragments such as "evidence-backed"
  // on their own. Message chunks are grouped by their timeline position
  // below; tool updates continue to collapse by their exact activity ref.
  if (messageChunk(event) !== null) return undefined;
  const ref = event.activity_ref;
  // Runtime's tool-error observation names the same exact tool call with
  // this distinct prefix. Fold it into that call's row, retaining the row.
  return ref?.startsWith("tool-error:") ? `tool:${ref.slice("tool-error:".length)}` : ref;
}

/** One visible row per exact durable activity ref, at its first occurrence.
 * Unkeyed events stay separate: a matching tool name is not an identity. */
export function groupWorkerEvents(events: WorkerEvent[]): TranscriptEntry[] {
  const ordered = events.filter((event) => !internalEvent(event)).sort((a, b) => a.id - b.id);
  const rows: TranscriptEntry[] = [];
  const byRef = new Map<string, TranscriptEntry>();
  for (const event of ordered) {
    const ref = groupingRef(event);
    const existing = ref ? byRef.get(ref) : undefined;
    if (existing) {
      existing.updates.push(event);
      existing.event = {
        ...existing.event,
        ...Object.fromEntries(Object.entries(event).filter(([, value]) => value != null)),
        id: existing.event.id,
      };
      continue;
    }
    const row = { event: { ...event }, updates: [event] };
    rows.push(row);
    if (ref) byRef.set(ref, row);
  }
  const transcript: TranscriptEntry[] = [];
  for (const row of rows) {
    const chunk = messageChunk(row.event);
    const previous = transcript[transcript.length - 1];
    if (chunk !== null && previous && previous.updates.every((update) => messageChunk(update) !== null)) {
      previous.event = { ...previous.event, text: previous.event.text + chunk };
      previous.updates.push(...row.updates);
    } else if (chunk !== null) {
      transcript.push({ event: { ...row.event, text: chunk }, updates: row.updates });
    } else {
      transcript.push(row);
    }
  }
  // The heading already carries the delegation title and current assignment
  // state. These two bookkeeping rows contain no chat content.
  return transcript.filter(({ event }) => event.type !== "delegation_requested" && event.type !== "assignment_ready");
}
