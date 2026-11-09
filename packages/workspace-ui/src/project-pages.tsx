import { Show } from "solid-js";
import { capabilityOf, machineDefaults } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, I } from "./kit";
import type { ProjectSummary } from "./selectors";
import { Srow } from "./settings-kit";

/** These commands become ordinary tasks: review, verification and Ready to merge apply to them like any other work. */
export function KnowledgePage(props: { summary: ProjectSummary }) {
  const ws = useWs();
  const off = () => props.summary.offline ? `${props.summary.machine.name} is offline` : undefined;
  const create = async (description: string) => {
    const { settings } = machineDefaults(props.summary.machine);
    const receipt = await ws.run({ type: "create_task", projectId: props.summary.project.id, description, settings, checkout: { kind: "main" } });
    if (!receipt.ok || !receipt.entityId) return false;
    if (ws.can(capabilityOf("generate_title"))) void ws.run({ type: "generate_title", taskId: receipt.entityId });
    ws.go({ name: "task", machine: props.summary.machine.id, task: receipt.entityId });
    return true;
  };
  const ask = (title: string, label: string, action: string, prefix: string, hint: string) => ws.open({ kind: "prompt", title, label, action, hint, optional: true, go: async scope => {
      const description = ws.store.mode === "real" ? String((await ws.store.machineAction("workflow.describe", {action:prefix, scope})).description) : `${prefix}: ${scope}`;
      return create(description);
    } });
  const button = (text: string, run: () => void) => <button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) run(); }}>{text}</button>;
  return <div class="page"><div class="page-in">
    <div class="page-h"><div><h1>Knowledge and skills</h1><p>Each of these starts a normal task, with the usual review and Ready to merge.</p></div></div>
    <div class="surface">
      <Srow t="Harvest project knowledge" d="Reads the project and writes what future tasks should know into its docs.">{button("Collect knowledge…", () => ask("Collect project knowledge", "Scope, optional", "Create the task", "harvest", "Leave it empty to cover the whole project."))}</Srow>
      <Srow t="Refresh knowledge" d="Updates the existing docs from the current source.">{button("Refresh knowledge…", () => ask("Refresh project knowledge", "Scope, optional", "Create the task", "harvest-refresh", "Leave it empty to cover the whole project."))}</Srow>
      <Srow t="Project skills" d="Create a skill, change one, or import one from a link.">{button("New skill…", () => ask("New project skill", "What should the skill do", "Create the task", "skill-management", "Describe the skill, or paste a link to import."))}</Srow>
    </div>
  </div></div>;
}

export function ProjectSettingsPage(props: { summary: ProjectSummary }) {
  const ws = useWs();
  const p = () => props.summary.project, m = () => props.summary.machine;
  const rename = (name: string) => { if (name.trim() && name.trim() !== p().name) void ws.run({ type: "rename_project", projectId: p().id, name: name.trim() }); };
  const own = () => { const c = m().config.projects[p().id]; return [c?.routing ? "its own routing file" : null, c?.ceiling ? "its own limits" : null].filter(Boolean).join(" and "); };
  return <div class="page"><div class="page-in">
    <div class="page-h"><div><h1>Project settings</h1><p>What sets this project apart from its machine.</p></div></div>
    <div class="surface">
      <Srow t="Name" d="A new project starts with its folder's name."><input class="inp" aria-label="Project name" value={p().name} disabled={!!ws.gate("rename_project")} onChange={e => rename(e.currentTarget.value)} /><Show when={ws.gate("rename_project")}>{why => <span class="ws-why">{why()}</span>}</Show></Srow>
      <Srow t="Repository" d="Folder on this machine."><span class="mono">{p().repository}</span></Srow>
      <Srow t="Open branch" d="Where the main checkout is now."><span class="mono">{p().openBranch}</span></Srow>
      <Srow t="Git default branch" d="Tasks merge here unless you pick another target."><span class="mono">{p().defaultBranch}</span></Srow>
      <Srow t="Machine" d="Where this project lives."><A class="lnk" to={{ name: "settings", section: "overview", machine: m().id }}>{m().name}</A></Srow>
      <Srow t="Worktrees" d="New worktrees for this project go under the machine's folder."><A class="lnk mono" to={{ name: "settings", section: "worktrees", machine: m().id }}>{m().worktreeRoot}</A></Srow>
    </div>
    <div class="set-h">Orchestration</div>
    <div class="surface"><Srow t="Project override" d={own() ? `This project has ${own()}.` : "Follows the machine."}>
      <button class="btn btn-line btn-sm" onClick={() => { ws.settings.setScope(p().id); ws.go({ name: "settings", section: "orchestration", machine: m().id, page: "routing" }); }}><I n="sliders" s={12} />Open overrides</button>
    </Srow></div>
    <Show when={props.summary.offline}><div class="cap faint">{m().name} is offline. This is what it last reported.</div></Show>
  </div></div>;
}
