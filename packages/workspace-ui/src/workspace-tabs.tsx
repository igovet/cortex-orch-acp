import { createMemo, createRoot, createSignal, For, getOwner, onCleanup, untrack } from "solid-js";
import { I } from "./kit";
import { hrefOf, parseRoute, type Route, type Router } from "./router";
import { activeOf, addTab, closeTab, cycleTab, encodeTabs, hrefIn, isStoredTabs, jumpTab, moveTab, selectTab, startTabs, stepTab, tabOf, visitTab, type StoredTabs, type TabList } from "./tabs";
import { persisted } from "./theme";

const here = () => location.pathname + location.search;

/**
 * Every tab routes on its own; the window's address follows the active one. The address is only ever replaced, so the
 * webview's history stays out of the way of each tab's Back and Forward.
 */
export function createTabs() {
  const owner = getOwner();
  const [saved, save] = persisted<StoredTabs | null>("cw.ws.tabs", null, v => v === null || isStoredTabs(v));
  const [list, setList] = createSignal<TabList>(startTabs(saved(), here()));
  const publish = () => {
    const now = untrack(list), href = hrefIn(activeOf(now));
    if (here() !== href) history.replaceState(history.state, "", href);
    save(encodeTabs(now));
  };
  publish();
  const commit = (next: TabList) => { if (next !== untrack(list)) { setList(next); publish(); } };
  const current = () => untrack(list);
  const routers = new Map<number, { router: Router; dispose: () => void }>();
  /** One small reactive scope per tab, so a route change in one tab wakes nothing in another. */
  const routerOf = (id: number): Router => routers.get(id)?.router ?? createRoot(dispose => {
    const tab = createMemo(() => tabOf(list(), id));
    const href = createMemo<string>(prev => { const t = tab(); return t ? hrefIn(t) : prev; }, "/");
    const path = createMemo(() => href().split("?")[0]);
    const search = createMemo(() => { const at = href().indexOf("?"); return at < 0 ? "" : href().slice(at); });
    // Parsed once per address, not by every row that asks on every update.
    const route = createMemo(() => parseRoute(path(), search()));
    const router: Router = {
      path, search, route,
      canBack: createMemo(() => (tab()?.at ?? 0) > 0),
      canForward: createMemo(() => { const t = tab(); return !!t && t.at < t.entries.length - 1; }),
      back: () => commit(stepTab(current(), id, -1)), forward: () => commit(stepTab(current(), id, 1)),
      go(to: string | Route, opts: { replace?: boolean } = {}) { commit(visitTab(current(), id, typeof to === "string" ? to : hrefOf(to), !!opts.replace)); },
    };
    routers.set(id, { router, dispose });
    return router;
  }, owner);
  const activeId = createMemo(() => list().active);
  const active: Router = {
    path: () => routerOf(activeId()).path(), search: () => routerOf(activeId()).search(), route: () => routerOf(activeId()).route(),
    canBack: () => routerOf(activeId()).canBack(), canForward: () => routerOf(activeId()).canForward(),
    back: () => routerOf(activeId()).back(), forward: () => routerOf(activeId()).forward(),
    go: (to, opts) => routerOf(activeId()).go(to, opts),
  };
  // Nothing in the app adds history entries now: this is the address arriving from outside.
  const onPop = () => { if (here() !== hrefIn(activeOf(current()))) active.go(here()); };
  window.addEventListener("popstate", onPop);
  onCleanup(() => window.removeEventListener("popstate", onPop));
  return {
    ids: createMemo(() => list().tabs.map(t => t.id), undefined, { equals: (a, b) => a.length === b.length && a.every((id, i) => id === b[i]) }),
    activeId, active, routerOf,
    add: () => commit(addTab(current())),
    select: (id: number) => commit(selectTab(current(), id)),
    move: (id: number, to: number) => commit(moveTab(current(), id, to)),
    cycle: (by: number) => commit(cycleTab(current(), by)),
    jump: (n: number) => commit(jumpTab(current(), n)),
    close(id: number) {
      commit(closeTab(current(), id));
      routers.get(id)?.dispose();
      routers.delete(id);
    },
  };
}
export type Tabs = ReturnType<typeof createTabs>;

/** Ids belong to the screen on view: a parked tab keeps its own out of the document, so popovers and `for=` never reach into it. */
export function parkPane(pane: HTMLElement, parked: boolean) {
  for (const el of pane.querySelectorAll(parked ? "[id]" : "[data-parked-id]")) {
    if (parked) { el.setAttribute("data-parked-id", el.id); el.removeAttribute("id"); }
    else { el.id = el.getAttribute("data-parked-id")!; el.removeAttribute("data-parked-id"); }
  }
  if (parked) for (const pop of pane.querySelectorAll<HTMLElement>("[popover]")) { try { pop.hidePopover?.(); } catch { /* not open */ } }
}

export function TabStrip(props: { tabs: Tabs; title: (id: number) => string; onSelect: (id: number, keepFocus?: boolean) => void; onClose: (id: number) => void }) {
  const [dragged, setDragged] = createSignal<number | null>(null);
  const onArrow = (e: KeyboardEvent, id: number) => {
    const by = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!by) return;
    e.preventDefault();
    const ids = props.tabs.ids(), to = ids[(ids.indexOf(id) + by + ids.length) % ids.length];
    props.onSelect(to, true);
    queueMicrotask(() => document.getElementById(`ws-tab-${to}`)?.focus());
  };
  return <div class="ws-tabstrip" role="tablist" aria-label="Open tabs">
    <For each={props.tabs.ids()}>{(id, at) => {
      const on = () => props.tabs.activeId() === id;
      return <div class="ws-tab" data-on={on() ? "" : undefined} draggable={true}
        onDragStart={e => { setDragged(id); e.dataTransfer?.setData("application/x-cortex-tab", String(id)); }}
        onDragEnd={() => setDragged(null)}
        onDragOver={e => { if (dragged() !== null) e.preventDefault(); }}
        onDrop={e => { const from = dragged(); if (from === null) return; e.preventDefault(); setDragged(null); props.tabs.move(from, at()); }}
        onMouseDown={e => { if (e.button === 1) e.preventDefault(); }}
        on:auxclick={e => { if (e.button === 1) { e.preventDefault(); props.onClose(id); } }}>
        <button id={`ws-tab-${id}`} class="ws-tab-name" role="tab" aria-selected={on()} tabindex={on() ? 0 : -1} onClick={() => props.onSelect(id)} onKeyDown={e => onArrow(e, id)}><span class="trunc">{props.title(id)}</span></button>
        <button class="ws-tab-x" aria-label={`Close ${props.title(id)}`} tabindex={on() ? 0 : -1} onClick={() => props.onClose(id)}><I n="x" s={12} /></button>
      </div>;
    }}</For>
    <button class="ib ws-tab-new" aria-label="New tab" onClick={() => props.tabs.add()}><I n="plus" s={14} /></button>
  </div>;
}
