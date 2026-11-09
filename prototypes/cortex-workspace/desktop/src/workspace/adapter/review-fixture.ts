import { beginMerge, emptySession, newWorker, type Check, type Commit, type ConflictBlock, type DiffHunk, type DiffLine, type FileDiff, type GitFile, type MergeSeed, type ProjectRepo, type RepoCheckout, type Task, type TaskChanges, type Worker, type WorkspaceState } from "../domain";

/** Synthetic Git and review data for Cedar Notes, taken from the approved mockups. Nothing here came from a real repository. */
const T = (minute: number, second = 0) => Date.UTC(2030, 0, 1, 2, minute, second);
const id = (kind: string, n: number) => "mock-" + kind + "-" + String(n).padStart(5, "0");
export const REVIEW_WORKERS = { explorer: id("worker", 20), backend: id("worker", 21), author: id("worker", 22), docs: id("worker", 23), reviewer: id("worker", 24) };

const ctx = (old: number, neu: number, text: string): DiffLine => ({ kind: "ctx", old, new: neu, text });
const add = (neu: number, text: string): DiffLine => ({ kind: "add", old: null, new: neu, text });
const del = (old: number, text: string): DiffLine => ({ kind: "del", old, new: null, text });
const hunk = (header: string, lines: DiffLine[]): DiffHunk => ({ header, lines, staged: false });
const adds = (start: number, lines: string[]) => lines.map((text, i) => add(start + i, text));

const REFRESH = `"""Refresh token rotation."""
from datetime import datetime, timezone

from sqlalchemy.orm import Session

from .errors import TokenReused
from .models import RefreshToken

ROTATION_TIMEOUT = 30  # seconds, keep in step with the client


def rotate_refresh_token(session: Session, token_id: str) -> RefreshToken:
    """Rotate a refresh token, revoking the old row."""
    row = (session.query(RefreshToken)
           .filter_by(id=token_id)
           .with_for_update().one())
    if row.revoked_at is not None:
        raise TokenReused(token_id)

    now = datetime.now(timezone.utc)
    row.revoked_at = now
    replacement = RefreshToken.issue_for(row.user_id, version=row.version + 1)
    session.add(replacement)
    session.commit()
    return replacement


def revoke_all(session: Session, user_id: str) -> int:
    """Revoke every live token for a user."""
    live = session.query(RefreshToken).filter_by(user_id=user_id, revoked_at=None)
    count = 0
    for row in live:
        row.revoked_at = datetime.now(timezone.utc)
        count += 1
    session.commit()
    return count


def purge_expired(session: Session, before: datetime) -> int:
    """Delete revoked rows older than the retention window."""
    rows = session.query(RefreshToken).filter(RefreshToken.revoked_at < before)
    deleted = rows.delete(synchronize_session=False)
    session.commit()
    return deleted
`;
const REFRESH_MAIN = REFRESH
  .replace(`    row = (session.query(RefreshToken)
           .filter_by(id=token_id)
           .with_for_update().one())`, "    row = session.get(RefreshToken, token_id)")
  .replace("RefreshToken.issue_for(row.user_id, version=row.version + 1)", "RefreshToken.issue_for(row.user_id)");

const SOURCES: Record<string, string> = {
  "auth/__init__.py": `"""Sessions and refresh tokens."""\n`,
  "auth/errors.py": `class TokenReused(Exception):\n    """A revoked refresh token was presented again."""\n`,
  "auth/models.py": `from sqlalchemy import Column, DateTime, Integer, String\n\nfrom .base import Base\n\n\nclass RefreshToken(Base):\n    __tablename__ = "refresh_tokens"\n\n    id = Column(String, primary_key=True)\n    user_id = Column(String, nullable=False)\n    version = Column(Integer)\n    revoked_at = Column(DateTime)\n`,
  "auth/refresh.py": REFRESH_MAIN,
  "auth/sessions.py": `"""Live sessions."""\n\n\ndef lock_session(query, session_id):\n    row = query.with_for_update(nowait=True)\n    return row.filter_by(id=session_id).first()\n\n\ndef touch(q, session_id, now):\n    row = q.with_for_update().first()\n    row.seen_at = now\n    return row\n`,
  "billing/invoices.py": `def total(lines):\n    return sum(line.amount for line in lines)\n`,
  "billing/ledger.py": `def next_entry(query):\n    rows = query.with_for_update(skip_locked=True)\n    return rows.first()\n`,
  "docs/auth-runbook.md": `# Auth runbook\n\nRotate refresh tokens with the admin tool.\n`,
  "tests/auth/conftest.py": `from pytest import fixture\n\n\n@fixture\ndef session(db):\n    return db.session()\n`,
  "tests/auth/test_refresh.py": `def test_rotation_revokes_the_old_row(session, token):\n    fresh = rotate_refresh_token(session, token.id)\n    assert token.revoked_at is not None\n    assert fresh.id != token.id\n`,
  "pyproject.toml": `[project]\nname = "cedar-notes"\nversion = "2.4.0"\n`,
  "README.md": `# Cedar Notes\n\nSynthetic project for the Workspace prototype.\n`,
};
const RACE_TEST = `import threading\n\n\ndef test_parallel_refresh_rotates_once(session, token):\n    results = []\n    workers = [threading.Thread(target=refresh, args=(token, results)) for _ in range(2)]\n    for worker in workers:\n        worker.start()\n    for worker in workers:\n        worker.join()\n    assert len(set(results)) == 1\n`;

