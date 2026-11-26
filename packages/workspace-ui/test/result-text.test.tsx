import { render } from "solid-js/web";
import { createSignal, type Setter } from "solid-js";
import { afterEach, expect, it, vi } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Task } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import type { ResultTarget } from "../src/result-links";
import { flush, projection, scriptedPort, withTask } from "./support";
import { ResultText } from "../src/result-text";
import { Outcomes } from "../src/review";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); Reflect.deleteProperty(navigator, "clipboard"); });
const mount = (view: () => import("solid-js").JSX.Element) => {
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(view, root);
};

it("lays out real runtime result blocks without interpreting supplied HTML", () => {
  mount(() => <ResultText text={'# Done\n\nThe requested work is done.\n\nWork summary:\n> Kept **existing work**.\n\nFiles: `a.txt`.\n\n- Verified locally\n- No push\n\n```sh\nprintf done\n```\n\n<img src=x onerror=alert(1)>'} />);
  expect(document.querySelector("h1")?.textContent).toBe("Done");
  expect(document.querySelector("blockquote strong")?.textContent).toBe("existing work");
  expect(document.querySelectorAll("li")).toHaveLength(2);
  expect(document.querySelector("pre")?.textContent).toBe("printf done");
  expect(document.querySelector("img")).toBeNull();
  expect(document.body.textContent).toContain("<img src=x onerror=alert(1)>");
});

it("formats an owner-style report with heading levels, tables, ordered lists and literal code", () => {
  mount(() => <ResultText text={'Cortex draft ID: `d_fixture`\n\n# Verdict: REQUEST_CHANGES\n\n## Finding\n### P3 — Published count\nLocation: `docs/project/verification.md:80`.\n\n| Check | Outcome |\n| --- | --- |\n| Links | **Passed** |\n| Code `a|b` | Escaped \\| pipe |\n\n1. Inspect source\n2. Re-run checks\n\n```sh\nprintf "a\\.b"\n```'} />);
  expect(document.querySelector("h1")?.textContent).toBe("Verdict: REQUEST_CHANGES");
  expect(document.querySelector("h2")?.textContent).toBe("Finding");
  expect(document.querySelector("h3")?.textContent).toBe("P3 — Published count");
  expect(document.querySelectorAll("table thead th")).toHaveLength(2);
  expect(document.querySelectorAll("table tbody tr")).toHaveLength(2);
  expect(document.querySelector("table strong")?.textContent).toBe("Passed");
  expect(document.querySelector("table code")?.textContent).toBe("a|b");
  expect(document.querySelectorAll("table tbody tr")[1].textContent).toContain("Escaped | pipe");
  expect(document.querySelectorAll("ol li")).toHaveLength(2);
  expect(document.querySelector("pre code")?.textContent).toBe('printf "a\\.b"');
});

it("renders links and escapes as text while refusing executable URLs and HTML", () => {
  mount(() => <ResultText text={'#### Four\n##### Five\n###### Six\n\n[Read **docs**](https://example.invalid/docs) [Jump](#finding) [Unsafe](javascript:alert) [Data](data:text/html,no)\n\n> Keep **bold** and `a\\\\.b` literal.\n\nEscaped \\*not bold\\* and \\[brackets\\]. <script>alert(1)</script>'} />);
  expect(document.querySelector("h4")?.textContent).toBe("Four");
  expect(document.querySelector("h5")?.textContent).toBe("Five");
  expect(document.querySelector("h6")?.textContent).toBe("Six");
  expect(document.querySelectorAll("a")).toHaveLength(2);
  expect(document.querySelector("a strong")?.textContent).toBe("docs");
  expect(document.querySelector('a[href="#finding"]')).not.toBeNull();
  expect(document.querySelector("script")).toBeNull();
  expect(document.querySelector("blockquote code")?.textContent).toBe("a\\\\.b");
  expect(document.body.textContent).toContain("Escaped *not bold* and [brackets].");
});

function clickLink(link: HTMLAnchorElement) {
  const click = new MouseEvent("click", { bubbles: true, cancelable: true });
  link.dispatchEvent(click);
  expect(click.defaultPrevented).toBe(true);
}

