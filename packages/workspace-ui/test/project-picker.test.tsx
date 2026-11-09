// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { folderName, folderNameProblem, matchesPath, moveActive, pickerPaths, shortenPath } from "../src/project-picker-model";
import { capabilitiesWire, flush, scriptedTransport } from "./support";

/** The whole flow against the real port, with the machine scripted. The search itself has its own tests on the helper. */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
const HOME = "/home/owner";
const dialog = () => document.querySelector<HTMLDialogElement>("dialog[open]");
const button = (text: string) => all("button").find(b => b.textContent?.trim().endsWith(text))!;
const options = () => all('dialog[open] [role="option"]');
const titles = () => options().map(o => o.querySelector(".grow")!.firstChild!.textContent);
const option = (title: string) => options().find(o => o.querySelector(".grow")!.firstChild!.textContent === title)!;
const selected = () => options().findIndex(o => o.getAttribute("aria-selected") === "true");
const field = () => dialog()!.querySelector<HTMLInputElement>("input")!;
const type = (text: string) => { const el = field(); el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); };
const press = (key: string) => (dialog()!.querySelector("input") ?? dialog()!.querySelector('[role="listbox"]'))!.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
const escape = () => dialog()!.dispatchEvent(new Event("cancel", { cancelable: true }));
const answered = () => wait(300).then(settle);
const METHODS = ["projects.list", "projects.register", "projects.activate", "projects.status", "native.pick_folder", "directories.search", "directories.cancel"];
const found = (...paths: string[]) => (message: unknown) => ({ query: (message as { query: string }).query, home: HOME, complete: true, entries: (message as { query: string }).query.trim() ? paths.map(path => ({ path, git: path.endsWith("cedar") })) : [] });

function machine(answers: Parameters<typeof scriptedTransport>[0] = {}, methods = METHODS) {
  return scriptedTransport({
    "service.capabilities": () => ({ methods }),
    "projects.list": () => [],
    "capabilities.get": () => capabilitiesWire([]),
    "directories.search": found(),
    "directories.cancel": () => ({ outcome: "cancelled" }),
    ...answers,
  });
}
async function mount(s: ReturnType<typeof machine>) {
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "peer", registry: true })} />, root);
  await settle();
}
const open = async () => { button("Add project").click(); await settle(); };
const search = async () => { await open(); option("Search for folder").click(); await settle(); };
const registered = (s: ReturnType<typeof machine>) => s.calls.filter(c => c.operation === "projects.register").map(c => c.message);
const searches = (s: ReturnType<typeof machine>) => s.calls.filter(c => c.operation === "directories.search").map(c => (c.message as { query: string }).query);
const added = (id = "p1") => () => ({ projectId: id });

describe("project progress", () => {
  it("announces progress immediately while registration is pending and sends only once", async () => {
    let release!: (value: unknown) => void;
    const s = machine({ "projects.register": () => new Promise(resolve => { release = resolve; }) });
    await mount(s); await search(); type("/large/repo"); await answered();
    press("Enter"); await settle();
    expect(dialog()!.querySelector('[role="status"]')?.textContent).toContain("Adding project...");
    expect(dialog()!.querySelector('[role="listbox"]')?.getAttribute("aria-busy")).toBe("true");
    press("Enter"); await settle();
    expect(registered(s)).toHaveLength(1);
    release({projectId:"p1"}); await settle();
    expect(dialog()).toBeNull();
  });

  it("shows progress while the native folder chooser is pending", async () => {
    let release!: (value: unknown) => void;
    const s = machine({ "native.pick_folder": () => new Promise(resolve => { release = resolve; }) });
    await mount(s); await open(); option("Browse").click(); await settle();
    expect(dialog()!.querySelector('[role="status"]')?.textContent).toContain("Choosing folder...");
    release({path:null}); await settle();
    expect(dialog()!.querySelector('[role="listbox"]')?.getAttribute("aria-busy")).toBe("false");
  });
});

