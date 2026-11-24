// Real Russian or English fixture decoding in a disposable state directory. No microphone or cloud model.
"use strict";
const assert = require("node:assert/strict"), fs = require("node:fs"), path = require("node:path"), crypto = require("node:crypto");
const voice = require(path.resolve(process.argv[2], "voice.cjs"));
const state = process.argv[4], language = process.argv[5] ?? "ru";
const catalogue = require(path.resolve(process.argv[2], "model.json"));
const fixtures = {
  ru: ["d6e768803b3bc3afcb08326677f3bb872c9beeed29af40d3579bcc14e74484f8", "родион потапыч", "каждый новый вершок"],
  en: ["6bc58a4efdf20daac252b6b1502632601a71efe0308f6757dc1eda34891a7e4f", "after early nightfall", "yellow lamps"],
};
async function checkUninstalledAttempts() {
  const failureState = fs.mkdtempSync(path.join(require("node:os").tmpdir(), "cw-voice-failure-"));
  const fetch = globalThis.fetch;
  try {
    globalThis.fetch = async () => new Response("not the pinned archive");
    await assert.rejects(voice.prepare(failureState, language, () => {}, new AbortController().signal), /model_integrity_failed/);
    assert.deepEqual(fs.readdirSync(path.join(failureState, "voice")), []);

    const abort = new AbortController();
    globalThis.fetch = async (url, { signal }) => {
      assert.equal(url, catalogue[language].url);
      assert.equal(signal, abort.signal);
      return new Response(new ReadableStream({ pull(controller) {
        abort.abort();
        controller.error(signal.reason);
      } }));
    };
    await assert.rejects(voice.prepare(failureState, language, () => {}, abort.signal), { name: "AbortError" });
    assert.deepEqual(fs.readdirSync(path.join(failureState, "voice")), []);
  } finally {
    globalThis.fetch = fetch;
    fs.rmSync(failureState, { recursive: true, force: true });
  }
}

(async () => {
  assert(Object.hasOwn(fixtures, language));
  await checkUninstalledAttempts();
  const [digest, ...expected] = fixtures[language];
  const fixture = fs.readFileSync(process.argv[3]);
  assert.equal(crypto.createHash("sha256").update(fixture).digest("hex"), digest);
  const progress = [];
  const directory = await voice.prepare(state, language, p => progress.push(p.progress), new AbortController().signal);
  assert(voice.verified(directory, language));
  const engine = voice.recognizer(directory, language), pcm = fixture.subarray(44), started = performance.now();
  const partial = await voice.decode(engine, pcm.subarray(0, 4 * 32000), language);
  const text = await voice.decode(engine, pcm, language);
  assert(partial.length > 10);
  if (language === "en") {
    assert.match(partial, /[a-z]/, "English partial must not be all caps");
    assert.match(text, /[a-z]/, "English final text must not be all caps");
  }
  for (const phrase of expected) assert(text.toLowerCase().includes(phrase));
  if (progress.length) assert(progress[0] === 0 && progress.at(-1) === 100);
  let downloaded = false;
  assert.equal(await voice.prepare(state, language, () => { downloaded = true; }, new AbortController().signal), directory);
  assert(!downloaded, "cached model must not download again");
  console.log(JSON.stringify({check:language === "ru" ? "real_russian_fixture" : "real_english_fixture",passed:true,fixture_sha256:digest,
    download_progress:progress.length,download_complete:progress.at(-1) === 100,cached_without_download:true,failed_or_cancelled_not_installed:true,partial_characters:partial.length,text,elapsed_ms:Math.round(performance.now()-started)}));
})().catch(e => { console.error("real_voice_fixture_failed", language, e.message); process.exit(1); });
