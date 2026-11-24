/** Local on-device dictation only. No audio/file transport or engine call is offered here. */
export type VoicePhase = "starting" | "downloading" | "loading" | "recording" | "stopping" | "ready" | "error" | "cancelled";
export type VoiceLanguage = "ru" | "en";
export const voiceLanguageName = (language: VoiceLanguage) => language === "en" ? "English" : "Russian";
const isVoiceLanguage = (value: unknown): value is VoiceLanguage => value === "ru" || value === "en";
export type VoicePage = { sessionId: string; phase: VoicePhase; progress: number; text: string; seconds: number; error: string; language?: VoiceLanguage };
export type VoiceModel = { language: VoiceLanguage; name: string; bytes: number; phase: "ready" | "not_downloaded" | "integrity_failed" };
export type VoiceSettingsPage = { language: VoiceLanguage; busy: boolean; models: VoiceModel[] };
export type VoicePort = {
  start(requestId: string): Promise<VoicePage>;
  read(sessionId: string): Promise<VoicePage>;
  stop(sessionId: string): Promise<VoicePage>;
  cancel(sessionId: string): Promise<VoicePage>;
  /** Owner-only controls for the desktop recognizer, not the phone's system recognizer. */
  local?: {
    settings(): Promise<VoiceSettingsPage>;
    setLanguage(language: VoiceLanguage): Promise<void>;
    download(requestId: string): Promise<VoicePage>;
  };
  /** What this recognizer is, in the words shown beside a recording. Absent for the desktop's local dictation. */
  badge?: string;
};
export const voiceReason = (code: string, language?: VoiceLanguage): string => ({
  voice_unavailable: "Local dictation is not installed. Reinstall the complete desktop package.",
  capture_unavailable: "Microphone capture is unavailable: pulseaudio-utils (parec) is missing.",
  microphone_unavailable: "The desktop audio server could not open the default microphone. Check microphone access, then retry.",
  model_download_failed: `The local ${language ? voiceLanguageName(language) + " " : ""}speech model could not be downloaded. Check the network, then retry.`,
  model_integrity_failed: "The speech model failed its content check. It was not loaded. Retry the download.",
  voice_session_lost: "Dictation stopped because its local session was lost. Nothing was sent.",
  voice_settings_unavailable: "Voice settings are unavailable. Recheck and try again.",
  voice_busy: "Another composer is using the microphone. Stop or cancel it first.",
  voice_session_unknown: "This dictation session is gone. Start again; nothing was resent.",
  voice_request_expired: "This recording request has expired. Start a new recording.",
  transcription_failed: "The local recognizer could not transcribe this recording. Nothing was sent; retry.",
  transcript_limit: "The transcript reached its local size limit. Nothing was sent.",
}[code] ?? "Local dictation is unavailable. Nothing was sent; try again.");
export function createVoicePort(transport: { request(operation: string, message: unknown): Promise<unknown> }): VoicePort {
  const request = async (op: string, message: unknown, session?: string): Promise<VoicePage> => {
    const p = await transport.request("voice." + op, message) as VoicePage;
    if (!p || typeof p.sessionId !== "string" || !p.sessionId || (session && p.sessionId !== session)
      || !isVoiceLanguage(p.language)
      || !["starting","downloading","loading","recording","stopping","ready","error","cancelled"].includes(p.phase)
      || typeof p.text !== "string" || [...p.text].length > 16000 || typeof p.error !== "string"
      || !Number.isFinite(p.progress) || p.progress < 0 || p.progress > 100
      || !Number.isFinite(p.seconds) || p.seconds < 0 || p.seconds > 100)
      throw new Error("Invalid local dictation response. Nothing was sent.");
    return p;
  };
  return {
    local: {
      async settings() {
        const p = await transport.request("voice.settings", {}) as VoiceSettingsPage;
        if (!p || !isVoiceLanguage(p.language) || typeof p.busy !== "boolean"
          || !Array.isArray(p.models) || p.models.length !== 2
          || !p.models.every(m => m && isVoiceLanguage(m.language) && typeof m.name === "string"
            && Number.isSafeInteger(m.bytes) && m.bytes > 0
            && ["ready","not_downloaded","integrity_failed"].includes(m.phase))
          || new Set(p.models.map(m => m.language)).size !== 2)
          throw new Error("Invalid local dictation settings.");
        return p;
      },
      async setLanguage(language) {
        const p = await transport.request("voice.setLanguage", { language }) as { language?: unknown };
        if (p?.language !== language) throw new Error("Invalid local dictation settings.");
      },
      download: requestId => request("download", { requestId }),
    },
    async start(requestId) {
      const caps = await transport.request("service.capabilities", {}) as { methods?: string[] };
      if (!["start","read","stop","cancel"].every(op => caps.methods?.includes("voice." + op))) throw new Error(voiceReason("voice_unavailable"));
      return request("start", { requestId });
    },
    read: sessionId => request("read", { sessionId }, sessionId),
    stop: sessionId => request("stop", { sessionId }, sessionId),
    cancel: sessionId => request("cancel", { sessionId }, sessionId),
  };
}
