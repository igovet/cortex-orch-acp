import { describe, expect, it } from "vitest";
import { emptyWorkspace, NEUTRAL_POLICY } from "@cortex/workspace-domain";
import { planFrom, sessionOptions } from "../src/port/real";

describe("coordinator choices follow the Models profile", () => {
  const config = () => {
    const c = emptyWorkspace().machines[0].config, r = c.surfaces.routing.engines["claude-agent-acp"];
    c.setup["claude-agent-acp"].status = "connected";
    c.catalogue["claude-agent-acp"].models = [
      { id: "opus", label: "Opus", efforts: ["low", "medium", "high", "xhigh", "max"], observed: true },
      { id: "haiku", label: "Haiku", efforts: ["low", "medium"], observed: true },
    ];
    r.enabled = ["opus", "haiku"];
    return c;
  };
  it("offers every enabled model while no model has a tier", () => {
    const options = sessionOptions(config());
    expect(options.models.map(m => [m.label, m.efforts])).toEqual([["Opus", ["low", "medium", "high", "xhigh", "max"]], ["Haiku", ["low", "medium"]]]);
  });
  it("offers only models with a tier, at their tiers' efforts", () => {
    const c = config(), r = c.surfaces.routing.engines["claude-agent-acp"];
    r.tiers = { opus: ["deep", "flagship"] };
    r.policy = { opus: { ...NEUTRAL_POLICY, tierEfforts: { deep: { min: "xhigh", max: "xhigh" } } } };
    const options = sessionOptions(c);
    // Flagship keeps the legacy rule (up to high), deep is its range.
    expect(options.models.map(m => [m.label, m.efforts])).toEqual([["Opus", ["low", "medium", "high", "xhigh"]]]);
    expect(options.efforts.map(e => e.id)).toEqual(["low", "medium", "high", "xhigh"]);
  });
  it("keeps a plan item's complexity from the planner's answer", () => {
    expect(planFrom("```json\n{\"tasks\":[{\"key\":\"a\",\"description\":\"Do\",\"dependsOn\":[],\"complexity\":\"high\"},{\"key\":\"b\",\"description\":\"Do\",\"dependsOn\":[],\"complexity\":\"huge\"}]}\n```"))
      .toEqual([{ key: "a", title: "Do", description: "Do", dependsOn: [], complexity: "high" }, { key: "b", title: "Do", description: "Do", dependsOn: [] }]);
  });
});