describe("entry points", () => {
  it("opens the same flow from the empty state, the sidebar and the palette", async () => {
    await mount(machine());
    for (const control of all("button").filter(b => b.textContent?.includes("Add project"))) {
      control.click(); await settle();
      expect(dialog()?.getAttribute("aria-label")).toBe("Add a project");
      expect(titles()).toContain("Search for folder");
      escape(); await settle();
      expect(dialog()).toBeNull();
    }
    expect(all("button").filter(b => b.textContent?.includes("Add project")).length).toBeGreaterThan(1);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
    await settle();
    const palette = dialog()!.querySelector<HTMLInputElement>("input")!;
    palette.value = ">add project"; palette.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    all('dialog[open] [role="option"]').find(o => o.textContent?.includes("Add project"))!.click(); await settle();
    expect(dialog()?.getAttribute("aria-label")).toBe("Add a project");
  });

  it("names the machine it searches, and offers the system chooser only for this machine", async () => {
    await mount(machine());
    await open();
    expect(dialog()!.textContent).toContain("Add project");
    expect(dialog()!.textContent).toContain("This machine");
    expect(titles()).toEqual(["Search for folder", "Clone from GitHub", "Browse", "New folder"]);
  });

  it("leaves Browse out when no chooser is offered", async () => {
    await mount(machine({}, METHODS.filter(m => m !== "native.pick_folder")));
    await open();
    expect(titles()).toEqual(["Search for folder", "Clone from GitHub", "New folder"]);
  });
});

