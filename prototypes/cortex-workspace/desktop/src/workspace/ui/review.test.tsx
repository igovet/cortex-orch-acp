import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { WORKSPACE_CASES as C } from "../adapter/fixtures";
import { highlight } from "@cortex/workspace-ui/highlight";
import Workspace from "./index";
import { splitRows } from "@cortex/workspace-ui/review-model";
import { hrefOf, parseRoute, type Route } from "@cortex/workspace-ui/router";

const LOCAL = "mock-machine-00000", CEDAR = "mock-project-00001";
const TASK = (id: string, tail = "") => `/m/${LOCAL}/t/${id}${tail}`;
const PROJECT = (tail: string) => `/m/${LOCAL}/p/${CEDAR}/${tail}`;

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
}
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const named = (selector: string, name: string) => all(selector).find(el => (el.getAttribute("aria-label") ?? el.textContent ?? "").includes(name))!;
const click = async (el: Element) => { (el as HTMLElement).click(); await tick(); };
const type = async (el: HTMLTextAreaElement | HTMLInputElement, text: string) => { el.value = text; el.dispatchEvent(new InputEvent("input", { bubbles: true })); await tick(); };
const dialog = () => $("dialog[open]");
const inDialog = (name: string) => named("dialog[open] button, dialog[open] a", name);
const check = async (selector: string) => { const box = $<HTMLInputElement>(selector); box.checked = !box.checked; box.dispatchEvent(new Event("change", { bubbles: true })); await tick(); };
const mock = async (name: string) => { await click($(".mock-chip")); await click(named("#m-mock .mi", name)); };
const chip = () => $(".tb-actions .chip").textContent!;

describe("routes", () => {
  it("round-trips a conflict, a place in the code and a chosen checkout", () => {
    const task = { name: "task", machine: LOCAL, task: "t" } as const;
    const project = { name: "project", machine: LOCAL, project: "p", scope: { kind: "all" } } as const;
    const routes: Route[] = [{ ...task, tab: "changes", sub: "conflict" }, { ...task, tab: "changes", sub: "diff" }, { ...task, tab: "code", file: "a/b.py", line: 12 },
      { ...project, view: "git", checkout: "c" }, { ...project, view: "code", checkout: "c", file: "a b.py", line: 3 }, { ...project, view: "worktrees" }];
    for (const route of routes) expect(parseRoute(...splitHref(hrefOf(route)))).toEqual(route);
    expect(parseRoute(TASK("t", "/changes/nope")).name).toBe("notfound");
  });
});
function splitHref(href: string): [string, string] { const at = href.indexOf("?"); return at < 0 ? [href, ""] : [href.slice(0, at), href.slice(at)]; }

