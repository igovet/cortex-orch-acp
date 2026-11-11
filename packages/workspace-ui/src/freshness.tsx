import { createContext, createEffect, createSignal, onCleanup, Show, useContext } from "solid-js";
import { useWs } from "./ctx";

/** Whether the screen around a component is the one on view. A kept-alive screen that is not, passes false here. */
export const ScreenActive = createContext<() => boolean>(() => true);

/** The part of the page on view: the active tab, or the whole page where no tabs surround it. A lookup by class never lands in a parked tab. */
export const onScreen = (): ParentNode => document.querySelector(".ws-pane:not([data-parked])") ?? document;

/** Where a form says it holds text that closing its tab would lose. */
export const UnsentRegistry = createContext<(check: () => boolean) => () => void>(() => () => undefined);
export function createUnsent(check: () => boolean) {
  const add = useContext(UnsentRegistry);
  onCleanup(add(check));
}

const bodyMarks = new Map<string, number>();
let nextMark = 0;
/** A mark on `document.body` for the stylesheet, held only by the screen on view: a parked tab neither sets one nor clears another tab's. */
export function createBodyMark(name: string, value: () => string | undefined) {
  const shown = useContext(ScreenActive), me = ++nextMark;
  const clear = () => { if (bodyMarks.get(name) === me) { bodyMarks.delete(name); delete document.body.dataset[name]; } };
  createEffect(() => {
    if (!shown()) return clear();
    const mark = value();
    bodyMarks.set(name, me);
    if (mark === undefined) delete document.body.dataset[name]; else document.body.dataset[name] = mark;
  });
  onCleanup(clear);
}

/** Repeats `work` only while it can matter: `when` holds, the screen is on view and the window is not hidden. */
export function createTicker(work: () => void, every: number, when: () => boolean = () => true) {
  const active = useContext(ScreenActive);
  const [visible, setVisible] = createSignal(typeof document === "undefined" || !document.hidden);
  const change = () => setVisible(!document.hidden);
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", change);
    onCleanup(() => document.removeEventListener("visibilitychange", change));
  }
  createEffect(() => {
    if (!when() || !active() || !visible()) return;
    work();
    const timer = setInterval(work, every);
    onCleanup(() => clearInterval(timer));
  });
}

const age = (seconds: number) => seconds < 60 ? `${seconds}s` : seconds < 3600 ? `${Math.floor(seconds / 60)}m` : seconds < 86400 ? `${Math.floor(seconds / 3600)}h` : `${Math.floor(seconds / 86400)}d`;

/** Words a person reads: the local clock time and a rounded age, never a machine timestamp. */
export function freshness(at: number | null | undefined, now: number): string {
  if (at == null || !Number.isFinite(at)) return "";
  const time = new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  return `Last confirmed ${time} · ${age(Math.max(0, Math.floor((now - at) / 1000)))} old`;
}

/** Live data needs no stamp. This appears only while the machine is away, when what is on screen may be out of date. */
export function Confirmed(props: { at?: number | null }) {
  const ws = useWs();
  const [now, setNow] = createSignal(Date.now());
  // Only while the stamp shows, and no faster than its smallest unit a person reads.
  createTicker(() => setNow(Date.now()), 15_000, () => ws.store.connection().state !== "connected");
  const text = () => ws.store.connection().state === "connected" ? "" : freshness(props.at ?? ws.store.lastRead(), now());
  return <Show when={text()}><span class="cap faint ws-confirmed">{text()}</span></Show>;
}
