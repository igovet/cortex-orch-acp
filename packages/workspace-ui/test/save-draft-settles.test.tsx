import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyConfig, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, flush, scriptedTransport, wireTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); vi.useRealTimers(); });

/**
 * The installed gate's journey step on a native machine: board and queries through the service's native reads, every
 * request answered after `latency` ms (a loaded machine), and a title job that ends only when `titleAt()` says so.
 */
function machine(latency: number, titleAt: () => boolean) {
  let high = 3;
  const tasks: ReturnType<typeof wireTask>[] = [];
  const envelope = (data: unknown) => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: String(high), highWatermark: String(high), data });
  const product = { tasks: {} as Record<string, Record<string, unknown>>, sprints: [], messages: [] };
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board", "machine.call"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "journey", canonicalRoot: "/p", activation: {} }],
    "projects.status": () => ({ ready: true }),
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "history.page"]),
    "native.board": () => envelope({ type: "board", value: { items: tasks, nextCursor: null, highWatermark: String(high) } }),
    "native.query": () => envelope({ type: "history", value: { items: [], nextCursor: null, highWatermark: "0", tailCursor: "nt1.0" } }),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "1" }),
    "tasks.create": (m: any) => {
      high++;
      tasks.push(wireTask({ taskId: "t2", description: m.payload.description, executionId: null, phase: "not_started", runtimeState: null, executionState: "not_started", version: "1" }));
      product.tasks.t2 = { checkout: { kind: "main" } };
      return { requestId: m.requestId, outcome: "recorded", target: "t2", entityVersion: "1", recordedAt: "2026-11-05T00:00:00Z" };
    },
    "machine.call": (raw: any) => {
      const op = raw.op;
      if (op === "overview") return { ok: true, result: { name: "This machine" } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      if (op === "product.task" && raw.params?.action === "generate_title") return { ok: true, result: { jobId: "j1" } };
      if (op === "product.task" && raw.params?.action === "short_text_poll") {
        if (!titleAt()) return { ok: true, result: { state: "running" } };
        // The machine saved the title it wrote; the job's last read says so.
        product.tasks.t2 = { ...product.tasks.t2, title: "Generated journey title", titleState: "generated" };
        return { ok: true, result: { state: "done", source: "model", saved: true } };
      }
      return { ok: true, result: op === "repo.snapshot" ? null : op === "workers" ? [] : product };
    },
  });
  const raw = s.transport.request.bind(s.transport);
  if (latency) s.transport.request = async (operation, message) => { await new Promise(resolve => setTimeout(resolve, latency)); return raw(operation, message); };
  return s;
}

const saveDraftButton = () => [...document.querySelectorAll("button")].find(b => b.getAttribute("aria-label") === "Save draft" || b.textContent?.trim() === "Save draft");
const syncing = () => !!document.querySelector(".ws-sync");
const landed = () => !saveDraftButton() && !!document.querySelector('[aria-label="Board scope: All work"]') && !syncing();
const cardTitle = (text: string) => [...document.querySelectorAll<HTMLElement>(".card-t span")].find(e => e.textContent?.includes(text));
const callNames = (calls: { operation: string; message: unknown }[]) => calls.map(c => c.operation === "machine.call"
  ? `${(c.message as { op: string }).op}/${(c.message as { params?: { action?: string } }).params?.action ?? ""}` : c.operation);