const tokenFiles = (): GitFile[] => [
  { path: "auth/models.py", status: "M", add: 3, del: 1, stage: "staged", changedBy: "Backend Dev", hunks: [hunk("@@ -8,5 +8,7 @@ class RefreshToken(Base):", [ctx(9, 9, "    id = Column(String, primary_key=True)"), ctx(10, 10, "    user_id = Column(String, nullable=False)"), del(11, "    version = Column(Integer)"), add(11, "    version = Column(Integer, nullable=False)"), add(12, "    # bumped on every rotation"), add(13, "    rotated_at = Column(DateTime)"), ctx(12, 14, "    revoked_at = Column(DateTime)")])] },
  { path: "auth/refresh.py", status: "M", add: 18, del: 4, stage: "unstaged", changedBy: "Backend Dev", hunks: [hunk("@@ -11,13 +11,15 @@ ROTATION_TIMEOUT = 30", [
    ctx(11, 11, ""), ctx(12, 12, "def rotate_refresh_token(session: Session, token_id: str) -> RefreshToken:"), ctx(13, 13, `    """Rotate a refresh token, revoking the old row."""`),
    del(14, "    row = session.get(RefreshToken, token_id)"), add(14, "    row = (session.query(RefreshToken)"), add(15, "           .filter_by(id=token_id)"), add(16, "           .with_for_update().one())"),
    ctx(15, 17, "    if row.revoked_at is not None:"), ctx(16, 18, "        raise TokenReused(token_id)"), ctx(17, 19, ""), ctx(18, 20, "    now = datetime.now(timezone.utc)"), ctx(19, 21, "    row.revoked_at = now"),
    del(20, "    replacement = RefreshToken.issue_for(row.user_id)"), add(22, "    replacement = RefreshToken.issue_for(row.user_id, version=row.version + 1)"),
    ctx(21, 23, "    session.add(replacement)"), ctx(22, 24, "    session.commit()"), ctx(23, 25, "    return replacement"),
  ])] },
  { path: "tests/auth/test_race.py", status: "U", add: 64, del: 0, stage: "untracked", changedBy: "Test Author", hunks: [hunk("@@ -0,0 +1,11 @@", adds(1, RACE_TEST.trimEnd().split("\n")))] },
];

