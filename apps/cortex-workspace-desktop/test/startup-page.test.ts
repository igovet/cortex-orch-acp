import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";

const at = (path: string) => fileURLToPath(new URL(path, import.meta.url));
const html = readFileSync(at("../index.html"), "utf8");
const boot = readFileSync(at("../public/boot.css"), "utf8");
const tokens = readFileSync(at("../../../packages/workspace-ui/src/mockup-styles.css"), "utf8");
const sha = (path: string) => createHash("sha256").update(readFileSync(at(path))).digest("hex");

it("paints the real logo on the theme surface before any script runs", () => {
  expect(html).toMatch(/<div id="boot"><img src="\/cortex-256\.png" alt="" width="96" height="96"/);
  expect(html.indexOf('id="boot"')).toBeLessThan(html.indexOf('id="root"'));
  // A stylesheet link in the head blocks the first paint, so the boot surface is drawn before the logo.
  expect(html.indexOf('<link rel="stylesheet" href="/boot.css">')).toBeGreaterThan(-1);
  expect(html.indexOf('href="/boot.css"')).toBeLessThan(html.indexOf("</head>"));
  expect(sha("../public/cortex-256.png")).toBe(sha("../../../packaging/icons/cortex-256.png"));
  // The page cannot read the stylesheet before it loads, so the two surfaces are written out and must stay the tokens' values.
  const canvas = [...tokens.matchAll(/--bg-canvas: (#[0-9a-f]+)/g)].map(m => m[1]);
  expect(canvas.length).toBe(2);
  expect(boot).toContain(`html,body{margin:0;background:${canvas[0]}}`);
  expect(boot).toContain(`html[data-theme="light"] body{background:${canvas[1].replace("#ffffff", "#fff")}}`);
});

it("has no inline style element, so the code editor and terminal keep their styles", () => {
  // Tauri adds a CSP nonce and hash for every inline <style>. A nonce in style-src makes WebKit ignore 'unsafe-inline',
  // and then the <style> elements CodeMirror and xterm insert at run time are blocked: the Code view drew line numbers with no text.
  expect(html).not.toMatch(/<style[\s>]/i);
  expect(html).not.toMatch(/\sstyle=/i);
});
