import { render } from "solid-js/web";
import { afterEach, describe, expect, it } from "vitest";
import { decodeResponse } from "@cortex/workspace-contract";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, mapDecision, type RepoCheckout, type WorkspaceState } from "@cortex/workspace-domain";
import { archiveRow } from "../src/archive-model";
import { fileType } from "../src/code";
import { archiveBlocked, DIVERGED, gitItems, gitPrimary, PULL_FIRST, type GitFacts } from "../src/git-actions-model";
import { decodeEntities, ResultText } from "../src/result-text";
import { buildIndex } from "../src/selectors";
import { queueStall } from "../src/sprint-model";
import { Workspace } from "../src";
import { draftTask, flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const mount = (view: () => import("solid-js").JSX.Element) => {
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(view, root);
};

const checkout = (patch: Partial<RepoCheckout> = {}): RepoCheckout => ({ id: "p:main", projectId: "p", kind: "main", branch: "main", path: "/p", taskId: null, label: null, base: "origin/main", upstream: "origin/main", ahead: 0, behind: 0, behindOf: {}, files: [], draft: "", sources: {}, updateConflict: null, ...patch });
function world(c: RepoCheckout): WorkspaceState {
  const s = emptyWorkspace();
  s.machines[0].connection = "connected";
  s.projects = [{ id: "p", machineId: s.machines[0].id, name: "Project", repository: "/p", openBranch: "main", defaultBranch: "main", hasGit: true }];
  s.repos = [{ projectId: "p", hasGit: true, remote: "git@example.invalid:o/p.git", tree: [], sources: {}, commits: [], branches: [], checkouts: [c], fetchedAt: null }];
  return s;
}
const facts = (s: WorkspaceState): GitFacts => ({ checkout: s.repos[0].checkouts[0], repo: s.repos[0], task: null, running: null, gate: () => undefined, offered: () => true });

describe("a diverged checkout in the title band", () => {
  it("offers no push or pull that cannot succeed, and says why and what to do", () => {
    const f = facts(world(checkout({ ahead: 1, behind: 2 })));
    const why = Object.fromEntries(gitItems(f).map(i => [i.id, i.why]));
    expect(why.push).toBe(`${PULL_FIRST} ${DIVERGED}`);
    expect(why.push).toContain("The remote has commits you do not have. Pull first.");
    // Pull… asks how to bring the remote commits in (merge or rebase); Pull and push stays off until the branch has them.
    expect(why.pull).toBeUndefined();
    expect(why.pull_push).toBe(DIVERGED);
    expect(DIVERGED).toMatch(/diverged/);
    expect(DIVERGED).toMatch(/merge or rebase/);
    expect(DIVERGED).not.toMatch(/force/i);
    expect(gitPrimary(f)).toMatchObject({ id: "pull", label: "Pull…", why: undefined });
    // Only ahead, or only behind, is unchanged.
    expect(gitPrimary(facts(world(checkout({ ahead: 1 }))))).toMatchObject({ id: "push", why: undefined });
    expect(gitPrimary(facts(world(checkout({ behind: 1 }))))).toMatchObject({ id: "pull", why: undefined });
  });
});

describe("archiving follows the machine's own rule", () => {
  it("offers Archive independently of task, runtime and conflict state", () => {
    for (const status of ["draft", "working", "needs_you", "stopping", "paused", "cancelled", "closed_unverified", "done"]) {
      const task = { status, archive: null, observedPhase: "unknown", warnings: ["contradictory_evidence"] };
      expect(archiveBlocked(task)).toBeUndefined();
    }
    expect(archiveBlocked({ archive: { at: 1, previousStatus: "working" } })).toBe("Already in the Archive");
    const machine = world(checkout()).machines[0];
    expect(archiveBlocked({ archive: null }, { ...machine, connection: "disconnected" })).toMatch(/offline/i);
  });

  it("never shows more days left than the machine keeps an archived task", () => {
    const s = world(checkout());
    s.machines[0].retentionDays = 30;
    // Archived after this view's clock last moved.
    s.tasks = [{ ...draftTask(), status: "done", archive: { at: 1_000_000 + 60_000, previousStatus: "done" } }];
    const index = buildIndex(s, 1_000_000);
    const v = index.archive[0] ?? index.byId.get(s.tasks[0].id)!;
    expect(archiveRow(index, v).left).toBe(30);
  });

  it("uses the completion-based cleanup clock for a Done sprint archive", () => {
    const s = world(checkout()), day = 86400000;
    s.machines[0].retentionDays = 30;
    s.tasks = [{ ...draftTask(), status: "done", work: "merged", doneAt: day,
      archive: { at: 20 * day, previousStatus: "done", retentionAt: 8 * day } }];
    const index = buildIndex(s, 20 * day);
    expect(archiveRow(index, index.archive[0]).left).toBe(18);
  });
});

describe("a sprint's dependency note", () => {
  it("names the step its column shows: Ready to finish for a task on its target branch", () => {
    const s = world(checkout());
    const dep = { ...draftTask(), id: "dep", title: "First", status: "working" as const, sprintId: "sp" };
    const next = { ...draftTask(), id: "next", title: "Second", status: "draft" as const, sprintId: "sp", dependencies: ["dep"] };
    s.tasks = [dep, next];
    s.sprints = [{ id: "sp", projectId: "p", description: "", title: "Sprint", runMode: "manual", targetBranch: null, planning: "created", planningTaskId: null, materializedVersion: 1, plan: { version: 1, items: [] }, conversation: [], taskIds: ["dep", "next"], queueState: "idle", pauseReason: null } as never];
    const index = buildIndex(s, 1);
    const stall = queueStall(index, s.sprints[0]);
    expect(stall?.reason).toBe(`Second waits for First to reach Ready to finish`);
  });
});

describe("the Code inspector's file facts", () => {
  it("names a picture or binary file's type instead of a language", () => {
    expect(fileType("assets/logo.png")).toBe("Image (PNG)");
    expect(fileType("dist/app.wasm")).toBe("Binary file (WASM)");
    expect(fileType("docs/spec.pdf")).toBe("PDF document");
    expect(fileType("src/a.ts")).toBeUndefined();
  });
});

describe("a native permission decision", () => {
  it("is a permission card although the contract's decoder reads its kind as unknown", () => {
    const page = { items: [{ decisionId: "wd_np1", kind: "permission", questionId: "np1", batchId: null, executionId: "e", acpSessionId: "s", scope: "task",
      prompt: "Owner decision", options: [{ optionId: "wo_allow", label: "Allow once" }, { optionId: "wo_deny", label: "Deny" }], version: "7", ownerEpoch: "1",
      requirementRevision: "0", status: "pending", allowedOperations: ["decisions.answer"],
      permission: { operation: "read", paths: ["/home/o/a.txt"], reason: "outside_project", reasonText: "it reads outside the project", access: "guarded", rule: "outside_read" } }],
      nextCursor: null, highWatermark: "1" };
    const decoded = decodeResponse("DecisionPage", page).value.items[0];
    expect(decoded.kind).toBe("unknown");
    const d = mapDecision(decoded);
    expect(d.kind).toBe("permission");
    expect(d.permission?.paths).toEqual(["/home/o/a.txt"]);
    // A question stays a question.
    expect(mapDecision({ ...decoded, kind: "question", permission: undefined } as never).kind).toBe("question");
  });
});

describe("a runtime stop question rendered in its task", () => {
  it("has full-width answer rows, the Recommended mark and the targets it names", async () => {
    const state = withTask(emptyWorkspace(), "Fixture task");
    state.machines[0].connection = "connected";
    state.tasks[0].status = "needs_you"; state.tasks[0].process = "live";
    const session = emptySession("t");
    session.decisions = [{ id: "d1", kind: "question", from: "", workerId: null, at: null, withdrawn: false, answer: null, text: "Stopped: files changed after review.",
      options: [
        { label: "Продолжить: координатор решит, как действовать дальше", sub: "координатор заново отправит текущие файлы на ревью и проверку", optionId: "a", recommended: true },
        { label: "Завершить", sub: "задача закрывается как есть", optionId: "b" }],
      targets: [{ kind: "report", reportId: "r_0123456789ab", role: "code-reviewer" }, { kind: "task", taskId: "t2", title: "Other" }] }];
    session.timeline = [{ id: "e-d1", kind: "decision", decisionId: "d1" }];
    state.sessions = [session];
    const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
    history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
    mount(() => <Workspace port={p.port} />);
    for (let i = 0; i < 8; i++) await flush();
    p.emit(projection("1", state));
    for (let i = 0; i < 8; i++) await flush();
    expect([...document.querySelectorAll(".ws-dc-targets .btn")].map(b => b.textContent)).toEqual(["Open review findings", "Open task “Other”"]);
    const group = document.querySelector(".dc-opts")!;
    expect(group.classList.contains("dc-opts-rows")).toBe(true);
    const options = [...group.querySelectorAll(".opt")];
    expect(options).toHaveLength(2);
    for (const o of options) {
      // The label and its consequence are two blocks of one option, in that order.
      expect([...o.children].map(c => c.className)).toEqual(["n", "t", "sub"]);
    }
    expect(options[0].querySelector(".t .ws-dc-rec")?.textContent).toBe("Recommended");
  });
});

describe("message text", () => {
  it("reads character references as the characters they stand for, never as markup, and keeps them literal in code", () => {
    expect(decodeEntities("a&#x20;b &#32;c &amp; &lt;b&gt; &nbsp;&copy; &#0; &bogus;")).toBe("a b  c & <b>  © � &bogus;");
    mount(() => <ResultText text={"The end of a line&#x20;\nnext &amp; last &lt;img src=x&gt;\n\n`&amp; stays`\n\n```\n&#x20; stays too\n```"} />);
    const text = document.body.textContent!;
    expect(document.querySelector("p")!.textContent).not.toContain("&#x20;");
    expect(text).toContain("next & last <img src=x>");
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("p code")?.textContent).toBe("&amp; stays");
    expect(document.querySelector("pre")?.textContent).toBe("&#x20; stays too");
  });

  it("shows an image reference as a labelled reference and loads no picture", () => {
    mount(() => <ResultText text={"See ![image](https://example.invalid/shots/run.png) and ![](docs/diagram.svg) or ![chart](javascript:void)."} />);
    expect(document.querySelector("img")).toBeNull();
    const refs = [...document.querySelectorAll(".ws-img-ref")];
    expect(refs.map(r => r.textContent)).toEqual(["Image: image (example.invalid · run.png)", "Image: diagram.svg", "Image: chart (javascript:void)"]);
    expect(refs[0].tagName).toBe("A");
    expect(refs[0].getAttribute("href")).toBe("https://example.invalid/shots/run.png");
    expect(refs[1].tagName).toBe("SPAN");
    expect(refs[2].tagName).toBe("SPAN");
    expect(document.body.textContent).not.toContain("!");
  });
});
