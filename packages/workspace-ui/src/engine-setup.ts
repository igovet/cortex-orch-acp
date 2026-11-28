import { ENGINES, type EngineSetup } from "@cortex/workspace-domain";

const notices = new Map([
  ["downloading", "Preparing the qualified adapter…"],
  ["connecting", "Connecting to the native account…"],
  ["refreshing_models", "Discovering available models…"],
]);

/** The desktop poll returns machine-wide phases, independently of the open task and its route. */
export function engineSetupNotices(phases: Record<string, string> = {}) {
  const groups = new Map<string, string[]>();
  for (const engine of ENGINES) {
    const text = notices.get(phases[engine.id]);
    if (!text) continue;
    const group = groups.get(text) ?? [];
    group.push(engine.label);
    groups.set(text, group);
  }
  return [...groups].map(([text, engines]) => ({ engines: engines.join(" and "), text }));
}

const setupState = {
  not_downloaded: "Not downloaded", downloaded_no_login: "Downloaded, sign in on this machine",
  ready_to_connect: "Ready to connect", connected: "Connected", discovery_unavailable: "Discovery unavailable",
};

const setupStages = new Map([
  ["installing_adapter", "preparing the adapter"],
  ["reading_connections", "reading saved connections"],
  ["connecting", "connecting the native account"],
  ["checking_saved_catalog", "checking the saved model list"],
  ["refreshing_models", "refreshing available models"],
]);

export function engineSetupNote(setup: EngineSetup) {
  const phase = setup.automaticPhase;
  let detail: string | undefined;
  if (phase === "interrupted") {
    const next = setup.status === "not_downloaded" ? "Use Download engine to retry." :
      setup.status === "ready_to_connect" ? "Use Add connection to continue." :
      setup.status === "downloaded_no_login" ? "Sign in on this machine to continue." : "";
    detail = ["Last automatic setup was interrupted.", next].filter(Boolean).join(" ");
  } else if (phase === "setup_failed") {
    const stage = setupStages.get(setup.automaticStage ?? "");
    detail = stage ? `Last automatic setup failed while ${stage}.` : "Last automatic setup failed.";
  } else if (phase) {
    const info: Record<string, string> = {
      download_failed: "The last automatic adapter setup failed.", login_required: "The last automatic setup required native sign-in.",
      catalog_failed: "Automatic model discovery failed. Use Refresh models to retry.",
    };
    detail = info[phase] ?? notices.get(phase);
  }
  const reason = setup.reason && /^[a-z][a-z0-9_]{0,63}(?::[A-Za-z][A-Za-z0-9_]{0,63})?$/.test(setup.reason) ? setup.reason : null;
  return [setupState[setup.status], detail, reason, setup.autoConnectDisabled ? "Automatic connection is off." : ""].filter(Boolean).join(" · ");
}
