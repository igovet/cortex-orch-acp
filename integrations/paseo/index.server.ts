import type { PluginServerContext } from "@getpaseo/plugin/server";
import { createCortexProvider } from "./server/provider.js";
import { getWorkerHistory } from "./server/worker-history.js";
import { readWorkerHistory } from "./shared/worker-history.js";
import { getPipeline } from "./server/pipeline.js";
import { readPipeline } from "./shared/pipeline.js";
import { getWorkerResults } from "./server/worker-results.js";
import { readWorkerResults } from "./shared/worker-results.js";
import { instrumentHandler } from "./server/invoke-diagnostics.js";
import { readRunningCommands, requestCommandStop } from "./shared/command-stop.js";
import { getRunningCommands, stopCommandHandler } from "./server/command-stop.js";
import {
  applyDefaultPreset, discoverConnections, discoverOpenCode, installEngine, onboardConnection, onboardOpenCode, readConfig, readDefaultPreset,
  refreshModelCatalog, resetDefaultPreset, updateConfig,
} from "./shared/config.js";
import {
  applyDefaultPresetHandler, discoverConnectionsHandler, discoverOpenCodeHandler, installEngineHandler, onboardConnectionHandler, onboardOpenCodeHandler,
  readConfigHandler, readDefaultPresetHandler, refreshModelCatalogHandler, resetDefaultPresetHandler, updateConfigHandler,
} from "./server/config.js";

export default function contribute(server: PluginServerContext) {
  server.registerProvider(createCortexProvider());
  server.handle(readWorkerHistory, instrumentHandler(readWorkerHistory.name, getWorkerHistory));
  server.handle(readPipeline, instrumentHandler(readPipeline.name, getPipeline));
  server.handle(readWorkerResults, instrumentHandler(readWorkerResults.name, getWorkerResults));
  // A user's stop of one running worker command, with a required reason.
  server.handle(readRunningCommands, instrumentHandler(readRunningCommands.name, getRunningCommands));
  server.handle(requestCommandStop, instrumentHandler(requestCommandStop.name, stopCommandHandler));
  // Cortex owner configuration (the Cortex JSON files and project store are
  // the source of truth; Paseo's plugin settings document is not used).
  server.handle(readConfig, instrumentHandler(readConfig.name, readConfigHandler));
  server.handle(updateConfig, instrumentHandler(updateConfig.name, updateConfigHandler));
  server.handle(discoverOpenCode, instrumentHandler(discoverOpenCode.name, discoverOpenCodeHandler));
  server.handle(onboardOpenCode, instrumentHandler(onboardOpenCode.name, onboardOpenCodeHandler));
  server.handle(refreshModelCatalog, instrumentHandler(refreshModelCatalog.name, refreshModelCatalogHandler));
  // First-run setup: engine status, pinned adapter download, native-account
  // connection (explicit opt-in only) and the bundled default preset.
  server.handle(discoverConnections, instrumentHandler(discoverConnections.name, discoverConnectionsHandler));
  server.handle(onboardConnection, instrumentHandler(onboardConnection.name, onboardConnectionHandler));
  server.handle(installEngine, instrumentHandler(installEngine.name, installEngineHandler));
  server.handle(readDefaultPreset, instrumentHandler(readDefaultPreset.name, readDefaultPresetHandler));
  server.handle(applyDefaultPreset, instrumentHandler(applyDefaultPreset.name, applyDefaultPresetHandler));
  server.handle(resetDefaultPreset, instrumentHandler(resetDefaultPreset.name, resetDefaultPresetHandler));
  return () => {};
}
