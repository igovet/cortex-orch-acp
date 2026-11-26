import { createContext, createEffect, createMemo, createSignal, onCleanup, onMount, Show, useContext, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { WsContext, type Ws } from "./ctx";
import { keepInside, place, placeFocus, Pop } from "./kit";
import { writeClipboard } from "./message-copy";
import { checkoutFiles, repoOf, taskCheckout } from "./review-model";

export type ResultTarget = { kind: "url"; url: string } | { kind: "file"; path: string; line?: number; column?: number };
export const ResultScope = createContext<() => { projectId: string; taskId?: string; projectCode?: boolean }>();

type FileReference = { path: string; line?: number; column?: number; /** A bare root-level file name: a link only when the checkout has that file. */ tentative?: boolean };
const controls = /[\u0000-\u001f\u007f]/;
const scheme = /^[a-z][a-z\d+.-]*:/i;

/** A numeric line suffix is not a URI scheme. Keep prose detection stricter than an explicit Markdown destination. */
export function fileReference(value: string, explicit = false): FileReference | undefined {
  const match = /^(.+?)(?::(\d+)(?::(\d+))?|#L(\d+))?$/.exec(value);
  if (!match) return;
  const [, path, line, column, fragment] = match;
  if (scheme.test(path) || path.startsWith("//") || path.endsWith("/") || controls.test(path) || /[\\<>]/.test(path)) return;
  if (!explicit && !(/^(?:\/|\.\.?\/)/.test(path) || (path.includes("/") && /\.[\p{L}\d_-]+$/u.test(path)) || ((line || fragment) && /^[\p{L}\d_./ -]+$/u.test(path)))) return;
  const positive = (n?: string) => n && Number.isSafeInteger(Number(n)) && Number(n) > 0 ? Number(n) : undefined;
  return { path, line: positive(line ?? fragment), column: positive(column) };
}

/** A single-segment name in a code span (`README.md`, `Makefile`). It becomes a link only when the checkout lists it. */
export function bareFileReference(value: string): FileReference | undefined {
  if (value.length > 100 || !(/^[\p{L}\d_-][\p{L}\d_.-]*\.[\p{L}\d_-]{1,10}$/u.test(value) || /^(?:Makefile|Dockerfile|LICENSE)$/.test(value))) return;
  return { path: value, tentative: true };
}

/** Ubuntu checkout paths are POSIX paths. This only classifies a target; the service still owns Code's file access. */
function absolutePath(root: string, path: string) {
  const parts: string[] = [];
  for (const part of (path.startsWith("/") ? path : `${root}/${path}`).split("/")) {
    if (part === "..") parts.pop();
    else if (part && part !== ".") parts.push(part);
  }
  return "/" + parts.join("/");
}

export function goToCode(ws: Ws, projectId: string, checkoutId: string, taskId?: string, file?: string, line?: number) {
  const machine = ws.machine().id;
  if (taskId) ws.go({ name: "task", machine, task: taskId, tab: "code", file, line });
  else ws.go({ name: "project", machine, project: projectId, view: "code", scope: { kind: "all" }, checkout: checkoutId, file, line });
}

// Match the desktop opener's artifact types; these are opened as files, not read into Code.
const mediaArtifact = /\.(?:mp4|m4v|webm|ogv|mov|mkv|avi|srt|vtt|ass|ssa)$/i;
const ARTIFACT_CHECKOUT_WAIT_MS = 5000;
type PathInfo = { path: string; notice?: string; artifact?: boolean; resolving?: boolean };
function PathPopover(props: { trigger: HTMLAnchorElement; info: PathInfo; open?: () => Promise<void>; retry?: () => void; openLabel: string; close: () => void }) {
  let panel!: HTMLDivElement;
  let copyButton!: HTMLButtonElement;
  const [status, setStatus] = createSignal("");
  const [opening, setOpening] = createSignal(false);
  const copy = async () => setStatus(await writeClipboard(props.info.path) ? "Path copied." : "Could not copy the path. Select and copy it above.");
  const open = async () => {
    if (!props.open || opening()) return;
    setOpening(true);
    setStatus("");
    try { await props.open(); props.close(); }
    catch (error) { setStatus(typeof error === "string" || error instanceof Error ? String(error) : "Could not open the file."); }
    finally { setOpening(false); }
  };
  const position = () => {
    place(panel, props.trigger, "left");
    keepInside(panel);
  };
  onMount(() => {
    const toggle = (e: Event) => { if ((e as ToggleEvent).newState === "closed") props.close(); };
    const outside = (e: PointerEvent) => {
      if (e.target instanceof Node && !panel.contains(e.target) && !props.trigger.contains(e.target)) props.close();
    };
    panel.addEventListener("toggle", toggle);
    if (!panel.showPopover) document.addEventListener("pointerdown", outside);
    window.addEventListener("resize", position);
    onCleanup(() => {
      panel.removeEventListener("toggle", toggle);
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("resize", position);
    });
    panel.showPopover?.();
    placeFocus(copyButton, { preventScroll: true });
  });
  createEffect(() => {
    // The same dialog grows or shrinks as its path and actions arrive.
    props.info.path; props.info.notice; props.open; props.retry; status();
    position();
  });
  return <Portal><Pop id={`${props.trigger.id}-details`} class="ws-path-pop" role="dialog" label="File path" ref={el => { panel = el; }}>
    <div onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); props.close(); } }}>
      <div class="mono ws-path-value">{props.info.path}</div>
      <Show when={props.info.notice}><div class="muted" aria-live="polite">{props.info.notice}</div></Show>
      <div class="ws-path-actions">
        <button ref={el => { copyButton = el; }} class="btn btn-sm" type="button" onClick={() => void copy()}>Copy path</button>
        <Show when={props.retry}>{retry => {
          let button!: HTMLButtonElement;
          onCleanup(() => {
            if (document.activeElement === button) placeFocus(copyButton, { preventScroll: true });
          });
          return <button ref={el => { button = el; }} class="btn btn-sm" type="button" onClick={() => retry()()}>Retry</button>;
        }}</Show>
        <Show when={props.open}><button class="btn btn-sm" type="button" aria-disabled={opening() || undefined} data-why={opening() ? "Waiting for the external app." : undefined} onClick={() => void open()}>{opening() ? "Opening…" : props.openLabel}</button></Show>
      </div>
      <Show when={status()}><div role="status">{status()}</div></Show>
    </div>
  </Pop></Portal>;
}

