import { isHiddenWorkerEvent } from "../shared/timeline-visibility.ts";
import type { WorkerHistory } from "../shared/worker-history.js";

export type WorkerEvent = WorkerHistory["assignments"][number]["attempts"][number]["events"][number];
export type TranscriptEntry = { event: WorkerEvent; updates: WorkerEvent[] };

const MESSAGE_MARKER = "[worker message] ";
const FINAL_RESPONSE_MARKER = "[final response] ";

function messageChunk(event: WorkerEvent): string | null {
  if (event.tool_kind != null || event.type !== "progress") return null;
  const marker = event.text.indexOf(MESSAGE_MARKER);
  return marker < 0 ? null : event.text.slice(marker + MESSAGE_MARKER.length);
}

function messageId(event: WorkerEvent): string | undefined {
  const chunk = messageChunk(event);
  return chunk === null ? undefined : event.message_id;
}

function entryMessageId(entry: TranscriptEntry): string | undefined {
  return messageId(entry.updates[0] ?? entry.event);
}

function groupingRef(event: WorkerEvent): string | undefined {
  // Streamed worker prose intentionally reuses the logical `message` ref.
  // Its durable message_id is the actual logical-message identity; grouping
  // by activity_ref would retain only the latest chunk. Legacy rows without
  // message_id are grouped by adjacent timeline position below.
  if (messageChunk(event) !== null) return undefined;
  const ref = event.activity_ref;
  // Runtime's tool-error observation names the same exact tool call with
  // this distinct prefix. Fold it into that call's row, retaining the row.
  return ref?.startsWith("tool-error:") ? `tool:${ref.slice("tool-error:".length)}` : ref;
}

/** One visible row per exact durable activity ref, at its first occurrence.
 * Unkeyed events stay separate: a matching tool name is not an identity. */
export function groupWorkerEvents(events: WorkerEvent[]): TranscriptEntry[] {
  const ordered = [...events].sort((a, b) => a.id - b.id);
  const rows: TranscriptEntry[] = [];
  const byRef = new Map<string, TranscriptEntry>();
  for (const event of ordered) {
    if (isHiddenWorkerEvent(event)) continue;
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
  // The engine's authoritative final-response text sometimes restates every
  // worker-message segment shown so far verbatim, glued together with no
  // separators (see orchestrating_runtime._flush_worker_message_block). Track
  // that same concatenation here so a final-response row never repeats what
  // is already on screen; it clears each time a final-response is emitted,
  // matching the server's own buffer reset.
  let shownMessageText = "";
  for (const row of rows) {
    const chunk = messageChunk(row.event);
    const previous = transcript[transcript.length - 1];
    const currentMessageId = entryMessageId(row);
    const previousMessageId = previous && entryMessageId(previous);
    const sameMessage = currentMessageId !== undefined && previousMessageId !== undefined
      ? currentMessageId === previousMessageId
      : currentMessageId === undefined && previousMessageId === undefined;
    if (chunk !== null && previous && previous.updates.every((update) => messageChunk(update) !== null) && sameMessage) {
      previous.event = { ...previous.event, text: previous.event.text + chunk };
      previous.updates.push(...row.updates);
      shownMessageText += chunk;
    } else if (chunk !== null) {
      transcript.push({ event: { ...row.event, text: chunk }, updates: row.updates });
      shownMessageText += chunk;
    } else if (row.event.activity_ref === "final-response") {
      const marker = row.event.text.indexOf(FINAL_RESPONSE_MARKER);
      const body = marker < 0 ? row.event.text : row.event.text.slice(marker + FINAL_RESPONSE_MARKER.length);
      const remainder = body.startsWith(shownMessageText.trim()) && shownMessageText.trim()
        ? body.slice(shownMessageText.trim().length).trim()
        : body;
      shownMessageText = "";
      if (remainder) transcript.push({ event: { ...row.event, text: marker < 0 ? remainder : `${row.event.text.slice(0, marker + FINAL_RESPONSE_MARKER.length)}${remainder}` }, updates: row.updates });
    } else {
      transcript.push(row);
    }
  }
  return transcript;
}
