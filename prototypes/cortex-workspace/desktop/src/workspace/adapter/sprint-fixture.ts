import { applyCommand, captureProfile, generateMockTitle, mockConnect, newMachine, newMcpServer, setPin, setProvider, validatePlan, type Machine, type PlanItem, type Sprint, type Task, type WorkspaceState } from "../domain";

const MIN = 60000;
const worktree = (slug: string) => ({ kind: "worktree" as const, branch: `cw/${slug}`, path: `worktrees/${slug}` });
const AUDIT_PLAN: PlanItem[] = [
  { key: "table", title: "Add the audit events table", description: "Append-only table with a retention index.", acceptance: "Migration applies and rolls back on a copy", dependsOn: [], checkout: worktree("audit-events-table") },
  { key: "write", title: "Write audit events from the API", description: "Record who did what for every admin action.", acceptance: "Contract test for each admin route", dependsOn: ["table"], checkout: worktree("audit-events-write") },
  { key: "csv", title: "Stream the export as CSV", description: "Paged export that never loads the full table.", acceptance: "Stays under 200 MB on 1M rows", dependsOn: ["table"], checkout: worktree("audit-export-csv") },
  { key: "page", title: "Add the export page", description: "Date range, filters, download button.", acceptance: "Keyboard run through, empty and error states", dependsOn: ["csv"], checkout: worktree("audit-export-page") },
  { key: "docs", title: "Document the audit log", description: "Admin guide and the retention policy.", acceptance: "Links and commands checked against source", dependsOn: ["write", "page"], checkout: worktree("audit-log-docs") },
];

/** The approved planning screen: a conversation, Plan v2 beside it, and one question still open. */
export function seedAudit(state: WorkspaceState, projectId: string, now: number) {
  const id = applyCommand(state, { type: "create_sprint", projectId, description: "Admins need to export an audit log of who did what, with a date range, and we must keep it for a year.", runMode: "manual" })!;
  const sprint = state.sprints.find(s => s.id === id)!;
  sprint.title = "Audit log export"; sprint.settings = { model: "opus[1m]", effort: "high" };
  const at = (minutesAgo: number) => now - minutesAgo * MIN;
  sprint.conversation = [
    { kind: "note", text: "I read the admin routes and the export code. Two things change the plan, so I have two questions.", at: at(14), mock: true },
    { kind: "question", text: "How long should events be kept?", options: ["12 months", "24 months"], at: at(13), mock: true },
    { kind: "answer", text: "Keep events for 12 months", at: at(10), mock: true },
    { kind: "note", text: "Plan v2 splits the work into five tasks. The table comes first. The writer and the export do not depend on each other.", at: at(8), mock: true },
    { kind: "message", text: "Keep the export server side, no client paging.", at: at(5), mock: true },
    { kind: "question", text: "Should the export include failed login attempts?", options: ["Yes, all of them", "Only for admin accounts"], at: at(1), mock: true },
  ];
  validatePlan(AUDIT_PLAN);
  sprint.history = [{ version: 1, items: structuredClone(AUDIT_PLAN).slice(0, 4).map(i => ({ ...i, dependsOn: i.dependsOn.filter(d => d !== "docs") })) }];
  sprint.plan = { version: 2, items: structuredClone(AUDIT_PLAN) };
  sprint.planning = "question";
}

