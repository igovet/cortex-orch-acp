import { NEUTRAL_POLICY, newMcpServer, openCeiling } from "@cortex/workspace-domain";
import type { EngineCatalogue, EngineKey, EngineRules, McpServer, McpTool, ModelPolicy, RoutingRules, UserDefaults } from "./types";

/**
 * What the mock machine starts with, mirrored from policy/model-settings-default.json. A test keeps this equal to that file.
 * Paths are synthetic: the real preset has placeholders there.
 */
const E4 = ["low", "medium", "high", "xhigh"], E5 = [...E4, "max"];
const m = (id: string, label: string, efforts = E4, observed = true) => ({ id, label, efforts, observed });
/** What the engines report in the fixture. It includes models the preset denies, as a real catalogue does. */
export const CATALOGUE_MODELS = {
  "claude-agent-acp": [m("default", "Default"), m("opus[1m]", "Opus"), m("claude-fable-5-1[1m]", "Fable"), m("claude-sonnet-5", "Sonnet"), m("haiku", "Haiku", E5), m("claude-opus-4-8", "Opus 4.8"), m("claude-sonnet-4-6", "Sonnet 4.6"), m("claude-haiku-4-5-20251001", "Haiku 4.5", E5)],
  "codex-acp": [m("gpt-6.1-sol", "GPT-6.1 Sol"), m("gpt-6-astra", "GPT-6 Astra"), m("gpt-6-luna", "GPT-6 Luna", E5), m("gpt-6.1-terra", "GPT-6.1 Terra"), m("gpt-6-sol", "GPT-6 Sol"), m("gpt-5.6-terra", "GPT-5.6 Terra")],
  opencode: [],
};
export const newCatalogue = (): Record<EngineKey, EngineCatalogue> => {
  const of = (id: EngineKey): EngineCatalogue => ({ observedAt: null, failed: false, models: structuredClone(CATALOGUE_MODELS[id]) });
  return { "claude-agent-acp": of("claude-agent-acp"), "codex-acp": of("codex-acp"), opencode: of("opencode") };
};

const pol = (p: Partial<ModelPolicy>): ModelPolicy => ({ ...NEUTRAL_POLICY, ...p });
const emptyEngine = (): EngineRules => ({ enabled: [], auto: [], tiers: {}, policy: {}, quota: [] });
const OPUS_WHEN = "Use for the listed kinds. Always use for implementation_frontend_design and UI/UX design: layout, styling, markup, responsive layout, UI components, visual polish, UX flows, mockups, wireframes and design systems. Never mark such nodes low complexity: that leaves Opus ineligible. For implementation_backend, use only for very complex, narrowly scoped tasks.";
const LIGHT_WORK = ["analysis", "writing", "implementation_backend", "implementation_frontend", "debugging", "investigation", "operations"];

