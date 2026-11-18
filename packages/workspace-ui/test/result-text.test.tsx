import { render } from "solid-js/web";
import { createSignal, type Setter } from "solid-js";
import { afterEach, expect, it } from "vitest";
import { ResultText } from "../src/result-text";
import { Outcomes } from "../src/review";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
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