describe("the search field", () => {
  it("waits for a pause, then asks the machine once for the latest text", async () => {
    const s = machine();
    await mount(s); await search();
    s.calls.length = 0;
    type("c"); type("ce"); type("ced"); await settle();
    expect(searches(s)).toEqual([]);
    await answered();
    expect(searches(s)).toEqual(["ced"]);
    expect(s.calls.find(c => c.operation === "directories.search")!.message).toMatchObject({ limit: 30 });
  });

  it("shows results as the home sees them, with Git repositories marked", async () => {
    await mount(machine({ "directories.search": found(`${HOME}/code/cedar`, "/mnt/data/cedar-old") }));
    await search(); type("cedar"); await answered();
    expect(titles()).toEqual(["~/code/cedar", "/mnt/data/cedar-old"]);
    expect(option("~/code/cedar").textContent).toContain(`${HOME}/code/cedar`);
    expect(option("~/code/cedar").textContent).toContain("Git repository");
    expect(option("/mnt/data/cedar-old").textContent).not.toContain("Git repository");
  });

  it("offers a typed path as itself until the machine reports it", async () => {
    let reported = false;
    await mount(machine({ "directories.search": message => { const reply = found(...(reported ? [`${HOME}/code`] : []))(message); return reply; } }));
    await search(); type("~/co"); await answered();
    expect(titles()).toEqual(["~/co"]);
    expect(option("~/co").textContent).toContain("Open this path");
    reported = true;
    type("~/code"); await answered();
    expect(titles()).toEqual(["~/code"]);
    expect(option("~/code").textContent).not.toContain("Open this path");
  });

  it("lists the machine's projects first and narrows them by the text", async () => {
    const s = machine({ "projects.list": () => [{ projectId: "p", displayName: "Cedar", canonicalRoot: `${HOME}/code/cedar`, activation: null }, { projectId: "q", displayName: "Maple", canonicalRoot: "/mnt/data/maple", activation: null }], "directories.search": found(`${HOME}/code/cedar`, `${HOME}/code/cedar-app`) });
    await mount(s);
    await search();
    expect(titles()).toEqual(["~/code/cedar", "/mnt/data/maple"]);
    type("cedar"); await answered();
    expect(titles()).toEqual(["~/code/cedar", "~/code/cedar-app"]);
  });

  it("keeps rows that still fit while the machine answers, so the list narrows without blinking", async () => {
    const paths = [`${HOME}/code/cedar`, `${HOME}/code/maple`];
    let release: () => void = () => {};
    const s = machine({ "directories.search": message => (message as { query: string }).query === "c" ? found(...paths)(message) : new Promise(resolve => { release = () => resolve(found(paths[0])(message)); }) });
    await mount(s); await search(); type("c"); await answered();
    expect(titles()).toEqual(["~/code/cedar", "~/code/maple"]);
    type("ced"); await settle();
    expect(titles()).toEqual(["~/code/cedar"]);
    await wait(300); await settle();
    expect(dialog()!.textContent).toContain("Searching...");
    release(); await settle();
    expect(dialog()!.textContent).not.toContain("Searching...");
  });

  it("drops an answer for text that has since changed", async () => {
    const pending: Record<string, () => void> = {};
    const s = machine({ "directories.search": message => { const query = (message as { query: string }).query; return query ? new Promise(resolve => { pending[query] = () => resolve(found(`${HOME}/${query}`)(message)); }) : found()(message); } });
    await mount(s); await search();
    type("old"); await answered();
    type("new"); await answered();
    pending.new(); await settle();
    pending.old(); await settle();
    expect(titles()).toEqual(["~/new"]);
  });

  it("says when nothing matched and when a limit ended the search", async () => {
    await mount(machine({ "directories.search": message => ({ ...found()(message), complete: false }) }));
    await search(); type("nothing"); await answered();
    expect(dialog()!.textContent).toContain("No matching folders");
    expect(dialog()!.textContent).toContain("Stopped early");
  });

  it("keeps a failed search to itself: the machine stays connected and a typed path still works", async () => {
    const s = machine({ "directories.search": message => { if ((message as { query: string }).query) throw new Error("refused:The search took too long. Type more of the path."); return found()(message); }, "projects.register": added() });
    await mount(s); await search();
    type("/srv/cedar"); await answered();
    expect(dialog()!.querySelector('[role="alert"]')?.textContent).toBe("The search took too long. Type more of the path.");
    expect(document.querySelector(".ws-conn[role=alert]")).toBeNull();
    press("Enter"); await settle();
    expect(registered(s)).toEqual([{ root: "/srv/cedar", displayName: "cedar" }]);
  });

  it("works from typed paths alone on a machine with no search", async () => {
    const s = machine({ "projects.register": added() }, METHODS.filter(m => !m.startsWith("directories.")));
    await mount(s); await search();
    expect(dialog()!.textContent).toContain("Enter the full path");
    type("relative/path"); await answered();
    press("Enter"); await settle();
    expect(registered(s)).toEqual([]);
    expect(dialog()!.textContent).toContain("Enter the full path");
    type("~/code"); await settle(); press("Enter"); await settle();
    expect(registered(s)).toEqual([{ root: "~/code", displayName: "code" }]);
    expect(s.calls.some(c => c.operation === "directories.search")).toBe(false);
  });
});

