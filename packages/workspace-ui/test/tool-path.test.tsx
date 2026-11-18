// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { WsContext, type Ws } from "../src/ctx";
import { ToolSearchPath } from "../src/tool-path";
import { flush } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 6; i++) await flush(); };
const button = (label: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === label || b.getAttribute("aria-label") === label);
const input = (value: string) => { const e = document.querySelector<HTMLInputElement>('input[aria-label="Extra tool folder"]')!; e.value = value; e.dispatchEvent(new Event("input", { bubbles: true })); };
function mount(action: (op: string, p: Record<string, unknown>) => Promise<Record<string, unknown>>) {
  const ws = { machine: () => ({ id: "local", name: "This machine", kind: "local" }), store: { mode: "real", machineAction: action }, demo: undefined } as unknown as Ws;
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <WsContext.Provider value={ws}><ToolSearchPath /></WsContext.Provider>, root);
}
const at = Date.UTC(2026, 10, 18, 9, 30);

it("shows the detected path, re-reads the shell and edits extra folders through the service", async () => {
  const calls: Record<string, unknown>[] = [];
  let extra: string[] = [];
  let source = "fallback";
  mount(async (op, p) => {
    expect(op).toBe("maintenance");
    calls.push(p);
    if (p.action === "tool_path.reread") source = "shell";
    if (p.action === "tool_path.set_extra") {
      const dirs = p.directories as string[];
      if (dirs.some(d => !d.startsWith("/"))) throw new Error("Use an absolute folder, or one under ~.");
      extra = dirs;
    }
    return { source, shell: "bash", reason: source === "fallback" ? "timeout" : null, readAt: at, detected: ["/home/owner/.nvm/versions/node/v24.14.1/bin", "/usr/bin"], extra, rejected: [{ entry: "relative/bin", reason: "relative" }] };
  });
  await settle();
  expect(document.body.textContent).toContain("did not finish within a few seconds");
  expect(document.body.textContent).toContain("/home/owner/.nvm/versions/node/v24.14.1/bin");
  expect(document.body.textContent).toContain("relative/bin: not an absolute folder");
  button("Re-read from my shell")!.click(); await settle();
  expect(calls.map(c => c.action)).toEqual(["tool_path.read", "tool_path.reread"]);
  expect(document.body.textContent).toContain("Read from your shell (bash)");
  input("relative"); await settle();
  button("Add folder")!.click(); await settle();
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("absolute folder");
  expect(extra).toEqual([]);
  input("/opt/tools/bin"); await settle();
  button("Add folder")!.click(); await settle();
  expect(calls.at(-1)).toEqual({ action: "tool_path.set_extra", directories: ["/opt/tools/bin"] });
  expect(document.querySelector<HTMLInputElement>('input[aria-label="Extra tool folder"]')!.value).toBe("");
  expect(document.querySelector('[role="alert"]')).toBeNull();
  button("Remove /opt/tools/bin")!.click(); await settle();
  expect(calls.at(-1)).toEqual({ action: "tool_path.set_extra", directories: [] });
  expect(button("Remove /opt/tools/bin")).toBeUndefined();
});

it("says when the service cannot show the path instead of inventing one", async () => {
  mount(async () => { throw new Error("This action needs the local desktop service."); });
  await settle();
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("local desktop service only");
  expect(button("Re-read from my shell")!.disabled).toBe(true);
});
