import type { PluginServerContext } from "@getpaseo/plugin/server";
import { createCortexProvider } from "./server/provider.js";
import { getWorkerHistory } from "./server/worker-history.js";
import { readWorkerHistory } from "./shared/worker-history.js";
import { getPipeline } from "./server/pipeline.js";
import { readPipeline } from "./shared/pipeline.js";
import { getWorkerResults } from "./server/worker-results.js";
import { readWorkerResults } from "./shared/worker-results.js";
import { instrumentHandler } from "./server/invoke-diagnostics.js";
import { discoverOpenCode, onboardOpenCode, readConfig, updateConfig } from "./shared/config.js";
import { discoverOpenCodeHandler, onboardOpenCodeHandler, readConfigHandler, updateConfigHandler } from "./server/config.js";

export default function contribute(server: PluginServerContext) {
  server.registerProvider(createCortexProvider());
  server.handle(readWorkerHistory, instrumentHandler(readWorkerHistory.name, getWorkerHistory));
  server.handle(readPipeline, instrumentHandler(readPipeline.name, getPipeline));
  server.handle(readWorkerResults, instrumentHandler(readWorkerResults.name, getWorkerResults));
  // Cortex owner configuration (the Cortex JSON files and project store are
  // the source of truth; Paseo's plugin settings document is not used).
  server.handle(readConfig, instrumentHandler(readConfig.name, readConfigHandler));
  server.handle(updateConfig, instrumentHandler(updateConfig.name, updateConfigHandler));
  server.handle(discoverOpenCode, instrumentHandler(discoverOpenCode.name, discoverOpenCodeHandler));
  server.handle(onboardOpenCode, instrumentHandler(onboardOpenCode.name, onboardOpenCodeHandler));
  return () => {};
}
