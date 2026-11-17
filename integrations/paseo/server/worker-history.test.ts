import assert from "node:assert/strict";
import { resolve } from "node:path";
import { loadServerModuleForTest } from "./test-bundle.ts";

const { parsePaseoRuntimeSessionId } = await loadServerModuleForTest<typeof import("./worker-history.ts")>(resolve("server/worker-history.ts"));

const SESSION_ID = "acps_0123456789ab";

function wrappedSessionId(sessionId: string): string {
  return `plugin:${JSON.stringify({ version: 1, data: { sessionId } })}`;
}

// `parsePaseoRuntimeSessionId` accepts the exact wrapped v1 shape and
// rejects malformed/ambiguous wrappers -- the identity boundary the rest of
// this module trusts. The store reads themselves are served by
// `cortex-paseo-tools` and covered end to end by store-reader.test.ts.
{
  assert.equal(parsePaseoRuntimeSessionId({ sessionId: wrappedSessionId(SESSION_ID) }), SESSION_ID);
  assert.equal(parsePaseoRuntimeSessionId({ sessionId: "plugin:not-json" }), null);
  assert.equal(parsePaseoRuntimeSessionId({ sessionId: `plugin:${JSON.stringify({ version: 2, data: { sessionId: SESSION_ID } })}` }), null);
  assert.equal(parsePaseoRuntimeSessionId({ sessionId: `plugin:${JSON.stringify({ version: 1, data: { sessionId: "not-a-session" } })}` }), null);
  assert.equal(parsePaseoRuntimeSessionId(null), null);
  assert.equal(parsePaseoRuntimeSessionId({}), null);
}

console.log("worker-history: ok");
