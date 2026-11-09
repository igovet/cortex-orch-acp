import { createContext, useContext, type Accessor } from "solid-js";
import type { MockStore } from "../../store";
import type { Connection } from "./model";

export type DeepSurface = "discussion" | "diff" | "code" | "git" | "worker";
export type ConfirmRequest = { title: string; effect: string; scope: string; confirmLabel: string };

export type ConceptContext = {
  store: MockStore;
  conn: Accessor<Connection>;
  /** Reason every mock mutating control is disabled, or null when actions are available. */
  blocked: Accessor<string | null>;
  receipts: Accessor<Record<string, string>>;
  setReceipt: (key: string, message: string) => void;
  announce: (message: string) => void;
  openDeep: (surface: DeepSurface) => void;
  openTask: (taskId: string) => void;
  openCreate: () => void;
  openAll: () => void;
  openPage: (page: "connection" | "settings") => void;
  confirm: (request: ConfirmRequest) => Promise<boolean>;
  textScale: Accessor<number>;
  sectionOpen: (taskId: string, n: number) => boolean;
  toggleSection: (taskId: string, n: number, open?: boolean) => void;
  terminalOpen: Accessor<boolean>;
  setTerminalOpen: (open: boolean) => void;
};

export const Ctx = createContext<ConceptContext>();
export function useC(): ConceptContext {
  const value = useContext(Ctx);
  if (!value) throw new Error("Concept C context is missing");
  return value;
}

let counter = 0;
export const uid = (prefix: string) => `c-${prefix}-${++counter}`;