export const presetRouting = (): RoutingRules => ({
  version: 2,
  deny: ["gpt-5.5", "claude-agent-acp/claude-haiku-4-5-20251001", "claude-agent-acp/claude-sonnet-4-6", "claude-agent-acp/claude-opus-4-6", "claude-agent-acp/claude-opus-4-7", "claude-agent-acp/claude-opus-4-8", "claude-agent-acp/claude-opus-5[1m]", "claude-agent-acp/claude-fable-5", "codex-acp/gpt-5.6-luna", "codex-acp/gpt-5.6-sol", "codex-acp/gpt-5.6-terra", "codex-acp/gpt-6-sol", "opencode/*"],
  providers: ["claude-agent-acp", "codex-acp"],
  engines: {
    "claude-agent-acp": {
      enabled: ["default", "opus[1m]", "claude-fable-5-1[1m]", "claude-sonnet-5", "haiku"],
      auto: ["default", "opus[1m]", "claude-fable-5-1[1m]", "claude-sonnet-5", "haiku"],
      tiers: { "claude-fable-5-1[1m]": ["flagship"], "claude-sonnet-5": ["standard", "deep"], default: ["deep"], haiku: ["light"], "opus[1m]": ["flagship", "deep"] },
      policy: {
        "claude-fable-5-1[1m]": pol({ effortFloor: "low", maxEffort: "high", preferredFor: ["design"], requestNames: ["fable", "фейбл", "фэйбл"] }),
        "claude-sonnet-5": pol({ effortFloor: "medium", maxEffort: "xhigh", preferredFor: ["implementation_backend", "implementation_frontend", "investigation", "review", "analysis", "operations"] }),
        default: pol({ effortFloor: "medium", maxEffort: "xhigh" }),
        haiku: pol({ alsoStandardFor: ["testing"], effortFloor: "high", effortLadder: ["high", "xhigh", "max"], maxEffort: "max", preferredFor: LIGHT_WORK }),
        "opus[1m]": pol({ effortFloor: "medium", maxEffort: "xhigh", preferredFor: ["design", "planning", "implementation_frontend_design", "debugging", "analysis", "implementation_backend", "implementation_frontend"], preferredWhen: OPUS_WHEN, requestNames: ["opus", "опус"] }),
      },
      quota: [{ window: "seven_day_opus", models: ["opus[1m]"] }, { window: "seven_day_sonnet", models: ["claude-sonnet-5"] }, { window: "seven_day_fable", models: ["claude-fable-5-1[1m]"] }, { window: "seven_day_haiku", models: ["haiku"] }],
    },
    "codex-acp": {
      enabled: ["gpt-6.1-sol", "gpt-6-astra", "gpt-6-luna", "gpt-6.1-terra"],
      auto: ["gpt-6.1-sol", "gpt-6-astra", "gpt-6-luna", "gpt-6.1-terra"],
      tiers: { "gpt-6-astra": ["flagship"], "gpt-6-luna": ["light"], "gpt-6.1-sol": ["standard", "deep"], "gpt-6.1-terra": ["standard"] },
      policy: {
        "gpt-6-astra": pol({ effortFloor: "low", maxEffort: "high", preferredFor: ["design"], requestNames: ["astra", "астр"] }),
        "gpt-6-luna": pol({ alsoStandardFor: ["testing"], effortFloor: "high", effortLadder: ["high", "xhigh", "max"], maxEffort: "max", preferredFor: LIGHT_WORK }),
        "gpt-6.1-sol": pol({ effortFloor: "medium", maxEffort: "xhigh", preferredFor: ["implementation_backend", "implementation_frontend", "planning", "design", "review", "operations", "analysis", "investigation"] }),
        "gpt-6.1-terra": pol({ effortFloor: "medium", maxEffort: "high", preferredFor: ["writing", "testing", "debugging"] }),
      },
      quota: [],
    },
    opencode: emptyEngine(),
  },
  pins: {}, affinity: {}, autoAllow: [], legacyTiers: [],
});

export const presetDefaults = (): UserDefaults => ({
  coordinator: { engine: "claude-agent-acp", model: "opus[1m]", effort: "medium", parameters: {} }, routeDefault: null,
  language: "ru", mode: "development", checkout: "worktree", ceiling: openCeiling(), preserved: [],
});

const CBM_READ = ["search_graph", "query_graph", "trace_path", "get_code_snippet", "get_graph_schema", "get_architecture", "search_code", "list_projects", "index_status", "check_index_coverage", "detect_changes"];
export const presetServers = (): McpServer[] => [
  {
    ...newMcpServer(), name: "codebase-memory-mcp", command: "/opt/cortex-tools/codebase-memory-mcp/bin/codebase-memory-mcp",
    allowedTools: ["index_repository", "search_graph", "query_graph", "trace_path", "get_code_snippet", "get_graph_schema", "get_architecture", "search_code", "list_projects", "delete_project", "index_status", "check_index_coverage", "detect_changes", "manage_adr", "ingest_traces"],
    readOnlyTools: [...CBM_READ], coordinatorTools: ["search_graph", "search_code", "get_architecture", "list_projects", "index_status", "check_index_coverage", "trace_path", "get_code_snippet"], alwaysLoadTools: [...CBM_READ],
    readPaths: ["/opt/cortex-tools/codebase-memory-mcp"], writePaths: ["/opt/cortex-tools/codebase-memory-mcp"], runtimeDirEnv: ["CBM_RUNTIME_DIR"],
  },
  {
    ...newMcpServer(), name: "playwright", command: "/usr/bin/node", args: ["/opt/cortex-tools/playwright/cli.js", "--headless", "--isolated", "--browser", "chromium", "--executable-path", "/opt/chrome/chrome"],
    roles: ["worker"], engines: ["claude-agent-acp"], network: true, runtimeDirEnv: ["PLAYWRIGHT_MCP_OUTPUT_DIR"], readPaths: ["/usr/bin/node", "/opt/cortex-tools/playwright", "/usr/share/fonts", "/opt/chrome"],
    allowedTools: ["browser_close", "browser_resize", "browser_console_messages", "browser_handle_dialog", "browser_evaluate", "browser_file_upload", "browser_drop", "browser_find", "browser_fill_form", "browser_press_key", "browser_type", "browser_navigate", "browser_navigate_back", "browser_network_requests", "browser_network_request", "browser_take_screenshot", "browser_snapshot", "browser_click", "browser_drag", "browser_hover", "browser_select_option", "browser_tabs", "browser_wait_for", "browser_webmcp_list", "browser_webmcp_call"],
  },
  {
    ...newMcpServer(), name: "vivaldi", command: "/usr/bin/node", args: ["/opt/cortex-tools/vivaldi/mcp-server-stdio.js"], roles: ["worker"], network: true, readPaths: ["/opt/cortex-tools/vivaldi", "/usr/bin/node"],
    allowedTools: ["get_windows_and_tabs", "chrome_navigate", "chrome_screenshot", "chrome_close_tabs", "chrome_switch_tab", "chrome_get_web_content", "chrome_javascript", "chrome_click_element", "chrome_fill_or_select", "chrome_keyboard", "chrome_handle_dialog"],
    readOnlyTools: ["get_windows_and_tabs", "chrome_get_web_content"],
  },
];
/** Ready-made starts for Add server. */
export const SERVER_TEMPLATES = () => presetServers().map(s => ({ ...s, enabled: false, allowedTools: [], readOnlyTools: [], coordinatorTools: null, alwaysLoadTools: null }));

