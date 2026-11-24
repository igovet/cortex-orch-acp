import "./fixture-storage";
import { render } from "solid-js/web";
import { emptyWorkspace, type Sprint, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import type { PhoneFrame } from "../src/ctx";
import { draftTask, projection, scriptedPort } from "./support";

// Disposable fake data for capturing the sidebar's unseen-change marks: a first read taken as seen, then a read in which some
// rows changed. `?phone` renders the phone shell; its drawer opens from the Menu button. No service, engines or real stores:
// the page keeps its storage in memory (`fixture-storage`), so every load starts empty and the app's own storage is untouched.
const phone = new URLSearchParams(location.search).has("phone");
const task = (id: string, projectId: string, title: string, patch: Partial<Task> = {}): Task => ({ ...draftTask(), id, projectId, title, status: "working", ...patch });
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({ id: "sp", projectId: "p", description: "Checkout redesign", title: "Checkout redesign", runMode: "manual", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["s1", "s2"], queueState: "running", pauseReason: null, ...patch } as Sprint);
const world = (tasks: Task[], sprints: Sprint[]): WorkspaceState => {
  const s = emptyWorkspace();
  s.machines[0].connection = "connected";
  const project = (id: string, name: string) => ({ id, machineId: s.machines[0].id, name, repository: `/home/owner/${id}`, openBranch: "main", defaultBranch: "main" });
  return { ...s, projects: [project("p", "Storefront"), project("q", "Documentation site")], tasks, sprints };
};
const before = world([
  task("a", "p", "Fix the login race on slow networks"), task("b", "p", "Add a dark theme to the settings page"), task("c", "p", "Rename the billing tables"),
  task("s1", "p", "Cart summary layout"), task("s2", "p", "Payment step errors"), task("d", "q", "Rewrite the install guide"),
], [sprint()]);
const after = world([
  task("a", "p", "Fix the login race on slow networks", { status: "needs_you", issue: "question: Should a retry keep the typed password?" }),
  task("b", "p", "Add a dark theme to the settings page", { status: "done", doneAt: Date.now() }),
  task("c", "p", "Rename the billing tables"),
  task("s1", "p", "Cart summary layout", { status: "done", doneAt: Date.now() }), task("s2", "p", "Payment step errors", { status: "done", doneAt: Date.now() }),
  task("d", "q", "Rewrite the install guide", { status: "needs_you", issue: "question: Keep the old screenshots?" }),
], [sprint({ queueState: "complete" })]);

const shell: PhoneFrame | undefined = phone ? {
  kind: "phone", machine: () => ({ name: "Workstation", state: "connected", path: "direct", lastConnectedAt: Date.now() }),
  openMachines() {}, openSettings() {}, retry() {}, attach() {},
} : undefined;
const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
history.replaceState(null, "", `/m/${before.machines[0].id}`);
render(() => <Workspace port={s.port} shell={shell} />, document.getElementById("root")!);
setTimeout(() => s.emit(projection("1", before)), 50);
setTimeout(() => { s.emit(projection("2", after)); document.documentElement.dataset.fixture = "ready"; }, 400);
