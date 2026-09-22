import assert from "node:assert/strict";
import { withModelEfforts } from "./provider.ts";

// Every model gets its own efforts, not the catalog-time model's.
const catalog = withModelEfforts(
  { models: [{ id: "gpt-5.6-luna" }, { id: "haiku" }], thinkingOptions: [{ id: "low", label: "low" }] },
  { "gpt-5.6-luna": ["low", "max"] },
);
const [luna, haiku] = catalog.models as unknown as Array<{ thinkingOptions: Array<{ id: string }> }>;
assert.deepEqual(luna.thinkingOptions.map((option) => option.id), ["low", "max"]);
assert.deepEqual(haiku.thinkingOptions, []);
console.log("ok");
