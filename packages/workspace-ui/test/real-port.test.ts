import { describe, expect, it, vi } from "vitest";
import { LIMITS, validate } from "@cortex/workspace-contract";
import { emptyWorkspace } from "@cortex/workspace-domain";
import type { CommandEnvelope, MachineFacts, PortUpdate, ProjectionUpdate, WorkspaceCommand } from "@cortex/workspace-domain";
import { createRealPort, MACHINE_POLL_MS, machineReadDelay, planFrom, unavailableTransport, usageObserved, USAGE_GRACE_MS, USAGE_LATE_MS, USAGE_REFRESH_MS, USAGE_RETRY_MS, type ProjectLocator } from "../src/port/real";
import { capabilitiesWire, flush, scriptedTransport, snapshot, wireDecision, wireTask } from "./support";
import { REPO_RETRY_MS } from "../src/port/real";

const projects: ProjectLocator[] = [{ projectId: "p", name: "Project", repository: "/p", openBranch: "main", defaultBranch: "main" }];
const ALL = ["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "results.get"];
const envelope = (payload: WorkspaceCommand, requestId = "req-1"): CommandEnvelope => ({ requestId, operation: payload.type, expectedVersion: null, payload });
const receipt = (message: unknown, outcome = "recorded", target = (message as { target: string }).target) => ({ requestId: (message as { requestId: string }).requestId, outcome, target, entityVersion: "2", recordedAt: "2026-11-05T00:00:00Z" });
const service = (tasks = [wireTask()], decisions: unknown[] = []) => scriptedTransport({
  "capabilities.get": () => capabilitiesWire(ALL),
  "project.snapshot": () => snapshot("4", tasks),
  "decisions.list": () => ({ items: decisions, nextCursor: null, highWatermark: "4" }),
  "tasks.create": message => receipt(message),
  "tasks.start": message => receipt(message),
  "decisions.answer": message => receipt(message),
});
const port = (s: ReturnType<typeof service>) => createRealPort({ transport: s.transport, actorId: "local-peer", projects });
const loaded = async (s: ReturnType<typeof service>) => { const p = port(s); await p.query("capabilities.get", {}); await p.query("project.snapshot", {}); return p; };

describe("real port", () => {
  it("does not replay unchanged task history on another project's or Git refresh", async () => {
    let version = "1", executionId = "e", reads = 0, watermark = "4", appended = false;
    const s = scriptedTransport({
      "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "history.page"]),
      "project.snapshot": () => ({...snapshot("4", [wireTask({version, executionId, phase:"working", executionState:"executing", runtimeState:"EXECUTING"})]), highWatermark:watermark}),
      "history.page": () => { reads++; return {items:[{historyId:"h",sequence:"1",revision:"1",kind:"assistant",text:appended ? "New message" : "Retained message",toolStatus:null,recordedAt:"2026-11-05T00:00:00Z",availability:"available"}], nextCursor:null, highWatermark:watermark}; },
    });
    const p = port(s as ReturnType<typeof service>);
    await p.query("capabilities.get", {});
    await p.query("project.snapshot", {});
    const [reused] = await p.query("project.snapshot", {}) as Extract<ProjectionUpdate,{scope:"project"}>[];
    expect(reused.slice.sessions[0].timeline).toContainEqual(expect.objectContaining({text:"Retained message"}));
    expect(reads).toBe(1);
    appended = true; watermark = "5";
    const [changed] = await p.query("project.snapshot", {}) as Extract<ProjectionUpdate,{scope:"project"}>[];
    expect(changed.slice.sessions[0].timeline).toContainEqual(expect.objectContaining({text:"New message"}));
    expect(reads).toBe(2);
    await p.query("project.snapshot", {});
    expect(reads).toBe(2);
    version = "2";
    await p.query("project.snapshot", {});
    expect(reads).toBe(3);
    executionId = "replacement";
    await p.query("project.snapshot", {});
    expect(reads).toBe(4);
  });

  it("keeps recorded task and project times through repeated reads", async () => {
    const at = "2026-11-05T00:00:00Z";
    const s = service([wireTask({lastConfirmedAt:at})]);
    const p = await loaded(s);
    const first = (await p.query("project.snapshot", {}))[0] as Extract<ProjectionUpdate,{scope:"project"}>;
    const second = (await p.query("project.snapshot", {}))[0] as Extract<ProjectionUpdate,{scope:"project"}>;
    expect(first.slice.project.confirmedAt).toBe(Date.parse(at));
    expect(second.slice.project.confirmedAt).toBe(first.slice.project.confirmedAt);
    expect(second.slice.tasks[0].lastConfirmedAt).toBe(Date.parse(at));
  });

  it("is real, and offers only what both the service and this port can do", async () => {
    const s = scriptedTransport({ "capabilities.get": () => capabilitiesWire(["capabilities.get", "tasks.create", "history.page"]) });
    const p = port(s as ReturnType<typeof service>);
    expect(p.mode).toBe("real");
    expect(await p.query("capabilities.get", {})).toEqual({ everything: false, operations: ["tasks.create"] });
  });

  it("refuses a peer on another protocol major instead of guessing", async () => {
    const s = scriptedTransport({ "capabilities.get": () => ({ ...capabilitiesWire(ALL), protocol: { major: 2, minor: 0 } }) });
    await expect(port(s as ReturnType<typeof service>).query("capabilities.get", {})).rejects.toThrow(/version_mismatch/);
  });

  it("maps each registered project's snapshot, with its version", async () => {
    const s = service([wireTask({ title: "Named", titleAvailability: "available" })], [wireDecision({ executionId: "e" })]);
    const updates = await port(s).query("project.snapshot", {});
    expect(updates).toHaveLength(1);
    const update = updates[0] as Extract<ProjectionUpdate, { scope: "project" }>;
    expect(update.version).toBe("4");
    expect(update.slice.project.name).toBe("Project");
    expect(update.slice.tasks[0]).toMatchObject({ id: "task-1", title: "Named", status: "draft", projectId: "p" });
  });

  it("starts with an empty registry", async () => {
    const s = service();
    const p = createRealPort({ transport: s.transport, actorId: "local-peer" });
    expect(await p.query("project.snapshot", {})).toEqual([]);
    expect(s.calls).toEqual([]);
  });

  it("refuses a malformed snapshot rather than showing it", async () => {
    const s = service();
    s.answers["project.snapshot"] = () => ({ projectId: "p", version: 7 });
    await expect(port(s).query("project.snapshot", {})).rejects.toThrow();
  });

  it("follows the task pages of a large project", async () => {
    const s = service();
    s.answers["project.snapshot"] = () => ({ ...snapshot("4", [wireTask({ taskId: "a" })]), tasks: { items: [wireTask({ taskId: "a" })], nextCursor: "next", highWatermark: "4" } });
    s.answers["tasks.list"] = () => ({ items: [wireTask({ taskId: "b" })], nextCursor: null, highWatermark: "4" });
    const [update] = await port(s).query("project.snapshot", {}) as Extract<ProjectionUpdate, { scope: "project" }>[];
    expect(update.slice.tasks.map(t => t.id)).toEqual(["a", "b"]);
  });

  it("sends a schema-valid create, and a receipt alone publishes nothing", async () => {
    const s = service();
    const p = await loaded(s);
    const updates: PortUpdate[] = [];
    p.subscribe(u => updates.push(u));
    const result = await p.command(envelope({ type: "create_task", projectId: "p", description: "New work", checkout: { kind: "main" } }));
    const sent = s.calls.find(c => c.operation === "tasks.create")!.message;
    expect(() => validate("Command", sent)).not.toThrow();
    expect(sent).toMatchObject({ actorId: "local-peer", projectId: "p", requestId: "req-1", expectedVersion: "0", ownerEpoch: null, payload: { description: "New work" } });
    expect(result).toMatchObject({ ok: true, outcome: "recorded", requestId: "req-1" });
    expect(result.entityId).toBeTruthy();
    expect(updates).toEqual([]);
  });

  it("carries task creation attachment bytes, not just filenames", async () => {
    const s = service(), p = await loaded(s);
    const attachments = [{name:"note.txt",mimeType:"text/plain",data:btoa("fixture")}];
    await p.command(envelope({type:"create_task",projectId:"p",description:"Task with file",checkout:{kind:"main"},attachments}));
    expect(s.calls.find(c => c.operation === "tasks.create")!.message).toMatchObject({payload:{attachments}});
  });

  it("carries the task route and checkout choice in a valid create command", async () => {
    const s = service(), p = await loaded(s);
    const settings = { model: "route:model", effort: "high", mode: "guarded", language: "en" } as const;
    await p.command(envelope({ type: "create_task", projectId: "p", description: "New work", settings, checkout: { kind: "worktree", branch: "ignored", path: "ignored" } }));
    const sent = s.calls.find(c => c.operation === "tasks.create")!.message;
    expect(() => validate("Command", sent)).not.toThrow();
    expect(sent).toMatchObject({payload:{description:"New work",settings,checkout:{kind:"worktree"}}});
    expect((sent as {payload:{checkout:unknown}}).payload.checkout).toEqual({kind:"worktree"});
  });

  it("requests a coherent reread, never a wire-only projection, when the service says the project changed", async () => {
    const s = service();
    const p = await loaded(s);
    const updates: PortUpdate[] = [];
    p.subscribe(u => updates.push(u));
    s.send({ type: "invalidated", projectId: "p" });
    await flush();
    expect(updates).toHaveLength(1);
    expect(updates[0]).toEqual({ kind: "refresh" });
  });

  it("starts a draft against its current entity version", async () => {
    const s = service([wireTask({ version: "6" })]);
    const p = await loaded(s);
    await p.command(envelope({ type: "start_task", taskId: "task-1" }));
    const sent = s.calls.find(c => c.operation === "tasks.start")!.message;
    expect(() => validate("Command", sent)).not.toThrow();
    expect(sent).toMatchObject({ target: "task-1", expectedVersion: "6", ownerEpoch: null, payload: {} });
  });

  it("answers a named question with its exact identities and option", async () => {
    const task = wireTask({ executionId: "e", phase: "waiting", runtimeState: "WAITING_USER", executionState: "waiting_user" });
    const s = service([task], [wireDecision()]);
    const p = await loaded(s);
    await p.command(envelope({ type: "answer_decision", taskId: "task-1", decisionId: "d", answer: "A" }));
    const sent = s.calls.find(c => c.operation === "decisions.answer")!.message;
    expect(() => validate("Command", sent)).not.toThrow();
    expect(sent).toMatchObject({ target: "d", expectedVersion: "2", ownerEpoch: "3", payload: { executionId: "e", acpSessionId: "s", batchId: null, answers: [{ decisionId: "d", expectedVersion: "2", questionId: "q", optionId: "o", text: "A" }] } });
  });

  it("keeps a free-form answer exactly as written, with no option", async () => {
    const s = service([wireTask({ executionId: "e" })], [wireDecision()]);
    const p = await loaded(s);
    await p.command(envelope({ type: "answer_decision", taskId: "task-1", decisionId: "d", answer: "line one\nline two" }));
    expect((s.calls.find(c => c.operation === "decisions.answer")!.message as { payload: { answers: { optionId: unknown; text: string }[] } }).payload.answers[0]).toMatchObject({ optionId: null, text: "line one\nline two" });
  });

  it("a refused outcome is a failed receipt, and an unknown outcome is never read as success", async () => {
    const s = service();
    const p = await loaded(s);
    s.answers["tasks.create"] = message => receipt(message, "stale_version");
    expect(await p.command(envelope({ type: "create_task", projectId: "p", description: "x", checkout: { kind: "main" } }))).toMatchObject({ ok: false, outcome: "stale_version" });
    s.answers["tasks.create"] = message => receipt(message, "future_success");
    expect(await p.command(envelope({ type: "create_task", projectId: "p", description: "x", checkout: { kind: "main" } }, "req-2"))).toMatchObject({ ok: false, outcome: "unknown" });
  });

  it("an unconnected intent never reaches the service", async () => {
    const s = service();
    const p = await loaded(s);
    const before = s.calls.length;
    expect(await p.command(envelope({ type: "run_sprint", sprintId: "s" }))).toMatchObject({ ok: false, outcome: "unsupported" });
    expect(s.calls).toHaveLength(before);
  });

  it("a lost response is an unknown outcome, and the command is not sent again", async () => {
    const s = service();
    const p = await loaded(s);
    s.answers["tasks.create"] = () => { throw new Error("socket closed"); };
    const before = s.calls.length;
    const result = await p.command(envelope({ type: "create_task", projectId: "p", description: "x", checkout: { kind: "main" } }));
    expect(result).toMatchObject({ ok: false, outcome: "unknown_outcome" });
    expect(s.calls).toHaveLength(before + 1);
  });

  it("rejects a snapshot for another project", async () => {
    const s = service();
    s.answers["project.snapshot"] = () => ({...snapshot("4", [wireTask()]),projectId:"foreign"});
    await expect(port(s).query("project.snapshot",{})).rejects.toThrow(/identity mismatch/);
  });

  it("does not turn an unrelated receipt into a successful action", async () => {
    const s = service();
    const p = await loaded(s);
    s.answers["tasks.start"] = message => ({...receipt(message),requestId:"other"});
    expect(await p.command(envelope({type:"start_task",taskId:"task-1"}))).toMatchObject({ok:false,outcome:"unknown_outcome"});
  });

  it("never converts a free-form answer that equals an option label into a selected option", async () => {
    const s = service([wireTask({executionId:"e"})],[wireDecision()]);
    const p = await loaded(s);
    await p.command(envelope({type:"answer_decision",taskId:"task-1",decisionId:"d",answer:"A",optionId:null}));
    expect(s.calls.find(c => c.operation === "decisions.answer")!.message).toMatchObject({payload:{answers:[{optionId:null,text:"A"}]}});
  });

  it.each([false, true])("answers a batch across harmless version refreshes, never changed questions (%s)", async changed => {
    let version="2";
    const members=()=>[
      wireDecision({decisionId:"d1",questionId:"q1",batchId:"batch",version,prompt:changed && version==="3"?"Changed owner decision":"Choose direction?"}),
      wireDecision({decisionId:"d2",questionId:"q2",batchId:"batch",version,prompt:"Choose format?"}),
    ];
    const s=service([wireTask({executionId:"e"})],members()), p=await loaded(s);
    s.answers["decisions.list"]=()=>({items:members(),nextCursor:null,highWatermark:version});
    expect((await p.command(envelope({type:"answer_decision",taskId:"task-1",decisionId:"d1",answer:"A"}))).ok).toBe(true);
    expect(s.calls.filter(c=>c.operation==="decisions.answer")).toHaveLength(0);
    version="3";
    await p.query("project.snapshot",{});
    const result=await p.command(envelope({type:"answer_decision",taskId:"task-1",decisionId:"d2",answer:"A"},"last-answer"));
    const sent=s.calls.filter(c=>c.operation==="decisions.answer");
    if(changed) { expect(result.ok).toBe(false); expect(sent).toHaveLength(0); }
    else {
      expect(result.ok).toBe(true); expect(sent).toHaveLength(1);
      expect(sent[0].message).toMatchObject({expectedVersion:"3",payload:{answers:[
        {questionId:"q1",expectedVersion:"3",text:"A"},{questionId:"q2",expectedVersion:"3",text:"A"},
      ]}});
    }
  });

  it("passes connection changes through", () => {
    const s = service();
    const p = port(s);
    const updates: PortUpdate[] = [];
    p.subscribe(u => updates.push(u));
    s.send({ type: "connection", state: "disconnected", reason: "closed" });
    expect(updates[0]).toMatchObject({ kind: "connection", connection: { state: "disconnected", reason: "closed" } });
  });

  describe("with the machine helper", () => {
    const repo = { projectId: "p", remote: "", tree: ["a.txt"], sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "dev", defaultBranch: "main",
      checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "dev", path: "/p", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null,
        files: [{ path: "a.txt", status: "M", add: 1, del: 0, hunks: [], stage: "unstaged" }, { path: "a.txt", status: "M", add: 1, del: 1, hunks: [], stage: "staged" }] }] };
    const helper = (product: Record<string, unknown> = {}, refuse?: string) => {
      const s = service([wireTask({ executionId: "e", phase: "result", runtimeState: "COMPLETED", executionState: "result" })]);
      const asked: { op: string; projectId?: string; params?: Record<string, unknown> }[] = [];
      s.answers["service.capabilities"] = () => ({ methods: ["machine.call"], machine: true });
      s.answers["projects.list"] = () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }];
      s.answers["executions.get"] = () => ({ executionId: "e", taskId: "task-1", version: "2", runtimeTaskId: "r", acpSessionId: "s", runtimePlanRevision: "0", ownerEpoch: "3", state: "result", warnings: [], resultId: "res" });
      s.answers["results.get"] = () => ({ kind: "answer", resultId: "res", taskId: "task-1", executionId: "e", version: "1", text: "Done", evidence: [], checks: [], observedWorkspaceDigest: null });
      s.answers["executions.cancel"] = () => ({ outcome: "cancellation_requested" });
      s.answers["executions.control"] = () => ({outcome:"recorded"});
      s.answers["executions.prompt"] = () => ({ outcome: "recorded" });
      s.answers["machine.call"] = message => {
        const call = message as (typeof asked)[number];
        asked.push(call);
        if (refuse === call.op) return { ok: false, code: "git_failed", message: "nothing to commit" };
        if (call.op === "repo.snapshot") return { ok: true, result: repo };
        if (call.op === "product.read") return { ok: true, result: { tasks: {}, sprints: [], ...product } };
        if (call.op === "workers") return { ok: true, result: [] };
        if (call.op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 9, autoArchiveDays: 3, worktreeRoot: "~/t", version: "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
        if (call.op === "project.config.read") return {ok:true,result:{value:{},tokens:{routing:"absent",project_ceiling:"absent"}}};
        if (call.op === "config.read") return { ok: true, result: { config: { active: "default", projects: {}, revisions: {routing:1} }, connections: [], tokens: { routing: "sha256:a", routing_profiles: "sha256:b", profile_sections: { routing: "x" } }, usage: [{ engine: "codex", connected: true, usedPercent: 40, resetAt: null, observedAt: null, window: "weekly" }] } };
        return { ok: true, result: { done: true } };
      };
      const p = createRealPort({ transport: s.transport, actorId: "local-peer", registry: true });
      return { s, p, asked, ready: async () => { await p.query("capabilities.get", {}); await p.query("project.snapshot", {}); await p.query("project.facts", {}); await p.query("machine.facts", {}); } };
    };

    it("routes sprint controls from held facts without scanning project products", async () => {
      const sprint = {id:"sp",projectId:"p",planning:"created",materializedVersion:1,taskIds:[],conversation:[],history:[]};
      const {p,ready,s} = helper({sprints:[sprint]});
      await ready(); s.calls.length=0;
      expect((await p.command(envelope({type:"run_sprint",sprintId:"sp"}))).ok).toBe(true);
      expect(s.calls.filter(c => c.operation === "machine.call").map(c => (c.message as {op:string}).op)).toEqual(["product.sprint"]);
      s.calls.length=0;
      await p.query("project.facts",{projectId:"p"});
      const ops = s.calls.filter(c => c.operation === "machine.call").map(c => (c.message as {op:string}).op);
      expect(ops).toContain("product.facts");
      expect(ops).not.toContain("product.read");
      expect(ops).not.toContain("workers");
    });

    it("keeps cached Git facts after an action and publishes the fresh snapshot later", async () => {
      vi.useFakeTimers();
      let stop = () => {};
      try {
        const {s,p,ready} = helper({hasGit:true});
        await ready();
        const answer = s.answers["machine.call"]!;
        let finish!: (v: unknown) => void;
        s.answers["machine.call"] = message => (message as {op:string}).op === "repo.snapshot"
          ? new Promise(resolve => {finish = resolve;}) : answer(message);
        const updates: PortUpdate[] = [];
        stop = p.subscribe(update => updates.push(update));
        await p.command(envelope({type:"git_stage",checkoutId:"p:main",path:"a.txt",staged:true}));
        const reading = p.query("project.facts", {});
        await vi.advanceTimersByTimeAsync(51);
        const facts = await reading;
        expect(facts[0].hasGit).toBe(true);
        expect(facts[0].repo).toEqual(repo);
        const fresh = {...repo, openBranch:"next"};
        finish({ok:true,result:fresh});
        await vi.advanceTimersByTimeAsync(0);
        expect(updates).toContainEqual(expect.objectContaining({kind:"facts",facts:expect.objectContaining({repo:fresh})}));
      } finally { stop(); vi.useRealTimers(); }
    });

    it("names a failed repository read and recovers on its own once the cause is fixed", async () => {
      vi.useFakeTimers();
      let stop = () => {};
      try {
        const {s,p,ready} = helper({hasGit:true});
        const answer = s.answers["machine.call"]!;
        const reason = "Cortex's private folder has unsafe permissions (writable by every account on this machine): /fixture/config/cortex-workspace-dev. Make it private to your account (for example chmod 700 on that folder), then try again.";
        let broken = true, snapshots = 0;
        s.answers["machine.call"] = message => {
          if ((message as {op:string}).op !== "repo.snapshot") return answer(message);
          snapshots++;
          return broken ? { ok: false, code: "private_directory_unsafe", message: reason } : answer(message);
        };
        const updates: PortUpdate[] = [];
        stop = p.subscribe(update => updates.push(update));
        await ready();
        const [failed] = await p.query("project.facts", {});
        expect(failed.repo).toBeNull();
        expect(failed.reviews[0].unavailableReason).toBe(reason);
        // The owner fixes the folder. No event, poll round or restart is needed to see the repository again.
        broken = false;
        const before = snapshots;
        await vi.advanceTimersByTimeAsync(REPO_RETRY_MS + 1);
        expect(snapshots).toBe(before + 1);
        expect(updates).toContainEqual(expect.objectContaining({kind:"facts",facts:expect.objectContaining({repo})}));
        const [recovered] = await p.query("project.facts", {});
        expect(recovered.repo).toEqual(repo);
        expect(recovered.reviews[0].unavailableReason).toBeUndefined();
        // A recovered read schedules nothing further of its own.
        const settled = snapshots;
        await vi.advanceTimersByTimeAsync(REPO_RETRY_MS * 4);
        expect(snapshots).toBe(settled);
      } finally { stop(); vi.useRealTimers(); }
    });

    it("backs off repository retries while the failure lasts", async () => {
      vi.useFakeTimers();
      let stop = () => {};
      try {
        const {s,p,ready} = helper({hasGit:true});
        const answer = s.answers["machine.call"]!;
        let snapshots = 0;
        s.answers["machine.call"] = message => {
          if ((message as {op:string}).op !== "repo.snapshot") return answer(message);
          snapshots++;
          return { ok: false, code: "machine_tool_failed", message: "Filesystem operation failed." };
        };
        stop = p.subscribe(() => {});
        await ready();
        const [failed] = await p.query("project.facts", {});
        // A generic failure keeps the generic guidance rather than echoing an internal message.
        expect(failed.reviews[0].unavailableReason).toMatch(/Repository details are updating or unavailable/);
        const first = snapshots;
        await vi.advanceTimersByTimeAsync(REPO_RETRY_MS + 1);
        expect(snapshots).toBe(first + 1);
        await vi.advanceTimersByTimeAsync(REPO_RETRY_MS + 1);
        expect(snapshots).toBe(first + 1);
        await vi.advanceTimersByTimeAsync(REPO_RETRY_MS);
        expect(snapshots).toBe(first + 2);
      } finally { stop(); vi.useRealTimers(); }
    });

    it("uses authoritative repository presence while the first Git snapshot is pending", async () => {
      vi.useFakeTimers();
      try {
        for (const hasGit of [true, false]) {
          const {s,p} = helper({hasGit});
          const answer = s.answers["machine.call"]!;
          s.answers["machine.call"] = message => (message as {op:string}).op === "repo.snapshot"
            ? new Promise(() => {}) : answer(message);
          await p.query("capabilities.get", {});
          const reading = p.query("project.facts", {});
          await vi.advanceTimersByTimeAsync(51);
          expect((await reading)[0]).toMatchObject({hasGit,repo:null});
        }
      } finally { vi.useRealTimers(); }
    });

    it("publishes slow Git facts without triggering a full-read feedback loop", async () => {
      vi.useFakeTimers();
      let stop = () => {};
      try {
        const {s,p} = helper();
        const answer = s.answers["machine.call"]!;
        let finish!: (v: unknown) => void;
        s.answers["machine.call"] = message => (message as {op:string}).op === "repo.snapshot"
          ? new Promise(resolve => {finish = resolve;}) : answer(message);
        await p.query("capabilities.get", {});
        const snapshotRead = p.query("project.snapshot", {});
        await vi.advanceTimersByTimeAsync(51);
        await snapshotRead;
        const reading = p.query("project.facts", {});
        await vi.advanceTimersByTimeAsync(51);
        await reading;
        const updates: PortUpdate[] = [];
        stop = p.subscribe(update => updates.push(update));
        finish({ok:true,result:repo});
        await vi.advanceTimersByTimeAsync(0);
        expect(updates.some(u=>u.kind === "refresh")).toBe(false);
        expect(updates).toContainEqual(expect.objectContaining({kind:"facts",facts:expect.objectContaining({repo})}));
      } finally { stop(); vi.useRealTimers(); }
    });

    it("offers the machine's commands once the service says the helper is there", async () => {
      const { p } = helper();
      const caps = await p.query("capabilities.get", {});
      expect(caps.operations).toEqual(expect.arrayContaining(["workspace.git_commit", "workspace.archive_task", "workspace.save_surface"]));
      expect(caps.operations).not.toContain("workspace.pair_machine");
    });

    it("reads Git, branches and one review per started task from the machine", async () => {
      const { p, ready } = helper();
      await ready();
      const [facts] = await p.query("project.facts", {});
      expect(facts).toMatchObject({ openBranch: "dev", defaultBranch: "main" });
      expect(facts.reviews.map(r => r.taskId)).toEqual(["task-1"]);
      expect(facts.reviews[0].files).toEqual([{ path: "a.txt", status: "M", add: 2, del: 1, hunks: [] }]);
    });

    // Live run: closing an accepted project-folder task as is turned its Changes tab into "No diff recorded, 0 files".
    // Its changes stay in the folder, so the files its workers recorded writing stay its diff; nothing else is attributed to it.
    it("a task closed as is keeps the diff of the files its workers recorded", async () => {
      const worker = (edits: {path:string;add:number;del:number}[]) => ({ id: "w", taskId: "task-1", executionId: "e", role: "technical-writer", status: "completed", lane: "execution", goal: "Write", now: null, waitsFor: null,
        scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], messages: [], result: { summary: "Wrote", checks: [], edits } });
      for (const [edits, files] of [[[{ path: "a.txt", add: 2, del: 1 }], ["a.txt"]], [[{ path: "elsewhere.txt", add: 1, del: 0 }], []], [[], null]] as const) {
        const { s, p, ready } = helper({ tasks: { "task-1": { status: "done", closedAs: "as_is", acceptance: { at: 1, digest: "d", version: "1" } } } });
        const answer = s.answers["machine.call"]!;
        s.answers["machine.call"] = message => (message as {op:string}).op === "workers" ? { ok: true, result: [worker([...edits])] } : answer(message);
        await ready();
        const [facts] = await p.query("project.facts", {});
        const review = facts.reviews.find(r => r.taskId === "task-1");
        if (files === null) expect(review).toBeUndefined();
        else expect(review?.files.map(f => f.path)).toEqual(files);
      }
    });

    it("reports the machine, its orchestration and the plan usage engines gave", async () => {
      const { p, ready, asked } = helper();
      await ready();
      const [facts] = await p.query("machine.facts", {});
      expect(facts.patch.config).toMatchObject({ active: "default" });
      expect(facts.patch.connections).toEqual([]);
      expect(facts.engineUsage![0]).toMatchObject({ engine: "codex", usedPercent: 40 });
      // Startup reads only what the first screen needs: storage accounting and the rest of the overview wait for a screen.
      expect(facts.patch).not.toHaveProperty("name");
      expect(facts.patch).not.toHaveProperty("retentionDays");
      expect(asked.map(a => a.op)).not.toContain("overview");
      const configReads = asked.filter(a => a.op === "config.read").length;
      const [overview] = await p.query("machine.overview", {});
      expect(overview.patch).toEqual({ name: "Desk", retentionDays: 9, autoArchiveDays: 3, worktreeRoot: "~/t", version: "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null, ghAvailable: undefined, reclamationFailures: undefined, defaultWorktreeRoot: undefined });
      // An overview-only update leaves the engine usage and the composer's options as they were.
      expect(overview).not.toHaveProperty("engineUsage");
      expect(overview).not.toHaveProperty("options");
      expect(asked.filter(a => a.op === "overview")).toHaveLength(1);
      // The on-demand read is the overview alone: it does not reread the configuration.
      expect(asked.filter(a => a.op === "config.read")).toHaveLength(configReads);
    });

    it("keeps an overview a screen asked for current on the machine poll, and never reads it before", async () => {
      vi.useFakeTimers();
      let stop = () => {};
      try {
        const { p, ready, asked } = helper();
        await ready();
        const machines: MachineFacts[] = [];
        stop = p.subscribe(update => { if (update.kind === "machine") machines.push(update.facts); });
        await vi.advanceTimersByTimeAsync(2 * MACHINE_POLL_MS);
        expect(asked.map(a => a.op)).not.toContain("overview");
        expect(machines.length).toBeGreaterThan(0);
        expect(machines.every(m => !("name" in m.patch))).toBe(true);
        await p.query("machine.overview", {});
        asked.length = 0; machines.length = 0;
        await vi.advanceTimersByTimeAsync(2 * MACHINE_POLL_MS);
        expect(asked.filter(a => a.op === "overview").length).toBeGreaterThan(0);
        expect(machines.find(m => "retentionDays" in m.patch)?.patch).toMatchObject({ retentionDays: 9, name: "Desk" });
      } finally { stop(); vi.useRealTimers(); }
    });

    /** A machine whose service refreshes the limits as `usage_refresh.rs` does: from the first settings read, every
     *  five minutes plus one pass (15 s), each read taking a few seconds; before that it holds a snapshot two hours old. */
    const refreshing = (works = true) => {
      const h = helper();
      let start: number | null = null;
      const old = Date.now() - 2 * 3600_000;
      const snapshotAt = (now: number) => start === null || !works || now < start + 3_000 ? old : start + 3_000 + Math.floor((now - start - 3_000) / 315_000) * 315_000;
      const answer = h.s.answers["machine.call"]!;
      h.s.answers["machine.call"] = message => {
        const reply = answer(message) as { ok: boolean; result: Record<string, unknown> };
        if ((message as { op: string }).op !== "config.read") return reply;
        start ??= Date.now();
        const at = new Date(snapshotAt(Date.now())).toISOString();
        return { ...reply, result: { ...reply.result, usage: [{ engine: "codex", connected: true, usedPercent: 40, resetAt: null, observedAt: at, window: "weekly" },
          { engine: "claude", connected: true, usedPercent: 50, resetAt: null, observedAt: at, window: "weekly", windows: [{ name: "seven_day", usedPercent: 50, resetAt: null, observedAt: at }] }] } };
      };
      return h;
    };
    /** Drives a visible window for `minutes`; returns the oldest any shown limit got, and how many settings reads it took. */
    const watch = async (minutes: number, works = true) => {
      const { p, ready, asked } = refreshing(works);
      await ready();
      let shown = Math.min(...((await p.query("machine.facts", {}))[0].engineUsage ?? []).map(u => Date.parse(u.observedAt!)));
      let oldest = 0;
      const stop = p.subscribe(update => {
        if (update.kind !== "machine" || !update.facts.engineUsage) return;
        oldest = Math.max(oldest, Date.now() - shown);
        shown = Math.min(...update.facts.engineUsage.map(u => Date.parse(u.observedAt!)));
      });
      const reads = asked.filter(a => a.op === "config.read").length;
      // The first minute is the service catching up from a snapshot it held before the app opened.
      await vi.advanceTimersByTimeAsync(60_000);
      oldest = 0;
      for (let t = 60_000; t < minutes * 60_000; t += 5_000) { await vi.advanceTimersByTimeAsync(5_000); oldest = Math.max(oldest, Date.now() - shown); }
      stop();
      return { oldest, reads: asked.filter(a => a.op === "config.read").length - reads };
    };

    it("keeps the limits a visible window shows at most about six minutes old, reading the machine about once per snapshot", async () => {
      vi.useFakeTimers();
      try {
        const { oldest, reads } = await watch(60);
        // Two unaligned five-minute timers in a row let a snapshot stay on screen for twenty minutes in a live run.
        expect(oldest).toBeLessThanOrEqual(USAGE_REFRESH_MS + USAGE_GRACE_MS + 15_000);
        expect(oldest).toBeLessThanOrEqual(6 * 60_000);
        // Lined up with the service, not polling faster: about one read per five-minute snapshot, plus the catch-up at start.
        expect(reads).toBeLessThanOrEqual(Math.ceil(60 / 5) + 3);
      } finally { vi.useRealTimers(); }
    });

    it("never reads the machine more often than the plain poll once an engine stops reporting", async () => {
      vi.useFakeTimers();
      try {
        const { reads } = await watch(60, false);
        // The old snapshot is asked for once soon after start, then only on the five-minute poll.
        expect(reads).toBeLessThanOrEqual(Math.ceil(60 / 5) + 2);
      } finally { vi.useRealTimers(); }
    });

    it("lines the next machine read up with the service's next limits snapshot", () => {
      const now = Date.parse("2026-11-17T04:20:00Z");
      // Just read: due one refresh and the grace after it was observed.
      expect(machineReadDelay([now - 10_000], now, false)).toBe(USAGE_REFRESH_MS + USAGE_GRACE_MS - 10_000);
      // The engine that goes stale first sets it.
      expect(machineReadDelay([now - 10_000, now - 100_000], now, false)).toBe(USAGE_REFRESH_MS + USAGE_GRACE_MS - 100_000);
      // A late snapshot is asked for again soon, for a while; then the plain poll.
      expect(machineReadDelay([now - USAGE_REFRESH_MS - USAGE_GRACE_MS - 10_000], now, false)).toBe(USAGE_RETRY_MS);
      expect(machineReadDelay([now - USAGE_REFRESH_MS - USAGE_GRACE_MS - USAGE_LATE_MS - 1], now, false)).toBe(MACHINE_POLL_MS);
      // The first read, or the first after the window was hidden, asks once more soon for an old snapshot.
      expect(machineReadDelay([now - 2 * 3600_000], now, true)).toBe(USAGE_RETRY_MS);
      // Nothing observed: the plain poll. Never longer than one snapshot and its grace, whatever the clock says.
      expect(machineReadDelay([], now, true)).toBe(MACHINE_POLL_MS);
      expect(machineReadDelay([now + 3600_000], now, false)).toBe(USAGE_REFRESH_MS + USAGE_GRACE_MS);
      expect(usageObserved([{ engine: "claude", connected: true, usedPercent: 1, resetAt: null, observedAt: null, window: "weekly",
        windows: [{ name: "seven_day", usedPercent: 1, resetAt: null, observedAt: "2026-11-17T04:15:00Z" }, { name: "seven_day_fable", usedPercent: null, resetAt: null, observedAt: null }] },
      { engine: "codex", connected: true, usedPercent: null, resetAt: null, observedAt: null, window: "weekly" }])).toEqual([Date.parse("2026-11-17T04:15:00Z")]);
    });

    it("keeps the machine orchestration when one project's settings are refused", async () => {
      const { p } = helper({}, "project.config.read");
      await p.query("capabilities.get", {});
      const [facts] = await p.query("machine.facts", {});
      expect(facts.patch.config).toMatchObject({ active: "default" });
      expect(facts.patch.config?.projects).toEqual({});
      await expect(p.command(envelope({ type: "save_surface", machineId: "local", projectId: "p", surface: "routing", revision: 0, value: {} }))).resolves.toMatchObject({ ok: false });
    });

    it("stops and steers through the execution, never through a helper", async () => {
      const { s, p, ready } = helper();
      await ready();
      await p.command(envelope({ type: "stop_task", taskId: "task-1", confirmed: true }));
      await p.command(envelope({ type: "send_message", taskId: "task-1", text: "Use the other file", mode: "send_now" }));
      expect(s.calls.find(c => c.operation === "executions.control")!.message).toMatchObject({ projectId: "p", executionId: "e", action:"pause",expectedVersion:"1" });
      expect(s.calls.find(c => c.operation === "executions.prompt")!.message).toEqual({ projectId: "p", executionId: "e", text: "Use the other file", requestId:"req-1" });
    });

    // Live D3 remainder: a native host cannot run a task in a separate worktree, so the
    // composer must not offer one (it showed the option and Start was then refused).
    it("offers task worktrees only when the service does not rule them out", async () => {
      const { s, p } = helper();
      expect((await p.query("capabilities.get", {})).operations).toContain("workspace.task_worktrees");
      s.answers["service.capabilities"] = () => ({ methods: ["machine.call"], machine: true, taskWorktrees: false });
      const native = (await p.query("capabilities.get", {})).operations;
      expect(native).not.toContain("workspace.task_worktrees");
      expect(native).toContain("workspace.close_task");
    });

    // Live N3/N4: on a native execution, Request changes and Close task reached the runtime
    // but the saved card never reopened or closed, so the task stayed in review.
    it("runs native owner mutations on the execution, then updates the saved card as Python does", async () => {
      const { s, p, asked, ready } = helper();
      await ready();
      s.answers["executions.prompt"] = () => ({outcome:"recorded",native:true});
      s.answers["executions.control"] = () => ({outcome:"recorded",native:true});
      asked.length = 0;
      expect(await p.command(envelope({type:"stop_task",taskId:"task-1",confirmed:true},"stop"))).toMatchObject({ok:true});
      expect(await p.command(envelope({type:"continue_task",taskId:"task-1"},"continue"))).toMatchObject({ok:true});
      expect(asked.some(c=>c.op==="product.task")).toBe(false);
      expect(await p.command(envelope({type:"request_changes",taskId:"task-1",text:"Add a docstring"},"changes"))).toMatchObject({ok:true});
      expect(s.calls.filter(c => c.operation === "executions.prompt").at(-1)!.message).toMatchObject({executionId:"e",text:"Add a docstring"});
      expect(asked.filter(c=>c.op==="product.task").map(c=>c.params)).toEqual([{action:"reopen",taskId:"task-1"}]);
      expect(await p.command(envelope({type:"close_task",taskId:"task-1",choice:"as_is",confirmed:true,now:1},"close"))).toMatchObject({ok:true});
      expect(s.calls.filter(c => c.operation === "executions.control").at(-1)!.message).toMatchObject({action:"close_task",confirmed:true});
      expect(asked.filter(c=>c.op==="product.task").at(-1)?.params).toEqual({action:"close",taskId:"task-1",choice:"as_is",confirmed:true});
    });

    it("carries actual bytes for prompt, queue and queued delivery", async () => {
      const attachments = [{name:"notes.txt",mimeType:"text/plain",data:btoa("fixture")}];
      const message = {id:"q1",taskId:"task-1",workerId:null,text:"Queued",mode:"queue",delivery:"queued",at:1,deliveredAt:null,failure:null,attachments};
      const {s,p,asked,ready} = helper({messages:[message]});
      await ready();
      expect(await p.command(envelope({type:"send_message",taskId:"task-1",text:"Now",mode:"send_now",attachments}))).toMatchObject({ok:true});
      expect(s.calls.find(c => c.operation === "executions.prompt")!.message).toMatchObject({text:"Now",attachments});
      expect(await p.command(envelope({type:"send_message",taskId:"task-1",text:"Later",mode:"queue",attachments},"queue-new"))).toMatchObject({ok:true});
      expect(asked.find(c => c.op === "product.message" && c.params?.action === "add")?.params).toMatchObject({message:{attachments}});
      expect(await p.command(envelope({type:"send_queued",messageId:"q1"},"queue-send"))).toMatchObject({ok:true});
      expect(s.calls.filter(c => c.operation === "executions.prompt").at(-1)!.message).toMatchObject({text:"Queued",requestId:"q1",attachments});
    });

    it("carries sprint creation and every planner note as bytes, including legacy planning prompts", async () => {
      const attachments = [{name:"screen.png",mimeType:"image/png",data:btoa("screenshot")}];
      const sprint = {id:"sp",projectId:"p",planning:"question",conversation:[],attachments};
      const {s,p,asked,ready} = helper({sprints:[sprint]});
      await ready();
      const original = s.answers["machine.call"]!;
      s.answers["machine.call"] = message => {
        const call = message as {op:string;params?:Record<string,unknown>};
        if (call.op === "config.read") {
          const config = structuredClone(emptyWorkspace().machines[0].config);
          config.setup["codex-acp"].status = "connected";
          config.catalogue["codex-acp"].models = [{id:"model",label:"Model",efforts:[],observed:true}];
          config.surfaces.routing.engines["codex-acp"].enabled = ["model"];
          return {ok:true,result:{config}};
        }
        if (call.op === "product.sprint" && call.params?.action === "create") {
          asked.push(call); return {ok:true,result:{sprintId:"sp"}};
        }
        return original(message);
      };
      expect(await p.command(envelope({type:"create_sprint",projectId:"p",description:"Plan",attachments,runMode:"manual",checkout:{kind:"worktree"},settings:{model:"model",effort:"",mode:"guarded",language:"en"}}))).toMatchObject({ok:true,entityId:"sp"});
      expect(asked.find(c => c.op === "product.sprint" && c.params?.action === "create")?.params).toMatchObject({attachments,checkout:{kind:"worktree"}});
      for (const payload of [{type:"plan_message",sprintId:"sp",text:"Look",attachments},{type:"answer_planning",sprintId:"sp",answer:"Use it",attachments}] as WorkspaceCommand[]) {
        expect(await p.command(envelope(payload))).toMatchObject({ok:true});
      }
      expect(asked.filter(c => c.op === "product.sprint" && c.params?.action === "note").every(c => c.params?.attachments === attachments || JSON.stringify(c.params?.attachments) === JSON.stringify(attachments))).toBe(true);
      (sprint as typeof sprint & {planningTaskId?:string}).planningTaskId = "task-1";
      expect(await p.command(envelope({type:"plan_message",sprintId:"sp",text:"Legacy",attachments}))).toMatchObject({ok:true});
      expect(s.calls.filter(c => c.operation === "executions.prompt").at(-1)?.message).toMatchObject({text:"Legacy",attachments});
      const [facts] = await p.query("project.facts", {});
      expect(facts.sprints[0].attachments).toEqual(attachments);
    });

    it("carries review comments, edits, queue edits and resolution guidance without losing bytes", async () => {
      const attachments = [{name:"screen.png",mimeType:"image/png",data:btoa("fixture")}];
      const comment = {id:"c",path:"a.txt",line:1,text:"Fix",sent:false,attachments};
      const message = {id:"q",taskId:"task-1",workerId:null,text:"Queued",mode:"queue",delivery:"queued",at:1,deliveredAt:null,failure:null,attachments};
      const {s,p,asked,ready} = helper({comments:{"task-1":[comment]},messages:[message]});
      await ready();
      for (const payload of [
        {type:"add_comment",taskId:"task-1",path:"a.txt",line:1,text:"Look",attachments},
        {type:"edit_comment",taskId:"task-1",commentId:"c",text:"Changed",attachments},
        {type:"edit_queued",messageId:"q",text:"Changed",attachments},
        {type:"send_comments",taskId:"task-1"},
        {type:"resolve_with_cortex",taskId:"task-1",guidance:"Keep this",attachments},
      ] as WorkspaceCommand[]) expect(await p.command(envelope(payload))).toMatchObject({ok:true});
      expect(asked.find(c => c.op === "product.comment" && c.params?.action === "add")?.params).toMatchObject({comment:{attachments}});
      expect(asked.find(c => c.op === "product.comment" && c.params?.action === "edit")?.params).toMatchObject({attachments});
      expect(asked.find(c => c.op === "product.message" && c.params?.action === "edit")?.params).toMatchObject({attachments});
      expect(s.calls.filter(c => c.operation === "executions.prompt").every(c => JSON.stringify((c.message as {attachments?:unknown}).attachments) === JSON.stringify(attachments))).toBe(true);
      const before = s.calls.length;
      for (const payload of [
        {type:"add_comment",taskId:"task-1",path:"a.txt",line:1,text:"Invalid"},
        {type:"edit_comment",taskId:"task-1",commentId:"c",text:"Invalid"},
        {type:"edit_queued",messageId:"q",text:"Invalid"},
        {type:"resolve_with_cortex",taskId:"task-1",guidance:"Invalid"},
        {type:"plan_message",sprintId:"missing",text:"Invalid"},
      ]) expect(await p.command(envelope({...payload,attachments:["name only"]} as WorkspaceCommand))).toMatchObject({ok:false});
      // Local validation of message-bearing intents never emits a prompt with bad bytes.
      expect(s.calls.slice(before).some(c => c.operation === "executions.prompt")).toBe(false);
    });

    it("reads one task\u0027s saved creation bytes on demand", async () => {
      const {s,p,ready} = helper();
      await ready();
      const attachments = [{name:"screen.png",mimeType:"image/png",data:"eA=="}];
      const original = s.answers["machine.call"]!;
      s.answers["machine.call"] = message => (message as {params?:{attachments?:boolean}}).params?.attachments
        ? {ok:true,result:attachments} : original(message);
      expect(await p.readTaskAttachments!("task-1")).toEqual(attachments);
      await expect(p.readTaskAttachments!("not-a-task")).rejects.toThrow("Unknown task");
    });

    it("does not silently turn a stale attached planning answer into plain steering", async () => {
      const {s,p,asked,ready} = helper({sprints:[{id:"sp",projectId:"p",planningTaskId:"task-1",planning:"question",conversation:[]}]});
      await ready();
      expect(await p.command(envelope({type:"answer_planning",sprintId:"sp",answer:"Keep files",attachments:[{name:"a",mimeType:"text/plain",data:"YQ=="}]}))).toMatchObject({ok:false,outcome:"superseded"});
      expect(s.calls.some(c => c.operation === "executions.prompt")).toBe(false);
      expect(asked.some(c => c.op === "product.sprint" && c.params?.action === "note")).toBe(false);
    });

    it("rejects name-only or oversized attachments before prompt delivery", async () => {
      const {s,p,ready} = helper();
      await ready();
      for (const attachments of [["legacy.txt"], [{name:"big",mimeType:"text/plain",data:btoa("x".repeat(LIMITS.attachmentBytes + 1))}]]) {
        expect(await p.command(envelope({type:"send_message",taskId:"task-1",text:"Keep me",mode:"send_now",attachments}))).toMatchObject({ok:false});
      }
      expect(s.calls.some(c => c.operation === "executions.prompt")).toBe(false);
    });

    it("returns draft review comments to Cortex before marking them sent", async () => {
      const { s, p, asked, ready } = helper({ comments: { "task-1": [{ id:"c1", path:"a.txt", line:3, text:"Keep both lines\nunchanged", sent:false }] } });
      await ready();
      expect(await p.command(envelope({ type:"send_comments", taskId:"task-1" }))).toMatchObject({ok:true});
      expect(s.calls.find(c => c.operation === "executions.prompt")!.message).toMatchObject({text:"Please address these review comments:\n\na.txt:3\nKeep both lines\nunchanged"});
      expect(asked.find(c => c.op === "product.comment")?.params).toEqual({action:"mark_sent",taskId:"task-1",ids:["c1"]});
      expect(asked.some(c => c.op === "product.task" && c.params?.action === "reopen")).toBe(true);
    });

    it("keeps review comments unsent when durable prompt capture fails", async () => {
      const { s, p, asked, ready } = helper({ comments: { "task-1": [{ id:"c1", path:"a.txt", line:3, text:"Fix this", sent:false }] } });
      await ready();
      s.answers["executions.prompt"] = () => ({outcome:"unsupported"});
      expect(await p.command(envelope({type:"send_comments",taskId:"task-1"}))).toMatchObject({ok:false});
      expect(asked.some(c => c.op === "product.comment" && c.params?.action === "mark_sent")).toBe(false);
    });

    it("binds result commands to displayed facts, preserving stale, cleared and missing-version refusals", async () => {
      const result = {version:2,sourceVersion:"47",digest:"native-digest",summary:"Native",checks:[{name:"Native check",outcome:"not_run",detail:"",command:["/bin/true"],kind:"automated",rerunnable:true}]};
      const overlay: {result:typeof result | null} = {result};
      const {s,p,asked,ready} = helper({tasks:{"task-1":overlay}});
      await ready();
      expect(await p.command(envelope({type:"accept_result",taskId:"task-1",digest:"old"}))).toMatchObject({ok:false,outcome:"stale_result"});
      expect(asked.some(c => c.op === "product.task")).toBe(false);
      expect(await p.command(envelope({type:"rerun_check",taskId:"task-1",digest:result.digest,checkIndex:0}))).toMatchObject({ok:true});
      expect(s.calls.find(c => c.operation === "executions.control")?.message).toMatchObject({checkName:"Native check",resultVersion:"47",digest:result.digest});
      expect(await p.command(envelope({type:"accept_result",taskId:"task-1",digest:result.digest,openChecks:true}))).toMatchObject({ok:true});
      expect(asked.find(c => c.op === "product.task")?.params).toMatchObject({action:"accept",version:"47",digest:result.digest,acknowledgeOpenChecks:true});

      for (const missing of [null, {...result,sourceVersion:""}]) {
        overlay.result = missing;
        await p.query("project.facts",{fresh:true});
        asked.length=0;
        expect(await p.command(envelope({type:"accept_result",taskId:"task-1",digest:result.digest}))).toMatchObject({ok:false,outcome:"stale_result"});
        expect(asked.some(c => c.op === "product.task")).toBe(false);
      }
      overlay.result = null;
      await p.query("project.facts",{fresh:true});
      s.calls.length=0;
      expect(await p.command(envelope({type:"rerun_check",taskId:"task-1",digest:"result:res:1",checkIndex:0}))).toMatchObject({ok:false,outcome:"stale_result"});
      expect(s.calls.some(c => c.operation === "executions.control")).toBe(false);
    });

    it("retains the server's stale refusal after a result command is dispatched", async () => {
      const result = {version:2,sourceVersion:"47",digest:"native-digest",summary:"Native",checks:[]};
      const {s,p,ready} = helper({tasks:{"task-1":{result}}});
      await ready();
      const answer=s.answers["machine.call"]!;
      s.answers["machine.call"]=message => (message as {op:string}).op === "product.task"
        ? {ok:false,code:"stale_result",message:"Files changed after review"} : answer(message);
      expect(await p.command(envelope({type:"accept_result",taskId:"task-1",digest:result.digest}))).toMatchObject({ok:false,outcome:"stale_result",message:"Files changed after review"});
    });

    it("sends a forced check request bound to the current result, and refuses a stale result", async () => {
      const { s, p, ready } = helper();
      s.answers["results.get"] = () => ({kind:"answer",resultId:"res",taskId:"task-1",executionId:"e",version:"47",sequenceNumber:"1",text:"Done",evidence:[],checks:[{name:"Focused test",outcome:"not_run",evidenceRef:null,detail:"Technical blocker",command:["python3","check.py"],rerunnable:true}],observedWorkspaceDigest:null});
      await ready();
      const [update] = await p.query("project.snapshot", {}) as Extract<ProjectionUpdate,{scope:"project"}>[];
      const result = update.slice.tasks[0].result!;
      expect(await p.command(envelope({type:"rerun_check",taskId:"task-1",digest:"stale",checkIndex:0}))).toMatchObject({ok:false,outcome:"stale_result"});
      expect(await p.command(envelope({type:"rerun_check",taskId:"task-1",digest:result.digest,checkIndex:0}))).toMatchObject({ok:true});
      expect(s.calls.find(c => c.operation === "executions.control")!.message).toMatchObject({action:"rerun_check",checkName:"Focused test",checkIndex:0,digest:result.digest,resultVersion:"47"});
      expect(result.version).toBe(1);
    });

    it("uses native host check observations when the ACP result adapter is absent", async () => {
      const { s, p, ready } = helper();
      s.answers["capabilities.get"] = () => capabilitiesWire(ALL.filter(op => op !== "results.get"));
      await ready();
      s.answers["machine.call"] = message => {
        const call = message as {op:string};
        if (call.op === "product.read") return {ok:true,result:{tasks:{"task-1":{result:{version:17,sourceVersion:"17",digest:"native-digest",summary:"Host checks",acceptanceAvailable:false,checks:[{name:"Host check",outcome:"failed",detail:"owner_stopped",command:["/bin/true"],kind:"automated",rerunnable:true}]}}},sprints:[]}};
        return {ok:true,result:{}};
      };
      expect(await p.command(envelope({type:"rerun_check",taskId:"task-1",digest:"native-digest",checkIndex:0}))).toMatchObject({ok:true});
      expect(s.calls.find(c => c.operation === "executions.control")!.message).toMatchObject({action:"rerun_check",checkName:"Host check",digest:"native-digest",resultVersion:"17"});
    });
    it("does not send rerun requests for manual or policy items", async () => {
      for (const kind of ["manual", "policy"]) {
        const { s, p, ready } = helper();
        s.answers["results.get"] = () => ({kind:"answer",resultId:"res",taskId:"task-1",executionId:"e",version:"47",sequenceNumber:"1",text:"Done",evidence:[],checks:[{name:"Owner check",outcome:"not_run",evidenceRef:null,kind,rerunnable:false}],observedWorkspaceDigest:null});
        await ready();
        const [update] = await p.query("project.snapshot", {}) as Extract<ProjectionUpdate,{scope:"project"}>[];
        const result = update.slice.tasks[0].result!;
        expect(await p.command(envelope({type:"rerun_check",taskId:"task-1",digest:result.digest,checkIndex:0}))).toMatchObject({ok:false,outcome:"unsupported"});
        expect(s.calls.some(c => c.operation === "executions.control")).toBe(false);
      }
    });

    it("sends a Git action for its checkout and shows the machine's refusal in its own words", async () => {
      const { p, asked, ready } = helper({}, "git");
      await ready();
      const result = await p.command(envelope({ type: "git_commit", checkoutId: "p:main", message: "x" }));
      expect(asked.find(a => a.op === "git")).toMatchObject({ projectId: "p", params: { action: "commit", checkoutId: "p:main", message: "x" } });
      expect(result).toMatchObject({ ok: false, outcome: "git_failed", message: "nothing to commit" });
    });

    it("forwards each Git command identity and the push dialog URL exactly once, while fetch and pull need no remote field", async () => {
      const { p, asked, ready } = helper();
      await ready();
      const remote = "git@github.com:igovet/cortex-orch-acp.git";
      expect(await p.command(envelope({type:"git_push",checkoutId:"p:main",remote,branch:"dev",confirmed:true}))).toMatchObject({ok:true});
      await p.command(envelope({type:"git_fetch",projectId:"p",now:1}));
      await p.command(envelope({type:"git_pull",checkoutId:"p:main",now:1}));
      expect(asked.filter(a => a.op === "git")).toEqual([
        {op:"git",projectId:"p",params:{requestId:"req-1",action:"push",checkoutId:"p:main",remote,branch:"dev",confirmed:true}},
        {op:"git",projectId:"p",params:{requestId:"req-1",action:"fetch"}},
        {op:"git",projectId:"p",params:{requestId:"req-1",action:"pull",checkoutId:"p:main"}},
      ]);
    });

    it("keeps a commit message draft local, so typing never reaches the machine", async () => {
      const { p, asked, ready } = helper();
      await ready();
      const before = asked.length;
      expect(await p.command(envelope({ type: "git_draft", checkoutId: "p:main", message: "wip" }))).toMatchObject({ ok: true, outcome: "local" });
      expect(asked).toHaveLength(before);
      const [facts] = await p.query("project.facts", {});
      expect(facts.repo?.checkouts[0].draft).toBe("wip");
    });

    it("finishes an in-flight machine read when activation or a save invalidates the config cache", async () => {
      const {s,p,ready} = helper();
      await ready();
      const answer=s.answers["machine.call"]!;
      let release!: () => void, entered!: () => void;
      const held=new Promise<void>(resolve => {release=resolve;});
      const reading=new Promise<void>(resolve => {entered=resolve;});
      s.answers["machine.call"]=async message => {
        if ((message as {op:string}).op==="project.config.read") {entered(); await held;}
        return answer(message);
      };
      const pending=p.query("machine.facts",{});
      await reading;
      expect((await p.command(envelope({type:"save_surface",machineId:"local",surface:"routing",value:{deny:[]},revision:1}))).ok).toBe(true);
      release();
      const [facts]=await pending;
      expect(facts.patch.config?.projects.p).toEqual({});
      expect(facts.patch.connections).toEqual([]);
      expect(facts.engineUsage![0]).toMatchObject({ engine: "codex" });
    });

    it("saves a surface with the revision it read, then rereads the configuration", async () => {
      const { p, asked, ready } = helper();
      await ready();
      await p.command(envelope({ type: "save_surface", machineId: "local", surface: "routing", value: { deny: [] }, revision: 1 }));
      expect(asked.find(a => a.op === "config.save")!.params).toMatchObject({ surface: "routing", tokens: { routing: "sha256:a" } });
      await p.query("machine.facts", {});
      expect(asked.filter(a => a.op === "config.read")).toHaveLength(2);
    });

    it.each([false, true])("starts a confirmed rename without waiting for board reconciliation (late read: %s)", async late => {
      const {s,p,ready} = helper();
      s.answers["project.snapshot"] = () => snapshot("4", [wireTask({version:"1",executionId:null})]);
      await ready();
      let finish!: (value: unknown) => void;
      const pending = late ? (() => {
        s.answers["project.snapshot"] = () => new Promise(resolve => {finish=resolve;});
        return p.query("project.snapshot", {});
      })() : null;
      s.answers["machine.call"] = () => ({ok:true,result:{done:true,version:"2"}});
      expect((await p.command(envelope({type:"rename_task",taskId:"task-1",title:"Owner title"}))).ok).toBe(true);
      if (pending) { finish(snapshot("4", [wireTask({version:"1",executionId:null})])); await pending; }
      await p.command(envelope({type:"start_task",taskId:"task-1"}));
      expect(s.calls.find(c => c.operation==="tasks.start")!.message).toMatchObject({expectedVersion:"2"});
    });

    it("does not invent a rename version on refusal or replay a stale Start", async () => {
      const {s,p,ready} = helper();
      s.answers["project.snapshot"] = () => snapshot("4", [wireTask({version:"1",executionId:null})]);
      await ready();
      s.answers["machine.call"] = () => ({ok:false,code:"task_started",message:"Task started"});
      expect((await p.command(envelope({type:"rename_task",taskId:"task-1",title:"Not saved"}))).ok).toBe(false);
      s.answers["tasks.start"] = message => receipt(message,"stale_version");
      expect((await p.command(envelope({type:"start_task",taskId:"task-1"}))).outcome).toBe("stale_version");
      const starts=s.calls.filter(c => c.operation==="tasks.start");
      expect(starts).toHaveLength(1);
      expect(starts[0].message).toMatchObject({expectedVersion:"1"});
    });

    it("a lost reply stays unknown, and an intent with no machine effect stays unsupported", async () => {
      const { s, p, ready } = helper();
      await ready();
      s.answers["machine.call"] = () => { throw new Error("socket closed"); };
      expect(await p.command(envelope({ type: "rename_task", taskId: "task-1", title: "T" }))).toMatchObject({ ok: false, outcome: "unknown_outcome" });
      expect(await p.command(envelope({ type: "pair_machine", id: "m", name: "M", code: "c", fingerprintsMatch: true }))).toMatchObject({ ok: false, outcome: "unsupported" });
    });

    it("sends cleanup consent and preserves a partial settings failure", async () => {
      const {s,p,asked,ready} = helper();
      await ready();
      await p.command(envelope({type:"integrate",taskId:"task-1",method:"squash",confirmed:true,digest:"d",targetBranch:"main",message:"Task",deleteWorktree:true,now:1}));
      expect(asked.find(a => a.op === "product.integrate")?.params).toMatchObject({deleteWorktree:true});
      s.answers["executions.control"] = () => ({outcome:"partial_effect",message:"Applied: model, effort. Not confirmed: language, mode."});
      expect(await p.command(envelope({type:"set_task_settings",taskId:"task-1",settings:{model:"r"}}))).toMatchObject({ok:false,outcome:"partial_effect",message:expect.stringContaining("Not confirmed: language, mode")});
    });

    it("lists unsaved MCP tools through the real service seam", async () => {
      const {s,p,ready} = helper();
      await ready();
      const previous = s.answers["machine.call"];
      s.answers["machine.call"] = message => (message as {op:string}).op === "config.mcp.discover"
        ? {ok:true,result:{ok:true,observation:{at:1,name:"draft",version:"reported",tools:[{name:"read",description:"",hint:"read"}]}}}
        : previous!(message);
      const result = await p.discoverMcp!({name:"draft"} as never);
      expect(result).toMatchObject({ok:true,observation:{tools:[{name:"read"}]}});
    });

    it.each(["main", "worktree"] as const)("materialized tasks inherit sprint route and %s checkout before any start", async kind => {
      const settings = {model:"connection-two:model",effort:"high",mode:"guarded",language:"en"} as const;
      const sprint = {id:"sp",projectId:"p",description:"Sprint",runMode:"automatic",checkout:{kind},targetBranch:"release",settings,planning:"ready",conversation:[],plan:{version:1,items:[{key:"a",title:"Planned name",description:"First",dependsOn:[]},{key:"b",description:"Next",dependsOn:["a"]}]},materializedVersion:null,taskIds:[],queueState:"idle",pauseReason:null};
      const {s,p,ready} = helper({sprints:[sprint]});
      await ready();
      s.answers["tasks.create"] = message => receipt(message,"recorded",(message as {requestId:string}).requestId);
      expect(await p.command(envelope({type:"materialize_plan",sprintId:"sp",version:1}))).toMatchObject({ok:true});
      const creates = s.calls.filter(c => c.operation === "tasks.create");
      expect(creates).toHaveLength(2);
      for (const call of creates) expect(call.message).toMatchObject({payload:{settings,checkout:{kind}}});
      expect(s.calls.some(c => (c.message as {params?:{action?:string}}).params?.action === "rename")).toBe(false);
      // Live run: every sprint task read as the first sentence of its description. The plan's own title names
      // the task when it joins the sprint; an item without one keeps the description label.
      const joins = s.calls.map(c => (c.message as {params?:{action?:string;title?:string}}).params).filter(p => p?.action === "sprint");
      expect(joins.map(p => p!.title)).toEqual(["Planned name", undefined]);
      expect(s.calls.some(c => c.operation === "tasks.start")).toBe(false);
    });

    it("reading an automatic sprint never schedules work in the window", async () => {
      const sprint = { id: "sp", projectId: "p", description: "S", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["a", "b"], queueState: "running", pauseReason: null };
      const { s, p, asked, ready } = helper({ sprints: [sprint], tasks: { a: { status: "ready_to_merge" }, b: { dependencies: ["a"] } } });
      s.answers["project.snapshot"] = () => snapshot("4", [wireTask({ taskId: "a", executionId: "e", phase: "result", runtimeState: "COMPLETED", executionState: "result" }), wireTask({ taskId: "b" })]);
      s.answers["results.get"] = () => ({ kind: "answer", resultId: "res", taskId: "a", executionId: "e", version: "1", text: "Done", evidence: [], checks: [], observedWorkspaceDigest: null });
      s.answers["executions.get"] = () => ({ executionId: "e", taskId: "a", version: "2", runtimeTaskId: "r", acpSessionId: "s", runtimePlanRevision: "0", ownerEpoch: "3", state: "result", warnings: [], resultId: "res" });
      await ready();
      expect(s.calls.filter(c => c.operation === "tasks.start").map(c => (c.message as { target: string }).target)).toEqual([]);
      expect(asked.some(a => a.op === "product.sprint" && (a.params?.patch as { queueState?: string })?.queueState === "complete")).toBe(false);
    });

    it("reads a plan only from a block that can run", () => {
      expect(planFrom("Here:\n```json\n{\"tasks\":[{\"key\":\"a\",\"description\":\"Do\",\"dependsOn\":[]}]}\n```")).toEqual([{ key: "a", title: "Do", description: "Do", dependsOn: [] }]);
      expect(planFrom("no plan")).toBeNull();
      expect(planFrom("```json\n{\"tasks\":[{\"key\":\"a\",\"description\":\"Do\",\"dependsOn\":[\"a\"]}]}\n```")).toBeNull();
      expect(planFrom("```json\nnot json\n```")).toBeNull();
    });
  });

  it("the transport for a build with no service fails, loudly and with no data", async () => {
    await expect(createRealPort({ transport: unavailableTransport(), actorId: "local-peer" }).query("capabilities.get", {})).rejects.toThrow(/not connected/i);
  });
});
