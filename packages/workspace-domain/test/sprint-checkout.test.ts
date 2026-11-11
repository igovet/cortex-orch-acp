import { expect, it } from "vitest";
import { sprintTaskCheckout } from "../src";

it.each(["main", "worktree"] as const)("uses the owner's %s choice for every plan item, not the planner's hint", kind => {
  const opposite = kind === "main" ? "worktree" : "main";
  expect(sprintTaskCheckout({ checkout: { kind } }, { checkout: { kind: opposite } })).toEqual({ kind });
  expect(sprintTaskCheckout({ checkout: { kind } }, {})).toEqual({ kind });
});

it("keeps older sprints unchanged, including per-item choices", () => {
  const checkout = { kind: "worktree" as const, branch: "existing", path: "/existing" };
  expect(sprintTaskCheckout({}, { checkout })).toEqual(checkout);
  expect(sprintTaskCheckout({}, {})).toEqual({ kind: "main" });
});
