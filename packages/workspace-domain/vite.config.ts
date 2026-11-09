import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const at = (path: string) => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  resolve: { alias: [{ find: /^@cortex\/workspace-contract$/, replacement: at("../workspace-contract/index.js") }] },
  clearScreen: false,
});
