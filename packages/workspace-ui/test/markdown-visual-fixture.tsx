import { render } from "solid-js/web";
import { emptyWorkspace, emptySession } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import "../src/fonts.css";
import "../src/mockup-styles.css";
import "../src/workspace.css";
import "../src/layout.css";

// Disposable, opt-in fake data for checking Markdown in the conversation and in the writing area. No service, engines or stores.
// `?theme=dark|light` picks the theme; `?align=1` paints the field's own glyphs red over the layer, so any drift shows.
const MESSAGE = [
  "## Release notes",
  "Ship **1.0.6** with *italic*, ~~old~~ and `inline code`.",
  "> Quoted from the owner: keep the **exact** text.",
  "",
  "- first item",
  "- second item with a [link](https://example.invalid/docs)",
  "",
  "```sh",
  "npm --prefix packages/workspace-ui run test",
  "  echo \"indented\"   ",
  "```",
  "---",
  "| Check | Result |",
  "| --- | --- |",
  "| Types | **passed** |",
].join("\n");
const s = emptyWorkspace();
s.projects = [{ id: "p", machineId: s.machines[0].id, name: "Project", repository: "/p", openBranch: "main", defaultBranch: "main" }];
s.tasks = [{
  id: "t", projectId: "p", sprintId: null, description: "Format Markdown", title: "Format Markdown", titleState: "generated", checkout: { kind: "main", branch: "main" }, targetBranch: null, dependencies: [],
  status: "working", resumeStatus: "working", issue: null, executionId: "e", result: null, workspaceDigest: null, acceptance: null, conflict: "none", merge: null, resolvedBy: null, integration: null,
  closedAs: null, notVerifiedByCortex: false, doneAt: null, archive: null, work: "unmerged", process: "live", unexpectedChanges: false, purgeBlocker: null,
  settings: { model: "", effort: "", mode: "guarded", language: "auto" },
}];
s.machines[0].connection = "connected";
const session = emptySession("t");
session.timeline = [
  { id: "said", kind: "said", text: MESSAGE },
  { id: "reply", kind: "cortex", text: "Here is the command:\n\n```ts\nconst answer = await run({ fast: true });\nconsole.log(answer);\n```\n\n> A quote in a reply, with `code`." },
];
s.sessions = [session];
const listeners = new Set<(u: unknown) => void>();
let version = 0;
const emit = () => listeners.forEach(f => f({ kind: "projection", scope: "workspace", version: String(++version), state: structuredClone(s) }));
const port: any = {
  mode: "real",
  query: async (op: string) => ({ "capabilities.get": { everything: true, operations: [] }, "project.snapshot": [], "project.facts": [], "machine.facts": [] }[op]),
  subscribe: (f: (u: unknown) => void) => { listeners.add(f); setTimeout(emit, 0); return () => listeners.delete(f); },
  command: async (envelope: any) => ({ requestId: envelope.requestId, ok: false, outcome: "refused", message: "This fixture sends nothing" }),
};
const params = new URLSearchParams(location.search);
document.documentElement.dataset.theme = params.get("theme") === "light" ? "light" : "dark";
if (params.get("align")) {
  const style = document.createElement("style");
  style.textContent = ".mc-field > textarea { -webkit-text-fill-color: rgba(255, 0, 0, .55) !important; }";
  document.head.append(style);
}
history.replaceState(null, "", "/m/local/t/t");
render(() => <Workspace port={port} />, document.getElementById("root")!);
(window as any).fixture = { message: MESSAGE };
