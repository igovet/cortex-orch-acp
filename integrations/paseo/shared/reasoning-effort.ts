// Keep provider-advertised ids intact; only omit inherited/default sentinels.
const levels = ["low", "medium", "high", "xhigh", "max", "ultra"];
export const isConcreteEffort = (id: string) => !["", "default", "engine_default"].includes(id.trim().toLowerCase());
const rank = (id: string) => {
  const index = levels.indexOf(id.toLowerCase());
  return index < 0 ? levels.length : index;
};
export const orderedEfforts = (values: readonly string[]) =>
  [...new Set(values.filter(isConcreteEffort))].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));

export function preferredEffort(offered: readonly string[], preferred?: string): string | undefined {
  if (preferred && offered.includes(preferred)) return preferred;
  const target = preferred && isConcreteEffort(preferred) && rank(preferred) < levels.length ? rank(preferred) : rank("medium");
  return offered.reduce<string | undefined>((best, value) =>
    !best || Math.abs(rank(value) - target) < Math.abs(rank(best) - target) ? value : best, undefined);
}
