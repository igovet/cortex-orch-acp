import { createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import { useDesk } from "./ctx";
import { Icon } from "./icons";
import { AS_OF_SHORT, type Attention, type ProjectSummary } from "./model";
import type { ThemePref } from "./prefs";
import { A, IconBtn, Menu, Monogram, Popover, Skeleton } from "./ui";

type TreeItem =
  | { kind: "project"; ps: ProjectSummary; key: string }
  | { kind: "task"; ps: ProjectSummary; att: Attention; key: string }
  | { kind: "all"; ps: ProjectSummary; key: string }
  | { kind: "none"; ps: ProjectSummary; key: string }
  | { kind: "create"; ps: ProjectSummary; key: string };

export function projectSignal(ps: ProjectSummary): { text: string; tone: "danger" | "warn"; icon: "plugOff" | "gitMerge" | "xCircle" | null } | null {
  if (ps.disconnected) return { text: "Offline", tone: "danger", icon: "plugOff" };
  if (ps.conflict) return { text: "Conflict", tone: "danger", icon: "gitMerge" };
  if (ps.needsYou > 0) return { text: String(ps.needsYou), tone: "warn", icon: null };
  if (ps.failed > 0) return { text: String(ps.failed), tone: "danger", icon: "xCircle" };
  return null;
}
export function projectAccessibleName(ps: ProjectSummary) {
  const parts = [ps.project.name, `${ps.taskCount} tasks`];
  if (ps.disconnected) parts.push("disconnected", "worker state unknown");
  if (ps.conflict) parts.push("Git conflict");
  if (ps.needsYou) parts.push(`${ps.needsYou} needs you`);
  if (ps.failed) parts.push(`${ps.failed} failed check${ps.failed > 1 ? "s" : ""}`);
  return parts.join(", ");
}

export function Sidebar() {
  const desk = useDesk();
  const route = () => desk.router.route();
  return <nav class="sidebar" aria-label="Sidebar" id="sidebar-nav" tabindex="-1">
    <div class="sb-header"><span>Cortex Workspace</span></div>
    <Show when={route().name === "settings"} fallback={<MainNav />}><SettingsNav /></Show>
    <ResizeHandle />
    <SidebarFooter />
  </nav>;
}

/** 220–360px; drag or focus and press ←/→ for 8px steps. Width persists; the sidebar never collapses automatically. */
function ResizeHandle() {
  const desk = useDesk();
  const measured = () => document.getElementById("sidebar-nav")?.getBoundingClientRect().width || (desk.wide() ? 280 : 260);
  const width = () => desk.prefs.sidebarWidth() || Math.round(measured());
  const max = () => Math.max(360, Math.round(window.innerWidth * 0.4));
  const set = (w: number) => desk.prefs.setSidebarWidth(Math.round(Math.max(220, Math.min(max(), w))));
  let startX = 0, startW = 0, dragging = false;
  return <div class="resize-handle" role="separator" aria-orientation="vertical" aria-label="Resize sidebar" tabindex="0"
    aria-valuenow={width()} aria-valuemin={220} aria-valuemax={max()}
    onKeyDown={e => { if (e.key === "ArrowLeft") { e.preventDefault(); set(width() - 8); } if (e.key === "ArrowRight") { e.preventDefault(); set(width() + 8); } }}
    onPointerDown={e => { dragging = true; startX = e.clientX; startW = width(); (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId); }}
    onPointerMove={e => { if (dragging) set(startW + e.clientX - startX); }}
    onPointerUp={() => (dragging = false)} />;
}

function MainNav() {
  const desk = useDesk();
  const route = () => desk.router.route();
  const needs = () => desk.data.needs().length;
  return <>
    <ul class="sb-shortcuts" role="list">
      <li><button type="button" class="sb-row" onClick={() => desk.openCreate()} disabled={false}>
        <Icon name="plus" /><span class="sb-label">New task</span><kbd class="sb-hint">Ctrl Alt N</kbd></button></li>
      <li><button type="button" class="sb-row" onClick={() => desk.openPalette("all")}>
        <Icon name="search" /><span class="sb-label">Search</span><kbd class="sb-hint">Ctrl K</kbd></button></li>
      <li><A href="/needs-you" class="sb-row" aria-current={route().name === "needs" ? "page" : undefined}
        aria-label={desk.data.status() === "ready" ? `Needs you, ${needs()} open decision${needs() === 1 ? "" : "s"}${desk.data.offline() ? `, as of ${AS_OF_SHORT}` : ""}` : desk.data.status() === "error" ? "Needs you, count unavailable: couldn't load decisions" : "Needs you, loading"}>
        <Icon name="inbox" /><span class="sb-label">Needs you</span>
        <Switch>
          <Match when={desk.data.status() === "loading"}><Skeleton w="1rem" h="0.75rem" /></Match>
          <Match when={desk.data.status() === "error"}><span class="count-chip tone-neutral" title="Couldn't load decisions">?</span></Match>
          <Match when={needs() > 0}><span class="count-chip tone-warn">{needs()}</span></Match>
        </Switch>
      </A></li>
      <li><A href="/" class="sb-row" aria-current={route().name === "home" ? "page" : undefined}><Icon name="grid" /><span class="sb-label">All projects</span></A></li>
    </ul>
    <div class="sb-divider" />
    <div class="sb-group-label"><span id="sb-projects-label">Projects</span>
      <Popover label="Sort projects" place="below-end" trigger={p => <IconBtn {...p} icon="sort" label="Sort projects" class="sm" />}>
        {close => <Menu label="Sort projects" radio close={close} items={([["fixture", "Fixture order"], ["name", "Name"], ["activity", "Last activity"]] as const).map(([v, l]) => ({
          label: l, checked: desk.prefs.sort() === v, onSelect: () => desk.prefs.setSort(v),
        }))} />}
      </Popover>
    </div>
    <ProjectTree />
  </>;
}

function ProjectTree() {
  const desk = useDesk();
  const route = () => desk.router.route();
  const currentSlug = () => { const r = route(); return r.name === "project" || r.name === "task" ? r.slug : r.name === "settings" ? r.slug : undefined; };
  const sorted = createMemo(() => {
    const ps = [...desk.data.projects()];
    if (desk.prefs.sort() === "name") ps.sort((a, b) => a.project.name.localeCompare(b.project.name));
    if (desk.prefs.sort() === "activity") ps.sort((a, b) => (b.lastActivity ?? "").localeCompare(a.lastActivity ?? ""));
    return ps;
  });
  const items = createMemo<TreeItem[]>(() => {
    const out: TreeItem[] = [];
    for (const ps of sorted()) {
      out.push({ kind: "project", ps, key: ps.project.id });
      if (!desk.prefs.expanded().includes(ps.project.id) || desk.data.status() !== "ready") continue;
      if (ps.taskCount === 0) { out.push({ kind: "none", ps, key: `${ps.project.id}:none` }, { kind: "create", ps, key: `${ps.project.id}:create` }); continue; }
      const att = desk.data.attention(ps.project.id);
      if (!att.length) out.push({ kind: "none", ps, key: `${ps.project.id}:none` });
      att.forEach(a => out.push({ kind: "task", ps, att: a, key: a.tv.task.id }));
      out.push({ kind: "all", ps, key: `${ps.project.id}:all` });
    }
    return out;
  });
  const focusable = () => items().filter(i => i.kind !== "none");
  const [active, setActive] = createSignal<string>("");
  const activeKey = () => focusable().some(i => i.key === active()) ? active() : (focusable().find(i => i.kind === "project" && i.ps.slug === currentSlug())?.key ?? focusable()[0]?.key);
  let tree!: HTMLDivElement;
  const focusKey = (key: string) => { setActive(key); queueMicrotask(() => tree.querySelector<HTMLElement>(`[data-key="${key}"]`)?.focus()); };
  const activate = (item: TreeItem) => {
    if (item.kind === "project") desk.go(`/p/${item.ps.slug}`);
    else if (item.kind === "task") desk.go(`/p/${item.ps.slug}/t/${item.att.tv.num}/${item.att.tv.needsYou ? "decisions" : "discussion"}`);
    else if (item.kind === "all") desk.go(`/p/${item.ps.slug}/board`);
    else if (item.kind === "create") desk.openCreate(item.ps.project.id);
  };
  const onKey = (e: KeyboardEvent) => {
    const list = focusable(), i = list.findIndex(x => x.key === activeKey()), item = list[i];
    if (!item) return;
    const expanded = desk.prefs.expanded().includes(item.ps.project.id);
    let next: TreeItem | undefined;
    switch (e.key) {
      case "ArrowDown": next = list[Math.min(list.length - 1, i + 1)]; break;
      case "ArrowUp": next = list[Math.max(0, i - 1)]; break;
      case "Home": next = list[0]; break;
      case "End": next = list[list.length - 1]; break;
      case "ArrowRight":
        if (item.kind === "project" && !expanded) desk.prefs.toggleExpanded(item.ps.project.id);
        else if (item.kind === "project") next = list[i + 1];
        break;
      case "ArrowLeft":
        if (item.kind === "project" && expanded) desk.prefs.toggleExpanded(item.ps.project.id);
        else if (item.kind !== "project") next = list.find(x => x.kind === "project" && x.ps.project.id === item.ps.project.id);
        break;
      case "Enter": case " ": e.preventDefault(); activate(item); return;
      default: return;
    }
    e.preventDefault();
    if (next) focusKey(next.key);
  };
  return <div class="sb-tree" role="tree" aria-labelledby="sb-projects-label" ref={tree} onKeyDown={onKey}>
    <Show when={desk.data.status() !== "loading"} fallback={<For each={desk.data.projects()}>{() => <div class="sb-row skeleton-row" aria-hidden="true"><Skeleton w="1rem" h="1rem" /><Skeleton w="60%" /></div>}</For>}>
      <For each={sorted()}>{ps => {
        const expanded = () => desk.prefs.expanded().includes(ps.project.id);
        const children = () => items().filter(i => i.kind !== "project" && i.ps.project.id === ps.project.id);
        const sig = () => desk.data.status() === "ready" || ps.disconnected || ps.conflict ? projectSignal(ps) : null;
        return <div role="treeitem" class="tree-node" aria-expanded={expanded()} aria-level="1" data-key={ps.project.id}
          aria-label={projectAccessibleName(ps)} aria-current={currentSlug() === ps.slug ? "page" : undefined}
          tabindex={activeKey() === ps.project.id ? 0 : -1} onFocus={e => { if (e.target === e.currentTarget) setActive(ps.project.id); }}>
          <div class={`sb-row project-row ${currentSlug() === ps.slug ? "is-selected" : ""}`} onClick={() => { setActive(ps.project.id); activate({ kind: "project", ps, key: ps.project.id }); }} title={ps.project.name}>
            <button type="button" class="tree-chevron" tabindex="-1" aria-hidden="true" onClick={e => { e.stopPropagation(); desk.prefs.toggleExpanded(ps.project.id); }}>
              <Icon name={expanded() ? "chevronDown" : "chevronRight"} size={12} />
            </button>
            <Monogram name={ps.project.name} index={ps.index} />
            <span class="sb-label">{ps.project.name}</span>
            <Show when={sig()}>{s => <span class={`sb-signal tone-${s().tone} ${s().icon ? "" : "count-chip"}`} aria-hidden="true">
              <Show when={s().icon}><Icon name={s().icon!} size={14} /></Show>{s().text}</span>}</Show>
          </div>
          <Show when={expanded() && children().length}>
            <div role="group">
              <For each={children()}>{item => <Show when={item.kind !== "none"} fallback={<div class="sb-row nested muted" role="none">{item.ps.taskCount === 0 ? "No tasks yet" : "Nothing needs attention"}</div>}>
                <div role="treeitem" aria-level="2" class="sb-row nested" data-key={item.key} tabindex={activeKey() === item.key ? 0 : -1}
                  aria-current={item.kind === "task" && route().name === "task" && (route() as { num: string }).num === item.att.tv.num ? "page" : undefined}
                  onFocus={() => setActive(item.key)} onClick={e => { e.stopPropagation(); setActive(item.key); activate(item); }}>
                  <Switch>
                    <Match when={item.kind === "task" ? item : undefined}>{t => <>
                      <span class={`status-dot tone-${t().att.tone}`} aria-hidden="true" />
                      <span class="sb-label">{t().att.tv.task.title}</span>
                      <span class="sb-status">{t().att.label}</span>
                    </>}</Match>
                    <Match when={item.kind === "all"}><span class="sb-label">All {item.ps.taskCount} tasks</span><Icon name="arrowRight" size={14} /></Match>
                    <Match when={item.kind === "create"}><Icon name="plus" size={14} /><span class="sb-label">New task in {item.ps.project.name}</span></Match>
                  </Switch>
                </div>
              </Show>}</For>
            </div>
          </Show>
        </div>;
      }}</For>
    </Show>
  </div>;
}

function SettingsNav() {
  const desk = useDesk();
  const route = () => desk.router.route();
  const is = (section: string, slug?: string) => { const r = route(); return r.name === "settings" && r.section === section && (!slug || r.slug === slug); };
  return <div class="sb-settings">
    <button type="button" class="sb-row" onClick={() => desk.go(sessionStorage.getItem("cw.beforeSettings") ?? "/")}><Icon name="arrowLeft" /><span class="sb-label">Back</span></button>
    <div class="sb-group-label"><span>App</span></div>
    <ul role="list" class="sb-shortcuts">
      <li><A class="sb-row" href="/settings/appearance" aria-current={is("appearance") ? "page" : undefined}><Icon name="sun" /><span class="sb-label">Appearance</span></A></li>
      <li><A class="sb-row" href="/settings/keyboard" aria-current={is("keyboard") ? "page" : undefined}><Icon name="keyboard" /><span class="sb-label">Keyboard</span></A></li>
    </ul>
    <div class="sb-group-label"><span>Projects</span></div>
    <ul role="list" class="sb-shortcuts sb-scroll">
      <For each={desk.data.projects()}>{ps => <li><A class="sb-row" href={`/settings/projects/${ps.slug}`} aria-current={is("project", ps.slug) ? "page" : undefined}
        aria-label={`${ps.project.name} connection${ps.disconnected ? ", disconnected" : ""}`}>
        <Monogram name={ps.project.name} index={ps.index} /><span class="sb-label">{ps.project.name}</span>
        <Show when={ps.disconnected}><span class="sb-signal tone-danger" aria-hidden="true"><Icon name="plugOff" size={14} />Disconnected</span></Show>
      </A></li>}</For>
    </ul>
    <div class="sb-group-label"><span>Prototype</span></div>
    <ul role="list" class="sb-shortcuts">
      <li><A class="sb-row" href="/settings/about" aria-current={is("about") ? "page" : undefined}><Icon name="flask" /><span class="sb-label">About this mock</span></A></li>
    </ul>
  </div>;
}

function SidebarFooter() {
  const desk = useDesk();
  const t = desk.data.totals;
  const anyDown = () => t().connected < t().projects;
  const themeIcon = () => desk.theme.pref() === "system" ? "monitor" : desk.theme.pref() === "light" ? "sun" : "moon";
  // "Device offline" (this computer) is distinct from a project's disconnected machine ("Offline" signal in the tree).
  const connText = () => desk.data.offline() ? "Device offline" : `${t().connected} of ${t().projects} connected`;
  return <div class="sb-footer">
    <Popover label="Connections" kind="dialog" place="above-start" width="20rem" class="sb-conn-anchor" trigger={p => <button type="button" {...p} class="sb-conn"
      title={desk.data.offline() ? `This device is offline; data as of ${AS_OF_SHORT} (fixture)` : connText()}
      aria-label={`Connections: ${t().connected} of ${t().projects} connected${desk.data.offline() ? `; this device is offline, showing data as of ${AS_OF_SHORT}` : ""}. Open connection list`}>
      <Icon name={desk.data.offline() ? "plugZap" : anyDown() ? "plugOff" : "plug"} class={desk.data.offline() || anyDown() ? "tone-danger-fg" : ""} />
      <span>{connText()}</span>
    </button>}>
      {close => <div class="conn-pop">
        <h2 class="pop-title">Machines</h2>
        <Show when={desk.data.offline()}><p class="pop-note">This device is offline. States are as of {AS_OF_SHORT} (fixture); actions are unavailable.</p></Show>
        <ul role="list">
          <For each={desk.data.projects()}>{ps => <li>
            <A href={`/p/${ps.slug}/machine`} class="pop-row" onClick={() => close(false)}>
              <Icon name={ps.disconnected ? "plugOff" : "plug"} class={ps.disconnected ? "tone-danger-fg" : ""} />
              <span class="pop-row-main"><span>{ps.machineName}</span><span class="muted-xs">{ps.project.name}</span></span>
              <span class={ps.disconnected ? "tone-danger-fg" : "muted-xs"}>{ps.disconnected ? "Disconnected" : "Connected"}</span>
            </A>
          </li>}</For>
        </ul>
      </div>}
    </Popover>
    <div class="sb-footer-icons">
      <Popover label="Theme" place="above-end" trigger={p => <IconBtn {...p} icon={themeIcon()} label={desk.theme.label()} />}>
        {close => <Menu label="Theme" radio close={close} items={([["system", `System (${desk.theme.systemDark() ? "Dark" : "Light"})`], ["light", "Light"], ["dark", "Dark"]] as [ThemePref, string][]).map(([v, l]) => ({
          label: l, checked: desk.theme.pref() === v, onSelect: () => desk.theme.setPref(v),
        }))} />}
      </Popover>
      <IconBtn icon="settings" label="Settings (Ctrl+,)" onClick={() => desk.go("/settings/appearance")} />
      <IconBtn icon="help" label="Keyboard shortcuts and help (?)" onClick={() => desk.openHelp()} />
    </div>
  </div>;
}
