import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const at = (path: string) => new URL(path, import.meta.url).pathname;

// --mode a/b/c reserves independent owner ports; no silent port fallback.
export default defineConfig(({ mode }) => ({
  plugins: [solid()],
  resolve: {
    // The demo entry builds the shared packages from source, against this app's one copy of Solid.
    alias: [
      { find: /^@cortex\/workspace-contract$/, replacement: at("../../../packages/workspace-contract/index.js") },
      { find: /^@cortex\/workspace-domain$/, replacement: at("../../../packages/workspace-domain/src/index.ts") },
      { find: /^@cortex\/workspace-ui$/, replacement: at("../../../packages/workspace-ui/src/index.ts") },
      { find: /^@cortex\/workspace-ui\/(.*)$/, replacement: at("../../../packages/workspace-ui/src/") + "$1" },
    ],
    dedupe: ["solid-js"],
  },
  server: { port: ({ a: 5181, b: 5182, c: 5183 } as Record<string, number>)[mode] ?? 5181, strictPort: true, fs: { allow: [at("../../..")] } },
  clearScreen: false,
}));
