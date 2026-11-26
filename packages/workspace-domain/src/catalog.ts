import type { Ceiling, EngineId, EngineKey, McpServer, ModelPolicy, PlanComplexity, RoleAccess, SessionChoice, Tier } from "./types";

/** The fixed facts of the runtime: engines, tiers, kinds of work and roles. What a machine is set to lives on the machine. */
export const ENGINES: readonly { id: EngineKey; label: string; usage: "claude" | "codex" | null; recipient: string }[] = [
  { id: "claude-agent-acp", label: "Claude", usage: "claude", recipient: "anthropic" },
  { id: "codex-acp", label: "Codex", usage: "codex", recipient: "openai" },
  { id: "opencode", label: "OpenCode", usage: null, recipient: "opencode" },
];
export const ENGINE_KEYS = ENGINES.map(e => e.id);
/** What a connection calls its engine, as the routing rules call it. */
export const CONNECTION_ENGINE: Record<EngineId, EngineKey> = { claude: "claude-agent-acp", codex: "codex-acp", opencode: "opencode" };
export const engineLabel = (id: EngineKey) => ENGINES.find(e => e.id === id)?.label ?? id;
export const TIERS: readonly Tier[] = ["flagship", "deep", "standard", "light"];
export const TIER_LABELS: Record<Tier, string> = { flagship: "Flagship", deep: "Deep", standard: "Standard", light: "Light" };
export const EFFORT_LEVELS = ["low", "medium", "high", "xhigh", "max"] as const;
export const EFFORT_LABELS: Record<string, string> = { low: "Low", medium: "Medium", high: "High", xhigh: "Extra high", max: "Max" };
export const isConcreteEffort = (id: string) => !["", "default", "engine_default"].includes(id.trim().toLowerCase());
const effortRank = (id: string) => {
  const rank = EFFORT_LEVELS.findIndex(level => level === id.toLowerCase());
  return rank < 0 ? id.toLowerCase() === "ultra" ? EFFORT_LEVELS.length : EFFORT_LEVELS.length + 1 : rank;
};
export function orderedEfforts(ids: readonly string[]): string[] {
  return [...new Set(ids.filter(isConcreteEffort))].sort((a, b) => effortRank(a) - effortRank(b) || a.localeCompare(b));
}
/** Missing or legacy choices inherit Settings, then medium. Prefer the lower effort on ties. */
export function resolveEffort(chosen = "", setting = "", offered?: readonly string[]): string {
  const preferred = isConcreteEffort(chosen) ? chosen : isConcreteEffort(setting) ? setting : "medium";
  if (!offered) return preferred;
  const levels = orderedEfforts(offered);
  if (levels.includes(preferred)) return preferred;
  const rank = effortRank(preferred);
  const target = rank > EFFORT_LEVELS.length ? effortRank("medium") : rank;
  return levels.reduce((best, level) =>
    !best || Math.abs(effortRank(level) - target) < Math.abs(effortRank(best) - target) ? level : best, "");
}
export const effortLabel = (id: string) => EFFORT_LABELS[id] ?? id;
export const COMPLEXITY_LABELS: Record<PlanComplexity, string> = { low: "Low", normal: "Normal", high: "High" };
export const EFFORT_BY_TIER: Record<Tier, string> = { flagship: "high", deep: "high", standard: "medium", light: "low" };
export const FALLBACK_ORDER: Record<Tier, Tier[]> = {
  flagship: ["flagship", "deep", "standard", "light"], deep: ["deep", "standard", "light"], standard: ["standard", "deep", "light"], light: ["light", "standard", "deep"],
};
export const HOT_PERCENT = 95;
export const RANK_ORDER = ["Preference", "Purpose", "Hint", "Quota", "Retry on another engine", "Affinity", "Used percent", "Engine order"];
export const HARD_DENIALS = ["cancellation", "refusal", "authorization_loss", "provider_restriction"];
export const FAILURE_CLASSES = [{ id: "engine_failure", label: "The engine failed" }, { id: "capacity_unavailable", label: "No capacity" }];
export const PERMISSION_LEVELS = ["READ_ONLY", "STANDARD", "ELEVATED"];

type Kind = { id: string; label: string; lean: EngineKey | null; bound?: boolean };
export const WORK_KINDS: readonly Kind[] = [
  { id: "implementation_backend", label: "Backend implementation", lean: "codex-acp" },
  { id: "implementation_frontend", label: "Frontend implementation", lean: "codex-acp" },
  { id: "implementation_frontend_design", label: "Frontend design implementation", lean: "claude-agent-acp", bound: true },
  { id: "operations", label: "Operations", lean: "codex-acp" },
  { id: "debugging", label: "Debugging", lean: null },
  { id: "testing", label: "Testing", lean: null },
  { id: "investigation", label: "Investigation", lean: "claude-agent-acp" },
  { id: "review", label: "Review", lean: "claude-agent-acp" },
  { id: "design", label: "Design", lean: "claude-agent-acp" },
  { id: "writing", label: "Writing", lean: "claude-agent-acp" },
  { id: "planning", label: "Planning", lean: "claude-agent-acp" },
  { id: "analysis", label: "Analysis", lean: "claude-agent-acp" },
  { id: "consultation", label: "Senior consultation", lean: "claude-agent-acp" },
];
export const kindLabel = (id: string) => WORK_KINDS.find(k => k.id === id)?.label ?? id;

