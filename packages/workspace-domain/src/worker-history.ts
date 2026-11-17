/** Framework-free retained activity shape. RPC validation belongs to the transport. */
export type WorkerEvent = {
  id: number;
  type: string;
  createdAt: string;
  text: string;
  activity_ref?: string;
  message_id?: string;
  input?: unknown;
  output?: unknown;
  status?: string | null;
  error?: unknown;
  diff?: string | null;
  tool_kind?: string | null;
  title?: string | null;
  locations?: unknown;
  content?: unknown;
  raw_input?: unknown;
  raw_output?: unknown;
  payload_offloaded?: boolean;
  large_output_id?: string;
  sha256?: string;
  byte_count?: number;
};
export type TranscriptEntry = { event: WorkerEvent; updates: WorkerEvent[] };
