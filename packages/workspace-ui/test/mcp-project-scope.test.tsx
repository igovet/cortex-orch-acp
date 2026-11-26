import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, newMcpServer, type MachineConfig, type McpServer, type Project, type WorkspaceCommand, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort } from "./support";

/**
 * MCP servers with a project chosen: the machine's servers seen from the project (read-only, with the project's own on or off
 * choice) and the project's own servers. One write per project, never touching the machine's file.
 */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const button = (text: string) => all("button, a").find(b => b.textContent?.trim() === text || b.getAttribute("aria-label") === text);
const M = `/settings/machine/${LOCAL_MACHINE_ID}/orchestration/tools`;

const server = (name: string, patch: Partial<McpServer> = {}): McpServer => ({ ...newMcpServer(), name, command: `/opt/${name}/bin/${name}`, enabled: true, allowedTools: ["generate"], readOnlyTools: ["generate"], ...patch });
const cedar: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" };
const life: Project = { id: "q", machineId: LOCAL_MACHINE_ID, name: "Life Is Broken", repository: "/life", openBranch: "main", defaultBranch: "main" };
function world(patch?: (c: MachineConfig) => void): WorkspaceState {
  const state = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push(cedar, life);
  const c = state.machines[0].config;
  c.surfaces.mcp_servers = [server("kling", { enabled: false }), server("docs")];
  c.revisions.mcp_servers = 7;
  c.projects.p = { mcp: { servers: [], overrides: {} }, revisions: { mcp_servers: 0 } };
  c.projects.q = { mcp: { servers: [server("scratch")], overrides: { kling: true } }, revisions: { mcp_servers: 5 } };
  patch?.(c);
  return state;
}
async function open(path: string, state = world()) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settled();
  s.emit(projection("1", state));
  await settled();
  return s;
}
const choose = async (name: string) => { all("#m-scope-project .mi").find(b => b.textContent?.includes(name))!.click(); await settled(); };
const row = (name: string) => all(`[data-inherited="${name}"]`)[0];
const save = () => all("button").find(b => /^Save MCP servers on/.test(b.textContent ?? ""));
const last = (s: ReturnType<typeof scriptedPort>) => s.commands.at(-1)!.payload as Extract<WorkspaceCommand, { type: "save_surface" }>;

describe("MCP servers for one project", () => {
  it("offers the project picker, and with no project chosen shows only the machine's servers", async () => {
    await open(M);
    expect($('[popovertarget="m-scope-project"]')).toBeTruthy();
    expect(document.body.textContent).toContain("kling");
    expect(row("kling")).toBeUndefined();
    expect(document.body.textContent).not.toContain("From the machine");
  });

  it("shows the machine's servers read-only with the project's choice, and its own servers apart", async () => {
    await open(M);
    await choose("Life Is Broken");
    expect(document.body.textContent).toContain("From the machine");
    expect(document.body.textContent).toContain("Only in this project");
    // Kling is off on the machine and switched on here; docs follows the machine; the project's own server is listed.
    expect(row("kling").textContent).toContain("On for this project only");
    expect(row("docs").textContent).toContain("On, as on the machine");
    expect(all<HTMLInputElement>('input[aria-label="kling for this project"]')[0].checked).toBe(true);
    expect(document.body.textContent).toContain("scratch");
    expect(button("Edit on the machine")).toBeTruthy();
  });

  it("switching a machine server off for a project is a draft that one project write carries", async () => {
    const s = await open(M);
    await choose("Cedar");
    expect(row("kling").textContent).toContain("Off, as on the machine");
    // Turn docs off for Cedar only.
    all<HTMLInputElement>('input[aria-label="docs for this project"]')[0].click();
    await settled();
    expect(row("docs").textContent).toContain("Off for this project only");
    expect(s.commands).toHaveLength(0);
    save()!.click();
    await settled();
    expect(last(s)).toMatchObject({ type: "save_surface", projectId: "p", surface: "mcp_servers", revision: 0, value: { servers: [], overrides: { docs: false } } });
  });

  it("a choice equal to the machine's setting is not stored", async () => {
    const s = await open(M);
    await choose("Cedar");
    const toggle = () => all<HTMLInputElement>('input[aria-label="docs for this project"]')[0];
    toggle().click(); await settled();
    toggle().click(); await settled();
    expect(row("docs").textContent).toContain("On, as on the machine");
    expect(save()).toBeUndefined();
    expect(s.commands).toHaveLength(0);
  });

  it("switches a machine server on for the project that needs it and writes only that project", async () => {
    const s = await open(M);
    await choose("Cedar");
    all<HTMLInputElement>('input[aria-label="kling for this project"]')[0].click();
    await settled();
    save()!.click();
    await settled();
    expect(last(s)).toMatchObject({ projectId: "p", surface: "mcp_servers", value: { overrides: { kling: true } } });
    expect(s.commands.every(c => (c.payload as { projectId?: string }).projectId === "p")).toBe(true);
  });

  it("a project server with a machine server's name says it replaces it, and the machine row has no switch", async () => {
    await open(M, world(c => { c.projects.q.mcp!.servers = [server("docs", { command: "/opt/other/docs" })]; c.projects.q.mcp!.overrides = {}; }));
    await choose("Life Is Broken");
    expect(row("docs").textContent).toContain("Replaced by this project's server of the same name");
    expect(all('input[aria-label="docs for this project"]')).toHaveLength(0);
    expect(document.body.textContent).toContain("Replaces the machine's server of the same name here");
  });

  it("removing a project server and saving writes the project's list without it", async () => {
    const s = await open(M);
    await choose("Life Is Broken");
    button("More for scratch")!.click(); await settled();
    all("#m-srv-scratch .mi").find(b => b.textContent?.includes("Remove"))!.click(); await settled();
    button("Remove")!.click(); await settled();
    save()!.click(); await settled();
    expect(last(s)).toMatchObject({ projectId: "q", surface: "mcp_servers", value: { servers: [], overrides: { kling: true } } });
  });

  it("refuses edits while the project's registry cannot be read or changed", async () => {
    await open(M, world(c => { c.projects.q.mcp!.problem = "mcp-servers.json is not valid UTF-8 JSON; repair it by hand. Nothing was changed."; }));
    await choose("Life Is Broken");
    const toggle = all<HTMLInputElement>('input[aria-label="docs for this project"]')[0];
    expect(toggle.disabled || toggle.getAttribute("aria-disabled") === "true").toBe(true);
    expect(document.body.textContent).toContain("repair it by hand");
  });

  it("with no project chosen, a machine save is unchanged: the server list, no project", async () => {
    const s = await open(M);
    all<HTMLInputElement>('input[aria-label="docs enabled"]')[0].click();
    await settled();
    save()!.click();
    await settled();
    const sent = last(s);
    expect(sent.projectId).toBeUndefined();
    expect(sent.surface).toBe("mcp_servers");
    expect(Array.isArray(sent.value)).toBe(true);
    expect(sent.revision).toBe(7);
  });
});
