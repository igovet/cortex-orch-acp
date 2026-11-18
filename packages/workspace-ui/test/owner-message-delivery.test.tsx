import { render } from "solid-js/web";
import { afterEach, describe, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Message, type Task, type TimelineEntry } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { messageDelivery } from "../src/session-model";
import { clock } from "../src/selectors";
import { flush, projection, scriptedPort, withTask } from "./support";

// An owner message to the coordinator shows where it is from the runtime's facts, and the task lists the owner's own
// write denials with a Remove control that lifts exactly that one.
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const AT = Date.UTC(2026, 10, 18, 9, 30);
const message = (id: string, text: string, delivery: Message["delivery"], deliveredAt: number | null = null): Message =>
  ({ id, taskId: "t", workerId: null, text, mode: "send_now", delivery, at: AT, deliveredAt, failure: null } as unknown as Message);

function world() {
  const state = withTask(emptyWorkspace(), "Steering facts");
  Object.assign(state.tasks[0], { status: "working", process: "live", executionId: "e",
    writeDenials: [{ path: "secrets/key.txt", setAt: AT }] } as Partial<Task>);
  const session = emptySession("t");
  session.timeline = [
    { id: "s1", kind: "said", text: "Use the other output" },
    { id: "m1", kind: "message", messageId: "taken" },
    { id: "m2", kind: "message", messageId: "waiting" },
  ] as TimelineEntry[];
  state.sessions = [session];
  state.messages = [
    message("said", "Use the other output", "delivered", AT + 60_000),
    message("taken", "Queued and taken", "sending"),
    message("waiting", "Recorded, not yet handed", "recorded"),
  ];
  state.machines[0].connection = "connected";
  return state;
}
async function mount(path: string) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", world())); await settle();
  return p;
}

describe("owner message delivery", () => {
  it("names each runtime state in the owner's words", () => {
    expect(messageDelivery({ delivery: "sending", deliveredAt: null, failure: null }, false)).toBe("Taken from the queue for delivery");
    expect(messageDelivery({ delivery: "recorded", deliveredAt: null, failure: null }, false)).toBe("Received, waiting for the coordinator's next step");
    expect(messageDelivery({ delivery: "recorded", deliveredAt: null, failure: null }, true)).toBe("Received, kept for when you resume");
    expect(messageDelivery({ delivery: "delivered", deliveredAt: AT, failure: null }, false)).toBe(`Delivered to the coordinator ${clock(AT)}`);
    expect(messageDelivery({ delivery: "failed", deliveredAt: null, failure: "Not delivered: invalid_message" }, false)).toBe("Not delivered: invalid_message");
  });

  it("shows taken, waiting and delivered messages in the conversation", async () => {
    await mount(`/m/${LOCAL_MACHINE_ID}/t/t`);
    const meta = all(".timeline .mine-meta").map(m => m.textContent ?? "");
    expect(meta.some(t => t.includes(`Delivered to the coordinator ${clock(AT + 60_000)}`))).toBe(true);
    expect(meta.some(t => t.includes("Taken from the queue for delivery"))).toBe(true);
    expect(meta.some(t => t.includes("Received, waiting for the coordinator's next step"))).toBe(true);
  });

  it("lists the owner's write denials and lifts one with Remove", async () => {
    const p = await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers`);
    const row = all("[aria-label='Write denied: secrets/key.txt']")[0];
    expect(row).toBeTruthy();
    expect(row.textContent).toContain(`Set by your message ${clock(AT)}`);
    row.querySelector<HTMLButtonElement>("button")!.click();
    await settle();
    expect(p.commands.at(-1)?.payload).toMatchObject({ type: "allow_write", taskId: "t", path: "secrets/key.txt" });
  });
});
