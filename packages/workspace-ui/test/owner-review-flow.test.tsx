import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Activity, type ProjectRepo, type RepoCheckout, type TimelineEntry, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { clock } from "../src/selectors";
import { conversationOf } from "../src/port/history";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const buttons = () => [...document.querySelectorAll<HTMLButtonElement>("button, a.btn")];
const button = (text: string) => buttons().find(b => b.textContent?.trim() === text);
const text = () => document.body.textContent ?? "";
const world = () => { const s = withTask(emptyWorkspace(), "Work"); s.machines[0].connection = "connected"; return s; };
async function mount(state: WorkspaceState, path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const taskPath = `/m/${LOCAL_MACHINE_ID}/t/t`;
const inReview = (checks: WorkspaceState["tasks"][number]["result"] extends infer R ? R extends { checks: infer C } ? C : never : never) => {
  const s = world(), t = s.tasks[0];
  t.status = "review"; t.workspaceDigest = "digest";
  t.result = { version: 1, digest: "digest", summary: "Created the landing page docs", checks };
  return s;
};
const checkout = (patch: Partial<RepoCheckout> = {}): RepoCheckout => ({
  id: "p:main", projectId: "p", kind: "main", branch: "main", path: "/p", base: "main", ahead: 0, behind: 0, behindOf: {}, taskId: null,
  files: [], draft: "", sources: {}, ...patch,
} as RepoCheckout);
const repo = (patch: Partial<ProjectRepo> = {}): ProjectRepo => ({ projectId: "p", remote: "", tree: [], sources: {}, commits: [], branches: [], checkouts: [checkout()], fetchedAt: null, ...patch });

it("review shows the result, checks in plain words and a working way forward when Accept is blocked", async () => {
  const p = await mount(inReview([{ name: "Focused tests", detail: "Exit 1", outcome: "failed", command: ["pytest"], rerunnable: true }, { name: "Lint", detail: "", outcome: "passed" }]), taskPath + "/changes");
  const bar = document.querySelector(".rv-bar")!;
  expect(bar).not.toBeNull();
  expect(document.querySelector(".rv-sum")?.textContent).toContain("Created the landing page docs");
  expect(bar.textContent).toContain("1 passed, 1 failed");
  expect(bar.textContent).toMatch(/Run the unpassed checks first/);
  expect(bar.textContent).toContain("Request changes…");
  const fix = [...bar.querySelectorAll("button")].find(b => b.textContent?.trim() === "Rerun the failed check")!;
  expect(fix).toBeTruthy();
  fix.click(); await settle();
  expect(p.commands.map(c => c.operation)).toContain("rerun_check");
});

it("manual checks are separate, uncounted and never offer rerun or block Accept", async () => {
  await mount(inReview([{ name: "Screen reader and visual judgement", detail: "Check the finished page", outcome: "not_run", kind: "manual", rerunnable: false }]), taskPath + "/changes");
  expect(text()).toContain("Check manually");
  expect(text()).toContain("Screen reader and visual judgement");
  expect(text()).not.toContain("1 not run");
  expect(button("Rerun check")).toBeUndefined();
  expect(button("Accept v1")?.getAttribute("aria-disabled")).toBeNull();
});

it("fresh successful check output remains readable after the failure clears", async () => {
  await mount(inReview([{ name: "Focused tests", detail: "", outcome: "passed", kind: "automated", command: ["python3", "check.py"], cwd: "/p/checks", rerunnable: true, output: "All assertions passed", exitCode: 0 }]), taskPath + "/changes");
  expect(text()).toContain("All assertions passed");
  expect(text()).toContain("Working folder: /p/checks");
  expect(text()).toContain("Command and output");
});

it("policy checks remain explicitly not run without a misleading rerun", async () => {
  await mount(inReview([{ name: "Paid browser task", detail: "", outcome: "not_run", kind: "policy", technicalReason: "Paid live check not requested by the owner", rerunnable: false }]), taskPath + "/changes");
  expect(text()).toContain("Paid live check not requested by the owner");
  expect(button("Rerun check")).toBeUndefined();
});

it("review offers Accept as the primary action that moves the task to Ready to merge", async () => {
  const p = await mount(inReview([{ name: "Focused tests", detail: "", outcome: "passed" }]), taskPath + "/changes");
  const bar = document.querySelector(".rv-bar")!;
  expect(bar.textContent).toContain("Accept moves the task to Ready to merge");
  const accept = [...bar.querySelectorAll<HTMLButtonElement>("button.btn-primary")].find(b => b.textContent?.includes("Accept v1"))!;
  expect(accept.getAttribute("aria-disabled")).toBeNull();
  // Request changes and Accept are one row of one bar, and the explanation sits in that bar's slot, not under Accept.
  expect([...bar.querySelectorAll("[data-bar] .l-bar-actions > .btn")].map(b => b.textContent?.trim())).toEqual(["Request changes…", "Accept v1"]);
  expect(bar.querySelector("[data-bar] .l-reason")?.textContent).toContain("Accept moves the task to Ready to merge");
  expect(document.querySelector('[aria-label="Review comments"]')).not.toBeNull();
  accept.click(); await settle();
  expect(p.commands.map(c => c.operation)).toContain("accept_result");
});

it("an Accept refusal remains beside the result with a next action after the toast is dismissed", async () => {
  const p = await mount(inReview([{ name: "Focused tests", detail: "", outcome: "passed" }]), taskPath + "/changes");
  button("Accept v1")!.click(); await settle();
  p.receipt(false); await settle();
  const message = document.querySelector(".ws-receipt")!.textContent;
  document.body.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true })); await settle();
  expect(document.querySelector(".ws-receipt")).toBeNull();
  expect(document.querySelector(".rv-accept-error")?.textContent).toContain(message);
  expect(button("Refresh result")).toBeDefined();
});

