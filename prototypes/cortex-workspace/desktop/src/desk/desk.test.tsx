import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";
import { render } from "solid-js/web";
import { createMockStore } from "../store";
import Desk from "./index";
import { createTheme, readThemePref, resolveTheme, THEME_KEY } from "./prefs";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); document.documentElement.removeAttribute("data-theme"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.unstubAllGlobals(); });

const tick = () => new Promise(r => setTimeout(r, 0));
function mount(path = "/") {
  history.replaceState(null, "", path);
  const store = createMockStore();
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Desk store={store} />, root);
  return { store, root };
}
const text = () => document.body.textContent ?? "";
const button = (name: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => (b.getAttribute("aria-label") ?? b.textContent ?? "").includes(name));
const go = async (path: string) => { history.pushState(null, "", path); window.dispatchEvent(new PopStateEvent("popstate")); await tick(); };

describe("theme preference", () => {
  it("defaults to system, resolves through prefers-color-scheme and follows live system changes", () => {
    let listener: ((e: MediaQueryListEvent) => void) | undefined;
    vi.stubGlobal("matchMedia", (q: string) => ({ matches: true, media: q, addEventListener: (_: string, l: (e: MediaQueryListEvent) => void) => (listener = l), removeEventListener: () => undefined }));
    expect(readThemePref()).toBe("system");
    const [theme, d] = createRoot(d => [createTheme(), d] as const);
    expect(theme.resolved()).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(theme.label()).toBe("Theme: System, currently dark");
    listener?.({ matches: false } as MediaQueryListEvent);
    expect(theme.resolved()).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
    theme.setPref("dark");
    listener?.({ matches: false } as MediaQueryListEvent);
    expect(theme.resolved()).toBe("dark"); // explicit choice ignores system changes
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    d();
    expect(resolveTheme("light", true)).toBe("light");
  });

  it("persists an explicit choice immediately, survives prototype Reset and is restored on the next mount", async () => {
    mount("/");
    const themeButton = button("Theme: System")!;
    themeButton.click();
    await tick();
    const dark = [...document.querySelectorAll<HTMLButtonElement>("[role=menuitemradio]")].find(b => b.textContent === "Dark")!;
    expect(dark.getAttribute("aria-checked")).toBe("false");
    dark.click();
    await tick();
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");
    // Reset clears fixture state only, never the theme preference.
    button("More")!.click();
    await tick();
    [...document.querySelectorAll<HTMLButtonElement>("[role=menuitem]")].find(b => b.textContent?.includes("Reset prototype"))!.click();
    await tick();
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    dispose?.(); document.body.innerHTML = "";
    document.documentElement.dataset.theme = "light";
    mount("/");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(button("Theme: Dark, currently dark")).toBeTruthy();
  });
});

describe("all-projects dashboard", () => {
  it("summarises the eight projects without listing tasks, with honest unknown and empty values", () => {
    const { root } = mount("/");
    expect(root.querySelector("h1")?.textContent).toBe("All projects");
    const rows = [...root.querySelectorAll<HTMLAnchorElement>(".proj-row")];
    expect(rows.map(r => r.querySelector(".proj-name")?.textContent)).toEqual(["Mosaic Studio", "Cedar Notes", "Orbit Ledger", "Quartz Search", "Amber Components", "Indigo Maps", "Willow Archive", "Harbor Sandbox"]);
    expect(rows[0].textContent).toContain("Disconnected");
    expect(rows[0].textContent).toContain("Unknown");
    expect(rows[3].textContent).toContain("1 failed · 1 not run");
    expect(rows[4].textContent).toContain("Git conflict");
    expect(rows[7].textContent).toContain("No activity");
    expect(rows[1].getAttribute("href")).toBe("/p/cedar-notes");
    expect(root.querySelectorAll(".needs-list .needs-row")).toHaveLength(2);
    expect(root.querySelector(".card")).toBeNull();
    expect(root.querySelector(".mock-chip")?.getAttribute("aria-label")).toContain("cw-fixtures-v3");
  });

  it("never shows empty copy that contradicts a counter, in loading, error and empty scenarios", async () => {
    const { store } = mount("/");
    const emptyVisible = () => text().includes("Nothing needs your decision");
    expect(emptyVisible()).toBe(false);
    store.selectScenario("empty");
    await tick();
    expect(emptyVisible()).toBe(true);
    expect(document.querySelector("[aria-label^='Needs you,']")?.getAttribute("aria-label")).toBe("Needs you, 0 open decisions");
    store.selectScenario("error");
    await tick();
    expect(emptyVisible()).toBe(false);
    expect(text()).toContain("Couldn't load decisions");
    store.selectScenario("default");
    await tick();
    expect(emptyVisible()).toBe(false);
  });
});

