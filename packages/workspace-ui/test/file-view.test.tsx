import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type FileRead } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { FilePreview, parseDelimited, safeHtml, viewKind, viewPlan } from "../src/file-view";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, flush, scriptedTransport, snapshot } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => {
  localStorage.clear(); sessionStorage.clear();
  Range.prototype.getClientRects = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => ({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON() {} });
});
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const mount = (view: () => import("solid-js").JSX.Element) => { const root = document.createElement("div"); document.body.append(root); dispose = render(view, root); };
const settled = async () => { for (let i = 0; i < 14; i++) await flush(); };
const PNG = "iVBORw0KGgo=";
const none = () => Promise.resolve<FileRead>({ text: null, reason: "none" });

describe("choosing how a file is shown", () => {
  it.each([
    ["README.md", "markdown", ["preview", "source"], "preview"],
    ["docs/Guide.MARKDOWN", "markdown", ["preview", "source"], "preview"],
    ["data/table.csv", "csv", ["preview", "source"], "preview"],
    ["data/table.tsv", "tsv", ["preview", "source"], "preview"],
    ["icons/logo.svg", "svg", ["preview", "source"], "preview"],
    ["site/index.html", "html", ["source", "preview"], "source"],
    ["package.json", "json", ["source", "preview"], "source"],
    ["shot.PNG", "image", ["preview"], "preview"],
    ["a/b/photo.jpeg", "image", ["preview"], "preview"],
    ["clip.mp4", "video", ["preview"], "preview"],
    ["clip.webm", "video", ["preview"], "preview"],
    ["sound.mp3", "audio", ["preview"], "preview"],
    ["paper.pdf", "pdf", ["preview"], "preview"],
    ["bundle.zip", "binary", ["preview"], "preview"],
    ["src/main.rs", "text", ["source"], "source"],
    ["Makefile", "text", ["source"], "source"],
    [".env", "text", ["source"], "source"],
  ])("%s is %s", (path, kind, modes, initial) => {
    expect(viewKind(path)).toBe(kind);
    expect(viewPlan(path)).toEqual({ kind, modes, initial });
  });

  it("reads quoted delimited rows", () => {
    expect(parseDelimited('a,b\r\n"x, y","say ""hi"""\n3,\n', ",")).toEqual([["a", "b"], ["x, y", 'say "hi"'], ["3", ""]]);
    expect(parseDelimited("a\tb\n1\t2", "\t")).toEqual([["a", "b"], ["1", "2"]]);
  });
});