describe("choosing", () => {
  const three = [`${HOME}/a`, `${HOME}/b`, `${HOME}/c`];
  it("moves with the arrows, wraps at both ends, and Enter adds the folder and opens it", async () => {
    const s = machine({ "directories.search": found(...three), "projects.register": added() });
    await mount(s); await search(); type("x"); await answered();
    expect(selected()).toBe(0);
    press("ArrowDown"); await settle(); expect(selected()).toBe(1);
    press("ArrowUp"); press("ArrowUp"); await settle(); expect(selected()).toBe(2);
    press("ArrowDown"); await settle(); expect(selected()).toBe(0);
    press("ArrowDown"); await settle();
    expect(field().getAttribute("aria-activedescendant")).toBe(options()[1].id);
    press("Enter"); await settle();
    expect(registered(s)).toEqual([{ root: `${HOME}/b`, displayName: "b" }]);
    expect(dialog()).toBeNull();
    expect(location.pathname).toContain("/p/p1/");
  });

  it("adds the folder a click lands on, and the pointer moves the selection", async () => {
    const s = machine({ "directories.search": found(...three), "projects.register": added() });
    await mount(s); await search(); type("x"); await answered();
    option("~/c").dispatchEvent(new MouseEvent("mousemove", { bubbles: true })); await settle();
    expect(selected()).toBe(2);
    option("~/c").click(); await settle();
    expect(registered(s)).toEqual([{ root: `${HOME}/c`, displayName: "c" }]);
  });

  it("Enter on half a path takes what the machine found, and the typed path stays available below it", async () => {
    const s = machine({ "directories.search": found(`${HOME}/code/cedar`), "projects.register": added() });
    await mount(s); await search(); type("~/code/ced"); await settle();
    expect(titles()).toEqual(["~/code/ced"]);
    await answered();
    expect(titles()).toEqual(["~/code/cedar", "~/code/ced"]);
    expect(selected()).toBe(0);
    press("Enter"); await settle();
    expect(registered(s)).toEqual([{ root: `${HOME}/code/cedar`, displayName: "cedar" }]);
  });

  it("keeps the choice on the same folder when more rows arrive", async () => {
    let release: () => void = () => {};
    const s = machine({ "directories.search": message => (message as { query: string }).query === "ho" ? new Promise(resolve => { release = () => resolve(found(`${HOME}/first`, ...three)(message)); }) : found(...three)(message) });
    await mount(s); await search(); type("h"); await answered();
    type("ho"); await answered();
    press("ArrowDown"); await settle();
    expect(titles()[selected()]).toBe("~/b");
    release(); await settle();
    expect(titles()[0]).toBe("~/first");
    expect(titles()[selected()]).toBe("~/b");
  });

  it("shows the machine's reason, sends nothing twice, and hands the field back", async () => {
    let attempts = 0;
    const s = machine({ "projects.register": () => { attempts++; throw new Error("refused:This folder is private to Cortex or outside what this machine allows."); } });
    await mount(s); await search(); type("/var/private"); await answered();
    press("Enter"); await settle();
    expect(dialog()!.querySelector('[role="alert"]')?.textContent).toContain("private to Cortex");
    expect(dialog()).not.toBeNull();
    expect(document.body.textContent).not.toMatch(/not known/);
    expect(document.querySelector(".ws-conn[role=alert]")).toBeNull();
    expect(attempts).toBe(1);
    expect(document.activeElement).toBe(field());
    type("/var/other"); await settle();
    expect(dialog()!.querySelector('[role="alert"]')).toBeNull();
  });

  it("offers to create a folder that is not there, and only then does", async () => {
    const sent: unknown[] = [];
    const s = machine({ "projects.register": message => { sent.push(message); if (!(message as { create?: boolean }).create) throw new Error("project_folder_missing"); return { projectId: "p1" }; } });
    await mount(s); await search(); type("/new/fresh"); await answered();
    press("Enter"); await settle();
    expect(dialog()!.textContent).toContain("does not exist yet");
    expect(titles()[0]).toBe("Create this folder");
    expect(sent).toEqual([{ root: "/new/fresh", displayName: "fresh" }]);
    press("Enter"); await settle();
    expect(sent[1]).toEqual({ root: "/new/fresh", displayName: "fresh", create: true });
    expect(dialog()).toBeNull();
  });

  it("adds what the system chooser returns, and does nothing when it was cancelled", async () => {
    const s = machine({ "native.pick_folder": () => ({ path: null }), "projects.register": added() });
    await mount(s); await open();
    option("Browse").click(); await settle();
    expect(dialog()).not.toBeNull();
    expect(registered(s)).toEqual([]);
    s.answers["native.pick_folder"] = () => ({ path: "/mnt/data/cedar-notes" });
    option("Browse").click(); await settle();
    expect(registered(s)).toEqual([{ root: "/mnt/data/cedar-notes", displayName: "cedar-notes" }]);
  });
});

