// Local dictation worker. Only the same-user service starts it; no project or model tools.
"use strict";
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const { spawn } = require("node:child_process");
const { once } = require("node:events");
const model = require("./model.json");
const hash = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
const verified = directory => Object.entries(model.files).every(([name, digest]) => {
  try { const p = path.join(directory, name); return fs.lstatSync(p).isFile() && hash(fs.readFileSync(p)) === digest; }
  catch { return false; }
});

async function prepare(state, emit, signal) {
  const base = path.join(state, "voice");
  fs.mkdirSync(base, { recursive: true, mode: 0o700 });
  const directory = path.join(base, model.name);
  if (verified(directory)) return directory;
  // A failed or cancelled attempt is never installed as a model.
  // A process-group cancellation can interrupt finally; remove only our stale staging directories on retry.
  for (const entry of fs.readdirSync(base, { withFileTypes: true })) {
    if (entry.isDirectory() && /^download-[A-Za-z0-9]{6}$/.test(entry.name)) fs.rmSync(path.join(base, entry.name), { recursive: true });
  }
  const stage = fs.mkdtempSync(path.join(base, "download-"));
  const archive = path.join(stage, "model.tar.bz2");
  try {
    emit({ phase: "downloading", progress: 0 });
    const response = await fetch(model.url, { signal });
    if (!response.ok || !response.body) throw Error("model_download_failed");
    const output = fs.openSync(archive, "wx", 0o600), sha = crypto.createHash("sha256");
    let bytes = 0, last = -1;
    try {
      for await (const chunk of response.body) {
        bytes += chunk.length;
        if (bytes > model.bytes) throw Error("model_integrity_failed");
        sha.update(chunk); fs.writeSync(output, chunk);
        const progress = Math.floor(bytes * 100 / model.bytes);
        if (progress !== last) { emit({ phase: "downloading", progress }); last = progress; }
      }
    } finally { fs.closeSync(output); }
    if (bytes !== model.bytes || sha.digest("hex") !== model.sha256) throw Error("model_integrity_failed");
    const unpacked = path.join(stage, "model");
    fs.mkdirSync(unpacked, { mode: 0o700 });
    const tar = spawn("/usr/bin/tar", ["-xjf", archive, "--strip-components=1", "-C", unpacked,
      ...Object.keys(model.files).map(f => model.name + "/" + f)], { stdio: "ignore", signal });
    if ((await once(tar, "exit"))[0] !== 0 || !verified(unpacked)) throw Error("model_integrity_failed");
    if (fs.existsSync(directory)) fs.rmSync(directory, { recursive: true });
    fs.renameSync(unpacked, directory);
    return directory;
  } catch (e) {
    if (signal.aborted || ["model_download_failed", "model_integrity_failed"].includes(e.message)) throw e;
    throw Error("model_download_failed");
  } finally { fs.rmSync(stage, { recursive: true, force: true }); }
}
function recognizer(directory) {
  const sherpa = require("sherpa-onnx-node");
  return new sherpa.OfflineRecognizer({
    featConfig: { sampleRate: 16000, featureDim: 80 },
    modelConfig: { transducer: {
      encoder: path.join(directory, "encoder.int8.onnx"), decoder: path.join(directory, "decoder.onnx"),
      joiner: path.join(directory, "joiner.int8.onnx") },
      tokens: path.join(directory, "tokens.txt"), numThreads: 2, provider: "cpu", debug: false },
    decodingMethod: "greedy_search",
  });
}
async function decode(recognizer, pcm) {
  const samples = new Float32Array(pcm.length / 2);
  for (let i = 0; i < samples.length; i++) samples[i] = pcm.readInt16LE(i * 2) / 32768;
  const stream = recognizer.createStream();
  stream.acceptWaveform({ samples, sampleRate: 16000 });
  return (await recognizer.decodeAsync(stream)).text.trim();
}
async function main(state) {
  let capture, ticker, cancelled = false, stopping = false, busy = false, text = "", fixed = "";
  let chunks = [], length = 0, seconds = 0, pending = Buffer.alloc(0), silence = 0;
  const abort = new AbortController();
  const emit = update => process.stdout.write(JSON.stringify({ text, seconds, progress: 100, error: "", ...update }) + "\n");
  const finishCapture = () => { clearInterval(ticker); capture?.kill("SIGTERM"); };
  const cancel = () => { cancelled = true; abort.abort(); finishCapture(); };
  process.on("SIGTERM", () => { cancel(); process.exit(0); });
  process.stdin.setEncoding("utf8");
  let commands = "";
  process.stdin.on("data", data => {
    commands += data;
    if (commands.length > 64 || commands.includes("cancel\n")) { cancel(); process.exit(0); }
    if (commands.includes("stop\n")) { stopping = true; capture?.kill("SIGTERM"); }
  });
  process.stdin.on("end", () => { cancel(); process.exit(0); });
  try {
    // Fail before downloading when no native capture executable is installed.
    if (!fs.existsSync("/usr/bin/parec")) throw Error("capture_unavailable");
    const directory = await prepare(state, emit, abort.signal);
    if (cancelled || stopping) { emit({ phase: "ready" }); process.exit(0); }
    emit({ phase: "loading" });
    const engine = recognizer(directory);
    if (cancelled || stopping) { emit({ phase: "ready" }); process.exit(0); }
    capture = spawn("/usr/bin/parec", ["--raw", "--format=s16le", "--rate=16000", "--channels=1", "--latency-msec=100"],
      { stdio: ["ignore", "pipe", "ignore"] });
    const started = Date.now();
    let lastAudio = started, failed = false;
    capture.on("error", () => { failed = true; });
    capture.on("exit", () => { if (!stopping && !cancelled) failed = true; });
    capture.stdout.on("data", bytes => {
      if (cancelled) return;
      lastAudio = Date.now();
      pending = Buffer.concat([pending, bytes]);
      const n = pending.length - pending.length % 2;
      if (!n) return;
      const data = pending.subarray(0, n); pending = pending.subarray(n);
      chunks.push(data); length += n; seconds += n / 32000;
      let energy = 0;
      for (let i = 0; i < n; i += 2) energy += (data.readInt16LE(i) / 32768) ** 2;
      silence = Math.sqrt(energy / (n / 2)) < .008 ? silence + n / 32000 : 0;
      // Bound memory even if a decoder stalls.
      if (length > 32 * 32000) { failed = true; finishCapture(); }
    });
    emit({ phase: "recording" });
    const tick = async () => {
      if (busy || cancelled) return;
      busy = true;
      try {
        if (failed || Date.now() - lastAudio > 5000) throw Error("microphone_unavailable");
        if (seconds >= 90) { stopping = true; capture?.kill("SIGTERM"); }
        const final = stopping;
        if (length >= 16000 || final) {
          const commit = stopping || length >= 20 * 32000 || silence >= .8;
          const audio = Buffer.concat(chunks, length);
          if (commit) { chunks = []; length = 0; silence = 0; }
          const partial = audio.length ? await decode(engine, audio) : "";
          if (cancelled) return;
          text = [fixed, partial].filter(Boolean).join(" ");
          if (text.length > 16000) throw Error("transcript_limit");
          if (commit) fixed = text;
        }
        emit({ phase: final ? "ready" : "recording" });
        if (final) { finishCapture(); process.exit(0); }
      } catch (e) { finishCapture(); emit({ phase: "error", error: ["transcript_limit", "microphone_unavailable"].includes(e.message) ? e.message : "transcription_failed" }); process.exitCode = 1; process.stdin.destroy(); }
      finally { busy = false; }
    };
    ticker = setInterval(() => void tick(), 800);
  } catch (e) {
    finishCapture();
    if (!cancelled) emit({ phase: "error", error: ["capture_unavailable", "model_integrity_failed", "model_download_failed"].includes(e.message) ? e.message : "voice_unavailable" });
    process.exitCode = 1; process.stdin.destroy();
  }
}
module.exports = { prepare, recognizer, decode, verified, model };
if (require.main === module) main(process.argv[2]);
