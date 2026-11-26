import { describe, expect, it } from "vitest";
import { currentModelId, plainChoice } from "../src/selectors";

const models = [
  { id: "xr_5f46d413ac842d86e4b25f1a", previousIds: ["xr_03485bd94a43755028acf08a"] },
  { id: "xr_549a569a505e1d2248e2b432" },
];

describe("a saved route id the catalogue replaced", () => {
  it("reads as the current id of the same model", () => {
    expect(currentModelId(models, "xr_03485bd94a43755028acf08a")).toBe("xr_5f46d413ac842d86e4b25f1a");
  });
  it("keeps a current id and an unknown id as they are", () => {
    expect(currentModelId(models, "xr_549a569a505e1d2248e2b432")).toBe("xr_549a569a505e1d2248e2b432");
    expect(currentModelId(models, "xr_03f36841ccf45fa27956f1cc")).toBe("xr_03f36841ccf45fa27956f1cc");
    expect(plainChoice("xr_03f36841ccf45fa27956f1cc")).toBe("Model no longer offered");
  });
  it("does not guess between two models that both held the id", () => {
    const both = [{ id: "xr_a00000000000", previousIds: ["xr_b00000000000"] }, { id: "xr_c00000000000", previousIds: ["xr_b00000000000"] }];
    expect(currentModelId(both, "xr_b00000000000")).toBe("xr_b00000000000");
  });
});