const exportFiles = (): FileDiff[] => [
  { path: "cedar/export/api.py", status: "M", add: 7, del: 1, changedBy: "Backend Dev", hunks: [hunk("@@ -18,5 +18,11 @@ async def export_workspace(request, workspace_id):", [
    ctx(18, 18, "    workspace = await load_workspace(workspace_id)"), ctx(19, 19, "    await require_export_scope(request, workspace)"),
    add(20, "    decision = await limiter.check(workspace.id, limit=60, window=60)"), add(21, "    if not decision.allowed:"), add(22, "        return Response("), add(23, "            status=429,"), add(24, `            headers={"Retry-After": str(decision.retry_after)},`), add(25, "        )"),
    del(20, "    return await stream_export(workspace)"), add(26, "    return await stream_export(workspace, chunk_size=CHUNK)"), ctx(21, 27, ""), ctx(22, 28, "async def export_status(request, job_id):"),
  ])] },
  { path: "cedar/export/limits.py", status: "A", add: 8, del: 0, changedBy: "Backend Dev", hunks: [hunk("@@ -0,0 +1,8 @@", adds(1, [
    "class SlidingWindowLimiter:", `    """Counts requests per workspace in a Redis sorted set."""`, "", "    async def check(self, key, limit, window):", "        now = time.time()", "        await self.redis.zremrangebyscore(key, 0, now - window)", "        count = await self.redis.zcard(key)", "        return Decision(allowed=count < limit, retry_after=window)",
  ]))] },
  { path: "cedar/settings.py", status: "M", add: 6, del: 2, changedBy: "Backend Dev", hunks: [hunk("@@ -40,4 +40,8 @@ EXPORT_TIMEOUT = 30", [ctx(40, 40, "EXPORT_TIMEOUT = 30"), del(41, "EXPORT_CHUNK = 4096"), add(41, "EXPORT_CHUNK = 8192"), add(42, "EXPORT_RATE_LIMIT = 60"), add(43, "EXPORT_RATE_WINDOW = 60")])] },
  { path: "tests/export/test_rate_limit.py", status: "A", add: 77, del: 0, changedBy: "Test Author", hunks: [hunk("@@ -0,0 +1,77 @@", adds(1, ["async def test_limit_returns_429_after_60_requests(client, workspace):", "    for _ in range(60):", "        assert (await client.export(workspace)).status == 200", "    blocked = await client.export(workspace)", "    assert blocked.status == 429", `    assert blocked.headers["Retry-After"]`]))] },
  { path: "tests/load/test_export_burst.py", status: "A", add: 22, del: 0, changedBy: "Test Author", hunks: [hunk("@@ -0,0 +1,22 @@", adds(1, ["async def test_burst_stays_under_budget(client):", "    results = await run_clients(client, count=200)", "    assert p95(results) < 0.300"]))] },
  { path: "docs/api/export.md", status: "M", add: 0, del: 11, changedBy: "Backend Dev", hunks: [hunk("@@ -12,14 +12,3 @@ ## Limits", [ctx(12, 12, "## Limits"), del(13, "Exports are not limited."), ctx(14, 13, "")])] },
];
const usageFiles = (): FileDiff[] => [
  { path: "cedar/usage/schema.sql", status: "A", add: 24, del: 0, changedBy: "Migration Dev", hunks: [hunk("@@ -0,0 +1,24 @@", adds(1, ["CREATE TABLE usage_events (", "    id BIGSERIAL PRIMARY KEY,", "    workspace_id TEXT NOT NULL,", "    kind TEXT NOT NULL,", "    at TIMESTAMPTZ NOT NULL", ");"]))] },
  { path: "cedar/usage/usage_api.py", status: "M", add: 188, del: 6, changedBy: "Backend Dev", hunks: [hunk("@@ -4,6 +4,9 @@ from .db import pool", [ctx(4, 4, "from .db import pool"), add(5, "from .events import record_event"), add(6, "from .rollups import daily_index"), ctx(5, 7, "")])] },
];

const check = (name: string, outcome: Check["outcome"], detail: string, source: Check["source"]): Check => ({ name, outcome, detail, source, ...(outcome === "not_run" ? { technicalReason: detail.replace("Not run: ", "") } : {}) });
const EXPORT_CHECKS: Check[] = [
  check("pytest tests/export -q", "passed", "Passed: 9 of 9 cases", "runtime"),
  check("ruff check .", "passed", "Passed: no findings", "runtime"),
  check("mypy cedar", "passed", "Passed", "runtime"),
  check("Limit returns 429 after 60 requests", "passed", "Passed: 9 of 9 cases", "runtime"),
  check("Independent review", "passed", "Approved, 1 remark", "runtime"),
  check("pytest tests/load/test_export_burst.py", "failed", "p95 was 410 ms, the limit is 300 ms", "runtime"),
  check("Full integration suite", "not_run", "Not run: test database was busy", "worker"),
  check("OpenAPI contract check", "not_run", "Not run: no schema tool on this machine", "runtime"),
  check("Redis failover keeps the counters", "unverified", "Unverified: worker says it passed, no command receipt", "worker"),
];
const USAGE_CHECKS: Check[] = [
  ...["pytest tests/usage -q", "ruff check .", "mypy cedar", "Migration applies cleanly", "Rollup index exists", "Independent review"].map(name => check(name, "passed", "Mock outcome, no command ran", "runtime")),
  check("Load test", "not_run", "Not run: no fixture on this machine", "worker"),
];
const RESOLVE_CHECKS: Check[] = [
  ...["pytest tests/usage -q", "ruff check .", "mypy cedar", "Rollups match the base totals", "Independent review"].map(name => check(name, "passed", "Mock outcome, no command ran", "runtime")),
  check("Load test", "not_run", "Not run: no fixture on this machine", "worker"),
];