describe("the name", () => {
  it("comes from the folder and can be changed in the project's settings", async () => {
    let name = "cedar";
    const s = machine({ "projects.list": () => [{ projectId: "p", displayName: name, canonicalRoot: "/srv/cedar", activation: null }], "projects.rename": message => { name = (message as { displayName: string }).displayName; return { projectId: "p" }; } }, [...METHODS, "projects.rename", "machine.call"]);
    history.replaceState(null, "", "/m/local/p/p/settings");
    await mount(s);
    const field = await (async () => { for (let i = 0; i < 20; i++) { const el = document.querySelector<HTMLInputElement>('input[aria-label="Project name"]'); if (el) return el; await flush(); } throw new Error("no name field: " + location.pathname); })();
    expect(field.value).toBe("cedar");
    field.value = "Cedar Notes"; field.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(s.calls.filter(c => c.operation === "projects.rename").map(c => c.message)).toEqual([{ projectId: "p", displayName: "Cedar Notes" }]);
    expect(document.querySelector<HTMLInputElement>('input[aria-label="Project name"]')!.value).toBe("Cedar Notes");
  });
});

describe("a new folder", () => {
  const toName = async () => { await open(); option("New folder").click(); await settle(); type("~/code"); await answered(); press("Enter"); await settle(); };
  it("takes a parent, then a name, and shows where it will go", async () => {
    const s = machine({ "directories.search": found(`${HOME}/code`), "projects.register": added() });
    await mount(s); await toName();
    expect(dialog()!.textContent).toContain("Name folder");
    type("fresh"); await settle();
    expect(titles()).toEqual(["~/code/fresh"]);
    press("Enter"); await settle();
    expect(registered(s)).toEqual([{ root: `${HOME}/code/fresh`, displayName: "fresh", newFolder: true }]);
    expect(dialog()).toBeNull();
  });

  it("asks again for a name that cannot be a folder", async () => {
    const s = machine({ "directories.search": found(`${HOME}/code`) });
    await mount(s); await toName();
    press("Enter"); await settle();
    expect(dialog()!.querySelector('[role="alert"]')?.textContent).toContain("Enter a folder name");
    type("a/b"); await settle(); press("Enter"); await settle();
    expect(dialog()!.querySelector('[role="alert"]')?.textContent).toContain("slash");
    expect(registered(s)).toEqual([]);
  });

  it("stays on the name when the folder is already there", async () => {
    const s = machine({ "directories.search": found(`${HOME}/code`), "projects.register": () => { throw new Error("refused:A folder with this name is already there."); } });
    await mount(s); await toName();
    type("taken"); await settle(); press("Enter"); await settle();
    expect(dialog()!.querySelector('[role="alert"]')?.textContent).toContain("already there");
    expect(field().value).toBe("taken");
  });
});

describe("leaving", () => {
  it("Escape steps back through the pages and then closes", async () => {
    await mount(machine({ "directories.search": found(`${HOME}/code`) }));
    await open(); option("New folder").click(); await settle();
    type("~/code"); await answered(); press("Enter"); await settle();
    expect(dialog()!.textContent).toContain("Name folder");
    escape(); await settle();
    expect(dialog()!.textContent).toContain("Choose parent folder");
    expect(dialog()!.textContent).toContain("Back");
    escape(); await settle();
    expect(dialog()!.textContent).toContain("Close");
    expect(titles()).toContain("New folder");
    escape(); await settle();
    expect(dialog()).toBeNull();
  });

  it("the back button steps back, and pressing outside closes from any page", async () => {
    await mount(machine());
    await search();
    all('dialog[open] button[aria-label="Back"]')[0].click(); await settle();
    expect(titles()).toContain("Browse");
    option("Search for folder").click(); await settle();
    dialog()!.dispatchEvent(new MouseEvent("click", { bubbles: true })); await settle();
    expect(dialog()).toBeNull();
  });

  it("closing stops a search that is still running", async () => {
    const s = machine({ "directories.search": message => (message as { query: string }).query ? new Promise(() => {}) : found()(message) });
    await mount(s); await search(); type("cedar"); await answered();
    escape(); await settle(); escape(); await settle();
    const session = (s.calls.find(c => c.operation === "directories.search")!.message as { session: string }).session;
    expect(s.calls.filter(c => c.operation === "directories.cancel").map(c => c.message)).toEqual([{ session }]);
  });

  it("starts clean the next time", async () => {
    await mount(machine({ "directories.search": found(`${HOME}/code`) }));
    await search(); type("code"); await answered();
    escape(); await settle(); escape(); await settle();
    await open();
    expect(titles()).toContain("Browse");
    option("Search for folder").click(); await settle();
    expect(field().value).toBe("");
  });
});