it("renders without Workspace and prevents navigation on every rendered anchor", () => {
  mount(() => <ResultText text={"src/app.ts:42 and `/fixture/src/app.ts:42` [relative](guide.md) [web](https://example.invalid) [script](javascript:42) ![preview](https://example.invalid/image.png)"} />);
  const links = [...document.querySelectorAll<HTMLAnchorElement>("a")];
  expect(links.map(a => a.textContent)).toEqual(["relative", "web", "Image: preview (example.invalid · image.png)"]);
  for (const link of links) clickLink(link);
  expect(document.querySelector("code")?.textContent).toBe("/fixture/src/app.ts:42");
});

const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };

async function mountConversation(text: string, openResultTarget?: (target: ResultTarget) => Promise<void>, resultRead?: () => Promise<never>, pendingFacts?: () => Promise<never>) {
  const s = withTask(emptyWorkspace(), "Read links");
  Object.assign(s.tasks[0], { status: "working", process: "live", executionId: "e" });
  s.machines[0].connection = "connected";
  s.projects[0].repository = "/fixture";
  const session = emptySession("t");
  session.timeline = [{ id: "reply", kind: "cortex", text }];
  if (resultRead) {
    Object.assign(s.tasks[0], { status: "review", process: "stopped", workspaceDigest: "d",
      result: { version: 1, digest: "d", summary: text, checks: [] } } as unknown as Partial<Task>);
    session.timeline = [];
  }
  s.sessions = [session];
  if (!pendingFacts) s.repos = [{ projectId: "p", remote: "", tree: ["README.md", "src/app.ts", "out/video.mp4", "out/sub titles.srt"], sources: {}, commits: [], branches: [], fetchedAt: null,
    checkouts: [{ id: "p:main", projectId: "p", kind: "main", path: "/fixture", branch: "main", files: [], sources: {}, taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", updateConflict: null }] }];
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  if (pendingFacts) {
    const query = p.port.query;
    p.port.query = (operation, parameters) => operation === "project.facts" && "fresh" in parameters && parameters.fresh
      ? pendingFacts() : query(operation, parameters);
  }
  p.port.readFile = resultRead ?? (async () => ({ text: "example\n", revision: "r" }));
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  mount(() => <Workspace port={p.port} openResultTarget={openResultTarget} />);
  await settle(); p.emit(projection("1", s)); await settle();
  return [...document.querySelectorAll<HTMLAnchorElement>(resultRead ? ".timeline .rcard .ws-result-copy a" : ".timeline .ai .ws-result-copy a")];
}

it.each([
  ["Check src/app.ts:42:7.", "src/app.ts:42:7"],
  ["Check `/fixture/src/app.ts:42`.", "/fixture/src/app.ts:42"],
  ["Check [the file](src/app.ts:42).", "the file"],
])("opens a file named in %s in the current task's Code tab at the line", async (text, label) => {
  const [link] = await mountConversation(text);
  expect(link?.textContent).toBe(label);
  clickLink(link); await settle();
  expect(location.pathname).toBe(`/m/${LOCAL_MACHINE_ID}/t/t/code`);
  expect([...new URLSearchParams(location.search)]).toEqual([["file", "src/app.ts"], ["line", "42"]]);
});

it("links a bare root-level file name in a code span only when the checkout has it", async () => {
  const [link, ...rest] = await mountConversation("See `README.md`, `ws.store`, `npm run test`, `NOTES.md` and plain README.md.");
  expect(link?.textContent).toBe("README.md");
  expect(rest).toEqual([]);
  expect([...document.querySelectorAll(".ws-result-copy code")].map(c => c.textContent)).toEqual(["README.md", "ws.store", "npm run test", "NOTES.md"]);
  clickLink(link); await settle();
  expect([...new URLSearchParams(location.search)]).toEqual([["file", "README.md"]]);
  expect(location.pathname).toBe(`/m/${LOCAL_MACHINE_ID}/t/t/code`);
});

it("shows a copyable full path outside the checkout, and requests native open only on the second click", async () => {
  const copy = vi.fn(async (_path: string) => {});
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: copy } });
  const open = vi.fn(async (_target: ResultTarget) => {});
  const [link] = await mountConversation("Try `../external/notes.txt:12:3`.", open);
  clickLink(link); await settle();
  const dialog = document.querySelector(".ws-path-pop")!;
  expect(dialog.textContent).toContain("/external/notes.txt");
  expect(open).not.toHaveBeenCalled();
  const [copyButton, openButton] = [...dialog.querySelectorAll("button")];
  expect(copyButton.textContent).toBe("Copy path");
  expect(document.activeElement).toBe(copyButton);
  copyButton.click(); await settle();
  expect(copy).toHaveBeenCalledWith("/external/notes.txt");
  expect(dialog.querySelector('[role="status"]')?.textContent).toBe("Path copied.");
  openButton.click(); await settle();
  expect(open).toHaveBeenCalledWith({ kind: "file", path: "/external/notes.txt", line: 12, column: 3 });
  expect(document.querySelector(".ws-path-pop")).toBeNull();
  expect(document.activeElement).toBe(link);
});

it("explains a missing file, leaves fenced text alone, and hands HTTPS and mail links off without changing the route", async () => {
  const open = vi.fn(async (_target: ResultTarget) => {});
  const links = await mountConversation("Check src/missing.ts and [Docs](https://example.invalid/docs) or [Mail](mailto:owner@example.invalid).\n\n```sh\ncat src/app.ts:42\n```", open);
  expect(links.map(a => a.textContent)).toEqual(["src/missing.ts", "Docs", "Mail"]);
  clickLink(links[0]); await settle();
  expect(document.querySelector(".ws-path-pop")?.textContent).toContain("File not found in this checkout");
  document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  expect(document.querySelector(".ws-path-pop")).toBeNull();
  expect(document.querySelector("pre code a")).toBeNull();
  const route = location.href;
  clickLink(links[1]); clickLink(links[2]); await settle();
  expect(open.mock.calls.map(([target]) => target)).toEqual([{ kind: "url", url: "https://example.invalid/docs" }, { kind: "url", url: "mailto:owner@example.invalid" }]);
  expect(location.href).toBe(route);
});

it("opens video and subtitle artifacts without waiting for Code and stays on the Result card", async () => {
  const read = vi.fn(() => new Promise<never>(() => {}));
  const copy = vi.fn(async (_text: string) => {});
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: copy } });
  let finish!: () => void;
  const open = vi.fn((_target: ResultTarget) => new Promise<void>(resolve => { finish = resolve; }));
  const text = "[Full video](out/linked-directory/../video.mp4) · [English subtitles](/fixture/out/sub%20titles.srt)";
  const links = await mountConversation(text, open, read);
  expect(links.map(link => link.textContent)).toEqual(["Full video", "English subtitles"]);
  const route = location.href;
  const resultCopy = document.querySelector<HTMLButtonElement>('.rcard [aria-label="Copy the result as Markdown"]')!;
  for (const [i, path] of ["/fixture/out/linked-directory/../video.mp4", "/fixture/out/sub titles.srt"].entries()) {
    clickLink(links[i]);
    const dialog = document.querySelector('[role="dialog"][aria-label="File path"]')!;
    expect(dialog).not.toBeNull();
    expect(dialog.querySelector(".ws-path-value")?.textContent).toBe(path);
    expect(links[i].hasAttribute("aria-busy")).toBe(false);
    expect(read).not.toHaveBeenCalled();
    expect(open).toHaveBeenCalledTimes(i);
    const [copyPath, button] = [...dialog.querySelectorAll("button")];
    expect(copyPath.textContent).toBe("Copy path");
    expect(button.textContent).toBe("Open in default app");
    copyPath.click(); await settle();
    expect(copy).toHaveBeenLastCalledWith(path);
    expect(open).toHaveBeenCalledTimes(i);
    button.click(); await settle();
    expect(open).toHaveBeenLastCalledWith({ kind: "file", path });
    expect(button.textContent).toBe("Opening…");
    resultCopy.click(); await settle();
    expect(copy).toHaveBeenLastCalledWith(text);
    expect(location.href).toBe(route);
    finish(); await settle();
    expect(document.querySelector(".ws-path-pop")).toBeNull();
  }
});