const LOAD_BASE = [`def roll_up(day):`, `    """Sum a day."""`, `    rows = load(day)`, `    totals = sum_rows(rows)`, `    write_rollup(day, totals)`, `    return totals`];
const LOAD_OURS = LOAD_BASE.map(l => l.replace("load(day)", "load(day, n=500)"));
const LOAD_THEIRS = LOAD_BASE.map(l => l.replace("roll_up(day)", `roll_up(day, tz="UTC")`).replace("load(day)", "load(day, tz)"));
const LOAD_BOTH = LOAD_THEIRS.map(l => l.replace("load(day, tz)", "load(day, tz, n=500)"));
function rollupBlock(branch: string, target: string): ConflictBlock {
  const both = { start: 59, lines: LOAD_BOTH, hit: [2] };
  return {
    id: "load", title: "Both sides changed how events are loaded", choice: null, cortexNote: `Resolved automatically: kept tz from ${target} and n=500 from this branch.`,
    base: { ref: "a03be17", start: 56, lines: LOAD_BASE, hit: [2] }, ours: { ref: branch, start: 56, lines: LOAD_OURS, hit: [2] }, theirs: { ref: target, start: 59, lines: LOAD_THEIRS, hit: [0, 2] },
    results: { ours: { start: 56, lines: LOAD_OURS, hit: [2] }, theirs: { start: 59, lines: LOAD_THEIRS, hit: [0, 2] }, both, cortex: both },
  };
}
function schemaBlock(branch: string, target: string, choice: ConflictBlock["choice"]): ConflictBlock {
  const ours = ["CREATE TABLE usage_rollups (", "    day DATE NOT NULL,", "    total BIGINT NOT NULL,", "    tz TEXT NOT NULL", ");"], theirs = ["CREATE TABLE usage_rollups (", "    day DATE NOT NULL,", "    total BIGINT NOT NULL,", "    source TEXT", ");"];
  const both = ["CREATE TABLE usage_rollups (", "    day DATE NOT NULL,", "    total BIGINT NOT NULL,", "    tz TEXT NOT NULL,", "    source TEXT", ");"];
  return {
    id: "columns", title: "Both sides added a column", choice, cortexNote: "Resolved automatically: kept both columns.",
    base: { ref: "a03be17", start: 12, lines: ["CREATE TABLE usage_rollups (", "    day DATE NOT NULL,", "    total BIGINT NOT NULL", ");"], hit: [2] }, ours: { ref: branch, start: 12, lines: ours, hit: [3] }, theirs: { ref: target, start: 12, lines: theirs, hit: [3] },
    results: { ours: { start: 12, lines: ours, hit: [3] }, theirs: { start: 12, lines: theirs, hit: [3] }, both: { start: 12, lines: both, hit: [3, 4] }, cortex: { start: 12, lines: both, hit: [3, 4] } },
  };
}
export const usageConflict = (branch: string, target: string, schemaChoice: ConflictBlock["choice"]): MergeSeed => ({
  files: [{ path: "cedar/usage/usage_rollups.py", blocks: [rollupBlock(branch, target)] }, { path: "cedar/usage/schema.sql", blocks: [schemaBlock(branch, target, schemaChoice)] }],
  clean: [{ path: "rollup_jobs.py", status: "M" }, { path: "usage_api.py", status: "M" }, { path: "test_rollups.py", status: "A" }, { path: "settings.py", status: "M" }],
  checks: RESOLVE_CHECKS,
});
/** The older Amber Components conflict, built from its fixture file. */
export const layoutConflict = (branch: string, target: string): MergeSeed => {
  const side = (ref: string, value: string) => ({ ref, start: 1, lines: [`layout = ${value}`], hit: [0] });
  const result = (value: string) => ({ start: 1, lines: [`layout = ${value}`], hit: [0] });
  return {
    files: [{ path: "synthetic/layout.ts", blocks: [{ id: "layout", title: "Both sides changed the layout", choice: null, cortexNote: "Resolved automatically: kept this branch's value.", base: side("base", "compact"), ours: side(branch, "comfortable"), theirs: side(target, "focused"), results: { ours: result("comfortable"), theirs: result("focused"), both: result("comfortable"), cortex: result("comfortable") } }] }],
    clean: [], checks: [],
  };
};

