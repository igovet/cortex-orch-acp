// Real Russian fixture decoding in the disposable builder. No microphone or cloud model.
"use strict";
const assert = require("node:assert/strict"), fs = require("node:fs"), path = require("node:path"), crypto = require("node:crypto");
const voice = require(path.resolve(process.argv[2], "voice.cjs"));
(async () => {
  const fixture = fs.readFileSync(process.argv[3]);
  assert.equal(crypto.createHash("sha256").update(fixture).digest("hex"), "d6e768803b3bc3afcb08326677f3bb872c9beeed29af40d3579bcc14e74484f8");
  const progress = [];
  const directory = await voice.prepare(process.argv[4], p => progress.push(p.progress), new AbortController().signal);
  assert(voice.verified(directory));
  const engine = voice.recognizer(directory), pcm = fixture.subarray(44), started = performance.now();
  const partial = await voice.decode(engine, pcm.subarray(0, 4 * 32000));
  const text = await voice.decode(engine, pcm);
  assert(partial.length > 10);
  assert(text.includes("родион потапыч") && text.includes("каждый новый вершок"));
  let downloaded = false;
  assert.equal(await voice.prepare(process.argv[4], () => { downloaded = true; }, new AbortController().signal), directory);
  assert(!downloaded, "cached model must not download again");
  console.log(JSON.stringify({check:"real_russian_fixture",passed:true,fixture_sha256:"d6e768803b3bc3afcb08326677f3bb872c9beeed29af40d3579bcc14e74484f8",
    download_progress:progress.length,download_complete:progress.at(-1) === 100,cached_without_download:true,partial_characters:partial.length,text,elapsed_ms:Math.round(performance.now()-started)}));
})().catch(() => { console.error("real_russian_fixture_failed"); process.exit(1); });