describe("the model", () => {
  it("shortens a path by the machine's own home", () => {
    expect(shortenPath("/home/owner/code", "/home/owner")).toBe("~/code");
    expect(shortenPath("/home/owner", "/home/owner/")).toBe("~");
    expect(shortenPath("/home/ownerless", "/home/owner")).toBe("/home/ownerless");
    expect(shortenPath("/srv/x", null)).toBe("/srv/x");
  });
  it("names a project after the folder", () => {
    expect(folderName("/a/b/", null)).toBe("b");
    expect(folderName("~", "/home/owner")).toBe("owner");
    expect(folderName("~/code", "/home/owner")).toBe("code");
    expect(folderName("/", null)).toBe("/");
  });
  it("does not repeat a typed path the machine already reported", () => {
    const paths = (query: string, ...found: string[]) => pickerPaths({ query, recent: [], found: found.map(path => ({ path, git: false })), home: "/home/owner" });
    expect(paths("~/code/", "/home/owner/code").map(r => r.literal)).toEqual([false]);
    expect(paths("/srv/x", "/srv/x").map(r => r.literal)).toEqual([false]);
    expect(paths("/srv/x", "/srv/x/y").map(r => r.literal)).toEqual([false, true]);
    expect(paths("cedar").length).toBe(0);
  });
  it("fits a known path strictly: a typed path leads to it, a fragment appears in it", () => {
    const fits = (query: string, path: string) => matchesPath(query, path, "/home/owner");
    expect(fits("~/code/ced", "/home/owner/code/cedar")).toBe(true);
    expect(fits("/home/owner/CODE", "/home/owner/code/cedar")).toBe(true);
    expect(fits("CED", "/home/owner/code/cedar")).toBe(true);
    expect(fits("zzz", "/home/owner/code/cedar")).toBe(false);
    expect(fits("~", "/mnt/anything")).toBe(true);
    expect(fits("~/Code/", "/home/owner/vault/locked-repo")).toBe(false);
    expect(fits("cdr", "/home/owner/code/cedar")).toBe(false);
  });
  it("puts the folder the text names before a project inside it, so Enter takes what was typed", () => {
    const rows = pickerPaths({ query: "~/code/", recent: ["/home/owner/code/cedar", "/home/owner/vault/locked-repo"], found: [{ path: "/home/owner/code", git: false }, { path: "/home/owner/code/cedar", git: true }], home: "/home/owner" });
    expect(rows.map(r => r.path)).toEqual(["/home/owner/code", "/home/owner/code/cedar"]);
  });
  it("refuses names that are not one folder", () => {
    for (const name of ["", " ", ".", "..", "a/b", "a\\b"]) expect(folderNameProblem(name)).not.toBeNull();
    expect(folderNameProblem("fresh")).toBeNull();
  });
  it("skips rows that cannot be chosen", () => {
    expect(moveActive(0, [true, false, true], 1)).toBe(2);
    expect(moveActive(0, [true, false, true], -1)).toBe(2);
    expect(moveActive(0, [false, false], 1)).toBe(0);
  });
});
