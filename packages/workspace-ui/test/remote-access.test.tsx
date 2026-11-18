// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { LOCAL_MACHINE_ID, type RemoteStatus } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { relayProblem } from "../src/remote-access";
import { capabilitiesWire, flush, scriptedTransport } from "./support";

/** Settings > Remote access of this computer, against the real port with the machine service's remote.* scripted. */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); document.documentElement.removeAttribute("data-theme"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
const button = (text: string, within: ParentNode = document) => [...within.querySelectorAll<HTMLElement>("button")].find(b => b.textContent?.trim() === text);
const page = () => document.querySelector<HTMLElement>(".set-page")!;
const REMOTE = ["remote.status", "remote.enable", "remote.pairing.start", "remote.pairing.cancel", "remote.pairing.decide", "remote.devices.revoke", "remote.relay.set"];

const base = (patch: Partial<RemoteStatus> = {}): RemoteStatus => ({ available: true, enabled: true, machineId: "m-1", relay: { url: "https://relay.example.org", source: "default" }, pairing: null, pending: [], devices: [], ...patch });
/** A 21 x 21 matrix with finder-like corners, so the module count is known. */
const QR = Array.from({ length: 21 }, (_, y) => Array.from({ length: 21 }, (_, x) => (x < 7 && y < 7) || (x > 13 && y < 7) || (x < 7 && y > 13) || (x + y) % 3 === 0 ? "1" : "0").join(""));

function machine(status: () => RemoteStatus, methods = ["projects.list", ...REMOTE]) {
  const answer = (operation: string) => (message: unknown) => { s.last = { operation, message }; return status(); };
  const s = Object.assign(scriptedTransport({
    "service.capabilities": () => ({ methods }),
    "projects.list": () => [],
    "capabilities.get": () => capabilitiesWire([]),
    ...Object.fromEntries(REMOTE.map(op => [op, answer(op)])),
  }), { last: null as null | { operation: string; message: unknown } });
  return s;
}
async function mount(s: ReturnType<typeof machine>) {
  history.replaceState(null, "", `/settings/machine/${LOCAL_MACHINE_ID}/remote`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "peer", registry: true })} />, root);
  await settle();
}
const sent = (s: ReturnType<typeof machine>, operation: string) => s.calls.filter(c => c.operation === operation).map(c => c.message);

