import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type ProjectRepo, type RepoCheckout } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { draftTask, flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.innerHTML = "";
  localStorage.clear();
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

async function settle() { for (let i = 0; i < 8; i++) await flush(); }

function repo(projectId: string, path: string, file: string): ProjectRepo {
  const checkout: RepoCheckout = {
    id: projectId + "-main", projectId, kind: "main", branch: "main", path, taskId: null, label: null,
    base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null,
    files: [{ path: file, status: "M", stage: "unstaged", add: 1, del: 0, hunks: [] }],
  };
  return { projectId, hasGit: true, remote: "", tree: [], sources: {}, commits: [], branches: [], checkouts: [checkout], fetchedAt: 1 };
}

async function mount(wide: boolean, hasGit = true) {
  const media = new EventTarget();
  vi.stubGlobal("matchMedia", (query: string) => Object.assign(new EventTarget(), {
    matches: wide && query.includes("min-width"),
    addEventListener: media.addEventListener.bind(media),
    removeEventListener: media.removeEventListener.bind(media),
  }));
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/new?kind=task&project=p`);
  const state = withTask(emptyWorkspace(), "Running fixture");
  state.machines[0].connection = "connected";
  Object.assign(state.projects[0], { hasGit, openBranch: hasGit ? "main" : "", defaultBranch: hasGit ? "main" : "" });
  Object.assign(state.tasks[0], { executionId: "running", status: "working", process: "live" });
  state.tasks.push({ ...draftTask(), id: "stale", executionId: "stale-execution", title: "Uncertain fixture", status: "working", process: "unknown" });
  state.projects.push({ ...state.projects[0], id: "q", repository: "/q", name: "Unrelated project" });
  state.tasks.push({ ...state.tasks[0], projectId: "q", id: "elsewhere", executionId: "elsewhere-execution", title: "Unrelated running task" });
  const port = scriptedPort({ capabilities: { everything: true, operations: [] } });
  Object.defineProperty(port.port, "readsFacts", { value: true });
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={port.port} />, root);
  await settle();
  port.emit(projection("1", state));
  await settle();
  return { ...port, resize(matches: boolean) {
    media.dispatchEvent(Object.assign(new Event("change"), { matches }));
  } };
}

const panel = () => document.querySelector<HTMLElement>(".task-workspace")!;

it("docks the new task panel and preserves the machine inspector, with only selected-project facts", async () => {
  const port = await mount(true);
  expect(panel().closest("aside.inspector")).not.toBeNull();
  const detail = document.querySelector<HTMLDetailsElement>(".task-workspace__machine")!;
  expect(detail.firstElementChild?.textContent).toBe("Machine details");
  expect(detail.open).toBe(false);
  (detail.firstElementChild as HTMLElement).click();
  expect(detail.open).toBe(true);
  expect(panel().textContent).toContain("No checkout status has been read yet.");
  expect(panel().textContent).toContain("Other task states have not been confirmed yet.");

  const emitFacts = (factsRepo: ProjectRepo) => port.emit({ kind: "facts", facts: {
    projectId: factsRepo.projectId, openBranch: "main", defaultBranch: "main", repo: factsRepo, workers: [], reviews: [], sprints: [],
    overlays: factsRepo.projectId === "p" ? { t: { checkout: { kind: "main", branch: "main" } } } : {},
  } });
  emitFacts(repo("q", "/q", "not-this-project.txt"));
  await settle();
  expect(panel().textContent).toContain("No checkout status has been read yet.");

  emitFacts(repo("p", "/different-checkout", "wrong-checkout.txt"));
  await settle();
  expect(panel().textContent).not.toContain("wrong-checkout.txt");

  emitFacts(repo("p", "/p", "changed.txt"));
  await settle();
  expect(panel().textContent).toContain("changed.txt");
  expect(panel().textContent).not.toContain("not-this-project.txt");
  expect(panel().textContent).not.toContain("Unrelated running task");
  const tasks = [...panel().querySelectorAll(".task-workspace__tasks li")].map(item => item.textContent);
  expect(tasks.sort()).toEqual(["Running fixtureActiveProject folder", "Uncertain fixtureUnknownCheckout not confirmed"]);
});

it("keeps controls inline at narrow widths and saves the selected checkout without starting", async () => {
  const port = await mount(false);
  expect(panel().closest(".task-workspace__inline")).not.toBeNull();
  const modes = panel().querySelectorAll<HTMLInputElement>('input[type="radio"]');
  modes[1].click();
  expect(panel().textContent).toContain("including uncommitted changes");
  modes[0].click();
  expect(panel().textContent).toContain("committed HEAD");
  const textarea = document.querySelector<HTMLTextAreaElement>('[aria-label="Describe the task"]')!;
  textarea.value = "Keep this saved request";
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  document.querySelector<HTMLButtonElement>('button[aria-label="Save draft"]')!.click();
  await settle();
  expect(port.commands.map(command => command.payload)).toEqual([
    expect.objectContaining({ type: "create_task", projectId: "p", description: "Keep this saved request", checkout: expect.objectContaining({ kind: "worktree" }) }),
  ]);
});

it("explains a non-Git project and keeps its project-folder mode available", async () => {
  await mount(false, false);
  expect(panel().textContent).toContain("Git changes are unavailable in this folder.");
  expect(panel().querySelector('input[type="radio"]:disabled')).not.toBeNull();
  expect(panel().querySelector<HTMLInputElement>('input[type="radio"]:checked')?.disabled).toBe(false);
});

it("moves between the dock and inline without changing the owner's inspector or checkout choices", async () => {
  const port = await mount(true);
  const dock = () => panel().closest("[data-task-workspace-dock]");
  expect(dock()).not.toBeNull();
  panel().querySelectorAll<HTMLInputElement>('input[type="radio"]')[0].click();
  port.resize(false);
  await settle();
  expect(panel().closest(".task-workspace__inline")).not.toBeNull();
  port.resize(true);
  await settle();
  expect(dock()).not.toBeNull();
  expect(panel().querySelector<HTMLInputElement>('input[type="radio"]:checked')).toBe(panel().querySelector('input[type="radio"]'));

  const inspector = document.querySelector<HTMLButtonElement>('button[aria-label="Inspector"]')!;
  inspector.focus();
  inspector.click();
  await settle();
  expect(document.activeElement).toBe(inspector);
  expect(panel().closest(".task-workspace__inline")).not.toBeNull();
  port.resize(false);
  await settle();
  port.resize(true);
  await settle();
  expect(inspector.getAttribute("aria-pressed")).toBe("false");
  expect(panel().closest(".task-workspace__inline")).not.toBeNull();
  inspector.click();
  await settle();
  expect(document.activeElement).toBe(inspector);
  expect(dock()).not.toBeNull();
  expect(panel().textContent).toContain("committed HEAD");
});
