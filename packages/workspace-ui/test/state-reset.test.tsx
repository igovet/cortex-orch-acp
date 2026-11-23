// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import type { Worker } from "@cortex/workspace-domain";
import { WsContext, type Ws } from "../src/ctx";
import { Head, StopCommand } from "../src/worker";
import { flush } from "./support";

/** A background refresh replaces records with new objects of the same identity. Local view state keyed by identity must survive it. */
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const worker = (patch: Partial<Worker> = {}): Worker => ({ id: "w1", taskId: "t", executionId: "e", role: "backend-dev", status: "working", lane: "execution",
  goal: Array.from({ length: 12 }, (_, i) => `Goal line ${i + 1}`).join("\n"), now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null,
  context: null, usage: null, activity: [{ sequence: 1, text: "Reading", kind: "note", at: null }], result: null, ...patch });
const ws = { route: () => null, needs: () => ({}), gate: () => undefined, can: () => true, t: () => "", voiceAvailable: () => false, voice: undefined,
  ok: () => undefined, run: async () => ({ ok: true }), store: { receipt: () => null, sends: { returned: () => null, shown: () => [] } } } as unknown as Ws;
const mount = (view: () => any) => { const root = document.createElement("div"); document.body.append(root); dispose = render(() => <WsContext.Provider value={ws}>{view()}</WsContext.Provider>, root); };
const toggle = () => document.querySelector<HTMLButtonElement>(".ws-goal-toggle")!;

it("keeps a worker's expanded goal open when a background refresh replaces its record, and folds it for another worker", async () => {
  const [w, setW] = createSignal(worker());
  mount(() => <Head w={w()} now={0} />);
  expect(toggle().getAttribute("aria-expanded")).toBe("false");
  toggle().click();
  await flush();
  expect(toggle().getAttribute("aria-expanded")).toBe("true");
  setW(worker({ activity: [{ sequence: 1, text: "Reading", kind: "note", at: null }, { sequence: 2, text: "Editing", kind: "note", at: null }], elapsedMs: 5000 }));
  await flush();
  expect(toggle().getAttribute("aria-expanded")).toBe("true");
  expect(document.querySelector(".d-goal")?.className).toContain("ws-goal-open");
  setW(worker({ id: "w2" }));
  await flush();
  expect(toggle().getAttribute("aria-expanded")).toBe("false");
});

it("keeps the reason typed in the stop-command dialog across a refresh of the same command, and clears it for another", async () => {
  const item = (sequence: number, text = "sleep 60") => ({ sequence, text, kind: "command" as const, at: null });
  const [target, setTarget] = createSignal<{ worker: Worker; item: ReturnType<typeof item> } | null>({ worker: worker(), item: item(3) });
  mount(() => <StopCommand target={target()} onClose={() => undefined} />);
  await flush();
  const field = () => document.querySelector<HTMLTextAreaElement>("textarea, [contenteditable=true], input[type=text]")!;
  field().value = "Too slow";
  field().dispatchEvent(new Event("input", { bubbles: true }));
  await flush();
  setTarget({ worker: worker({ elapsedMs: 9000 }), item: item(3) });
  await flush();
  expect(field().value).toBe("Too slow");
  setTarget({ worker: worker(), item: item(4, "sleep 90") });
  await flush();
  expect(field().value).toBe("");
});
