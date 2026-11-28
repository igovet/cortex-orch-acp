import { workspaceCopy, type CopyKey } from "./copy";
import type { RealTransport } from "./port/real";
import type { VoicePort } from "./voice-client";

/**
 * The contract between the shared interface and the Android app. The native side implements `PhoneShell` exactly; the
 * interface never reaches the network, the camera or the key store itself.
 */
export type PhoneMachine = { id: string; name: string; state: "connecting" | "connected" | "offline" | "rejected" | "update_needed";
  path: "direct" | "relay" | "unknown" | null; lastConnectedAt: number | null; pairedAt?: number };
export type PhoneState = { active: string | null; machines: PhoneMachine[] };
export interface PhoneShell {
  state(): Promise<PhoneState>;
  subscribe(listener: (state: PhoneState) => void): () => void;
  /** Opens the camera scanner. Resolves the QR text, or null when the owner backs out. Rejects with Error("camera_denied") or Error("camera_unavailable"). */
  scan(): Promise<string | null>;
  /** Starts pairing from a scanned or pasted link. Rejects with Error("invalid_link" | "unsupported_pairing_link_version" | "remote_incompatible_version" | "unreachable" | "expired" | "refused"). */
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
  /** The phone's own notifications of every paired machine's owner moments, where the phone has them. */
  notifications?: PhoneNotifications;
  version: string;
  deviceName: string;
}
/** The words the phone hands to Android, in the interface's language: the ongoing notification, each channel's name and note, and
 *  the title of each kind of moment. */
export type PhoneNotificationWords = {
  ongoingTitle: string; ongoingText: string;
  channels: Record<"ongoing" | "answer" | "done" | "failure", { name: string; note: string }>;
  kinds: Record<"question" | "stop" | "access" | "planning" | "result" | "failed" | "paused" | "stalled", string>;
};
/** Where the phone stands on its own notifications after turning them on or off. */
export type PhoneNotificationState = { enabled: boolean; permission: "granted" | "denied"; batteryExempt: boolean };
/** A tap on one of the phone's notifications: the machine and the place to open there (checked again with `parseTarget`). */
export type PhoneTap = { machineId: string; target: unknown };
/**
 * The phone's own notifications. No push service: while they are on, the phone keeps its connections in the background (a
 * foreground service with one quiet notification) and raises a notification itself when a machine reports a question, a result
 * or a failure.
 */
export interface PhoneNotifications {
  /** Turns them on (asking Android for the notification permission when needed) or off. */
  set(enabled: boolean, words: PhoneNotificationWords): Promise<PhoneNotificationState>;
  /** Opens Android's battery optimisation list, where the owner can let the app keep its connection while the phone sleeps. */
  battery(): Promise<void>;
  /** Taps on the phone's notifications. Returns the way to stop listening. */
  taps(listener: (tap: PhoneTap) => void): () => void;
}
/** What a pairing link starts with: the same text the computer's QR code holds. */
export const PAIR_LINK_PREFIX = "cortex-pair:";
/** The notification words in the interface's language, for Android (which shows them outside the app). */
export function notificationWords(t: (key: CopyKey) => string = key => workspaceCopy(key)): PhoneNotificationWords {
  const channel = (name: CopyKey, note: CopyKey) => ({ name: t(name), note: t(note) });
  return {
    ongoingTitle: t("phoneNotifyOngoingTitle"), ongoingText: t("phoneNotifyOngoingText"),
    channels: { ongoing: channel("phoneNotifyChannelOngoing", "phoneNotifyChannelOngoingNote"), answer: channel("phoneNotifyChannelAnswer", "phoneNotifyChannelAnswerNote"),
      done: channel("phoneNotifyChannelDone", "phoneNotifyChannelDoneNote"), failure: channel("phoneNotifyChannelFailure", "phoneNotifyChannelFailureNote") },
    kinds: { question: t("phoneNotifyQuestion"), stop: t("phoneNotifyStop"), access: t("phoneNotifyAccess"), planning: t("phoneNotifyPlanning"),
      result: t("phoneNotifyResult"), failed: t("phoneNotifyFailed"), paused: t("phoneNotifyPaused"), stalled: t("phoneNotifyStalled") },
  };
}