describe("per-project views", () => {
  it("lands on the board first, then reopens the last-used view", async () => {
    mount("/p/quartz-search");
    await tick();
    expect(location.pathname).toBe("/p/quartz-search/board");
    await go("/p/quartz-search/overview");
    await go("/p/cedar-notes/board");
    await go("/p/quartz-search");
    expect(location.pathname).toBe("/p/quartz-search/overview");
  });

  it("overview stats read the project aggregates; disconnected workers stay Unknown", async () => {
    const { root } = mount("/p/quartz-search/overview");
    await tick();
    const stat = (label: string) => [...root.querySelectorAll(".stat")].find(s => s.querySelector(".stat-label")?.textContent === label)!;
    expect(stat("Tasks").textContent).toContain("50");
    expect(stat("Failed checks").textContent).toContain("1");
    expect(stat("Not run").textContent).toContain("1");
    expect(stat("Stale results").textContent).toContain("1");
    expect(stat("Running workers").textContent).toContain("0");
    expect(root.textContent).toContain("Backlog 48");
    await go("/p/mosaic-studio/overview");
    expect([...root.querySelectorAll(".stat")].find(s => s.textContent?.includes("Running workers"))!.textContent).toContain("Unknown");
    expect(root.textContent).toContain("is disconnected");
  });

  it("board columns show 'No tasks' only when the column count is 0, and virtualise long columns", async () => {
    const { root } = mount("/p/harbor-sandbox/board");
    await tick();
    expect([...root.querySelectorAll(".col-empty")].map(e => e.textContent)).toEqual(["No tasks", "No tasks", "No tasks", "No tasks"]);
    await go("/p/mosaic-studio/board");
    const heads = [...root.querySelectorAll(".col-head")].map(h => h.textContent);
    expect(heads[0]).toContain("Backlog219");
    const cards = root.querySelectorAll(".card");
    expect(cards.length).toBeGreaterThan(0);
    expect(cards.length).toBeLessThan(40); // 220 tasks, only the visible window is in the DOM
    expect(root.querySelectorAll(".col-empty")).toHaveLength(2);
  });
});