const plain = (x = 14): Commit["graph"] => ({ paths: [{ cls: "g1", d: `M${x} 0V34` }], dot: { cls: "d1", x } });
const onLane = (start: boolean): Commit["graph"] => ({ paths: [{ cls: "g2", d: start ? "M34 17V34" : "M34 0V34" }, { cls: "g1", d: "M14 0V34" }], dot: { cls: "d2", x: 34 } });
const commit = (sha: string, message: string, author: string, when: string, branch: string, taskId: string | null, tags: string[], files: number, addN: number, delN: number, graph: Commit["graph"], pushed = true): Commit => ({ sha, message, author, when, branch, taskId, tags, files, add: addN, del: delN, pushed, graph });

function workers(task: Task): Worker[] {
  const base = { taskId: task.id, executionId: task.executionId! }, R = REVIEW_WORKERS;
  const done = (at: number, text: string) => [{ sequence: 1, kind: "note" as const, text, at, mock: true as const }];
  return [
    newWorker({ ...base, id: R.explorer, role: "Code Explorer", status: "completed", goal: "Find the export entry points.", route: { model: "claude-sonnet-5", effort: "low" }, attempt: { n: 1, of: 1 }, elapsedMs: 182000, usage: 14000, activity: done(T(5), "Found the export entry points"),
      result: { summary: "Export traffic enters through one handler in cedar/export/api.py.", checks: [], mock: true } }),
    newWorker({ ...base, id: R.backend, role: "Backend Dev", status: "completed", goal: "Add the Redis limiter.", route: { model: "opus[1m]", effort: "high" }, attempt: { n: 2, of: 2 }, elapsedMs: 18 * 60000, usage: 71000, activity: done(T(19), "Added the Redis limiter"),
      result: { summary: "Added a sliding window limiter in Redis and wired it into the export handler.", checks: [], mock: true, edits: [{ path: "cedar/export/api.py", add: 7, del: 1 }, { path: "cedar/export/limits.py", add: 8, del: 0 }], attempts: [{ n: 1, ok: false, detail: "interrupted: route unavailable", at: T(2) }, { n: 2, ok: true, detail: "finished", at: T(19) }] } }),
    newWorker({ ...base, id: R.author, role: "Test Author", status: "completed", goal: "Cover the limiter with contract and burst tests.", route: { model: "claude-sonnet-5", effort: "medium" }, attempt: { n: 1, of: 1 }, elapsedMs: 584000, usage: 33000, activity: done(T(21), "Submitted its result and report"),
      result: { summary: "Added contract tests for the 429 response and a burst test. Contract tests pass. The burst test exceeds the p95 limit under 200 parallel clients.", mock: true, edits: [{ path: "tests/export/test_rate_limit.py", add: 77, del: 0 }, { path: "tests/load/test_export_burst.py", add: 22, del: 0 }],
        checks: [check("Limit returns 429 after 60 requests", "passed", "Passed: 9 of 9 cases", "runtime"), check("ruff check tests/export", "passed", "Passed: no findings", "runtime"), check("pytest tests/load/test_export_burst.py", "failed", "p95 410 ms, 3 of 3 runs", "runtime"), check("Full integration suite", "not_run", "Not run: test database was busy", "worker")] } }),
    newWorker({ ...base, id: R.docs, role: "Docs Writer", status: "cancelled", goal: "Update the API reference.", route: { model: "claude-sonnet-5", effort: "low" }, attempt: { n: 1, of: 1 }, elapsedMs: 41000, usage: 2000, activity: done(T(8), "Cancelled by the coordinator: the plan no longer needed it"),
      result: { summary: "Cancelled by the coordinator. The reference is generated, so no edit was needed.", checks: [], mock: true } }),
    newWorker({ ...base, id: R.reviewer, role: "Security Reviewer", status: "completed", lane: "review", goal: "Independent review of the finished change.", route: { model: "opus[1m]", effort: "high" }, attempt: { n: 1, of: 1 }, elapsedMs: 380000, usage: 29000, activity: done(T(21), "Approved with 1 remark"),
      result: { summary: "No blocking findings. One remark: the limit key should not include the raw API token.", checks: [], mock: true } }),
  ];
}

