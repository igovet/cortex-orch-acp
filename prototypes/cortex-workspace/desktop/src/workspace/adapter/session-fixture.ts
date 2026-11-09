import { emptySession, newWorker, type Activity, type Decision, type Message, type Task, type TaskSession, type Worker, type WorkspaceState } from "../domain";

/** Synthetic session data, not a transcript. Times sit just before the fixture clock (02:46:40), so elapsed figures tick on from there. */
const T = (minute: number, second = 0) => Date.UTC(2030, 0, 1, 2, minute, second);
const NOW = T(46, 40);
const id = (kind: string, n: number) => "mock-" + kind + "-" + String(n).padStart(5, "0");
const W = { backend: id("worker", 4), qa: id("worker", 5), migration: id("worker", 6), reviewer: id("worker", 7), author: id("worker", 8), explorer: id("worker", 9) };

export const WORKSPACE_SESSION_WORKERS = W;
/** What the simulation adds to a running worker, in order. Mock only. */
export const WORKSPACE_SCRIPT: Record<string, { text: string; entry?: Partial<Omit<Activity, "sequence" | "text" | "mock">> }[]> = {
  [W.backend]: [
    { text: "The race test is still running. The lock path holds so far." },
    { text: "Edited", entry: { kind: "edit", code: "auth/refresh.py", diff: { add: 2, del: 0, lines: ["@@ rotate_refresh_token(session, token_id) @@", "+    wait = monotonic() - started", "+    log.debug(\"lock wait %.2fs\", wait)"] } } },
    { text: "Read", entry: { kind: "tools", rows: [{ icon: "file", code: "tests/auth/test_race.py", note: "96 lines" }] } },
    { text: "The double issue is gone in the last 8 runs." },
  ],
};

const message = (n: number, taskId: string, workerId: string | null, text: string, delivery: Message["delivery"], at: number | null, extra: Partial<Message> = {}): Message =>
  ({ id: id("message", n), taskId, workerId, text, mode: "send_now", delivery, at, deliveredAt: null, failure: null, mock: true, ...extra });

function backendActivity(): Activity[] {
  const items: Omit<Activity, "sequence" | "mock">[] = [
    { kind: "divider", text: "Attempt 1 ended 02:33, route unavailable", at: T(33) },
    { kind: "message", text: "Start with the mobile client path, it refreshes in the background.", at: T(32), messageId: id("message", 3) },
    { kind: "divider", text: "Attempt 2 started 02:34 on Opus, High", at: T(34) },
    { kind: "assignment", text: "Assignment: lock the refresh path so concurrent refreshes share one rotation. Scope", code: "auth/**, tests/auth/**", at: T(34) },
    { kind: "note", text: "I will read the refresh path first, then check who else rotates tokens.", at: T(34) },
    { kind: "tools", text: "Read 3 files, searched 2 patterns", at: T(35), rows: [
      { icon: "file", code: "auth/refresh.py", note: "212 lines" }, { icon: "file", code: "auth/models.py", note: "148 lines" }, { icon: "file", code: "tests/auth/conftest.py", note: "96 lines" },
      { icon: "search", code: "rotate_refresh_token", note: "6 matches" }, { icon: "search", code: "session.get(RefreshToken", note: "2 matches" },
    ] },
    { kind: "tool", text: "Called", code: "trace_path", note: "Done, 0.4s", at: T(36), args: "function_name: \"rotate_refresh_token\", direction: \"inbound\", depth: 2\n4 callers: api.refresh, api.mobile_refresh, jobs.session_cleanup, tests" },
    { kind: "message", text: "Check the session cleanup job as well.", at: T(37), messageId: id("message", 4) },
    { kind: "note", text: "The cleanup job only reads revoked rows, so it is safe. Holding a row lock across the rotation stops the double issue. Adding SELECT FOR UPDATE.", at: T(39) },
    { kind: "edit", text: "Edited", code: "auth/refresh.py", at: T(40), diff: { add: 18, del: 4, lines: [
      "@@ rotate_refresh_token(session, token_id) @@", "-    row = session.get(RefreshToken, token_id)", "+    row = (session.query(RefreshToken)", "+           .filter_by(id=token_id)", "+           .with_for_update().one())",
    ] } },
    { kind: "command", text: "pytest tests/auth -q", at: T(41), command: { state: "passed", exit: 0, took: "9s", tail: "", reason: null } },
    { kind: "command", text: "mypy auth", at: T(42), command: { state: "failed", exit: 1, took: "4s", tail: "auth/refresh.py:88: error: Incompatible return value (got \"Row\", expected \"RefreshToken\")\nFound 1 error in 1 file", reason: null } },
    { kind: "edit", text: "Edited", code: "auth/models.py", at: T(42), diff: { add: 3, del: 1, lines: ["@@ class RefreshToken @@", "-    version = Column(Integer)", "+    version = Column(Integer, nullable=False)", "+    # bumped on every rotation", "+    rotated_at = Column(DateTime)"] } },
    { kind: "command", text: "git diff --stat", at: T(43), command: { state: "native", exit: null, took: null, tail: "", reason: null } },
    { kind: "message", text: "Rename the lock helper.", at: T(43), messageId: id("message", 5) },
    { kind: "command", text: "pytest tests/auth/test_race.py -q", at: T(46), command: { state: "running", exit: null, took: null, tail: "collected 12 items\ntests/auth/test_race.py ........", reason: null } },
    { kind: "message", text: "Use the existing retry helper, not a new one.", at: T(46), messageId: id("message", 6) },
    { kind: "message", text: "Keep the 30 second timeout.", at: T(46), messageId: id("message", 7) },
    { kind: "message", text: "Also log when the lock wait is long.", at: T(46), messageId: id("message", 8) },
  ];
  return items.map((item, i) => ({ ...item, sequence: i + 1, mock: true }));
}
const notes = (lines: [number, string][]): Activity[] => lines.map(([at, text], i) => ({ sequence: i + 1, kind: "note", text, at, mock: true }));

