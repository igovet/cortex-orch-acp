import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { loadFixtures } from "../../fixtures";
import { WORKSPACE_CASES, WORKSPACE_FIXTURE_NOW } from "../adapter/fixtures";
import { archiveView, NO_FILTER } from "@cortex/workspace-ui/archive-model";
import Workspace from "./index";
import { buildIndex } from "@cortex/workspace-ui/selectors";
import { planCaption, planGraph, sprintSummary } from "@cortex/workspace-ui/sprint-model";

const LOCAL = "mock-machine-00000", REMOTE = "mock-machine-00001", GPU = "mock-machine-00002", CEDAR = "mock-project-00001";
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

const tick = () => new Promise(r => setTimeout(r, 0));
async function mount(path: string) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace autoplay={false} />, root);
  await tick();
  return root;
}
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const named = (selector: string, name: string) => all(selector).find(el => (el.getAttribute("aria-label") ?? el.textContent ?? "").includes(name))!;
const click = async (el: Element) => { (el as HTMLElement).click(); await tick(); };
const type = async (el: HTMLInputElement | HTMLTextAreaElement, value: string) => { el.value = value; el.dispatchEvent(new Event("input", { bubbles: true })); await tick(); };
const index = () => buildIndex(loadFixtures().workspace, WORKSPACE_FIXTURE_NOW);
const auditPath = () => { const s = loadFixtures().workspace.sprints.find(x => x.title === "Audit log export")!; return `/m/${LOCAL}/p/${CEDAR}/s/${s.id}/planning`; };
const mockStep = async () => click(named("#m-mock .mi", "Simulate the next step"));

