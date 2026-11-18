import { render } from "solid-js/web";
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { capabilityOf, emptyWorkspace, MACHINE_COMMANDS, type CommandEnvelope, type CommandReceipt, type PortUpdate, type QueryOperation, type QueryResults, type RepoCheckout, type WorkspacePort, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { DIVERGED, gitItems, gitPrimary, RUNNING, type GitFacts } from "../src/git-actions-model";
import { openTargets } from "../src/git-actions";
import { buildIndex } from "../src/selectors";
import { draftTask, flush, projection } from "./support";

const checkout = (patch: Partial<RepoCheckout> = {}): RepoCheckout => ({ id: "p:main", projectId: "p", kind: "main", branch: "main", path: "/p", taskId: null, label: null, base: "origin/main", upstream: "origin/main", ahead: 0, behind: 0, behindOf: {}, files: [], draft: "", sources: {}, updateConflict: null, ...patch });
const file = (path: string, stage: "staged" | "unstaged" | "untracked" = "unstaged") => ({ path, status: "M" as const, add: 1, del: 0, hunks: [], stage });
function world(c: RepoCheckout, task: Partial<ReturnType<typeof draftTask>> | null = {}, remote = "git@example.invalid:o/p.git"): WorkspaceState {
  const s = emptyWorkspace();
  s.machines[0].connection = "connected";
  s.machines[0].ghAvailable = true;
  s.projects = [{ id: "p", machineId: s.machines[0].id, name: "Project", repository: "/p", openBranch: "main", defaultBranch: "main", hasGit: true }];
  if (task) s.tasks = [{ ...draftTask(), title: "Fix the race", ...task }];
  s.repos = [{ projectId: "p", hasGit: true, remote, tree: [], sources: {}, commits: [], branches: [], checkouts: [c], fetchedAt: null }];
  return s;
}
const off = () => undefined;
const facts = (s: WorkspaceState, patch: Partial<GitFacts> = {}): GitFacts => {
  const index = buildIndex(s, 1), view = s.tasks[0] ? index.byId.get(s.tasks[0].id)! : null;
  return { checkout: s.repos[0].checkouts[0], repo: s.repos[0], task: view, mode: view?.task.status === "ready_to_merge" || view?.task.status === "review" ? "review" : "draft", running: null, gate: off, offered: () => true, ...patch };
};
const ready = { status: "ready_to_merge" as const, checkout: { kind: "worktree" as const, branch: "task/fix", path: "/w" }, result: { version: 1, digest: "d", summary: "Done", checks: [] }, workspaceDigest: "d", acceptance: { version: 1, digest: "d" } };

describe("the title band's Git action for each state", () => {
  it("chooses Commit, Push, Pull, Pull and push, Merge or Up to date from the checkout and the task", () => {
    expect(gitPrimary(facts(world(checkout({ files: [file("a.ts")] }))))).toMatchObject({ id: "commit", label: "Commit…", why: undefined });
    expect(gitPrimary(facts(world(checkout({ ahead: 2 }))))).toMatchObject({ id: "push", label: "Push" });
    expect(gitPrimary(facts(world(checkout({ behind: 1 }))))).toMatchObject({ id: "pull", label: "Pull" });
    expect(gitPrimary(facts(world(checkout({ ahead: 2, behind: 1 }))))).toMatchObject({ id: "pull_push", label: "Pull and push" });
    const merge = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t", files: [file("a.ts")] }), ready);
    expect(gitPrimary(facts(merge))).toMatchObject({ id: "merge", label: "Merge…", strong: true });
    // A recorded pull request does not end the task: Merge stays its next step, and Create PR… says one is open.
    const pr = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t" }), { ...ready, integration: { method: "pr", targetBranch: "main", digest: "d" } });
    expect(gitPrimary(facts(pr))).toMatchObject({ id: "merge" });
    expect(gitItems(facts(pr)).find(i => i.id === "pr")?.why).toBe("A pull request is already open for this version");
    const clean = gitPrimary(facts(world(checkout())));
    expect(clean).toMatchObject({ id: null, label: "Up to date" });
    expect(clean.why).toMatch(/Nothing to commit, pull or push/);
  });

  it("lets counts against the target branch alone never suggest a pull or a push", () => {
    const p = gitPrimary(facts(world(checkout({ upstream: "", base: "main", ahead: 3, behind: 2 }))));
    expect(p).toMatchObject({ id: null, label: "Up to date" });
    expect(p.why).toContain("No upstream branch");
  });

  it("lists every item in one fixed order, each on or off with its reason", () => {
    const s = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t", behindOf: { main: 2 } }), { ...ready, status: "review", acceptance: null });
    const items = gitItems(facts(s));
    expect(items.map(i => i.id)).toEqual(["commit", "fetch", "pull", "push", "pull_push", "update", "merge", "pr", "archive"]);
    expect(Object.fromEntries(items.map(i => [i.id, i.why]))).toEqual({
      commit: "Nothing to commit", fetch: undefined, pull: "Nothing to pull in the last fetched state. Fetch to check the upstream.", push: "Nothing to push", pull_push: "Nothing to pull in the last fetched state. Fetch to check the upstream.",
      update: undefined, merge: "Result not accepted yet", pr: "Result not accepted yet", archive: "Available when the task is done",
    });
    expect(items.find(i => i.id === "update")?.label).toBe("Update from main");
    const noGh = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t" }), ready);
    noGh.machines[0].ghAvailable = false;
    expect(gitItems(facts(noGh)).find(i => i.id === "pr")?.why).toBe("GitHub CLI not available");
  });

  it("leaves out what the machine cannot run and every network action of a local-only repository", () => {
    const s = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t" }), ready, "");
    const ids = gitItems(facts(s, { offered: type => type !== "update_from_target" })).map(i => i.id);
    expect(ids).toEqual(["commit", "merge", "archive"]);
  });

  it("turns every item off with the same reason while one of its actions runs, and shows that action's progress", () => {
    const f = facts(world(checkout({ files: [file("a.ts")], ahead: 1, behind: 1 }), { status: "done" }), { running: "pull_push", progress: "Pushing…" });
    expect(gitItems(f).length).toBeGreaterThan(4);
    expect(gitItems(f).every(i => i.why === RUNNING)).toBe(true);
    expect(gitPrimary(f)).toMatchObject({ id: "pull_push", label: "Pushing…", icon: "loader", why: RUNNING });
  });

  it("acts on the project folder without a task, with no task-only items", () => {
    const ids = gitItems(facts(world(checkout({ files: [file("a.ts")] }), null))).map(i => i.id);
    expect(ids).toEqual(["commit", "fetch", "pull", "push", "pull_push"]);
  });
});

describe("Open in", () => {
  const ws = (open: boolean, terminal: boolean) => ({ can: (c: string) => open && c === "workspace.open_editor", externalLabel: () => "Open in VS Code", terminalAvailable: () => terminal });
  it("lists only what this computer and machine can open", () => {
    expect(openTargets(ws(true, true), checkout(), false).map(t => t.label)).toEqual(["Open in VS Code", "Open in Code tab", "Open terminal here"]);
    expect(openTargets(ws(false, false), checkout(), false).map(t => t.label)).toEqual(["Open in Code tab"]);
    // A worktree has no shell of its own here, and opens in the editor only from its own record.
    expect(openTargets(ws(true, true), null, true).map(t => t.id)).toEqual(["code"]);
  });
});

describe("the band's word room", () => {
  it("folds the split buttons' words to icons and drops the counts on a narrow band, by container rule", () => {
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../src/workspace.css"), "utf8");
    expect(css).toMatch(/@container tb \(max-width: \d+px\) \{ \.split\.hs > \.btn-line \.hs-t \{ display: none; \} \}/);
    expect(css).toMatch(/@container tb \(max-width: \d+px\) \{\s*\.tb-actions > :is\(\.hg-sync, \.hs-open, \.tb-state\) \{ display: none; \}/);
  });
});

/** A port that answers each command by its type, with the service's own words for a refusal. */
function port(answer: (c: CommandEnvelope) => Partial<CommandReceipt>, draft?: string, operations?: string[]) {
  const listeners = new Set<(u: PortUpdate) => void>();
  const commands: CommandEnvelope[] = [];
  const p: WorkspacePort = {
    mode: "real",
    query<O extends QueryOperation>(operation: O): Promise<QueryResults[O]> {
      const a: QueryResults = { "capabilities.get": operations ? { everything: false, operations } : { everything: true, operations: [] }, "project.snapshot": [], "project.facts": [], "machine.facts": [], "machine.overview": [] };
      return Promise.resolve(a[operation]);
    },
    command(envelope) { commands.push(envelope); return Promise.resolve({ requestId: envelope.requestId, ok: true, outcome: "recorded", message: "Recorded", ...answer(envelope) }); },
    subscribe(l) { listeners.add(l); return () => listeners.delete(l); },
    ...(draft ? { draftIntegrationText: async () => ({ source: "model" as const, message: draft }) } : {}),
  };
  return { port: p, commands, emit: (u: PortUpdate) => { for (const l of listeners) l(u); } };
}
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
async function open(s: WorkspaceState, path: string, p = port(() => ({}))) {
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", s)); await settle();
  return { root, p };
}
const band = (root: HTMLElement) => root.querySelector<HTMLElement>(".titlebar .tb-actions")!;
const gitPrimaryButton = (root: HTMLElement) => band(root).querySelector<HTMLButtonElement>('[aria-label="Git"] .hg-a')!;
const menuLabels = (root: HTMLElement) => [...root.querySelectorAll<HTMLElement>("#m-git .mi")].map(b => b.querySelector(".grow")!.firstChild!.textContent);

describe("the band on screen", () => {
  it("commits from the band only through its dialog, with the drafted message, after the owner confirms", async () => {
    const s = world(checkout({ files: [file("src/a.ts", "staged"), file("b.ts")] }), { status: "review", result: { version: 1, digest: "d", summary: "Done", checks: [] } });
    const p = port(() => ({}), "Fix the login race");
    const { root } = await open(s, "/m/local/t/t", p);
    const primary = gitPrimaryButton(root);
    expect(primary.getAttribute("aria-label")).toBe("Commit…");
    expect(band(root).querySelector(".hg-sync")?.getAttribute("aria-label")).toBe("2 uncommitted files");
    primary.click(); await settle();
    const dialog = root.querySelector<HTMLDialogElement>("dialog[open]")!;
    expect(dialog.textContent).toContain("1 staged, 1 unstaged");
    expect(dialog.querySelector("textarea")!.value).toBe("Fix the login race");
    expect(dialog.textContent).toContain("Written from the changes, you can edit it");
    expect(p.commands.some(c => c.payload.type === "git_commit")).toBe(false);
    [...dialog.querySelectorAll("button")].find(b => b.textContent === "Commit")!.click(); await settle();
    expect(p.commands.filter(c => c.payload.type === "git_commit").map(c => c.payload)).toEqual([expect.objectContaining({ type: "git_commit", checkoutId: "p:main", message: "Fix the login race" })]);
  });

  it("asks first while a task works in the checkout, as saving does", async () => {
    const s = world(checkout({ files: [file("a.ts")] }), { status: "working", process: "live" });
    const { root, p } = await open(s, "/m/local/t/t");
    gitPrimaryButton(root).click(); await settle();
    const dialog = root.querySelector<HTMLDialogElement>("dialog[open]")!;
    const confirm = [...dialog.querySelectorAll("button")].find(b => b.textContent === "Commit")!;
    expect(confirm.getAttribute("data-why")).toContain("A task is working in this checkout");
    confirm.click(); await settle();
    expect(p.commands.some(c => c.payload.type === "git_commit")).toBe(false);
    expect([...dialog.querySelectorAll("button")].map(b => b.textContent)).toEqual(expect.arrayContaining(["Stop the task first", "Commit anyway"]));
    [...dialog.querySelectorAll("button")].find(b => b.textContent === "Commit anyway")!.click(); await settle();
    expect(confirm.getAttribute("aria-disabled")).toBeNull();
  });

  it("offers no Pull and push on diverged histories: it says they diverged, a fast-forward is impossible and where to reconcile", async () => {
    const s = world(checkout({ ahead: 1, behind: 2 }), null);
    const p = port(() => ({}));
    const { root } = await open(s, "/m/local/p/p/board", p);
    const primary = gitPrimaryButton(root);
    expect(primary.getAttribute("aria-label")).toBe("Pull and push");
    expect(band(root).querySelector(".hg-sync")?.textContent).toBe("↑1↓2");
    // Nothing to run: the button opens the menu, where Pull and push, Pull and Push each say why they are off.
    expect(primary.getAttribute("aria-haspopup")).toBe("menu");
    primary.click(); await settle();
    expect(p.commands).toEqual([]);
    const menu = root.querySelector("#m-git")!.textContent!;
    expect(menu).toContain(DIVERGED);
    expect(menu).toContain("The remote has commits you do not have. Pull first.");
  });

  it("shows Git's classified refusal of a pull beside the button, and pushes nothing", async () => {
    // Counts from the last fetch said behind only; the remote moved since, so the fast-forward pull is refused.
    const s = world(checkout({ behind: 2 }), null);
    const why = "The local and remote histories diverged, so Git cannot fast-forward this branch. Nothing was pulled. Merge or rebase the remote branch in the terminal (Ctrl+J), then push.";
    const p = port(c => c.payload.type === "git_pull" ? { ok: false, outcome: "refused", message: why } : {});
    const { root } = await open(s, "/m/local/p/p/board", p);
    gitPrimaryButton(root).click(); await settle();
    expect(p.commands.map(c => c.payload.type)).toEqual(["git_pull"]);
    const alert = root.querySelector("#m-git-fail [role=alert]")!;
    expect(alert.textContent).toContain(why);
    expect([...alert.querySelectorAll("button")].map(b => b.textContent)).toEqual(["Retry", "Dismiss"]);
  });

  it("shows another Git action's refusal in the service's own words, and offers Retry", async () => {
    const s = world(checkout({ behind: 1 }), null);
    const busy = "A fetch, pull or push of this project is still running. Try again when it finishes.";
    const p = port(c => c.payload.type === "git_pull" ? { ok: false, outcome: "refused", message: busy } : {});
    const { root } = await open(s, "/m/local/p/p/board", p);
    gitPrimaryButton(root).click(); await settle();
    expect(p.commands.map(c => c.payload)).toEqual([expect.objectContaining({ type: "git_pull", checkoutId: "p:main" })]);
    expect(root.querySelector("#m-git-fail [role=alert]")!.textContent).toContain(busy);
  });

  it("keeps the integration as the task's next step and its closing alternatives in their own place", async () => {
    const s = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t", path: "/w" }), ready);
    const { root } = await open(s, "/m/local/t/t");
    const primary = gitPrimaryButton(root);
    expect(primary.getAttribute("aria-label")).toBe("Merge…");
    expect(primary.classList.contains("btn-primary")).toBe(true);
    expect([...band(root).querySelectorAll("button")].filter(b => b.textContent === "Merge…")).toHaveLength(1);
    // This port offers every command, so Update from main is listed too; the real service does not offer it today.
    expect(menuLabels(root)).toEqual(["Commit…", "Fetch", "Pull", "Push", "Pull and push", "Update from main", "Merge locally…", "Create PR…", "Archive task"]);
    primary.click(); await settle();
    expect(root.querySelector("dialog[open]")?.getAttribute("data-dialog")).toBe("Finish task");
  });

  it("shows no task item on the project's own board, and the open-in menu holds only what is there", async () => {
    const { root } = await open(world(checkout(), null), "/m/local/p/p/board");
    expect(menuLabels(root)).toEqual(["Commit…", "Fetch", "Pull", "Push", "Pull and push"]);
    expect(gitPrimaryButton(root).getAttribute("aria-label")).toBe("Up to date");
    // Nothing to do is not a dead control: it opens the menu, where each item says why it is on or off.
    expect(gitPrimaryButton(root).getAttribute("aria-disabled")).toBeNull();
    expect(gitPrimaryButton(root).getAttribute("aria-haspopup")).toBe("menu");
    expect([...root.querySelectorAll("#m-open-in .mi")].map(b => b.textContent)).toEqual(expect.arrayContaining(["Open in Code tab"]));
  });

  it("shows no Update from the target in the Merge dialog or the menu where the machine has no such operation", async () => {
    const s = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t", path: "/w", behindOf: { main: 2 } }), ready);
    const p = port(() => ({}), undefined, MACHINE_COMMANDS.map(capabilityOf));
    const { root } = await open(s, "/m/local/t/t", p);
    expect(menuLabels(root)).not.toContain("Update from main");
    gitPrimaryButton(root).click(); await settle();
    const dialog = root.querySelector<HTMLDialogElement>("dialog[open]")!;
    expect(dialog.textContent).toContain("Behind main by 2 commits");
    expect([...dialog.querySelectorAll("button")].map(b => b.textContent)).not.toContain("Update from main");
  });
});
