import { Match, Show, Switch } from "solid-js";
import { AttachChips } from "./composer";
import { useWs } from "./ctx";
import { I, Keyed } from "./kit";
import { CopySource } from "./message-copy";
import { ResultText } from "./result-text";
import { humanStatus } from "./selectors";
import type { Sending, SendPlace } from "./sends";

/** What a sent message's line says while the machine confirms it, and what the owner can do when it did not arrive. */
function SentStatus(props: { e: Sending }) {
  const ws = useWs();
  return <Switch>
    <Match when={props.e.state === "sending"}><I n="loader" s={12} class="spin ws-wait" /><span aria-live="polite">Sending…</span></Match>
    <Match when={props.e.state === "sent"}><I n="check" s={12} /><span aria-live="polite">Sent</span></Match>
    <Match when={props.e.state === "failed"}><I n="alert" s={12} class="st-bad" /><span class="st-bad" role="alert">Not sent: {humanStatus(props.e.reason)}</span>
      <span class="act">
        <button type="button" class="lnk" onClick={() => void ws.store.sends.retry(props.e.key)}>Retry</button>
        <button type="button" class="lnk" onClick={() => ws.store.sends.edit(props.e.key)}>Edit</button>
      </span></Match>
  </Switch>;
}

/**
 * The owner's messages sent from this window that do not read in their place yet: shown the moment they are sent, with their
 * state, until the machine's record of each takes its place. A message that did not arrive keeps its text and files, says why,
 * and can be sent again or taken back into the composer.
 */
export function SentMessages(props: { place: SendPlace; worker?: boolean }) {
  const ws = useWs();
  const shown = () => ws.store.sends.shown(props.place);
  return <Keyed each={shown()} by={e => e.key}>{e => <Show when={props.worker} fallback={<>
    <div class={`mine${e().state === "failed" ? " ws-sent-bad" : ""}`} data-sent={e().state}><ResultText breaks text={e().text} /><AttachChips files={e().attachments} open /></div>
    <div class="mine-meta ws-sent-meta"><SentStatus e={e()} /><CopySource text={e().text} /></div>
  </>}>
    <div class={`me${e().state === "failed" ? " bad" : ""}`} data-sent={e().state}>
      <div class="b"><ResultText breaks text={e().text} /><AttachChips files={e().attachments} open /></div>
      <div class="ms"><SentStatus e={e()} /><CopySource text={e().text} /></div>
    </div>
  </Show>}</Keyed>;
}
