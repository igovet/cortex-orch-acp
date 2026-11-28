import { render } from "solid-js/web";
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { capabilityOf, emptyWorkspace, MACHINE_COMMANDS, type CommandEnvelope, type CommandReceipt, type IntegrationText, type PortUpdate, type QueryOperation, type QueryResults, type RepoCheckout, type WorkspacePort, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { DETACHED, DIRTY, DIVERGED, gitItems, gitPrimary, PR_UNKNOWN, RUNNING, STOPPED, type GitFacts } from "../src/git-actions-model";
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
    expect(gitPrimary(facts(world(checkout({ ahead: 2, behind: 1 }))))).toMatchObject({ id: "pull", label: "Pull…" });
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
      update: undefined, merge: "Result not accepted yet", pr: "Result not accepted yet", archive: undefined,
    });
    expect(items.find(i => i.id === "update")?.label).toBe("Update from main…");
    const noGh = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t" }), ready);
    noGh.machines[0].ghAvailable = false;
    expect(gitItems(facts(noGh)).find(i => i.id === "pr")?.why).toBe("GitHub CLI not available");
  });

  it("leaves out what the machine cannot run and every network action of a local-only repository", () => {
    const s = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t" }), ready, "");
    const ids = gitItems(facts(s, { offered: type => type !== "update_from_target" })).map(i => i.id);
    expect(ids).toEqual(["commit", "merge", "archive"]);
  });

  it("turns Git actions off while one runs, but leaves Archive available", () => {
    const f = facts(world(checkout({ files: [file("a.ts")], ahead: 1, behind: 1 }), { status: "done" }), { running: "pull_push", progress: "Pushing…" });
    const items = gitItems(f);
    expect(items.length).toBeGreaterThan(4);
    expect(items.filter(i => i.id !== "archive").every(i => i.why === RUNNING)).toBe(true);
    expect(items.find(i => i.id === "archive")).toMatchObject({ label: "Archive task", why: undefined });
    expect(gitPrimary(f)).toMatchObject({ id: "pull_push", label: "Pushing…", icon: "loader", why: RUNNING });
  });

  it("acts on the project folder without a task, with no task-only items", () => {
    const ids = gitItems(facts(world(checkout({ files: [file("a.ts")] }), null))).map(i => i.id);
    expect(ids).toEqual(["commit", "fetch", "pull", "push", "pull_push"]);
  });
});

