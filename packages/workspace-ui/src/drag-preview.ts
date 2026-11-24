/** The Linux Tauri shell uses WebKitGTK/GTK3. Its native drag surface treats
 * snapshot device pixels as logical pixels. Only this shell uses a DOM preview:
 * browser/phone engines keep their native feedback, without guessing from DPR. */
function usesDomPreview(win: Window) {
  const bridge = (win as Window & { __TAURI_INTERNALS__?: { invoke?: unknown } }).__TAURI_INTERNALS__;
  const ua = win.navigator.userAgent;
  return typeof bridge?.invoke === "function" && /\bLinux\b/.test(ua) && /\bAppleWebKit\//.test(ua)
    && !/Android|Mobile|Chrom(?:e|ium)\//.test(ua);
}

export function setDragPreview(e: DragEvent & { currentTarget: HTMLElement }) {
  const source = e.currentTarget, doc = source.ownerDocument, win = doc.defaultView;
  const transfer = e.dataTransfer;
  if (!win || !transfer?.setDragImage) return;
  const box = source.getBoundingClientRect();
  if (!box.width || !box.height) return;

  const x = Math.max(0, Math.min(box.width, e.clientX - box.left));
  const y = Math.max(0, Math.min(box.height, e.clientY - box.top));
  const ghost = source.cloneNode(true) as HTMLElement;
  ghost.classList.add("ws-drag-preview");
  for (const attr of ["id", "data-q", "data-task", "draggable"]) ghost.removeAttribute(attr);
  for (const el of ghost.querySelectorAll("[id]")) el.removeAttribute("id");
  const style = win.getComputedStyle(source);
  Object.assign(ghost.style, { width: `${box.width}px`, height: `${box.height}px`, font: style.font, color: style.color });

  const layer = doc.createElement("div");
  layer.className = "ws-drag-layer";
  layer.inert = true;
  layer.setAttribute("aria-hidden", "true");
  // Keep rules such as the queue row's narrow-page wrapping when the clone
  // leaves the scroller. The fixed layer itself never changes the source layout.
  const page = source.closest<HTMLElement>(".page");
  if (page) {
    layer.style.container = "page / inline-size";
    layer.style.width = `${page.clientWidth}px`;
  }
  layer.append(ghost);
  doc.body.append(layer);

  if (!usesDomPreview(win)) {
    transfer.setDragImage(ghost, x, y);
    win.setTimeout(() => layer.remove(), 0);
    return;
  }

  // A painted but empty node yields a transparent native surface, even if
  // GTK scales it. The visible clone and its hotspot both stay in CSS pixels.
  const blank = doc.createElement("div");
  blank.className = "ws-drag-blank";
  blank.setAttribute("aria-hidden", "true");
  doc.body.append(blank);
  transfer.setDragImage(blank, 0, 0);
  win.setTimeout(() => blank.remove(), 0);

  let target: EventTarget | null = null;
  const move = (event: DragEvent) => {
    if (!source.isConnected) { finish(); return; }
    target = event.target;
    ghost.style.left = `${event.clientX - x}px`;
    ghost.style.top = `${event.clientY - y}px`;
    layer.style.visibility = "";
  };
  const leave = (event: DragEvent) => {
    // WebKit dispatches enter on the new element before leaving the old one,
    // and its leave event need not supply relatedTarget.
    if (event.target === target && !event.relatedTarget) {
      target = null;
      layer.style.visibility = "hidden";
    }
  };
  const listeners = new AbortController(), capture = { capture: true, signal: listeners.signal };
  const finish = () => {
    listeners.abort();
    blank.remove();
    layer.remove();
  };
  source.addEventListener("dragend", finish, { signal: listeners.signal });
  doc.addEventListener("dragenter", move, capture);
  doc.addEventListener("dragover", move, capture);
  doc.addEventListener("dragleave", leave, capture);
  doc.addEventListener("drop", finish, capture);
  win.addEventListener("pagehide", finish, { signal: listeners.signal });
  layer.classList.add("ws-drag-overlay");
  move(e);
}