const t = (name: string, description: string, hint: McpTool["hint"] = null): McpTool => ({ name, description, hint });
/** What the simulated connect lists, per server. Hints are claims a server may or may not make. */
export const MOCK_SERVER_TOOLS: { match: string; name: string; version: string; tools: McpTool[] }[] = [
  { match: "codebase-memory", name: "codebase-memory-mcp", version: "0.9.3", tools: [
    t("index_repository", "Build the index"), t("search_graph", "Find symbols in the index", "read"), t("query_graph", "Run a graph query", "read"), t("trace_path", "Follow callers and callees", "read"), t("get_code_snippet", "Read a symbol's source", "read"),
    t("get_graph_schema", "Describe the graph", "read"), t("get_architecture", "Summarise the architecture", "read"), t("search_code", "Search text in the code", "read"), t("list_projects", "List indexed projects", "read"), t("delete_project", "Remove a project's index", "destructive"),
    t("index_status", "Read index health", "read"), t("check_index_coverage", "Check what the index covers", "read"), t("detect_changes", "Map a diff to its impact", "read"), t("manage_adr", "Write decision records"), t("ingest_traces", "Load runtime traces"),
  ] },
  { match: "playwright", name: "playwright-mcp", version: "0.2.1", tools: [
    t("browser_close", "Close the page"), t("browser_resize", "Resize the window"), t("browser_console_messages", "Read console messages", "read"), t("browser_handle_dialog", "Answer a dialog"), t("browser_evaluate", "Run JavaScript on the page"), t("browser_file_upload", "Upload files"),
    t("browser_drop", "Drop onto an element"), t("browser_find", "Find text on the page", "read"), t("browser_fill_form", "Fill a form"), t("browser_press_key", "Press a key"), t("browser_type", "Type text"), t("browser_navigate", "Open a URL"), t("browser_navigate_back", "Go back"),
    t("browser_network_requests", "List network requests", "read"), t("browser_network_request", "Read one request", "read"), t("browser_take_screenshot", "Capture the page", "read"), t("browser_snapshot", "Read the accessibility tree", "read"), t("browser_click", "Click an element"), t("browser_drag", "Drag an element"),
    t("browser_hover", "Hover an element"), t("browser_select_option", "Pick a select option"), t("browser_tabs", "Manage tabs"), t("browser_wait_for", "Wait for text"), t("browser_webmcp_list", "List page tools", "read"), t("browser_webmcp_call", "Call a page tool"),
    t("browser_pdf_save", "Save the page as PDF"), t("browser_install", "Install the browser", "destructive"),
  ] },
  { match: "vivaldi", name: "vivaldi-mcp", version: "1.4.0", tools: [
    t("get_windows_and_tabs", "List windows and tabs", "read"), t("chrome_navigate", "Open a URL"), t("chrome_screenshot", "Capture a tab"), t("chrome_close_tabs", "Close tabs", "destructive"), t("chrome_switch_tab", "Switch tab"), t("chrome_get_web_content", "Read a tab's content", "read"),
    t("chrome_javascript", "Run JavaScript in a tab"), t("chrome_click_element", "Click an element"), t("chrome_fill_or_select", "Fill a field"), t("chrome_keyboard", "Send keys"), t("chrome_handle_dialog", "Answer a dialog"),
  ] },
  { match: "docs-search", name: "docs-search", version: "2.0.0", tools: [
    t("search_docs", "Search the documentation", "read"), t("get_page", "Read one page", "read"), t("get_index", "List every page", "read"),
  ] },
];
export const MOCK_GENERIC_TOOLS: McpTool[] = [t("list_items", "List items", "read"), t("get_item", "Read one item", "read"), t("write_item", "Change an item"), t("delete_item", "Delete an item", "destructive")];
