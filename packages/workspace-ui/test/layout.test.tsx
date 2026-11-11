// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Modal, placeFocus, Pop } from "../src/kit";
import { Action, ActionBar, Region, ReasonTip, revealCutIn, Row, SectionHead, StatusLane, Strip, Toolbar, Whole } from "../src/layout";
import { flush } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const mount = (make: () => unknown) => { const root = document.createElement("div"); document.body.append(root); dispose = render(make as never, root); return root; };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector)!;

describe("a bar says why in its slot", () => {
  it("shows the primary's reason in words, inside the slot and never inside the control", async () => {
    const root = mount(() => <ActionBar start={<span>2 comments</span>} hint="Nothing is blocked">
      <Action kind="line" why="Commit is off">Commit</Action>
      <Action kind="primary" why="Say what should change">Send</Action>
    </ActionBar>);
    await flush();
    const slot = $(".l-reason");
    expect(slot.textContent).toBe("Say what should change");
    const send = [...root.querySelectorAll("button")].find(b => b.textContent === "Send")!;
    expect(send.getAttribute("aria-describedby")).toBe(slot.id);
    expect(send.textContent).toBe("Send");
    expect(root.querySelectorAll(".l-reason")).toHaveLength(1);
  });

  it("takes the reason from what the control declares, so ws.needs keeps working unchanged", async () => {
    const needs = { "aria-disabled": "true", "data-why": "Not offered on this machine" } as const;
    mount(() => <ActionBar><Action kind="primary" {...needs}>Start</Action></ActionBar>);
    await flush();
    expect($(".l-reason").textContent).toBe("Not offered on this machine");
    expect($("button").dataset.why).toBe("Not offered on this machine");
  });

  it("goes back to the hint and clears the description the moment the control can be used", async () => {
    const [why, setWhy] = createSignal<string | undefined>("Tick the box first");
    mount(() => <ActionBar hint="Accept moves the task along"><Action kind="primary" why={why()}>Accept</Action></ActionBar>);
    await flush();
    expect($(".l-reason").textContent).toBe("Tick the box first");
    setWhy(undefined);
    await flush();
    expect($(".l-reason").textContent).toBe("Accept moves the task along");
    expect($("button").hasAttribute("aria-describedby")).toBe(false);
    expect($("button").hasAttribute("aria-disabled")).toBe(false);
  });

  it("swaps the words for the control the owner moved to, and only the words", async () => {
    const root = mount(() => <ActionBar>
      <button class="btn btn-line" id="other" aria-disabled="true" data-why="No upstream branch">Push</button>
      <Action kind="primary" why="Tick the box first">Accept</Action>
    </ActionBar>);
    await flush();
    expect($(".l-reason").textContent).toBe("Tick the box first");
    root.querySelector<HTMLElement>("#other")!.focus();
    await flush();
    expect($(".l-reason").textContent).toBe("No upstream branch");
    root.querySelector<HTMLElement>("#other")!.blur();
    await flush();
    expect($(".l-reason").textContent).toBe("Tick the box first");
  });

  it("gives a dialog footer and a card the same bar with their own height", async () => {
    mount(() => <><ActionBar place="dialog"><Action>Cancel</Action></ActionBar><ActionBar place="card"><Action>Start</Action></ActionBar><ActionBar><Action>Go</Action></ActionBar></>);
    await flush();
    expect([...document.querySelectorAll<HTMLElement>(".l-bar")].map(b => b.dataset.bar)).toEqual(["dialog", "card", "action"]);
  });
});

describe("Action", () => {
  it("stays focusable and looks like a button while it cannot be pressed", () => {
    const press = vi.fn();
    mount(() => <Action kind="primary" why="Tick the box first" onClick={press}>Accept</Action>);
    const b = $<HTMLButtonElement>("button");
    expect(b.disabled).toBe(false);
    expect(b.classList.contains("btn-primary")).toBe(true);
    b.click();
    expect(press).not.toHaveBeenCalled();
  });
  it("answers a press once it can be used", () => {
    const press = vi.fn();
    mount(() => <Action kind="line" onClick={press}>Go</Action>);
    $("button").click();
    expect(press).toHaveBeenCalledOnce();
  });
});