describe("the preview", () => {
  it("renders Markdown formatted without interpreting its HTML, and keeps links from navigating", () => {
    mount(() => <FilePreview path="README.md" plan={viewPlan("README.md")} text={"# Title\n\nSome **bold** [link](https://example.com).\n\n<img src=x onerror=alert(1)>"} readMedia={none} />);
    expect(document.querySelector(".fv-doc h1")?.textContent).toBe("Title");
    expect(document.querySelector(".fv-doc strong")?.textContent).toBe("bold");
    expect(document.querySelector(".fv-doc img")).toBeNull();
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    document.querySelector(".fv-doc a")!.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
  });

  it("shows a picture from the machine's bytes", async () => {
    const asked: string[] = [];
    mount(() => <FilePreview path="img/shot.png" plan={viewPlan("img/shot.png")} text={null} readMedia={() => { asked.push("x"); return Promise.resolve({ text: null, media: { mime: "image/png", data: PNG } }); }} />);
    await settled();
    const img = document.querySelector<HTMLImageElement>(".fv-media img")!;
    expect(img.getAttribute("src")).toMatch(/^(blob:|data:image\/png;base64,)/);
    expect(img.alt).toBe("shot.png");
    expect(asked).toHaveLength(1);
  });

  it("plays a video inline with controls", async () => {
    mount(() => <FilePreview path="clip.mp4" plan={viewPlan("clip.mp4")} text={null} readMedia={() => Promise.resolve({ text: null, media: { mime: "video/mp4", data: "AAAA" } })} />);
    await settled();
    const video = document.querySelector<HTMLVideoElement>(".fv-media video")!;
    expect(video.controls).toBe(true);
    expect(video.getAttribute("src")).toMatch(/^(blob:|data:video\/mp4;base64,)/);
  });

  it("explains a picture the machine would not send, and a broken one", async () => {
    mount(() => <FilePreview path="big.png" plan={viewPlan("big.png")} text={null} readMedia={() => Promise.resolve({ text: null, reason: "This file is too large to show here." })} />);
    await settled();
    expect(document.querySelector(".fv-none")?.textContent).toContain("This file is too large to show here.");
    dispose?.(); document.body.innerHTML = "";
    mount(() => <FilePreview path="odd.avif" plan={viewPlan("odd.avif")} text={null} readMedia={() => Promise.resolve({ text: null, media: { mime: "image/avif", data: "AAAA" } })} />);
    await settled();
    document.querySelector(".fv-media img")!.dispatchEvent(new Event("error"));
    await settled();
    expect(document.querySelector(".fv-none")?.textContent).toContain("cannot be played or shown here");
  });

  it("gives an unsupported file a clear placeholder with a way to its text", () => {
    let toSource = 0;
    mount(() => <FilePreview path="bundle.zip" plan={viewPlan("bundle.zip")} text={null} readMedia={none} onSource={() => toSource++} />);
    const panel = document.querySelector(".fv-none")!;
    expect(panel.getAttribute("role")).toBe("status");
    expect(panel.textContent).toContain("This file type has no view here.");
    (panel.querySelector("button") as HTMLButtonElement).click();
    expect(toSource).toBe(1);
    dispose?.(); document.body.innerHTML = "";
    mount(() => <FilePreview path="paper.pdf" plan={viewPlan("paper.pdf")} text={null} readMedia={none} />);
    expect(document.querySelector(".fv-none")?.textContent).toContain("PDF files cannot be shown inside this window.");
  });

  it("formats JSON and says when it does not parse", () => {
    mount(() => <FilePreview path="a.json" plan={viewPlan("a.json")} text={'{"a":[1,2]}'} readMedia={none} />);
    expect(document.querySelector(".fv-json")?.textContent).toContain('"a": [');
    dispose?.(); document.body.innerHTML = "";
    mount(() => <FilePreview path="a.json" plan={viewPlan("a.json")} text={"{oops"} readMedia={none} />);
    expect(document.querySelector(".fv-none")?.textContent).toContain("does not parse");
  });

  it("shows a CSV as a table", () => {
    mount(() => <FilePreview path="t.csv" plan={viewPlan("t.csv")} text={"name,n\nfig,2\n"} readMedia={none} />);
    expect([...document.querySelectorAll(".fv-table th")].map(e => e.textContent)).toEqual(["name", "n"]);
    expect([...document.querySelectorAll(".fv-table td")].map(e => e.textContent)).toEqual(["fig", "2"]);
  });

  it("previews SVG only as an image, so its scripts never run", () => {
    mount(() => <FilePreview path="a.svg" plan={viewPlan("a.svg")} text={'<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>'} readMedia={none} />);
    expect(document.querySelector("script")).toBeNull();
    expect(document.querySelector(".fv-media img")?.getAttribute("src")).toMatch(/^data:image\/svg\+xml;charset=utf-8,/);
  });

  it("rebuilds HTML from structure only: no script, handler, style, frame, form or remote load", () => {
    const { node } = safeHtml('<h1 onclick="x()" style="color:red">Hi</h1><script>alert(1)</script><style>body{}</style><iframe src="https://e.x"></iframe>' +
      '<form><input></form><a href="javascript:alert(1)">go</a><img src="https://e.x/t.png" alt="t"><img src="data:image/png;base64,iVBORw0KGgo=" alt="ok"><custom-el><b>kept</b></custom-el>');
    const box = document.createElement("div"); box.append(node);
    expect(box.querySelector("h1")?.textContent).toBe("Hi");
    expect(box.querySelector("h1")?.attributes).toHaveLength(0);
    expect(box.querySelectorAll("script, style, iframe, form, input, [onclick], [style], [href]")).toHaveLength(0);
    expect(box.querySelector("a")?.attributes).toHaveLength(1);
    expect([...box.querySelectorAll("img")].map(i => i.getAttribute("src"))).toEqual(["data:image/png;base64,iVBORw0KGgo="]);
    expect(box.textContent).toContain("[image: t]");
    expect(box.querySelector("b")?.textContent).toBe("kept");
  });
});

