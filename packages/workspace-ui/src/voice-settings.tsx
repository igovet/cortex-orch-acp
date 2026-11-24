import { createEffect, createSignal, For, on, onMount, Show } from "solid-js";
import { useWs } from "./ctx";
import { createLocalDictation } from "./voice";
import { voiceLanguageName, voiceReason, type VoiceLanguage, type VoiceSettingsPage } from "./voice-client";
import { Srow } from "./settings-kit";

export function VoiceSettings() {
  const ws = useWs();
  const port = ws.voice, local = port?.local;
  const voice = port && local ? createLocalDictation(port) : undefined;
  const [settings, setSettings] = createSignal<VoiceSettingsPage>();
  const [checking, setChecking] = createSignal(true), [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal("");
  let revision = 0;
  const refresh = async () => {
    if (!local) return;
    const run = ++revision;
    setChecking(true);
    try {
      const result = await local.settings();
      if (run === revision) setSettings(result);
    } catch (cause) {
      const code = cause instanceof Error ? cause.message : String(cause);
      if (run === revision) setError(code === "voice_settings_unavailable"
        ? voiceReason(code) : "Model status is unavailable. Recheck and try again.");
    } finally {
      if (run === revision) setChecking(false);
    }
  };
  const active = () => voice?.on() && !["ready", "cancelled", "error"].includes(voice.phase());
  const blocked = () => checking() || saving() || active() || settings()?.busy;
  const select = async (language: VoiceLanguage) => {
    if (!local || blocked() || settings()?.language === language) return;
    setSaving(true); setError("");
    try {
      await local.setLanguage(language);
      setSettings(value => value && ({ ...value, language }));
    } catch (cause) {
      setError(voiceReason(cause instanceof Error ? cause.message : String(cause)));
      await refresh();
    } finally { setSaving(false); }
  };
  onMount(() => void refresh());
  if (voice) createEffect(on(voice.phase, phase => {
    if (["ready", "error", "cancelled"].includes(phase)) void refresh();
  }, { defer: true }));
  return <div class="set-main"><div class="set-in">
    <h1 class="set-title">Voice input</h1>
    <p class="set-lead">Local Russian and English dictation. Audio stays on this machine; text is never sent automatically.</p>
    <div class="surface">
      <Srow t="Provider" d="sherpa-onnx on CPU, packaged with this application."><span>On-device</span></Srow>
      <Show when={settings()} fallback={local && <Srow t="Language" d={checking() ? "Checking models…" : "Model status unavailable."} />}>{current => <>
        <Srow t="Language" d="Choose the language for the next recording. It cannot change during recording or model download.">
          <div role="group" aria-label="Dictation language">
            <For each={["ru", "en"] as const}>{lang => <button type="button" class="btn btn-line"
              disabled={blocked()} aria-pressed={current().language === lang} aria-label={voiceLanguageName(lang)}
              onClick={() => void select(lang)}>{lang.toUpperCase()}</button>}</For>
          </div>
        </Srow>
        <For each={current().models}>{model => <Srow t={voiceLanguageName(model.language) + " model"}
          d={model.phase === "ready" ? "Verified local model ready." : model.phase === "integrity_failed"
            ? voiceReason("model_integrity_failed") : "Speech model is not downloaded."}>
          <span>{Math.round(model.bytes / 1048576)} MiB</span>
          <Show when={local && voice && model.language === current().language && model.phase !== "ready"}>
            <button type="button" class="btn btn-line" disabled={blocked()} onClick={() => void voice!.start(local!.download)}>Download</button>
          </Show>
        </Srow>}</For>
      </>}</Show>
      <Srow t="Microphone" d="PulseAudio / PipeWire default source. Change the input in your desktop sound settings."><span>System default</span></Srow>
    </div>
    <p class="hint">First use downloads and verifies only the selected model. A failed or cancelled download is not installed. Retry starts a fresh verified download.</p>
    <Show when={voice && local} fallback={<p class="hint">The on-device models on this page need the complete local desktop package.</p>}>
      <button type="button" class="btn btn-line" disabled={checking() || saving() || active()} onClick={() => {setError(""); void refresh();}}>Recheck models</button>
      <button type="button" class="btn btn-line" disabled={blocked() || !settings()} onClick={() => void voice!.start()}>Test microphone</button>
      <Show when={active()}><p role="status">{voice!.phase()} {voice!.progress()}%</p>
        <button type="button" class="btn" onClick={() => {
          setChecking(true);
          void Promise.resolve(voice!.stop()).then(refresh);
        }}>Cancel test or download</button></Show>
      <Show when={voice!.fault() || error()}><p class="err" role="alert">{voice!.fault() || error()}</p></Show>
      <Show when={voice!.text()}><p>{voice!.text()}</p></Show>
      <Show when={voice!.on() && !active() && voice!.text()}><button type="button" class="btn" onClick={() => voice!.stop()}>Discard test text</button></Show>
    </Show>
    <p class="hint">The microphone test records only after the model loads. It never inserts or sends text.</p>
  </div></div>;
}
