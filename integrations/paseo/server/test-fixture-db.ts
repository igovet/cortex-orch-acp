import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";

type SqliteDatabase = { exec(sql: string): void; close(): void };
const { DatabaseSync } = createRequire(import.meta.url)("node:sqlite") as { DatabaseSync: new (path: string) => SqliteDatabase };

/** Run SQL statements against a fixture database with Node's built-in
 * `node:sqlite` (loaded through `require`: the pinned `@types/node` predates
 * its typings). The database file is created when absent. */
export function execSqlite(dbPath: string, sql: string): void {
  const db = new DatabaseSync(dbPath);
  try {
    db.exec(sql);
  } finally {
    db.close();
  }
}

/** A literal SQL tuple for a fixture `insert ... values (...)`, for
 * test-only, hardcoded-value fixture rows (never untrusted input). */
export function sqlValues(values: readonly (string | number | null)[]): string {
  return `(${values.map(sqlLiteral).join(", ")})`;
}

function sqlLiteral(value: string | number | null): string {
  if (value === null) return "null";
  if (typeof value === "number") return String(value);
  return `'${value.replace(/'/g, "''")}'`;
}

/** Copy the built Rust tools into `directory` next to a stand-in `cortex-acp`
 * (only its file name and location select the Rust launch; nothing runs it).
 * Build first: cargo build --locked --offline -p cortex-paseo-tools */
export function installRustToolsFixture(repoRoot: string, directory: string): { acp: string; tools: string } {
  const built = process.env.CORTEX_PASEO_TOOLS_TEST_BINARY?.trim() || join(repoRoot, "target", "debug", "cortex-paseo-tools");
  if (!existsSync(built)) throw new Error(`cortex-paseo-tools is not built at ${built}; run cargo build -p cortex-paseo-tools`);
  mkdirSync(directory, { recursive: true });
  const acp = join(directory, "cortex-acp");
  const tools = join(directory, "cortex-paseo-tools");
  writeFileSync(acp, "#!/bin/sh\nexit 9\n", { mode: 0o700 });
  copyFileSync(built, tools);
  chmodSync(tools, 0o700);
  return { acp, tools };
}

/** An empty format-12 project store (cortex-core's own schema, header and
 * access lock), for settings tests that only need the store to exist. */
export function initEmptyStore(repoRoot: string, projectRoot: string, store = ".cortex"): void {
  const directory = join(projectRoot, store);
  mkdirSync(directory, { mode: 0o700 });
  writeFileSync(join(directory, ".access.lock"), "", { mode: 0o600 });
  const dbPath = join(directory, "cortex.sqlite3");
  const schema = readFileSync(join(repoRoot, "crates", "cortex-core", "src", "store", "schema.sql"), "utf8");
  execSqlite(dbPath, `${schema}\npragma application_id=1129467218;\npragma user_version=12;\n`);
  chmodSync(dbPath, 0o600);
}
