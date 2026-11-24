import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LOCAL_MACHINE_ID, type Activity, type FileDiff, type ProjectRepo, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { pagesWorld } from "./pages-world";
import { flush, projection, scriptedPort } from "./support";

/**
 * Background updates never reset what the owner is doing. Every store read rebuilds its records as new objects, so on each
 * main screen this opens something (a menu, a fold, a list, a file), scrolls, types or focuses, then lets the machine send
 * (1) the very same data as fresh objects and (2) unrelated news: another task's worker moves on and the same task gets a
 * new event. The same nodes must still be on screen, with the same scroll, open state, highlight, focus and typed text.
 * jsdom has no layout, so a scroller's position is set by hand: a rebuilt scroller starts again at 0.
 */
let dispose: (() => void) | undefined;
beforeEach(() => {
  localStorage.clear(); sessionStorage.clear();
  Range.prototype.getClientRects = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => ({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON() {} });
});
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string, within: ParentNode = document) => within.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string, within: ParentNode = document) => [...within.querySelectorAll<T>(selector)];
const M = LOCAL_MACHINE_ID;

const TREE = ["README.md", "src/auth/token.ts", "src/auth/refresh.ts", "src/main.ts", "docs/notes.md"];
const LONG = Array.from({ length: 80 }, (_, i) => `line ${i + 1}`).join("\n");
const command = (sequence: number): Activity => ({ sequence, kind: "command", text: `step ${sequence}`, at: null,
  args: JSON.stringify({ kind: "execute", title: "Bash", command: `step ${sequence}`, output: "ok", status: "completed" }), command: { state: "native", exit: 0, took: null, tail: "ok", reason: null } });
const diff = (path: string, add: number): FileDiff => ({ path, status: "M", add, del: 0,
  hunks: [{ header: "@@ -1 +1 @@", lines: Array.from({ length: add }, (_, i) => ({ kind: "add" as const, old: null, new: i + 1, text: `${path} ${i}` })) }] } as FileDiff);
const skills = Array.from({ length: 12 }, (_, i) => ({ name: `skill-${i}`, path: `.agents/skills/skill-${i}/SKILL.md`, description: `Skill number ${i}`, pathOnly: false }));

/** The pages world plus a repository, a review, a working worker's activity and a long goal: every screen has state to keep. */
function world(): WorkspaceState {
  const s = pagesWorld();
  const repo: ProjectRepo = { projectId: "p", hasGit: true, remote: "", tree: TREE, sources: Object.fromEntries(TREE.map(p => [p, LONG])), commits: [], branches: [], fetchedAt: null,
    checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "main", path: "/home/owner/projects/atlas-api", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, files: [], draft: "", sources: {}, updateConflict: null }] };
  s.repos = [repo];
  s.reviews = [{ taskId: "c-done", sha: "abc", files: [diff("a.ts", 30), diff("b.ts", 40)], scopes: [], comments: [] } as never];
  const be2 = s.workers.find(w => w.id === "w-be2")!;
  be2.goal = Array.from({ length: 12 }, (_, i) => `Goal line ${i + 1}: lock the rotation and add a version column.`).join("\n");
  be2.activity = [1, 2, 3, 4].map(command);
  return s;
}
/** Unrelated news: another task's worker moves on, and the task on screen gets one more event. */
function news(taskId: string): WorkspaceState {
  const s = world();
  const other = s.workers.find(w => w.taskId !== taskId && w.status === "working")!;
  other.now = "Running the concurrency test";
  const session = s.sessions.find(x => x.taskId === taskId);
  if (session) session.timeline = [...session.timeline, { id: `${taskId}-late`, kind: "cortex", text: "One more thing happened." }];
  const be2 = s.workers.find(w => w.id === "w-be2")!;
  if (taskId === "c-work") be2.activity = [...be2.activity, command(5)];
  return s;
}

async function mount(path: string, state = world()) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  p.port.listProjectSkills = async () => ({ skills, omitted: 0 });
  p.port.readFile = async () => ({ text: LONG, revision: "r1", readOnly: null, writer: null });
  history.replaceState(null, "", path.replaceAll("<M>", M));
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return p;
}
/** A scroller left at `at`, as if the owner scrolled there. A rebuilt node starts again at 0. */
function scrolledTo(el: HTMLElement, at: number) {
  let top = at;
  Object.defineProperties(el, { clientHeight: { configurable: true, get: () => 200 }, scrollHeight: { configurable: true, get: () => 4000 }, scrollTop: { configurable: true, get: () => top, set: (v: number) => { top = v; } } });
  el.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 })); el.dispatchEvent(new Event("scroll"));
}
/** Sends the same data again as fresh objects, then unrelated news, and runs `kept` after each. */
async function background(p: Awaited<ReturnType<typeof mount>>, taskId: string, kept: () => void) {
  p.emit(projection("2", world())); await settle();
  kept();
  p.emit(projection("3", news(taskId))); await settle();
  kept();
}
const type = (el: HTMLTextAreaElement | HTMLInputElement, text: string) => { el.focus(); el.value = text; el.setSelectionRange?.(text.length, text.length); el.dispatchEvent(new Event("input", { bubbles: true })); };
const press = (el: HTMLElement, key: string) => el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
/** Every element under `root`, to check that none was replaced. */
const nodes = (root: Element) => [root, ...root.querySelectorAll("*")];
const allConnected = (list: Element[]) => list.every(n => n.isConnected);

