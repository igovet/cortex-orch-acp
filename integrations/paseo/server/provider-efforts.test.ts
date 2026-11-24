import assert from "node:assert/strict";
import { withModelEfforts } from "./provider.ts";

// Every model gets its own efforts, not the catalog-time model's.
const catalog = withModelEfforts(
  {
    models: [{ id: "gpt-5.6-luna" }, { id: "haiku" }],
    thinkingOptions: [{ id: "low", label: "low" }],
    defaultThinkingOption: "medium",
  },
  { "gpt-5.6-luna": ["max", "default", "low", "medium", "engine_default"] },
);
const [luna, haiku] = catalog.models as unknown as Array<{
  thinkingOptions: Array<{ id: string }>;
  defaultThinkingOptionId?: string;
}>;
assert.deepEqual(luna.thinkingOptions.map((option) => option.id), ["low", "medium", "max"]);
assert.equal(luna.defaultThinkingOptionId, "medium");
assert.deepEqual(haiku.thinkingOptions, []);
assert.equal(haiku.defaultThinkingOptionId, undefined);
const legacy = withModelEfforts(
  { models: [{ id: "codex", defaultThinkingOptionId: "default" }], defaultThinkingOption: "default" },
  { codex: ["high", "medium", "default"] },
);
assert.equal(legacy.defaultThinkingOption, undefined);
assert.equal(legacy.models[0].defaultThinkingOptionId, "medium");
console.log("ok");
