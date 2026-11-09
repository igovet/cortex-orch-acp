import config from "../packages/workspace-ui/vite.config";

const at = (path: string) => new URL(path, import.meta.url).pathname;
const baseline = !!process.env.WORKSPACE_BUSY_BASELINE;
export default {
  ...config,
  resolve: {
    ...config.resolve,
    alias: [
      ...config.resolve!.alias as any[],
      { find: /^@busy\/port$/, replacement: at(baseline
        ? "../.workspace-build/service-busy/baseline-port.ts"
        : "../packages/workspace-ui/src/port/real.ts") },
      { find: /^@busy\/store$/, replacement: at(baseline
        ? "../.workspace-build/service-busy/baseline-store.ts"
        : "../packages/workspace-ui/src/store.ts") },
      { find: /^solid-js$/, replacement: at("../packages/workspace-ui/node_modules/solid-js/dist/solid.js") },
    ],
  },
  test: { ...config.test, include: ["../../tests/workspace-busy.test.ts"], environment: "jsdom" },
};