function workers(task: Task): Worker[] {
  const base = { taskId: task.id, executionId: task.executionId! };
  return [
    newWorker({ ...base, id: W.backend, role: "Backend Dev", status: "working", goal: "Lock the refresh path so concurrent refreshes share one rotation.", now: "Running pytest tests/auth/test_race.py",
      route: { model: "opus[1m]", effort: "high" }, attempt: { n: 2, of: 2 }, elapsedMs: 0, runningSince: NOW - 372000, context: { used: 76000, window: 200000 }, usage: 64000, scope: ["auth/**", "tests/auth/**"], activity: backendActivity() }),
    newWorker({ ...base, id: W.qa, role: "QA Engineer", status: "waiting_you", goal: "Build the concurrency regression suite for token refresh.", now: "Asked which concurrency level to assert",
      route: { model: "claude-sonnet-5", effort: "medium" }, attempt: { n: 1, of: 1 }, elapsedMs: 220000, context: { used: 44000, window: 200000 }, usage: 21000, scope: ["tests/auth/**"],
      activity: notes([[T(34), "Read tests/auth/conftest.py"], [T(35), "Asked a question and paused."]]) }),
    newWorker({ ...base, id: W.migration, role: "Migration Dev", status: "pending", goal: "Add the token version migration.", waitsFor: "Lock the refresh path" }),
    newWorker({ ...base, id: W.reviewer, role: "Security Reviewer", status: "not_started", lane: "review", goal: "Independent review of the finished change.", now: "Starts after execution finishes" }),
    newWorker({ ...base, id: W.author, role: "Test Author", status: "completed", goal: "Reproduce the race with a failing test.", route: { model: "claude-sonnet-5", effort: "medium" }, attempt: { n: 2, of: 2 }, elapsedMs: 171000, usage: 18000,
      activity: notes([[T(28), "Read the refresh path and its fixtures"], [T(30), "Added the failing race test"], [T(32), "Finished with one check not run"]]),
      result: { summary: "Added tests/auth/test_race.py. It fails on main in 9 of 10 runs with two parallel refreshes.", mock: true,
        checks: [{ name: "Race test fails on main as expected", outcome: "passed", detail: "Mock outcome", source: "runtime" }, { name: "ruff check tests/auth", outcome: "passed", detail: "Mock outcome", source: "runtime" }, { name: "Full auth suite", outcome: "not_run", detail: "Test database was busy", source: "worker" }],
        edits: [{ path: "tests/auth/test_race.py", add: 64, del: 0 }],
        attempts: [{ n: 1, ok: false, detail: "interrupted: route unavailable", at: T(25) }, { n: 2, ok: true, detail: "finished", at: T(32) }] } }),
    newWorker({ ...base, id: W.explorer, role: "Code Explorer", status: "completed", goal: "Trace the 401 race.", route: { model: "claude-sonnet-5", effort: "low" }, attempt: { n: 1, of: 1 }, elapsedMs: 161000, usage: 12000,
      activity: notes([[T(21), "Searched the refresh path"], [T(23), "Traced the 401 race to token rotation"]]),
      result: { summary: "Two refreshes read the same row before either commits. The second one presents a token the first has revoked.", checks: [], mock: true } }),
  ];
}

const question = (n: number, from: string, workerId: string | null, at: number, text: string, options: Decision["options"]): Decision =>
  ({ id: id("decision", n), kind: "question", from, workerId, at, text, options, answer: null, withdrawn: false });