it("confirms absolute Result media paths while project facts are pending", async () => {
  const facts = vi.fn(() => new Promise<never>(() => {}));
  const read = vi.fn(() => new Promise<never>(() => {}));
  const copy = vi.fn(async (_path: string) => {});
  const open = vi.fn(async (_target: ResultTarget) => {});
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: copy } });
  const text = "[Video](/fixture/out/video.mp4) · [Subtitles](/fixture/out/linked-directory/../sub%20titles.srt)";
  const links = await mountConversation(text, open, read, facts);
  expect(links.map(link => link.textContent)).toEqual(["Video", "Subtitles"]);
  expect(facts).toHaveBeenCalled();
  const factsCount = facts.mock.calls.length;
  const route = location.href;
  for (const [i, path] of ["/fixture/out/video.mp4", "/fixture/out/linked-directory/../sub titles.srt"].entries()) {
    const link = links[i];
    clickLink(link);
    const dialog = document.querySelector('[role="dialog"][aria-label="File path"]')!;
    expect(dialog).not.toBeNull();
    expect(dialog.textContent).toContain(path);
    expect(link.hasAttribute("aria-busy")).toBe(false);
    expect(open).toHaveBeenCalledTimes(i);
    const [copyButton, openButton] = [...dialog.querySelectorAll("button")];
    expect(copyButton.textContent).toBe("Copy path");
    expect(openButton.textContent).toBe("Open in default app");
    expect(document.activeElement).toBe(copyButton);
    copyButton.click(); await settle();
    expect(copy).toHaveBeenLastCalledWith(path);
    expect(open).toHaveBeenCalledTimes(i);
    openButton.click(); await settle();
    expect(open).toHaveBeenCalledTimes(i + 1);
    expect(open).toHaveBeenLastCalledWith({ kind: "file", path });
    expect(document.querySelector(".ws-path-pop")).toBeNull();
    expect(document.activeElement).toBe(link);
    expect(location.href).toBe(route);
  }
  expect(facts).toHaveBeenCalledTimes(factsCount);
  expect(read).not.toHaveBeenCalled();
});