type Parts = { session: Task; review: Task; merge: Task; next: Task; rollups: Task; merged: Task };
const worktree = (projectId: string, n: number, branch: string, taskId: string | null, extra: Partial<RepoCheckout> = {}): RepoCheckout => ({
  id: id("checkout", n), projectId, kind: "worktree", branch, path: `worktrees/${branch.replace(/^cw\//, "")}`, taskId, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, files: [], draft: "", sources: {}, updateConflict: null, ...extra,
});

function repo(parts: Parts): ProjectRepo {
  const projectId = parts.session.projectId, P = parts;
  const tree = [...Object.keys(SOURCES), "tests/auth/test_race.py"].sort();
  const mainFiles: GitFile[] = [
    { path: "README.md", status: "M", add: 2, del: 0, stage: "unstaged", hunks: [hunk("@@ -1,3 +1,5 @@", [ctx(1, 1, "# Cedar Notes"), ctx(2, 2, ""), add(3, "Run the app with make dev."), add(4, ""), ctx(3, 5, "Synthetic project for the Workspace prototype.")])] },
    { path: "docs/auth-runbook.md", status: "M", add: 1, del: 0, stage: "unstaged", hunks: [hunk("@@ -2,2 +2,3 @@", [ctx(2, 2, ""), ctx(3, 3, "Rotate refresh tokens with the admin tool."), add(4, "Rotations are logged to the audit stream.")])] },
  ];
  return {
    projectId, remote: "origin", tree, sources: SOURCES, fetchedAt: null, mock: true,
    commits: [
      commit("6be10a4", "Lock the refresh rotation", "Backend Dev", "02:41", P.session.checkout.branch, P.session.id, [P.session.checkout.branch], 2, 21, 5, onLane(true), false),
      commit("c31f0d2", "Add the failing race test", "Test Author", "02:30", P.session.checkout.branch, P.session.id, [], 1, 64, 0, onLane(false), false),
      commit("4f2a91c", "Merge pull request from cw/retry-after", "You", "01:30", "main", null, ["main"], 3, 40, 9, { paths: [{ cls: "g2", d: "M34 0C34 12 14 12 14 22" }, { cls: "g1", d: "M14 0V34" }], dot: { cls: "d1", x: 14 } }),
      commit("81ad7e0", "Respect Retry-After in the webhook client", "Backend Dev", "Yesterday", "cw/retry-webhooks", P.merged.id, [], 2, 18, 3, { paths: [{ cls: "g3", d: "M34 17V34" }, { cls: "g1", d: "M14 0V34" }], dot: { cls: "d3", x: 34 } }),
      commit("a03be17", "Add session cleanup job", "You", "Yesterday", "main", null, [], 2, 31, 2, { paths: [{ cls: "g3", d: "M34 0C34 12 14 12 14 22" }, { cls: "g1", d: "M14 0V34" }], dot: { cls: "d1", x: 14 } }),
      commit("19fd2c6", "Bump the billing client to 3.2", "You", "Dec 30", "main", null, [], 1, 2, 2, plain()),
      commit("e07b3a1", "Split usage events from invoices", "You", "Dec 29", "main", null, ["v2.4.0"], 6, 120, 88, plain()),
      commit("b8c4d11", "Add workspace scope to export", "You", "Dec 27", "main", null, [], 3, 44, 7, plain()),
      commit("2d90ef3", "Move refresh logic into auth", "You", "Dec 26", "main", null, [], 5, 210, 196, plain()),
      commit("6a71c08", "Remove the legacy session store", "You", "Dec 24", "main", null, [], 4, 0, 312, plain()),
    ],
    branches: [
      { name: "release/2.4", where: "origin/release/2.4", task: null, ahead: 0, behind: 12 },
      { name: "spike/backpressure", where: "Local only", task: "Spike: queue backpressure", ahead: 5, behind: 40 },
    ],
    checkouts: [
      { id: id("checkout", 0), projectId, kind: "main", branch: "main", path: "fictional/cedar-notes", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, files: mainFiles, draft: "", sources: {}, updateConflict: null },
      worktree(projectId, 1, P.session.checkout.branch, P.session.id, { path: "worktrees/fix-token-refresh", ahead: 2, files: tokenFiles(), draft: "Add token version to refresh rows", sources: { "auth/refresh.py": REFRESH, "tests/auth/test_race.py": RACE_TEST } }),
      worktree(projectId, 2, P.review.checkout.branch, P.review.id, { ahead: 3, behind: 1, behindOf: { main: 1, "release/2.4": 9 } }),
      worktree(projectId, 3, P.merge.checkout.branch, P.merge.id, { ahead: 2, behind: 3, behindOf: { main: 3, "release/2.4": 12 }, updateConflict: usageConflict(P.merge.checkout.branch, "main", null) }),
      worktree(projectId, 4, P.rollups.checkout.branch, P.rollups.id, { ahead: 4, behind: 3, behindOf: { main: 3 } }),
      worktree(projectId, 5, P.merged.checkout.branch, P.merged.id, {}),
      worktree(projectId, 6, "cw/old-import", null, { label: "No task", ahead: 2, behind: 7, files: [{ path: "docs/import-notes.md", status: "U", add: 12, del: 0, stage: "untracked", hunks: [] }] }),
    ],
  };
}

