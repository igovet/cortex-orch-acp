import { Text } from "@codemirror/state";
import type { CodeLspDiagnostic, CodeLspSnapshot } from "@cortex/workspace-contract";
import { afterEach, expect, it, vi } from "vitest";
import { createBuffers } from "../src/editor-buffers";
import { CodeRefusal, leaseMessage, type CodeEvent } from "../src/code-support";
import { diagnosticRanges, openLanguageDocument } from "../src/code-lsp";

const file = { projectId: "p", checkoutId: "p:main", path: "example.rs" };
const grant = { leaseToken: "a".repeat(64), expiresInMs: 60_000, renewAfterMs: 20_000 };
const fixture = () => {
  const lease = {
    acquire: vi.fn().mockResolvedValue(grant),
    renew: vi.fn().mockResolvedValue({ expiresInMs: 60_000 }),
    release: vi.fn().mockResolvedValue({ released: true }),
    save: vi.fn().mockResolvedValue({ outcome: "saved", revision: "second", size: 4 }),
  };
  const buffers = createBuffers({ lease, read: async () => ({ text: "base", revision: "first" }), remember: () => undefined });
  return { lease, buffers, key: "p:main|example.rs" };
};
afterEach(() => vi.useRealTimers());

it("renews while held, fails closed on renewal loss, and keeps the draft without replay", async () => {
  vi.useFakeTimers();
  const { lease, buffers: b, key } = fixture();
  await b.open(file.projectId, file.checkoutId, file.path);
  b.edit(key, "not granted");
  await b.save(key);
  expect(lease.save).not.toHaveBeenCalled();
  await b.acquire(key);
  b.edit(key, "mine");
  await vi.advanceTimersByTimeAsync(19_999);
  expect(lease.renew).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(lease.renew).toHaveBeenCalledWith(expect.objectContaining(file), grant.leaseToken);
  lease.renew.mockRejectedValueOnce(new CodeRefusal("edit_lease_lost", "opaque"));
  await vi.advanceTimersByTimeAsync(20_000);
  expect(b.find(key)).toMatchObject({ lease: "idle", leaseNotice: expect.stringContaining("Edit lease lost"), dirty: true });
  expect(b.text(key)).toBe("mine");
  await b.save(key);
  expect(lease.save).not.toHaveBeenCalled();
  await b.acquire(key);
  expect(lease.save).not.toHaveBeenCalled();
  await b.save(key);
  expect(lease.save).toHaveBeenCalledWith(expect.objectContaining(file), "mine", "first", grant.leaseToken);
  b.close(key);
  expect(lease.release).toHaveBeenCalledTimes(2);
});

it("releases a grant that arrives after switch and never turns the old tab editable", async () => {
  const { lease, buffers: b, key } = fixture();
  await b.open(file.projectId, file.checkoutId, file.path);
  let reply!: (value: typeof grant) => void;
  lease.acquire.mockImplementationOnce(() => new Promise(resolve => { reply = resolve; }));
  const acquiring = b.acquire(key);
  b.release(key);
  reply(grant);
  await acquiring;
  expect(b.find(key)?.lease).toBe("idle");
  expect(lease.release).toHaveBeenCalledWith(expect.objectContaining(file), grant.leaseToken);
});

it("maps a task title or identity and keeps useful copy when there is no holder identity", () => {
  expect(leaseMessage(new CodeRefusal("task_file_lease_held", "", "t_1"), "Ship it")).toBe("Task Ship it is editing this file.");
  expect(leaseMessage(new CodeRefusal("task_file_lease_held", "", "t_1"))).toContain("t_1");
  expect(leaseMessage(new CodeRefusal("task_file_lease_held", ""))).toBe("A task is editing this file.");
  expect(leaseMessage(new CodeRefusal("conflict", ""))).toContain("changed on disk");
});

const error: CodeLspDiagnostic = { message: "Not a name", severity: 1, range: { start: { line: 0, character: 2 }, end: { line: 0, character: 9 } } };
it("maps UTF-16 findings, clamps a long range, and omits lines not in the document", () => {
  const doc = Text.of(["😀x", ""]);
  expect(diagnosticRanges([error, { ...error, range: { ...error.range, start: { line: 2, character: 0 } } }], doc)).toEqual([
    { ...error, from: 2, to: 3 },
  ]);
});

it("filters snapshots by handle, version and generation, then syncs before definition", async () => {
  vi.useFakeTimers();
  const snapshot: CodeLspSnapshot = { eventId: "e", version: 1, generation: 2, status: "ready", diagnostics: [error] };
  let event!: (e: CodeEvent) => void;
  const publish = vi.fn(), unsubscribe = vi.fn();
  const io = {
    open: vi.fn().mockResolvedValue({ ...snapshot, documentId: "d" }),
    change: vi.fn().mockResolvedValue({ ...snapshot, version: 2, diagnostics: [] }),
    status: vi.fn().mockResolvedValue(snapshot),
    close: vi.fn().mockResolvedValue({ closed: true }),
    definition: vi.fn().mockResolvedValue({ version: 2, generation: 2, targets: [] }),
    subscribe: vi.fn(listener => { event = listener; return unsubscribe; }),
  };
  const doc = openLanguageDocument(io, file, "base", publish);
  await vi.advanceTimersByTimeAsync(0);
  publish.mockClear();
  event({ ...snapshot, type: "code_lsp", projectId: "other" });
  event({ ...snapshot, type: "code_lsp", projectId: "p", eventId: "other" });
  event({ ...snapshot, type: "code_lsp", projectId: "p", generation: 1 });
  expect(publish).not.toHaveBeenCalled();
  doc.changed("draft");
  expect(publish).toHaveBeenLastCalledWith(null);
  event({ ...snapshot, type: "code_lsp", projectId: "p" });
  expect(publish).toHaveBeenLastCalledWith(null);
  await expect(doc.definition({ line: 0, character: 2 })).resolves.toEqual([]);
  expect(io.change).toHaveBeenCalledWith("p", "d", "draft", 2);
  expect(io.definition).toHaveBeenCalledWith("p", { documentId: "d", version: 2, position: { line: 0, character: 2 } });
  doc.dispose();
  await vi.advanceTimersByTimeAsync(250);
  expect(io.change).toHaveBeenCalledTimes(1);
  expect(io.close).toHaveBeenCalledWith("p", "d");
  expect(unsubscribe).toHaveBeenCalledOnce();
});

it("does not keep restarting a known unavailable language server while typing", async () => {
  vi.useFakeTimers();
  const publish = vi.fn();
  const io = {
    open: vi.fn().mockRejectedValue(new CodeRefusal("lsp_unavailable", "missing server")),
    change: vi.fn(), status: vi.fn(), definition: vi.fn(), close: vi.fn(), subscribe: () => () => undefined,
  };
  const doc = openLanguageDocument(io, file, "base", publish);
  await vi.advanceTimersByTimeAsync(0);
  doc.changed("new draft");
  await vi.advanceTimersByTimeAsync(300);
  await doc.definition({ line: 0, character: 0 });
  expect(io.open).toHaveBeenCalledOnce();
  expect(publish.mock.calls).toEqual([[null, true]]);
  doc.dispose();
});
