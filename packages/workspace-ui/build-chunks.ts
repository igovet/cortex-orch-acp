// Split both desktop entries without raising Vite's 500 kB diagnostic threshold.
// Preserve source initialization order when modules move between chunks.
export const workspaceOutput = {
  strictExecutionOrder: true,
  codeSplitting: {
    maxSize: 350_000,
    groups: [
      { name: "vendor", test: /[/\\]node_modules[/\\]/ },
      { name: "workspace" },
    ],
  },
};