it("shows opener refusal on the Result card and permits another interaction and retry", async () => {
  const read = vi.fn(() => new Promise<never>(() => {}));
  const open = vi.fn((_target: ResultTarget) => Promise.resolve()).mockRejectedValueOnce("The default app could not open this file.");
  const [video, invalid, web] = await mountConversation(
    "[Video](out/video.mp4) [Invalid](out/%ZZ.srt) [Web](https://example.invalid/subtitles.srt) [Unsafe](file:///tmp/subtitles.srt)", open, read);
  expect([...document.querySelectorAll(".rcard .ws-result-copy a")]).toHaveLength(3);
  const route = location.href;
  clickLink(video); await settle();
  const dialog = document.querySelector(".ws-path-pop")!;
  const opener = [...dialog.querySelectorAll("button")].find(el => el.textContent === "Open in default app")!;
  opener.click(); await settle();
  expect(dialog.querySelector('[role="status"]')?.textContent).toBe("The default app could not open this file.");
  opener.click(); await settle();
  expect(open).toHaveBeenCalledTimes(2);
  expect(document.querySelector(".ws-path-pop")).toBeNull();
  clickLink(invalid); await settle();
  expect(document.querySelector('.rcard .ws-result-copy [role="status"]')?.textContent).toContain("Could not open this link");
  clickLink(web); await settle();
  expect(open).toHaveBeenLastCalledWith({ kind: "url", url: "https://example.invalid/subtitles.srt" });
  expect(location.href).toBe(route);
});

it("keeps result nodes through equal-content background reads", () => {
  let publish!: Setter<{ text: string }>;
  mount(() => {
    const [value, setValue] = createSignal({ text: "# Same result\n\nNo files lost." });
    publish = setValue;
    return <ResultText text={value().text} />;
  });
  const heading = document.querySelector("h1");
  expect(heading).not.toBeNull();
  publish({ text: "# Same result\n\nNo files lost." });
  expect(document.querySelector("h1")).toBe(heading);
});

it("does not show three empty check outcomes beside a passing check", () => {
  mount(() => <Outcomes checks={[{ name: "Local verification", outcome: "passed", detail: "Recorded" }]} />);
  expect(document.querySelectorAll(".rv-outc > span")).toHaveLength(1);
  expect(document.querySelector(".rv-outc")?.textContent).toBe("1 passed");
});

const rowsOf = (text: string) => { mount(() => <ResultText text={text} />); return [...document.querySelectorAll("table tbody tr")].map(tr => [...tr.children].map(td => td.textContent)); };
const URL_CELL = "https://example.org/w/A%27s_Arrest_(X60AA035X).webm";

