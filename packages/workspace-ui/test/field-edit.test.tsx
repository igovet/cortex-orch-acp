// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CommandReceipt } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { FieldEdit, NOT_SAVED, NOTHING_TO_CANCEL, NOTHING_TO_SAVE, SETTLE_MS, WAIT_FOR_SAVE, type FieldEditProps } from "../src/field-edit";
import { flush, initial, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.useRealTimers(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const receipt = (ok: boolean, message = ok ? "Recorded" : "Refused"): CommandReceipt => ({ requestId: "r", ok, outcome: ok ? "recorded" : "refused", message });
const field = (label: string) => document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`);
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === text) ?? null;
const type = (el: HTMLInputElement, text: string) => { el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); };
const key = (el: Element, k: string) => el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
const off = (b: HTMLElement | null) => b?.getAttribute("aria-disabled") === "true" ? b.getAttribute("data-why") : null;

/** One editor on its own, with the saved value and the save under the test's hand. */
function mountField(extra: Partial<FieldEditProps> = {}, answers: ((text: string) => Promise<CommandReceipt>)[] = []) {
  const [value, setValue] = createSignal("cedar");
  const sent: string[] = [];
  const save = vi.fn((text: string) => { sent.push(text); return (answers.shift() ?? (() => Promise.resolve(receipt(true))))(text); });
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <FieldEdit label="Project name" value={value()} save={save} {...extra} />, root);
  return { setValue, save, sent };
}

describe("the shared Save and Cancel editor", () => {
  it("keeps the typed text when the saved value is read again, and saves it only on Save", async () => {
    const f = mountField();
    const el = field("Project name")!;
    type(el, "Cedar Notes");
    f.setValue("cedar"); f.setValue("renamed elsewhere"); await settle();
    expect(field("Project name")).toBe(el);
    expect(el.value).toBe("Cedar Notes");
    expect(f.save).not.toHaveBeenCalled();
    el.dispatchEvent(new Event("change", { bubbles: true })); el.dispatchEvent(new FocusEvent("focusout", { bubbles: true })); await settle();
    expect(f.save).not.toHaveBeenCalled();
    button("Save")!.click(); await settle();
    expect(f.sent).toEqual(["Cedar Notes"]);
  });

  it("keeps Save off with its reason while nothing changed or the text is empty, and draws no words beside the buttons", async () => {
    const f = mountField();
    const el = field("Project name")!;
    expect(off(button("Save"))).toBe(NOTHING_TO_SAVE);
    expect(off(button("Cancel"))).toBe(NOTHING_TO_CANCEL);
    type(el, "   ");
    expect(off(button("Save"))).toBe("Project name cannot be empty.");
    expect(el.getAttribute("aria-invalid")).toBe("true");
    button("Save")!.click(); key(el, "Enter"); await settle();
    expect(f.save).not.toHaveBeenCalled();
    // The reason is a declaration for the reason box; none of it is printed in the editor.
    expect(el.closest(".ws-fe")!.textContent).toBe("SaveCancel");
    // Each button names its own message line, so a settings row's slot does not take its reason.
    expect(button("Save")!.getAttribute("aria-describedby")).toBe(el.closest(".ws-fe")!.querySelector('[role="alert"]')!.id);
    type(el, "cedar  ");
    expect(off(button("Save"))).toBe(NOTHING_TO_SAVE);
  });

  it("saves the trimmed text on Enter, and Escape puts the saved text back", async () => {
    const f = mountField();
    const el = field("Project name")!;
    el.focus();
    type(el, "abc"); key(el, "Escape"); await settle();
    expect(el.value).toBe("cedar");
    expect(document.activeElement).toBe(el);
    expect(f.save).not.toHaveBeenCalled();
    type(el, "  Cedar Notes  "); key(el, "Enter"); await settle();
    expect(f.sent).toEqual(["Cedar Notes"]);
    expect(document.activeElement).toBe(el);
  });

  it("keeps the typed text and says why when a save is refused, until the next Save or Cancel", async () => {
    const f = mountField({}, [() => Promise.resolve(receipt(false, "That name is already used on this machine.")), () => Promise.reject(new Error("lost")), () => Promise.resolve(receipt(true))]);
    const el = field("Project name")!;
    const message = () => el.closest(".ws-fe")!.querySelector('[role="alert"]')!.textContent;
    type(el, "Cedar"); key(el, "Enter"); await settle();
    expect(el.value).toBe("Cedar");
    expect(message()).toBe("That name is already used on this machine.");
    expect(off(button("Save"))).toBeNull();
    // A read of the machine changes neither the text nor the message.
    f.setValue("cedar "); await settle();
    expect(el.value).toBe("Cedar");
    expect(message()).toBe("That name is already used on this machine.");
    button("Save")!.click(); await settle();
    expect(message()).toBe(NOT_SAVED);
    expect(el.value).toBe("Cedar");
    button("Cancel")!.click(); await settle();
    expect(message()).toBe("");
    expect(el.value).toBe("cedar ");
    expect(f.sent).toEqual(["Cedar", "Cedar"]);
  });

  it("waits while a save is under way: read-only, busy, and saved once however often Enter is pressed", async () => {
    let finish!: (r: CommandReceipt) => void;
    const f = mountField({}, [() => new Promise(resolve => { finish = resolve; })]);
    const el = field("Project name")!;
    type(el, "Cedar"); key(el, "Enter"); await settle();
    expect(button("Save")!.getAttribute("aria-busy")).toBe("true");
    expect(el.readOnly).toBe(true);
    expect(off(button("Cancel"))).toBe(WAIT_FOR_SAVE);
    key(el, "Enter"); button("Save")!.click(); key(el, "Escape"); await settle();
    expect(f.save).toHaveBeenCalledTimes(1);
    expect(el.value).toBe("Cedar");
    finish(receipt(true)); await settle();
    expect(button("Save")!.getAttribute("aria-busy")).toBeNull();
    expect(el.readOnly).toBe(false);
  });

  it("shows the saved text until the machine reads it back, and the machine's value after that at the latest", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const f = mountField();
    const el = field("Project name")!;
    type(el, "Cedar"); key(el, "Enter");
    for (let i = 0; i < 8; i++) await Promise.resolve();
    expect(f.sent).toEqual(["Cedar"]);
    expect(el.value).toBe("Cedar");
    expect(off(button("Save"))).toBe(NOTHING_TO_SAVE);
    vi.advanceTimersByTime(SETTLE_MS);
    // The machine never confirmed it: what it reports is what the field shows.
    expect(el.value).toBe("cedar");
    vi.useRealTimers();
    type(el, "Cedar Notes"); key(el, "Enter"); await settle();
    f.setValue("Cedar Notes"); await settle();
    expect(el.value).toBe("Cedar Notes");
  });

  it("opens a name where it stands, and gives focus back to it after Save or Cancel", async () => {
    const f = mountField({ closed: { text: saved => saved, class: "ws-edit", label: saved => `Project name: ${saved}` } });
    const name = () => document.querySelector<HTMLButtonElement>("button.ws-edit");
    expect(field("Project name")).toBeNull();
    name()!.click(); await settle();
    const el = field("Project name")!;
    expect(document.activeElement).toBe(el);
    expect(el.value).toBe("cedar");
    button("Cancel")!.click(); await settle();
    expect(field("Project name")).toBeNull();
    expect(document.activeElement).toBe(name());
    name()!.click(); await settle();
    type(field("Project name")!, "Cedar"); key(field("Project name")!, "Enter"); await settle();
    expect(f.sent).toEqual(["Cedar"]);
    expect(field("Project name")).toBeNull();
    expect(name()!.textContent).toBe("Cedar");
    expect(name()!.getAttribute("aria-label")).toBe("Project name: Cedar");
    expect(document.activeElement).toBe(name());
  });

  it("closes an opened name that was not changed when focus leaves, and keeps a changed one open", async () => {
    mountField({ closed: { text: saved => saved, class: "ws-edit" } });
    const outside = document.createElement("button"); document.body.append(outside);
    const name = () => document.querySelector<HTMLButtonElement>("button.ws-edit");
    name()!.click(); await settle();
    outside.focus(); await settle();
    expect(field("Project name")).toBeNull();
    name()!.click(); await settle();
    type(field("Project name")!, "Cedar"); outside.focus(); await settle();
    expect(field("Project name")!.value).toBe("Cedar");
    key(field("Project name")!, "Escape"); await settle();
    expect(field("Project name")).toBeNull();
    expect(name()!.textContent).toBe("cedar");
  });

  it("in the title band, opens a refused save in a box under the field and keeps the text", async () => {
    const f = mountField({ band: true, closed: { text: saved => saved, class: "ws-title" } }, [() => Promise.resolve(receipt(false, "The task can no longer be renamed."))]);
    document.querySelector<HTMLButtonElement>("button.ws-title")!.click(); await settle();
    const el = field("Project name")!;
    type(el, "Cedar"); key(el, "Enter"); await settle();
    expect(f.sent).toEqual(["Cedar"]);
    const tip = el.closest(".ws-fe")!.querySelector<HTMLElement>(".ws-fe-tip")!;
    expect(tip.getAttribute("role")).toBe("alert");
    expect(tip.textContent).toBe("The task can no longer be renamed.");
    expect(el.closest(".ws-fe-line")!.textContent).toBe("SaveCancel");
    expect(field("Project name")!.value).toBe("Cedar");
  });

  it("says why a field that cannot be edited is off, on the field and on Save", async () => {
    const f = mountField({ off: "Mars is offline" });
    const el = field("Project name")!;
    expect(el.readOnly).toBe(true);
    expect(off(el)).toBe("Mars is offline");
    expect(off(button("Save"))).toBe("Mars is offline");
    key(el, "Enter"); await settle();
    expect(f.save).not.toHaveBeenCalled();
    dispose!(); document.body.innerHTML = "";
    mountField({ off: "Mars is offline", closed: { text: saved => saved, class: "ws-edit" } });
    const name = document.querySelector<HTMLButtonElement>("button.ws-edit")!;
    expect(off(name)).toBe("Mars is offline");
    name.click(); await settle();
    expect(field("Project name")).toBeNull();
  });
});

describe("renaming a project", () => {
  const mountProject = async () => {
    const s = scriptedPort({ capabilities: { everything: false, operations: ["workspace.rename_project"] } });
    const state = withTask(initial(), "One");
    history.replaceState(null, "", `/m/${state.machines[0].id}/p/p/settings`);
    const root = document.createElement("div"); document.body.append(root);
    dispose = render(() => <Workspace port={s.port} />, root);
    await settle();
    s.emit(projection("1", state)); await settle();
    return { s, state };
  };

  it("keeps the typed name when an unrelated read rebuilds the page, and sends it on Save", async () => {
    const { s, state } = await mountProject();
    const el = field("Project name")!;
    expect(el.value).toBe("Project");
    el.focus(); type(el, "Cedar Notes"); await settle();
    // Any read of the machine rebuilt the project's summary, and the field used to be written back to the saved name.
    s.emit(projection("2", withTask(initial(), "Two"))); await settle();
    expect(field("Project name")).toBe(el);
    expect(el.value).toBe("Cedar Notes");
    expect(document.activeElement).toBe(el);
    expect(s.commands).toEqual([]);
    button("Save")!.click(); await settle();
    expect(s.commands.map(c => c.payload)).toEqual([{ type: "rename_project", projectId: "p", name: "Cedar Notes" }]);
    s.receipt(true); await settle();
    expect(el.value).toBe("Cedar Notes");
    s.emit(projection("3", { ...state, projects: state.projects.map(p => ({ ...p, name: "Cedar Notes" })) })); await settle();
    expect(el.value).toBe("Cedar Notes");
    expect(off(button("Save"))).toBe(NOTHING_TO_SAVE);
  });

  it("keeps the typed name and shows the machine's reason when the rename is refused", async () => {
    const { s } = await mountProject();
    const el = field("Project name")!;
    type(el, "Cedar Notes"); key(el, "Enter"); await settle();
    expect(s.commands).toHaveLength(1);
    s.receipt(false); await settle();
    expect(el.value).toBe("Cedar Notes");
    expect(el.closest(".ws-fe")!.querySelector('[role="alert"]')!.textContent).toBe("Refused");
  });
});
