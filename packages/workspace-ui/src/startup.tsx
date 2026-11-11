import { createSignal, Match, onCleanup, Show, Switch, type JSX } from "solid-js";
import { useWs } from "./ctx";
import { bandHandlers, WindowControls } from "./frame";
import { I } from "./kit";

const SLOW_MS = 2000, LONG_MS = 15000, LOGO_TRIES = 5;

/**
 * The built page names the logo by a relative path (`./cortex-256.png`). That resolves against the page's address, which the
 * router changes to `/m/…` as soon as the interface starts: any later request for the same image (a retry below, or the engine
 * loading it again as it is moved into the startup screen) would ask for `/m/…/cortex-256.png` and draw the missing-image glyph.
 * Called once before the first route change, it fixes the address to the absolute one it resolves to at that moment.
 */
export function pinLogoSource(logo: HTMLImageElement | null | undefined) {
  const relative = logo?.getAttribute("src");
  if (logo && relative && relative !== logo.src) logo.setAttribute("src", logo.src);
}

/** A logo image that failed to arrive would show the engine's missing-image glyph in its place: it stays invisible and is asked
 *  for again a few times, so the screen shows the logo or nothing, never a broken picture. */
function keepLogo(logo: unknown) {
  if (typeof HTMLImageElement === "undefined" || !(logo instanceof HTMLImageElement)) return;
  let tries = 0, timer: ReturnType<typeof setTimeout> | undefined;
  const src = logo.getAttribute("src");
  const check = () => {
    const broken = !!src && logo.complete && logo.naturalWidth === 0;
    logo.style.visibility = broken ? "hidden" : "";
    if (broken && tries < LOGO_TRIES && timer === undefined) timer = setTimeout(() => { timer = undefined; tries++; logo.src = src!; }, 300 * (tries + 1));
  };
  logo.addEventListener("load", check); logo.addEventListener("error", check);
  check();
  onCleanup(() => { clearTimeout(timer); logo.removeEventListener("load", check); logo.removeEventListener("error", check); });
}

/**
 * Until the first coherent read of the machine there is nothing true to show, so nothing is shown but the logo: the frame is not
 * mounted at all. A window with no native frame still needs its own controls, and a start that stalls or fails says so, under the
 * logo and without moving it. A slow start shows a turning loader beside its words, so the window never looks frozen, but no
 * percentage: nothing here knows how far along the service is.
 */
export function StartupScreen(props: { logo?: JSX.Element; notice?: JSX.Element }) {
  const ws = useWs();
  const [waited, setWaited] = createSignal<"" | "slow" | "long">("");
  const timers = [setTimeout(() => setWaited("slow"), SLOW_MS), setTimeout(() => setWaited("long"), LONG_MS)];
  onCleanup(() => timers.forEach(clearTimeout));
  keepLogo(props.logo);
  const failed = () => !!ws.store.readIssue() || ws.store.connection().state === "unavailable";
  const reason = () => ws.store.readIssue() ?? ws.store.connection().reason;
  const stalled = () => failed() || waited() === "long";
  return <div class="l-startup">
    <header class="l-startup-band" {...bandHandlers(ws.chrome)}><WindowControls /></header>
    <div class="l-startup-logo">{props.logo}</div>
    <div class="l-startup-slot" role="status" aria-live="polite">
      <Switch>
        <Match when={failed()}>
          <span>The workspace could not be loaded.</span>
          <Show when={reason()}>{why => <span class="l-startup-why">{why()}</span>}</Show>
          <button class="btn btn-line" onClick={() => void ws.store.reload()}>Try again</button>
        </Match>
        <Match when={waited() === "long"}><span class="ws-loading"><I n="loader" s={12} class="spin" />Still waiting for the service. The first start can take longer.</span></Match>
        <Match when={waited() === "slow"}><span class="ws-loading"><I n="loader" s={12} class="spin" />Loading workspace…</span></Match>
      </Switch>
      <Show when={stalled()} fallback={<Show when={ws.chrome?.closeError?.()}>{why => <span>{why()}</span>}</Show>}>
        <div class="l-startup-notice">{props.notice}</div>
      </Show>
    </div>
  </div>;
}
