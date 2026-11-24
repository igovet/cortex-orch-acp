// Imported first by a browser fixture: this page's own local and session storage, in memory, so a fixture served on the same
// origin as the app neither reads nor clears what the app keeps there. Nothing outlives the page.
class MemoryStorage {
  private items = new Map<string, string>();
  get length() { return this.items.size; }
  clear() { this.items.clear(); }
  getItem(key: string) { return this.items.has(key) ? this.items.get(key)! : null; }
  key(index: number) { return [...this.items.keys()][index] ?? null; }
  removeItem(key: string) { this.items.delete(key); }
  setItem(key: string, value: string) { this.items.set(String(key), String(value)); }
}
for (const name of ["localStorage", "sessionStorage"] as const)
  Object.defineProperty(window, name, { configurable: true, enumerable: true, value: new MemoryStorage() });
