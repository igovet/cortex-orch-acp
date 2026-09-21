import assert from "node:assert/strict";
import { workspaceFileRoute } from "./open-workspace-file.ts";

// Expected values follow Paseo's own encoder (UTF-8 base64url, no padding).
const decode = (value: string) => Buffer.from(value.replace(/-/g, "+").replace(/_/g, "/"), "base64").toString("utf8");

const plain = new URL(workspaceFileRoute("srv 1", "ws-1", "docs/ä.md"), "http://x");
assert.equal(plain.pathname, "/h/srv%201/workspace/ws-1");
const open = plain.searchParams.get("open")!;
assert.ok(open.startsWith("file:") && !open.includes("="));
assert.equal(decode(open.slice(5)), "docs/ä.md");

const path = new URL(workspaceFileRoute("s", "/home/u/proj", "/abs/a.ts"), "http://x").pathname;
const segment = decodeURIComponent(path.split("/").pop()!);
assert.ok(segment.startsWith("b64_"));
assert.equal(decode(segment.slice(4)), "/home/u/proj");
