import { Match, Show, Switch } from "solid-js";
import { useWs } from "./ctx";
import { CoordinatorModels } from "./orch-coordinator";
import { Defaults } from "./orch-defaults";
import { ProjectCeiling } from "./orch-limits";
import { Tools } from "./orch-mcp";
import { EnginesModels, Models } from "./orch-models";
import { RoutePreview } from "./orch-preview";
import { Files, Profiles } from "./orch-profiles";
import { Routing } from "./orch-routing";
import type { OrchestrationPage } from "./router";

export { useReadOnly } from "./orch-shared";

/**
 * The eight sections of the Cortex settings panel, in its order, plus the pages they open (Route preview, Files and sources,
 * the MCP add and edit pages). The bare route opens Defaults. The old Limits and fallback link lands on the machine ceiling
 * under Defaults, or on Project ceiling when a project is chosen.
 */
export function Orchestration(props: { page?: OrchestrationPage; server?: string; tab?: string }) {
  const ws = useWs();
  return <Switch fallback={<Defaults />}>
    <Match when={props.page === "coordinator"}><CoordinatorModels /></Match>
    <Match when={props.page === "models"}><Models /></Match>
    <Match when={props.page === "engines"}><EnginesModels /></Match>
    <Match when={props.page === "ceiling"}><ProjectCeiling /></Match>
    <Match when={props.page === "routing"}><Routing tab={props.tab} /></Match>
    <Match when={props.page === "effective"}><RoutePreview /></Match>
    <Match when={props.page === "policy"}><Show when={ws.settings.scope()} fallback={<Defaults focus="ceiling" />}><ProjectCeiling /></Show></Match>
    <Match when={props.page === "tools"}><Tools server={props.server} tab={props.tab} /></Match>
    <Match when={props.page === "profiles"}><Profiles /></Match>
    <Match when={props.page === "files"}><Files /></Match>
  </Switch>;
}