describe("modal dialogs and mock actions", () => {
  it("create task: description required, title generated and renamable, background inert, mock outcome only", async () => {
    const { root, store } = mount("/p/cedar-notes/board");
    await tick();
    const invoker = button("New task")!;
    invoker.focus();
    invoker.click();
    await tick();
    expect(document.getElementById("app")?.hasAttribute("inert")).toBe(true);
    const dialog = document.querySelector("dialog")!;
    expect(dialog).toBeTruthy();
    expect(root.contains(dialog)).toBe(false);
    // No title field: the description is the only required input, and focus starts there.
    expect(dialog.querySelector("#ct-title")).toBeNull();
    expect(document.activeElement?.id).toBe("ct-desc");
    expect(dialog.querySelector("select")).toBeNull(); // project picker is a menu button, not a native select
    expect(dialog.querySelector("#ct-project")?.textContent).toContain("Cedar Notes");
    const submit = [...dialog.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.includes("Create and open"))!;
    expect(submit.disabled).toBe(true);
    expect(dialog.textContent).toContain("Describe the task");
    const desc = dialog.querySelector<HTMLTextAreaElement>("#ct-desc")!;
    // Ctrl+Enter on an empty description shows the inline error once (the footer helper hides) and creates nothing.
    desc.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true }));
    await tick();
    expect(dialog.querySelector("#ct-desc-err")?.textContent).toContain("Describe the task");
    expect(dialog.textContent?.split("Describe the task").length).toBe(2);
    expect(desc.getAttribute("aria-invalid")).toBe("true");
    expect(store.state().fixture.tasks).toHaveLength(500);
    desc.value = "Add an export button to the notes list. It should produce Markdown.";
    desc.dispatchEvent(new InputEvent("input", { bubbles: true }));
    await tick();
    expect(submit.disabled).toBe(false);
    submit.click();
    await tick(); await tick();
    expect(document.querySelector("dialog")).toBeNull();
    expect(document.getElementById("app")?.hasAttribute("inert")).toBe(false);
    expect(store.state().fixture.tasks).toHaveLength(501);
    expect(store.state().fixture.tasks[500].projectId).toBe("mock-project-00001");
    expect(location.pathname).toBe("/p/cedar-notes/t/0501/discussion");
    expect(text()).toContain("Mock outcome: task recorded in memory only. No agent was started.");
    // Title generated from the first sentence of the description (mock rule, no model call), then renamed.
    expect(store.state().fixture.tasks[500].title).toBe("Add an export button to the notes list");
    expect(store.state().fixture.tasks[500].description).toContain("It should produce Markdown.");
    expect(document.querySelector("#view-title")?.textContent).toBe("Add an export button to the notes list");
    button("Rename task")!.click();
    await tick();
    const rename = document.querySelector<HTMLInputElement>("#rename-input")!;
    rename.value = "Markdown export";
    rename.dispatchEvent(new InputEvent("input", { bubbles: true }));
    rename.form!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await tick();
    expect(document.querySelector("#view-title")?.textContent).toBe("Markdown export");
    expect(document.querySelector(".breadcrumb")?.textContent).toContain("#0501 Markdown export");
  });

  it("shows project-neutral display titles for generated fixture tasks", async () => {
    const { root, store } = mount("/p/cedar-notes/list");
    await tick();
    const raw = store.state().fixture.tasks.filter(t => t.projectId === "mock-project-00001" && / synthetic task \d+$/.test(t.title));
    expect(raw.length).toBeGreaterThan(10);
    const names = ["Mosaic", "Cedar", "Orbit", "Quartz", "Amber", "Indigo"];
    const titles = [...root.querySelectorAll(".td-title")].map(t => t.textContent ?? "");
    expect(titles.length).toBeGreaterThan(0);
    expect(titles.some(t => names.some(n => t.startsWith(n)) && t.includes("synthetic task"))).toBe(false);
  });

  it("disconnected project: create and decision actions are disabled with a visible reason", async () => {
    mount("/p/mosaic-studio/board");
    await tick();
    button("New task")!.focus();
    button("New task")!.click();
    await tick();
    const dialog = document.querySelector("dialog")!;
    expect(dialog.textContent).toContain("Mosaic Studio machine is disconnected; tasks can't be created");
    [...dialog.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "Cancel")!.click();
    await tick(); await tick();
    expect(document.querySelector("dialog")).toBeNull();
    expect(document.activeElement).toBe(button("New task"));
  });

  it("permission decision records a mock response without granting anything", async () => {
    const { store } = mount("/p/cedar-notes/t/0002/decisions");
    await tick();
    const approve = button("Approve once")!;
    expect(approve.textContent).toContain("Mock");
    expect(approve.textContent).toContain("(mock)");
    approve.click();
    await tick();
    expect(store.state().responses["mock-decision-permission"]).toBe("Approved once");
    expect(text()).toContain("No authorization was granted");
    expect(text()).toContain("execution facts are unchanged");
    await go("/");
    expect(document.querySelectorAll(".needs-list .needs-row")).toHaveLength(1);
  });

  it("stale result cannot be accepted and unrun checks are never styled as passed", async () => {
    mount("/p/quartz-search/t/0005/result");
    await tick();
    const accept = button("Accept mock-result-stale")!;
    expect(accept.disabled).toBe(true);
    expect(text()).toContain("This result changed after it was requested");
    expect(document.querySelector(".check-passed .icon")).toBeTruthy();
    expect([...document.querySelectorAll(".row-label")].find(r => r.textContent?.includes("Not run ·"))!.className).not.toContain("passed");
  });
});