describe("a stopped pull, Update from the target and View PR", () => {
  const op = (conflicts: string[]) => ({ kind: "merge" as const, conflicts });
  it("puts a stopped pull first: Resolve conflicts, then Continue, with Abort always there and every other action off", () => {
    const f = facts(world(checkout({ operation: op(["a.ts"]), files: [file("a.ts")] }), null));
    expect(gitPrimary(f)).toMatchObject({ id: "resolve", label: "Resolve conflicts", strong: true });
    const why = Object.fromEntries(gitItems(f).map(i => [i.id, i.why]));
    expect(gitItems(f).map(i => i.id)).toEqual(["commit", "fetch", "pull", "push", "pull_push", "resolve", "continue", "abort_pull"]);
    expect([why.commit, why.pull, why.push, why.pull_push]).toEqual([STOPPED, STOPPED, STOPPED, STOPPED]);
    expect(why.continue).toMatch(/Resolve 1 conflicting file/);
    expect(why.abort_pull).toBeUndefined();
    const done = facts(world(checkout({ operation: { kind: "rebase", conflicts: [] } }), null));
    expect(gitPrimary(done)).toMatchObject({ id: "continue", label: "Continue rebase", why: undefined });
  });
  it("leaves a task's own update conflict to the task", () => {
    const s = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t", operation: op(["a.ts"]) }), { ...ready, conflict: "conflict" });
    expect(gitItems(facts(s)).some(i => i.id === "resolve")).toBe(false);
  });
  it("offers Update from the target only for a finished worktree task behind it, never over uncommitted work or a live worker", () => {
    const at = (patch: Partial<RepoCheckout>, task: object = {}) => gitItems(facts(world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t", behindOf: { main: 2 }, ...patch }), { ...ready, ...task }))).find(i => i.id === "update");
    expect(at({})?.why).toBeUndefined();
    expect(at({ behindOf: { main: 0 } })?.why).toBe("Up to date with main");
    expect(at({ files: [file("a.ts")] })?.why).toMatch(/uncommitted changes/);
    expect(at({}, { process: "live" })?.why).toMatch(/A task is working in this checkout/);
    expect(at({}, { status: "working" })?.why).toBe("Available for a finished task");
  });
  it("shows View PR only for a recorded pull request, and says when its address was not reported", () => {
    const pr = (url?: string) => world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t" }), { ...ready, integration: { method: "pr", targetBranch: "main", digest: "d", ...(url ? { url } : {}) } });
    expect(gitItems(facts(pr("https://github.com/o/r/pull/1"))).find(i => i.id === "view_pr")?.why).toBeUndefined();
    expect(gitItems(facts(pr())).find(i => i.id === "view_pr")?.why).toBe(PR_UNKNOWN);
    expect(gitItems(facts(pr("https://github.com/o/r/pull/1"), { offered: t => t !== "open_pull_request" })).some(i => i.id === "view_pr")).toBe(false);
    expect(gitItems(facts(world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t" }), ready))).some(i => i.id === "view_pr")).toBe(false);
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

function typeInto(field: HTMLTextAreaElement, value: string) {
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
}
const dialogOf = (root: HTMLElement) => root.querySelector<HTMLDialogElement>("dialog[open]")!;
const press = (within: ParentNode, label: string) => [...within.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === label)!.click();
function pendingDraft() {
  let answer!: (text: IntegrationText | null) => void;
  const ready = new Promise<IntegrationText | null>(resolve => { answer = resolve; });
  return { answer, request: vi.fn(() => ready) };
}

it("lists main and linked checkout branches only in their checkout sections", async () => {
  const s = world(checkout(), null);
  const repo = s.repos[0];
  repo.checkouts.push(checkout({ id: "p:linked", kind: "worktree", branch: "linked", path: "/linked" }));
  repo.branches = ["main", "linked", "spare"].map(name => ({ name, where: "", task: null, ahead: 0, behind: 0 }));
  const { root } = await open(s, "/m/local/p/p/worktrees");
  const rows = [...root.querySelectorAll('.hist[aria-label="Branches and worktrees"] .gr')];
  expect(rows.filter(row => row.textContent?.includes("No checkout")).map(row => row.querySelector(".mono")?.textContent)).toEqual(["spare"]);
  expect(rows.map(row => row.querySelector(".mono")?.textContent)).toEqual(["main", "linked", "spare"]);
});

describe("editable Git drafts", () => {
  const detailed = "Fix the race\n\n- Serialize updates to avoid concurrent writes.";
  it.each(["generated", "typed", "failed"])("handles a %s checkout commit draft", async scenario => {
    const p = port(() => ({})), pending = pendingDraft();
    p.port.draftCheckoutCommit = pending.request;
    const state = world(checkout({ files: [file("a.ts", "staged"), file("b.ts")] }), null);
    const { root } = await open(state, "/m/local/p/p/git", p);
    p.emit({ kind: "facts", facts: { projectId: "p", openBranch: "main", defaultBranch: "main", repo: state.repos[0], workers: [], reviews: [], overlays: {}, sprints: [] } });
    await settle();
    const box = root.querySelector(".commit-box")!;
    const field = box.querySelector("textarea")!;
    press(box, "Generate");
    expect(pending.request).toHaveBeenCalledWith({ checkoutId: "p:main" });
    expect(box.textContent).toContain("Writing from the changes…");
    if (scenario === "typed") typeInto(field, "Owner text");
    pending.answer(scenario === "failed" ? { source: "fallback", reason: "changes_unreadable" } : { source: "model", message: detailed });
    await settle();
    if (scenario === "failed") {
      expect(box.textContent).toContain("Git couldn’t read the changes.");
      expect(field.value).toBe("");
      typeInto(field, "Owner text");
    }
    const expected = scenario === "generated" ? detailed : "Owner text";
    expect(field.value).toBe(expected);
    expect(p.commands.filter(c => c.payload.type === "git_draft").map(c => c.payload)).toEqual([
      expect.objectContaining({ message: expected }),
    ]);
    box.querySelector<HTMLButtonElement>(".btn-primary")!.click();
    await settle();
    expect(p.commands.at(-1)?.payload).toMatchObject({ type: "git_commit", message: expected });
  });

  it("shows a detailed fallback and reason in Task Commit", async () => {
    const p = port(() => ({}));
    p.port.draftIntegrationText = async () => ({ source: "fallback", message: detailed, reason: "timeout" });
    const { root } = await open(world(checkout({ files: [file("a.ts")] })), "/m/local/t/t", p);
    gitPrimaryButton(root).click(); await settle();
    expect(dialogOf(root).querySelector("textarea")?.value).toBe(detailed);
    expect(dialogOf(root).textContent).toContain("The model took too long.");
  });

  it("shows a detailed fallback and reason in Finish", async () => {
    const p = port(() => ({}));
    p.port.draftIntegrationText = async () => ({ source: "fallback", message: detailed, reason: "timeout" });
    const { root } = await open(world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t" }), ready), "/m/local/t/t/changes", p);
    gitPrimaryButton(root).click(); await settle();
    expect(dialogOf(root).querySelector("textarea")?.value).toBe(detailed);
    expect(dialogOf(root).textContent).toContain("The model took too long.");
  });

  it.each(["Push", "Typed", "Fallback", "Failed", "Empty"] as const)("sends the confirmed body for %s", async kind => {
    const p = port(() => ({})), pending = pendingDraft();
    p.port.draftPushSummary = pending.request;
    const { root } = await open(world(checkout({ ahead: 1, behind: 1 }), null), "/m/local/p/p/git", p);
    root.querySelector<HTMLButtonElement>('.side-pane [aria-label="Push"]')!.click();
    await settle();
    const dialog = dialogOf(root);
    const field = dialog.querySelector("textarea")!;
    expect(pending.request).toHaveBeenCalledWith({ checkoutId: "p:main", branch: "main", remote: "git@example.invalid:o/p.git" });
    expect(dialog.textContent).toContain("Writing a summary");
    if (kind === "Push" || kind === "Fallback") {
      pending.answer(kind === "Push" ? { source: "model", body: detailed } : { source: "fallback", body: detailed, reason: "timeout" });
      await settle();
      expect(field.value).toBe(detailed);
      expect(dialog.textContent).toContain(kind === "Push" ? "Written from the outgoing commits." : "The model took too long.");
      typeInto(field, "Owner summary");
    } else if (kind === "Typed") {
      typeInto(field, "Owner summary");
      pending.answer({ source: "model", body: detailed });
      await settle();
      expect(field.value).toBe("Owner summary");
    } else if (kind === "Failed") {
      pending.answer({ source: "fallback", reason: "push_baseline_unavailable" });
      await settle();
      expect(dialog.textContent).toContain("Fetch the selected remote");
      typeInto(field, "Owner summary");
    }
    expect(p.commands).toEqual([]);
    press(dialog, "Push"); await settle();
    if (kind === "Empty") pending.answer(null);
    expect(p.commands.map(c => c.payload.type)).toEqual(["git_push"]);
    expect(p.commands.at(-1)?.payload).toMatchObject({ type: "git_push", confirmed: true, summary: kind === "Empty" ? "" : "Owner summary" });
  });

  it.each([false, true])("uses the saved Push summary as the PR description (owner types first: %s)", async manual => {
    const p = port(() => ({})), pending = pendingDraft();
    p.port.draftIntegrationText = pending.request;
    const c = checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t", path: "/w" });
    const { root } = await open(world(c, ready), "/m/local/t/t/changes", p);
    press(root.querySelector(".ready")!, "Create PR…"); await settle();
    const dialog = dialogOf(root), field = dialog.querySelector("textarea")!;
    if (manual) typeInto(field, "My description");
    pending.answer({ source: "fallback", bodySource: "saved_push_summary", title: "Separate title", body: detailed });
    await settle();
    expect(field.value).toBe(manual ? "My description" : detailed);
    expect(dialog.textContent).toContain("Loaded from the confirmed Push summary");
    dialog.querySelector<HTMLButtonElement>('[aria-label="Create PR (no push)"]')!.click(); await settle();
    expect(p.commands.at(-1)?.payload).toMatchObject({ type: "integrate", method: "pr", title: "Separate title", message: manual ? "My description" : detailed });
  });
});

describe("accepted result freshness at Finish", () => {
  it("allows an unchanged committed result from both surfaces, and blocks drift even in an open PR dialog", async () => {
    const c = checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t", path: "/w", upstream: "origin/task/fix" });
    const fresh = world(c, ready);
    const { root, p } = await open(fresh, "/m/local/t/t/changes");
    const cardPr = () => [...root.querySelectorAll<HTMLButtonElement>(".ready button")].find(b => b.textContent === "Create PR…")!;
    const cardMerge = () => [...root.querySelectorAll<HTMLButtonElement>(".ready button")].find(b => b.textContent === "Merge…")!;
    const menu = (label: string) => [...root.querySelectorAll<HTMLButtonElement>("#m-git .mi")].find(b => b.querySelector(".grow")?.firstChild?.textContent === label)!;
    const confirmPr = () => root.querySelector<HTMLButtonElement>('dialog[open] [aria-label="Create PR (no push)"]')!;
    expect(cardPr().getAttribute("aria-disabled")).toBeNull();
    expect(cardMerge().getAttribute("aria-disabled")).toBeNull();
    expect(menu("Create PR…").getAttribute("aria-disabled")).toBeNull();
    cardPr().click(); await settle();
    expect(confirmPr().getAttribute("aria-disabled")).toBeNull();

    p.emit(projection("2", world(c, { ...ready, workspaceDigest: "different" }))); await settle();
    for (const button of [cardPr(), cardMerge(), menu("Create PR…"), confirmPr()]) {
      expect(button.getAttribute("aria-disabled")).toBe("true");
      expect(button.textContent ?? "").not.toContain("View PR");
    }
    expect(confirmPr().getAttribute("data-why")).toContain("stale");
    confirmPr().click(); await settle();
    expect(p.commands).toEqual([]);
    expect(menuLabels(root)).not.toContain("View PR");

    p.emit(projection("3", fresh)); await settle();
    expect(confirmPr().getAttribute("aria-disabled")).toBeNull();
    confirmPr().click(); await settle();
    expect(p.commands.map(c => c.payload)).toEqual([expect.objectContaining({ type: "integrate", method: "pr", digest: "d" })]);
  });
});

describe("View PR in the merge card", () => {
  const url = "https://github.com/example/a-very-long-repository-name-for-a-recorded-pull-request/pull/123";
  const state = (address?: string) => world(
    checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t", path: "/w" }),
    { ...ready, integration: { method: "pr", targetBranch: "main", digest: "d", draft: true, ...(address ? { url: address } : {}) } },
  );
  const row = (root: HTMLElement) => root.querySelector<HTMLElement>(".ready .rv-pr")!;
  const viewPr = (el: HTMLElement) => [...el.querySelectorAll("button")].find(b => b.textContent === "View PR");

  it.each(["merge card", "Git menu"] as const)("shows the recorded address and requests opening it from the %s", async (entry) => {
    const { root, p } = await open(state(url), "/m/local/t/t/changes");
    const pr = row(root);
    expect(pr.querySelector(".l-row-d .mono")?.textContent).toBe(url);
    expect(pr.textContent).toContain("Pull request created into main");
    expect(pr.textContent).toContain("Draft. It is not merged, so this task stays here.");
    expect(pr.textContent).toContain("GitHub CLI");
    // The port fixture receives the persisted URL, not the subprocess stdout line.
    const menuButton = [...root.querySelectorAll<HTMLButtonElement>("#m-git .mi")]
      .find(b => b.querySelector(".grow")?.firstChild?.textContent === "View PR");
    expect(menuButton).toBeDefined();
    const button = entry === "merge card" ? viewPr(pr)! : menuButton!;
    expect(button.getAttribute("aria-disabled")).toBeNull();
    expect(p.commands).toEqual([]);
    button.click(); await settle();
    expect(p.commands.map(c => c.payload)).toEqual([{ type: "open_pull_request", projectId: "p", taskId: "t" }]);
  });

  it("explains a missing address and has no dead button", async () => {
    const { root, p } = await open(state(), "/m/local/t/t/changes");
    expect(row(root).textContent).toContain(PR_UNKNOWN);
    expect(viewPr(row(root))).toBeUndefined();
    expect([...root.querySelectorAll("#m-git [data-why]")].map(e => e.getAttribute("data-why"))).toContain(PR_UNKNOWN);
    expect(p.commands).toEqual([]);
  });

  it("shows an opener refusal and lets the owner retry with the same button", async () => {
    const message = "No browser opener is installed on this machine.";
    const p = port(c => c.payload.type === "open_pull_request" ? { ok: false, outcome: "refused", message } : {});
    const { root } = await open(state(url), "/m/local/t/t/changes", p);
    viewPr(row(root))!.click(); await settle();
    expect(root.querySelector(".ws-receipt")?.textContent).toContain(message);
    viewPr(row(root))!.click(); await settle();
    expect(p.commands.filter(c => c.payload.type === "open_pull_request")).toHaveLength(2);
  });

  it("keeps the address visible when browser opening is not offered", async () => {
    const p = port(() => ({}), undefined, MACHINE_COMMANDS.map(capabilityOf));
    const { root } = await open(state(url), "/m/local/t/t/changes", p);
    expect(row(root).textContent).toContain(url);
    expect(viewPr(row(root))).toBeUndefined();
    expect(menuLabels(root)).not.toContain("View PR");
  });
});

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
    expect(dialog.textContent).toContain("Written from the changes. You can edit it.");
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
    expect(confirm.getAttribute("aria-disabled")).toBe("true");
    typeInto(dialog.querySelector("textarea")!, "Manual commit");
    expect(confirm.getAttribute("aria-disabled")).toBeNull();
  });

  it("on diverged histories asks merge or rebase in its own dialog, and pulls only after the owner confirms", async () => {
    const s = world(checkout({ ahead: 1, behind: 2 }), null);
    const p = port(() => ({}));
    const { root } = await open(s, "/m/local/p/p/board", p);
    const primary = gitPrimaryButton(root);
    expect(primary.getAttribute("aria-label")).toBe("Pull…");
    expect(band(root).querySelector(".hg-sync")?.textContent).toBe("↑1↓2");
    const menu = [...root.querySelectorAll("#m-git [data-why]")].map(e => e.getAttribute("data-why")).join("\n");
    expect(menu).toContain(DIVERGED);
    expect(menu).toContain("The remote has commits you do not have. Pull first.");
    primary.click(); await settle();
    expect(p.commands).toEqual([]);
    const dialog = root.querySelector<HTMLDialogElement>("dialog[open]")!;
    expect(dialog.textContent).toContain("this branch has 1 commit that");
    expect(dialog.textContent).toContain("has 2 commits that this branch does not");
    const radios = [...dialog.querySelectorAll<HTMLInputElement>("input[type=radio]")];
    expect(radios.map(r => r.checked)).toEqual([true, false]);
    radios[1].click(); await settle();
    [...dialog.querySelectorAll("button")].find(b => b.textContent === "Pull with rebase")!.click(); await settle();
    expect(p.commands.map(c => c.payload)).toEqual([expect.objectContaining({ type: "git_pull", checkoutId: "p:main", mode: "rebase" })]);
  });

  it("refuses a merge or rebase pull over uncommitted tracked changes, and offers Commit… instead", async () => {
    const s = world(checkout({ ahead: 1, behind: 2, files: [file("a.ts")] }), null);
    const f = facts(s);
    expect(gitItems(f).find(i => i.id === "pull")?.why).toBe(DIRTY);
    // Untracked files alone do not block: Git refuses to overwrite one.
    expect(gitItems(facts(world(checkout({ ahead: 1, behind: 2, files: [file("n.ts", "untracked")] }), null))).find(i => i.id === "pull")?.why).toBeUndefined();
    expect(gitItems(facts(world(checkout({ ahead: 1, behind: 2, branch: "" }), null))).find(i => i.id === "pull")?.why).toBe(DETACHED);
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
    expect(menuLabels(root)).toEqual(["Commit…", "Fetch", "Pull", "Push", "Pull and push", "Update from main…", "Merge locally…", "Create PR…", "Archive task…"]);
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

  it("opens Update from main and sends the update command only after confirmation", async () => {
    const s = world(checkout({ id: "p:wt", kind: "worktree", branch: "task/fix", taskId: "t", path: "/w", behindOf: { main: 2 } }), ready);
    const p = port(() => ({}), undefined, MACHINE_COMMANDS.map(capabilityOf));
    const { root } = await open(s, "/m/local/t/t", p);
    expect(menuLabels(root)).toContain("Update from main…");
    gitPrimaryButton(root).click(); await settle();
    const finish = root.querySelector<HTMLDialogElement>('dialog[open][data-dialog="Finish task"]')!;
    expect(finish.textContent).toContain("Behind main by 2 commits");
    expect(p.commands).toEqual([]);
    [...finish.querySelectorAll("button")].find(b => b.textContent === "Update from main…")!.click(); await settle();

    expect(p.commands).toEqual([]);
    const update = root.querySelector<HTMLDialogElement>('dialog[open][data-dialog="Update from target"]')!;
    expect(update?.querySelector("h2")?.textContent).toBe("Update task/fix from main?");
    [...update.querySelectorAll("button")].find(b => b.textContent === "Update from main")!.click(); await settle();

    expect(p.commands.map(c => c.payload)).toEqual([
      { type: "update_from_target", taskId: "t", confirmed: true, digest: "d", now: expect.any(Number) },
    ]);
  });
});
