import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { expect, it, vi } from "vitest";
import type { Attachment } from "@cortex/workspace-domain";
import { createAttachments } from "../src/composer";
import { WsContext, type Ws } from "../src/ctx";

it("returns a pending paste to its original editor across navigation and remount", async () => {
  let finish!: (files: Attachment[]) => void;
  const reading = new Promise<Attachment[]>(resolve => { finish = resolve; });
  const ws = { store: { readClipboardImage: () => reading, uploadAttachments: async (items: Attachment[]) => items } } as unknown as Ws;
  const [scope, select] = createSignal("worker A");
  let stage!: ReturnType<typeof createAttachments>;
  function Editor() { stage = createAttachments(() => ["task", scope()]); return null; }
  const mount = () => render(() => <WsContext.Provider value={ws}><Editor /></WsContext.Provider>, document.createElement("div"));
  let dispose = mount();
  try {
    stage.pasteNative();
    expect(stage.busy()).toBe(true);
    select("worker B");
    expect(stage.busy()).toBe(false);
    stage.refuse("B only");
    dispose();
    dispose = mount();
    const file = { name: "a.png", mimeType: "image/png", data: "YQ==" };
    finish([file]);
    select("worker A");
    await vi.waitFor(() => expect(stage.files()).toEqual([file]));
    expect(stage.error()).toBe("");
    select("worker B");
    expect(stage.files()).toEqual([]);
    expect(stage.error()).toBe("B only");
  } finally { dispose(); }
});