/** The real preset, as a machine in daily use has it: two servers on, one with a stale tool, one that cannot start, and a saved second profile. */
function seedOrchestration(local: Machine) {
  const c = local.config, at = Date.UTC(2030, 0, 1, 2, 25);
  for (const e of Object.values(c.catalogue)) e.observedAt = at;
  c.quotaObserved["codex-acp"] = [{ window: "seven_day", percent: 42 }];
  c.surfaces.user_defaults.ceiling.attempts = 8;
  c.surfaces.user_defaults.ceiling.stored.tokens = 2000000;
  c.files.preservedRouting = ["owner_note"]; c.files.preservedDefaults = ["project_defaults"];
  c.surfaces.routing.engines["claude-agent-acp"].enabled.push("claude-opus-4-1");
  c.catalogue["codex-acp"].models.find(x => x.id === "gpt-5.6-terra")!.observed = false;
  c.surfaces.routing.autoAllow = ["opencode/local/qwen"];
  const servers = c.surfaces.mcp_servers;
  servers[0].enabled = true; servers[1].enabled = true;
  servers.push({ ...newMcpServer(), name: "docs-search", command: "/opt/cortex-tools/docs-search/server", env: [{ name: "DOCS_API_KEY", stored: true, action: "keep", value: "" }], args: ["--token", "[redacted]"], enabled: true, network: true, allowedTools: ["search_docs", "get_page", "lookup_legacy"], readOnlyTools: ["search_docs", "get_page"], readPaths: ["/opt/cortex-tools/docs-search"] });
  servers.push({ ...newMcpServer(), name: "broken-exporter", command: "/opt/cortex-tools/broken-exporter/run", allowedTools: ["export_all"], readPaths: ["/opt/missing/exporter"] });
  servers.push({ ...newMcpServer(), name: "legacy-bridge", command: "/opt/cortex-tools/legacy-bridge/run", allowedTools: ["ping"], unsupportedKeys: ["timeout_ms"] });
  const known = mockConnect(servers[3], null, at);
  if (known.ok) c.mcpObserved["docs-search"] = known.observation;
  const seen = mockConnect(servers[0], null, at);
  if (seen.ok) c.mcpObserved["codebase-memory-mcp"] = seen.observation;
  c.profiles[0].sections = captureProfile(local);
  c.profiles[0].updatedAt = at;
  const rules = setPin(c.surfaces.routing, "worker-qa-engineer", { engine: "codex-acp", model: "gpt-6.1-terra", effort: "medium" });
  c.profiles.push({ id: "profile-night-runs", name: "night-runs", description: "Long unattended runs on Codex.", updatedAt: at, sections: { routing: setProvider(rules, "claude-agent-acp", false) } });
  c.surfaces.routing = setPin(c.surfaces.routing, "worker-security-auditor", { engine: "claude-agent-acp", model: "opus[1m]", effort: "high" });
}

/** Machines are explicit records. GPU box is the connected remote, so Inbox and Archive have more than one machine to cover. */
export function seedMachines(state: WorkspaceState, gpu: string) {
  const [local, offline] = state.machines;
  Object.assign(local, {
    latencyMs: null, diskMb: 1843, storage: [{ label: "Cedar Notes", mb: 1126 }, { label: "Orbit Ledger", mb: 410 }, { label: "Quartz Search", mb: 180 }, { label: "Other 3 projects", mb: 127 }],
    remoteAccess: { allowed: true, devices: [{ id: "device-1", name: "Work laptop", paired: "28 Dec", lastSeen: "09:40" }] },
    cleanup: { at: Date.UTC(2030, 0, 1, 2, 0), removed: 4, skipped: 2, asked: 1, freedMb: 212 }, diagnostics: { at: Date.UTC(2030, 0, 1, 1, 12), status: "ok", checks: 14 },
  });
  seedOrchestration(local);
  offline.version = "1.0.5";
  state.machines.push(newMachine({ id: gpu, name: "GPU box", kind: "remote", connection: "connected", latencyMs: 38, lastConfirmedAt: "2030-01-01T02:46:00.000Z", diskMb: 320, storage: [{ label: "Willow Archive", mb: 320 }] }));
  const gpuConfig = state.machines.at(-1)!.config;
  gpuConfig.files.migration = true; gpuConfig.surfaces.routing.legacyTiers = ["haiku: light"];
}

/** The sprint the summary mockup shows: one task at Ready to merge, one working, one next, one blocked. */
export function seedUsageBilling(state: WorkspaceState, merge: Task, chart: Task, meter: Task, invoice: Task, limits: Task) {
  const sprint = state.sprints.find(s => s.id === merge.sprintId) as Sprint;
  const name = (task: Task, title: string, branch: string) => {
    task.title = title; task.titleState = "generated"; task.description = `${title}.`; task.sprintId = sprint.id;
    task.checkout = { kind: "worktree", branch, path: `worktrees/${branch.slice(3)}` };
  };
  name(meter, "Meter API calls per workspace", "cw/meter-api-calls"); name(invoice, "Email a monthly invoice", "cw/invoice-email"); name(limits, "Add usage limits to plan settings", "cw/plan-limits");
  meter.status = "working"; meter.process = "live"; meter.dependencies = [merge.id];
  limits.dependencies = [meter.id];
  sprint.taskIds = [merge.id, meter.id, chart.id, invoice.id, limits.id];
  sprint.plan = { version: 1, items: [merge, meter, chart, invoice, limits].map((t, i) => ({ key: `u${i}`, title: t.title, description: t.description, dependsOn: i === 1 || i === 2 ? ["u0"] : i === 4 ? ["u1"] : [] })) };
  sprint.title = generateMockTitle(sprint.description);
  sprint.queueState = "running"; sprint.pauseReason = null;
}
