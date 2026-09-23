import type { Readable } from "node:stream";
export type ReplayManifest = { count: number; bytes: number; sha256: string };
export const REPLAY_LIMITS: { items: number; bytes: number; frameBytes: number };
export class ReplaySpool {
  constructor(limits?: typeof REPLAY_LIMITS);
  count: number;
  bytes: number;
  append(frame: unknown): void;
  manifest(): ReplayManifest;
  frames(): Generator<unknown>;
  close(): void;
}
export function readFrames(stream: Readable, maxBytes?: number): AsyncGenerator<any>;