describe("sprint planning", () => {
  it("reads the approved screen: conversation, open question, Plan v2 with its dependencies", async () => {
    await mount(auditPath());
    expect($(".crumbs .ws-title").textContent).toBe("Sprint: Audit log export");
    expect($(".sp-h .tt").textContent).toBe("Audit log export");
    expect($(".sp-conv .dc-q").textContent).toBe("Should the export include failed login attempts?");
    expect(all(".sp-plan .pt-row:not(.th)").length).toBe(5);
    expect($(".sp-plan-h").textContent).toContain("Plan v2");
    expect(all(".pt-row .dep")[4].textContent).toBe("After 2, 4");
    expect($(".depg svg").getAttribute("aria-label")).toContain("Task 1 leads to 2, 3.");
    expect(named('[role="tab"]', "Planning").getAttribute("aria-selected")).toBe("true");
    expect(named(".tb-fixed .ib", "Inspector").getAttribute("aria-disabled")).toBe("true");
  });

  it("will not create tasks over an open question, then creates them once and offers task 1", async () => {
    await mount(auditPath());
    const create = () => named(".sp-h .btn-primary", "Create 5 tasks");
    expect(create().getAttribute("aria-disabled")).toBe("true");
    expect(create().title).toContain("Answer the open question");
    await click(create());
    expect($(".sp-h .btn-primary").textContent).toContain("Create 5 tasks");
    await click(named(".sp-conv .opt", "Only for admin accounts"));
    expect($(".sp-conv .dc")).toBeNull();
    expect(all(".sp-conv .qa-a").map(a => a.textContent)).toContain("Only for admin accounts");
    await mockStep();
    expect($(".sp-plan-h").textContent).toContain("Plan v3");
    await click(named(".sp-h .btn-primary", "Create 5 tasks from Plan v3"));
    expect($(".sp-h .st-ok").textContent).toContain("5 tasks created from Plan v3");
    expect($(".sp-h .btn-primary").textContent).toBe("Start task 1");
    expect(all(".pt-row .dep").map(el => el.textContent)).toEqual(["Ready", "Waits for 1", "Waits for 1", "Waits for 3", "Waits for 2, 4"]);
    expect($<HTMLTextAreaElement>(".sp-conv textarea").disabled).toBe(true);
    await click($(".sp-h .btn-primary"));
    expect(all(".pt-row .dep").map(el => el.textContent).slice(0, 2)).toEqual(["Working", "Waits for 1"]);
  });

  it("an Other answer takes line breaks", async () => {
    await mount(auditPath());
    await click(named(".sp-conv .opt", "Other answer"));
    const field = $<HTMLTextAreaElement>('.sp-conv [aria-label="Your answer"]');
    expect(field.tagName).toBe("TEXTAREA");
    expect(named(".ws-dc-other .ib", "Attach files")).toBeTruthy();
    await type(field, "First\nSecond");
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", shiftKey: true, bubbles: true }));
    await tick();
    expect($(".sp-conv .dc")).toBeTruthy();
    await click(named(".ws-dc-other .btn", "Send answer"));
    expect($(".sp-conv").textContent).toContain("First\nSecond");
  });

  it("refuses a cycle in the row with the reason, and Done editing makes a new version", async () => {
    await mount(auditPath());
    await click(named(".sp-plan-h .btn", "Edit"));
    await click(named("#m-dep-3 .mi", "4 Add the export page"));
    expect($(".pt-row .err").textContent).toContain("Not applied: 3 after 4 makes a cycle, because 4 already waits for 3.");
    expect(all(".ws-dep")[2].textContent).toBe("1");
    await type(all<HTMLInputElement>(".pt-row .inp")[0], "Add the audit table");
    await click(named(".sp-plan-h .btn", "Done editing"));
    expect($(".sp-plan-h").textContent).toContain("Plan v3");
    expect(all(".pt-row .tt")[0].textContent).toBe("Add the audit table");
    await click(named("#m-plan-history .mi", "Plan v2"));
    expect($(".sp-plan-h").textContent).toContain("Plan v2");
    expect(all(".pt-row .tt")[0].textContent).toBe("Add the audit events table");
    await click(named(".sp-plan-h .btn", "Back to v3"));
    expect(all(".pt-row .tt")[0].textContent).toBe("Add the audit table");
  });

  it("asks for changes through the composer, and cancelling keeps everything for later", async () => {
    await mount(auditPath());
    await click(named(".sp-conv .opt", "Yes, all of them"));
    await mockStep();
    const box = $<HTMLTextAreaElement>(".sp-conv textarea");
    await type(box, "Split the export page in two");
    await click(named(".sp-conv .c-send", "Send to the planner"));
    expect($(".sp-conv").textContent).toContain("Split the export page in two");
    expect($(".sp-h .btn-primary").title).toContain("Still working on the plan");
    await click(named(":is(.strip, .l-strip) .btn", "Cancel planning"));
    expect($(".sp-h .st-idle").textContent).toContain("Planning cancelled");
    expect($(".sp-plan-h").textContent).toContain("Plan v3");
    await click(named(":is(.strip, .l-strip) .btn", "Resume planning"));
    expect($(".sp-h .st-idle").textContent).toContain("Planning");
  });

  it("a new sprint plans, questions and proposes, all by the mock planner", async () => {
    await mount(`/m/${LOCAL}/new?kind=sprint&project=${CEDAR}`);
    await type($<HTMLTextAreaElement>(".composer textarea"), "Add usage alerts.");
    await click(named(".create-foot .btn", "Start planning"));
    await mockStep(); await mockStep();
    expect($(".sp-conv .dc-q").textContent).toBe("Should this ship behind a setting first?");
    expect($(".crumbs .ws-title").textContent).toBe("Sprint: Add usage alerts");
    await click(all(".sp-conv .opt")[0]);
    await mockStep();
    expect(all(".pt-row:not(.th)").length).toBe(4);
    expect($(".sp-plan-h").textContent).toContain("Plan v1");
  });
});

