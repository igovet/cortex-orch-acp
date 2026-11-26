import { expect, it } from "vitest";
import { isSecretName, maskSecrets } from "../src/feed";

const KEY = "sk-qa0123456789abcdefghijklmnop";

it("masks well-known secret formats wherever they appear, keeping the rest of the text", () => {
  const out = maskSecrets(JSON.stringify({ argv: ["printf", `OPENAI_API_KEY=${KEY}\\n`], cwd: "/tmp/p" }, null, 2));
  expect(out).not.toContain("qa0123");
  expect(out).toContain("OPENAI_API_KEY=[REDACTED]");
  expect(out).toContain('"/tmp/p"');
  expect(maskSecrets(`OPENAI_API_KEY=${KEY}\n`)).toBe("OPENAI_API_KEY=[REDACTED]\n");
  for (const [text, leak] of [
    ["sk-proj-abcDEF_ghi-JKL0123456789xyz", "abcDEF"],
    ["ghp_0123456789abcdefghijABCDEFGHIJ012345", "0123456789abcdefghij"],
    ["github_pat_11ABCDEFG0123456789_abcdefghijklmnop", "11ABCDEFG"],
    ["xoxb-123456789012-abcdefghij", "123456789012"],
    ["AKIAIOSFODNN7EXAMPLE", "IOSFODNN7"],
    ["-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----", "abc\n"],
    ["eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abcdefgh12345678", "hbGciOi"],
    ["Authorization: Bearer abcdefgh12345678", "abcdefgh12"],
    ["Basic dXNlcjpwYXNzd29yZA==", "dXNlcjpw"],
    ["DB_PASSWORD=hunter2hunter2", "hunter2"],
    ["MY_TOKEN='a b c'", "a b c"],
    ["SSH_KEY=abcdef", "abcdef"],
    ["client_secret: abcdef12", "abcdef12"],
  ] as const) expect(maskSecrets(text), text).not.toContain(leak);
  expect(maskSecrets("MY_TOKEN='a b c' next")).toBe("MY_TOKEN='[REDACTED]' next");
});

it("masks the value of a whole-word secret name in JSON and nothing else", () => {
  const rev = "ab12".repeat(16);
  const out = JSON.parse(maskSecrets(JSON.stringify({ graphRevision: rev, input_tokens: 42, tokens: "many", tokenCount: 3, apiKey: "k1", nested: { sudo_password: "p", authToken: "t" }, count: 2 })));
  expect(out).toEqual({ graphRevision: rev, input_tokens: 42, tokens: "many", tokenCount: 3, apiKey: "[REDACTED]", nested: { sudo_password: "[REDACTED]", authToken: "[REDACTED]" }, count: 2 });
});

it("does not mask harmless text", () => {
  const rev = "ab12".repeat(16);
  for (const text of [`graphRevision: ${rev}`, `{"graphRevision":"${rev}"}`, "max_tokens=100 input_tokens: 42 tokenCount=7", "commit 0123456789abcdef0123456789abcdef01234567", "sort_key=name Basic usage of the key", "sk-learn is a package"]) {
    expect(maskSecrets(text)).toBe(text);
  }
  const names = ["graphRevision", "revision", "tokens", "input_tokens", "tokenCount", "max_tokens", "keyboard", "key", "monkey", "authority", "passport", "secretary", "sort_key"];
  for (const n of names) expect(isSecretName(n), n).toBe(false);
  for (const n of ["api_key", "apiKey", "apikey", "token", "secret", "password", "passwd", "Authorization", "credentials", "private_key", "client_secret", "access_key", "sudo_password"]) expect(isSecretName(n), n).toBe(true);
});

it("leaves an already hidden marker whole", () => {
  const marked = "[HIDDEN_SECRET:0123456789abcdef]";
  expect(maskSecrets(marked)).toBe(marked);
  expect(maskSecrets(`graphRevision [HIDDEN_SECRET:0123456789abcdef]`)).toBe(`graphRevision ${marked}`);
});