function reviews(parts: Parts): TaskChanges[] {
  const scoped = (taskId: string, sha: string, files: FileDiff[], last: string[], comments: TaskChanges["comments"] = []): TaskChanges => ({
    taskId, sha, files, comments, mock: true,
    scopes: [{ id: "turn", label: "Last turn", paths: last }, { id: "uncommitted", label: "Uncommitted", paths: [] }, { id: "branch", label: "Branch against base", paths: files.map(f => f.path) }],
  });
  const token = scoped(parts.session.id, "6be10a4", tokenFiles().map(({ stage: _stage, ...f }) => f), ["auth/refresh.py"]);
  token.scopes[1].paths = token.files.map(f => f.path);
  return [
    token,
    scoped(parts.review.id, "7c1e0b2", exportFiles(), ["tests/load/test_export_burst.py"], [{ id: id("comment", 0), path: "cedar/export/api.py", line: 25, text: "Should a blocked export also write an audit event?", sent: false, mock: true }]),
    scoped(parts.merge.id, "9be2a41", usageFiles(), ["cedar/usage/usage_api.py"]),
    scoped(parts.rollups.id, "5d1c7aa", [{ path: "cedar/usage/usage_rollups.py", status: "M", add: 64, del: 31, changedBy: "Backend Dev", hunks: [hunk("@@ -56,6 +56,6 @@ def roll_up(day):", [ctx(56, 56, "def roll_up(day):"), ctx(57, 57, `    """Sum a day."""`), del(58, "    rows = load(day)"), add(58, "    rows = load(day, n=500)"), ctx(59, 59, "    totals = sum_rows(rows)")])] }], ["cedar/usage/usage_rollups.py"]),
  ];
}