it("a project without Git says so calmly and offers an empty repository", async () => {
  const s = world(); s.projects[0].hasGit = false;
  const p = await mount(s, `/m/${LOCAL_MACHINE_ID}/p/p/git`);
  expect(text()).toContain("This project has no Git repository");
  expect(text()).not.toMatch(/No Git record/);
  button("Create empty repository")!.click(); await settle();
  const confirm = [...document.querySelectorAll<HTMLButtonElement>("dialog .btn-primary")].find(b => b.textContent?.includes("Create repository"))!;
  confirm.click(); await settle();
  const sent = p.commands.find(c => c.operation === "git_init");
  expect(sent?.payload).toMatchObject({ type: "git_init", projectId: "p" });
});

it("review in a project without Git says the changes are in the folder and offers no Git actions", async () => {
  const s = inReview([{ name: "Focused tests", detail: "", outcome: "passed" }]); s.projects[0].hasGit = false;
  await mount(s, taskPath + "/changes");
  expect(text()).toContain("Changes were applied in the project folder");
  expect(button("Commit…")).toBeUndefined();
  expect(button("Merge…")).toBeUndefined();
  // Comments need a line of a diff: no comment bar, no empty panel, and the folder note once.
  const panel = document.querySelector('[aria-label="Changes"]')!;
  expect(document.querySelector('[aria-label="Review comments"]')).toBeNull();
  expect(button("Send")).toBeUndefined();
  expect(panel.querySelectorAll(".note-panel")).toHaveLength(0);
  expect(panel.textContent!.match(/Changes were applied in the project folder/g)).toHaveLength(1);
});

it("Request changes keeps Send and reopen a real button, with its reason in the composer's reason slot", async () => {
  await mount(inReview([{ name: "Focused tests", detail: "", outcome: "passed" }]), taskPath + "/changes");
  button("Request changes…")!.click(); await settle();
  // The shared composer's paper plane, named for what it does; its reason stays outside the button, in the bar's slot.
  const send = document.querySelector<HTMLButtonElement>('dialog[open] button[aria-label="Send and reopen"]')!;
  const slot = document.querySelector("dialog[open] .c-bar .l-reason")!;
  expect(send.tagName).toBe("BUTTON");
  expect(send.getAttribute("aria-disabled")).toBe("true");
  expect(slot.textContent).toBe("Say what should change");
  expect(send.getAttribute("aria-describedby")).toBe(slot.id);
  const field = document.querySelector<HTMLTextAreaElement>("dialog[open] textarea")!;
  field.value = "Keep the old menu order"; field.dispatchEvent(new Event("input", { bubbles: true })); await settle();
  expect(send.getAttribute("aria-disabled")).toBeNull();
  expect(slot.textContent).toBe("");
});

it("a refused action's message never stays on screen", async () => {
  const s = world(); s.repos = [repo({ checkouts: [checkout({ files: [{ path: "a.md", status: "M", stage: "staged", add: 1, del: 0, hunks: [] } as never] })] })];
  s.repos[0].checkouts[0].draft = "Update a.md";
  const p = await mount(s, `/m/${LOCAL_MACHINE_ID}/p/p/git`);
  document.querySelector<HTMLButtonElement>(".commit-box .btn-primary")!.click(); await settle();
  p.receipt(false); await settle();
  expect(document.querySelector(".ws-receipt")).not.toBeNull();
  document.body.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
  await settle();
  expect(document.querySelector(".ws-receipt")).toBeNull();
});

