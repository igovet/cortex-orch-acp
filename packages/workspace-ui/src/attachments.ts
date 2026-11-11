import type { Attachment, AttachmentInput } from "@cortex/workspace-domain";

import { LIMITS } from "@cortex/workspace-contract";

export const MAX_ATTACHMENT_BYTES = LIMITS.attachmentBytes, MAX_ATTACHMENT_FILES = LIMITS.attachmentFiles;
export const attachmentName = (f: AttachmentInput) => typeof f === "string" ? f : f.name;
/** A saved or draft file that still has its contents here. A bare name was only recorded, and cannot be opened. */
export const attachmentFile = (f: AttachmentInput): Attachment | null => typeof f === "string" || !f.data ? null : f;
const RASTER = /^image\/(png|jpeg|gif|webp|bmp)$/;
/** A `data:` address the desktop's content security policy allows for images. Never for SVG or anything that can run. */
export const attachmentPreview = (f: AttachmentInput) => { const file = attachmentFile(f); return file && RASTER.test(file.mimeType) ? `data:${file.mimeType};base64,${file.data}` : null; };

export function wireAttachments(files: AttachmentInput[] = []): Attachment[] {
  if (files.length > MAX_ATTACHMENT_FILES) throw new Error("Attach at most 8 files.");
  let bytes = 0;
  for (const f of files) {
    if (typeof f === "string") throw new Error("Choose the file again to attach its contents.");
    if (!f.name || f.name.length > 255 || /[\\/\x00-\x1f\x7f]/.test(f.name)) throw new Error(`“${f.name.slice(0, 80)}” cannot be attached: its name is empty, too long, or has a slash or control character. Rename the file and try again.`);
    if (!/^[a-zA-Z0-9.+_-]+\/[a-zA-Z0-9.+_-]+$/.test(f.mimeType)) throw new Error(`“${f.name}” cannot be attached: its file type is not recognized.`);
    let raw: string;
    try { raw = atob(f.data); } catch { throw new Error(`“${f.name}” could not be read. Choose it again.`); }
    if (btoa(raw) !== f.data) throw new Error(`“${f.name}” could not be read. Choose it again.`);
    bytes += raw.length;
  }
  if (bytes > MAX_ATTACHMENT_BYTES) throw new Error("Attachments must total 4 MiB or less.");
  return files as Attachment[];
}

/** A clipboard image can arrive with no name of its own. */
const fileName = (file: File) => file.name || `pasted.${(/^image\/([a-z0-9]+)/.exec(file.type)?.[1] ?? "bin")}`;

export async function readAttachments(files: File[], existing: AttachmentInput[]): Promise<Attachment[]> {
  if (files.length + existing.length > MAX_ATTACHMENT_FILES) throw new Error("Attach at most 8 files.");
  if (files.reduce((n, f) => n + f.size, 0) + existing.reduce((n, f) => n + (typeof f === "string" ? 0 : atob(f.data).length), 0) > MAX_ATTACHMENT_BYTES)
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
