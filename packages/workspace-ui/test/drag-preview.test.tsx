// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Message } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

const BOARD = `/m/${LOCAL_MACHINE_ID}/p/p/board`;
const TASK = `/m/${LOCAL_MACHINE_ID}/t/t`;
const WEBKIT = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/60.5 Safari/605.1.15";
const queued = (id: string): Message => ({ id, taskId: "t", workerId: null, text: `Queued ${id}`,
  mode: "queue", delivery: "queued", at: 1, deliveredAt: null, failure: null });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => {
  document.querySelector('[draggable="true"]')?.dispatchEvent(new Event("dragend"));
  window.dispatchEvent(new Event("pagehide"));
  dispose?.(); dispose = undefined;
  document.body.replaceChildren();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

async function mount(path: string) {
  const state = withTask(emptyWorkspace(), "Drag fixture");
  state.machines[0].connection = "connected";
  Object.assign(state.tasks[0], { status: "working", process: "live", executionId: "e" });
  state.messages = [queued("first"), queued("second")];
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return { root, p };
}

function platform(ua: string, ratio: number, tauri: boolean) {
  vi.spyOn(window.navigator, "userAgent", "get").mockReturnValue(ua);
  vi.stubGlobal("devicePixelRatio", ratio);
  if (tauri) vi.stubGlobal("__TAURI_INTERNALS__", { invoke: vi.fn() });
}

function transferFixture() {
  const data = new Map<string, string>();
  return {
    effectAllowed: "uninitialized",
    setData: vi.fn((type: string, value: string) => data.set(type, value)),
    getData: (type: string) => data.get(type) ?? "",
    get types() { return [...data.keys()]; },
    setDragImage: vi.fn((node: HTMLElement, _x: number, _y: number) => { expect(node.isConnected).toBe(true); }),
  };
}
type Transfer = ReturnType<typeof transferFixture>;
function drag(node: Element, type: string, transfer: Transfer, clientX: number, clientY: number) {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientX, clientY });
  Object.defineProperty(event, "dataTransfer", { value: transfer });
  node.dispatchEvent(event);
  return event;
}

const box = { left: 30.25, top: 40.5, width: 248.5, height: 48.75 };
function start(source: HTMLElement, from: Element = source) {
  expect(source.draggable).toBe(true);
  vi.spyOn(source, "getBoundingClientRect").mockReturnValue(DOMRect.fromRect({ x: box.left, y: box.top, width: box.width, height: box.height }));
  const transfer = transferFixture();
  drag(from, "dragstart", transfer, 53, 55);
  return transfer;
}
const by = (selector: string) => document.querySelector<HTMLElement>(selector);

const sources = [
  { name: "queued message", route: TASK, selector: '[data-q="first"]', child: ".grip", mime: "text/plain", payload: "first", className: "qrow" },
  { name: "workspace tab", route: BOARD, selector: ".ws-tab", child: ".ws-tab-name", mime: "application/x-cortex-tab", payload: "1", className: "ws-tab" },
];

describe.each([1.25, 1.5, 2, 4])("Linux desktop drag preview at DPR %s", ratio => {
  it.each(sources)("keeps the $name at the same CSS size and pointer hotspot", async ({ route, selector, child, mime, payload, className }) => {
    platform(WEBKIT, ratio, true);
    const { root } = await mount(route);
    const source = root.querySelector<HTMLElement>(selector)!;
    const transfer = start(source, source.querySelector(child)!);
    const layer = by(".ws-drag-overlay")!, ghost = by(".ws-drag-preview")!;
    expect(transfer.setData).toHaveBeenCalledExactlyOnceWith(mime, payload);
    expect(transfer.setDragImage).toHaveBeenCalledExactlyOnceWith(by(".ws-drag-blank"), 0, 0);
    expect(by(".ws-drag-blank")!.childNodes).toHaveLength(0);
    expect(ghost.classList.contains(className)).toBe(true);
    expect(ghost.textContent).toBe(source.textContent);
    expect(ghost.style.width).toBe("248.5px");
    expect(ghost.style.height).toBe("48.75px");
    expect(ghost.style.left).toBe("30.25px");
    expect(ghost.style.top).toBe("40.5px");
    expect(ghost.querySelector("[id]")).toBeNull();
    expect(layer.inert).toBe(true);
    expect(layer.getAttribute("aria-hidden")).toBe("true");
    expect(root.querySelectorAll(selector)).toHaveLength(1);

    const over = drag(root, "dragover", transfer, 400, 150);
    expect(over.defaultPrevented).toBe(false); // Feedback must not turn an arbitrary surface into a drop target.
    expect(ghost.style.left).toBe("377.25px");
    expect(ghost.style.top).toBe("135.5px");
    drag(source, "dragenter", transfer, 300, 100);
    expect(ghost.style.left).toBe("277.25px");
    drag(root, "dragleave", transfer, 300, 100); // WebKit enters the new target before leaving the old one.
    expect(layer.style.visibility).toBe("");
    drag(source, "dragleave", transfer, 300, 100);
    expect(layer.style.visibility).toBe("hidden");
    drag(root, "dragenter", transfer, 400, 150);
    expect(layer.style.visibility).toBe("");
    drag(source, "dragend", transfer, 400, 150);
    expect(by(".ws-drag-layer")).toBeNull();
    expect(by(".ws-drag-blank")).toBeNull();
  });
});

it("removes the desktop overlay on drop without changing queued-message reorder", async () => {
  platform(WEBKIT, 2, true);
  const { root, p } = await mount(TASK);
  const transfer = start(root.querySelector<HTMLElement>('[data-q="first"]')!);
  const second = root.querySelector('[data-q="second"]')!;
  const drop = drag(second, "drop", transfer, 80, 100);
  expect(drop.defaultPrevented).toBe(true);
  expect(by(".ws-drag-overlay")).toBeNull();
  expect(p.commands.at(-1)?.payload).toMatchObject({ type: "move_queued", messageId: "first", before: "second" });
});

it.each([
  ["Chromium", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/134.0.0.0", false],
  ["Firefox", "Mozilla/5.0 (X11; Linux x86_64; rv:142.0) Gecko/20100101 Firefox/142.0", false],
  ["Android webview", "Mozilla/5.0 (Linux; Android 16) AppleWebKit/537.36 Chrome/142.0.0.0 Mobile Safari/537.36", true],
  ["Linux browser without the desktop bridge", WEBKIT, false],
])("%s receives the full-size native clone and its measured offset", async (_name, ua, bridge) => {
  platform(ua, 2, bridge);
  const { root } = await mount(TASK);
  const transfer = start(root.querySelector<HTMLElement>('[data-q="first"]')!);
  const ghost = by(".ws-drag-preview")!;
  expect(transfer.setDragImage).toHaveBeenCalledExactlyOnceWith(ghost, 22.75, 14.5);
  expect(ghost.style.width).toBe("248.5px");
  expect(ghost.style.height).toBe("48.75px");
  expect(by(".ws-drag-overlay")).toBeNull();
  expect(by(".ws-drag-blank")).toBeNull();
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(by(".ws-drag-layer")).toBeNull();
});
