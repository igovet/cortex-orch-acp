import { createMemo, For, Show } from "solid-js";
import { FIXTURE_VERSION, SCENARIOS, SCENARIO_DETAILS, type Scenario } from "../fixtures";
import { SCENARIO_SCRIPTS } from "../scenarios";
import { useDesk } from "./ctx";
import { Icon } from "./icons";
import { A, Btn, IconBtn, Menu, Popover, SelectMenu } from "./ui";

const SCENARIO_LABEL: Record<Scenario, string> = { default: "Default", blocked: "Blocked", stale: "Stale", offline: "Offline", disconnected: "Disconnected", conflict: "Conflict", empty: "Empty", error: "Error" };

export function Breadcrumb() {
  const desk = useDesk();
  const crumbs = createMemo<{ label: string; href?: string }[]>(() => {
    const r = desk.router.route();
    const ps = r.name === "project" || r.name === "task" || (r.name === "settings" && r.slug) ? desk.data.projectBySlug().get((r as { slug: string }).slug) : undefined;
    switch (r.name) {
      case "home": return [{ label: "All projects" }];
      case "needs": return [{ label: "All projects", href: "/" }, { label: "Needs you" }];
      case "project": return [{ label: "All projects", href: "/" }, { label: ps?.project.name ?? "Unknown project" }];
      case "task": {
        const tv = desk.data.taskByNum().get(r.num);
        return [{ label: "All projects", href: "/" }, { label: ps?.project.name ?? "Unknown project", href: `/p/${r.slug}` },
          { label: tv ? `${tv.ref} ${tv.task.title}` : `#${r.num}` }];
      }
      case "settings": return [{ label: "Settings", href: "/settings/appearance" }, { label: r.section === "project" ? (ps?.project.name ?? "Project") : r.section === "about" ? "About this mock" : r.section[0].toUpperCase() + r.section.slice(1) }];
      default: return [{ label: "Not found" }];
    }
  });
  return <nav class="breadcrumb" aria-label="Breadcrumb">
    <ol>
      <For each={crumbs()}>{(c, i) => <li>
        <Show when={i() > 0}><span class="crumb-sep" aria-hidden="true">/</span></Show>
        <Show when={c.href && i() < crumbs().length - 1} fallback={<span class="crumb-current" aria-current="page" title={c.label}>{c.label}</span>}>
          <A href={c.href!} class="crumb-link">{c.label}</A>
        </Show>
      </li>}</For>
    </ol>
  </nav>;
}

export function TitleBand() {
  const desk = useDesk();
  return <header class="titleband">
    <IconBtn icon="panelLeft" label={desk.prefs.sidebarHidden() ? "Show sidebar (Ctrl+B)" : "Hide sidebar (Ctrl+B)"} aria-pressed={!desk.prefs.sidebarHidden()}
      onClick={() => desk.prefs.setSidebarHidden(!desk.prefs.sidebarHidden())} />
    <Breadcrumb />
    <div class="tb-right">
      <button type="button" class="search-trigger" onClick={() => desk.openPalette("all")} aria-label="Search and commands (Ctrl+K)">
        <Icon name="search" size={14} /><span class="search-ph">Search</span><kbd class="kbd-hint">Ctrl K</kbd>
      </button>
      <MockChip />
      <Popover label="More" place="below-end" trigger={p => <IconBtn {...p} icon="more" label="More" />}>
        {close => <Menu label="More" close={close} items={[
          { label: "Reset prototype", icon: "refresh", onSelect: () => desk.reset() },
          { label: "Keyboard shortcuts", icon: "keyboard", hint: "?", onSelect: () => desk.openHelp() },
          { label: "About this mock", icon: "info", onSelect: () => desk.go("/settings/about") },
          { label: "Archived exploration B", icon: "history", section: "Round one (archived)", onSelect: () => { location.href = "/archive/b"; } },
          { label: "Archived exploration C", icon: "history", section: "Round one (archived)", onSelect: () => { location.href = "/archive/c"; } },
        ]} />}
      </Popover>
    </div>
  </header>;
}

/** The single, quiet mock indicator. Visible on every route; opens the prototype controls. */
export function MockChip() {
  const desk = useDesk();
  const script = SCENARIO_SCRIPTS[0];
  const scenario = () => desk.store.state().scenario;
  const detail = () => SCENARIO_DETAILS[scenario()];
  const scenarioTask = () => {
    const id = detail().taskId;
    return id ? desk.data.tasks().find(t => t.task.id === id) : undefined;
  };
  const step = () => script.steps[desk.local().step % script.steps.length];
  const question = () => desk.store.state().fixture.decisions.find(d => d.kind === "question");
  return <Popover label="Prototype controls" kind="dialog" place="below-end" width="20rem" trigger={p =>
    <button type="button" {...p} class="mock-chip" aria-label={`Prototype: mock data, fixture ${FIXTURE_VERSION}. Open prototype controls`}>
      <Icon name="flask" size={14} /><span class="mock-word">Mock</span><span class="mock-rev">· fixture v2</span>
    </button>}>
    {close => <div class="proto-pop">
      <h2 class="pop-title">Prototype</h2>
      <span class="field-label" id="proto-scenario-label">Scenario</span>
      <SelectMenu id="proto-scenario" labelId="proto-scenario-label" label="Scenario" value={scenario()} autofocus
        options={SCENARIOS.map(s => ({ value: s, label: SCENARIO_LABEL[s] }))} onChange={s => desk.store.selectScenario(s as Scenario)} />
      <p class="proto-guide"><strong>{detail().title}.</strong> {detail().message}</p>
      <Show when={scenarioTask()}>{tv => <A class="inline-link" href={`/p/${tv().slug}/t/${tv().num}/${tv().needsYou ? "decisions" : tv().stale ? "result" : tv().conflict ? "git" : "discussion"}`}
        onClick={() => close(false)}>Open {tv().project.name} · {tv().ref}</A>}</Show>
      <label class="check-row"><input type="checkbox" checked={desk.local().loading} onChange={e => desk.setLocal(l => ({ ...l, loading: e.currentTarget.checked }))} /> Simulate loading</label>
      <label class="check-row"><input type="checkbox" checked={desk.local().diskChanged} onChange={e => desk.setLocal(l => ({ ...l, diskChanged: e.currentTarget.checked }))} /> Simulate a file change on disk (Code tab)</label>
      <div class="proto-step">
        <div class="muted-xs">Step {desk.local().step % script.steps.length + 1} of {script.steps.length} (fixture)</div>
        <p>{step().action} · {step().target}: {step().expected}</p>
        <div class="btn-row">
          <Btn onClick={() => desk.setLocal(l => ({ ...l, step: l.step + 1 }))}>Next step</Btn>
          <Btn onClick={() => { desk.reset(); close(); }}>Reset</Btn>
        </div>
      </div>
      <Btn icon="phone" mock disabled={!question() || question()!.id in desk.store.state().responses} onClick={() => {
        const q = question();
        if (!q) return;
        desk.store.decide(q.id, "Compact");
        desk.setLocal(l => ({ ...l, elsewhere: { ...l.elsewhere, [q.id]: { answer: "Compact", at: "14:05" } }, changes: [...l.changes, { text: `Answered on Phone · ${q.title} · 14:05 (mock)`, at: "14:05" }] }));
        desk.announce("Orbit Ledger question answered on Phone (mock)");
      }}>Simulate answer on phone</Btn>
      <p class="proto-disclaimer">All actions change only in-memory state. Fixture data, not runtime evidence.</p>
    </div>}
  </Popover>;
}

