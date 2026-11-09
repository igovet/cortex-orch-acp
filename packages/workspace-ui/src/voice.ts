import { createSignal, onCleanup } from "solid-js";
import { voiceReason, type VoicePage, type VoicePhase, type VoicePort } from "./voice-client";

/** Only an explicit Insert action consumes a transcript. Stop/timeout never sends. */
export function createLocalDictation(port: VoicePort) {
  const [on, setOn] = createSignal(false), [phase, setPhase] = createSignal<VoicePhase>("ready");
  const [text, setText] = createSignal(""), [seconds, setSeconds] = createSignal(0), [progress, setProgress] = createSignal(0);
  const [fault, setFault] = createSignal<string | null>(null);
  let session: string | undefined, generation = 0, timer: ReturnType<typeof setTimeout> | undefined;
  const terminal = () => ["ready", "error", "cancelled"].includes(phase());
  const apply = (page: VoicePage) => {
    setPhase(page.phase); setText(page.text); setSeconds(page.seconds); setProgress(page.progress);
    if (page.phase === "error") { setFault(voiceReason(page.error)); setOn(false); }
  };
  const stop = () => {
    generation++; clearTimeout(timer); setOn(false); setText("");
    const id = session; session = undefined;
    if (id) void port.cancel(id).catch(() => {}); // The service lease also closes a lost window.
  };
  const fail = (error: unknown) => {
    const message = error instanceof Error ? error.message : String(error);
    stop(); setFault(message.startsWith("voice_") ? voiceReason(message) : message);
  };
  const poll = async (run: number) => {
    if (run !== generation || !session) return;
    try {
      const page = await port.read(session);
      if (run !== generation) return;
      apply(page);
      if (!terminal()) timer = setTimeout(() => void poll(run), 250);
    } catch (error) { if (run === generation) fail(error); }
  };
  const start = async () => {
    stop();
    const run = generation;
    setFault(null); setOn(true); setPhase("starting"); setSeconds(0); setProgress(0);
    try {
      const page = await port.start("voice-" + crypto.randomUUID());
      if (run !== generation) { void port.cancel(page.sessionId).catch(() => {}); return; }
      session = page.sessionId; apply(page);
      if (!terminal()) timer = setTimeout(() => void poll(run), 250);
    } catch (error) { if (run === generation) fail(error); }
  };
  const pause = async () => {
    if (!session || ["starting", "downloading", "loading"].includes(phase())) { stop(); return; }
    if (terminal() || phase() === "stopping") return;
    const run = generation;
    setPhase("stopping");
    try { const page = await port.stop(session); if (run === generation) apply(page); }
    catch (error) { if (run === generation) fail(error); }
  };
  const finish = async () => {
    const run = generation;
    await pause();
    while (run === generation && on() && !terminal()) await new Promise(resolve => setTimeout(resolve, 100));
    if (run !== generation || phase() !== "ready") return "";
    const value = text(); stop(); return value;
  };
  onCleanup(stop);
  return { on, phase, progress, seconds, fault, text, stop, finish, pause,
    words: () => text().split(/\s+/).length, finished: text,
    clearFault: () => setFault(null), start,
    toggle: () => on() && !terminal() ? void pause() : void start() };
}