describe("sprint runs", () => {
  const created = async (mode: "Manual" | "Automatic") => {
    await mount(auditPath());
    await click(named(".sp-conv .opt", "Yes, all of them"));
    await mockStep();
    await click(named(".sp-h .btn-primary", "Create 5 tasks"));
    if (mode === "Automatic") await click(named('[aria-label="Sprint run mode"] button', "Automatic"));
  };
  it("automatic: reaches Ready to merge and continues without Accept", async () => {
    await created("Automatic");
    await click(named(".sp-h .btn-primary", "Run sprint"));
    expect(all(".pt-row .dep").map(el => el.textContent).slice(0, 2)).toEqual(["Working", "Waits for 1"]);
    expect(named(".sp-h .btn", "Pause queue")).toBeTruthy();
    await mockStep();
    expect(all(".pt-row .dep")[0].textContent).toBe("Ready to merge");
    expect(document.body.textContent).not.toContain("Waiting for your review");
    expect(document.body.textContent).toContain("Nothing merges without your confirmation");
    await expect.poll(() => all(".pt-row .dep").map(el => el.textContent).slice(0, 3)).toEqual(["Ready to merge", "Working", "Ready"]);
  });
  it("automatic: a failed check pauses the queue, and the reason stays on the plan, board and summary", async () => {
    await created("Automatic");
    await click(named(".sp-h .btn-primary", "Run sprint"));
    await click(named("#m-mock .mi", "Active task fails a check"));
    expect($(".l-strip[data-tone='warn']").textContent).toContain("Queue paused: Result has failed, not run or unverified checks");
    expect(named(".sp-h .btn-primary", "Continue queue").getAttribute("aria-disabled")).toBe("true");
    expect(named(".sp-h .btn-primary", "Continue queue").title).toContain("Review Add the audit events table first");
    await click(named('[role="tab"]', "Summary"));
    expect($(".l-strip[data-tone='warn']").textContent).toContain("Queue paused");
    expect($(".gr:not(.th) .cap.muted").textContent).toBe("1 failed");
    await click(named('[role="tab"]', "Board"));
    expect($("[aria-label=\"Sprint queue\"]")).toBeTruthy();
    expect($(".l-strip[data-tone='warn']").textContent).toContain("Queue paused");
  });
  it("the summary reports the fixture sprint in the mockup's words, with no invented pass", async () => {
    const usage = loadFixtures().workspace.sprints.find(s => s.description === "Usage billing")!;
    await mount(`/m/${LOCAL}/p/${CEDAR}/s/${usage.id}/summary`);
    expect(all(".sum-grid .v").map(el => el.textContent)).toEqual(["1", "1", "1", "2", "0"]);
    const rows = all(".gr:not(.th)").map(el => el.textContent);
    expect(rows[0]).toContain("Add the usage events table");
    expect(rows[0]).toContain("Ready to merge");
    expect(rows[1]).toContain("Working");
    expect(rows[1]).toContain("No result yet");
    expect(rows.some(r => r!.includes("Blocked") && r!.includes("Waits for Meter API calls per workspace"))).toBe(true);
    expect(named(".tb-actions .btn", "Pause queue")).toBeTruthy();
  });
  it("draws the plan as layers, and says what the selected task needs and unblocks", () => {
    const items = loadFixtures().workspace.sprints.find(s => s.title === "Audit log export")!.plan!.items;
    const g = planGraph(items, "write");
    expect(g.nodes.map(n => n.x)).toEqual([12, 188, 188, 364, 540]);
    expect(g.edges.filter(e => e.on).map(e => e.key)).toEqual(["table>write", "write>docs"]);
    expect(planCaption(items, "write")).toBe("Task 2 needs 1 and unblocks 5. Automatic run order, one at a time: 1, 2, 3, 4, 5.");
  });
  it("keeps sprint facts out of a cancelled, empty summary", () => {
    const state = loadFixtures().workspace, sprint = state.sprints.find(s => s.title === "Audit log export")!;
    expect(sprintSummary(buildIndex(state, WORKSPACE_FIXTURE_NOW), sprint).rows).toEqual([]);
  });
});

