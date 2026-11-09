import { render } from "solid-js/web";
import { emptyWorkspace, emptySession, type Activity, type Worker } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { withTask } from "./support";
import "../src/mockup-styles.css";
import "../src/workspace.css";

// Disposable, opt-in fake data and scripted UI responses. No service, engines or real stores.
const s = withTask(emptyWorkspace(), "Inspect the landing page");
s.machines[0].connection = "connected";
Object.assign(s.tasks[0], { status: "working", process: "live", executionId: "e", coordinatorRoute: { model: "gpt-6-luna", effort: "medium" } });
const step = (detail: object) => JSON.stringify(detail);
const command = "python3 -B -c 'import html.parser as H, os, re\ns = open(\"index.html\", encoding=\"utf-8\").read()\nclass P(H.HTMLParser):\n    def handle_starttag(self, t, a):\n        self.tags.append((t, dict(a)))'";
const activity: Activity[] = [
  { sequence: 1, kind: "note", text: "Looking at what is already in the project.", at: 1000 },
  { sequence: 2, kind: "tool", text: "mcp__cortex__project_read", at: 1200, args: step({ kind: "read", title: "Read", path: "src/cortex_runtime/orchestration/gateway.py" }) },
  { sequence: 3, kind: "tool", text: "mcp__cortex__project_read", at: 1500, args: step({ kind: "read", title: "Read", path: "packages/workspace-ui/src/selectors.ts" }) },
  { sequence: 4, kind: "command", text: command, at: 2000, args: step({ kind: "execute", title: "Shell", command, output: "12 tags checked\nno problems" }), command: { state: "native", exit: null, took: null, tail: "12 tags checked\nno problems", reason: null } },
  { sequence: 5, kind: "tool", text: "mcp__codebase-memory-mcp__search_graph", at: 2600 },
  { sequence: 6, kind: "edit", text: "mcp__cortex__project_edit", at: 22000, args: step({ kind: "edit", title: "Edit", path: "index.html", oldText: "<h1>Old</h1>", newText: "<h1>Coffee</h1>" }), diff: { add: 1, del: 1, lines: ["-<h1>Old</h1>", "+<h1>Coffee</h1>"] } },
  { sequence: 7, kind: "note", text: "The page builds cleanly. Writing the styles now.", at: 23000 },
  { sequence: 8, kind: "tool", text: "mcp__cortex__project_read", at: 24000, args: step({ kind: "read", title: "Read", path: "styles.css" }) },
  { sequence: 9, kind: "command", text: "npm test", at: 25000, error: true, args: step({ kind: "execute", title: "Shell", command: "npm test", output: "1 failed", status: "failed" }), command: { state: "failed", exit: 1, took: null, tail: "1 failed", reason: null } },
  { sequence: 10, kind: "note", text: "One test failed, so I fixed the selector and it passes.", at: 40000 },
];
const worker = (patch: Partial<Worker>): Worker => ({ id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "completed", lane: "execution", goal: "Create index.html, styles.css and script.js at the repository root", now: null, waitsFor: null, scope: [], route: { model: "opus-5.5", effort: "high" }, attempt: { n: 1, of: 1 }, elapsedMs: 83000, runningSince: null, context: null, usage: 42000, activity, result: { summary: "The one-page landing is built and checked. It loads nothing from the network, and clicking the call-to-action moves to the contacts block.", checks: [{ name: "Render at 375, 768 and 1440 px", outcome: "passed", detail: "", source: "worker" }], reportId: "r_fixture", edits: [{ path: "index.html", add: 42, del: 3 }] } as Worker["result"], ...patch });
s.workers = [
  worker({}),
  worker({ id: "w2", role: "reviewer", status: "working", goal: "Review the landing page", result: null, route: null, runningSince: null, elapsedMs: 12000, activity: [{ sequence: 20, kind: "note", text: "Reading the change.", at: 1000 }, { sequence: 21, kind: "tool", text: "mcp__cortex__read_report", at: 2000 }] }),
];
const session = emptySession("t");
session.timeline = [
  { id: "said", kind: "cortex", text: "Starting with what is already in the project." },
  { id: "fold", kind: "fold", text: "Worked 21s, used 3 other tools and read 1 file", steps: [`Workspace step: ${step({ kind: "read", title: "Read", path: "." })}`, "List route options", "Read Git status", "Submit plan"] },
  { id: "line", kind: "cortex", text: "Building the coffee page." },
  { id: "fold2", kind: "fold", text: "Worked 3m 12s, ran 1 command and read 1 file, 1 tool call needs attention", steps: [`Workspace step: ${step({ kind: "read", title: "Read", path: "styles.css" })}`, `Workspace step: ${step({ kind: "execute", title: "Shell", command })}`] },
  { id: "workers", kind: "workers", title: "Workers", workerIds: ["w", "w2"] },
];
s.sessions = [session];
let listeners = new Set<(u: unknown) => void>(), version = 0;
const emit = () => listeners.forEach(f => f({ kind: "projection", scope: "workspace", version: String(++version), state: structuredClone(s) }));
const port: any = {
  mode: "real",
  readWorkerActivity: async () => ({ activity: [
    { sequence: 1, kind: "note", text: "Planning the page in two steps.", at: 1000 },
    { sequence: 2, kind: "tool", text: "mcp__cortex__plan_submit", at: 3000, args: step({ kind: "other", title: "mcp__cortex__plan_submit" }) },
    { sequence: 3, kind: "tool", text: "mcp__cortex__project_read", at: 3500, args: step({ kind: "read", title: "Read", path: "." }) },
    { sequence: 4, kind: "note", text: "Started the frontend worker.", at: 5000 },
  ], activityCursor: null, messages: [] }),
  readReport: async () => ({ reportId: "r_fixture", title: "Landing page", sha256: "a".repeat(64), markdown: "# Done\n\nFixture report.", totalCharacters: 22, nextCursor: null, partial: false }),
  query: async (op: string) => ({ "capabilities.get": { everything: true, operations: [] }, "project.snapshot": [], "project.facts": [], "machine.facts": [] }[op]),
  subscribe: (f: (u: unknown) => void) => { listeners.add(f); setTimeout(emit, 0); return () => listeners.delete(f); },
  // Scripted UI-only responses. Real command execution/persistence is covered by the Python service tests.
  command: async (envelope: any) => {
    const task = s.tasks[0], c = envelope.payload;
    if (c.type === "rerun_check") {
      task.result!.checks[1] = { ...task.result!.checks[1], outcome: "passed", exitCode: 0, output: "Fresh rerun: all assertions passed", detail: "" };
      emit();
    } else if (c.type === "accept_result") {
      task.status = "ready_to_merge";
      task.acceptance = { version: 1, digest: "fixture-digest", at: Date.now(), open: { failed: 0, not_run: 0, unverified: 0 } };
      emit();
    } else throw Error("This fixture only supports Rerun and Accept");
    return { requestId: envelope.requestId, ok: true, outcome: "recorded", message: "Fixture response recorded" };
  },
};
const selectedView = new URLSearchParams(location.search).get("view") ?? "worker";
if (selectedView === "review") {
  Object.assign(s.tasks[0], {
    title: "Review the local landing page", status: "review", process: "stopped", workspaceDigest: "fixture-digest",
    integrationFacts: { targetBranch: "main", alreadyInTarget: false, sameBranch: true, commitAvailable: true, pendingPaths: ["index.html"], otherUncommitted: false, reason: null, pushStatus: "unknown" },
    result: { version: 1, digest: "fixture-digest", summary: "Created the local landing page. Review the check output, then accept the result.",
      checks: [
        { name: "Markup validation", kind: "automated", outcome: "passed", detail: "", source: "runtime", output: "Valid HTML", command: ["python3", "validate.py"], cwd: "/fixture", exitCode: 0 },
        { name: "Focused tests", kind: "automated", outcome: "failed", detail: "An assertion failed", source: "runtime", output: "Earlier run: expected a heading", command: ["python3", "check.py"], cwd: "/fixture", exitCode: 1, rerunnable: true },
        { name: "Visual judgement", kind: "manual", outcome: "not_run", detail: "Check the page as a person. This item does not block Accept.", rerunnable: false }
      ] }
  });
}
const VIEW: Record<string, string> = { review: "/m/local/t/t/changes", worker: "/m/local/t/t/workers/w", working: "/m/local/t/t/workers/w2", coordinator: "/m/local/t/t/workers/cortex", conversation: "/m/local/t/t", settings: "/settings/app/appearance", board: "/m/local/p/p/board" };
history.replaceState(null, "", VIEW[selectedView] ?? VIEW.worker);
render(() => <Workspace port={port} />, document.getElementById("root")!);
(window as any).fixture = { emit };
