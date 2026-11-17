import { beforeAll, expect, it } from "vitest";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../..", import.meta.url));
const cargo = join(process.env.CARGO_HOME ?? join(homedir(), ".cargo"), "bin/cargo");
const xtask = join(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "debug/cortex-xtask");
const desktops = ["apps/cortex-workspace-desktop", "prototypes/cortex-workspace/desktop"];
const editor = "packages/workspace-ui";

beforeAll(() => {
  const built = spawnSync(cargo, ["build", "--locked", "--offline", "-p", "cortex-xtask"], { cwd: root, encoding: "utf8" });
  expect(built.status, built.stdout + built.stderr).toBe(0);
}, 600_000);

function checkPins(directory: string) {
  return spawnSync(xtask, ["check-pins", "--root", directory], { encoding: "utf8" });
}

/** A complete, otherwise valid copy of every input the pin check reads. */
function fixture(): string {
  const target = mkdtempSync(join(tmpdir(), "cw-pin-check-"));
  const manifest = readFileSync(join(root, "Cargo.toml"), "utf8");
  const members = [...(/members = \[([^\]]*)\]/.exec(manifest)?.[1] ?? "").matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  const files = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "pinned-dependencies.json"];
  files.push(...members.map((member) => `${member}/Cargo.toml`));
  files.push(...[...desktops, editor].flatMap((dir) => ["package.json", "package-lock.json"].map((name) => `${dir}/${name}`)));
  for (const name of files) {
    mkdirSync(dirname(join(target, name)), { recursive: true });
    copyFileSync(join(root, name), join(target, name));
  }
  return target;
}

it("accepts this checkout and a complete copy of its pin inputs", () => {
  const live = checkPins(root);
  expect(live.status, live.stdout + live.stderr).toBe(0);
  const copy = fixture();
  try {
    const result = checkPins(copy);
    expect(result.status, result.stdout + result.stderr).toBe(0);
  } finally {
    rmSync(copy, { recursive: true, force: true });
  }
});

it.each([
  ["apps/cortex-workspace-desktop/src-tauri/Cargo.toml", "=2.10.1", "^2.10.1", "exact =x.y.z"],
  ["apps/cortex-workspace-desktop/package.json", '"@tauri-apps/cli": "2.10.1"', '"@tauri-apps/cli": "2.10.2"', "reviewed exact pin"],
  ["Cargo.toml", 'default-members = ["crates/workspace-protocol"]', 'default-members = ["apps/cortex-workspace-desktop/src-tauri"]', "not a default member"],
])("detects production pin drift in %s", (name, from, to, diagnostic) => {
  const copy = fixture();
  try {
    const path = join(copy, name);
    const original = readFileSync(path, "utf8");
    expect(original).toContain(from);
    writeFileSync(path, original.replace(from, to));
    const result = checkPins(copy);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain(diagnostic);
    writeFileSync(path, original);
    expect(checkPins(copy).status).toBe(0);
  } finally {
    rmSync(copy, { recursive: true, force: true });
  }
});