/** The Code screen itself, on a real port: what the owner sees on opening a file. */
function repoTransport(files: Record<string, FileRead>) {
  const repoFacts = { projectId: "p", remote: "", tree: Object.keys(files), sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "main", defaultBranch: "main",
    checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "main", path: "/cedar", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null, files: [] }] };
  const config = structuredClone(emptyWorkspace().machines[0].config);
  const reads: { file: string; media: boolean }[] = [];
  const offered = ["capabilities.get", "project.snapshot", "decisions.list", "history.page"];
  const d = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: {} }],
    "capabilities.get": () => capabilitiesWire(offered),
    "project.snapshot": () => snapshot("4", []),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "4" }),
    "history.page": () => ({ items: [], nextCursor: null, highWatermark: "0" }),
    "machine.call": message => {
      const { op, params } = message as { op: string; params: { file: string; media?: boolean } };
      if (op === "repo.snapshot") return { ok: true, result: repoFacts };
      if (op === "product.read") return { ok: true, result: { tasks: {}, sprints: [], messages: [] } };
      if (op === "workers") return { ok: true, result: [] };
      if (op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
      if (op === "config.read") return { ok: true, result: { config, connections: [], tokens: {}, usage: [] } };
      if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
      if (op === "repo.file") {
        reads.push({ file: params.file, media: !!params.media });
        const f = files[params.file];
        if (params.media) return f.media ? { ok: true, result: { file: params.file, text: null, revision: "m", media: f.media } } : { ok: false, code: "not_media", message: "This file type cannot be shown here." };
        return { ok: true, result: { file: params.file, revision: "r1", readOnly: null, writer: null, ...f, media: undefined } };
      }
      return { ok: true, result: {} };
    },
  });
  return { ...d, reads };
}
async function openCode(file: string, files: Record<string, FileRead>) {
  const d = repoTransport(files);
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/p/p/code?file=${encodeURIComponent(file)}`);
  mount(() => <Workspace port={createRealPort({ transport: d.transport, actorId: "owner", registry: true, localCode: true })} />);
  for (let i = 0; i < 40 && !document.querySelector(".fv-body, .cm-content"); i++) await settled();
  await settled();
  return d;
}
const button = (name: string) => [...document.querySelectorAll<HTMLButtonElement>(".fv-switch button")].find(b => b.textContent === name)!;

describe("opening a file in Code", () => {
  const FILES: Record<string, FileRead> = {
    "README.md": { text: "# Cedar\n\nHello **owner**.\n" },
    "shot.png": { text: null, reason: "This is a binary file.", media: { mime: "image/png", data: PNG } },
    "clip.webm": { text: null, reason: "This is a binary file.", media: { mime: "video/webm", data: "AAAA" } },
    "bundle.zip": { text: null, reason: "This is a binary file." },
  };

  it("opens Markdown formatted and switches to its source and back in one action each", async () => {
    await openCode("README.md", FILES);
    expect(document.querySelector(".fv-doc h1")?.textContent).toBe("Cedar");
    expect(button("Preview").getAttribute("aria-pressed")).toBe("true");
    button("Source").click();
    // CodeMirror can attach an empty editor before the asynchronous source is visible.
    await vi.waitFor(() => {
      expect(document.querySelector(".cm-content")?.textContent).toContain("# Cedar");
    }, { timeout: 5000 });
    expect(document.querySelector(".fv-doc")).toBeNull();
    button("Preview").click();
    await settled();
    expect(document.querySelector(".fv-doc h1")?.textContent).toBe("Cedar");
    expect(document.querySelector(".cm-content")).toBeNull();
  });

  it("shows a picture and a video from bytes read once, for viewing only", async () => {
    const d = await openCode("shot.png", FILES);
    for (let i = 0; i < 20 && !document.querySelector(".fv-media img"); i++) await settled();
    expect(document.querySelector(".fv-media img")).not.toBeNull();
    expect(d.reads.filter(r => r.media)).toEqual([{ file: "shot.png", media: true }]);
    expect(document.querySelector(".fv-switch")).toBeNull();
    dispose?.(); dispose = undefined; document.body.innerHTML = "";
    await openCode("clip.webm", FILES);
    for (let i = 0; i < 20 && !document.querySelector(".fv-media video"); i++) await settled();
    expect(document.querySelector<HTMLVideoElement>(".fv-media video")?.controls).toBe(true);
  });

  it("keeps the screen whole on an unsupported file and offers its text", async () => {
    await openCode("bundle.zip", FILES);
    expect(document.querySelector(".fv-none")?.textContent).toContain("This file type has no view here.");
    expect(document.querySelector(".etabs")).not.toBeNull();
    [...document.querySelectorAll<HTMLButtonElement>(".fv-none button")].find(b => b.textContent === "Show as text")!.click();
    await settled();
    expect(document.querySelector(".fv-body")).toBeNull();
    expect(document.body.textContent).toContain("This is a binary file.");
    expect(button("Preview")).toBeDefined();
  });
});
