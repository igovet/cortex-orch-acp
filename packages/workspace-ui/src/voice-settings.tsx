import { createSignal, onMount, Show } from "solid-js";
import { useWs } from "./ctx";
import { createLocalDictation } from "./voice";
import { Srow } from "./settings-kit";

export function VoiceSettings() {
  const ws = useWs();
  const [model, setModel] = createSignal("Checking model…");
  const voice = ws.voice ? createLocalDictation(ws.voice) : undefined;
  const refresh = () => void ws.store.machineAction("maintenance", {action:"voice.status"}).then(r => setModel(String(r.message)), () => setModel("Model status is unavailable. Retry from a composer."));
  onMount(refresh);
  return <div class="set-main"><div class="set-in">
    <h1 class="set-title">Voice input</h1>
    <p class="set-lead">Local Russian dictation. Audio stays on this machine; text is never sent automatically.</p>
    <div class="surface">
      <Srow t="Provider" d="sherpa-onnx on CPU, packaged with this application."><span>On-device</span></Srow>
      <Srow t="Language" d="This packaged recognizer supports Russian only. The message language does not change recognition."><span>Russian</span></Srow>
      <Srow t="Model" d={model()}><span class="mono">small-zipformer-ru-2024-09-18</span></Srow>
      <Srow t="Microphone" d="PulseAudio / PipeWire default source. Change the input in your desktop sound settings."><span>System default</span></Srow>
    </div>
    <p class="hint">First use downloads about 105 MiB and verifies its pinned SHA-256 and model files. A failed or cancelled download is not installed. Retry starts a fresh verified download.</p>
    <button class="btn btn-line" onClick={refresh}>Recheck model</button>
    <Show when={voice} fallback={<p class="hint">Dictation needs the complete local desktop package.</p>}>{v => <>
      <button class="btn btn-line" disabled={v().on()} onClick={() => void v().start()}>Test microphone / retry model download</button>
      <Show when={v().on()}><p role="status">{v().phase()} {v().progress()}%</p><button class="btn" onClick={() => {v().stop(); refresh();}}>Cancel test</button></Show>
      <Show when={v().fault()}><p class="err" role="alert">{v().fault()}</p></Show>
      <Show when={v().text()}><p>{v().text()}</p></Show>
    </>}</Show>
    <p class="hint">This test records only after the model loads. Cancel discards its text. Use the microphone in a composer to Insert or Insert and send explicitly.</p>
  </div></div>;
}
