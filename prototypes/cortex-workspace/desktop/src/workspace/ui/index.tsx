import { createEffect, onCleanup, untrack } from "solid-js";
import { Workspace as SharedWorkspace, type DemoSlot, type Ws } from "@cortex/workspace-ui";
import { Pick } from "@cortex/workspace-ui/settings-kit";
import type { ConnectOutcome, WorkspaceCommand as OwnerCommand } from "@cortex/workspace-domain";
import { loadFixtures } from "../../fixtures";
import { WORKSPACE_FIXTURE_NOW } from "../adapter/fixtures";
import { createMockWorkspace } from "../adapter/mock";
import { generateMockTitle, mockConnect, SERVER_TEMPLATES, type WorkspaceCommand, type WorkspacePort } from "../domain";
import { createDemoBackend } from "./demo-backend";
import { DEMO_COPY } from "./demo-copy";
import { DemoMenu } from "./demo-menu";
import { createMockControls, createSimulator } from "./sim";
import "./demo.css";

/** Long enough to see the state, short enough for a mock. Nothing is started: the tools come from the fixture. */
export const MOCK_CONNECT_MS = 400;
const SPOKEN = "and keep the lock scope to the rotation only".split(" ");
const MIC_FAULTS = { denied: "Microphone access was refused. Typing still works.", no_provider: "No dictation provider is set up. Typing still works." };
const OUTCOMES: { id: ConnectOutcome | "auto"; label: string }[] = [
  { id: "auto", label: "As the fixture says" }, { id: "command_not_found", label: "Command not found" }, { id: "exited", label: "Exited at start" }, { id: "timeout", label: "Timed out" },
  { id: "path_refused", label: "Path refused" }, { id: "needs_variable", label: "Needs a variable" }, { id: "not_supported", label: "Not supported" },
];

/** The labelled demo entry: the accepted Workspace on simulated data. It is the only place that chooses a mock backend. */
export default function Workspace(props: { port?: () => WorkspacePort; autoplay?: boolean }) {
  const backend = createDemoBackend(props.port ?? (() => createMockWorkspace(loadFixtures().workspace)), WORKSPACE_FIXTURE_NOW);
  const mock = createMockControls(props.autoplay ?? true);
  const sim = createSimulator(backend, mock);
  let live: Ws | undefined;
  const run = (command: WorkspaceCommand) => live?.run(command as OwnerCommand);
  const demo: DemoSlot = {
    chip: () => <button class="mock-chip" popovertarget="m-mock" title="Mock prototype. Fixture data, not runtime evidence">Mock</button>,
    menu: () => <DemoMenu backend={backend} mock={mock} sim={sim} />,
    copy: DEMO_COPY,
    attach(ws) {
      live = ws;
      // Planners and running queues move on their own here, wherever the owner is looking.
      createEffect(() => {
        if (!mock.autoplay()) return;
        const timer = setInterval(() => sim.background(), 2500);
        onCleanup(() => clearInterval(timer));
      });
    },
    autoplay: mock.autoplay,
    step: id => { sim.step(id); },
    title: { draft: generateMockTitle, fails: mock.titleFails },
    stopConfirmed() { const confirmed = !mock.unconfirmedStop(); mock.setUnconfirmedStop(false); return confirmed; },
    takeSaveFault() { const fault = untrack(mock.saveFault); mock.setSaveFault(null); return fault; },
    pairFault: mock.pairFault, clearPairFault: () => mock.setPairFault(null),
    dictation: { spoken: SPOKEN, fault() { const fault = mock.micFault(); return fault ? MIC_FAULTS[fault] : null; } },
    connect: {
      delayMs: MOCK_CONNECT_MS, run: mockConnect, templates: SERVER_TEMPLATES,
      outcome: (forced, set, id) => <div class="row ws-gap4"><span class="mock-chip chip ws-static">Mock</span><span class="cap muted">Next result</span><Pick id={id} label="Mock: next connect result" width="180px" value={forced()} options={OUTCOMES} onPick={x => set(x as ConnectOutcome | "auto")} /></div>,
    },
    touchConfig: (machineId, surface) => void run({ type: "config_changed", machineId, surface }),
    reconnect: machineId => void run({ type: "set_connection", machineId, connection: "connected" }),
    reloadProfiles: machineId => void run({ type: "set_config_fault", machineId, fault: "unavailable", on: false }),
    newId(kind) { const state = backend.state(); return `mock-${kind}-${state.nextId + (kind === "machine" ? state.machines : state.projects).length}`; },
    unique: () => String(backend.state().nextId),
  };
  return <>
    <div class="ws-demo-banner" role="note">Demo — simulated data, no real actions</div>
    <SharedWorkspace port={backend.port} initial={backend.initial()} now={WORKSPACE_FIXTURE_NOW} demo={demo} />
  </>;
}
