import { createContext, useContext, type Accessor, type Setter } from "solid-js";
import type { MockStore } from "../store";
import type { DeskData, FilterId } from "./model";
import type { Prefs, Theme } from "./prefs";
import type { Route, Router } from "./router";

/** Desk-local mock memory. Cleared by Prototype → Reset together with the shared store; never persisted. */
export type Local = {
  receipts: Record<string, string>;
  messages: Record<string, string[]>;
  elsewhere: Record<string, { answer: string; at: string }>;
  loading: boolean;
  filters: Record<string, FilterId[]>;
  step: number;
  buffers: Record<string, string>;
  diskChanged: boolean;
  pushed: Record<string, boolean>;
  revoked: string[];
  relay: string;
  changes: { text: string; at: string }[];
  /** Task titles renamed by the user (mock), keyed by task id. */
  titles: Record<string, string>;
};
export const initialLocal = (): Local => ({
  receipts: {}, messages: {}, elsewhere: {}, loading: false, filters: {}, step: 0, buffers: {}, diskChanged: false,
  pushed: {}, revoked: [], relay: "", changes: [], titles: {},
});

export type ConfirmOpts = { title: string; body: string; confirm: string; danger?: boolean; mock?: boolean };
export type PaletteMode = "all" | "projects";

export type Desk = {
  store: MockStore; data: DeskData; router: Router; prefs: Prefs; theme: Theme;
  local: Accessor<Local>; setLocal: Setter<Local>;
  receipt(id: string, text: string, change?: string): void;
  /** Why mock mutations are unavailable for a project (or globally), or null. */
  blocked(projectId?: string): string | null;
  go(to: string | Route, opts?: { replace?: boolean }): void;
  openCreate(projectId?: string): void;
  openPalette(mode: PaletteMode): void;
  openHelp(): void;
  confirm(opts: ConfirmOpts): Promise<boolean>;
  announce(text: string): void;
  reset(): void;
  wide: Accessor<boolean>;
};
export const DeskContext = createContext<Desk>();
export function useDesk(): Desk {
  const desk = useContext(DeskContext);
  if (!desk) throw new Error("Desk context missing");
  return desk;
}