/** No model-written URL is used as a webview navigation. Hand off only from a direct activation. */
export function ResultLink(props: { href: string; reference?: FileReference; class?: string; inert?: boolean; children: JSX.Element }) {
  const ws = useContext(WsContext), scope = useContext(ResultScope);
  const [info, setInfo] = createSignal<PathInfo>();
  const [relativeArtifact, setRelativeArtifact] = createSignal<{ path: string; projectId: string; taskId?: string }>();
  const [checkoutTimedOut, setCheckoutTimedOut] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [status, setStatus] = createSignal("");
  let active = true;
  let trigger!: HTMLAnchorElement;
  const id = () => ws?.unique();
  // A link and its popover keep one shared identity for the life of the link.
  const linkId = ws && scope && props.reference ? `result-path-${id()}` : undefined;
  onCleanup(() => { active = false; });
  const artifactInfo = (path: string): PathInfo => ({
    path, artifact: true, notice: !ws?.openResultTarget || !ws.can("workspace.open_editor") ? "Opening in a desktop app is unavailable." : undefined,
  });
  const resolvedArtifactPath = createMemo(() => {
    const request = relativeArtifact();
    if (!ws || !request) return;
    const { path, projectId, taskId } = request, state = ws.store.state();
    const task = state.tasks.find(t => t.id === taskId && t.projectId === projectId);
    const checkout = task ? taskCheckout(state, task) : !taskId ? repoOf(state, projectId)?.checkouts.find(c => c.kind === "main") : undefined;
    const root = checkout?.path ?? (!taskId || task?.checkout.kind === "main" ? state.projects.find(p => p.id === projectId)?.repository : undefined);
    // Preserve parent segments: symlinks affect which file they name. Never use the main root for a worktree.
    if (root?.startsWith("/")) return `${root.replace(/\/$/, "")}/${path}`;
  });
  const pathInfo = createMemo((): PathInfo | undefined => {
    const request = relativeArtifact();
    if (!request) return info();
    const path = resolvedArtifactPath();
    if (path) return artifactInfo(path);
    const timedOut = checkoutTimedOut();
    return { path: request.path, artifact: true, resolving: !timedOut,
      notice: timedOut ? "Could not confirm the checkout path." : "Finding the checkout path…" };
  });
  createEffect(() => {
    if (!relativeArtifact() || resolvedArtifactPath()) return;
    const timer = setTimeout(() => setCheckoutTimedOut(true), ARTIFACT_CHECKOUT_WAIT_MS);
    onCleanup(() => clearTimeout(timer));
  });
  const resolveArtifact = (request: NonNullable<ReturnType<typeof relativeArtifact>>) => {
    setCheckoutTimedOut(false);
    setRelativeArtifact({ ...request });
    // Registry reads can return before the repository publishes its verified checkout.
    if (!resolvedArtifactPath()) void ws?.store.refreshProjectFacts(request.projectId);
  };
  const retry = () => {
    const request = relativeArtifact();
    if (request) resolveArtifact(request);
  };
  const close = () => {
    setInfo(undefined);
    setRelativeArtifact(undefined);
    if (document.activeElement === document.body || document.activeElement?.closest(".ws-path-pop")) trigger.focus({ preventScroll: true });
  };
  const activate = async (event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (props.inert || !ws) return;
    if (/^(https?:\/\/|mailto:)/i.test(props.href)) {
      if (busy()) return;
      try {
        setStatus("");
        setBusy(true);
        if (ws.openResultTarget) await ws.openResultTarget({ kind: "url", url: props.href });
        else window.open(props.href, "_blank", "noopener,noreferrer");
      } catch { setStatus("Could not open this link in another app."); }
      finally { setBusy(false); }
      return;
    }
    if (props.href.startsWith("#") && !props.reference) {
      const slug = (s: string) => s.toLowerCase().replace(/[^\p{L}\d\s_-]/gu, "").trim().replace(/\s+/g, "-");
      const heading = [...(trigger.closest(".ws-result-copy")?.querySelectorAll("h1,h2,h3,h4,h5,h6") ?? [])].find(el => slug(el.textContent ?? "") === props.href.slice(1));
      if (heading) heading.scrollIntoView?.({ block: "nearest" });
      else setStatus("Could not find this heading.");
      return;
    }
    const ref = props.reference, source = scope?.();
    if (!ref || !source) { setStatus("Could not open this link."); return; }
    if (busy()) return;
    if (pathInfo()) { close(); return; }
    setStatus("");
    const artifact = !props.href.startsWith("#") && ws.machine().kind === "local" && mediaArtifact.test(ref.path);
    if (artifact) {
      // Only the explicit second action reaches the native opener.
      if (ref.path.startsWith("/")) setInfo(artifactInfo(ref.path));
      else resolveArtifact({ path: ref.path, projectId: source.projectId, taskId: source.taskId });
      return;
    }
    const { projectId, taskId, projectCode } = source;
    const state = ws.store.state;
    const checkoutForScope = () => {
      const task = taskId ? state().tasks.find(t => t.id === taskId && t.projectId === projectId) : undefined;
      return taskId ? task && taskCheckout(state(), task) : repoOf(state(), projectId)?.checkouts.find(c => c.kind === "main");
    };
    setBusy(true);
    let full = ref.path;
    try {
      if (!checkoutForScope()) await ws.store.refreshProjectFacts(projectId);
      if (!active) return;
      const checkout = checkoutForScope(), repo = repoOf(state(), projectId);
      const project = state().projects.find(p => p.id === projectId);
      const task = taskId ? state().tasks.find(t => t.id === taskId) : undefined;
      const root = checkout?.path ?? (!taskId || task?.checkout.kind === "main" ? project?.repository : undefined);
      if (root?.startsWith("/") || ref.path.startsWith("/")) full = absolutePath(root ?? "/", ref.path);
      if (!root?.startsWith("/")) { setInfo({ path: full, notice: "Checkout is unavailable. Reconnect and try again." }); return; }
      const prefix = absolutePath("/", root).replace(/\/$/, "") + "/";
      if (!full.startsWith(prefix)) { setInfo({ path: full }); return; }
      if (!checkout || !repo) { setInfo({ path: full, notice: "Checkout is unavailable. Reconnect and try again." }); return; }
      const file = full.slice(prefix.length);
      // Code combines the snapshot with checkout changes and bounded on-demand directory listings.
      const files = checkoutFiles(state(), repo, checkout);
      const diff = files.diffs.find(d => d.path === file);
      let found = files.tree.includes(file) && diff?.status !== "D";
      if (!found) {
        const slash = file.lastIndexOf("/");
        const dir = slash < 0 ? "" : file.slice(0, slash), name = file.slice(slash + 1);
        const page = await ws.store.listFolders(checkout.id, { dirs: [dir] });
        if (!active) return;
        const folder = page.folders.find(f => f.dir === dir);
        found = !!folder && "entries" in folder && folder.entries.some(e => e.name === name && !e.dir);
        if (!found && folder && ("entries" in folder ? !folder.exact || folder.more > 0 : folder.code !== "folder_missing")) {
          setInfo({ path: full, notice: "Could not confirm this file in the checkout. Refresh and try again." });
          return;
        }
      }
      if (!found) { setInfo({ path: full, notice: "File not found in this checkout" }); return; }
      if (ws.store.canEdit()) await ws.buffers.open(projectId, checkout.id, file);
      if (active) goToCode(ws, projectId, checkout.id, projectCode ? undefined : taskId, file, ref.line);
    } catch {
      if (active) setInfo({ path: full, notice: "Could not confirm this file in the checkout. Refresh and try again." });
    } finally { if (active) setBusy(false); }
  };
  const present = createMemo(() => {
    if (!props.reference?.tentative) return true;
    const source = scope?.();
    if (!ws || !source) return false;
    const state = ws.store.state(), task = source.taskId ? state.tasks.find(t => t.id === source.taskId && t.projectId === source.projectId) : undefined;
    const repo = repoOf(state, source.projectId);
    const checkout = source.taskId ? task && taskCheckout(state, task) : repo?.checkouts.find(c => c.kind === "main");
    if (!repo || !checkout) return false;
    const files = checkoutFiles(state, repo, checkout), path = props.reference.path;
    return files.tree.includes(path) && files.diffs.find(d => d.path === path)?.status !== "D";
  });
  const fileOpener = () => ws?.openResultTarget && ws.machine().kind === "local" && ws.can("workspace.open_editor") && pathInfo()?.path.startsWith("/") ? ws.openResultTarget : undefined;
  return <Show when={present()} fallback={props.children}><a id={linkId} ref={el => { trigger = el; }} class={props.class} href={props.href} rel="noreferrer" draggable={false}
    aria-busy={busy() || pathInfo()?.resolving || undefined} aria-haspopup={pathInfo() ? "dialog" : undefined} aria-expanded={pathInfo() ? true : undefined}
    aria-controls={pathInfo() && linkId ? `${linkId}-details` : undefined}
    onClick={e => void activate(e)} onAuxClick={e => { e.preventDefault(); e.stopPropagation(); }}>{props.children}</a>
    <Show when={status()}><span role="status"> {status()}</span></Show>
    <Show when={pathInfo()}>{path => <PathPopover trigger={trigger} info={path()} openLabel={path().artifact ? "Open in default app" : ws!.externalLabel()} close={close}
      retry={checkoutTimedOut() && relativeArtifact() && !resolvedArtifactPath() ? retry : undefined}
      open={fileOpener() ? () => fileOpener()!({ path: path().path, line: props.reference?.line, column: props.reference?.column, kind: "file" }) : undefined} />}</Show></Show>;
}