type Role = { id: string; label: string; access: RoleAccess; kind: string; base: Tier };
const role = (id: string, label: string, access: RoleAccess, kind: string, base: Tier = "standard"): Role => ({ id: `worker-${id}`, label, access, kind, base });
export const ROLE_CATALOG: readonly Role[] = [
  role("accessibility-fixer", "Accessibility Fixer", "writing", "implementation_frontend"),
  role("backend-dev", "Backend Dev", "writing", "implementation_backend"),
  role("data-engineer", "Data Engineer", "writing", "implementation_backend"),
  role("debugger", "Debugger", "writing", "debugging"),
  role("devops-engineer", "DevOps Engineer", "writing", "operations"),
  role("frontend-dev", "Frontend Dev", "writing", "implementation_frontend"),
  role("fullstack-dev", "Fullstack Dev", "writing", "implementation_backend"),
  role("general", "General", "writing", "implementation_backend"),
  role("mobile-dev", "Mobile Dev", "writing", "implementation_frontend"),
  role("qa-engineer", "QA Engineer", "writing", "testing"),
  role("refactorer", "Refactorer", "writing", "implementation_backend"),
  role("technical-writer", "Technical Writer", "writing", "writing"),
  role("accessibility-auditor", "Accessibility Auditor", "read_only", "review"),
  role("architect", "Architect", "read_only", "design", "deep"),
  role("build-verification", "Build Verification", "read_only", "testing"),
  role("code-reviewer", "Code Reviewer", "read_only", "review"),
  role("database-architect", "Database Architect", "read_only", "design", "deep"),
  role("explorer", "Explorer", "read_only", "investigation", "light"),
  role("performance-engineer", "Performance Engineer", "read_only", "debugging"),
  role("planner", "Planner", "read_only", "planning"),
  role("security-auditor", "Security Auditor", "read_only", "review", "deep"),
  role("ux-designer", "UX Designer", "read_only", "design"),
  role("senior-consultant", "Senior Consultant", "consultant", "consultation", "deep"),
];
export const RESERVED_ROLE = "worker-build-verification";
export const ACCESS_LABELS: Record<RoleAccess, string> = { writing: "Writing", read_only: "Read-only", consultant: "Consultant" };
export const RESERVED_WORDS = ["auto", "default", "model", "best", "fast", "cheap"];
export const NEUTRAL_POLICY: ModelPolicy = { effortFloor: null, maxEffort: null, effortLadder: null, alsoStandardFor: null, preferredFor: null, preferredWhen: null, requestNames: null, tierEfforts: null };
export const openCeiling = (): Ceiling => ({
  allowedRoutes: null, allowedConnections: null, allowedRecipients: null, attempts: null, failureClasses: null, targetRoutes: null, budget: null, consent: false,
  stored: { permissionLevel: null, calls: null, tokens: null, concurrency: null, spendLimit: null, spendEnforceable: false },
});
export const newMcpServer = (): McpServer => ({
  name: "", command: "", args: [], env: [], enabled: false, roles: null, engines: null, allowedTools: [], readOnlyTools: [], coordinatorTools: null, alwaysLoadTools: null,
  readPaths: [], writePaths: [], network: false, runtimeDirEnv: [], handTools: [], unsupportedKeys: [], problem: null, containment: "none",
});
/** Access modes and message languages are the runtime's words. Which of them a session offers is in its options. */
export const ACCESS_MODES = [
  // What each mode asks is about engine file tools; a command can read anything the owner's account can in every mode.
  { id: "development", label: "Development", sub: "Asks before deletions, engine writes outside the project and key files. Writing workers' commands read and write anywhere without asking" },
  { id: "guarded", label: "Guarded", sub: "Engine tools ask before reading or writing outside the project. Commands can read, not write, anywhere without asking" },
  { id: "auto", label: "Auto", sub: "Nothing asks you; each step is recorded. Writing workers' commands read and write anywhere" },
] as const satisfies readonly SessionChoice[];
export const LANGUAGES = [{ id: "auto", label: "Auto" }, { id: "en", label: "English" }, { id: "ru", label: "Russian" }] as const satisfies readonly SessionChoice[];
