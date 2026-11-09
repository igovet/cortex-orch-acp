import { describe, it, expect, vi } from "vitest";
import { createWindowChrome } from "../src/window";

const fake = (maximized = false) => {
  const state = { maximized, resized: () => {}, closing: (_event: { preventDefault(): void }) => {} };
  return {
    state,
    minimize: vi.fn(async () => {}), toggleMaximize: vi.fn(async () => {}), close: vi.fn(async () => {}), startDragging: vi.fn(async () => {}), destroy: vi.fn(async () => {}), finish: vi.fn(async () => {}),
    isMaximized: vi.fn(async () => state.maximized),
    onResized: vi.fn(async (handler: () => void) => { state.resized = handler; return () => {}; }),
    onCloseRequested: vi.fn(async (handler: (event: { preventDefault(): void }) => void) => { state.closing = handler; return () => {}; }),
  };
};
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

describe("window chrome", () => {
  it("each control reaches the one native call it names", () => {
    const native = fake();
    const chrome = createWindowChrome(native, native.finish);
    chrome.minimize(); chrome.toggleMaximize(); chrome.close(); chrome.startDragging();
    for (const call of [native.minimize, native.toggleMaximize, native.close, native.startDragging]) expect(call).toHaveBeenCalledOnce();
  });
  it("follows the real window state, including a change the app did not make", async () => {
    const native = fake(true);
    const chrome = createWindowChrome(native, native.finish);
    await tick();
    expect(chrome.maximized()).toBe(true);
    native.state.maximized = false;
    native.state.resized();
    await tick();
    expect(chrome.maximized()).toBe(false);
  });
  it("guards every close before entering the acknowledged native shutdown route", async () => {
    const native = fake(), chrome = createWindowChrome(native, native.finish), event = { preventDefault: vi.fn() };
    native.state.closing(event);
    expect(event.preventDefault).toHaveBeenCalledOnce();
    await tick();
    expect(native.finish).toHaveBeenCalledOnce();
    let unsaved = true;
    chrome.guardClose(() => unsaved);
    native.state.closing(event);
    expect(event.preventDefault).toHaveBeenCalledTimes(2);
    expect(native.finish).toHaveBeenCalledOnce();
    unsaved = false;
    native.state.closing(event);
    expect(event.preventDefault).toHaveBeenCalledTimes(3);
    chrome.closeNow();
    await tick();
    expect(native.finish).toHaveBeenCalledTimes(2);
    expect(native.destroy).not.toHaveBeenCalled();
  });
  it("coalesces repeated approved closes and can retry a refused shutdown", async () => {
    const native = fake();
    let reject!: (error: Error) => void;
    native.finish.mockImplementationOnce(() => new Promise<void>((_, fail) => { reject = fail; }));
    const chrome = createWindowChrome(native, native.finish);
    chrome.closeNow(); chrome.closeNow();
    expect(native.finish).toHaveBeenCalledOnce();
    reject(new Error("pending write failed"));
    await tick();
    expect(chrome.closeError()).toContain("could not close safely");
    chrome.closeNow();
    expect(chrome.closeError()).toBeNull();
    expect(native.finish).toHaveBeenCalledTimes(2);
    expect(native.destroy).not.toHaveBeenCalled();
  });
  it("a refused native call leaves the app running", async () => {
    const native = fake();
    native.close.mockRejectedValue(new Error("not allowed"));
    createWindowChrome(native, native.finish).close();
    await tick();
    expect(native.close).toHaveBeenCalledOnce();
  });
});
