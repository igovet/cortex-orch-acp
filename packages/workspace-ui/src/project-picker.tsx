import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "solid-js";
import { FEATURES, type DirectorySearch } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { CloneProject } from "./maintenance";
import { I, Modal } from "./kit";
import { folderName, folderNameProblem, joinPath, matchesPath, moveActive, pickerPaths, shortenPath } from "./project-picker-model";

type Page = { kind: "clone" } | { kind: "method" } | { kind: "search" } | { kind: "parent" } | { kind: "name"; parent: string };
type Row = { id: string; title: string; subtitle?: string | null; icon: string; git?: boolean; select: () => void };
const DEBOUNCE_MS = 250, RESULT_LIMIT = 30;
const TITLES: Record<Page["kind"], string> = { method: "Add project", clone: "Clone from GitHub", search: "Search for folder", parent: "Choose parent folder", name: "Name folder" };
const HINTS: Partial<Record<Page["kind"], string>> = { search: "Search folders or enter a path...", parent: "Search parent folders or enter a path...", name: "Folder name" };

/**
 * Adding a project works as it does in Paseo: one list of ways in, then a field that searches the selected machine as you
 * type. The machine does the searching, so this stays true for a machine that is not this one. Choosing a folder adds it.
 */
export function ProjectPicker(props: { open: boolean; onClose: () => void }) {
  const ws = useWs();
  const [pages, setPages] = createSignal<Page[]>([{ kind: "method" }]);
  const [text, setText] = createSignal("");
  const [active, setActive] = createSignal(0);
  const [reply, setReply] = createSignal<DirectorySearch | null>(null);
  const [searching, setSearching] = createSignal(false);
  const [failed, setFailed] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [missing, setMissing] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [noSearch, setNoSearch] = createSignal(false);
  let input: HTMLInputElement | undefined, list: HTMLDivElement | undefined;
  let session = "", turn = 0, timer: ReturnType<typeof setTimeout> | undefined;

  const page = () => pages()[pages().length - 1];
  const machine = () => ws.machine();
  const searches = () => page().kind === "search" || page().kind === "parent";
  const home = () => reply()?.home ?? null;
  const recent = () => ws.store.state().projects.filter(p => p.machineId === machine().id && p.repository).map(p => p.repository);
  const canSearch = () => ws.store.can(FEATURES.searchFolders) && !noSearch();

  const forget = () => {
    clearTimeout(timer);
    turn += 1;
    if (session && searching()) void ws.store.cancelDirectorySearch(machine().id, session);
    setSearching(false);
  };
  const clearNotes = () => { setError(null); setMissing(null); };
  createEffect(on(() => props.open, open => {
    forget();
    if (!open) return;
    session = `pick-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
    setPages([{ kind: "method" }]); setText(""); setActive(0); setReply(null); setFailed(null); clearNotes(); setBusy(null);
  }));
  onCleanup(forget);
  const go = (next: Page) => { enter(); setPages(stack => [...stack, next]); };
  const back = () => { if (pages().length === 1) { props.onClose(); return; } enter(); setPages(stack => stack.slice(0, -1)); };
  const enter = () => { forget(); setText(""); setActive(0); setFailed(null); clearNotes(); };

  createEffect(on(() => page().kind, () => queueMicrotask(() => (input ?? list)?.focus())));
  /** Every change waits a moment, so one search answers a burst of typing, and a blank field asks only for the home. */
  createEffect(on([text, searches], ([query, on]) => {
    clearTimeout(timer);
    if (!on) return;
    const mine = ++turn;
    if (!canSearch()) { setSearching(false); return; }
    setSearching(query.trim().length > 0);
    timer = setTimeout(() => {
      void ws.store.searchDirectories({ machineId: machine().id, session, query, limit: RESULT_LIMIT }).then(result => {
        if (mine !== turn || result?.cancelled) return;
        if (result) setReply(result); else setNoSearch(true);
        setFailed(null); setSearching(false);
      }, (reason: unknown) => {
        if (mine !== turn) return;
        setFailed(reason instanceof Error ? reason.message : "Unable to search folders"); setSearching(false);
      });
    }, query.trim() ? DEBOUNCE_MS : 0);
  }));

  const add = (path: string, how: { createFolder?: boolean; newFolder?: boolean } = {}) => {
    if (busy()) return;
    clearNotes();
    setBusy(how.newFolder || how.createFolder ? "Creating folder..." : "Adding project...");
    const real = ws.store.mode === "real", id = ws.newId("project");
    void ws.store.dispatch({ type: "add_project", id, machineId: machine().id, name: folderName(path, home()), repository: path, openBranch: real ? "" : "main", defaultBranch: real ? "" : "main", ...how }, { quiet: true }).then(receipt => {
      setBusy(null);
      if (receipt.ok) {
        props.onClose();
        ws.go({ name: "project", machine: machine().id, project: receipt.entityId ?? id, view: "board", scope: { kind: "all" } });
        return;
      }
      setError(receipt.message);
      if (receipt.outcome === "project_folder_missing") setMissing(path);
      setActive(0);
    });
  };
  const browse = () => {
    if (busy()) return;
    clearNotes();
    setBusy("Choosing folder...");
    void ws.store.pickFolder().then(path => { setBusy(null); if (path) add(path); }, () => { setBusy(null); setError("Unable to open the folder chooser."); });
  };
  const nameRow = (parent: string): Row[] => {
    const name = text().trim();
    if (folderNameProblem(name)) return [];
    const path = joinPath(parent, name);
    return [{ id: "create", title: shortenPath(path, home()), subtitle: "Create this folder", icon: "folder-plus", select: () => add(path, { newFolder: true }) }];
  };
  const pathRows = (): Row[] => {
    const query = text(), held = reply();
    // Rows that still fit stay on screen while the machine answers, so the list narrows instead of blinking.
    const found = !held || !query.trim() ? [] : held.query === query ? held.entries : held.entries.filter(entry => matchesPath(query, entry.path, home()));
    const parent = page().kind === "parent";
    return pickerPaths({ query, recent: recent(), found, home: home() }).map(row => {
      const title = shortenPath(row.path, home());
      return {
        id: row.path, title, git: row.git, icon: "folder",
        subtitle: row.literal ? (parent ? "Use this parent" : "Open this path") : title === row.path ? null : row.path,
        select: () => { if (parent) go({ kind: "name", parent: row.path }); else add(row.path); },
      };
    });
  };
  const rows = createMemo((): Row[] => {
    const current = page();
    if (current.kind === "method") return [
      { id: "search", title: "Search for folder", subtitle: `Find a folder on ${machine().name}`, icon: "search", select: () => go({ kind: "search" }) },
      { id: "clone", title: "Clone from GitHub", subtitle: `Clone onto ${machine().name}`, icon: "github", select: () => go({ kind: "clone" }) },
      ...(ws.store.can(FEATURES.pickFolder) && machine().kind === "local" ? [{ id: "browse", title: "Browse", subtitle: "Choose or create a folder with the system chooser", icon: "folder", select: browse }] : []),
      { id: "new", title: "New folder", subtitle: `Create an empty folder on ${machine().name}`, icon: "folder-plus", select: () => go({ kind: "parent" }) },
    ];
    if (current.kind === "clone") return [];
    if (current.kind === "name") return nameRow(current.parent);
    const path = missing();
    return [...(path ? [{ id: "create-missing", title: "Create this folder", subtitle: shortenPath(path, home()), icon: "folder-plus", select: () => add(path, { createFolder: true }) }] : []), ...pathRows()];
  });
  // New rows never move a choice the owner made: it stays on the same folder, or starts again when that one is gone.
  let chosen: string | undefined, moved = false;
  createEffect(on(rows, now => { const at = moved && chosen !== undefined ? now.findIndex(row => row.id === chosen) : -1; setActive(at < 0 ? 0 : at); }));
  createEffect(() => { chosen = rows()[active()]?.id; });
  createEffect(on(active, index => document.getElementById(`ws-pick-${index}`)?.scrollIntoView?.({ block: "nearest" })));
  // The field is off while the machine answers, and a refusal hands it back for the correction.
  createEffect(on(busy, now => { if (!now) queueMicrotask(() => input?.focus()); }, { defer: true }));

  const choose = () => {
    if (busy()) return;
    const current = page();
    if (current.kind === "name") {
      const problem = folderNameProblem(text());
      if (problem) { setError(problem); return; }
    }
    rows()[active()]?.select();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      moved = true;
      setActive(at => moveActive(at, rows().map(() => true), e.key === "ArrowDown" ? 1 : -1));
    } else if (e.key === "Enter" && !(e.target as HTMLElement).closest("button")) { e.preventDefault(); choose(); }
  };
  const emptyText = () => {
    if (page().kind === "name" || failed() || searching()) return "";
    if (!canSearch()) return "Enter the full path of a folder.";
    return text().trim() ? "No matching folders" : "Type a folder name, or a path like ~/projects.";
  };
  const status = () => busy() || (searching() ? "Searching..." : reply() && !reply()!.complete && text().trim() && !failed() ? "Stopped early. Type more of the path." : "");

  return <Modal open={props.open} onClose={back} onDismiss={props.onClose} label="Add a project" class="ws-palette ws-picker wide">
    <div class="ws-picker-flow" onKeyDown={onKey}>
      <div class="ws-picker-t">
        <Show when={pages().length > 1}><button class="ib" aria-label="Back" onClick={back}><I n="arrow-left" s={14} /></button></Show>
        <h2>{TITLES[page().kind]}</h2><span class="muted trunc">{machine().name}</span>
      </div>
      <Show when={page().kind === "clone"}><CloneProject added={path => add(path)} /></Show>
      <Show when={page().kind !== "method" && page().kind !== "clone"}>
        <div class="ws-pal-h">
          <input ref={el => { input = el; }} class="ws-pal-in" role="combobox" aria-expanded="true" aria-controls="ws-pick-list" aria-activedescendant={rows().length ? `ws-pick-${active()}` : undefined}
            aria-label={TITLES[page().kind]} placeholder={HINTS[page().kind]} spellcheck={false} autocomplete="off" disabled={!!busy()}
            value={text()} onInput={e => { moved = false; setText(e.currentTarget.value); setActive(0); clearNotes(); }} />
        </div>
      </Show>
      <div id="ws-pick-list" ref={el => { list = el; }} class="ws-pal-b" role="listbox" aria-label="Folders" aria-busy={!!busy()} tabindex="-1" data-autofocus
        aria-activedescendant={page().kind === "method" && rows().length ? `ws-pick-${active()}` : undefined}>
        <Show when={!busy()} fallback={<p class="ws-pal-none">{busy()}</p>}>
          <Show when={error()}><p class="err ws-picker-note" role="alert">{error()}</p></Show>
          <Show when={failed()}><p class="err ws-picker-note" role="alert">{failed()}</p></Show>
          <For each={rows()}>{(row, i) =>
            <div id={`ws-pick-${i()}`} class="mi ws-pal-o" role="option" aria-selected={active() === i()} onMouseMove={() => { moved = true; setActive(i()); }} onClick={() => { setActive(i()); row.select(); }}>
              <I n={row.icon} /><span class="grow trunc">{row.title}<Show when={row.subtitle}><span class="sub trunc">{row.subtitle}</span></Show></span>
              <Show when={row.git}><span class="kbd">Git repository</span></Show>
            </div>}</For>
          <Show when={page().kind !== "clone" && rows().length === 0 && emptyText()}><p class="ws-pal-none">{emptyText()}</p></Show>
          <Show when={page().kind === "method" && ws.t("addProjectNote")}><p class="ws-pal-none">{ws.t("addProjectNote")}</p></Show>
        </Show>
      </div>
      <div class="ws-picker-f">
        <span><span class="kbd">Up</span><span class="kbd">Down</span>Navigate</span>
        <span><span class="kbd">Enter</span>Select</span>
        <span><span class="kbd">Esc</span>{pages().length > 1 ? "Back" : "Close"}</span>
        <span class="ws-picker-s" role="status">{status()}</span>
      </div>
    </div>
  </Modal>;
}
