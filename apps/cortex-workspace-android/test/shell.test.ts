import { describe, expect, it, vi } from "vitest";
import { createPhoneShell, createPhoneVoice, type PhoneNative } from "../src/shell";

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
});
