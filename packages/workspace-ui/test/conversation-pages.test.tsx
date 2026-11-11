import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { LOCAL_MACHINE_ID, type WorkspaceCommand } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { arrange, convRoom, SWAP_BELOW } from "../src/sprint-space";
import { pagesWorld } from "./pages-world";
import { flush, projection, scriptedPort } from "./support";

// The pages around the task conversation: one Needs you list on Home, work requests in the shared composer, and a planning
// conversation that a phone-width pane still shows.
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const named = (label: string, within: ParentNode = document) => [...within.querySelectorAll<HTMLButtonElement>("button")].find(b => b.getAttribute("aria-label") === label || b.textContent?.trim() === label);

async function mount(path: string, mode: "real" | "demo" = "real") {
  const p = scriptedPort({ mode, capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path.replaceAll("<M>", LOCAL_MACHINE_ID));
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", pagesWorld())); await settle();
  return p;
}

it("lists every task waiting on the owner once on Home, in the order its decision matters, and counts the same list", async () => {
  await mount("/m/<M>");
  const rows = all(".home .need");
  expect(rows.map(r => r.querySelector(".kind")?.textContent)).toEqual(["Question", "Review", "Finish"]);
  expect(rows[0].querySelector(".ask")?.textContent).toBe("Which concurrency level should the regression test assert?");
  expect(rows[1].querySelector(".ask")?.textContent).toBe("Result v1 is ready: 2 passed, 1 not run");
  expect(rows[2].querySelector(".ask")?.textContent).toBe("Accepted v2: cw/rotate-signing-keys into main");
  expect(new Set(rows.map(r => r.getAttribute("href"))).size).toBe(3);
  // One list, not three competing summaries; engine limits live in the sidebar and in Connections.
  expect($(".home")?.textContent).not.toContain("Ready for review");
  expect($(".home")?.textContent).not.toContain("Engine limits");
  expect($(".tb-facts")?.textContent).toContain("3 need you");
});

it("makes the Inbox the same Needs you list, each row opening where its decision is made", async () => {
  await mount("/m/<M>/inbox");
  const rows = all(".page .need");
  expect(rows.map(r => r.querySelector(".kind")?.textContent)).toEqual(["Question", "Review", "Finish"]);
  expect(rows[1].querySelector(".ask")?.textContent).toBe("Result v1 is ready: 2 passed, 1 not run");
  // A result is accepted where it is shown, inline in the task's conversation, not on a second answering surface.
  expect(rows.every(r => /\/t\/[^/]+$/.test(r.getAttribute("href") ?? ""))).toBe(true);
  expect($(".page textarea, .page .composer")).toBeNull();
  expect(all(".nav").find(n => n.textContent?.includes("Inbox"))?.textContent).toContain("3");
});

it("gives each board card one concise fact and one next action, with Stop left to the task", async () => {
  await mount("/m/<M>/p/p/board");
  const card = (col: string) => $(`.col[data-col="${col}"] .card`)!;
  const action = (col: string) => card(col).querySelector(".card-a .btn")?.textContent?.trim();
  expect(card("needs").querySelector(".card-s")?.textContent).toBe("Which concurrency level should the regression test assert?");
  expect(action("needs")).toBe("Answer");
  expect(card("work").querySelector(".card-s")?.textContent).toBe("Working");
  expect(action("work")).toBe("Open");
  expect(card("work").querySelector(".card-a")?.textContent).not.toContain("Stop");
  expect(card("review").querySelector(".card-s")?.textContent).toBe("Result v1 is ready: 2 passed, 1 not run");
  expect(card("review").querySelector<HTMLAnchorElement>(".card-a .btn")?.getAttribute("href")).toMatch(/\/t\/[^/]+$/);
  expect(card("merge").querySelector(".card-s")?.textContent).toBe("Accepted v2: cw/rotate-signing-keys into main");
});

it("writes a knowledge request in the shared composer: Attach and Voice in place, and the plane only creates the task", async () => {
  const p = await mount("/m/<M>/p/p/knowledge", "demo");
  named("Collect knowledge…")!.click(); await settle();
  const dialog = $("dialog[open]")!;
  expect(dialog.querySelector(".mc textarea")).not.toBeNull();
  const attach = named("Attach files", dialog) ?? dialog.querySelector<HTMLButtonElement>(".mc-tools button")!;
  expect(attach.getAttribute("aria-disabled")).toBe("true");
  expect(attach.dataset.why).toContain("text only");
  expect(named("Voice input", dialog)).toBeDefined();
  expect(dialog.querySelector(".mc-effect")?.textContent).toContain("does not start");
  const area = dialog.querySelector<HTMLTextAreaElement>(".mc textarea")!;
  area.value = "docs/api"; area.dispatchEvent(new Event("input", { bubbles: true })); await settle();
  named("Create the task", dialog)!.click(); await settle();
  const created = p.commands.map(c => c.payload as WorkspaceCommand).filter(c => c.type === "create_task") as Extract<WorkspaceCommand, { type: "create_task" }>[];
  expect(created.map(c => c.description)).toEqual(["harvest: docs/api"]);
  expect(p.commands.some(c => (c.payload as WorkspaceCommand).type === "start_task")).toBe(false);
});

it("keeps the planning conversation on a pane too narrow for it beside the plan, and lets each lead to the other", () => {
  const at = (width: number, opened: Parameters<typeof arrange>[0]["opened"] = "none", details = false) =>
    arrange({ width, view: "list", full: false, sidebar: true, sidebarW: 260, conv: true, details, opened });
  expect(at(390)).toMatchObject({ swap: true, conv: true, sidebar: false });
  expect(at(390, "plan")).toMatchObject({ swap: true, conv: false, main: 390 });
  expect(at(390, "details", true)).toMatchObject({ swap: true, conv: false, details: true, replaced: true });
  expect(at(SWAP_BELOW)).toMatchObject({ swap: false });
  expect(convRoom(390, "graph")).toBe(true);
});
