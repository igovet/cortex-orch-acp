import { expect, it } from "vitest";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, scriptedTransport } from "./support";

it("keeps the settled result card facts when the next refresh only has pending facts for that result", async () => {
  const settled = {
    status: "review",
    result: {
      version: 2, summary: "Fixed the race", digest: "a".repeat(64), acceptanceAvailable: true,
      checks: [{ name: "Regression check", outcome: "passed", detail: "", source: "runtime" }],
      warnings: ["Reviewer and worker used the same model"],
    },
    workspaceDigest: "a".repeat(64),
    reviewActions: { accept: true, reason: null, unblockAction: null },
  };
  const pending = {
    ...settled,
    result: { ...settled.result, digest: null, acceptanceAvailable: false, checks: [], warnings: [] },
    workspaceDigest: null,
    reviewActions: { accept: false, pending: true, reason: "Checking the result against the project…", unblockAction: null },
  };
  const bundles = [settled, pending, { ...pending, result: { ...pending.result, version: 3 } }];
  const { transport, calls } = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get"]),
    "machine.call": raw => {
      const { op } = raw as { op: string };
      if (op === "repo.snapshot") return { ok: true, result: null };
      if (op === "product.read" && bundles.length) {
        return { ok: true, result: { tasks: { t: bundles.shift() }, sprints: [] } };
      }
      throw new Error(`Unexpected machine operation: ${op}`);
    },
  });
  const port = createRealPort({ transport, registry: true, actorId: "owner" });
  await port.query("capabilities.get", {});
  const refresh = async () => {
    const [facts] = await port.query("project.facts", { projectId: "p", fresh: true, git: false });
    return facts?.overlays.t;
  };

  expect(await refresh()).toEqual(settled);
  expect(await refresh(), "a partial reply for v2 must not erase the settled card").toEqual(settled);
  expect(await refresh(), "v3 is genuinely new and must not inherit v2's settled facts").toEqual({
    ...pending, result: { ...pending.result, version: 3 },
  });
  expect(bundles).toHaveLength(0);
  expect(calls.filter(c => c.operation === "machine.call" && (c.message as { op: string }).op === "product.facts"))
    .toMatchObject(Array.from({ length: 3 }, () => ({ message: { params: { defer: true } } })));
});
