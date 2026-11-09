import { For, Show } from "solid-js";
import { useWs } from "@cortex/workspace-ui";
import { MenuItem, Pop } from "@cortex/workspace-ui/kit";
import type { ConfigFault, Machine, WorkspaceCommand as OwnerCommand } from "@cortex/workspace-domain";
import type { WorkspaceCommand } from "../domain";
import type { DemoBackend } from "./demo-backend";
import type { MockControls, Simulator } from "./sim";

const CONFIG_FAULTS: [ConfigFault, string, (m: Machine) => boolean][] = [
  ["env_routing", "$CORTEX_ROUTING is set", m => !!m.config.files.envRouting], ["invalid_routing", "Routing file is invalid", m => m.config.files.invalidRouting],
  ["migration", "Model policy needs migration", m => m.config.files.migration], ["catalogue", "Codex model list unreadable", m => m.config.catalogue["codex-acp"].failed],
  ["interrupted", "Profile switch interrupted", m => m.config.interrupted], ["unavailable", "Profiles file unreadable", m => !!m.config.unavailable],
  ["engine_missing", "Codex adapter not downloaded", m => m.config.setup["codex-acp"].status === "not_downloaded"], ["preset_missing", "routing.json is absent", m => m.config.files.missing.length > 0],
];

/** The owner's controls for the simulation. It lives with the demo entry and is the only place these commands start. */
export function DemoMenu(props: { backend: DemoBackend; mock: MockControls; sim: Simulator }) {
  const ws = useWs();
  const { backend, mock, sim } = props;
  const run = (command: WorkspaceCommand) => ws.run(command as OwnerCommand);
  const current = () => ws.machine();
  const state = () => backend.state();
  const taskId = () => { const r = ws.router.route(); return r.name === "task" ? r.task : null; };
  const taskWorker = (id: string) => ws.store.state().workers.find(w => w.taskId === id && w.status === "working")?.id ?? null;
  const sprintId = () => { const r = ws.router.route(); return r.name === "sprint" ? r.sprint : r.name === "project" && r.scope.kind === "sprint" ? r.scope.id : null; };
  const activeTask = () => { const s = ws.store.state().sprints.find(x => x.id === sprintId()); return s?.taskIds.map(id => ws.store.state().tasks.find(t => t.id === id)).find(t => t?.status === "working") ?? null; };
  const noActive = () => activeTask() ? undefined : "No task of this sprint is running";
  const onActive = (act: (taskId: string) => void) => () => { const t = activeTask(); if (t) act(t.id); };
  return <Pop id="m-mock" class="pop-right" align="right" label="Mock prototype">
    <div class="mi-h">Mock prototype, fixture data, not runtime evidence</div>
    <Show when={taskId()}>{id => <>
      <MenuItem icon="play" label="Simulate the next step" sub="Moves a message on, or adds worker activity" onSelect={() => { sim.step(id()); }} />
      <MenuItem icon="question" label="Ask a question (simulated)" onSelect={() => { const w = taskWorker(id()); void run({ type: "ask_decision", taskId: id(), workerId: w, text: "Which mock option should this use?", options: [{ label: "First option" }, { label: "Second option" }], now: ws.store.clock() }); }} />
      <MenuItem icon="check-circle" label="Publish a result (simulated)" onSelect={() => void run({ type: "publish_result", taskId: id(), digest: `mock-demo-${state().nextId}`, checks: [{ name: "Mock check", outcome: "passed", detail: "Simulation, no command ran" }] })} />
      <div class="mi-sep"></div>
    </>}</Show>
    <Show when={sprintId()}>{id => <>
      <MenuItem icon="play" label="Simulate the next step" sub="The planner answers, or the active task finishes" onSelect={() => { sim.sprintStep(id()); }} />
      <MenuItem icon="question" label="Active task asks a question" disabled={noActive()} onSelect={onActive(taskId => void run({ type: "ask_decision", taskId, workerId: null, text: "Which mock option should this use?", options: [{ label: "First option" }, { label: "Second option" }], now: ws.store.clock() }))} />
      <MenuItem icon="x-circle" label="Active task fails a check" disabled={noActive()} onSelect={onActive(taskId => void run({ type: "publish_result", taskId, digest: `mock-failed-${state().nextId}`, checks: [{ name: "Mock check", outcome: "failed", detail: "Simulated failure, no command ran" }] }))} />
      <MenuItem icon="alert" label="Active task hits a conflict" disabled={noActive()} onSelect={onActive(taskId => void run({ type: "set_conflict", taskId }))} />
      <div class="mi-sep"></div>
    </>}</Show>
    <MenuItem label="Play the simulation on its own" check={mock.autoplay()} onSelect={() => mock.setAutoplay(!mock.autoplay())} />
    <MenuItem label="Next stop stays unconfirmed" check={mock.unconfirmedStop()} onSelect={() => mock.setUnconfirmedStop(!mock.unconfirmedStop())} />
    <MenuItem label="Next automatic resolution fails" check={mock.resolveFails()} onSelect={() => mock.setResolveFails(!mock.resolveFails())} />
    <MenuItem label="Next title generation fails" check={mock.titleFails()} onSelect={() => mock.setTitleFails(!mock.titleFails())} />
    <MenuItem label="Dictation: microphone refused" check={mock.micFault() === "denied"} onSelect={() => mock.setMicFault(mock.micFault() === "denied" ? null : "denied")} />
    <MenuItem label="Dictation: no provider" check={mock.micFault() === "no_provider"} onSelect={() => mock.setMicFault(mock.micFault() === "no_provider" ? null : "no_provider")} />
    <MenuItem label="Next pairing: code expired" check={mock.pairFault() === "expired"} onSelect={() => mock.setPairFault(mock.pairFault() === "expired" ? null : "expired")} />
    <MenuItem label="Next pairing: connector unreachable" check={mock.pairFault() === "connector"} onSelect={() => mock.setPairFault(mock.pairFault() === "connector" ? null : "connector")} />
    <MenuItem label="Next pairing: fingerprints differ" check={mock.pairFault() === "fingerprint"} onSelect={() => mock.setPairFault(mock.pairFault() === "fingerprint" ? null : "fingerprint")} />
    <MenuItem label="Next save hits a conflict" check={mock.saveFault() === "conflict"} onSelect={() => mock.setSaveFault(mock.saveFault() === "conflict" ? null : "conflict")} />
    <MenuItem label="Next save times out" check={mock.saveFault() === "timeout"} onSelect={() => mock.setSaveFault(mock.saveFault() === "timeout" ? null : "timeout")} />
    <Show when={ws.router.route().name === "settings"}>
      <div class="mi-sep"></div>
      <For each={CONFIG_FAULTS}>{([fault, label, on]) => <MenuItem label={label} check={on(current())} onSelect={() => void run({ type: "set_config_fault", machineId: current().id, fault, on: !on(current()) })} />}</For>
    </Show>
    <Show when={current().kind === "remote"}>
      <div class="mi-sep"></div>
      <MenuItem icon="wifi-off" label={current().connection === "connected" ? `Disconnect ${current().name}` : `Reconnect ${current().name}`} onSelect={() => void run({ type: "set_connection", machineId: current().id, connection: current().connection === "connected" ? "disconnected" : "connected", now: ws.store.clock() })} />
    </Show>
    <div class="mi-sep"></div>
    <MenuItem icon="undo" label="Reset prototype" sub="Restores the fixture. Panels and theme stay as they are." onSelect={() => { backend.reset(); sim.reset(); ws.drafts.clear(); ws.select(null); ws.store.clearReceipt(); }} />
  </Pop>;
}
