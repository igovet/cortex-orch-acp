import { createDesktopTransport } from "../../cortex-workspace-desktop/src/transport";
import { parseTarget, type NotificationTarget } from "../../../packages/workspace-ui/src/notification-target";
import type { PhoneNotifications, PhoneNotificationState, PhoneShell, PhoneState } from "../../../packages/workspace-ui/src/phone-shell";
import type { RealTransport } from "../../../packages/workspace-ui/src/port/real";
import type { VoicePage, VoicePort } from "../../../packages/workspace-ui/src/voice-client";

/** What the native side offers. Tests pass their own; the app passes Tauri's invoke, listen and the scanner plugin. */
export type PhoneNative = {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen(event: string, listener: (payload: unknown) => void): Promise<() => void>;
  scanner: {
    permission(): Promise<string>;
    request(): Promise<string>;
    /** The scanned text. Rejects when the owner backs out or the camera cannot open. */
    scan(): Promise<string>;
  };
};
export type PhoneInfo = { version: string; deviceName: string };

/** A native refusal is a short code; the interface matches on it, so it becomes the message of an Error. */
const coded = (error: unknown) => new Error(typeof error === "string" ? error : error instanceof Error ? error.message : "failed");
const isState = (value: unknown): value is PhoneState => !!value && typeof value === "object" && Array.isArray((value as PhoneState).machines);

/**
 * Dictation through the phone's own speech recognition. The system's recognizer screen listens and hands back text; the app
 * records nothing. One run is one recognizer screen: it cannot be paused, so Stop just waits for it. The text is only offered
 * for inserting, never sent.
 */
export function createPhoneVoice(native: Pick<PhoneNative, "invoke">, now: () => number = Date.now): VoicePort {
  type Run = { started: number; text?: string; failed?: boolean };
  const runs = new Map<string, Run>();
  const page = (sessionId: string, run: Run): VoicePage => ({ sessionId, progress: 0, error: "", seconds: Math.min(100, Math.floor((now() - run.started) / 1000)),
    phase: run.text === undefined ? "recording" : "ready", text: run.text ?? "" });
  const read = async (sessionId: string): Promise<VoicePage> => {
    const run = runs.get(sessionId);
    if (!run) throw new Error("This dictation is gone. Start again; nothing was sent.");
    if (run.failed) { runs.delete(sessionId); throw new Error("Speech recognition is not available on this phone. Nothing was sent."); }
    // Backing out of the recognizer, or saying nothing it could read, leaves nothing to insert.
    if (run.text === "") { runs.delete(sessionId); throw new Error("Nothing was heard. Nothing was sent."); }
    return page(sessionId, run);
  };
  return {
    badge: "Phone speech recognition · not sent",
    async start(requestId) {
      const run: Run = { started: now() };
      runs.set(requestId, run);
      void native.invoke<{ text?: unknown }>("phone_listen").then(heard => { run.text = typeof heard?.text === "string" ? heard.text.trim() : ""; }, () => { run.failed = true; });
      return page(requestId, run);
    },
    read, stop: read,
    async cancel(sessionId) { runs.delete(sessionId); return { sessionId, phase: "cancelled", progress: 0, text: "", seconds: 0, error: "" }; },
  };
}

const machineId = (v: unknown): v is string => typeof v === "string" && v.length > 0 && v.length <= 256 && !/[\s\p{Cc}]/u.test(v);
/** A tap the native side handed back, checked again: a machine id and a target of the closed shapes, or nothing. */
export function parseTap(raw: unknown): { machineId: string; target: NotificationTarget } | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  if (Object.keys(r).sort().join(",") !== "machineId,target" || !machineId(r.machineId)) return null;
  const target = parseTarget(r.target);
  return target ? { machineId: r.machineId, target } : null;
}

/**
 * The phone's own notifications through the native bridge. Taps are read one at a time: the native side answers the next one when it
 * arrives, so nothing is polled. A refused read waits before it asks again.
 */
export function createPhoneNotifications(native: Pick<PhoneNative, "invoke">, wait = (ms: number) => new Promise<void>(done => setTimeout(done, ms))): PhoneNotifications {
  return {
    set: async (enabled, words) => {
      const reply = await native.invoke<Partial<PhoneNotificationState>>("phone_notifications", { enabled, words }).catch(error => { throw coded(error); });
      return { enabled: reply?.enabled === true, permission: reply?.permission === "granted" ? "granted" : "denied", batteryExempt: reply?.batteryExempt === true };
    },
    battery: () => native.invoke<void>("phone_battery_settings").catch(error => { throw coded(error); }),
    taps(listener) {
      let ended = false;
      void (async () => {
        while (!ended) {
          try {
            const tap = parseTap(await native.invoke<unknown>("phone_notification_next"));
            if (!ended && tap) listener(tap);
            // Nothing that leads anywhere: never ask again in a tight loop.
            else if (!ended) await wait(250);
          } catch { if (!ended) await wait(2000); }
        }
      })();
      return () => { ended = true; };
    },
  };
}

/** The phone's native bridge as the interface's PhoneShell. No address, key or link is kept in JavaScript. */
export function createPhoneShell(native: PhoneNative, info: PhoneInfo): PhoneShell {
  const call = <T>(command: string, args?: Record<string, unknown>) => native.invoke<T>(command, args).catch(error => { throw coded(error); });
  const transports = new Map<string, RealTransport>();
  return {
    voice: createPhoneVoice(native),
    notifications: createPhoneNotifications(native),
    version: info.version,
    deviceName: info.deviceName,
    state: () => call<PhoneState>("phone_state"),
    subscribe(listener) {
      let stop: (() => void) | undefined, ended = false;
      void native.listen("phone-state", payload => { if (!ended && isState(payload)) listener(payload); })
        .then(off => { if (ended) off(); else stop = off; }, () => undefined);
      return () => { ended = true; stop?.(); };
    },
    async scan() {
      let permission: string;
      try {
        permission = await native.scanner.permission();
        if (permission !== "granted") permission = await native.scanner.request();
      } catch { throw new Error("camera_unavailable"); }
      if (permission !== "granted") throw new Error("camera_denied");
      try { return await native.scanner.scan(); }
      catch (error) {
        // Backing out of the scanner is not a failure.
        if (/cancel/i.test(coded(error).message)) return null;
        throw new Error("camera_unavailable");
      }
    },
    pairBegin: link => call("phone_pair_begin", { link }),
    pairFinish: name => call("phone_pair_finish", { name }),
    pairCancel: () => call("phone_pair_cancel"),
    select: machineId => call("phone_select", { machineId }),
    rename: (machineId, name) => call("phone_rename", { machineId, name }),
    forget: machineId => call("phone_forget", { machineId }),
    retry: machineId => call("phone_retry", { machineId }),
    transport(machineId) {
      let held = transports.get(machineId);
      if (!held) {
        // One transport per machine for the app's life: its events are told apart by the machine they name.
        held = createDesktopTransport({
          invoke: body => native.invoke("workspace_request", { machineId, operation: body.operation, message: body.message ?? {} }),
          listen: listener => native.listen("workspace-transport", payload => {
            const p = payload as { machineId?: unknown; event?: unknown } | null;
            if (p && p.machineId === machineId) listener(p.event);
          }),
        }).transport;
        transports.set(machineId, held);
      }
      return held;
    },
  };
}
