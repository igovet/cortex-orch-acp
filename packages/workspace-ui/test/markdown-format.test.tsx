import { render } from "solid-js/web";
import { createSignal, type JSX } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Task, type WorkspaceCommand } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { ResultText } from "../src/result-text";
import { MarkdownMirror } from "../src/markdown-field";
import { blockKinds, paintMarkdown, parse } from "../src/markdown";
import { flush, projection, scriptedPort, withTask } from "./support";

// Markdown reads the same in the writing area and in the conversation, from one reading of the text, and the text itself
// is never changed: the field keeps what was typed, the sent message is that string, and a code block copies its lines.
let dispose: (() => void) | undefined;
let written: string[] = [];
let refuse = false;
beforeEach(() => {
  written = []; refuse = false;
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn(async (text: string) => { if (refuse) throw new Error("denied"); written.push(text); }) } });
});
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); Reflect.deleteProperty(navigator, "clipboard"); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const mount = (view: () => JSX.Element) => { const root = document.createElement("div"); document.body.append(root); dispose?.(); dispose = render(view, root); return root; };

/** A message with every kind of Markdown the two surfaces share. */
const RICH = [
  "# Title with **bold**",
  "## Second",
  "",
  "Plain *italic*, _also italic_, ~~gone~~, `code`, **strong** and a [link](https://example.invalid/docs).",
  "",
  "> Quoted **words**",
  "> and a second line",
  "",
  "- first item",
  "- second `item`",
  "",
  "1. one",
  "2. two",
  "",
  "---",
  "",
  "```ts",
  "const x = 1;",
  "",
  "  indented();   ",
  "```",
  "",
  "| Name | Note |",
  "| --- | :-: |",
  "| a | **b** |",
].join("\n");

/** The text of every run in the layer that carries `cls` and is not a mark, one entry per token. */
function layerTokens(root: Element, cls: string): string[] {
  const out: string[] = [];
  let current: string | null = null;
  for (const node of root.querySelectorAll(".md-l > span, .md-l")) {
    if (node.classList.contains("md-l")) { if (current !== null) out.push(current); current = null; continue; }
    const content = node.classList.contains(cls) && !node.classList.contains("md-mark");
    if (content) current = (current ?? "") + node.textContent;
    else if (current !== null) { out.push(current); current = null; }
  }
  if (current !== null) out.push(current);
  return out;
}
const texts = (root: Element, selector: string) => [...root.querySelectorAll(selector)].map(e => e.textContent);