describe("archive", () => {
  it("groups every machine's tasks, with a countdown and the blocked ones apart", async () => {
    const view = archiveView(index(), NO_FILTER);
    expect(view.soon.map(r => r.left)).toEqual([1, 2, 3]);
    expect(view.blocked.map(r => r.view.task.title)).toEqual(expect.arrayContaining(["Add invoice numbering spike", "Nightly index rebuild"]));
    expect(new Set(view.rows.map(r => r.view.machine.name))).toEqual(new Set(["This machine", "GPU box", "Synthetic remote"]));
    await mount(`/m/${LOCAL}/archive`);
    expect($(".crumbs .cur").textContent).toBe("Archive");
    expect($(".ws-facts").textContent).toContain("3 soon");
    expect(all('[aria-labelledby="ar-soon"] .gr:not(.th)').length).toBe(3);
    expect($('[aria-labelledby="ar-soon"] .cd').textContent).toBe("1 day left");
    expect($('[aria-labelledby="ar-block"]').textContent).toContain("Needs you");
    expect($('[aria-labelledby="ar-block"]').textContent).toContain("Overdue");
    expect(named(".tb-fixed .ib", "Inspector").title).toBe("This screen has no inspector");
  });
  it("filters by text, machine and outcome", async () => {
    await mount(`/m/${LOCAL}/archive`);
    await type($<HTMLInputElement>('[aria-label="Filter archived tasks"]'), "cache");
    expect(all(".ar-cols:not(.th)").length).toBe(1);
    await type($<HTMLInputElement>('[aria-label="Filter archived tasks"]'), "");
    await click(named("#m-ar-machine .mi", "GPU box"));
    expect(all(".ar-cols:not(.th)").map(el => el.textContent)).toEqual([expect.stringContaining("Try a smaller PDF renderer")]);
    await click(named("#m-ar-machine .mi", "All"));
    await click(named("#m-ar-outcome .mi", "Cancelled"));
    expect(all(".ar-cols:not(.th)").every(el => el.textContent!.includes("Cancelled"))).toBe(true);
  });
  it("Keep leaves unmerged work out of the Archive; delete is a separate, named choice", async () => {
    await mount(`/m/${LOCAL}/archive`);
    const total = () => $(".ws-facts").textContent;
    expect(total()).toContain("1 wait");
    await click(named("#ar-wait ~ .surface .btn", "Decide"));
    expect(named("[data-bar=dialog] .btn", "Keep the work")).toBeTruthy();
    expect($("dialog[open]").textContent).toContain("5 commits on cw/async-webhooks");
    await click(named("[data-bar=dialog] .btn", "Keep the work"));
    expect($("#ar-wait")).toBeNull();
    expect(total()).not.toContain("wait");
  });
  it("Delete the work archives it on confirmation only", async () => {
    await mount(`/m/${LOCAL}/archive`);
    const before = archiveView(index(), NO_FILTER).total;
    await click(named("#ar-wait ~ .surface .btn", "Decide"));
    await click(all('input[name="unmerged"]')[1]);
    await click(named("[data-bar=dialog] .btn-danger", "Delete 5 commits and 2 files"));
    expect($(".ws-facts").textContent).toContain(`${before + 1} tasks`);
    expect($("#ar-wait")).toBeNull();
  });
  it("Restore sends a task back to Done, and it leaves the Archive", async () => {
    await mount(`/m/${LOCAL}/archive`);
    const row = named('[aria-labelledby="ar-soon"] .gr:not(.th)', "Migrate the cache layer");
    await click(row.querySelector(".btn")!);
    expect(document.body.textContent).not.toContain("Migrate the cache layer");
  });
  it("a bulk delete previews counts, skips the blocked and leaves them in place", async () => {
    await mount(`/m/${LOCAL}/archive`);
    await click(named(".tb-actions .btn", "Delete archive"));
    const text = () => $("dialog[open]").textContent;
    expect(text()).toContain("Skipped: Add invoice numbering spike");
    expect(text()).toContain("Skipped: Nightly index rebuild");
    expect(text()).toContain("Merged commits, remote branches and pull requests stay in Git");
    expect(text()).toContain("6 tasks and their saved history, 111 MB freed");
    await click(named("[data-bar=dialog] .btn-danger", "permanently"));
    expect(document.body.textContent).not.toContain("Migrate the cache layer");
    expect($('[aria-labelledby="ar-block"]').textContent).toContain("Add invoice numbering spike");
  });
  it("selection swaps the toolbar for its own actions, and a blocked task offers a decision, not a delete", async () => {
    await mount(`/m/${LOCAL}/archive`);
    await click(all(".row-ck")[0]);
    expect($(".sel-bar").textContent).toContain("1 selected");
    expect($(".tbar")).toBeNull();
    await click(named(".sel-bar .btn", "Clear"));
    await click(named('[aria-labelledby="ar-block"] .ib', "More for Add invoice numbering spike"));
    const del = named("#m-arrow .mi", "Delete now");
    expect(del.getAttribute("aria-disabled")).toBe("true");
    await click(named("#m-arrow .mi", "Decide"));
    await click(named("[data-bar=dialog] .btn-danger", "Discard the new files"));
    expect($('[aria-labelledby="ar-block"]').textContent).not.toContain("Add invoice numbering spike");
  });
  it("opens an archived task read-only, with Restore on it", async () => {
    await mount(`/m/${LOCAL}/t/${WORKSPACE_CASES.archive2}`);
    expect($(":is(.strip.info, .l-strip[data-tone=info])").textContent).toContain("Archived, read-only. 2 days left before cleanup.");
    await click(named(":is(.strip, .l-strip) .btn", "Restore"));
    expect($(":is(.strip.info, .l-strip[data-tone=info])")?.textContent ?? "").not.toContain("Archived");
  });
  it("the task menu's Archive asks first when work is unmerged", async () => {
    await mount(`/m/${LOCAL}/t/${WORKSPACE_CASES.cancelled}`);
    await click(named(".tb-actions .ib", "Task actions"));
    await click(named("#m-task .mi", "Archive"));
    expect($("dialog[open]").textContent).toContain("has unmerged work");
    await click(named("[data-bar=dialog] .btn", "Keep the work"));
    expect($("dialog[open]")).toBeNull();
  });
});

