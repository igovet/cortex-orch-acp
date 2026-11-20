// @vitest-environment jsdom
import { render } from "solid-js/web";
import { expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { projection, scriptedPort } from "./support";

it("opens the Testing preferred-model menu and saves an eligible selection only when routing is editable", async () => {
  localStorage.clear();
  sessionStorage.clear();
  history.replaceState(null, "", `/settings/machine/${LOCAL_MACHINE_ID}/orchestration/routing`);
  const state = emptyWorkspace();
  const machine = state.machines[0];
  machine.connection = "connected";
  const config = machine.config;
  const engine = "claude-agent-acp";
  config.catalogue[engine].models = [
    { id: "sonnet", label: "Sonnet", efforts: ["medium"], observed: true },
    { id: "opus", label: "Opus", efforts: ["medium"], observed: true },
    { id: "haiku", label: "Haiku", efforts: ["medium"], observed: true },
  ];
  const rules = config.surfaces.routing;
  rules.providers = [engine];
  rules.engines[engine].enabled = ["sonnet", "opus"];
  rules.engines[engine].auto = ["sonnet"];
  rules.engines[engine].tiers = { sonnet: ["standard"] };
  config.revisions.routing = 12;
  config.files.envRouting = "/machine/routing.json";

  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  const root = document.createElement("div");
  document.body.append(root);
  const add = () => root.querySelector<HTMLButtonElement>('button[aria-label="Add a preferred model for Testing"]');
  const menu = () => root.querySelector<HTMLElement>("#m-pref-testing");
  // jsdom has no Popover API. Supply only declarative button activation, using the
  // actual target ID and popover attribute from the rendered screen.
  const activate = (event: MouseEvent) => {
    if (event.defaultPrevented) return;
    const id = (event.target as Element).closest("button")?.getAttribute("popovertarget");
    const popover = id ? document.getElementById(id) : null;
    if (popover?.hasAttribute("popover")) popover.toggleAttribute("data-test-popover-open");
  };
  root.addEventListener("click", activate);
  const dispose = render(() => <Workspace port={s.port} />, root);
  try {
    await vi.waitFor(() => expect(s.listeners.size).toBeGreaterThan(0));
    s.emit(projection("1", structuredClone(state)));
    await vi.waitFor(() => expect(root.querySelector("h1.set-title")?.textContent).toBe("Routing"));
    expect(root.textContent?.includes("Routing uses an environment override and is read-only here")).toBe(true);
    expect(add()).toBeNull();
    expect(menu()).toBeNull();
    expect(s.commands).toHaveLength(0);

    config.files.envRouting = null;
    s.emit(projection("2", structuredClone(state)));
    await vi.waitFor(() => expect(add()).not.toBeNull());
    expect(menu()?.hasAttribute("data-test-popover-open")).toBe(false);
    add()!.click();
    const opened = menu();
    expect(opened?.hasAttribute("data-test-popover-open")).toBe(true);
    expect(opened?.getAttribute("aria-label")).toBe("Preferred for Testing");
    const options = [...opened!.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    const eligible = options.filter(option => option.getAttribute("aria-disabled") !== "true");
    expect(eligible.map(option => option.querySelector(".sub")?.textContent)).toEqual(["sonnet"]);
    const manual = options.find(option => option.querySelector(".sub")?.textContent === "opus");
    expect(manual?.getAttribute("aria-disabled")).toBe("true");
    expect(options.some(option => option.querySelector(".sub")?.textContent === "haiku")).toBe(false);
    manual!.click();
    expect(root.querySelector('[aria-label="Remove Opus from Testing"]')).toBeNull();

    eligible[0].click();
    expect(root.querySelector('[aria-label="Remove Sonnet from Testing"]')).not.toBeNull();
    expect(s.commands).toHaveLength(0);
    const save = [...root.querySelectorAll("button")].find(b => b.textContent?.trim() === "Save routing rules on This machine");
    expect(save).toBeDefined();
    save!.click();
    await vi.waitFor(() => expect(s.commands).toHaveLength(1));
    expect(s.commands[0].payload).toMatchObject({
      type: "save_surface", surface: "routing", machineId: LOCAL_MACHINE_ID, projectId: null, revision: 12,
      value: { engines: { [engine]: { policy: { sonnet: { preferredFor: ["testing"] } } } } },
    });
    s.receipt();
  } finally {
    dispose();
    root.remove();
  }
});