it("reads a message into the same blocks and inline tokens in the conversation and in the writing area", () => {
  const feed = mount(() => <ResultText breaks text={RICH} />);
  const feedCopy = { h1: texts(feed, "h1"), h2: texts(feed, "h2"), strong: texts(feed, "strong"), em: texts(feed, "em"), del: texts(feed, "del"),
    code: texts(feed, ":not(pre) > code"), link: texts(feed, "a"), pre: texts(feed, "pre code"), quotes: feed.querySelectorAll("blockquote").length,
    items: feed.querySelectorAll("li").length, hr: feed.querySelectorAll("hr").length, th: texts(feed, "th") };
  const layer = mount(() => <MarkdownMirror text={RICH} field={() => undefined} />).querySelector(".mc-mirror")!;
  const lines = [...layer.querySelectorAll(".md-l")];
  const body = (n: number) => lines.filter(l => l.classList.contains("md-code-body")).slice(n).map(l => l.textContent).join("\n");
  expect(texts(layer, ".md-h1").map(t => t!.replace(/^#+\s+/, "").replace(/\*\*/g, ""))).toEqual(feedCopy.h1);
  expect(texts(layer, ".md-h2").map(t => t!.replace(/^#+\s+/, ""))).toEqual(feedCopy.h2);
  expect(layerTokens(layer, "md-strong")).toEqual(feedCopy.strong);
  expect(layerTokens(layer, "md-em")).toEqual(feedCopy.em);
  expect(layerTokens(layer, "md-del")).toEqual(feedCopy.del);
  expect(layerTokens(layer, "md-code-span")).toEqual(feedCopy.code);
  expect(layerTokens(layer, "md-link")).toEqual(feedCopy.link);
  expect([body(0)]).toEqual(feedCopy.pre);
  expect(lines.filter(l => l.classList.contains("md-box-quote") && l.classList.contains("md-first"))).toHaveLength(feedCopy.quotes);
  expect(lines.filter(l => l.classList.contains("md-li"))).toHaveLength(feedCopy.items);
  expect(lines.filter(l => l.classList.contains("md-hr"))).toHaveLength(feedCopy.hr);
  expect(feedCopy.h2).toEqual(["Second"]);
  expect(feedCopy.th).toEqual(["Name", "Note"]);
  // Both read the same blocks from the same scanner.
  expect(blockKinds(parse(RICH, true))).toEqual(["h1", "h2", "text", "quote", "text", "list", "text", "text", "list", "text", "text", "hr", "code", "table"]);
});

it("keeps every character of the source in place in the writing area's layer", () => {
  const cases = [RICH, "a  \r\nb\r\n\r\n```\r\nx\r\n```", "Unicode \u{1F600} привет é **中文**\n> שלום", "trailing  \n\n\n", "```\nunclosed fence\n# not a heading", "| a | b |\n|---|---|\n| `x|y` | esc \\| pipe | extra |"];
  for (const text of cases) {
    const lines = paintMarkdown(text);
    expect(lines.map(l => l.runs.map(r => r.text).join("")), text).toEqual(text.split(/\r\n?|\n/));
    const layer = mount(() => <MarkdownMirror text={text} field={() => undefined} />).querySelector(".mc-mirror")!;
    expect([...layer.querySelectorAll(".md-l")].map(l => l.textContent).join("\n")).toBe(text.replace(/\r\n?/g, "\n"));
    expect(layer.getAttribute("aria-hidden")).toBe("true");
  }
  // An unclosed fence is code to the end in both readings.
  expect(blockKinds(parse("```\nunclosed fence\n# not a heading", true))).toEqual(["code"]);
  expect(paintMarkdown("```\nunclosed fence\n# not a heading").map(l => l.cls)).toEqual(["md-code md-fence", "md-code md-code-body", "md-code md-code-body"]);
});

it("dims the Markdown marks in the writing area and styles what they mark", () => {
  const [head, , quote, , fence, code] = paintMarkdown("### Plan\n\n> said\n\n```sh\nls -la\n```");
  expect(head.cls).toBe("md-h md-h3");
  expect(head.runs).toEqual([{ text: "### ", cls: "md-mark" }, { text: "Plan", cls: "" }]);
  expect(quote.cls).toBe("md-quote");
  expect(quote.runs[0]).toEqual({ text: "> ", cls: "md-mark" });
  expect(fence.runs).toEqual([{ text: "```", cls: "md-mark" }, { text: "sh", cls: "md-mark md-lang" }]);
  expect(code).toEqual({ cls: "md-code md-code-body", box: fence.box, runs: [{ text: "ls -la", cls: "" }] });
  const [inline] = paintMarkdown("a **b** `c`");
  expect(inline.runs).toEqual([{ text: "a ", cls: "" }, { text: "**", cls: "md-strong md-mark" }, { text: "b", cls: "md-strong" }, { text: "**", cls: "md-strong md-mark" },
    { text: " ", cls: "" }, { text: "`", cls: "md-code-span md-mark" }, { text: "c", cls: "md-code-span" }, { text: "`", cls: "md-code-span md-mark" }]);
});

it("reads italic, strikethrough and a rule, and leaves snake_case, globs and arithmetic alone", () => {
  const root = mount(() => <ResultText text={"An *emphasised* and _also_ word, ~~struck~~ text.\n\n***\n\nsnake_case_name, src/**/*.ts and 2 * 3 * 4 stay as written.\n\nPara\n---\nAfter"} />);
  expect(texts(root, "em")).toEqual(["emphasised", "also"]);
  expect(texts(root, "del")).toEqual(["struck"]);
  expect(root.querySelectorAll("hr")).toHaveLength(2);
  expect(root.textContent).toContain("snake_case_name, src/**/*.ts and 2 * 3 * 4 stay as written.");
  expect(texts(root, "p").at(-1)).toBe("After");
});

it("gives a code block a header with its language and a Copy that copies exactly the block's lines", async () => {
  const block = "const x = 1;\n\n  indented();   \n\ttab";
  const root = mount(() => <ResultText breaks text={"Before\n\n```ts title=x\n" + block + "\n```\n\nAfter"} />);
  const panel = root.querySelector<HTMLElement>(".ws-code")!;
  expect(panel.querySelector(".ws-code-lang")?.textContent).toBe("ts");
  expect(panel.querySelector("pre")?.textContent).toBe(block);
  const copy = panel.querySelector<HTMLButtonElement>("button.ws-code-copy")!;
  expect(copy.type).toBe("button");
  expect(copy.getAttribute("aria-label")).toBe("Copy ts code");
  copy.click(); await settle();
  expect(written).toEqual([block]);
  expect(panel.querySelector(".ws-copy-note")?.textContent).toBe("Copied");
  // A refused clipboard says so and claims nothing.
  refuse = true;
  copy.click(); await settle();
  expect(written).toEqual([block]);
  expect(panel.querySelector(".ws-copy-note")?.textContent).toBe("Not copied");
  // A fence with no language has no label and an unnamed Copy; it still copies only its lines.
  refuse = false; written = [];
  const plain = mount(() => <ResultText text={"~~~\nraw <b>not html</b>\n~~~"} />);
  expect(plain.querySelector(".ws-code-lang")?.textContent).toBe("");
  expect(plain.querySelector("b")).toBeNull();
  plain.querySelector<HTMLButtonElement>(".ws-code-copy")!.click(); await settle();
  expect(written).toEqual(["raw <b>not html</b>"]);
  expect(plain.querySelector(".ws-code-copy")?.getAttribute("aria-label")).toBe("Copy code");
});

it("keeps a code block's elements and its copy state through an equal-content refresh", async () => {
  const [text, setText] = createSignal("```\nx\n```\n\nTail");
  const root = mount(() => <ResultText text={text()} />);
  const panel = root.querySelector(".ws-code");
  root.querySelector<HTMLButtonElement>(".ws-code-copy")!.click(); await settle();
  setText("```\nx\n```\n\nTail changed");
  expect(root.querySelector(".ws-code")).toBe(panel);
  expect(root.querySelector(".ws-copy-note")?.textContent).toBe("Copied");
});

function liveWorld() {
  const state = withTask(emptyWorkspace(), "Format text");
  Object.assign(state.tasks[0], { status: "working", process: "live", executionId: "e" } as Partial<Task>);
  state.sessions = [emptySession("t")];
  state.machines[0].connection = "connected";
  return state;
}
async function mountTask() {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  mount(() => <Workspace port={p.port} />);
  await settle(); p.emit(projection("1", liveWorld())); await settle();
  return p;
}
const field = () => document.querySelector<HTMLTextAreaElement>('.dock [aria-label="Message"]')!;
const send = () => [...document.querySelectorAll<HTMLButtonElement>(".dock button")].find(b => b.getAttribute("aria-label") === "Send message now")!;

it.each([
  ["a fenced block", "Run this:\n```sh\nnpm test -- --run\n```\n"],
  ["a quote", "> quoted line\n> second\n\nreply"],
  ["trailing spaces", "line with two trailing spaces  \nnext line   "],
  ["line breaks written as CRLF", "first\r\nsecond\r\n\r\n- item"],
  ["unicode", "привет \u{1F680} é 中文 **жирный**"],
])("sends %s exactly as the field holds it, and the layer shows that same text", async (_name, typed) => {
  const p = await mountTask();
  const el = field();
  el.value = typed;
  const seen = el.value;
  el.dispatchEvent(new Event("input", { bubbles: true })); await settle();
  // The field is the source: nothing rewrote it, and what the browser keeps of a CRLF is what is sent.
  expect(el.value).toBe(seen);
  expect(seen.replace(/\r\n?/g, "\n")).toBe(typed.replace(/\r\n?/g, "\n"));
  const layer = el.parentElement!.querySelector(".mc-mirror")!;
  expect(el.parentElement!.classList.contains("mc-field")).toBe(true);
  expect(layer.getAttribute("aria-hidden")).toBe("true");
  expect([...layer.querySelectorAll(".md-l")].map(l => l.textContent).join("\n")).toBe(seen.replace(/\r\n?/g, "\n"));
  send().click(); await settle();
  const sent = p.commands.map(c => c.payload as WorkspaceCommand).filter(c => c.type === "send_message") as Extract<WorkspaceCommand, { type: "send_message" }>[];
  expect(sent.map(c => c.text)).toEqual([seen]);
  expect(field().value).toBe("");
});

it("lets the field draw its own text while an input method composes, and the layer again once it ends", async () => {
  await mountTask();
  const el = field(), box = el.parentElement!;
  el.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  await settle();
  expect(box.hasAttribute("data-composing")).toBe(true);
  el.value = "**中"; el.dispatchEvent(new Event("input", { bubbles: true })); await settle();
  expect(box.querySelector(".mc-mirror")?.textContent).toBe("**中");
  el.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
  await settle();
  expect(box.hasAttribute("data-composing")).toBe(false);
  expect(el.value).toBe("**中");
});
