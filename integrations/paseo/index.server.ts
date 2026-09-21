import type { PluginServerContext } from "@getpaseo/plugin/server";
import { createCortexProvider } from "./server/provider.js";
import { getWorkerHistory } from "./server/worker-history.js";
import { readWorkerHistory } from "./shared/worker-history.js";

export default function contribute(server: PluginServerContext) {
  server.registerProvider(createCortexProvider());
  server.handle(readWorkerHistory, getWorkerHistory);
  return () => {};
}
