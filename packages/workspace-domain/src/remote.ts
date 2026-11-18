/**
 * Remote access of the local machine: the machine service's own operations, asked over the same transport as every other
 * request (`transport.request(operation, message)`). Each one answers the whole new status. The service publishes a
 * `{ type: "refresh" }` transport event whenever this status changes.
 */
export type RemoteRelay = { url: string | null; source: "default" | "override" | "none" };
/** One live pairing offer. `qr` is the code's module matrix, one string of "0" and "1" per row; `expiresAt` is in ms. */
export type RemotePairing = { link: string; qr: string[]; expiresAt: number };
/** A device that asked to pair and waits for the owner's Allow or Deny on this computer. */
export type RemotePending = { requestId: string; deviceName: string; fingerprint: string; requestedAt: number };
export type RemoteDevice = { deviceId: string; name: string; pairedAt: number; lastSeenAt: number | null; connected: boolean };
export type RemoteStatus = {
  available: boolean; reason?: string; enabled: boolean; machineId: string;
  relay: RemoteRelay; pairing: null | RemotePairing; pending: RemotePending[]; devices: RemoteDevice[];
};
export type RemoteRequests = {
  "remote.status": Record<string, never>;
  "remote.enable": { enabled: boolean };
  "remote.pairing.start": Record<string, never>;
  "remote.pairing.cancel": Record<string, never>;
  "remote.pairing.decide": { requestId: string; allow: boolean };
  "remote.devices.revoke": { deviceId: string };
  "remote.relay.set": { url: string | null };
};
export type RemoteOperation = keyof RemoteRequests;
export const REMOTE_OPERATIONS: readonly RemoteOperation[] = ["remote.status", "remote.enable", "remote.pairing.start", "remote.pairing.cancel", "remote.pairing.decide", "remote.devices.revoke", "remote.relay.set"];

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isStr = (v: unknown): v is string => typeof v === "string";
/** The status as the service answered it, or an error naming what was malformed. Nothing absent is filled in. */
export function readRemoteStatus(raw: unknown): RemoteStatus {
  const bad = (what: string) => new Error(`Invalid remote access status: ${what}`);
  if (!raw || typeof raw !== "object") throw bad("not an object");
  const r = raw as Record<string, unknown>;
  if (typeof r.available !== "boolean" || typeof r.enabled !== "boolean" || !isStr(r.machineId)) throw bad("available, enabled or machineId");
  const relay = r.relay as Record<string, unknown> | undefined;
  if (!relay || !(relay.url === null || isStr(relay.url)) || !["default", "override", "none"].includes(relay.source as string)) throw bad("relay");
  const p = r.pairing as Record<string, unknown> | null | undefined;
  if (p !== null && (!p || !isStr(p.link) || !Array.isArray(p.qr) || !p.qr.every(row => isStr(row) && /^[01]+$/.test(row)) || !isNum(p.expiresAt))) throw bad("pairing");
  if (!Array.isArray(r.pending) || !r.pending.every(x => x && isStr(x.requestId) && isStr(x.deviceName) && isStr(x.fingerprint) && isNum(x.requestedAt))) throw bad("pending");
  if (!Array.isArray(r.devices) || !r.devices.every(x => x && isStr(x.deviceId) && isStr(x.name) && isNum(x.pairedAt) && (x.lastSeenAt === null || isNum(x.lastSeenAt)) && typeof x.connected === "boolean")) throw bad("devices");
  return {
    available: r.available, ...(isStr(r.reason) ? { reason: r.reason } : {}), enabled: r.enabled, machineId: r.machineId,
    relay: { url: relay.url as string | null, source: relay.source as RemoteRelay["source"] },
    pairing: p === null ? null : { link: p.link as string, qr: [...(p.qr as string[])], expiresAt: p.expiresAt as number },
    pending: (r.pending as RemotePending[]).map(x => ({ requestId: x.requestId, deviceName: x.deviceName, fingerprint: x.fingerprint, requestedAt: x.requestedAt })),
    devices: (r.devices as RemoteDevice[]).map(x => ({ deviceId: x.deviceId, name: x.name, pairedAt: x.pairedAt, lastSeenAt: x.lastSeenAt, connected: x.connected })),
  };
}