async function saveDraft(s: ReturnType<typeof scriptedTransport>, wait: (ms: number) => Promise<void>, latency: number) {
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/new?kind=task&project=p`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
  for (let i = 0; i < 30; i++) await wait(20 + latency);
  const field = document.querySelector<HTMLTextAreaElement>('[aria-label="Describe the task"]')!;
  field.value = "Installed input journey"; field.dispatchEvent(new Event("input", { bubbles: true }));
  await wait(0);
  saveDraftButton()!.click();
}

// However slow each reply is, the confirmed state after Save draft settles within a few round trips and then stays settled:
// the port's own follow-up reads (a board's details, facts that arrive after the board) never start another coherent read,
// and the title job, which here never ends, holds nothing.
it.each([0, 80, 200])("saving a draft settles its confirmed state promptly and stays quiet with %i ms replies", async latency => {
  const tick = async (ms: number) => { await new Promise(r => setTimeout(r, ms)); await flush(); };
  const s = machine(latency, () => false);
  await saveDraft(s, tick, latency);
  const before = s.calls.length, pressed = Date.now();
  const slot = document.querySelector(".main > .ws-pane-header > .ws-sync-slot");
  expect(slot).not.toBeNull();
  let sawSync = false;
  for (let i = 0; i < 100 && !landed(); i++) {
    await tick(50);
    const notice = document.querySelector(".ws-sync");
    if (notice) {
      sawSync = true;
      // The status sits in the shell, not in the pane that owns the form/composer.
      expect(notice.parentElement).toBe(slot);
      expect(notice.closest(".ws-panes, [role=tablist]")).toBeNull();
      expect(notice.getAttribute("role")).toBe("status");
      expect(notice.textContent).toBe("Updating confirmed state…");
    }
  }
  if (latency > 0) expect(sawSync).toBe(true);
  expect(slot!.childElementCount).toBe(0);
  expect(slot!.isConnected).toBe(true);
  const took = Date.now() - pressed;
  expect(landed()).toBe(true);
  // The save is a few requests in a row; a second is ample on a loaded test machine.
  expect(took).toBeLessThan(1000 + 8 * latency);
  expect(document.body.textContent).toContain("Installed input journey");
  expect(s.calls.slice(before).filter(c => c.operation === "tasks.create")).toHaveLength(1);
  // Then quiet. The board screen reads its facts once when it opens; after that, over two more seconds, only the title job's
  // polls: never a read loop. Nothing shows as being confirmed meanwhile.
  for (let i = 0; i < 40; i++) { await tick(50); expect(syncing()).toBe(false); }
  const settled = s.calls.length;
  for (let i = 0; i < 40; i++) { await tick(50); expect(syncing()).toBe(false); }
  const later = callNames(s.calls.slice(settled));
  expect(later.filter(c => c !== "product.task/short_text_poll")).toEqual([]);
  expect(later.length).toBeLessThanOrEqual(3);
}, 30_000);

// The owner's rule: the task is created under a temporary title (its description label), nothing shows as updating while the
// title is written, and the written title then replaces that text in place, on the same card.
it("keeps the temporary title without any updating state until the generated title arrives, then changes it in place", async () => {
  vi.useFakeTimers();
  const wait = async (ms: number) => { await vi.advanceTimersByTimeAsync(ms); };
  let titled: number | null = null;
  const s = machine(0, () => titled !== null && Date.now() >= titled);
  await saveDraft(s, wait, 0);
  titled = Date.now() + 10_000;
  await wait(1000);
  expect(landed()).toBe(true);
  const card = cardTitle("Installed input journey");
  expect(card).toBeDefined();
  // Nine seconds on: still the temporary title, nothing being confirmed, the card open to the owner.
  for (let i = 0; i < 8; i++) { await wait(1000); expect(syncing()).toBe(false); }
  expect(card!.isConnected).toBe(true);
  expect(card!.textContent).toContain("Installed input journey");
  expect(document.body.textContent).not.toContain("Generated journey title");
  // The job ends at ten seconds: the same card element now reads the generated title.
  for (let i = 0; i < 4 && !document.body.textContent?.includes("Generated journey title"); i++) await wait(1000);
  expect(card!.isConnected).toBe(true);
  expect(card!.textContent).toBe("Generated journey title");
  expect(syncing()).toBe(false);
  // The job is over: no more polls.
  const after = s.calls.length;
  await wait(5000);
  expect(callNames(s.calls.slice(after)).filter(c => c === "product.task/short_text_poll")).toEqual([]);
});
