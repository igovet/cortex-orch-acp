import { createHash } from "node:crypto";

/** The reference the machine answers for an uploaded file: what every later command carries instead of its bytes. */
export function refOf(file: { name: string; mimeType: string; data: string }) {
  const bytes = Buffer.from(file.data, "base64");
  return { name: file.name, mimeType: file.mimeType, size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
}
