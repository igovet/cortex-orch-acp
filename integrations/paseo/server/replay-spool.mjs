/** Private, ephemeral replay staging. Unlinked descriptors leave no files on crash. */
import { mkdtempSync, openSync, unlinkSync, rmdirSync, closeSync, writeSync, readSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";

export const REPLAY_LIMITS = Object.freeze({ items: 1_000_000, bytes: 512 * 1024 * 1024, frameBytes: 16 * 1024 * 1024 });

export class ReplaySpool {
  constructor(limits = REPLAY_LIMITS) {
    this.limits = limits;
    this.count = 0;
    this.bytes = 0;
    this.hash = createHash("sha256");
    const dir = mkdtempSync(join(tmpdir(), "cortex-replay-"));
    try {
      this.fd = openSync(join(dir, "frames"), "wx+", 0o600);
      unlinkSync(join(dir, "frames"));
    } finally { rmdirSync(dir); }
  }
  append(frame) {
    const data = Buffer.from(JSON.stringify(frame), "utf8");
    if (data.length > this.limits.frameBytes || this.count + 1 > this.limits.items ||
        this.bytes + data.length > this.limits.bytes) throw new Error("Incomplete history: replay staging limit exceeded");
    const size = Buffer.alloc(4);
    size.writeUInt32BE(data.length);
    for (const block of [size, data]) {
      let offset = 0;
      while (offset < block.length) {
        const written = writeSync(this.fd, block, offset);
        if (!written) throw new Error("Incomplete history: replay staging write failed");
        offset += written;
      }
    }
    this.count++;
    this.bytes += data.length;
    this.hash.update(data);
  }
  manifest() { return { count: this.count, bytes: this.bytes, sha256: this.hash.copy().digest("hex") }; }
  *frames() {
    let position = 0, bytes = 0;
    const hash = createHash("sha256");
    const read = (data) => {
      let offset = 0;
      while (offset < data.length) {
        const length = readSync(this.fd, data, offset, data.length - offset, position);
        if (!length) throw new Error("Incomplete history: truncated replay staging");
        offset += length;
        position += length;
      }
    };
    for (let index = 0; index < this.count; index++) {
      const size = Buffer.alloc(4); read(size);
      const length = size.readUInt32BE();
      if (length > this.limits.frameBytes) throw new Error("Incomplete history: corrupt replay staging");
      const data = Buffer.alloc(length); read(data);
      bytes += length; hash.update(data);
      yield JSON.parse(data.toString("utf8"));
    }
    if (bytes !== this.bytes || hash.digest("hex") !== this.manifest().sha256)
      throw new Error("Incomplete history: replay staging integrity mismatch");
  }
  close() { if (this.fd !== undefined) { closeSync(this.fd); this.fd = undefined; } }
}

/** Decode complete UTF-8 frames; enforce the same byte cap with or without a newline. */
export async function* readFrames(stream, maxBytes = REPLAY_LIMITS.frameBytes) {
  let pending = Buffer.alloc(0);
  const decoder = new TextDecoder("utf-8", { fatal: true });
  for await (const chunk of stream) {
    pending = Buffer.concat([pending, chunk]);
    let newline;
    while ((newline = pending.indexOf(10)) >= 0) {
      if (newline > maxBytes) throw new Error("Incomplete history: oversized ACP frame");
      const line = pending.subarray(0, newline);
      pending = pending.subarray(newline + 1);
      if (!line.length) continue;
      const frame = JSON.parse(decoder.decode(line));
      if (!frame || typeof frame !== "object" || Array.isArray(frame))
        throw new Error("Incomplete history: invalid ACP frame");
      yield frame;
    }
    if (pending.length > maxBytes) throw new Error("Incomplete history: oversized ACP frame");
  }
  if (pending.length) throw new Error("Incomplete history: truncated ACP frame");
}
