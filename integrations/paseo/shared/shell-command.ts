function fields(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function unwrapNativeInput(value: unknown): Record<string, unknown> {
  const envelope = fields(value);
  const nested = fields(envelope.input);
  // Codex ACP custom-tool calls retain arguments beneath `input`; the
  // envelope's `name` is optional on later/replayed updates.
  return Object.keys(nested).length > 0 ? nested : envelope;
}

function stripBashLoginWrapper(command: string): string {
  const match = command.trim().match(/^(?:\/usr\/bin\/env\s+)?(?:\/bin\/)?bash\s+-lc\s+([\s\S]+)$/);
  return match?.[1]?.trim() || command.trim();
}

/** Return the command retained by native ACP tool events for display only. */
export function shellCommand(value: unknown): string | null {
  const input = unwrapNativeInput(value);
  for (const key of ["command", "cmd"] as const) {
    const candidate = input[key];
    if (typeof candidate === "string" && candidate.trim()) return stripBashLoginWrapper(candidate);
  }
  const argv = input.argv;
  if (!Array.isArray(argv) || !argv.every((part) => typeof part === "string" && part.length > 0)) return null;
  const parts = argv as string[];
  const isBash = /(?:^|\/)bash$/.test(parts[0] ?? "") && parts[1] === "-lc";
  return (isBash ? parts.slice(2) : parts).join(" ").trim() || null;
}
