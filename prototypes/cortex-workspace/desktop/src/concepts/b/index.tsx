import { onCleanup, onMount, Show } from "solid-js";
import type { ConceptProps } from "../../store";
import "./b.css";
import { Overlays, PrototypeStrip, StatusBar } from "./chrome";
import { BottomPanel, Group1, Group2 } from "./groups";
import { createBModel, focusLater, type BModel } from "./model";
import { ActivityBar, SidePanel } from "./side";

/** Concept B: IDE with Mission Control. Code stays in group 1; orchestration opens beside it in group 2. */
export default function ConceptB(props: ConceptProps) {
  const m = createBModel(props.store);
  const onKey = (e: KeyboardEvent) => handleKey(m, e);
  onMount(() => {
    m.measureRoot();
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", m.measureRoot);
  });
  onCleanup(() => {
    window.removeEventListener("keydown", onKey);
    window.removeEventListener("resize", m.measureRoot);
    m.dispose();
  });
  const decisionsAnnouncement = () => m.ui.loading ? "" : `${m.needsYou().length} decision${m.needsYou().length === 1 ? "" : "s"} need you`;
  return <div class="cb-root" classList={{ "cb-no-side": !m.ui.sideVisible }}>
    <a class="skip-link cb-skip" href="#cb-g1" onClick={e => { e.preventDefault(); focusMain(); }}>Skip to main work area</a>
    <PrototypeStrip m={m} />
    <div class="cb-body">
      <ActivityBar m={m} />
      <Show when={m.ui.sideVisible}><SidePanel m={m} /></Show>
      <div class="cb-editors" classList={{ split: m.ui.split, "with-bottom": !m.ui.split && m.ui.bottomOpen }}>
        <Group1 m={m} />
        <Show when={m.ui.split}><Group2 m={m} /></Show>
        <Show when={!m.ui.split && m.ui.bottomOpen}><BottomPanel m={m} /></Show>
      </div>
    </div>
    <StatusBar m={m} />
    <div class="cb-sr" role="status" aria-live="polite">{decisionsAnnouncement()}</div>
    <Overlays m={m} />
  </div>;
}

function focusMain() {
  (document.querySelector<HTMLElement>("#cb-g1 textarea") ?? document.getElementById("cb-g1"))?.focus();
}

const REGIONS = (m: BModel) => ["cb-activity", m.ui.sideVisible ? "cb-side" : "", "cb-g1", m.ui.split ? "cb-g2" : m.ui.bottomOpen ? "cb-bottom" : "", "cb-status"].filter(Boolean);

function cycleRegion(m: BModel, dir: 1 | -1) {
  const regions = REGIONS(m);
  const current = regions.findIndex(id => document.getElementById(id)?.contains(document.activeElement));
  const next = regions[(current + dir + regions.length) % regions.length] ?? regions[0];
  focusLater(`#${next}`);
}

function handleKey(m: BModel, e: KeyboardEvent) {
  if (m.overlay() || m.confirm()) return;
  const t = e.target instanceof HTMLElement ? e.target : null;
  const typing = !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
  const ctrl = e.ctrlKey || e.metaKey;
  const group: 1 | 2 = t?.closest("#cb-g2") ? 2 : 1;
  const c = e.code;
  let handled = true;
  if (ctrl && e.shiftKey && c === "KeyM") m.showSide("mission");
  else if (ctrl && e.shiftKey && c === "KeyE") m.showSide("files");
  else if (ctrl && e.shiftKey && c === "KeyF") m.showSide("search");
  else if (ctrl && e.shiftKey && c === "KeyG") m.showSide("git");
  else if (ctrl && e.shiftKey && c === "KeyB") m.showSide("board");
  else if (ctrl && e.shiftKey && c === "KeyN") m.openNewTask();
  else if (ctrl && !e.shiftKey && c === "KeyP") m.setOverlay("files");
  else if (ctrl && !e.shiftKey && c === "KeyK") m.setOverlay("commands");
  else if (ctrl && !e.shiftKey && c === "KeyB") m.toggleSide();
  else if (ctrl && c === "Backslash") m.toggleSplit();
  else if (ctrl && c === "Backquote") m.openTerminal();
  else if (ctrl && !e.shiftKey && c === "KeyS") m.save(m.ui.g1Active);
  else if (ctrl && (e.key === "Tab" || e.key === "PageDown" || e.key === "PageUp")) m.cycleTab(group, e.key === "PageUp" || (e.key === "Tab" && e.shiftKey) ? -1 : 1);
  else if (ctrl && (c === "KeyW" || e.key === "F4")) m.closeActive(group);
  else if (ctrl && !e.shiftKey && c === "Digit1") focusMain();
  else if (ctrl && !e.shiftKey && c === "Digit2") { if (m.ui.split) focusLater("#cb-g2 [role=tab][aria-selected=true]"); }
  else if (e.altKey && !ctrl && c === "KeyD") m.focusNextDecision();
  else if (e.altKey && !ctrl && e.key === "ArrowLeft") m.back();
  else if (e.altKey && !ctrl && e.key === "ArrowRight") m.forward();
  else if (e.key === "F6") cycleRegion(m, e.shiftKey ? -1 : 1);
  else if (e.key === "F1" || (e.key === "?" && !typing && m.ui.charKeys)) m.setOverlay("keys");
  else handled = false;
  if (handled) { e.preventDefault(); e.stopPropagation(); }
}
