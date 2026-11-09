import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Workspace } from "../src";
import { emptyWorkspace, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { flush, projection, scriptedPort } from "./support";
import type { TerminalPort } from "../src/terminal-client";
const xterm = vi.hoisted(() => ({ data: (_data: string) => {}, writes: [] as Uint8Array[], focus: vi.fn(), dispose: vi.fn() }));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  options: any = {}; cols = 80; rows = 24;
  open() {} reset() {} loadAddon() {} focus() { xterm.focus(); }
  write(bytes: Uint8Array, done: () => void) { xterm.writes.push(bytes); done(); }
  onData(fn: (data: string) => void) { xterm.data = fn; }
  dispose() { xterm.dispose(); }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
let dispose: (() => void) | undefined;
beforeEach(() => {
  localStorage.clear(); sessionStorage.clear();
  xterm.writes = []; xterm.focus.mockClear(); xterm.dispose.mockClear();
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
});
afterEach(() => { dispose?.(); document.body.innerHTML = ""; vi.unstubAllGlobals(); });
function client(): TerminalPort {
  return {
    open: vi.fn(async () => ({sessionId:"s",data:[65],next:1,truncated:false,exited:false})),
    read: vi.fn(async () => ({sessionId:"s",data:[],next:1,truncated:false,exited:false})),
    write: vi.fn(async (_p: string, _s: string, data: string) => ({written:new TextEncoder().encode(data).length})),
    resize: vi.fn(async () => ({})), close: vi.fn(async () => ({})),
  };
}
async function mount(terminal: TerminalPort) {
  history.replaceState(null,"",`/m/${LOCAL_MACHINE_ID}/p/p/board`);
  const backend = scriptedPort();
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={backend.port} terminal={terminal} />, root);
  const state = emptyWorkspace();
  state.projects.push({id:"p",machineId:LOCAL_MACHINE_ID,name:"Project",repository:"/project",openBranch:"main",defaultBranch:"main"});
  // Publish after the initial coherent read, as the real port does on refresh.
  await flush(); await flush();
  backend.emit(projection("1",state));
  await flush(); await flush();
}
const shortcut = () => document.dispatchEvent(new KeyboardEvent("keydown", {key:"j",ctrlKey:true,bubbles:true,cancelable:true}));
it("opens on Ctrl+J, preserves input order, hides without closing, and explicitly closes", async () => {
  const terminal = client(); await mount(terminal);
  expect(terminal.open).not.toHaveBeenCalled();
  shortcut(); await flush(); await flush();
  expect(terminal.open).toHaveBeenCalledWith("p",80,24);
  xterm.data("first"); xterm.data("second"); await flush();
  expect(vi.mocked(terminal.write).mock.calls.map(c => c[2])).toEqual(["first","second"]);
  shortcut(); await flush();
  expect(terminal.close).not.toHaveBeenCalled();
  shortcut(); await flush();
  expect(terminal.open).toHaveBeenCalledTimes(1);
  document.querySelector<HTMLButtonElement>('[aria-label="Close shell"]')!.click();
  await flush();
  expect(terminal.close).toHaveBeenCalledWith("p","s");
  expect(document.querySelector(".terminal-status")?.textContent).toBe("Closed");
});
it("ignores a stale input refusal after an explicit close", async () => {
  const terminal = client();
  let reject!: (cause: Error) => void;
  vi.mocked(terminal.write).mockImplementation(() => new Promise((_resolve, fail) => { reject = fail; }));
  await mount(terminal); shortcut(); await flush(); await flush();
  xterm.data("once");
  document.querySelector<HTMLButtonElement>('[aria-label="Close shell"]')!.click();
  await flush();
  reject(new Error("stale refusal")); await flush();
  expect(document.querySelector(".terminal-status")?.textContent).toBe("Closed");
  expect(document.querySelector(".terminal-error")).toBeNull();
});
it("stops on uncertain input, drops unsent queue, and reconnects output without replay", async () => {
  const terminal = client();
  vi.mocked(terminal.write).mockRejectedValue(new Error("uncertain input"));
  await mount(terminal); shortcut(); await flush(); await flush();
  xterm.data("once"); xterm.data("queued"); await flush();
  expect(terminal.write).toHaveBeenCalledTimes(1);
  expect(document.querySelector(".terminal-error")?.textContent).toContain("uncertain input");
  [...document.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent==="Reconnect terminal")!.click();
  await flush(); await flush();
  expect(terminal.open).toHaveBeenCalledTimes(2);
  expect(terminal.write).toHaveBeenCalledTimes(1);
  dispose?.(); dispose = undefined;
  expect(xterm.dispose).toHaveBeenCalled();
  expect(terminal.close).not.toHaveBeenCalled();
});