it("a local-only repository offers no remote controls, so an empty push cannot be opened", async () => {
  const s = world(); s.repos = [repo()];
  await mount(s, `/m/${LOCAL_MACHINE_ID}/p/p/git`);
  for (const label of ["Fetch", "Pull", "Push"]) expect(document.querySelector(`button[aria-label="${label}"]`)).toBeNull();
  expect(document.querySelector("dialog[open]")).toBeNull();
});

it("Commit with nothing staged delegates staging and commit in one request", async () => {
  const files = [{ path: "a.md", status: "U", stage: "untracked", add: 1, del: 0, hunks: [] }, { path: "b.md", status: "U", stage: "untracked", add: 1, del: 0, hunks: [] }] as never;
  const s = world(); s.repos = [repo({ checkouts: [checkout({ files, draft: "Add docs" })] })];
  const p = await mount(s, `/m/${LOCAL_MACHINE_ID}/p/p/git`);
  const commit = document.querySelector<HTMLButtonElement>(".commit-box .btn-primary")!;
  expect(commit.getAttribute("aria-disabled")).toBeNull();
  commit.click(); await settle();
  expect(p.commands.map(c => c.operation)).toEqual(["git_commit"]);
  p.receipt(true); await settle();
  expect(p.commands.map(c => c.operation)).toEqual(["git_commit"]);
});

it("shows times in the computer's own time zone", () => {
  const before = process.env.TZ;
  process.env.TZ = "Asia/Tbilisi";
  try { expect(clock(Date.UTC(2026, 0, 1, 22, 0))).toBe("02:00"); }
  finally { process.env.TZ = before; }
});

it("worker activity hides internal steps, names tools plainly and expands to input and output", async () => {
  const s = world(); s.tasks[0].status = "working"; s.tasks[0].process = "live"; s.tasks[0].executionId = "e";
  const activity: Activity[] = [
    { sequence: 1, kind: "note", text: "mcp.cortex.project_read", at: 1 },
    { sequence: 2, kind: "note", text: "context 15721/475000", at: 2 },
    { sequence: 3, kind: "command", text: "mcp.cortex.read_skill", code: "mcp.cortex.read_skill", at: 3, command: { state: "native", exit: null, took: null, tail: "", reason: null } },
    { sequence: 4, kind: "tool", text: "mcp__cortex__project_edit", args: "docs/a.md", output: "Edited 1 file", at: 4 },
  ];
  const worker: Worker = { id: "w", taskId: "t", executionId: "e", role: "ux-designer", status: "working", lane: "execution", goal: "Design", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity, result: null };
  s.workers = [worker];
  await mount(s, taskPath + "/workers/w");
  const stream = document.querySelector(".feed, .stream")!;
  expect(stream.textContent).not.toMatch(/mcp[._]/);
  expect(stream.textContent).not.toContain("context 15721");
  expect(stream.textContent).not.toContain("Native, no receipt");
  (stream.querySelector("button.fold") as HTMLButtonElement).click(); await settle();
  expect(stream.textContent).toContain("Read");
  expect(stream.textContent).toContain("Read skill");
  const edit = [...stream.querySelectorAll<HTMLButtonElement>("button.ev-toggle")].find(b => b.textContent?.includes("Edit"))!;
  expect(edit.getAttribute("aria-expanded")).toBe("false");
  edit.click(); await settle();
  expect(edit.getAttribute("aria-expanded")).toBe("true");
  expect(stream.textContent).toContain("docs/a.md");
  expect(stream.textContent).toContain("Edited 1 file");
});

it("the conversation shows workers once: worker launches stay in the Workers card", () => {
  const rows = [
    { historyId: "1", revision: "1", kind: "tool", text: "Worker: ux-designer · codex · 6.1 Sol — Produce the art direction", recordedAt: "2026-11-07T22:00:00Z" },
    { historyId: "2", revision: "1", kind: "tool", text: "mcp__cortex__read_report", recordedAt: "2026-11-07T22:00:05Z" },
  ];
  const out = conversationOf(rows, { description: "", answers: [] }).map(d => d.entry) as TimelineEntry[];
  const fold = out.find(e => e.kind === "fold") as Extract<TimelineEntry, { kind: "fold" }>;
  expect(fold.steps.join(" ")).not.toContain("Worker:");
  expect(fold.text).toContain("Read 1 file");
});

it("a busy button keeps its fill and size and shows a spinner inside", async () => {
  const css = (await import("node:fs")).readFileSync(`${process.cwd()}/${(await import("node:fs")).existsSync("src/workspace.css") ? "" : "packages/workspace-ui/"}src/workspace.css`, "utf8");
  expect(css).toMatch(/\.btn-primary\[data-busy\]\[disabled\][^{]*\{[^}]*background: var\(--primary\)/);
});
