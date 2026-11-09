export type Token = { c: string | null; t: string };

const WORDS: Record<string, Set<string>> = {
  py: new Set("def class return if elif else not is None True False from import as await async for in while raise try except finally with and or pass lambda yield".split(" ")),
  sql: new Set("CREATE TABLE PRIMARY KEY NOT NULL SELECT FROM WHERE INSERT INTO VALUES ALTER ADD DATE TEXT BIGINT BIGSERIAL TIMESTAMPTZ".split(" ")),
  toml: new Set(["true", "false"]),
};
const language = (path: string) => { const ext = path.split(".").at(-1) ?? ""; return ext in WORDS ? ext : null; };
const PATTERN = /("""[\s\S]*?"""|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')|(#.*$|--.*$)|\b(\d+(?:\.\d+)?)\b|([A-Za-z_][A-Za-z0-9_]*)/g;

/** A small line tokenizer for a few file types. Other files stay plain text, which is honest and still readable. */
export function highlight(path: string, text: string): Token[] {
  const lang = language(path);
  if (!lang) return [{ c: null, t: text }];
  const words = WORDS[lang], out: Token[] = [];
  let at = 0;
  for (const match of text.matchAll(PATTERN)) {
    const [whole, str, comment, num, word] = match, index = match.index;
    if (lang === "py" && comment?.startsWith("--")) continue;
    if (lang === "sql" && comment?.startsWith("#")) continue;
    if (index > at) out.push({ c: null, t: text.slice(at, index) });
    const next = text[index + whole.length];
    const cls = str ? "tok-s" : comment ? "tok-c" : num ? "tok-n"
      : words.has(word) ? "tok-k" : next === "(" ? "tok-f" : /^[A-Z][a-z]/.test(word) ? "tok-t" : null;
    out.push({ c: cls, t: whole });
    at = index + whole.length;
  }
  if (at < text.length) out.push({ c: null, t: text.slice(at) });
  return out;
}
export const languageOf = (path: string) => ({ py: "Python", sql: "SQL", toml: "TOML", md: "Markdown", ts: "TypeScript" } as Record<string, string>)[path.split(".").at(-1) ?? ""] ?? "Plain text";
