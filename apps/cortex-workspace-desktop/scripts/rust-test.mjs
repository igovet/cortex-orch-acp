import { spawnSync } from "node:child_process";
import { homedir } from "node:os";
import { fileURLToPath } from "node:url";
const cwd = fileURLToPath(new URL("../../..", import.meta.url));
const result = spawnSync(homedir() + "/.cargo/bin/cargo", ["test", "--locked", "-p", "cortex-workspace-desktop", "--no-default-features"], { cwd, stdio: "inherit" });
process.exit(result.status ?? 1);