describe("machines and Inbox", () => {
  it("Inbox covers every connected machine and sends each decision where it is made", async () => {
    await mount(`/m/${LOCAL}/inbox`);
    expect($(".page-h p").textContent).toContain("all 2 connected machines");
    expect(document.body.textContent).toContain("GPU box / Willow Archive / Batch the embedding job");
    await click(named(".need", "Unmerged work"));
    expect(location.pathname).toBe(`/m/${LOCAL}/archive`);
    history.back();
    await expect.poll(() => location.pathname).toBe(`/m/${LOCAL}/inbox`);
    await tick();
    await click(named(".need", "Should the export include"));
    expect(location.pathname).toMatch(/\/planning$/);
  });
  it("adds a remote machine only after both fingerprints match, and a bad code connects nothing", async () => {
    await mount(`/m/${LOCAL}`);
    await click(named("#m-machine .mi", "Add remote machine"));
    await click(named("[data-bar=dialog] .btn-primary", "I have a code"));
    await type($<HTMLInputElement>('[aria-label="Pairing code or link"]'), "nope");
    await type($<HTMLInputElement>('[aria-label="Name for this machine"]'), "Lab box");
    await click(named("[data-bar=dialog] .btn-primary", "Connect"));
    expect($(".dlg-b .err").textContent).toContain("Enter the code");
    await click(named("#m-mock .mi", "code expired"));
    await type($<HTMLInputElement>('[aria-label="Pairing code or link"]'), "7kq2-m9xd");
    await click(named("[data-bar=dialog] .btn-primary", "Connect"));
    expect($(".dlg-b .err").textContent).toContain("This code expired");
    await click(named("[data-bar=dialog] .btn-primary", "Connect"));
    expect($(".fpr").textContent).toContain("4F2A 91C7 D03E");
    await click(named("[data-bar=dialog] .btn-primary", "Fingerprints match"));
    expect(named("#m-machine .mi", "Lab box")).toBeTruthy();
  });
  it("stops pairing when the fingerprints differ", async () => {
    await mount(`/settings/app/machines`);
    await click(named("#m-mock .mi", "fingerprints differ"));
    await click(named(".set-h .btn", "Add remote machine"));
    await click(named("[data-bar=dialog] .btn-primary", "I have a code"));
    await type($<HTMLInputElement>('[aria-label="Pairing code or link"]'), "7KQ2-M9XD");
    await type($<HTMLInputElement>('[aria-label="Name for this machine"]'), "Lab box");
    await click(named("[data-bar=dialog] .btn-primary", "Connect"));
    expect($(".note-box.bad").textContent).toContain("no project data has been sent");
    expect(all("[data-bar=dialog] .btn-primary").length).toBe(0);
    await click(named("[data-bar=dialog] .btn", "Cancel"));
    expect(all(".mcard").length).toBe(3);
  });
  it("a reconnect lets a machine act again, and every card reads as the machine it is", async () => {
    await mount(`/settings/app/machines`);
    expect(all(".mcard").map(el => el.querySelector(".tt")!.textContent)).toEqual(["This machine", "Synthetic remote", "GPU box"]);
    expect(all(".mcard")[1].textContent).toContain("Offline, last seen");
    await click(named(".mcard .btn", "Retry now"));
    expect(all(".mcard")[1].textContent).toContain("Connected");
  });
  it("adds a project to the machine in view and opens it", async () => {
    await mount(`/m/${GPU}`);
    await click(named(".sb-foot .btn", "Add project"));
    await click(all('dialog[open] [role="option"]').find(o => o.textContent?.startsWith("Search for folder"))!);
    await type($<HTMLInputElement>('[aria-label="Search for folder"]'), "~/projects/render-farm");
    await click(all('dialog[open] [role="option"]').find(o => o.textContent?.startsWith("~/projects/render-farm"))!);
    expect($(".crumbs .cur").textContent).toBe("render-farm");
    expect(location.pathname).toContain(`/m/${GPU}/p/`);
  });
});

