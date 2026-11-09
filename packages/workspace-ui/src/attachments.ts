import type { Attachment, AttachmentInput } from "@cortex/workspace-domain";

export const MAX_ATTACHMENT_BYTES = 512 * 1024, MAX_ATTACHMENT_FILES = 8;
export const attachmentName = (f: AttachmentInput) => typeof f === "string" ? f : f.name;

export function wireAttachments(files: AttachmentInput[] = []): Attachment[] {
  if (files.length > MAX_ATTACHMENT_FILES) throw new Error("Attach at most 8 files.");
  let bytes = 0;
  for (const f of files) {
    if (typeof f === "string" || !f.name || f.name.length > 255 || /[\\/\x00-\x1f\x7f]/.test(f.name)
      || !/^[a-zA-Z0-9.+_-]+\/[a-zA-Z0-9.+_-]+$/.test(f.mimeType)) throw new Error("Choose the file again to attach its contents.");
    let raw: string;
    try { raw = atob(f.data); } catch { throw new Error("This attachment could not be read."); }
    if (btoa(raw) !== f.data) throw new Error("This attachment could not be read.");
    bytes += raw.length;
  }
  if (bytes > MAX_ATTACHMENT_BYTES) throw new Error("Attachments must total 512 KiB or less.");
  return files as Attachment[];
}

export async function readAttachments(files: File[], existing: AttachmentInput[]): Promise<Attachment[]> {
  if (files.length + existing.length > MAX_ATTACHMENT_FILES) throw new Error("Attach at most 8 files.");
  if (files.reduce((n, f) => n + f.size, 0) + existing.reduce((n, f) => n + (typeof f === "string" ? 0 : atob(f.data).length), 0) > MAX_ATTACHMENT_BYTES)
    throw new Error("Attachments must total 512 KiB or less.");
  const added = await Promise.all(files.map(file => new Promise<Attachment>((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("This file could not be read. Choose it again."));
    reader.onload = () => resolve({ name: file.name, mimeType: file.type || "application/octet-stream", data: String(reader.result).split(",")[1] ?? "" });
    reader.readAsDataURL(file);
  })));
  wireAttachments(added);
  return added;
}
