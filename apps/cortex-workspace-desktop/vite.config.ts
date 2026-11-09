import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";
const at = (path: string) => fileURLToPath(new URL(path, import.meta.url));
export default defineConfig({
  plugins: [solid()],
  base: "./",
  resolve: {
    alias: [
      { find: /^@cortex\/workspace-contract$/, replacement: at("../../packages/workspace-contract/index.js") },
      { find: /^@cortex\/workspace-domain$/, replacement: at("../../packages/workspace-domain/src/index.ts") },
      { find: /^@cortex\/workspace-domain\/(.*)$/, replacement: at("../../packages/workspace-domain/src/") + "$1" },
    ],
    dedupe: ["solid-js"],
  },
  // Never inline an asset: the closed CSP allows fonts from 'self' only, so a data: font would silently not load.
  build: { outDir: "dist", emptyOutDir: true, assetsInlineLimit: 0 },
  test: { environment: "node", include: ["test/**/*.test.ts"] },
  clearScreen: false,
});
