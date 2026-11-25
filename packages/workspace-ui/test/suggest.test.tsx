import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Decision, type ProjectFacts, type ProjectRepo, type ProjectSkills, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { FILE_DEBOUNCE_MS, mention, rankFiles, skillText, SUGGEST_HINT, triggerAt } from "../src/suggest";
import { flush, projection, scriptedPort, withTask } from "./support";

// `@` names a project file and `/` a command or a project skill, picked from a list under the caret's field. What is picked
// is plain text in the field, so a draft, a copy and a send carry exactly what the owner sees.
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const rest = async () => { await new Promise(r => setTimeout(r, FILE_DEBOUNCE_MS + 30)); await settle(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const named = (label: string) => all<HTMLButtonElement>("button").find(b => b.getAttribute("aria-label") === label || b.textContent?.trim() === label);

const TREE = ["README.md", "src/auth/token.ts", "src/auth/refresh.ts", "src/main.ts", "docs/заметки.md", "docs/Отчёт по релизу.md", "test/token.test.ts"];
const repo = (tree = TREE): ProjectRepo => ({ projectId: "p", remote: "", tree, sources: {}, commits: [], branches: [], checkouts: [], fetchedAt: null });
const live = { status: "working", process: "live", executionId: "e" } as Partial<Task>;
const question = (): Decision => ({ id: "d1", kind: "question", from: "QA Engineer", workerId: null, at: null, text: "Which file?", options: [], answer: null, withdrawn: false });
function world(task: Partial<Task> = live, repos: ProjectRepo[] = [repo()], decisions: Decision[] = []): WorkspaceState {
  const state = withTask(emptyWorkspace(), "Fix token refresh race");
  Object.assign(state.tasks[0], task);
  const session = emptySession("t");
  session.decisions = decisions;
  session.timeline = decisions.map(d => ({ id: `e-${d.id}`, kind: "decision" as const, decisionId: d.id }));
  state.sessions = [session];
  state.repos = repos;
  state.machines[0].connection = "connected";
  return state;
}
async function mount(state: WorkspaceState, skills?: ProjectSkills, path = "/t/t") {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  const listed = vi.fn(async () => skills ?? { skills: [], omitted: 0 });
  p.port.listProjectSkills = listed;
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}${path}`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return { ...p, listed };
}
const field = (label = "Message") => $<HTMLTextAreaElement>(`.dock [aria-label="${label}"]`) ?? $<HTMLTextAreaElement>(`[aria-label="${label}"]`)!;
/** Types as a keyboard does: the text lands with the caret at its end, then the field hears an input. */
const type = (el: HTMLTextAreaElement, text: string) => { el.focus(); el.value = text; el.setSelectionRange(text.length, text.length); el.dispatchEvent(new Event("input", { bubbles: true })); };
const press = (el: HTMLTextAreaElement, key: string, init: KeyboardEventInit = {}) => { const e = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }); el.dispatchEvent(e); return e; };
const options = () => all('[role="listbox"] [role="option"]').map(o => o.querySelector(".grow > span")?.textContent);
const listbox = () => $('[role="listbox"]');

describe("what the caret is in", () => {
  it("finds an @ word at the start or after a space, never inside an address, and a / word only where a line begins", () => {
    expect(triggerAt("@", 1, true)).toEqual({ kind: "file", start: 0, query: "" });
    expect(triggerAt("see @src/ma", 11, true)).toEqual({ kind: "file", start: 4, query: "src/ma" });
    expect(triggerAt("mail me@host", 12, true)).toBeNull();
    expect(triggerAt("see @src now", 12, true)).toBeNull();
    expect(triggerAt("/sto", 4, true)).toEqual({ kind: "slash", start: 0, query: "sto" });
    expect(triggerAt("first line\n  /rev", 17, true)).toEqual({ kind: "slash", start: 13, query: "rev" });
    expect(triggerAt("go to src/main", 14, true)).toBeNull();
    expect(triggerAt("/stop", 5, false)).toBeNull();
    expect(triggerAt("@заме", 5, true)).toEqual({ kind: "file", start: 0, query: "заме" });
  });
  it("ranks a file name that starts with the words first, finds letters in order, ignores case and script, and caps the list", () => {
    expect(rankFiles(TREE, "token")).toEqual(["src/auth/token.ts", "test/token.test.ts"]);
    expect(rankFiles(TREE, "srm")[0]).toBe("src/main.ts");
    expect(rankFiles(TREE, "ЗАМЕ")).toEqual(["docs/заметки.md"]);
    expect(rankFiles(TREE, "отчёт")).toEqual(["docs/Отчёт по релизу.md"]);
    expect(rankFiles(Array.from({ length: 500 }, (_, i) => `f/${i}.ts`), "ts")).toHaveLength(20);
    expect(mention("docs/Отчёт по релизу.md")).toBe('@"docs/Отчёт по релизу.md"');
    expect(skillText({ name: "demo-data", path: ".agents/skills/demo-data/SKILL.md", description: null, pathOnly: false })).toBe("Use the project skill demo-data (.agents/skills/demo-data/SKILL.md).");
    expect(skillText({ name: "tool-discipline", path: ".agents/skills/tool-discipline/SKILL.md", description: null, pathOnly: true })).toBe("Use the project skill at .agents/skills/tool-discipline/SKILL.md.");
  });
});

describe("the task's message field", () => {
  it("says what it offers, opens @ on files as typing rests, and inserts the picked path as text", async () => {
    await mount(world());
    const el = field();
    expect(el.placeholder).toBe(SUGGEST_HINT);
    type(el, "Look at @");
    await settle();
    // Nothing typed after @ looks nothing up: the list says what to do.
    expect($(".ws-suggest [role='status']")?.textContent).toBe("Type to search project files");
    expect(listbox()).toBeNull();
    type(el, "Look at @tok");
    await settle();
    expect(options()).toEqual([]);
    await rest();
    expect(options()).toEqual(["src/auth/token.ts", "test/token.test.ts"]);
    expect(el.getAttribute("aria-activedescendant")).toBe(all('[role="option"]')[0].id);
    press(el, "ArrowDown");
    await settle();
    expect(all('[role="option"]')[1].getAttribute("aria-selected")).toBe("true");
    press(el, "Enter");
    await settle();
    expect(el.value).toBe("Look at @test/token.test.ts ");
    expect($(".ws-suggest")).toBeNull();
    // The next @ offers what was picked before anything is typed.
    type(el, "Look at @test/token.test.ts and @");
    await settle();
    expect(options()).toEqual(["test/token.test.ts"]);
  });

  it("keeps Enter from sending while the list is open, and Esc closes the list without leaving the field", async () => {
    const p = await mount(world());
    const el = field();
    type(el, "@main");
    await rest();
    expect(options()).toEqual(["src/main.ts"]);
    const enter = press(el, "Escape");
    expect(enter.defaultPrevented).toBe(true);
    await settle();
    expect($(".ws-suggest")).toBeNull();
    expect(el.value).toBe("@main");
    // Dismissed stays dismissed for this word; Enter now sends what is written.
    press(el, "Enter");
    await settle();
    expect(p.commands.map(c => c.payload.type)).toContain("send_message");
    expect(p.commands.find(c => c.payload.type === "send_message")!.payload).toMatchObject({ text: "@main" });
  });

  it("picks with Tab or a press, closes when Backspace removes the @, and reads Cyrillic as typed", async () => {
    const p = await mount(world());
    const el = field();
    type(el, "@заме");
    await rest();
    expect(options()).toEqual(["docs/заметки.md"]);
    press(el, "Tab");
    await settle();
    expect(el.value).toBe("@docs/заметки.md ");
    type(el, "x @Отч");
    await rest();
    all<HTMLElement>('[role="option"]')[0].click();
    await settle();
    expect(el.value).toBe('x @"docs/Отчёт по релизу.md" ');
    type(el, "x @READ");
    await rest();
    expect(options()).toEqual(["README.md"]);
    type(el, "x ");
    await settle();
    expect($(".ws-suggest")).toBeNull();
    expect(p.commands.filter(c => c.payload.type === "send_message")).toEqual([]);
  });

  it("reads the project's files once, after typing rests, and shows only the answer for the words still in the field", async () => {
    const p = await mount(world(live, []));
    let answer!: (facts: ProjectFacts[]) => void;
    const asked: unknown[] = [];
    const query = p.port.query.bind(p.port);
    p.port.query = ((operation: string, parameters: unknown) => {
      if (operation !== "project.facts") return query(operation as never, parameters as never);
      asked.push(parameters);
      return new Promise(resolve => { answer = resolve as never; });
    }) as typeof p.port.query;
    const el = field();
    type(el, "@m"); type(el, "@ma"); type(el, "@mai");
    await settle();
    expect(asked).toEqual([]);
    await rest();
    expect(asked).toEqual([{ projectId: "p", fresh: true, git: true }]);
    expect($(".ws-suggest [role='status']")?.textContent).toBe("Searching project files…");
    // The owner typed on while the files were read: the earlier words' answer is never shown.
    type(el, "@tok");
    await rest();
    expect(asked).toHaveLength(1);
    answer([{ projectId: "p", openBranch: "main", defaultBranch: "main", repo: repo(), workers: [], reviews: [], overlays: {}, sprints: [] }]);
    await settle();
    expect(options()).toEqual(["src/auth/token.ts", "test/token.test.ts"]);
  });

  it("lists this screen's commands and the project's skills on /, runs a command instead of sending, and writes a skill as a sentence", async () => {
    const p = await mount(world(), { skills: [{ name: "demo-data", path: ".agents/skills/demo-data/SKILL.md", description: "Refresh the demo dataset", pathOnly: false }], omitted: 0 });
    const el = field();
    type(el, "/");
    await settle();
    expect(p.listed).toHaveBeenCalledWith("p");
    expect(options()).toEqual(["/stop", "/queue", "/now", "/details", "/changes", "/code", "/new", "/demo-data"]);
    expect(all(".ws-suggest .mi-h").map(h => h.textContent)).toEqual(["Commands", "Project skills"]);
    type(el, "/que");
    await settle();
    expect(options()).toEqual(["/queue"]);
    press(el, "Enter");
    await settle();
    expect(el.value).toBe("");
    expect(named("Queue")?.getAttribute("aria-pressed")).toBe("true");
    expect(p.commands).toEqual([]);
    type(el, "Please\n/dem");
    await settle();
    expect(options()).toEqual(["/demo-data"]);
    press(el, "Enter");
    await settle();
    expect(el.value).toBe("Please\nUse the project skill demo-data (.agents/skills/demo-data/SKILL.md). ");
    // `/` inside a line is just text.
    type(el, "see a/b");
    await settle();
    expect($(".ws-suggest")).toBeNull();
    // The skills were read once for this list, not on every keystroke.
    expect(p.listed).toHaveBeenCalledTimes(1);
  });

  it("offers Resume, not Stop, on a paused task", async () => {
    await mount(world({ status: "paused", process: "stopped", executionId: "e" } as Partial<Task>));
    const el = field();
    type(el, "/");
    await settle();
    expect(options()).toContain("/resume");
    expect(options()).not.toContain("/stop");
  });
});

it.each(["task", "sprint"] as const)("offers editable skills and files but no runtime actions on New %s", async kind => {
  const p = await mount(world(), { skills: [{ name: "demo-data", path: ".agents/skills/demo-data/SKILL.md", description: null, pathOnly: false }], omitted: 0 }, `/new?kind=${kind}&project=p`);
  const el = field(`Describe the ${kind}`);
  type(el, "/");
  await settle();
  expect(p.listed).toHaveBeenCalledWith("p");
  expect(options()).toEqual(["/demo-data"]);
  press(el, "Enter");
  await settle();
  expect(el.value).toBe("Use the project skill demo-data (.agents/skills/demo-data/SKILL.md). ");
  expect(p.commands).toEqual([]);
  type(el, el.value + "@main");
  await rest();
  expect(options()).toEqual(["src/main.ts"]);
  press(el, "Tab");
  await settle();
  const description = "Use the project skill demo-data (.agents/skills/demo-data/SKILL.md). @src/main.ts ";
  expect(el.value).toBe(description);
  expect(p.commands).toEqual([]);
  press(el, "Enter", { ctrlKey: true });
  await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({ type: kind === "sprint" ? "create_sprint" : "create_task", description });
});

describe("an answer to a question", () => {
  it("tags files with @ and offers no / list", async () => {
    await mount(world(live, [repo()], [question()]));
    const el = $<HTMLTextAreaElement>('[aria-label="Your answer"]')!;
    expect(el.placeholder).toBe("Write your answer, tag @files…");
    type(el, "/");
    await settle();
    expect($(".ws-suggest")).toBeNull();
    type(el, "@main");
    await rest();
    expect(options()).toEqual(["src/main.ts"]);
    press(el, "Enter");
    await settle();
    expect(el.value).toBe("@src/main.ts ");
    expect(named("Send answer")).toBeDefined();
  });
});
