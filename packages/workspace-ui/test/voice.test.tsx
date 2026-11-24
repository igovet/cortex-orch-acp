import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import type { AttachmentInput } from "@cortex/workspace-domain";
import { WsContext, type Ws } from "../src/ctx";
import { createAttachments, createDictation, MessageComposer } from "../src/composer";
import { VoiceSettings } from "../src/voice-settings";

/** The shared composer as an answer draft: the plane and Insert and send are its only ways to send. */
function Answer(props: { onSend: (text: string, files: AttachmentInput[]) => void }) {
  const [text, setText] = createSignal("");
  const files = createAttachments(), mic = createDictation();
  return <MessageComposer label="Your answer" placeholder="Your answer" text={text()} onText={setText} files={files} mic={mic}
    send={{ label: "Send answer", why: text().trim() ? undefined : "Write an answer first", go: () => props.onSend(text(), files.files()), insertAndSend: value => props.onSend(value, files.files()) }} />;
}
import { createVoicePort, type VoiceLanguage, type VoicePage, type VoicePort } from "../src/voice-client";
const page = (phase: VoicePage["phase"], text = ""): VoicePage => ({sessionId:"s",phase,text,progress:100,seconds:3,error:"",language:"ru"});
let dispose: (() => void) | undefined;
const unmount = () => { dispose?.(); dispose = undefined; };
afterEach(() => { dispose?.(); document.body.innerHTML = ""; vi.useRealTimers(); });
function mount(voice: VoicePort) {
  const send = vi.fn(), root = document.createElement("div"); document.body.append(root);
  const ws = {voice,can:()=>false,t:()=>"",store:{}} as unknown as Ws;
  dispose = render(() => <WsContext.Provider value={ws}><Answer onSend={send} /></WsContext.Provider>,root);
  return send;
}
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent === text)!;
const mic = () => document.querySelector<HTMLButtonElement>('[aria-label="Voice input"]')!;
const input = () => document.querySelector<HTMLTextAreaElement>("textarea")!;
function port() {
  return {
    start: vi.fn(async()=>page("recording")), read: vi.fn(async()=>page("recording","привет мир")),
    stop: vi.fn(async()=>page("ready","привет мир")), cancel: vi.fn(async()=>page("cancelled")),
  } satisfies VoicePort;
}
it("Ctrl+D starts/stops, shows live local text, inserts without sending, and preserves the typed draft", async()=>{
  vi.useFakeTimers(); const voice = port(), send = mount(voice);
  input().value = "draft"; input().dispatchEvent(new Event("input",{bubbles:true})); input().focus();
  input().dispatchEvent(new KeyboardEvent("keydown",{key:"d",ctrlKey:true,bubbles:true,cancelable:true}));
  await vi.advanceTimersByTimeAsync(600);
  expect(voice.start).toHaveBeenCalledTimes(1);
  expect(document.querySelector(".mic-text")?.textContent).toBe("привет мир");
  expect(send).not.toHaveBeenCalled();
  input().dispatchEvent(new KeyboardEvent("keydown",{key:"d",ctrlKey:true,bubbles:true,cancelable:true}));
  await vi.advanceTimersByTimeAsync(20);
  expect(voice.stop).toHaveBeenCalledWith("s");
  expect(send).not.toHaveBeenCalled();
  button("Insert").click(); await vi.advanceTimersByTimeAsync(20);
  expect(input().value).toBe("draft привет мир"); expect(send).not.toHaveBeenCalled();
});
it("only Insert and send sends, one time, and Cancel discards just the recording", async()=>{
  vi.useFakeTimers(); const voice = port(), send = mount(voice);
  mic().click(); await vi.advanceTimersByTimeAsync(600);
  document.querySelector<HTMLButtonElement>(".micbar button")!.click(); await vi.advanceTimersByTimeAsync(20);
  expect(send).not.toHaveBeenCalled(); expect(voice.cancel).toHaveBeenCalledWith("s");
  mic().click(); await vi.advanceTimersByTimeAsync(600);
  button("Insert and send").click(); button("Insert and send").click();
  await vi.advanceTimersByTimeAsync(30);
  expect(send).toHaveBeenCalledTimes(1); expect(send).toHaveBeenCalledWith("привет мир",[]);
});
it("exposes a real capture failure without sending", async()=>{
  vi.useFakeTimers(); const voice = port();
  voice.read.mockResolvedValue({...page("error"),error:"microphone_unavailable"});
  const send = mount(voice); mic().click(); await vi.advanceTimersByTimeAsync(600);
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("default microphone");
  expect(send).not.toHaveBeenCalled();
  unmount(); expect(voice.cancel).toHaveBeenCalled();
});
it("explains unavailable voice settings without starting a recording or sending", async () => {
  vi.useFakeTimers();
  const request = vi.fn(async (operation: string) => {
    if (operation === "service.capabilities") return {methods:["voice.start","voice.read","voice.stop","voice.cancel"]};
    throw new Error("voice_settings_unavailable");
  });
  const voice = createVoicePort({request});
  const send = mount(voice);
  mic().click();
  await vi.advanceTimersByTimeAsync(20);
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("Voice settings are unavailable. Recheck and try again.");
  expect(request.mock.calls.map(([op]) => op)).toEqual(["service.capabilities", "voice.start"]);
  expect(send).not.toHaveBeenCalled();

  unmount();
  const root = document.createElement("div"); document.body.append(root);
  const ws = {voice} as unknown as Ws;
  dispose = render(() => <WsContext.Provider value={ws}><VoiceSettings /></WsContext.Provider>, root);
  await vi.advanceTimersByTimeAsync(0);
  expect(root.querySelector('[role="alert"]')?.textContent).toBe("Voice settings are unavailable. Recheck and try again.");
  expect(button("Test microphone").disabled).toBe(true);
  expect(request.mock.calls.map(([op]) => op)).toEqual(["service.capabilities", "voice.start", "voice.settings"]);
});

