import { createConnection } from "node:net";
import { render } from "solid-js/web";
import { expect, it } from "vitest";
import { Workspace } from "../src";
import { createRealPort, type RealTransport } from "../src/port/real";

const socket = process.env.WORKSPACE_CONTRACT_SOCKET;
function rpc(operation: string, message: unknown = {}): Promise<any> {
  return new Promise((resolve, reject) => {
    const stream = createConnection(socket!); let data = "";
    stream.setTimeout(12000, () => stream.destroy(new Error("private fixture timeout")));
    stream.on("error", reject);
    stream.on("connect", () => stream.write(JSON.stringify({operation, message}) + "\n"));
    stream.on("data", chunk => {
      data += chunk.toString(); if (!data.includes("\n")) return; stream.end();
      const reply = JSON.parse(data); if (reply.error) reject(new Error(reply.error)); else resolve(reply.result);
    });
  });
}
const transport: RealTransport = { request: rpc, subscribe: () => () => {} };
const until = async (test: () => boolean) => {
  const end = Date.now() + 10000;
  while (!test() && Date.now() < end) await new Promise(r => setTimeout(r, 25));
  expect(test(), "private copied review state").toBe(true);
};
it.skipIf(!socket)("existing answer and technically blocked work accept through the real UI and capture owner messages", async () => {
  const f = await rpc("fixture");
  for (const item of f.tasks) {
    localStorage.clear(); sessionStorage.clear();
    history.replaceState(null, "", `/m/local/t/${item.taskId}/changes`);
    const root = document.createElement("div"); document.body.append(root);
    const p = createRealPort({transport, actorId: f.actorId, registry: true});
    const dispose = render(() => <Workspace port={p} />, root);
    try {
      const accept = () => [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => /^Accept v1$/.test(b.textContent?.trim() ?? ""));
      await until(() => !!accept());
      expect(accept()!.getAttribute("aria-disabled")).toBeNull();
      expect(root.textContent).not.toContain("verification_executable_unavailable");
      expect(root.textContent).not.toMatch(/\br_[a-f0-9]{12}\b/);
      expect(root.textContent).not.toContain("1 check failed");
      accept()!.click();
      if (item.technical) {
        await until(() => !!document.querySelector('dialog[aria-label="Accept with open checks"] input'));
        const d = document.querySelector('dialog[aria-label="Accept with open checks"]')!;
        const confirm = d.querySelector<HTMLButtonElement>("[data-bar=\"dialog\"] .btn-primary")!;
        expect(confirm.getAttribute("aria-disabled")).toBe("true");
        d.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click();
        confirm.click();
      }
      await until(() => root.textContent?.includes("Accepted v1") === true);
      const result = await p.command({requestId: crypto.randomUUID(), operation:"send_message", expectedVersion:null,
        payload:{type:"send_message", taskId:item.taskId, text:"Please clarify this result.", mode:"send_now"}});
      expect(result.ok).toBe(true);
    } finally { dispose(); root.remove(); }
  }
});