it("keeps every row in a table until a blank line or another block, whatever the row holds", () => {
  const rows = rowsOf([
    "| Name | Source | Note |", "| --- | --- | --- |",
    `| **bold** | ${URL_CELL} | \`code\` (paren) |`,
    "| dash | — | |",
    "| short |",
    "| wide | a | b | extra | more |",
    "wrapped continuation without any pipe",
    "| after | the | wrap |",
    "| `a|b` | esc \\| pipe | z |",
  ].join("\n"));
  expect(rows).toHaveLength(7);
  expect(rows[0]).toEqual(["bold", URL_CELL, "code (paren)"]);
  expect(rows[1]).toEqual(["dash", "—", ""]);
  expect(rows[2]).toEqual(["short", "", ""]);
  expect(rows[3]).toEqual(["wide", "a", "b"]);
  expect(rows[4]).toEqual(["wrapped continuation without any pipe", "", ""]);
  expect(rows[5]).toEqual(["after", "the", "wrap"]);
  expect(rows[6]).toEqual(["a|b", "esc | pipe", "z"]);
  expect(document.querySelectorAll("p")).toHaveLength(0);
});

it("ends a table at a blank line, a heading, a quote or a fence, and starts a new block there", () => {
  const rows = rowsOf("| A | B |\n| - | - |\n| 1 | 2 |\n\n| 3 | 4 |");
  expect(rows).toEqual([["1", "2"]]);
  expect(document.querySelector("p")?.textContent).toBe("| 3 | 4 |");
  document.body.innerHTML = "";
  expect(rowsOf("| A |\n|---|\n| 1 |\n# Next\n| 2 |")).toEqual([["1"]]);
  document.body.innerHTML = "";
  expect(rowsOf("| A |\n|---|\n| 1 |\n> quoted")).toEqual([["1"]]);
  expect(document.querySelector("blockquote")?.textContent).toBe("quoted");
});

it("reads the delimiter row the GFM way: one hyphen is enough, colons align, a count mismatch is not a table", () => {
  const rows = rowsOf("| Left | Mid | Right | None |\n|:-|:-:|-:|---|\n| a | b | c | d |");
  expect(rows).toEqual([["a", "b", "c", "d"]]);
  const align = [...document.querySelectorAll("table thead th")].map(th => (th as HTMLElement).style.textAlign);
  expect(align).toEqual(["left", "center", "right", ""]);
  expect([...document.querySelectorAll("table tbody td")].map(td => (td as HTMLElement).style.textAlign)).toEqual(["left", "center", "right", ""]);
  document.body.innerHTML = "";
  expect(rowsOf("| A | B |\n| --- |\n| 1 | 2 |")).toEqual([]);
  expect(document.querySelector("table")).toBeNull();
  expect(document.querySelector(".ws-result-table")).toBeNull();
});

it("updates a refreshed document in place: unchanged blocks keep their elements and the reader keeps the place", () => {
  const [text, setText] = createSignal("# Title\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\nFirst paragraph.\n\nLast paragraph.");
  const scroller = document.createElement("div");
  document.body.append(scroller);
  dispose = render(() => <ResultText text={text()} />, scroller);
  scroller.scrollTop = 240;
  const [h1, table, first, last] = ["h1", "table", "p", "p:last-of-type"].map(s => scroller.querySelector(s));
  setText(text());
  expect(scroller.querySelector("table")).toBe(table);
  setText(text().replace("Last paragraph.", "Last paragraph, edited."));
  expect(scroller.querySelector("h1")).toBe(h1);
  expect(scroller.querySelector("table")).toBe(table);
  expect(scroller.querySelector("p")).toBe(first);
  expect(scroller.querySelector("p:last-of-type")).not.toBe(last);
  expect(scroller.querySelector("p:last-of-type")?.textContent).toBe("Last paragraph, edited.");
  expect(scroller.scrollTop).toBe(240);
  setText(text().replace("| 1 | 2 |", "| 1 | 2 |\n| 3 | 4 |"));
  expect(scroller.querySelectorAll("tbody tr")).toHaveLength(2);
  expect(scroller.querySelector("h1")).toBe(h1);
  expect(scroller.querySelector("p")).toBe(first);
});