it("ignores terminal Ctrl+D, reports progress, and cancels a late start after unmount", async()=>{
  vi.useFakeTimers(); const voice = port();
  voice.start.mockResolvedValue({...page("downloading"),progress:37});
  mount(voice); mic().click(); await vi.advanceTimersByTimeAsync(20);
  expect(document.querySelector('[role="status"]')?.textContent).toContain("37%");
  unmount();
  let resolve!: (p: VoicePage) => void;
  voice.start.mockImplementation(()=>new Promise(r=>{resolve=r;}));
  mount(voice);
  const terminal = document.createElement("div"); terminal.className="terminal";
  terminal.innerHTML="<textarea></textarea>"; document.body.append(terminal);
  terminal.querySelector("textarea")!.focus();
  terminal.dispatchEvent(new KeyboardEvent("keydown",{key:"d",ctrlKey:true,bubbles:true,cancelable:true}));
  expect(voice.start).toHaveBeenCalledTimes(1);
  mic().click(); await vi.advanceTimersByTimeAsync(20);
  unmount();
  resolve(page("recording")); await vi.advanceTimersByTimeAsync(20);
  expect(voice.cancel).toHaveBeenCalledWith("s");
});
it("validates service support, binds session identity, refuses bad data and never replays an uncertain start", async()=>{
  const request = vi.fn(async(op:string)=>op === "service.capabilities" ? {methods:["voice.start","voice.read","voice.stop","voice.cancel"]} : page("recording"));
  const voice = createVoicePort({request});
  await voice.start("r"); expect(request).toHaveBeenCalledWith("voice.start",{requestId:"r"});
  await expect(voice.read("other")).rejects.toThrow("Invalid local dictation");
  request.mockRejectedValueOnce(Error("lost"));
  await expect(voice.start("new")).rejects.toThrow("lost");
  expect(request.mock.calls.filter(c=>c[0]==="voice.start")).toHaveLength(1);
});
it("switches the machine to English, downloads only on request and uses English for subsequent dictation", async () => {
  vi.useFakeTimers();
  let language: VoiceLanguage = "ru", downloading = false, ready = false;
  const request = vi.fn(async (operation: string, message: unknown) => {
    switch (operation) {
      case "voice.settings":
        return {language, busy: downloading, models: [
          {language:"ru", name:"model-ru", bytes:109906654, phase:"integrity_failed"},
          {language:"en", name:"model-en", bytes:112232184, phase:ready ? "ready" : "not_downloaded"},
        ]};
      case "voice.setLanguage":
        language = (message as {language: VoiceLanguage}).language;
        return {language};
      case "voice.download":
        downloading = true;
        return {...page("downloading"), language, progress:40};
      case "service.capabilities":
        return {methods:["voice.start","voice.read","voice.stop","voice.cancel"]};
      case "voice.start":
        return {...page("recording"), language};
      case "voice.read":
        if (downloading) {
          downloading = false; ready = true;
          return {...page("ready"), language};
        }
        return {...page("recording", "good morning"), language};
      case "voice.stop":
        return {...page("ready", "good morning"), language};
      case "voice.cancel":
        return {...page("cancelled"), language};
      default:
        throw new Error("unexpected " + operation);
    }
  });
  const voice = createVoicePort({request});
  const ws = {voice} as unknown as Ws;
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <WsContext.Provider value={ws}><VoiceSettings /></WsContext.Provider>, root);
  const choice = (name: string) => root.querySelector<HTMLButtonElement>(`button[aria-label="${name}"]`)!;
  await vi.advanceTimersByTimeAsync(0);
  expect(choice("Russian").getAttribute("aria-pressed")).toBe("true");
  expect(root.textContent).toContain("failed its content check");
  choice("English").click();
  await vi.advanceTimersByTimeAsync(0);
  expect(choice("English").getAttribute("aria-pressed")).toBe("true");
  expect(request).toHaveBeenCalledWith("voice.setLanguage", {language:"en"});
  expect(request.mock.calls.map(([op]) => op)).not.toContain("voice.download");
  expect(root.textContent).toContain("Speech model is not downloaded.");
  button("Download").click();
  await vi.advanceTimersByTimeAsync(20);
  expect(choice("Russian").disabled).toBe(true);
  expect(root.querySelector('[role="status"]')?.textContent).toContain("40%");
  expect(request.mock.calls.map(([op]) => op)).not.toContain("voice.start");
  await vi.advanceTimersByTimeAsync(300);
  expect(root.textContent).toContain("Verified local model ready.");
  expect(button("Download")).toBeUndefined();

  unmount();
  const send = mount(voice);
  mic().click();
  await vi.advanceTimersByTimeAsync(300);
  expect(document.querySelector(".micbar")?.textContent).toContain("On-device · English · not sent");
  expect(document.querySelector(".mic-text")?.textContent).toBe("good morning");
  button("Insert").click();
  await vi.advanceTimersByTimeAsync(30);
  expect(input().value).toBe("good morning");
  expect(send).not.toHaveBeenCalled();
});
