import { isHiddenWorkerEvent } from "../shared/timeline-visibility.ts";
import type { WorkerHistory } from "../shared/worker-history.js";

export type WorkerEvent = WorkerHistory["assignments"][number]["attempts"][number]["events"][number];
export type TranscriptEntry = { event: WorkerEvent; updates: WorkerEvent[] };

const MESSAGE_MARKER = "[worker message] ";
const FINAL_RESPONSE_MARKER = "[final response] ";

function messageChunk(event: WorkerEvent): string | null {
  if (event.tool_kind != null || (event.type !== "progress" && event.type !== "worker_activity")) return null;
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

function finalResponseRemainder(body: string, segments: string[]): string {
  if (!segments.length) return body;
  let offset = 0;
  for (const segment of segments) {
    // The terminal response can trim the boundaries of each streamed prose
    // block independently. Chunks within a block have already been joined, so
    // whitespace inside the block is still compared exactly.
    const text = segment.trim();
    if (!text) continue;
    while (offset < body.length && /\s/.test(body[offset])) offset++;
    if (!body.startsWith(text, offset)) return body;
    offset += text.length;
  }
  return body.slice(offset).trimStart();
}

function sameProse(a: string, b: string): boolean {
  return a.trim().replace(/\s+/g, " ") === b.trim().replace(/\s+/g, " ");
}

/** Activity without the `worker_result` rows whose text is exactly the
 * summary the attempt's Result section already shows (whitespace aside).
 * Only that one durable event type is considered and only an exact match is
 * dropped, so any differing result text, the final response and other prose
 * stay; repeated identical rows (live, replay or refresh) all collapse into
 * the one Result. Stored history is never changed. */
export function withoutShownResult(events: WorkerEvent[], shownSummary: string | null | undefined): WorkerEvent[] {
  if (!shownSummary?.trim()) return events;
  return events.filter((event) => !(event.type === "worker_result" && sameProse(event.text, shownSummary)));
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
  // A terminal response may repeat the streamed prose after trimming each
  // message boundary. Keep complete logical blocks, rather than a single raw
  // concatenation, so replay and live updates compare the same text.
  const shownMessageSegments: string[] = [];
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
      shownMessageSegments[shownMessageSegments.length - 1] += chunk;
    } else if (chunk !== null) {
      transcript.push({ event: { ...row.event, text: chunk }, updates: row.updates });
      shownMessageSegments.push(chunk);
    } else if (row.event.activity_ref === "final-response") {
      const marker = row.event.text.indexOf(FINAL_RESPONSE_MARKER);
      const body = marker < 0 ? row.event.text : row.event.text.slice(marker + FINAL_RESPONSE_MARKER.length);
      const remainder = finalResponseRemainder(body, shownMessageSegments);
      shownMessageSegments.length = 0;
      if (remainder) transcript.push({ event: { ...row.event, text: marker < 0 ? remainder : `${row.event.text.slice(0, marker + FINAL_RESPONSE_MARKER.length)}${remainder}` }, updates: row.updates });
    } else {
      transcript.push(row);
    }
  }
  return transcript;
}
