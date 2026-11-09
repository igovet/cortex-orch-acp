import { createSignal, onCleanup, Show } from "solid-js";
import { useWs } from "./ctx";

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
  const timer = setInterval(() => setNow(Date.now()), 1000);
  onCleanup(() => clearInterval(timer));
  const text = () => ws.store.connection().state === "connected" ? "" : freshness(props.at ?? ws.store.lastRead(), now());
  return <Show when={text()}><span class="cap faint ws-confirmed">{text()}</span></Show>;
}
