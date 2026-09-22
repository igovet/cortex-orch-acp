import assert from "node:assert/strict";
import { coalesced } from "./coalesce.ts";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
let running = 0, runs = 0;
const releases: Array<() => void> = [];
const refresh = coalesced(async () => {
  runs += 1;
  running += 1;
  assert.equal(running, 1, "two refreshes ran at once");
  await new Promise<void>((resolve) => releases.push(resolve));
  running -= 1;
});

const first = refresh();
await tick();
assert.equal(runs, 1);
// Three notifications arrive while the first refresh is still in flight.
await refresh();
await refresh();
await refresh();
assert.equal(runs, 1, "a queued notification must not start a second concurrent run");
releases[0]();
await tick();
assert.equal(runs, 2, "the notifications that arrived mid-refresh were dropped");
releases[1]();
await first;
await tick();
assert.equal(runs, 2, "one queued re-run covers every notification that arrived");
console.log("coalesce: ok");
