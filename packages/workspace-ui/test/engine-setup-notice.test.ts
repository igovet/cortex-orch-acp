// @vitest-environment jsdom
import { createComponent } from "solid-js";
import { render } from "solid-js/web";
import { expect, it, vi } from "vitest";
import { EngineSetupNotice } from "../src/engine-setup-notice";
import { engineSetupNote } from "../src/engine-setup";

it("removes progress on the next status read when a pass stops or completes", async () => {
  vi.useFakeTimers();
  const root = document.createElement("div");
  let phase = "downloading";
  const read = vi.fn(async () => ({ engines: { "codex-acp": phase, "claude-agent-acp": "ready" } }));
  const dispose = render(() => createComponent(EngineSetupNotice, { read }), root);
  try {
    await vi.advanceTimersByTimeAsync(0);
    expect(root.textContent).toBe("Codex: Preparing the qualified adapter… ");
    for (const next of ["interrupted", "connecting", "ready", "opted_out", "downloaded", "download_failed", "login_required", "catalog_failed", "setup_failed"]) {
      phase = next;
      await vi.advanceTimersByTimeAsync(3000);
      if (next === "connecting") expect(root.textContent).toContain("Connecting");
      else expect(root.querySelector('[role="status"]')).toBeNull();
    }
  } finally { dispose(); vi.useRealTimers(); }
});

it("shows an interrupted first run alongside the observed adapter or connection in Settings", () => {
  const setup = { automatic: true, automaticPhase: "interrupted", autoConnectDisabled: false, reason: null };
  expect(engineSetupNote({ ...setup, status: "not_downloaded" })).toContain("Use Download engine to retry.");
  expect(engineSetupNote({ ...setup, status: "connected" })).toBe("Connected · Last automatic setup was interrupted.");
});

it("keeps the recorded failure stage and safe reason in Connections next to current connection status", () => {
  const setup = { status: "connected", automatic: true, automaticPhase: "setup_failed", autoConnectDisabled: false, reason: null } as const;
  expect(engineSetupNote(setup)).toBe("Connected · Last automatic setup failed.");
  expect(engineSetupNote({ ...setup, automaticStage: "checking_saved_catalog", reason: "catalog_unreadable" }))
    .toBe("Connected · Last automatic setup failed while checking the saved model list. · catalog_unreadable");
  expect(engineSetupNote({ ...setup, automaticStage: "/private/path", reason: "private stderr" }))
    .toBe("Connected · Last automatic setup failed.");
});
