/** The message shown in place of a crashed plugin surface. Kept apart from
 * the boundary component (JSX, not loadable directly by
 * `node --experimental-strip-types`) so it is testable without a renderer. */
export function boundaryFallbackMessage(label: string, error: unknown): string {
  let detail = "unknown error";
  try {
    const value = error instanceof Error ? error.message : error;
    if (value !== null && value !== undefined) {
      detail = String(value) || "unknown error";
    }
  } catch {
    // Formatting an arbitrary thrown value must not break the error boundary.
  }
  return `${label} could not be shown (${detail}). Reopen it to try again.`;
}
