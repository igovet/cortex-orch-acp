import { expect, it } from "vitest";
import { LIMITS, decodeFrame, encodeFrame, validate } from "@cortex/workspace-contract";
import { MAX_ATTACHMENT_BYTES, MAX_ATTACHMENT_FILES, wireAttachments } from "../src/attachments";

it("fits an ordinary screenshot and maximum description in the bounded local frame", () => {
  expect(MAX_ATTACHMENT_BYTES).toBe(4 * 1024 * 1024);
  expect(MAX_ATTACHMENT_FILES).toBe(8);
  const attachments = [{name:"screen.png",mimeType:"image/png",data:btoa("x".repeat(MAX_ATTACHMENT_BYTES))}];
  expect(wireAttachments(attachments)).toEqual(attachments);
  const payload = {description:"😀".repeat(32000),attachments};
  expect(() => validate("CreatePayload", payload)).not.toThrow();
  expect(decodeFrame(encodeFrame(payload))).toEqual(payload);
  expect(encodeFrame(payload).length).toBeLessThan(LIMITS.frameBytes);
});

it("rejects excess aggregate bytes, file count, invalid names and malformed encoding", () => {
  const f = {name:"screen.png",mimeType:"image/png",data:"eA=="};
  for (const files of [
    Array(9).fill(f), [{...f,data:btoa("x".repeat(MAX_ATTACHMENT_BYTES + 1))}],
    Array(2).fill({...f,data:btoa("x".repeat(MAX_ATTACHMENT_BYTES / 2 + 1))}),
    [{...f,name:"../outside"}], [{...f,data:"Zh=="}], [{...f,data:"invalid!"}],
  ]) expect(() => wireAttachments(files)).toThrow();
});
