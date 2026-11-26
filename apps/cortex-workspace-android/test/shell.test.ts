import { describe, expect, it, vi } from "vitest";
import { createPhoneNotifications, createPhoneShell, createPhoneVoice, parseTap, type PhoneNative } from "../src/shell";
import { workspaceCopy } from "../../../packages/workspace-ui/src/copy";
import { notificationWords } from "../../../packages/workspace-ui/src/phone-shell";

function fake(over: Partial<PhoneNative["scanner"]> = {}) {
  const listeners = new Map<string, (payload: unknown) => void>();
  const invoke = vi.fn(async (command: string, _args?: Record<string, unknown>): Promise<unknown> => {
    if (command === "phone_state") return { active: null, machines: [] };
    if (command === "phone_pair_begin") throw "invalid_link";
    return undefined;
  });
  const native: PhoneNative = {
    invoke: invoke as PhoneNative["invoke"],
    listen: async (event, listener) => { listeners.set(event, listener); return () => { listeners.delete(event); }; },
    scanner: { permission: async () => "granted", request: async () => "granted", scan: async () => "cortex-pair:abc", ...over },
  };
  return { native, invoke, emit: (event: string, payload: unknown) => listeners.get(event)?.(payload) };
}
const info = { version: "1.0.6", deviceName: "Pixel" };
const settle = () => new Promise(done => setTimeout(done, 0));