describe("a background update keeps every screen as the owner left it", () => {
  it("Conversation: the open workers menu, the scrolled / suggestions with their highlight, the typed text and its focus", async () => {
    const p = await mount("/m/<M>/t/c-work");
    const all_ = $<HTMLButtonElement>(".ws-team-all")!;
    all_.click(); await settle();
    const team = document.getElementById(all_.getAttribute("popovertarget")!)!;
    const teamOpen = team.matches(":popover-open");
    const teamRows = all(".wrow", team);
    expect(teamRows.length).toBeGreaterThan(0);
    const field = $<HTMLTextAreaElement>(".dock textarea")!;
    type(field, "/"); await settle();
    const list = $('[role="listbox"]')!;
    const pop = list.closest<HTMLElement>(".ws-suggest")!;
    const rows = all('[role="option"]', list);
    // Commands and the project's skills: more than the box shows, so it scrolls.
    expect(rows.length).toBeGreaterThan(8);
    press(field, "ArrowDown"); press(field, "ArrowDown"); press(field, "ArrowDown"); await settle();
    const picked = rows.find(r => r.getAttribute("aria-selected") === "true")!;
    expect(rows.indexOf(picked)).toBe(3);
    scrolledTo(pop, 120);
    const timeline = $<HTMLElement>(".timeline")!;
    scrolledTo(timeline, 300);
    const before = nodes(list);
    await background(p, "c-work", () => {
      expect($(".ws-team-all")).toBe(all_);
      expect(team.isConnected).toBe(true);
      expect(team.matches(":popover-open")).toBe(teamOpen);
      expect(allConnected(teamRows)).toBe(true);
      expect($('[role="listbox"]')).toBe(list);
      expect(allConnected(before)).toBe(true);
      expect(all('[role="option"]', list)).toEqual(rows);
      expect(picked.getAttribute("aria-selected")).toBe("true");
      expect(field.getAttribute("aria-activedescendant")).toBe(picked.id);
      expect(pop.scrollTop).toBe(120);
      expect($(".dock textarea")).toBe(field);
      expect(field.value).toBe("/");
      expect(document.activeElement).toBe(field);
      expect($(".timeline")).toBe(timeline);
      expect(timeline.scrollTop).toBe(300);
    });
    expect(timeline.textContent).toContain("One more thing happened.");
  });

  it("Conversation: the @ file list keeps its rows and highlight", async () => {
    const p = await mount("/m/<M>/t/c-work");
    const field = $<HTMLTextAreaElement>(".dock textarea")!;
    type(field, "@src"); await new Promise(r => setTimeout(r, 200)); await settle();
    const list = $('[role="listbox"]')!;
    const rows = all('[role="option"]', list);
    expect(rows.length).toBeGreaterThan(1);
    press(field, "ArrowDown"); await settle();
    const picked = rows[1];
    expect(picked.getAttribute("aria-selected")).toBe("true");
    await background(p, "c-work", () => {
      expect($('[role="listbox"]')).toBe(list);
      expect(all('[role="option"]', list)).toEqual(rows);
      expect(picked.getAttribute("aria-selected")).toBe("true");
      expect(field.value).toBe("@src");
    });
  });

  it("Work details: the selected worker, its opened goal and step, and the stream's scroll", async () => {
    const p = await mount("/m/<M>/t/c-work/workers/w-be2");
    const goal = $<HTMLButtonElement>(".ws-goal-toggle")!;
    goal.click(); await settle();
    expect(goal.getAttribute("aria-expanded")).toBe("true");
    const fold = $<HTMLButtonElement>("button.fold");
    fold?.click(); await settle();
    const step = all<HTMLButtonElement>("button.step-row")[0];
    step.click(); await settle();
    expect(step.getAttribute("aria-expanded")).toBe("true");
    const stream = $<HTMLElement>(".stream")!;
    scrolledTo(stream, 250);
    const pane = $(".ws-goal-toggle")!.closest("section, .wdet, .worker, div")!;
    await background(p, "c-work", () => {
      expect(location.pathname).toBe(`/m/${M}/t/c-work/workers/w-be2`);
      expect($(".ws-goal-toggle")).toBe(goal);
      expect(goal.getAttribute("aria-expanded")).toBe("true");
      if (fold) { expect(fold.isConnected).toBe(true); expect(fold.getAttribute("aria-expanded")).toBe("true"); }
      expect(step.isConnected).toBe(true);
      expect(step.getAttribute("aria-expanded")).toBe("true");
      expect($(".stream")).toBe(stream);
      expect(stream.scrollTop).toBe(250);
      expect(pane.isConnected).toBe(true);
    });
  });

  it("Changes: the picked file, its diff and the diff's scroll", async () => {
    const p = await mount("/m/<M>/t/c-done/changes");
    const items = all<HTMLButtonElement>(".flist .fitem");
    expect(items).toHaveLength(2);
    items[1].click(); await settle();
    expect(items[1].getAttribute("aria-current")).toBe("true");
    const view = $<HTMLElement>(".dview")!;
    scrolledTo(view, 500);
    const diffB = document.getElementById("diff-b.ts")!;
    const lines = nodes(diffB);
    await background(p, "c-done", () => {
      expect(all(".flist .fitem")).toEqual(items);
      expect(items[1].getAttribute("aria-current")).toBe("true");
      expect($(".dview")).toBe(view);
      expect(view.scrollTop).toBe(500);
      expect(document.getElementById("diff-b.ts")).toBe(diffB);
      expect(allConnected(lines)).toBe(true);
    });
  });

  it("restores both Changes scrollers after a trip to Conversation", async () => {
    await mount("/m/<M>/t/c-done/changes");
    $(".flist")!.scrollTop = 90;
    $(".dview")!.scrollTop = 500;

    $<HTMLAnchorElement>(".td-back")!.click(); await settle();
    expect($(".dview")).toBeNull();
    $<HTMLAnchorElement>('a.td-link[href$="/changes"]')!.click(); await settle();

    expect([$(".flist")!.scrollTop, $(".dview")!.scrollTop]).toEqual([90, 500]);
  });

  it("restores the Changes page body for a result with evidence but no diff", async () => {
    const state = world();
    state.reviews[0].files = [];
    state.tasks.find(t => t.id === "c-done")!.result!.checks = Array.from({ length: 30 }, (_, i) => ({
      name: `Check ${i + 1}`, detail: "Expected output was not found.", outcome: "failed", source: "runtime",
    }));
    await mount("/m/<M>/t/c-done/changes", state);
    const selector = '.rv-panel .l-page-body[data-scroll="vertical"]';
    const body = $(selector)!;
    expect($(".dview")).toBeNull();
    expect(body.textContent).toContain("Check 30");
    // jsdom has no layout; give this populated page an overflowing viewport.
    scrolledTo(body, 420);
    expect(body.scrollHeight).toBeGreaterThan(body.clientHeight);

    $<HTMLAnchorElement>(".td-back")!.click(); await settle();
    expect(body.isConnected).toBe(false);
    $<HTMLAnchorElement>('a.td-link[href$="/changes"]')!.click(); await settle();

    expect($(selector)!.scrollTop).toBe(420);
  });

  it("restores a worker's paused reading position, but follows the bottom after resuming", async () => {
    let height = 4000;
    const viewport = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(200);
    const content = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
    try {
      await mount("/m/<M>/t/c-work/workers/w-be2");
      const stream = () => $(".stream")!;
      // Latest stands in the worker's dock exactly while the feed does not follow.
      const latest = () => $<HTMLButtonElement>(".dock-w .new-pill");
      const pick = async (id: string) => {
        const select = $<HTMLSelectElement>(".ws-worker-picker select")!;
        select.value = id;
        select.dispatchEvent(new Event("change", { bubbles: true }));
        await settle();
      };
      const other = [...$<HTMLSelectElement>(".ws-worker-picker select")!.options]
        .map(option => option.value).find(id => !["w-be2", "verification", "cortex"].includes(id))!;
      expect(other).toBeDefined();
      expect(stream().scrollTop).toBe(3800);
      scrolledTo(stream(), 250);
      await pick(other);
      expect(location.pathname).toContain(`/workers/${other}`);
      expect(stream()).toBeNull(); // The other fixture worker has not started yet.
      await pick("w-be2");
      expect(stream().scrollTop).toBe(250);
      expect(latest()).not.toBeNull();

      latest()!.click();
      expect(stream().scrollTop).toBe(3800);
      await pick(other);
      height = 4700;
      await pick("w-be2");
      expect(latest()).toBeNull();
      expect(stream().scrollTop).toBe(4500);
    } finally {
      content.mockRestore(); viewport.mockRestore();
    }
  });

  it("Code: a folded folder stays folded, the open file and its preview's scroll stay", async () => {
    const p = await mount("/m/<M>/p/p/code?file=src%2Fmain.ts");
    for (let i = 0; i < 20 && !$(".cm-scroller, .code"); i++) await settle();
    const tree = $('[role="tree"]')!;
    const docs = all<HTMLButtonElement>('[role="treeitem"]', tree).find(b => b.textContent === "docs")!;
    docs.click(); await settle();
    expect(docs.getAttribute("aria-expanded")).toBe("false");
    const auth = all<HTMLButtonElement>('[role="treeitem"]', tree).find(b => b.textContent === "auth")!;
    expect(auth.getAttribute("aria-expanded")).toBe("true");
    const rows = all('[role="treeitem"]', tree);
    scrolledTo(tree, 40);
    const preview = $<HTMLElement>(".cm-scroller") ?? $<HTMLElement>(".code")!.parentElement!;
    scrolledTo(preview, 700);
    await background(p, "c-work", () => {
      expect($('[role="tree"]')).toBe(tree);
      expect(all('[role="treeitem"]', tree)).toEqual(rows);
      expect(docs.getAttribute("aria-expanded")).toBe("false");
      expect(auth.getAttribute("aria-expanded")).toBe("true");
      expect(tree.scrollTop).toBe(40);
      expect(preview.isConnected).toBe(true);
      expect(preview.scrollTop).toBe(700);
    });
  });

  it("Board: every card, a column's scroll and the typed filter", async () => {
    const p = await mount("/m/<M>/p/p/board");
    const cards = all(".col .card");
    expect(cards.length).toBeGreaterThan(2);
    const column = cards[0].closest<HTMLElement>(".col")!;
    scrolledTo(column, 90);
    const before = nodes($(".board, .cols, main") ?? document.body).filter(n => n.closest(".col"));
    await background(p, "c-work", () => {
      expect(all(".col .card")).toEqual(cards);
      expect(column.scrollTop).toBe(90);
      expect(allConnected(before)).toBe(true);
    });
  });

  it("Inbox: its rows and focus", async () => {
    const p = await mount("/m/<M>/inbox");
    const rows = all<HTMLAnchorElement>(".page .need");
    expect(rows.length).toBe(3);
    rows[1].focus();
    await background(p, "c-ask", () => {
      expect(all(".page .need")).toEqual(rows);
      expect(document.activeElement).toBe(rows[1]);
    });
  });

  it("Sprint planning: the plan rows, the planning conversation, the scroller and the typed message", async () => {
    const p = await mount("/m/<M>/p/p/s/sp/planning");
    const scroll = $<HTMLElement>(".sp-scroll")!;
    expect(scroll).not.toBeNull();
    scrolledTo(scroll, 160);
    const inside = nodes(scroll);
    const field = $<HTMLTextAreaElement>(".composer textarea")!;
    type(field, "Split the lockout in two");
    await background(p, "c-work", () => {
      expect($(".sp-scroll")).toBe(scroll);
      expect(scroll.scrollTop).toBe(160);
      expect(allConnected(inside)).toBe(true);
      expect($(".composer textarea")).toBe(field);
      expect(field.value).toBe("Split the lockout in two");
      expect(document.activeElement).toBe(field);
    });
  });

  it("Search and actions: the result rows, the highlight, the list's scroll and the typed search", async () => {
    const p = await mount("/m/<M>/inbox");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true })); await settle();
    const input = $<HTMLInputElement>("dialog[open] .ws-pal-in")!;
    type(input, "o"); await settle();
    const list = $<HTMLElement>("#ws-pal-list")!;
    const rows = all('[role="option"]', list);
    expect(rows.length).toBeGreaterThan(2);
    press(input, "ArrowDown"); press(input, "ArrowDown"); await settle();
    const picked = rows[2];
    expect(picked.getAttribute("aria-selected")).toBe("true");
    scrolledTo(list, 80);
    await background(p, "c-work", () => {
      expect(all('[role="option"]', list)).toEqual(rows);
      expect(picked.getAttribute("aria-selected")).toBe("true");
      expect(list.scrollTop).toBe(80);
      expect(input.value).toBe("o");
      expect(document.activeElement).toBe(input);
    });
  });

  it("Settings: the page, its controls and the focused one", async () => {
    const p = await mount(`/settings/machine/<M>/connections`);
    const page = $<HTMLElement>("main, .page, .set-page")!;
    const controls = all<HTMLElement>("button, input, select, a", page);
    expect(controls.length).toBeGreaterThan(0);
    const focused = controls.find(c => !c.hasAttribute("disabled")) ?? controls[0];
    focused.focus();
    scrolledTo(page, 60);
    await background(p, "c-work", () => {
      expect(allConnected(controls)).toBe(true);
      expect(document.activeElement).toBe(focused);
      expect(page.scrollTop).toBe(60);
    });
  });
});