/** Four finished stages of work on one project: under review, ready to merge, in conflict, and merged. */
export function seedReview(state: WorkspaceState, parts: Parts) {
  const { session, review, merge, next, rollups, merged } = parts;
  const set = (task: Task, title: string, description: string, branch: string) => {
    task.title = title; task.titleState = "generated"; task.description = description;
    task.checkout = { kind: "worktree", branch, path: `worktrees/${branch.replace(/^cw\//, "")}` };
  };
  set(review, "Rate-limit the export endpoint", "Limit the data export endpoint to 60 requests per minute per workspace, and return a clear 429 with Retry-After when a client goes over.", "cw/export-rate-limit");
  review.status = "review"; review.executionId = id("execution", 245); review.workspaceDigest = "sha-7c1e0b2";
  review.result = { version: 2, digest: "sha-7c1e0b2", at: T(22), summary: "The export endpoint now allows 60 requests per minute per workspace and answers 429 with Retry-After beyond that. Counters live in Redis. One load check is over its limit.", checks: EXPORT_CHECKS, mock: true };
  state.workers.push(...workers(review));
  state.sessions.push({
    ...emptySession(review.id, T(2)), base: { branch: "main", commit: "a03be17" },
    plan: { revision: 1, note: "ran to the end", nodes: [{ id: "find", title: "Find the export entry points", who: "Code Explorer", workerId: REVIEW_WORKERS.explorer }, { id: "limit", title: "Add the Redis limiter", who: "Backend Dev", workerId: REVIEW_WORKERS.backend }, { id: "cover", title: "Cover it with tests", who: "Test Author", workerId: REVIEW_WORKERS.author }] },
    context: { used: 118000, window: 200000 }, tokens: 241000, coordinator: { note: "Coordinated 5 workers", elapsedMs: 41 * 60000, usage: 88000 }, verification: ["pytest tests/export -q", "ruff check .", "mypy cedar"], changes: { files: 6, add: 120, del: 14 },
    timeline: [
      { id: "r1", kind: "fold", text: "Worked 41m, 96 steps, 6 files", steps: ["Traced the export handler", "Added the limiter and its settings", "Ran review, then runtime verification"] },
      { id: "r2", kind: "line", icon: "question", text: "You answered: count per workspace, not per user", at: T(11), rule: false },
      { id: "r3", kind: "cortex", text: "Plan r1 ran to the end. Backend Dev added the Redis limiter, Test Author covered it, and Security Reviewer approved it. Runtime verification then ran the planned checks." },
    ],
  });

  set(merge, "Add the usage events table", "Store one row per billable event, with a daily rollup index.", "cw/usage-events");
  merge.status = "ready_to_merge"; merge.workspaceDigest = "sha-9be2a41";
  merge.result = { version: 1, digest: "sha-9be2a41", at: T(12), summary: "Stores one row per billable event, with a daily rollup index.", checks: USAGE_CHECKS, mock: true };
  merge.acceptance = { version: 1, digest: "sha-9be2a41", at: T(14), open: { failed: 0, not_run: 1, unverified: 0 } };
  next.title = "Chart usage on the billing page"; next.titleState = "generated"; next.description = "Chart usage on the billing page."; next.dependencies = [merge.id];
  const sprintId = id("sprint", 2);
  state.sprints.push({ id: sprintId, projectId: merge.projectId, description: "Usage billing", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], materializedVersion: 1, taskIds: [merge.id, next.id], queueState: "paused", pauseReason: "Waiting for you to merge Add the usage events table", mock: true,
    plan: { version: 1, items: [{ key: "events", description: merge.description, dependsOn: [] }, { key: "chart", description: next.description, dependsOn: ["events"] }] } });
  merge.sprintId = sprintId; next.sprintId = sprintId;
  state.sessions.push({ ...emptySession(merge.id, T(2)), base: { branch: "main", commit: "a03be17" }, tokens: 96000, changes: { files: 2, add: 212, del: 6 }, coordinator: { note: "Coordinated 3 workers", elapsedMs: 22 * 60000, usage: 41000 },
    timeline: [{ id: "m1", kind: "cortex", text: "The table and its rollup index are in. Only the load test could not run on this machine." }] });

  set(rollups, "Rebase usage rollups onto main", "Bring the usage rollups branch up to date with main and keep its batching change.", "cw/usage-rollups");
  rollups.status = "review"; rollups.workspaceDigest = "sha-5d1c7aa";
  rollups.result = { version: 1, digest: "sha-5d1c7aa", at: T(10), summary: "The rollup job reads events in batches of 500.", checks: [check("pytest tests/usage -q", "passed", "Mock outcome, no command ran", "runtime")], mock: true };
  state.sessions.push({ ...emptySession(rollups.id, T(1)), base: { branch: "main", commit: "a03be17" }, tokens: 74000, changes: { files: 6, add: 64, del: 31 }, coordinator: { note: "Coordinated 2 workers", elapsedMs: 15 * 60000, usage: 33000 } });

  set(merged, "Retry failed webhooks", "Retry failed webhooks and respect Retry-After.", "cw/retry-webhooks");
  merged.status = "done"; merged.work = "merged"; merged.doneAt = T(0) - 2 * 86400000; merged.workspaceDigest = "sha-81ad7e0";
  merged.result = { version: 1, digest: "sha-81ad7e0", summary: "Failed webhooks retry with backoff.", checks: [check("pytest tests/webhooks -q", "passed", "Mock outcome, no command ran", "runtime")], mock: true };
  merged.acceptance = { version: 1, digest: "sha-81ad7e0", at: null, open: null };
  merged.integration = { method: "squash", targetBranch: "main", digest: "sha-81ad7e0", mock: true };

  state.repos.push(repo(parts));
  state.reviews.push(...reviews(parts));
  beginMerge(state, rollups, usageConflict(rollups.checkout.branch, "main", "ours"), T(22));
  rollups.issue = "Merge conflict";
  session.executionId ??= id("execution", 244);
}
