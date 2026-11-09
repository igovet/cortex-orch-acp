/** Local on-device dictation only. No audio/file transport or engine call is offered here. */
export type VoicePhase = "starting" | "downloading" | "loading" | "recording" | "stopping" | "ready" | "error" | "cancelled";
export type VoicePage = { sessionId: string; phase: VoicePhase; progress: number; text: string; seconds: number; error: string };
export type VoicePort = {
  start(requestId: string): Promise<VoicePage>;
  read(sessionId: string): Promise<VoicePage>;
  stop(sessionId: string): Promise<VoicePage>;
  cancel(sessionId: string): Promise<VoicePage>;
};
export const voiceReason = (code: string): string => ({
  voice_unavailable: "Local dictation is not installed. Reinstall the complete desktop package.",
  capture_unavailable: "Microphone capture is unavailable: pulseaudio-utils (parec) is missing.",
  microphone_unavailable: "The desktop audio server could not open the default microphone. Check microphone access, then retry.",
  model_download_failed: "The local Russian speech model could not be downloaded. Check the network, then retry.",
  model_integrity_failed: "The speech model failed its content check. It was not loaded. Retry the download.",
  voice_session_lost: "Dictation stopped because its local session was lost. Nothing was sent.",
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
      || !["starting","downloading","loading","recording","stopping","ready","error","cancelled"].includes(p.phase)
      || typeof p.text !== "string" || [...p.text].length > 16000 || typeof p.error !== "string"
      || !Number.isFinite(p.progress) || p.progress < 0 || p.progress > 100
      || !Number.isFinite(p.seconds) || p.seconds < 0 || p.seconds > 100)
      throw new Error("Invalid local dictation response. Nothing was sent.");
    return p;
  };
  return {
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