describe("machine settings", () => {
  const section = (path: string) => mount(`/settings/machine/${LOCAL}/${path}`);
  it("keeps a draft per file, saves it to the machine and new tasks follow", async () => {
    await section("orchestration/defaults");
    await click(named('[aria-label="Effort"] button', "Low"));
    await click(named('[aria-label="Default checkout"] button', "Main checkout"));
    expect($(".savebar").textContent).toContain("2 unsaved changes in coordinator and limits");
    await click(named(".savebar .btn-primary", "Save coordinator and limits on This machine"));
    expect($(".savebar").textContent).toContain("Saved on This machine");
    await click(named(".nav", "Back to workspace"));
    await click(named(".sb-top .nav", "New task"));
    expect(named(".c-bar .sel", "Reasoning effort").textContent).toContain("Low");
    expect(all<HTMLInputElement>('input[name="checkout"]')[1].checked).toBe(true);
  });
  it("leaving with unsaved changes asks Stay, Discard or Save here", async () => {
    await section("orchestration/defaults");
    await click(named('[aria-label="Effort"] button', "Low"));
    await click(named(".snav", "Engine connections"));
    expect(location.pathname).toContain("orchestration/defaults");
    await click(named("[data-bar=dialog] .btn", "Stay"));
    expect($(".savebar")).toBeTruthy();
    await click(named(".snav", "Engine connections"));
    await click(named("[data-bar=dialog] .btn", "Discard"));
    expect(location.pathname).toContain("/connections");
    await click(named(".snav.sub", "Coordinator"));
    expect($(".savebar")).toBeNull();
  });
  it("turning a model off also takes it out of automatic routing, and limits show Codex and Claude only", async () => {
    await section("orchestration/models");
    await click($('[aria-label="Opus enabled"]'));
    expect($<HTMLInputElement>('[aria-label="Opus in automatic routing"]').checked).toBe(false);
    await click(named(".savebar .btn", "Discard"));
    expect(document.body.textContent).not.toContain("Usage not tracked");
    await click(named(".snav", "Engine connections"));
    expect(all(".urow:not(.th) .w500").map(el => el.textContent)).toEqual(["Codex"]);
    expect(document.body.textContent).toContain("42% used");
    expect(document.body.textContent).toContain("Unavailable");
  });
  it("an offline machine's settings are read-only, with nothing queued", async () => {
    await mount(`/settings/machine/${REMOTE}/orchestration/defaults`);
    expect($(":is(.strip.info, .l-strip[data-tone=info])").textContent).toContain("Nothing is queued");
    expect(named('[aria-label="Effort"] button', "Low").getAttribute("aria-disabled")).toBe("true");
    await click(named('[aria-label="Effort"] button', "Low"));
    expect($(".savebar")).toBeNull();
  });
  it("storage saves retention per machine, and cleanup reports what it did", async () => {
    await section("storage");
    await click($('[aria-label="More: Retention"]'));
    await click(named(".savebar .btn-primary", "Save Storage and archive on This machine"));
    expect($('[aria-label="Retention"]').textContent).toContain("31 days");
    await click(named(".srow .btn", "Run cleanup now"));
    expect(named(".srow", "Last cleanup").textContent).toContain("skipped and waiting for you");
    await click(named(".nav", "Back to workspace"));
    await click(named(".sb-top .nav", "Archive"));
    expect($(".page-h p").textContent).toContain("Kept for 31 days");
  });
  it("Clear old tasks shows a preview first and never reaches unmerged work", async () => {
    await section("storage");
    await click(named(".srow .btn", "Clear old tasks"));
    expect($("dialog[open]").textContent).toContain("Will be removed: 0 tasks");
    expect($("dialog[open]").textContent).toContain("Kept: 3 with unmerged work");
    expect(named("[data-bar=dialog] .btn-danger", "Delete 0 tasks").getAttribute("aria-disabled")).toBe("true");
    for (let i = 0; i < 6; i++) await click($('[aria-label="Fewer: Older than"]'));
    expect($("dialog[open]").textContent).toContain("Will be removed: 2 tasks");
    await click(named("[data-bar=dialog] .btn-danger", "Delete 2 tasks"));
    expect($("dialog[open]")).toBeNull();
  });
  it("sets where worktrees live, with a default and a refusal for a relative path", async () => {
    await section("worktrees");
    const field = () => $<HTMLInputElement>('[aria-label="Worktree folder"]');
    expect(field().value).toBe("~/.cortex-workspace/worktrees");
    await type(field(), "relative/path");
    await click(named(".savebar .btn-primary", "Save Worktrees"));
    expect($(".savebar").textContent).toContain("unsaved");
    await type(field(), "~/work/trees");
    await click(named(".savebar .btn-primary", "Save Worktrees"));
    expect($(".savebar").textContent).toContain("Saved on This machine");
    expect(named(".srow", "Example path").textContent).toContain("~/work/trees/");
    await click(named(".srow .btn", "Use default"));
    expect(field().value).toBe("~/.cortex-workspace/worktrees");
  });
  it("covers voice, shortcuts, diagnostics and remote access as controls", async () => {
    await mount("/settings/app/voice");
    expect(named(".srow", "Provider").textContent).toContain("On-device");
    await click(named(".snav", "Shortcuts"));
    expect(document.body.textContent).toContain("Close, Cancel, Delete and Merge have no shortcut on purpose.");
    await click(named(".snav", "Diagnostics"));
    await click(named(".srow .btn", "Run"));
    expect(named(".srow", "Run diagnostics").textContent).toContain("ok, 14 checks");
    await click(named(".snav", "Remote access"));
    expect($(".pair").textContent).toBe("7KQ2-M9XD");
    await click(named(".srow .btn", "Revoke"));
    await click(named("[data-bar=dialog] .btn-danger", "Revoke"));
    expect(document.body.textContent).toContain("No device is paired");
    await click($('[aria-label="Allow remote access"]'));
    expect($(".pair")).toBeNull();
  });
  it("switches the machine these settings belong to", async () => {
    await section("storage");
    await click(named("#m-set-machine .mi", "GPU box"));
    expect(location.pathname).toBe(`/settings/machine/${GPU}/storage`);
    expect($(".ws-set-machine").textContent).toContain("GPU box");
  });
});

describe("commands as controls", () => {
  it("harvest and skills become ordinary tasks from the project", async () => {
    await mount(`/m/${LOCAL}/p/${CEDAR}/knowledge`);
    await click(named(".srow .btn", "Collect knowledge"));
    await type($<HTMLInputElement>('[aria-label="Scope, optional"]'), "the export code");
    await click(named("[data-bar=dialog] .btn-primary", "Create the task"));
    expect(location.pathname).toContain("/t/");
    expect($(".brief").textContent).toContain("harvest: the export code");
  });
  it("project settings lead to its override and its worktree folder", async () => {
    await mount(`/m/${LOCAL}/p/${CEDAR}/settings`);
    expect(document.body.textContent).toContain("~/.cortex-workspace/worktrees");
    await click(named(".srow .btn", "Open overrides"));
    expect(location.pathname).toContain("/orchestration/routing");
    expect(named('[aria-label="Scope"] button', "Project").getAttribute("aria-current")).toBe("true");
  });
});
