import type { Attachment, AttachmentInput, AttachmentRef } from "@cortex/workspace-domain";

import { LIMITS } from "@cortex/workspace-contract";

export const MAX_ATTACHMENT_BYTES = LIMITS.attachmentBytes, MAX_ATTACHMENT_FILES = LIMITS.attachmentFiles;
export const attachmentName = (f: AttachmentInput) => typeof f === "string" ? f : f.name;
/** A saved file: a reference to its private copy on the machine, opened by fetching its bytes. */
export const attachmentRef = (f: AttachmentInput): AttachmentRef | null => typeof f !== "string" && "sha256" in f && typeof f.sha256 === "string" ? f : null;
/** Bytes this window already has: files it uploaded itself, or opened. Bounded; the oldest go first. */
const KEPT_BYTES = 16 * 1024 * 1024, kept = new Map<string, string>();
let keptBytes = 0;
export function keepBytes(ref: AttachmentRef, data: string) {
  if (kept.has(ref.sha256) || data.length > KEPT_BYTES) return;
  kept.set(ref.sha256, data); keptBytes += data.length;
  for (const [sha, old] of kept) { if (keptBytes <= KEPT_BYTES) break; kept.delete(sha); keptBytes -= old.length; }
}
/** How many bytes a file holds, without reading a saved one. */
export const attachmentBytes = (f: AttachmentInput) => typeof f === "string" ? 0 : "sha256" in f ? f.size : Math.floor(f.data.length / 4) * 3 - (f.data.endsWith("==") ? 2 : f.data.endsWith("=") ? 1 : 0);
/** A file whose contents are here now: a draft's bytes, or a saved file this window already holds. A bare name was only recorded, and cannot be opened. */
export const attachmentFile = (f: AttachmentInput): Attachment | null => {
  if (typeof f === "string") return null;
  if ("sha256" in f) { const data = kept.get(f.sha256); return data === undefined ? null : { name: f.name, mimeType: f.mimeType, data }; }
  return f.data ? f : null;
};
const RASTER = /^image\/(png|jpeg|gif|webp|bmp)$/;
/** A `data:` address the desktop's content security policy allows for images. Never for SVG or anything that can run. */
export const attachmentPreview = (f: AttachmentInput) => { const file = attachmentFile(f); return file && RASTER.test(file.mimeType) ? `data:${file.mimeType};base64,${file.data}` : null; };

export function wireAttachments(files: AttachmentInput[] = []): (Attachment | AttachmentRef)[] {
  if (files.length > MAX_ATTACHMENT_FILES) throw new Error("Attach at most 8 files.");
  let bytes = 0;
  for (const f of files) {
    if (typeof f === "string") throw new Error("Choose the file again to attach its contents.");
    if ("sha256" in f) {
      if (!/^[0-9a-f]{64}$/.test(f.sha256) || !Number.isSafeInteger(f.size) || f.size < 0) throw new Error(`“${f.name}” could not be read. Choose it again.`);
      bytes += f.size;
      continue;
    }
    if (!f.name || f.name.length > 255 || /[\\/\x00-\x1f\x7f]/.test(f.name)) throw new Error(`“${f.name.slice(0, 80)}” cannot be attached: its name is empty, too long, or has a slash or control character. Rename the file and try again.`);
    if (!/^[a-zA-Z0-9.+_-]+\/[a-zA-Z0-9.+_-]+$/.test(f.mimeType)) throw new Error(`“${f.name}” cannot be attached: its file type is not recognized.`);
    let raw: string;
    try { raw = atob(f.data); } catch { throw new Error(`“${f.name}” could not be read. Choose it again.`); }
    if (btoa(raw) !== f.data) throw new Error(`“${f.name}” could not be read. Choose it again.`);
    bytes += raw.length;
  }
  if (bytes > MAX_ATTACHMENT_BYTES) throw new Error("Attachments must total 4 MiB or less.");
  return files as (Attachment | AttachmentRef)[];
}

/** A clipboard image can arrive with no name of its own. */
const fileName = (file: File) => file.name || `pasted.${(/^image\/([a-z0-9]+)/.exec(file.type)?.[1] ?? "bin")}`;

export async function readAttachments(files: File[], existing: AttachmentInput[]): Promise<Attachment[]> {
  if (files.length + existing.length > MAX_ATTACHMENT_FILES) throw new Error("Attach at most 8 files.");
  if (files.reduce((n, f) => n + f.size, 0) + existing.reduce((n, f) => n + attachmentBytes(f), 0) > MAX_ATTACHMENT_BYTES)
    throw new Error("Attachments must total 4 MiB or less.");
  const added = await Promise.all(files.map(file => new Promise<Attachment>((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error(`“${fileName(file)}” could not be read. Choose it again.`));
    reader.onload = () => resolve({ name: fileName(file), mimeType: file.type || "application/octet-stream", data: String(reader.result).split(",")[1] ?? "" });
    reader.readAsDataURL(file);
  })));
  wireAttachments(added);
  return added;
}

/** Outside the desktop app there is no native save, so a page download stands in. */
export function downloadAttachment(file: Attachment) {
  const link = document.createElement("a");
  link.href = `data:${file.mimeType};base64,${file.data}`;
  link.download = file.name;
  link.click();
}
