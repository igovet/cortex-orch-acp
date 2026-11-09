import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { COPY, Workspace as SharedWorkspace } from "@cortex/workspace-ui";
import { capabilityOf, MACHINE_COMMANDS, WIRE_OPERATIONS, type PortCapabilities, type WorkspacePort } from "@cortex/workspace-domain";
import { loadFixtures } from "../../fixtures";
import { WORKSPACE_CASES as C, WORKSPACE_FIXTURE_NOW } from "../adapter/fixtures";
import { WORKSPACE_SESSION_WORKERS as W } from "../adapter/session-fixture";

const LOCAL = "mock-machine-00000", CEDAR = "mock-project-00001", SPRINT = loadFixtures().workspace.sprints[0].id;
const tick = () => new Promise(r => setTimeout(r, 0));
/** A machine that offers nothing, in front of the richest state there is: the fixture, shown as if it were real. */
const offersNothing: WorkspacePort = {
  mode: "real",
  query: () => Promise.reject(new Error("not used")),
  command: () => Promise.reject(new Error("a control reached the port")),
  subscribe: () => () => {},
};
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); Element.prototype.scrollIntoView = () => {}; });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
async function mount(path: string, port = offersNothing, capabilities: PortCapabilities = { everything: false, operations: [] }) {
  dispose?.(); document.body.innerHTML = "";
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <SharedWorkspace port={port} now={WORKSPACE_FIXTURE_NOW} initial={{ state: loadFixtures().workspace, version: "1", capabilities }} />, root);
  await tick();
}
const label = (el: HTMLElement) => (el.getAttribute("aria-label") ?? el.textContent ?? el.title).trim().slice(0, 40);
const refused = () => document.querySelector(".ws-receipt")?.textContent === COPY.notOffered;

/** Presses every control that looks enabled. One that turns out to send a command the machine lacks should have been off. */
async function sweep(path: string) {
  const offenders: string[] = [];
  await mount(path);
  const count = document.querySelectorAll("button").length;
  for (let i = 0; i < count; i++) {
    await mount(path);
    const control = document.querySelectorAll<HTMLElement>("button")[i];
    if (!control || control.getAttribute("aria-disabled") === "true" || control.hasAttribute("disabled")) continue;
    control.click();
    await tick();
    if (refused()) offenders.push(label(control));
  }
  return offenders;
}

const SCREENS: [string, string][] = [
    ["home", `/m/${LOCAL}`], ["board", `/m/${LOCAL}/p/${CEDAR}/board`], ["list", `/m/${LOCAL}/p/${CEDAR}/list`], ["git", `/m/${LOCAL}/p/${CEDAR}/git`], ["worktrees", `/m/${LOCAL}/p/${CEDAR}/worktrees`],
    ["session", `/m/${LOCAL}/t/${C.session}`], ["workers", `/m/${LOCAL}/t/${C.session}/workers`], ["worker", `/m/${LOCAL}/t/${C.session}/workers/${W.backend}`], ["live worker", `/m/${LOCAL}/t/${C.session}/workers/${W.backend}/live`],
    ["review", `/m/${LOCAL}/t/${C.review}/changes`], ["ready to merge", `/m/${LOCAL}/t/${C.merge}/changes`], ["question", `/m/${LOCAL}/t/${C.question}`], ["archive", `/m/${LOCAL}/archive`], ["inbox", `/m/${LOCAL}/inbox`], ["new task", `/m/${LOCAL}/new`],
    ["conflict", `/m/${LOCAL}/t/${C.conflict}/changes/conflict`], ["paused", `/m/${LOCAL}/t/${C.paused}`], ["stopping", `/m/${LOCAL}/t/${C.stopping}`], ["code", `/m/${LOCAL}/p/${CEDAR}/code`], ["knowledge", `/m/${LOCAL}/p/${CEDAR}/knowledge`],
    ["sprint", `/m/${LOCAL}/p/${CEDAR}/s/${SPRINT}/planning`], ["sprint board", `/m/${LOCAL}/p/${CEDAR}/s/${SPRINT}/board`],
    ["machines", "/settings/app/machines"], ["overview", `/settings/machine/${LOCAL}/overview`], ["worktree settings", `/settings/machine/${LOCAL}/worktrees`], ["diagnostics", `/settings/machine/${LOCAL}/diagnostics`], ["remote", `/settings/machine/${LOCAL}/remote`],
    ["models", `/settings/machine/${LOCAL}/orchestration/models`], ["routing", `/settings/machine/${LOCAL}/orchestration/routing`], ["tools", `/settings/machine/${LOCAL}/orchestration/tools`], ["profiles", `/settings/machine/${LOCAL}/orchestration/profiles`], ["files", `/settings/machine/${LOCAL}/orchestration/files`],
    ["connections", `/settings/machine/${LOCAL}/connections`], ["storage", `/settings/machine/${LOCAL}/storage`], ["orchestration", `/settings/machine/${LOCAL}/orchestration`],
];

describe("a missing capability", () => {
  it.each(SCREENS)("leaves no enabled control that would be refused on %s", async (_name, path) => {
    expect(await sweep(path)).toEqual([]);
  }, 60000);
});

/** Exactly what a connected machine offers, so a control that is on here is on for the owner. */
const REAL: PortCapabilities = { everything: false, operations: [...WIRE_OPERATIONS, "workspace.add_project", "workspace.activate_project", ...MACHINE_COMMANDS.map(capabilityOf)] };

/** Presses every control that looks enabled. One that changes nothing, sends nothing and goes nowhere is a dead button. */
async function dead(path: string) {
  const sent: string[] = [], offenders: string[] = [];
  const port: WorkspacePort = {
    mode: "real", subscribe: () => () => {},
    query: operation => Promise.resolve(operation === "capabilities.get" ? REAL : []) as never,
    command: envelope => { sent.push(envelope.operation); return Promise.resolve({ requestId: envelope.requestId, ok: true, outcome: "local", message: "Recorded" }); },
  };
  await mount(path, port, REAL);
  const count = document.querySelectorAll("button").length;
  for (let i = 0; i < count; i++) {
    await mount(path, port, REAL);
    const control = document.querySelectorAll<HTMLElement>("button")[i];
    // A popover and its items open natively, which the test document cannot do. What is already chosen has nothing left to do, a grip only answers to keys and drags, and a copy reaches a clipboard this document lacks.
    if (!control || control.hasAttribute("popovertarget") || control.closest("[popover]") || control.closest('[aria-current="true"], [aria-selected="true"]') || control.matches(".grip") || /^Copy/.test(label(control))) continue;
    if (control.getAttribute("aria-disabled") === "true" || control.hasAttribute("disabled")) continue;
    const at = location.href;
    let changed = false;
    Element.prototype.scrollIntoView = () => { changed = true; };
    const watch = new MutationObserver(() => { changed = true; });
    watch.observe(document.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });
    sent.length = 0;
    control.click();
    await tick(); await tick();
    watch.disconnect();
    if (!changed && !sent.length && location.href === at) offenders.push(label(control));
  }
  return offenders;
}

describe("what a real machine offers", () => {
  it.each(SCREENS)("leaves no enabled control that does nothing on %s", async (_name, path) => {
    expect((await dead(path)).join(" | ")).toBe("");
  }, 60000);
});