describe("the rest of the bars and rows", () => {
  it("gives a toolbar its three zones and no reason slot", () => {
    const root = mount(() => <Toolbar start={<i id="s" />} mid={<i id="m" />} end={<i id="e" />} />);
    expect(root.querySelector(".l-z-start #s")).toBeTruthy();
    expect(root.querySelector(".l-z-mid #m")).toBeTruthy();
    expect(root.querySelector(".l-z-end #e")).toBeTruthy();
    expect(root.querySelector(".l-reason")).toBeNull();
    expect($(".l-toolbar").dataset.bar).toBe("toolbar");
  });
  it("heads a section with one heading, its count at the end and nothing for zero", () => {
    const root = mount(() => <><SectionHead title="Check manually" count={11} /><SectionHead title="Done" count={0} /></>);
    expect(root.querySelectorAll("h3")).toHaveLength(2);
    expect(root.querySelector(".l-sect .n")?.textContent).toBe("11");
    expect(root.querySelectorAll(".l-sect .n")).toHaveLength(1);
  });
  it("folds a long status lane behind a count, the most urgent first", () => {
    const item = (id: string, tone: "fail" | "warn" | "info") => ({ id, tone, text: id });
    const root = mount(() => <StatusLane items={[item("note", "info"), item("broken", "fail"), item("waiting", "warn"), item("extra", "info")]} />);
    expect([...root.querySelectorAll(".l-strip-t")].map(s => s.textContent)).toEqual(["broken", "waiting"]);
    const more = root.querySelector<HTMLElement>(".l-lane-more")!;
    expect(more.textContent).toBe("2 more");
    more.click();
    expect(root.querySelectorAll(".l-strip")).toHaveLength(4);
    expect(root.querySelector(".l-lane-more")?.textContent).toBe("Show less");
  });
  it("leaves a strip with nothing to fold alone", () => {
    const root = mount(() => <Strip tone="warn" action={<button>Open task</button>}>Queue paused</Strip>);
    expect(root.querySelector(".l-strip")?.getAttribute("data-tone")).toBe("warn");
    expect(root.querySelector(".l-strip-a button")?.textContent).toBe("Open task");
  });
  it("renders detail as the second line of a row and nothing when there is none", () => {
    const root = mount(() => <><Row title="installed fonts" detail="This claim stayed unverified" /><Row title="plain" /></>);
    expect(root.querySelectorAll(".l-row-d")).toHaveLength(1);
    expect(root.querySelector(".l-row-d")?.textContent).toBe("This claim stayed unverified");
  });
  it("keeps every class on the region element it was given", () => {
    const root = mount(() => <Region class="extra" data-test="r">Text</Region>);
    expect(root.querySelector(".l-region.extra")?.getAttribute("data-test")).toBe("r");
  });
});

describe("the reason of a control outside a bar", () => {
  const ask = (root: HTMLElement) => root.querySelector<HTMLElement>("[data-why]")!;
  it("opens the whole reason on a press, and closes on the next press elsewhere", async () => {
    const root = mount(() => <><ReasonTip /><button class="btn" aria-disabled="true" data-why="Resume it first">Continue</button><button id="other">Other</button></>);
    ask(root).click();
    expect($(".l-why-tip").textContent).toBe("Resume it first");
    expect(ask(root).textContent).toBe("Continue");
  });
  it("opens for keyboard movement only, never for a mouse hover or a script focus", () => {
    const root = mount(() => <><ReasonTip /><button class="btn" aria-disabled="true" data-why="Resume it first">Continue</button></>);
    ask(root).dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    ask(root).focus();
    expect($(".l-why-tip").textContent).toBe("");
    ask(root).blur();
    document.documentElement.setAttribute("data-keyboard", "");
    ask(root).focus();
    expect($(".l-why-tip").textContent).toBe("Resume it first");
    document.documentElement.removeAttribute("data-keyboard");
  });
  it("has nothing to add to a menu row, which already says it on its second line", () => {
    const root = mount(() => <><ReasonTip /><button class="mi" aria-disabled="true" data-why="Not offered">Archive<span class="sub">Not offered</span></button></>);
    ask(root).click();
    expect($(".l-why-tip").textContent).toBe("");
  });
});

