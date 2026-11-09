import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";

const at = (path: string) => fileURLToPath(new URL(path, import.meta.url));

/** `vite build` here is the production entry: the real port, with no demo, simulator or fixture anywhere in its graph. */
export default defineConfig({
  root: at("production"),
  plugins: [solid()],
  resolve: {
    alias: [
      { find: /^@cortex\/workspace-contract$/, replacement: at("../workspace-contract/index.js") },
      { find: /^@cortex\/workspace-domain$/, replacement: at("../workspace-domain/src/index.ts") },
      { find: /^@cortex\/workspace-domain\/(.*)$/, replacement: at("../workspace-domain/src/") + "$1" },
    ],
    dedupe: ["solid-js"],
  },
  build: { outDir: at("dist"), emptyOutDir: true },
  test: { root: at("."), include: ["test/**/*.test.{ts,tsx}"] },
  clearScreen: false,
});
