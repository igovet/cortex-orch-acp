import { render } from "solid-js/web";
import { emptyWorkspace, LOCAL_MACHINE_ID, type PlanItem } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, scriptedTransport, snapshot, wireTask } from "./support";
import "../src/mockup-styles.css";
import "../src/workspace.css";

// Disposable, opt-in fake data over the real port: long names everywhere a row could break. No service, engines or real stores.
const BRANCH = "feature/an-extremely-long-branch-name-that-keeps-going-well-past-any-sensible-width";
const BASE = "origin/release/2026-11-a-very-long-upstream-branch-name-for-the-summary-line";
const DIR = "apps/api/scripts/a-deeply-nested-directory/with-another-long-directory-name";
const NAMES = ["canonical-display-name-dev-repair-with-a-very-long-suffix.mjs", "canonical-exact-label-dev-repair-and-one-more-long-suffix.mjs", "canonical-pricing-label-decisions-from-the-review-round.json", "audit-company-websites-across-every-known-market-segment.mjs", "check-model-attribution-browser-with-an-unreasonable-name.mjs"];
const tree = [".prettierrc", "README.md", ...NAMES.map(n => `${DIR}/${n}`), ...Array.from({ length: 24 }, (_, i) => `${DIR}/generated-module-${i}-${"x".repeat(60)}.ts`)];
const changed = NAMES.map(n => `${DIR}/${n}`);
const hunk = { header: "@@ -1 +1,2 @@", staged: false, lines: [{ kind: "ctx", old: 1, new: 1, text: "line" }, { kind: "add", old: null, new: 2, text: "added" }] };
const graph = { paths: [], dot: { cls: "d1", x: 12 } };
const repoFacts = {
  projectId: "p", remote: "git@example.invalid:owner/a-repository-with-a-long-name.git", tree, sources: Object.fromEntries(tree.map(path => [path, "the first line of this file\nthe second line of this file\n"])), fetchedAt: 1000, openBranch: BRANCH, defaultBranch: "main",
  branches: [{ name: `${BRANCH}-spare`, where: "No checkout, an old branch left over from an earlier piece of work on this project", task: "A task title that is also very long and has no reason to fit in its column at all", ahead: 12, behind: 340 }],
  commits: Array.from({ length: 12 }, (_, i) => ({ sha: `abc${i}def`, message: `Normalize the moderation wrappers and everything around them, round ${i}, with a message nobody can read in one line`, author: "A person with a remarkably long name", when: `${i + 12} hours ago`, branch: BRANCH, taskId: null, tags: i % 3 ? [] : ["release-candidate-with-a-long-tag"], files: 3, add: 10, del: 2, pushed: true, graph })),
  checkouts: [
    { id: "p:main", projectId: "p", kind: "main", branch: BRANCH, path: "/home/owner/projects/a-repository-with-a-long-name", taskId: null, label: null, base: BASE, upstream: BASE, ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null,
      files: changed.map((path, i) => ({ path, status: i % 2 ? "M" : "U", add: 10, del: 2, hunks: [hunk], stage: i < 2 ? "staged" : i < 4 ? "unstaged" : "untracked" })) },
    { id: "p:wt", projectId: "p", kind: "worktree", branch: `${BRANCH}-worktree`, path: "/home/owner/.cortex/worktrees/a-worktree-with-a-long-path-of-its-own", taskId: null, label: null, base: BASE, ahead: 3, behind: 1, behindOf: {}, draft: "", sources: {}, updateConflict: null, files: [] },
  ],
};
const OWNER = "1. Make the dashboard simpler\n2. Load everything, **without** a limit\n\nFirst line of the note\nSecond line of the note\n\n- one\n- two";
const items: PlanItem[] = [
  { key: "a", title: "Write the brief", description: "# Goal\n\nFirst line\nSecond line\n\n1. Inspect\n2. Change", acceptance: "- Links work\n- `pnpm test` passes", dependsOn: [] },
  { key: "b", title: "Build the page", description: "Build it.", dependsOn: ["a"] },
];
const sprint = { id: "sp", projectId: "p", description: OWNER, title: "A sprint", runMode: "manual", targetBranch: null, planning: "ready", planningTaskId: "task-1", materializedVersion: null, plan: { version: 1, items }, conversation: [{ kind: "note", text: "Planner line one\nPlanner line two\n\n- **first** point" }, { kind: "message", text: "Owner line one\nOwner line two" }], taskIds: [], queueState: "idle", pauseReason: null };
const config = structuredClone(emptyWorkspace().machines[0].config);
const s = scriptedTransport({
  "service.capabilities": () => ({ methods: ["machine.call", "native.open_editor"], machine: true }),
  "projects.list": () => [{ projectId: "p", displayName: "A project with a rather long display name", canonicalRoot: "/home/owner/projects/a-repository-with-a-long-name", activation: {} }],
  "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "results.get", "executions.get", "history.page"]),
  "project.snapshot": () => snapshot("4", [wireTask({ executionId: "e", phase: "not_started", runtimeState: null, executionState: "not_started" }), wireTask({ taskId: "task-2", description: OWNER, title: "A task", titleAvailability: "generated", executionId: "e2", phase: "not_started", runtimeState: null, executionState: "not_started" })]),
  "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "4" }),
  "history.page": () => ({ items: [], nextCursor: null, highWatermark: "0" }),
  "machine.call": message => {
    const { op, params } = message as { op: string; params: { file: string } };
    if (op === "repo.snapshot") return { ok: true, result: repoFacts };
    if (op === "product.read") return { ok: true, result: structuredClone({ tasks: { "task-1": { planning: true } }, sprints: [sprint], messages: [] }) };
    if (op === "workers") return { ok: true, result: [] };
    if (op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
    if (op === "config.read") return { ok: true, result: { config, connections: [], tokens: {}, usage: [] } };
    if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
    if (op === "repo.file") return { ok: true, result: { file: params.file, text: "{\n  \"singleQuote\": true\n}\n", revision: "r1", readOnly: null, writer: null } };
    return { ok: true, result: {} };
  },
});
const M = LOCAL_MACHINE_ID;
const VIEW: Record<string, string> = { git: `/m/${M}/p/p/git`, code: `/m/${M}/p/p/code?file=${encodeURIComponent(changed[0])}`, sprint: `/m/${M}/p/p/s/sp/planning`, task: `/m/${M}/t/task-2` };
const query = new URLSearchParams(location.search);
history.replaceState(null, "", VIEW[query.get("view") ?? "git"] ?? VIEW.git);
render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, document.getElementById("root")!);
(window as unknown as { fixture: string | null }).fixture = query.get("load");
