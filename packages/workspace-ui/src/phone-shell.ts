import type { RealTransport } from "./port/real";
import type { VoicePort } from "./voice-client";

/**
 * The contract between the shared interface and the Android app. The native side implements `PhoneShell` exactly; the
 * interface never reaches the network, the camera or the key store itself.
 */
export type PhoneMachine = { id: string; name: string; state: "connecting" | "connected" | "offline";
  path: "direct" | "relay" | "unknown" | null; lastConnectedAt: number | null };
export type PhoneState = { active: string | null; machines: PhoneMachine[] };
export interface PhoneShell {
  state(): Promise<PhoneState>;
  subscribe(listener: (state: PhoneState) => void): () => void;
  /** Opens the camera scanner. Resolves the QR text, or null when the owner backs out. Rejects with Error("camera_denied") or Error("camera_unavailable"). */
  scan(): Promise<string | null>;
  /** Starts pairing from a scanned or pasted link. Rejects with Error("invalid_link" | "unreachable" | "expired" | "refused"). */
  pairBegin(link: string): Promise<{ machineName: string; fingerprint: string }>;
  /** Resolves when the computer allows it. Rejects with Error("denied" | "expired" | "cancelled" | "unreachable"). */
  pairFinish(name: string): Promise<{ machineId: string }>;
  pairCancel(): Promise<void>;
  select(machineId: string): Promise<void>;
  rename(machineId: string, name: string): Promise<void>;
  forget(machineId: string): Promise<void>;
  /** The transport bound to one machine; the phone app creates the real port from it. */
  transport(machineId: string): RealTransport;
  retry(machineId: string): Promise<void>;
  /** Present only when the phone itself can choose a relay. Each machine names its own relay in its pairing link. */
  relay?: { read(): Promise<{ url: string | null; isDefault: boolean }>; write(url: string | null): Promise<void> };
  appLock?: { read(): Promise<{ enabled: boolean; available: boolean }>; write(enabled: boolean): Promise<void> };
  /** Dictation through the phone's own speech recognition, where the phone has one. Text is only ever inserted for review. */
  voice?: VoicePort;
  version: string;
  deviceName: string;
}
/** What a pairing link starts with: the same text the computer's QR code holds. */
export const PAIR_LINK_PREFIX = "cortex-pair:";
