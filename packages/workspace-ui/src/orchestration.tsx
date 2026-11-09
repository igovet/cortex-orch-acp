import { Match, Switch } from "solid-js";
import { Coordinator, Limits } from "./orch-limits";
import { Tools } from "./orch-mcp";
import { Models } from "./orch-models";
import { RoutePreview } from "./orch-preview";
import { Files, Overview, Profiles } from "./orch-profiles";
import { Routing } from "./orch-routing";
import type { OrchestrationPage } from "./router";

export { useReadOnly } from "./orch-shared";

/** One page per thing the owner decides, in the order of a real setup. Each page edits the file it belongs to. */
export function Orchestration(props: { page?: OrchestrationPage; server?: string; tab?: string }) {
  return <Switch fallback={<Overview />}>
    <Match when={props.page === "defaults"}><Coordinator /></Match>
    <Match when={props.page === "models"}><Models /></Match>
    <Match when={props.page === "routing"}><Routing tab={props.tab} /></Match>
    <Match when={props.page === "effective"}><RoutePreview /></Match>
    <Match when={props.page === "policy"}><Limits /></Match>
    <Match when={props.page === "tools"}><Tools server={props.server} tab={props.tab} /></Match>
    <Match when={props.page === "profiles"}><Profiles /></Match>
    <Match when={props.page === "files"}><Files /></Match>
  </Switch>;
}
