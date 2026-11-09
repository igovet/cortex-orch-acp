import { FIXTURE_VERSION } from "./fixtures";

export const SCENARIO_SCRIPTS = [
  {
    id: "create-decision-result", fixtureVersion: FIXTURE_VERSION, mock: true,
    steps: [
      { device: "desktop", action: "create", target: "Synthetic new task", expected: "New task exists in memory only." },
      { device: "desktop", action: "selectScenario", target: "blocked", expected: "Permission and scope are visible." },
      { device: "desktop", action: "decide", target: "mock-decision-permission", expected: "Mock response is recorded; no real permission granted." },
      { device: "desktop", action: "inspectResult", target: "mock-result-complete", expected: "Revision and mock check are visible." },
      { device: "desktop", action: "acceptResult", target: "mock-result-complete", expected: "Mock acceptance only; no commit, push or runtime completion." },
      { device: "desktop", action: "selectScenario", target: "stale", expected: "Stale result cannot be accepted." },
    ],
  },
  {
    id: "code-git-conflict", fixtureVersion: FIXTURE_VERSION, mock: true,
    steps: [
      { device: "desktop", action: "selectScenario", target: "conflict", expected: "Both file and Git conflict are shown." },
      { device: "desktop", action: "compare", target: "mock-file-00000", expected: "Base, local and incoming text are distinct." },
      { device: "desktop", action: "resolveConflict", target: "local", expected: "Mock resolution recorded without filesystem or Git writes." },
    ],
  },
  {
    id: "desktop-phone-desktop", fixtureVersion: FIXTURE_VERSION, mock: true,
    steps: [
      { device: "desktop", action: "selectTask", target: "mock-task-00002", expected: "Open question for stable task identity." },
      { device: "phone", action: "setDevice", target: "phone", expected: "Same in-memory task identity; not physical-device qualification." },
      { device: "phone", action: "decide", target: "mock-decision-question", expected: "Mock answer recorded in shared memory." },
      { device: "desktop", action: "setDevice", target: "desktop", expected: "Task identity and mock answer are retained." },
    ],
  },
] as const;
