// No service, repo, credentials, or model calls: a disposable Code rendering fixture.
import { render } from "solid-js/web";
import { capabilities } from "@cortex/workspace-contract";
import { emptyWorkspace, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { createRealPort, Workspace } from "../src";

const names = ["main.rs", "helper.rs"];
const texts = ["fn main() {\n    helper::run();\n}\n", "pub fn run() {}\n"];
const revision = "a".repeat(64), leaseToken = "b".repeat(64);
const mode = new URLSearchParams(location.search).get("state");
const localCode = mode !== "remote";
const range = {start:{line:1,character:4},end:{line:1,character:15}};
const snapshot = {
  version: 1, generation: 1, eventId: "a".repeat(32), status: mode === "unavailable" ? "lsp_unavailable" : "ready",
  diagnostics: mode === "unavailable" ? null : [{ range, severity: 2, message: "This call expects a configuration value." }],
};
const port = createRealPort({
  localCode, registry: true, actorId: "visual-fixture",
  transport: {
    subscribe: () => () => undefined,
    async request(operation, message) {
      const frame = message as { op?: string; params?: { file?: string } };
      const file = frame.params?.file ?? names[0];
      const ok = (result: unknown) => ({ ok: true, result });
      switch (operation) {
        case "service.capabilities": return { methods: ["machine.call"], machine: true, codeLsp: true };
        case "projects.list": return [{ projectId: "visual", displayName: "Example checkout", canonicalRoot: "/fixture", activation: {} }];
        case "capabilities.get": return capabilities("code-visual-fixture", "1", ["capabilities.get", "project.snapshot"]);
        case "decisions.list": return { items:[], nextCursor:null, highWatermark:"1" };
        case "project.snapshot": return { version:"1", projectId:"visual", highWatermark:"1", tasks:{ items:[], nextCursor:null, highWatermark:"1" } };
        case "machine.call": switch (frame.op) {
          case "overview": return ok({ name:"Fixture", retentionDays:30, autoArchiveDays:7, worktreeRoot:"/fixture", version:"1", diskMb:2, storage:[], cleanup:null, diagnostics:null });
          case "config.read": return ok({ config:emptyWorkspace().machines[0].config, connections:[], tokens:{}, usage:[] });
          case "project.config.read": return ok({ value:{ revisions:{} }, tokens:{} });
          case "repo.snapshot": return ok({ projectId: "visual", remote: "", tree: names, sources: {}, commits: [], branches: [],
            fetchedAt: null, openBranch: "dev", defaultBranch: "main",
            checkouts: [{ id: "visual:main", projectId: "visual", kind: "main", branch: "dev", path: "/fixture", taskId: null,
              base: "main", ahead: 0, behind: 0, sources: {}, files: [] }] });
          case "repo.file": return ok({ file, text: texts[names.indexOf(file)] ?? null, revision });
          case "repo.file.lease.acquire":
            return mode === "held" ? { ok:false, code:"task_file_lease_held", holderTaskId:"sample-task" } : ok({ leaseToken, expiresInMs:60000, renewAfterMs:20000 });
          case "repo.file.lease.release": return ok({ released:true });
          case "repo.lsp.open": return ok({ ...snapshot, documentId: "c".repeat(64) });
          case "repo.lsp.status": return ok(snapshot);
          case "repo.lsp.definition": return ok({ version:1,generation:1,targets:[{ file:"helper.rs",checkoutId:"visual:main",range:{start:{line:0,character:7},end:{line:0,character:10}} }] });
          case "repo.lsp.close": return ok({ closed:true });
          case "product.facts": return ok({ product:{ tasks:{}, sprints:[], messages:[] }, workers:[] });
          case "workers": return ok([]);
          case "product.read": return ok({ tasks: {}, sprints: [], messages: [] });
          default: return ok({});
        }
        default: throw new Error("Unscripted visual fixture operation");
      }
    },
  },
});
history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/p/visual/code?file=main.rs`);
render(() => <Workspace port={port} />, document.getElementById("root")!);
