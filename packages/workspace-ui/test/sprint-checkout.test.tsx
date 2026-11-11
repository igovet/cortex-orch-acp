import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { LOCAL_MACHINE_ID, type WorkspaceCommand } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { pagesWorld } from "./pages-world";
import { flush, projection, scriptedPort } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); sessionStorage.clear(); });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
const named = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
async function mount(path: string, hasGit = true, worktrees = true) {
  const p = scriptedPort({ capabilities: { everything: worktrees, operations: worktrees ? [] : ["workspace.create_sprint", "tasks.create"] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/${path}`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root); await settle();
  const world = pagesWorld();
  world.projects[0].hasGit = hasGit;
  world.machines[0].config.surfaces.user_defaults.checkout = "worktree";
  world.sprints[0].checkout = { kind: "worktree" };
  p.emit(projection("1", world)); await settle();
  return { p, world };
}
const choose = async (kind: "main" | "worktree") => {
  const labels = [...document.querySelectorAll<HTMLLabelElement>("label.rc")];
  const input = labels.find(l => l.textContent?.includes(kind === "main" ? "Project folder" : "Separate worktree"))!.querySelector<HTMLInputElement>("input")!;
  input.click(); await settle();
};

it.each(["main", "worktree"] as const)("saves %s for a new sprint using the same composer and checkout controls as a task", async kind => {
  const { p } = await mount("new?kind=sprint&project=p");
  expect(document.querySelector('input[name="checkout"]:checked')?.closest("label")?.textContent).toContain("Separate worktree");
  await choose(kind);
  expect(named("Attach files")).toBeDefined();
  expect(named("Voice input")).toBeDefined();
  const area = document.querySelector<HTMLTextAreaElement>(".mc textarea")!;
  area.value = "Make these two changes"; area.dispatchEvent(new Event("input", { bubbles: true })); await settle();
  named("Start planning").click(); await settle();
  expect(p.commands.map(c => c.payload)).toContainEqual(expect.objectContaining({ type: "create_sprint", checkout: { kind }, description: "Make these two changes" }));
  p.receipt(false); await settle();
  expect(area.value).toBe("Make these two changes");
  expect(document.querySelector('input[name="checkout"]:checked')?.closest("label")?.textContent).toContain(kind === "main" ? "Project folder" : "Separate worktree");
  expect(p.commands.some(c => (c.payload as WorkspaceCommand).type === "start_task")).toBe(false);
});

it.each([false, true])("falls back to Project folder when worktrees are unavailable (Git: %s)", async hasGit => {
  await mount("new?kind=sprint&project=p", hasGit, false);
  const inputs = [...document.querySelectorAll<HTMLInputElement>('input[name="checkout"]')];
  expect(inputs[0].disabled).toBe(true);
  expect(inputs[1].checked).toBe(true);
});

it.each(["planning", "summary"] as const)("shows the saved choice on the sprint %s page and keeps legacy mode unknown", async tab => {
  const { p, world } = await mount(`p/p/s/sp/${tab}`);
  expect(document.querySelector('[aria-label="Sprint checkout"]')?.textContent).toBe("Tasks: Separate worktree per task");
  const updated = structuredClone(world);
  updated.sprints[0].checkout = { kind: "main" };
  p.emit(projection("2", updated)); await settle();
  expect(document.querySelector('[aria-label="Sprint checkout"]')?.textContent).toBe("Tasks: Project folder");
  const legacy = structuredClone(updated);
  delete legacy.sprints[0].checkout;
  p.emit(projection("3", legacy)); await settle();
  expect(document.querySelector('[aria-label="Sprint checkout"]')).toBeNull();
});