describe("phone shell", () => {
  it("turns a native refusal code into an Error the interface can match", async () => {
    const f = fake();
    await expect(createPhoneShell(f.native, info).pairBegin("nope")).rejects.toThrow("invalid_link");
  });
  it("scans only with camera permission, and backing out is not a failure", async () => {
    await expect(createPhoneShell(fake().native, info).scan()).resolves.toBe("cortex-pair:abc");
    await expect(createPhoneShell(fake({ permission: async () => "denied", request: async () => "denied" }).native, info).scan()).rejects.toThrow("camera_denied");
    await expect(createPhoneShell(fake({ scan: async () => { throw "Scan cancelled"; } }).native, info).scan()).resolves.toBeNull();
    await expect(createPhoneShell(fake({ scan: async () => { throw "no camera"; } }).native, info).scan()).rejects.toThrow("camera_unavailable");
  });
  it("binds every request and event of a transport to its machine", async () => {
    const f = fake();
    const shell = createPhoneShell(f.native, info);
    const a = shell.transport("a");
    expect(shell.transport("a")).toBe(a);
    const seen: unknown[] = [];
    a.subscribe(event => seen.push(event));
    await settle();
    f.emit("workspace-transport", { machineId: "b", event: { type: "refresh" } });
    f.emit("workspace-transport", { machineId: "a", event: { type: "refresh" } });
    expect(seen).toEqual([{ type: "refresh" }]);
    await a.request("service.status", {});
    expect(f.invoke).toHaveBeenCalledWith("workspace_request", { machineId: "a", operation: "service.status", message: {} });
  });
  it("delivers machine state updates until unsubscribed", async () => {
    const f = fake();
    const seen: unknown[] = [];
    const stop = createPhoneShell(f.native, info).subscribe(state => seen.push(state));
    await settle();
    f.emit("phone-state", { active: "a", machines: [] });
    f.emit("phone-state", "junk");
    stop();
    f.emit("phone-state", { active: null, machines: [] });
    expect(seen).toEqual([{ active: "a", machines: [] }]);
  });
  it("offers what the phone heard for inserting, and says so when it heard nothing or cannot listen", async () => {
    let answer: (value: unknown) => void = () => undefined, refuse: (why: unknown) => void = () => undefined;
    const invoke = vi.fn((_command: string) => new Promise<unknown>((resolve, reject) => { answer = resolve; refuse = reject; }));
    const voice = createPhoneVoice({ invoke: invoke as PhoneNative["invoke"] }, () => 5000);
    expect((await voice.start("a")).phase).toBe("recording");
    expect(invoke).toHaveBeenCalledWith("phone_listen");
    expect((await voice.read("a")).phase).toBe("recording");
    answer({ text: "  Проверь тесты  " }); await settle();
    expect(await voice.read("a")).toMatchObject({ phase: "ready", text: "Проверь тесты" });
    expect((await voice.cancel("a")).phase).toBe("cancelled");
    await expect(voice.read("a")).rejects.toThrow("This dictation is gone");
    await voice.start("b"); answer({ text: "" }); await settle();
    await expect(voice.read("b")).rejects.toThrow("Nothing was heard");
    await voice.start("c"); refuse("speech_unavailable"); await settle();
    await expect(voice.stop("c")).rejects.toThrow("not available on this phone");
  });
  it("opens a notification tap only for a machine id and a target of the closed shapes", () => {
    const target = { kind: "question", project: "p", task: "t", decision: "d" };
    expect(parseTap({ machineId: "m1", target })).toEqual({ machineId: "m1", target });
    expect(parseTap({ machineId: "m1", target: { kind: "sprint", project: "p", sprint: "s" } })?.target).toEqual({ kind: "sprint", project: "p", sprint: "s" });
    for (const bad of [
      null, "m1", { machineId: "m1" }, { target },
      { machineId: "", target }, { machineId: "a b", target }, { machineId: "x".repeat(257), target },
      { machineId: "m1", target, extra: 1 },
      { machineId: "m1", target: { ...target, url: "https://example.org" } },
      { machineId: "m1", target: { kind: "question", project: "p", task: "t" } },
      { machineId: "m1", target: JSON.stringify(target) },
      { machineId: "m1", target: { kind: "open", project: "p", task: "t" } },
    ]) expect(parseTap(bad), JSON.stringify(bad)).toBeNull();
  });
  it("reads taps one at a time until stopped, and turns notifications on and off natively", async () => {
    const taps = [{ machineId: "m1", target: { kind: "result", project: "p", task: "t" } }, { machineId: "m1", target: "junk" }, { machineId: "m2", target: { kind: "task", project: "p", task: "u" } }];
    let pending: ((value: unknown) => void) | null = null;
    const invoke = vi.fn((command: string, args?: Record<string, unknown>) => {
      if (command === "phone_notifications") return Promise.resolve({ enabled: args?.enabled, permission: "granted", batteryExempt: false, extra: "x" });
      if (command === "phone_notification_next") {
        const next = taps.shift();
        return next ? Promise.resolve(next) : new Promise(resolve => { pending = resolve; });
      }
      return Promise.resolve(undefined);
    });
    const waits: number[] = [];
    const notifications = createPhoneNotifications({ invoke: invoke as PhoneNative["invoke"] }, async ms => { waits.push(ms); });
    const words = notificationWords(key => workspaceCopy(key, "ru"));
    expect(words.ongoingTitle).toBe("На связи с компьютерами");
    await expect(notifications.set(true, words)).resolves.toEqual({ enabled: true, permission: "granted", batteryExempt: false });
    expect(invoke).toHaveBeenCalledWith("phone_notifications", { enabled: true, words });
    const seen: unknown[] = [];
    const stop = notifications.taps(tap => seen.push(tap));
    for (let i = 0; i < 10; i++) await settle();
    // The malformed tap leads nowhere and the next read waits a moment first.
    expect(seen).toEqual([{ machineId: "m1", target: { kind: "result", project: "p", task: "t" } }, { machineId: "m2", target: { kind: "task", project: "p", task: "u" } }]);
    expect(waits).toEqual([250]);
    stop();
    pending!({ machineId: "m1", target: { kind: "task", project: "p", task: "late" } });
    await settle();
    expect(seen).toHaveLength(2);
    expect(createPhoneShell(fake().native, info).notifications).toBeDefined();
  });
});
