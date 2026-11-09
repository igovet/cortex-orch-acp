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
  expect(document.querySelectorAll(".outc > div")).toHaveLength(1);
  expect(document.querySelector(".outc")?.textContent).toBe("1Passed");
});
