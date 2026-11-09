import type { Capabilities, WireTypes } from "./index.js";
export function validate<K extends keyof WireTypes>(type: K, value: unknown): WireTypes[K];
export function decodeResponse<K extends keyof WireTypes>(type: K, value: unknown): { value: WireTypes[K]; warnings: string[] };
export function capabilities(buildIdentity: string, generation: string, operations: string[], acpCapabilities?: Record<string, unknown>): Capabilities;
export function negotiate(local: Capabilities, remote: Capabilities): {
  protocol: { major: number; minor: number }; operations: string[];
  limits: Record<string, number>; mappingVersion: string | null;
};
export function encodeFrame(value: unknown): Uint8Array;
export function decodeFrame(frame: Uint8Array): unknown;
