import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { conversationOf } from "../src/port/history";
import { STEP_PREFIX, viewOfActivity, viewOfText } from "../src/tool-names";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const step = (d: Record<string, string>) => STEP_PREFIX + JSON.stringify(d);

// The steps of a native coordinator turn as the history projection returns them: native activity names the role before
// the engine's title. These are the seven calls of a retained turn (four project_read, two read_skill, one plan_submit)
// that recorded no input, so only their names are known.
const retained = ["project_read", "project_read", "read_skill", "read_skill", "project_read", "project_read", "plan_submit"]
  .map((name, i) => step({ title: `orchestrator: mcp__cortex__${name}`, callId: `xa_1:toolu_${i}`, kind: "other", status: "completed" }));

// The same kinds of call once the input selection is retained (crates/workspace-service/src/native_activity.rs).
const detailed = {
  read: step({ title: "orchestrator: mcp__cortex__project_read", callId: "xa_1:read", kind: "other", status: "completed", path: "src/app.ts" }),
  skill: step({ title: "orchestrator: mcp__cortex__read_skill", callId: "xa_1:skill", kind: "other", status: "completed", ref: "orchestrator-protocol" }),
  plan: step({ title: "orchestrator: mcp__cortex__plan_submit", callId: "xa_1:plan", kind: "other", status: "completed" }),
  edit: step({ title: "worker-frontend-dev: mcp__cortex__project_edit", callId: "xa_2:edit", kind: "other", status: "completed", path: "src/a.ts", oldText: "let a = 1;", newText: "let a = 2;" }),
  write: step({ title: "worker-frontend-dev: mcp__cortex__project_write", callId: "xa_2:write", kind: "other", status: "completed", path: "notes.md", newText: "hello\nworld" }),
  command: step({ title: "worker-frontend-dev: mcp__cortex__command_run", callId: "xa_2:cmd", kind: "other", status: "completed", command: "git status --short", output: "" }),
  bash: step({ title: "worker-frontend-dev: npm test", callId: "xa_2:bash", kind: "execute", status: "completed", command: "npm test", output: "" }),
  nativeRead: step({ title: "worker-frontend-dev: Read src/b.ts", callId: "xa_2:nread", kind: "read", status: "completed", path: "src/b.ts" }),
  nativeEdit: step({ title: "worker-frontend-dev: Edit src/b.ts", callId: "xa_2:nedit", kind: "edit", status: "completed", path: "src/b.ts", oldText: "old line", newText: "new line" }),
  grep: step({ title: "worker-frontend-dev: grep \"TODO\" src", callId: "xa_2:grep", kind: "search", status: "completed", path: "src", query: "TODO" }),
  report: step({ title: "orchestrator: mcp__cortex__read_report", callId: "xa_1:report", kind: "other", status: "completed", ref: "r_0123456789ab" }),
};

it("names a native coordinator's Cortex calls by what they are, not by the role in front of them", () => {
  const rows = conversationOf(retained.map((text, i) => ({ historyId: String(i), revision: "1", kind: "tool", text, toolStatus: "completed" })), { description: "", answers: [] });
  expect(rows).toHaveLength(1);
  const fold = rows[0].entry;
  if (fold.kind !== "fold") throw new Error("not a fold");
  expect(fold.steps.map(s => viewOfText(s).label)).toEqual(["Read", "Read", "Read", "Read"]);
  // Loading a skill and submitting a plan are the runtime's own steps: they are not shown or counted.
  expect(fold.text).toBe("Read 4 files");
  expect(fold.text).not.toMatch(/edited/i);
});

it.each([
  ["read", "Read", "src/app.ts", "read"],
  ["edit", "Edit", "src/a.ts", "edit"],
  ["write", "Edit", "notes.md", "edit"],
  ["command", "Shell", "git status --short", "shell"],
  ["bash", "Shell", "npm test", "shell"],
  ["nativeRead", "Read", "src/b.ts", "read"],
  ["nativeEdit", "Edit", "src/b.ts", "edit"],
  ["grep", "grep \"TODO\" src", "TODO", "search"],
  ["report", "Read report: r_0123456789ab", "", "other"],
] as const)("puts the %s call's target on its row", (name, label, target, kind) => {
  for (const view of [viewOfText(detailed[name]), viewOfActivity({ sequence: 1, kind: "tool", text: JSON.parse(detailed[name].slice(STEP_PREFIX.length)).title, at: null, args: detailed[name].slice(STEP_PREFIX.length) })]) {
    expect(view.label).toBe(label);
    expect(view.target).toBe(target);
    expect(view.kind).toBe(kind);
    expect(`${view.label} ${view.target}`).not.toMatch(/mcp[._]|orchestrator:|worker-frontend-dev:/);
  }
});

async function mountFold(steps: string[]) {
  const state = withTask(emptyWorkspace(), "Rows");
  state.tasks[0].status = "working"; state.tasks[0].process = "live";
  state.machines[0].connection = "connected";
  const session = emptySession("t");
  session.timeline = [{ id: "history:a:1", kind: "fold", text: "Worked", steps }];
  state.sessions = [session];
  state.repos = [{ projectId: "p", remote: "", tree: ["src/app.ts"], sources: {}, commits: [], branches: [], fetchedAt: null, checkouts: [{ id: "main", projectId: "p", kind: "main", path: "/fixture", branch: "main", files: [], sources: {}, taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", updateConflict: null }] }];
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  p.port.readFile = async () => ({ text: "export const app = 1;\n", revision: "fixture" });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  document.querySelector<HTMLButtonElement>(".timeline button.fold")!.click(); await settle();
  return [...document.querySelectorAll<HTMLElement>(".timeline .step-row")];
}

it("shows a read's file on its row and opens it in Code from there", async () => {
  const [read, skill] = await mountFold([detailed.read, detailed.report]);
  expect(read.tagName).toBe("DIV");
  expect(read.querySelector(".step-l")?.textContent).toBe("Read");
  const open = read.querySelector<HTMLButtonElement>("button.step-t")!;
  expect(open.textContent).toBe("src/app.ts");
  expect(open.getAttribute("aria-label")).toBe("Open src/app.ts in Code");
  expect(skill.querySelector(".step-l")?.textContent).toBe("Read report: r_0123456789ab");
  expect(skill.querySelector("button")).toBeNull();
  open.click(); await settle();
  expect(location.search).toContain("file=src%2Fapp.ts");
});

it("opens an edit on its diff and a command on its command line", async () => {
  const [edit, write, command] = await mountFold([detailed.edit, detailed.write, detailed.command]);
  for (const row of [edit, write, command]) expect(row.tagName).toBe("BUTTON");
  expect(edit.querySelector(".step-l")?.textContent).toBe("Edit");
  expect(edit.querySelector(".step-t")?.textContent).toBe("src/a.ts");
  edit.click(); await settle();
  const detail = document.querySelector(".ws-step-detail")!;
  expect(detail.querySelector(".ws-step-path")?.textContent).toBe("src/a.ts");
  expect([...detail.querySelectorAll(".diff .d")].map(l => l.textContent)).toEqual(["−let a = 1;"]);
  expect([...detail.querySelectorAll(".diff .a")].map(l => l.textContent)).toEqual(["+let a = 2;"]);
  edit.click(); await settle();
  command.click(); await settle();
  expect(document.querySelector(".ws-step-detail pre code")?.textContent).toBe("git status --short");
  expect(document.querySelector(".timeline")!.textContent).not.toMatch(/mcp[._]|orchestrator:|worker-frontend-dev:/);
});
