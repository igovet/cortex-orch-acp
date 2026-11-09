import { createContext, useContext, type Accessor } from "solid-js";
import type { SetStoreFunction } from "solid-js/store";
import type { MockStore } from "../../store";
import type { Conn, EventIndex, TabId, TaskView } from "./model";

export type Pane = { kind: "task"; tab: TabId } | { kind: "needs" } | { kind: "create" } | { kind: "connection" } | { kind: "code" };
export type ConfirmOpts = { title: string; effect: string; scope: string; confirmLabel: string; onConfirm: () => void };

/** Concept-local, memory-only UI state. Shared fixture facts stay in the shared store. */
export type Ui = {
  editor: Record<string, string>; saveConflict: boolean; compare: boolean; reloaded: boolean; saveAs: boolean; markedResolved: boolean;
  terminalOpen: boolean; terminal: string[]; codeMode: "files" | "search"; query: string; regex: boolean;
  requests: Record<string, "changes" | "rejected">; push: Record<string, "unknown">;
  phoneAnswers: string[]; changesSince: string[];
};

export type Desk = {
  store: MockStore;
  views: Accessor<TaskView[]>;
  /** Tasks the current scenario shows (empty for the empty and error scenarios). */
  visible: Accessor<TaskView[]>;
  byId: Accessor<Map<string, TaskView>>;
  events: Accessor<EventIndex>;
  conn: Accessor<Conn>;
  /** Reason every mock mutating control is unavailable, or null. */
  block: Accessor<string | null>;
  scale: Accessor<number>;
  pane: Accessor<Pane | null>;
  setPane(p: Pane | null): void;
  openTask(id: string, tab?: TabId): void;
  setTab(t: TabId): void;
  receipt(key: string): string | undefined;
  setReceipt(key: string, msg: string, focus?: boolean): void;
  confirm(opts: ConfirmOpts): void;
  ui: Ui;
  setUi: SetStoreFunction<Ui>;
  addDraft(id: string): void;
  focusSoon(id: string): void;
};

export const DeskContext = createContext<Desk>();
export const useDesk = () => useContext(DeskContext)!;
export const receiptId = (key: string) => `a-rcpt-${key.replace(/[^a-zA-Z0-9-]/g, "-")}`;