describe("cut text opens from the keyboard as from a press", () => {
  // jsdom lays nothing out, so the boxes the test calls cut report a wider text than their box.
  const cutWhere = (selector: string) => Object.defineProperty(HTMLElement.prototype, "scrollWidth", { configurable: true, get(this: HTMLElement) { return this.matches(selector) ? 500 : 0; } });
  afterEach(() => { delete (HTMLElement.prototype as { scrollWidth?: number }).scrollWidth; });
  const key = (el: Element, name: string) => { const e = new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }); el.dispatchEvent(e); return e; };
  const tip = () => $(".l-why-tip").textContent;

  it("opens a cut path whole on Enter, as Row's title does", () => {
    cutWhere(".l-whole");
    mount(() => <><ReasonTip /><Whole text="packages/workspace-ui/src/a/very/long/path.tsx" /></>);
    const path = $(".l-whole");
    expect([path.getAttribute("role"), path.tabIndex]).toEqual(["button", 0]);
    expect(key(path, "Enter").defaultPrevented).toBe(true);
    expect(tip()).toBe("packages/workspace-ui/src/a/very/long/path.tsx");
  });
  it("opens a cut heading on Enter and a cut notice on Space", () => {
    cutWhere(".l-whole, .l-strip-t");
    mount(() => <><ReasonTip /><SectionHead title="Проверки, которые нужно пройти вручную" /><Strip tone="warn">Queue paused: the task is stopped. Resume it first.</Strip></>);
    key($(".l-sect-t .l-whole"), "Enter");
    expect(tip()).toBe("Проверки, которые нужно пройти вручную");
    key($(".l-strip-t"), " ");
    expect(tip()).toBe("Queue paused: the task is stopped. Resume it first.");
  });
  it("opens a cut name inside a panel on Space", () => {
    cutWhere(".trunc");
    mount(() => <><ReasonTip /><div ref={revealCutIn}><span class="trunc">feature/very-long-branch-name</span></div></>);
    const name = $(".trunc");
    expect(name.getAttribute("role")).toBe("button");
    expect(key(name, " ").defaultPrevented).toBe(true);
    expect(tip()).toBe("feature/very-long-branch-name");
  });
  it("leaves text that fits, and other keys, alone", () => {
    mount(() => <><ReasonTip /><Whole text="short" /></>);
    const path = $(".l-whole");
    expect(path.hasAttribute("role")).toBe(false);
    expect(key(path, "Enter").defaultPrevented).toBe(false);
    cutWhere(".l-whole");
    expect(key(path, "a").defaultPrevented).toBe(false);
    expect(tip()).toBe("");
  });
});

describe("focus a dialog or a panel puts on its first control", () => {
  afterEach(() => document.documentElement.removeAttribute("data-keyboard"));
  it("carries the quiet line after a pointer opened the dialog, until the next press", async () => {
    const [open, setOpen] = createSignal(false);
    mount(() => <><button id="trigger" onClick={() => setOpen(true)}>Accept v1</button>
      <Modal open={open()} onClose={() => setOpen(false)} label="Accept with open checks"><button data-autofocus>Keep reviewing</button><button>Accept</button></Modal></>);
    window.dispatchEvent(new Event("pointerdown"));
    $("#trigger").click();
    await flush();
    const keep = [...document.querySelectorAll("button")].find(b => b.textContent === "Keep reviewing")!;
    expect(document.activeElement).toBe(keep);
    expect(keep.hasAttribute("data-placed-focus")).toBe(true);
    expect($("#trigger").hasAttribute("data-placed-focus")).toBe(false);
    window.dispatchEvent(new Event("pointerdown"));
    expect(keep.hasAttribute("data-placed-focus")).toBe(false);
  });
  it("moves on with the focus, and leaves keyboard movement to the usual line", () => {
    mount(() => <><button id="a">Close task details</button><button id="b">Other</button></>);
    placeFocus($("#a"));
    expect($("#a").hasAttribute("data-placed-focus")).toBe(true);
    $("#b").focus();
    expect($("#a").hasAttribute("data-placed-focus")).toBe(false);
    document.documentElement.setAttribute("data-keyboard", "");
    placeFocus($("#a"));
    expect(document.activeElement).toBe($("#a"));
    expect($("#a").hasAttribute("data-placed-focus")).toBe(false);
  });
});

describe("a popover's label", () => {
  it("is carried by a role: a menu when it holds menu items, otherwise a dialog", () => {
    mount(() => <><Pop id="m-a" label="Task actions"><button role="menuitem">Start</button></Pop>
      <Pop id="m-b" label="About the automatic queue"><p>Passed checks leave each task ready.</p></Pop>
      <Pop id="m-c" label="Checkout" role="dialog"><button role="menuitem">Open</button></Pop></>);
    expect(["m-a", "m-b", "m-c"].map(id => [$(`#${id}`).getAttribute("role"), $(`#${id}`).getAttribute("aria-label")])).toEqual([
      ["menu", "Task actions"], ["dialog", "About the automatic queue"], ["dialog", "Checkout"]]);
  });
});
