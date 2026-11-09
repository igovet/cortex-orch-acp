import type { PlanItem, Sprint } from "../domain";

/** What the simulated planner says. A real plan comes from a planning model; this one only keeps the screens moving. */
export const PLAN_OPENING = "I read the code this touches. One decision changes the plan, so I have a question.";
export const PLAN_QUESTION = { text: "Should this ship behind a setting first?", options: ["Yes, off by default", "No, on for everyone"] };

const slug = (text: string) => text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 24).replace(/-+$/, "") || "sprint";
export function mockPlan(sprint: Sprint): PlanItem[] {
  const base = slug(sprint.title ?? sprint.description), tag = sprint.id.slice(-5);
  const item = (key: string, title: string, description: string, acceptance: string, dependsOn: string[]): PlanItem =>
    ({ key, title, description, acceptance, dependsOn, checkout: { kind: "worktree", branch: `cw/${base}-${key}`, path: `worktrees/${base}-${key}-${tag}` } });
  return [
    item("data", "Add the data model", "Tables and the migration this needs.", "Migration applies and rolls back on a copy", []),
    item("api", "Build the service", "The server side of the change.", "Contract tests pass", ["data"]),
    item("ui", "Add the screen", "What the owner sees and does.", "Keyboard run through, empty and error states", ["api"]),
    item("docs", "Document the change", "User guide and release notes.", "Links and commands checked against source", ["api", "ui"]),
  ];
}