describe("result review", () => {
  it("shows every outcome honestly: only a pass is green", async () => {
    await mount(TASK(C.review));
    const card = $(".rcard");
    expect(card.textContent).toContain("Result v2");
    // The counts are one line; only reported outcomes are named.
    const cells = all(".rcard .rv-outc > .st");
    expect(cells.map(c => c.textContent)).toEqual(["5 passed", "1 failed", "2 not run", "1 unverified"]);
    expect(all(".rcard .rv-outc .st-ok")).toHaveLength(1);
    expect(cells[0].classList.contains("st-ok")).toBe(true);
    expect(cells[2].classList.contains("st-ok")).toBe(false);
    expect(cells[3].classList.contains("st-ok")).toBe(false);
    const open = all('.rcard section[aria-label="Not passed"] .l-row');
    expect(open.map(n => n.textContent)).toEqual(expect.arrayContaining([expect.stringContaining("Not run: test database was busy"), expect.stringContaining("Unverified: worker says it passed, no command receipt")]));
    expect(open[0].textContent).toContain("p95 was 410 ms");
    expect($(".phases").textContent).toContain("Result v2");
    expect(named('[role="tab"]', "Workers").textContent).toContain("4 finished, 1 cancelled");
  });

  it("requires reruns rather than accepting ordinary failures", async () => {
    await mount(TASK(C.review));
    const accept = named(".rcard .btn", "Accept v2");
    expect(accept.getAttribute("aria-disabled")).toBe("true");
    expect(accept.title).toContain("Run the unpassed checks first");
    await click(named(".rcard button", "Rerun pytest tests/load/test_export_burst.py"));
    expect($(".rcard").textContent).toContain("Running check");
    await mock("Simulate the next step");
    expect(all(".outc > div").map(c => c.textContent)).toEqual(["6Passed", "0Failed", "2Not run", "1Unverified"]);
    expect(document.body.textContent).toContain("Simulated rerun passed; no command ran");
  });
  it("never accepts open checks by accident, and acceptance is a separate step from merging", async () => {
    await mount(TASK(C.review));
    // Ordinary failures and unsupported worker claims must be checked first.
    for (const name of ["pytest tests/load/test_export_burst.py", "Redis failover keeps the counters"]) {
      await click(named(".rcard button", `Rerun ${name}`));
      await mock("Simulate the next step");
    }
    expect(all(".outc > div").map(c => c.textContent)).toEqual(["7Passed", "0Failed", "2Not run", "0Unverified"]);
    await click(named(".rcard .btn", "Accept v2"));
    expect(dialog().textContent).toContain("Technical blocker");
    expect(inDialog("Accept v2").getAttribute("aria-disabled")).toBe("true");
    await click(inDialog("Accept v2"));
    expect(chip()).toContain("Review");
    await check('dialog[open] input[type="checkbox"]');
    await click(inDialog("Accept v2"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect(chip()).toContain("Ready to merge");
    expect(location.pathname).toBe(TASK(C.review));
    expect(titleBtn("Merge…").getAttribute("aria-disabled")).toBeNull();
    expect($(".rcard").textContent).toContain("accepted by you");
  });

  it("keeps a stale result from being accepted, with the reason", async () => {
    await mount(TASK(C.stale));
    const accept = named(".rcard .btn", "Accept");
    expect(accept.getAttribute("aria-disabled")).toBe("true");
    expect(accept.title).toContain("no longer matches");
    expect($(".rcard").textContent).toContain("Stale result");
  });

  it("sends a result back from the composer, which reopens the task", async () => {
    await mount(TASK(C.review));
    const box = $<HTMLTextAreaElement>(".dock textarea");
    expect(box.disabled).toBe(false);
    await type(box, "Batch the Redis calls");
    await click(named(".dock button", "Send"));
    expect(all(".mine").at(-1)!.textContent).toBe("Batch the Redis calls");
    expect(chip()).toContain("Working");
  });

  it("Request changes asks what should change first", async () => {
    await mount(TASK(C.review));
    await click(named(".rcard .btn", "Request changes"));
    expect(inDialog("Send and reopen").getAttribute("aria-disabled")).toBe("true");
    await type($("dialog[open] textarea"), "Raise the budget");
    await click(inDialog("Send and reopen"));
    expect(chip()).toContain("Working");
  });
});
const titleBtn = (name: string) => named(".tb-actions .btn", name);

describe("changes", () => {
  const CHANGES = TASK(C.review, "/changes/diff");
  it("lists the pinned version, its evidence and every file", async () => {
    await mount(CHANGES);
    expect($(".chg-tools").textContent).toContain("Result v2");
    expect($(".chg-tools").textContent).toContain("6 files");
    expect($(".chg-tools").textContent).toContain("+120");
    expect($(".chg-tools").textContent).toContain("sha 7c1e0b2");
    expect($(".evid summary").textContent).toContain("1 failed");
    expect(all(".evid-b .chk")[0].textContent).toContain("Ask to fix");
    expect(all(".fitem")).toHaveLength(6);
    expect(named(".dl.add", "limiter.check").querySelector(".tok-k")).toBeTruthy();
  });

  it("keeps Accept, Commit and Merge apart, and Merge waits for acceptance", async () => {
    await mount(CHANGES);
    const groups = all(".chg-foot .grp").map(g => g.textContent);
    expect(groups).toEqual(["Accept v2", "Commit…", "Merge…"]);
    const merge = named(":is(.chg-foot, .rv-foot) .btn", "Merge…");
    expect(merge.getAttribute("aria-disabled")).toBe("true");
    expect(merge.title).toContain("Accepting is your decision, merging is a Git action");
  });

  it("switches scope and layout, and an empty scope says so", async () => {
    await mount(CHANGES);
    await click(named("#m-diff-scope .mi", "Last turn"));
    expect(all(".fitem")).toHaveLength(1);
    await click(named("#m-diff-scope .mi", "Uncommitted"));
    expect($(":is(.note-panel, .rv-note)").textContent).toContain("No changes in this view");
    await click(named("#m-diff-scope .mi", "Result v2"));
    await click(named('.seg[aria-label="Diff layout"] button', "Split"));
    expect($(".sbs")).toBeTruthy();
    const first = all(".sbs .side")[0], second = all(".sbs .side")[1];
    expect(first.querySelectorAll(".sl").length).toBe(second.querySelectorAll(".sl").length);
  });

  it("gathers line comments into one message that reopens the task", async () => {
    await mount(CHANGES);
    expect($(":is(.chg-foot, .rv-foot) .cap").textContent).toBe("1 comment");
    await click(named(".ws-plus", "Comment on line 21"));
    await type($(".cmt textarea"), "Say why it is blocked");
    await click(named(".cmt .btn", "Save comment"));
    expect($(":is(.chg-foot, .rv-foot) .cap").textContent).toBe("2 comments");
    await click(named(":is(.chg-foot, .rv-foot) .btn", "Send"));
    expect(chip()).toContain("Working");
    expect(titleBtn("Stop")).toBeTruthy();
  });

  it("opens a file in Code at its first changed line", async () => {
    await mount(CHANGES);
    await click(named(".fhead .btn", "Open in editor"));
    expect(location.pathname).toBe(TASK(C.review, "/code"));
    expect(location.search).toContain("line=20");
    expect($(".cl.hit .n").textContent).toBe("20");
    expect($(".cl.hit").textContent).toContain("limiter.check");
  });

  it("offers Stage and Revert only for uncommitted changes", async () => {
    await mount(TASK(C.session, "/changes/diff"));
    await click(named("#m-diff-scope .mi", "Uncommitted"));
    expect(all(".fhead")[1].textContent).toContain("auth/refresh.py");
    expect(named(".fhead .btn", "Stage").getAttribute("aria-disabled")).toBeNull();
    await click(all(".fhead")[1].querySelector(".btn")!);
    expect(all(".fhead")[1].textContent).toContain("Unstage");
    dispose?.(); dispose = undefined; document.body.innerHTML = "";
    await mount(CHANGES);
    // Files committed in the result say so instead of offering Stage or Revert.
    const heads = all(".fhead");
    expect(heads.length).toBeGreaterThan(0);
    for (const head of heads) {
      expect(head.textContent).toContain("Committed in this result");
      expect([...head.querySelectorAll(".btn")].map(b => b.textContent)).toEqual(["Open in editor"]);
    }
  });
});

describe("ready to merge", () => {
  const READY = TASK(C.merge, "/changes");
  it("shows the card and the honest preconditions", async () => {
    await mount(READY);
    expect($(".rcard-h").textContent).toContain("Accepted v1 by you");
    expect($(".rcard-b").textContent).toContain("Chart usage on the billing page");
    expect($(".flow").textContent).toContain("cw/usage-events");
    expect($(".flow").textContent).toContain("3 behind");
    expect($(".ready-in").textContent).toContain("Every task stops here");
    await click(named(":is(.rcard-f, .rv-card-f) .btn", "Merge…"));
    const pre = all(".pre .chk").map(c => c.textContent);
    expect(pre[0]).toContain("Result v1 accepted by you, with open checks");
    expect(pre[1]).toContain("Behind main by 3 commits");
    expect(pre[2]).toContain("Worktree is clean");
    expect(pre[3]).toContain("Checks: 6 passed, 1 not run");
    expect(all(".pre .chk")[3].querySelector(".st-ok")).toBeNull();
  });

  it("merges nothing until the owner confirms", async () => {
    await mount(READY);
    await click(named(":is(.rcard-f, .rv-card-f) .btn", "Merge…"));
    await click(inDialog("Cancel"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect(chip()).toContain("Ready to merge");
    expect($(".rcard-h").textContent).toContain("Ready to merge");
  });

  it("defaults to the Git default branch, and a changed target is what gets merged", async () => {
    await mount(READY);
    await click(named(":is(.rcard-f, .rv-card-f) .btn", "Merge…"));
    const label = () => named("dialog[open] .lab", "Merge into").textContent;
    expect(label()).toContain("Git default branch");
    expect(inDialog("Confirm merge into main")).toBeTruthy();
    expect(all("#m-merge-target .mi").map(m => m.textContent)).toEqual(expect.arrayContaining([expect.stringContaining("release/2.4"), expect.stringContaining("Branch of this sprint"), expect.stringContaining("Other branch…")]));
    await click(named("#m-merge-target .mi", "release/2.4"));
    expect(inDialog("Confirm merge into release/2.4")).toBeTruthy();
    expect(label()).not.toContain("Git default branch");
    expect(all(".pre .chk")[1].textContent).toContain("Behind release/2.4 by 12 commits");
    await click(inDialog("Confirm merge into release/2.4"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect($(".rcard-h").textContent).toContain("Merged into release/2.4");
    expect(chip()).toContain("Merged");
  });

  it("takes an other branch by name, and says when it was never compared", async () => {
    await mount(READY);
    await click(named(":is(.rcard-f, .rv-card-f) .btn", "Merge…"));
    await click(named("#m-merge-target .mi", "Other branch…"));
    const field = $<HTMLInputElement>('input[aria-label="Other target branch"]');
    await type(field, "hotfix/usage");
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })); await tick();
    expect(inDialog("Confirm merge into hotfix/usage")).toBeTruthy();
    expect(all(".pre .chk")[1].textContent).toContain("Not compared with hotfix/usage yet");
  });

  it("creating a pull request does not merge, and the task stays in Ready to merge", async () => {
    await mount(READY);
    await click(named(":is(.rcard-f, .rv-card-f) .btn", "Create PR…"));
    expect(inDialog("Push and create PR")).toBeTruthy();
    expect(dialog().textContent).toContain("Pushes cw/usage-events");
    await click(inDialog("Push and create PR"));
    expect(chip()).toContain("Ready to merge");
    expect($(".rcard").textContent).toContain("It is not merged");
    expect(named(":is(.rcard-f, .rv-card-f) .btn", "Create PR…").getAttribute("aria-disabled")).toBe("true");
  });

  it("a merge with the delete box on removes the worktree, and only when it is clean", async () => {
    await mount(READY);
    await click(named(":is(.rcard-f, .rv-card-f) .btn", "Merge…"));
    expect(dialog().textContent).toContain("Never removes unmerged or uncommitted work");
    await click(inDialog("Confirm merge into main"));
    await click($(".crumbs a"));
    await click(named(".ptab", "Worktrees"));
    expect(all(".gr").some(r => r.textContent!.includes("cw/usage-events"))).toBe(false);
  });
});

describe("conflict", () => {
  const CONFLICT = TASK(C.rollups, "/changes/conflict");
  const steps = () => all(".cx-step").map(s => s.textContent);
  it("shows both ways out, the three sides and an Abort that is always there", async () => {
    await mount(CONFLICT);
    expect(chip()).toContain("Conflict");
    expect($(":is(.strip.bad, .l-strip[data-tone=fail])").textContent).toContain("Merging main into cw/usage-rollups stopped at 2 conflicts. Nothing was stashed or reset.");
    expect(all(".cf-way .t").map(t => t.textContent)).toEqual(["Resolve it yourself", "Resolve automatically"]);
    expect(all(".cf-col b").map(b => b.textContent)).toEqual(["Base", "Ours", "Theirs", "Result"]);
    expect(all(".flist .fitem").map(f => f.textContent)).toEqual(["Cusage_rollups.py1 left", "Cschema.sqlResolved", "Mrollup_jobs.py", "Musage_api.py", "Atest_rollups.py", "Msettings.py"]);
    expect(named(":is(.strip, .l-strip) .btn", "Abort merge")).toBeTruthy();
  });

  it("Resolve yourself: Finish merge waits for the last block, then the result goes through Ready to merge", async () => {
    await mount(CONFLICT);
    const finish = () => named(":is(.chg-foot, .rv-foot) .btn", "Finish merge");
    expect(finish().getAttribute("aria-disabled")).toBe("true");
    await click(finish());
    expect(chip()).toContain("Conflict");
    await click(named(".cf-block .btn", "Accept both"));
    expect($(".cf-result").textContent).toContain("load(day, tz, n=500)");
    expect(finish().getAttribute("aria-disabled")).toBeNull();
    await click(finish());
    expect($(":is(.strip.ok, .l-strip[data-tone=ok])").textContent).toContain("You finished the merge");
    expect(chip()).toContain("Ready to merge");
    await click(named(":is(.chg-foot, .rv-foot) .btn", "Review in Ready to merge"));
    expect(location.pathname).toBe(TASK(C.rollups, "/changes"));
    expect($(".rcard-h").textContent).toContain("resolved by you");
    expect(named(":is(.rcard-f, .rv-card-f) .btn", "Merge…").getAttribute("aria-disabled")).toBe("true");
  });

  it("Resolve automatically: says what it will do, runs, and comes back unmerged for review", async () => {
    await mount(CONFLICT);
    await click(named(".cf-way .btn", "Resolve automatically"));
    expect(dialog().textContent).toContain("Your choice in schema.sql stays as you made it");
    expect(dialog().textContent).toContain("Nothing is merged or pushed on its own");
    expect(dialog().textContent).toContain("Uses Opus, High");
    await type($("dialog[open] textarea"), "Keep the tz argument");
    await click(inDialog("Resolve automatically"));
    expect(chip()).toContain("Resolving");
    expect(steps()[0]).toContain("Read both sides");
    expect($(".cf-block").textContent).toContain("Your choices are paused");
    expect(named(":is(.strip, .l-strip) .btn", "Abort merge")).toBeTruthy();
    expect(document.querySelectorAll(".cf-block .btn")).toHaveLength(0);
    await mock("Simulate the next step");
    expect($(".cx-step.now").textContent).toContain("Resolving usage_rollups.py");
    await mock("Simulate the next step");
    expect($(".cx-step.now").textContent).toContain("Run the checks");
    await mock("Simulate the next step");
    expect($(":is(.strip.ok, .l-strip[data-tone=ok])").textContent).toContain("Nothing is merged until you confirm it in Ready to merge");
    expect(chip()).toContain("Ready to merge");
    expect($(".cf-block.ok").textContent).toContain("kept tz from main and n=500 from this branch");
    expect(steps().join(" ")).toContain("Checks: 5 passed, 1 not passed");
    await click(named(":is(.chg-foot, .rv-foot) .btn", "Review in Ready to merge"));
    expect($(".rcard-h").textContent).toContain("resolved automatically");
    expect($(".rcard-b").textContent).toContain("This is result v2");
    const merge = named(":is(.rcard-f, .rv-card-f) .btn", "Merge…");
    expect(merge.getAttribute("aria-disabled")).toBe("true");
    expect(merge.title).toBe("Review result v2 first");
    await click(merge);
    expect(document.querySelector("dialog[open]")).toBeNull();
    await click(named(":is(.rcard-f, .rv-card-f) .btn", "Accept v2"));
    await check('dialog[open] input[type="checkbox"]');
    await click(inDialog("Accept v2"));
    expect($(".rcard-h").textContent).toContain("Accepted v2 by you");
    await click(named(":is(.rcard-f, .rv-card-f) .btn", "Merge…"));
    await click(inDialog("Confirm merge into main"));
    expect($(".rcard-h").textContent).toContain("Merged into main");
  });

  it("a failed Cortex run changes nothing and hands the block back", async () => {
    await mount(CONFLICT);
    await click(named(".cf-way .btn", "Resolve automatically"));
    await click(inDialog("Resolve automatically"));
    await mock("Next automatic resolution fails");
    for (let i = 0; i < 3; i++) await mock("Simulate the next step");
    expect($(":is(.strip.warn, .l-strip[data-tone=warn])").textContent).toContain("usage_rollups.py could not be resolved and nothing changed");
    expect(chip()).toContain("Conflict");
    expect(all(".cf-way")).toHaveLength(2);
    expect(named(".cf-block .btn", "Accept ours")).toBeTruthy();
    expect(all(".flist .fitem")[0].textContent).toContain("1 left");
  });

  it("Stop and resolve myself takes the conflict back; Abort restores the branch", async () => {
    await mount(CONFLICT);
    await click(named(".cf-way .btn", "Resolve automatically"));
    await click(inDialog("Resolve automatically"));
    await click(named(".cx-run .btn", "Stop and resolve myself"));
    expect(chip()).toContain("Conflict");
    expect(all(".cf-way")).toHaveLength(2);
    await click(named(":is(.strip, .l-strip) .btn", "Abort merge"));
    expect($(".note-panel").textContent).toContain("No merge in progress");
    expect(chip()).toContain("Review");
  });

  it("the Ready to merge card offers the same two ways out", async () => {
    await mount(TASK(C.rollups, "/changes"));
    expect($(".rcard-h").textContent).toContain("Conflict with main");
    expect($(".rcard-b").textContent).toContain("Nothing was stashed or reset");
    expect(all(".ways2 .t").map(t => t.textContent)).toEqual(["Resolve it yourself", "Resolve automatically"]);
    await click(named(".ways2 .btn", "Resolve automatically"));
    await click(inDialog("Resolve automatically"));
    expect($(".rcard-h").textContent).toContain("Resolving the conflict");
    expect($(".cx-steps").textContent).toContain("Read both sides");
    expect($(".rcard-b").textContent).toContain("Nothing reaches main until you confirm it");
  });

  it("updating a branch that conflicts lands on the conflict, not a silent merge", async () => {
    await mount(TASK(C.merge, "/changes"));
    await click(named(":is(.rcard-f, .rv-card-f) .btn", "Merge…"));
    await click(inDialog("Update from main"));
    expect($(".rcard-h").textContent).toContain("Conflict with main");
    expect(chip()).toContain("Conflict");
  });

  it("a board card sends a conflict straight to its screen", async () => {
    await mount(PROJECT("board"));
    const card = $(`[data-task="${C.rollups}"]`);
    expect(card.classList.contains("bad")).toBe(true);
    await click(named(`[data-task="${C.rollups}"] .btn`, "Resolve"));
    expect(location.pathname).toBe(TASK(C.rollups, "/changes/conflict"));
  });

  it("keeps the fixture's older conflict workable, with its own file", async () => {
    await mount(TASK(C.conflict, "/changes/conflict"));
    expect($(".fhead").textContent).toContain("synthetic/layout.ts");
    await click(named(".cf-block .btn", "Accept ours"));
    expect(named(":is(.chg-foot, .rv-foot) .btn", "Finish merge").getAttribute("aria-disabled")).toBeNull();
  });
});

describe("code", () => {
  const WORKTREE = "mock-checkout-00001";
  const CODE = PROJECT(`code?checkout=${WORKTREE}`);
  it("opens on the first changed file with its tree, tab and highlighting", async () => {
    await mount(CODE);
    expect(location.search).toContain("file=auth%2Fmodels.py");
    expect(named(".tn", "models.py").getAttribute("aria-current")).toBe("true");
    expect(named(".tn", "models.py").querySelector(".gs")!.textContent).toBe("M");
    expect(named(".tn", "test_race.py").querySelector(".gs")!.textContent).toBe("U");
    expect($(".etab").textContent).toContain("models.py");
    expect($(".side-opts").textContent).toContain("3 changed");
    await click(named(".tn", "refresh.py"));
    expect(all(".cl")).toHaveLength(44);
    expect(named(".cl", "with_for_update").classList.contains("chg")).toBe(true);
    expect(named(".cl", "def rotate_refresh_token").querySelector(".tok-k")!.textContent).toBe("def");
    expect(all(".etab")).toHaveLength(2);
  });

  it("warns before changes in a checkout Cortex is working in", async () => {
    await mount(CODE);
    expect($(":is(.strip.warn, .l-strip[data-tone=warn])").textContent).toContain("A task is working in this checkout");
    await click(named(":is(.strip, .l-strip) .btn", "Edit anyway"));
    expect($(":is(.strip.warn, .l-strip[data-tone=warn])").textContent).toContain("Verification may be blocked");
  });

  it("shows a preview tab until the file is kept, and closing moves to the next tab", async () => {
    await mount(CODE);
    await click(named(".tn", "errors.py"));
    expect($(".etab em").textContent).toBe("errors.py");
    await click(named(".tn", "sessions.py"));
    expect(all(".etab em").map(e => e.textContent)).toEqual(["sessions.py"]);
    await click(named(".etab .x", "Close sessions.py"));
    expect(all(".etab").map(t => t.textContent)).toEqual(["models.py"]);
    expect(location.search).toContain("file=auth%2Fmodels.py");
  });

  it("switches between file, inline and side by side, and stages a hunk", async () => {
    await mount(`${CODE}&file=auth%2Frefresh.py`);
    await click(named('.seg[aria-label="View"] button', "Inline diff"));
    expect(named(".dl.del", "session.get")).toBeTruthy();
    expect($(".ecrumb").textContent).toContain("+18");
    expect($(".ed").textContent).toContain("The fixture keeps only part of this diff");
    await click(named(".hact .btn", "Stage hunk"));
    expect(named(".hact .btn", "Unstage hunk")).toBeTruthy();
    await click(named('.seg[aria-label="View"] button', "Side by side"));
    expect($(".sbs")).toBeTruthy();
    await click(named('.seg[aria-label="View"] button', "File"));
    await click(named(".tn", "errors.py"));
    await click(named('.seg[aria-label="View"] button', "Inline diff"));
    expect($(".ed").textContent).toContain("No changes in this file");
  });

  it("searches every file and opens a hit at its line", async () => {
    await mount(CODE);
    await click(named('.seg[aria-label="Left pane"] button', "Search"));
    await type($('input[aria-label="Search files"]'), "with_for_update");
    expect($(".ws-search").textContent).toContain("4 matches in 3 files");
    expect(all(".res-file").map(f => f.textContent)).toEqual(["auth/refresh.py1", "auth/sessions.py2", "billing/ledger.py1"]);
    expect($(".res-hit mark").textContent).toBe("with_for_update");
    await click($(".res-hit"));
    expect(location.search).toContain("line=16");
    expect($(".cl.hit .n").textContent).toBe("16");
    await type($('input[aria-label="Search files"]'), "WITH_FOR_UPDATE");
    expect($(".ws-search").textContent).toContain("4 matches");
    await click($('button[aria-label="Match case"]'));
    expect($(".ws-search").textContent).toContain("0 matches");
  });

  it("filters the tree to changed files and collapses folders", async () => {
    await mount(CODE);
    await check('input[aria-label="Only changed files"]');
    expect(all(".tn").filter(t => t.querySelector(".gs")).map(t => t.textContent!.replace(/.$/, ""))).toEqual(["models.py", "refresh.py", "test_race.py"]);
    await click(named(".tn", "tests"));
    expect(all(".tn").some(t => t.textContent!.includes("test_race.py"))).toBe(false);
  });

  it("shows only what the fixture keeps, and says so", async () => {
    await mount(TASK(C.review, "/code?file=cedar%2Fexport%2Flimits.py"));
    expect(named(".cl", "class SlidingWindowLimiter")).toBeTruthy();
    expect($(".ebody").textContent).toContain("The fixture keeps only the changed lines");
  });

  it("is honest about a project with no code in the fixture", async () => {
    await mount(`/m/${LOCAL}/p/mock-project-00002/code`);
    expect($(".note-panel").textContent).toContain("The fixture keeps code for Cedar Notes only");
  });
});

describe("git", () => {
  const WORKTREE = "mock-checkout-00001";
  const GIT = PROJECT(`git?checkout=${WORKTREE}`);
  const groups = () => all(".git-g").map(g => g.textContent);
  it("shows status groups, ahead and behind, and the default checkout", async () => {
    await mount(PROJECT("git"));
    expect($(".scope-btn").textContent).toContain("cw/fix-token-refresh");
    expect($(".git-sum").textContent).toContain("2 ahead, 0 behind main");
    expect(groups()).toEqual(["Staged1", "Unstaged1", "Untracked1"]);
    expect($(".commit-box textarea").textContent ?? "").toBeDefined();
    expect($<HTMLTextAreaElement>(".commit-box textarea").value).toBe("Add token version to refresh rows");
  });

  it("stages and commits, and the commit lands in history", async () => {
    await mount(GIT);
    await click($('button[aria-label="Stage refresh.py"]'));
    expect(groups()).toEqual(["Staged2", "Untracked1"]);
    await click(named(".commit-box .btn", "Commit"));
    expect(groups()).toEqual(["Untracked1"]);
    expect($(".hrow").textContent).toContain("Add token version to refresh rows");
    expect($(".git-sum").textContent).toContain("3 ahead");
    expect(named(".commit-box .btn", "Commit").getAttribute("aria-disabled")).toBe("true");
  });

  it("asks before reverting a file", async () => {
    await mount(GIT);
    await click($('button[aria-label="Revert refresh.py"]'));
    expect(dialog().textContent).toContain("A task is working in this checkout");
    await click(inDialog("Cancel"));
    expect(groups()).toContain("Unstaged1");
    await click($('button[aria-label="Revert refresh.py"]'));
    await click(inDialog("Revert"));
    expect(groups()).not.toContain("Unstaged1");
  });

  it("push confirms remote and branch first", async () => {
    await mount(GIT);
    await click($('button[aria-label="Push"]'));
    expect(dialog().textContent).toContain("Push cw/fix-token-refresh to origin?");
    expect(dialog().textContent).toContain("never forces");
    await click(inDialog("Push"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect($(".cdetail").textContent).not.toContain("Not pushed");
  });

  it("shows history with task badges and a commit's actions", async () => {
    await mount(GIT);
    expect($(".hrow .tagc.task").textContent).toContain("Fix token refresh race");
    expect($(".cdetail").textContent).toContain("Lock the refresh rotation");
    await click(all(".hrow")[2]);
    expect($(".cdetail").textContent).toContain("Merge pull request from cw/retry-after");
    await click(named(".cdetail .btn", "Create branch here"));
    expect(dialog().textContent).toContain("4f2a91c");
    await type($("dialog[open] input"), "topic/from-here");
    await click(inDialog("Create branch"));
    await click(named('.seg[aria-label="Git view"] button', "Branches and worktrees"));
    expect(all(".gr").some(r => r.textContent!.includes("topic/from-here"))).toBe(true);
  });

  it("shows the conflict state of a checkout, with Commit held back", async () => {
    await mount(PROJECT("git?checkout=mock-checkout-00004"));
    expect($(":is(.strip.bad, .l-strip[data-tone=fail])").textContent).toContain("Merging main into cw/usage-rollups: 1 file with conflicts");
    expect(groups()).toEqual(["Conflicts1", "Staged5"]);
    expect(named(":is(.strip, .l-strip) .btn", "Resolve automatically")).toBeTruthy();
    const commit = named(".commit-box .btn", "Commit");
    expect(commit.getAttribute("aria-disabled")).toBe("true");
    expect(commit.title).toBe("Resolve the conflict first");
    await click(named(":is(.strip, .l-strip) .btn", "Resolve"));
    expect(location.pathname).toBe(TASK(C.rollups, "/changes/conflict"));
  });

  it("chooses a checkout from the strip", async () => {
    await mount(PROJECT("git"));
    await click(named("#m-checkout .mi", "main"));
    expect(location.search).toContain("checkout=mock-checkout-00000");
    expect($(".scope-btn").textContent).toContain("Main checkout");
    expect(groups()).toEqual(["Unstaged2"]);
  });

  it("is honest about a project with no Git record", async () => {
    await mount(`/m/${LOCAL}/p/mock-project-00002/git`);
    expect($(".note-panel").textContent).toContain("No Git record for this project");
  });
});

describe("worktrees", () => {
  const WORKTREES = PROJECT("worktrees");
  const row = (name: string) => all(".gr").find(r => r.textContent!.includes(name))!;
  const open = async (name: string) => click(row(name).querySelector("button")!);
  it("lists each checkout with its task, counts and state, and marks the tab", async () => {
    await mount(WORKTREES);
    expect(named(".ptab", "Worktrees").getAttribute("aria-selected")).toBe("true");
    expect(row("cw/fix-token-refresh").textContent).toContain("Active");
    expect(row("cw/export-rate-limit").textContent).toContain("Idle");
    expect(row("cw/usage-rollups").textContent).toContain("Conflict");
    expect(row("cw/usage-rollups").textContent).toContain("4 / 3");
    expect(row("cw/retry-webhooks").textContent).toContain("Merged");
    expect(row("cw/old-import").textContent).toContain("Orphaned");
    expect(row("release/2.4").textContent).toContain("No checkout");
  });

  it("keeps a live task with no Git record, and shows no figures for it", async () => {
    await mount(WORKTREES);
    expect(row("cw/task-228").textContent).not.toMatch(/\d+ \/ \d+|Unavailable/);
    await open("cw/task-228");
    expect(named("#m-worktree .mi", "Delete worktree").getAttribute("aria-disabled")).toBe("true");
  });

  it("deleting says what goes, keeps by default and needs discard to be named", async () => {
    await mount(WORKTREES);
    await open("cw/old-import");
    await click(named("#m-worktree .mi", "Delete worktree"));
    expect(dialog().textContent).toContain("2 unmerged commits");
    expect(dialog().textContent).toContain("1 uncommitted file");
    expect($<HTMLInputElement>('dialog[open] input[name="wt-keep"]').checked).toBe(true);
    expect(all("dialog[open] .btn-danger")).toHaveLength(0);
    await click(all<HTMLInputElement>('dialog[open] input[name="wt-keep"]')[1]);
    await click(inDialog("Delete and discard 1 file, 2 commits"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect(row("cw/old-import")).toBeUndefined();
  });

  it("a merged clean worktree goes in one step, and a running one cannot go at all", async () => {
    await mount(WORKTREES);
    await open("cw/retry-webhooks");
    await click(named("#m-worktree .mi", "Delete worktree"));
    expect(dialog().textContent).toContain("0 unmerged commits");
    await click(inDialog("Delete worktree"));
    expect(row("cw/retry-webhooks")).toBeUndefined();
    await open("cw/fix-token-refresh");
    await click(named("#m-worktree .mi", "Delete worktree"));
    expect(dialog().textContent).toContain("Close “Fix token refresh race” before deleting its worktree");
    expect(all("dialog[open] .btn-danger, dialog[open] .btn-primary")).toHaveLength(0);
  });

  it("turns an orphaned worktree into a task", async () => {
    await mount(WORKTREES);
    await open("cw/old-import");
    await click(named("#m-worktree .mi", "Attach as task"));
    expect(row("cw/old-import").textContent).toContain("Continue the work in cw/old-import");
    expect(row("cw/old-import").textContent).toContain("Idle");
  });

  it("merges from a row only when the task is in Ready to merge", async () => {
    await mount(WORKTREES);
    await open("cw/export-rate-limit");
    expect(named("#m-worktree .mi", "Merge…").getAttribute("aria-disabled")).toBe("true");
    await open("cw/usage-events");
    await click(named("#m-worktree .mi", "Merge…"));
    expect(inDialog("Confirm merge into main")).toBeTruthy();
  });
});

describe("read models", () => {
  it("pairs removed and added lines side by side, keeping the rows aligned", () => {
    const line = (kind: "ctx" | "add" | "del", old: number | null, neu: number | null, text: string) => ({ kind, old, new: neu, text });
    const rows = splitRows({ header: "@@", staged: false, lines: [line("ctx", 1, 1, "a"), line("del", 2, null, "b"), line("add", null, 2, "c"), line("add", null, 3, "d"), line("ctx", 3, 4, "e")] });
    expect(rows.map(r => [r.left?.text ?? null, r.right?.text ?? null])).toEqual([["a", "a"], ["b", "c"], [null, "d"], ["e", "e"]]);
  });
  it("highlights by file type and leaves unknown types as plain text", () => {
    expect(highlight("a.py", "def go(x):  # note").filter(t => t.c).map(t => [t.c, t.t])).toEqual([["tok-k", "def"], ["tok-f", "go"], ["tok-c", "# note"]]);
    expect(highlight("a.sql", "CREATE TABLE t (n BIGINT);").some(t => t.c === "tok-k" && t.t === "BIGINT")).toBe(true);
    expect(highlight("a.txt", "def go()")).toEqual([{ c: null, t: "def go()" }]);
  });
});