function session(task: Task): TaskSession {
  const steer = id("message", 2), asked = question(0, "QA Engineer", W.qa, T(43), "Which concurrency level should the regression test assert?", [
    { label: "50 parallel refreshes", sub: "Matches production peak" }, { label: "200 parallel refreshes", sub: "Stress, 40s per run" }, { label: "Both, with 200 marked slow" },
  ]);
  return {
    ...emptySession(task.id, T(20)), base: { branch: "main", commit: "4f2a91c" },
    plan: { revision: 2, note: "changed from r1: added migration", nodes: [
      { id: "trace", title: "Trace the 401 race", who: "Code Explorer", workerId: W.explorer }, { id: "repro", title: "Reproduce it with a failing test", who: "Test Author", workerId: W.author },
      { id: "lock", title: "Lock the refresh path", who: "Backend Dev", workerId: W.backend }, { id: "suite", title: "Concurrency regression suite", who: "QA Engineer", workerId: W.qa },
      { id: "migration", title: "Add the token version migration", who: "Migration Dev", workerId: W.migration },
    ] },
    decisions: [asked], context: { used: 82000, window: 200000 }, tokens: 182000, planUsage: { percent: 38, observedAt: T(20) },
    coordinator: { note: "Waiting on Backend Dev and your answer", elapsedMs: 18 * 60000, usage: 67000 },
    verification: ["pytest tests/auth", "mypy auth"], changes: { files: 3, add: 85, del: 5 },
    timeline: [
      { id: "e1", kind: "cortex", text: "I traced the 401s to two concurrent refreshes rotating the same token row. The second call reads a token that was already revoked. Plan r2 locks the rotation, adds a version column, and covers it with a concurrency test." },
      { id: "e2", kind: "fold", text: "Worked 4m 12s, 18 steps, 6 files", steps: ["Read the refresh path and its callers", "Started Code Explorer and Test Author", "Revised the plan to r2 after the race was traced"] },
      { id: "e3", kind: "plan" },
      { id: "e4", kind: "cortex", text: "The lock and the regression suite touch different files, so they run side by side. The migration waits for the lock." },
      { id: "e5", kind: "workers", title: "Started 3 workers", workerIds: [W.backend, W.qa, W.migration] },
      { id: "e6", kind: "line", icon: "shield", text: "You allowed once: run the test database outside the sandbox", at: T(37), rule: false },
      { id: "e7", kind: "message", messageId: steer },
      { id: "e8", kind: "decision", decisionId: asked.id },
    ],
  };
}

/** One task carries the whole approved session; two more carry a decision each, so Answer always has somewhere to land. */
export function seedSessions(state: WorkspaceState, full: Task, permission: Task, asking: Task) {
  full.title = "Fix token refresh race"; full.titleState = "generated";
  full.description = "Clients get random 401s when two tabs refresh at once. Find the race in the token refresh path, fix it, and prove it with a concurrency test. Keep the public API unchanged.";
  full.checkout = { kind: "worktree", branch: "cw/fix-token-refresh", path: "worktrees/fix-token-refresh" };
  full.status = "needs_you"; full.resumeStatus = "needs_you"; full.process = "live"; full.issue = "Question from QA Engineer"; full.executionId = id("execution", 244);
  state.workers.push(...workers(full));
  state.sessions.push(session(full));
  const backend = W.backend;
  state.messages.push(
    message(2, full.id, null, "Keep the 30 second refresh timeout. Do not raise it to make the test pass.", "recorded", T(40)),
    message(3, full.id, backend, "Start with the mobile client path, it refreshes in the background.", "failed", T(32), { failure: "Not delivered: attempt 1 ended first" }),
    message(4, full.id, backend, "Check the session cleanup job as well.", "delivered", T(37), { deliveredAt: T(38) }),
    message(5, full.id, backend, "Rename the lock helper.", "withdrawn", T(43)),
    message(6, full.id, backend, "Use the existing retry helper, not a new one.", "recorded", T(46)),
    message(7, full.id, backend, "Keep the 30 second timeout.", "sending", T(46)),
    message(8, full.id, backend, "Also log when the lock wait is long.", "failed", T(46), { failure: "Could not send: connection to the machine dropped" }),
    message(9, full.id, null, "When the lock lands, also update the auth runbook.", "queued", T(44), { mode: "queue" }),
    message(10, full.id, null, "Then split the fix into two commits for review.", "queued", T(45), { mode: "queue" }),
  );
  const allow: Decision = {
    id: id("decision", 1), kind: "permission", from: "", workerId: null, at: T(40), text: "Allow mock workspace edit?", options: [],
    effect: "Changes only the in-memory fixture.", scope: state.projects.find(p => p.id === permission.projectId)!.name, ifDenied: "Mock execution remains blocked.", answer: null, withdrawn: false,
  };
  const format = question(2, "", null, T(41), "Which synthetic export format?", [{ label: "CSV" }, { label: "JSON" }]);
  state.sessions.push(
    { ...emptySession(permission.id), decisions: [allow], timeline: [{ id: "p1", kind: "decision", decisionId: allow.id }] },
    { ...emptySession(asking.id), decisions: [format], timeline: [{ id: "q1", kind: "decision", decisionId: format.id }] },
  );
}
