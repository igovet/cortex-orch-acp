import { execFileSync } from "node:child_process";

/** Build a fixture `.cortex/cortex.sqlite3` for a server test by running the
 * given SQL statements through `python3 -m sqlite3` -- the same sqlite the
 * production Python readers use, and avoids depending on `node:sqlite`
 * (unavailable in this project's pinned `@types/node`). */
export function execSqlite(dbPath: string, sql: string): void {
  execFileSync("python3", ["-B", "-c", "import sqlite3,sys; db=sqlite3.connect(sys.argv[1]); db.executescript(sys.argv[2]); db.commit()", dbPath, sql]);
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