describe("remote access of this computer", () => {
  it("says so plainly when the service has no remote access, with no control", async () => {
    await mount(machine(() => base(), ["projects.list"]));
    expect(page().textContent).toContain("Remote access is not available in this build.");
    expect(page().querySelector("input")).toBeNull();
    expect(page().textContent).not.toMatch(/7KQ2|pairing code/i);
  });

  it("shows the reason when the machine cannot offer it, and nothing that could not work", async () => {
    await mount(machine(() => base({ available: false, reason: "The relay key store is locked." })));
    expect(page().textContent).toContain("Remote access is unavailable on this machine. The relay key store is locked.");
    expect(page().querySelector('input[aria-label="Allow remote access"]')).toBeNull();
    expect(button("Show QR code")).toBeUndefined();
  });

  it("is off by default and turns on through the service", async () => {
    let status = base({ enabled: false });
    const s = machine(() => status);
    await mount(s);
    const toggle = page().querySelector<HTMLInputElement>('input[aria-label="Allow remote access"]')!;
    expect(toggle.checked).toBe(false);
    expect(page().textContent).toContain("Turning it off disconnects every device.");
    expect(button("Show QR code")).toBeUndefined();
    status = base({ enabled: true });
    toggle.click(); await settle();
    expect(sent(s, "remote.enable")).toEqual([{ enabled: true }]);
    expect(button("Show QR code")).toBeTruthy();
  });

  it("says pairing works on the same network only when no relay is set", async () => {
    await mount(machine(() => base({ relay: { url: null, source: "none" } })));
    expect(page().textContent).toContain("pairing works on the same network only");
  });

  it("draws the QR code from the matrix on a light plate in the dark theme, with a countdown, Copy link and New link", async () => {
    document.documentElement.dataset.theme = "dark";
    const s = machine(() => base({ pairing: { link: "cortex-pair:abc", qr: QR, expiresAt: Date.now() + 272_400 } }));
    await mount(s);
    const svg = page().querySelector<SVGSVGElement>("svg.ra-qr")!;
    expect(svg.getAttribute("viewBox")).toBe("0 0 29 29");
    const ones = QR.join("").split("").filter(c => c === "1").length;
    expect(svg.querySelector("path")!.getAttribute("d")!.match(/M/g)).toHaveLength(ones);
    expect(svg.querySelector("path")!.getAttribute("d")).toMatch(/^M4 4h1v1h-1z/);
    expect(svg.querySelector("rect")!.getAttribute("fill")).toBe("#ffffff");
    expect(svg.querySelector("path")!.getAttribute("fill")).toBe("#000000");
    expect(svg.querySelector("image")).toBeNull();
    expect(page().querySelector('[role="timer"]')!.textContent).toMatch(/^Works once · expires in 4:3\d$/);
    expect(button("Copy link")).toBeTruthy();
    button("New link")!.click(); await settle();
    expect(sent(s, "remote.pairing.start")).toEqual([{}]);
    button("Stop pairing")!.click(); await settle();
    expect(sent(s, "remote.pairing.cancel")).toEqual([{}]);
    const before = sent(s, "remote.status").length;
    s.send({ type: "refresh" }); await settle();
    expect(sent(s, "remote.status").length).toBeGreaterThan(before);
  });

  it("lists every pending request with both fingerprints, and Allow and Deny go to the service", async () => {
    const s = machine(() => base({ pending: [
      { requestId: "r1", deviceName: "Pixel 9", fingerprint: "4F2A 91C7 D03E", requestedAt: Date.now() },
      { requestId: "r2", deviceName: "Galaxy S24", fingerprint: "77AA 0012 FFEE", requestedAt: Date.now() },
    ] }));
    await mount(s);
    const cards = all(".ra-ask");
    expect(cards.map(c => c.getAttribute("aria-label"))).toEqual(["Pixel 9 wants to connect", "Galaxy S24 wants to connect"]);
    expect(cards[0].querySelectorAll("code")).toHaveLength(2);
    expect([...cards[0].querySelectorAll("code")].every(c => c.textContent === "4F2A 91C7 D03E")).toBe(true);
    expect(button("Allow", cards[0])!.className).toContain("btn-primary");
    button("Allow", cards[0])!.click(); await settle();
    button("Deny", all(".ra-ask")[1])!.click(); await settle();
    expect(sent(s, "remote.pairing.decide")).toEqual([{ requestId: "r1", allow: true }, { requestId: "r2", allow: false }]);
  });

  it("revokes a device only after its confirmation", async () => {
    const s = machine(() => base({ devices: [{ deviceId: "d1", name: "Galaxy S24", pairedAt: Date.now() - 86_400_000 * 3, lastSeenAt: Date.now(), connected: false }] }));
    await mount(s);
    expect(page().textContent).toMatch(/Paired \d+ \w{3} · last connected today \d\d:\d\d/);
    button("Revoke…")!.click(); await settle();
    expect(sent(s, "remote.devices.revoke")).toEqual([]);
    const dialog = document.querySelector<HTMLDialogElement>("dialog[open]")!;
    expect(dialog.textContent).toContain("Revoke Galaxy S24?");
    expect(dialog.textContent).toContain("Revoking disconnects it immediately.");
    button("Revoke", dialog)!.click(); await settle();
    expect(sent(s, "remote.devices.revoke")).toEqual([{ deviceId: "d1" }]);
  });

  it("checks a relay override before sending it, and Use default clears it", async () => {
    const s = machine(() => base({ relay: { url: "https://relay.mine.example", source: "override" } }));
    await mount(s);
    button("Change…")!.click(); await settle();
    const field = page().querySelector<HTMLInputElement>('input[aria-label="Relay server address"]')!;
    field.value = "relay.mine"; field.dispatchEvent(new Event("input", { bubbles: true }));
    button("Save")!.click(); await settle();
    expect(sent(s, "remote.relay.set")).toEqual([]);
    expect(page().textContent).toContain("starting with https://");
    field.value = "https://relay.other.example"; field.dispatchEvent(new Event("input", { bubbles: true }));
    button("Save")!.click(); await settle();
    expect(sent(s, "remote.relay.set")).toEqual([{ url: "https://relay.other.example" }]);
    button("Use default")!.click(); await settle();
    expect(sent(s, "remote.relay.set")).toEqual([{ url: "https://relay.other.example" }, { url: null }]);
  });

  it("accepts only https relay addresses", () => {
    expect(relayProblem("https://relay.example.org")).toBeNull();
    expect(relayProblem("http://relay.example.org")).toMatch(/https:\/\//);
    expect(relayProblem("")).toMatch(/Enter/);
  });
});
