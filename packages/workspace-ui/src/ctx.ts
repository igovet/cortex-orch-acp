import { createContext, useContext, type Accessor, type JSX } from "solid-js";
import type { CommandReceipt, ConnectOutcome, ConnectResult, Machine, McpServer, Sprint, SurfaceKey, WorkerRoute, WorkspaceCommand } from "@cortex/workspace-domain";
import type { CopyKey } from "./copy";
import type { WsDialog } from "./dialogs";
import type { Buffers } from "./editor-buffers";
import type { Panels } from "./prefs";
import type { Route, Router } from "./router";
import type { SettingsState } from "./settings-state";
import type { WorkspaceStore } from "./store";
import type { Theme } from "./theme";
import type { TerminalPort } from "./terminal-client";
import type { VoicePort } from "./voice-client";

/**
 * What a demo entry may add. The production entry passes none of this, so every line of simulation lives outside these
 * packages, and a screen that reads an absent slot simply offers less.
 */
export type DemoSlot = {
  /** The title-band chip and the menu it opens. */
  chip: () => JSX.Element; menu: () => JSX.Element;
  /** Words that say a screen is simulated. */
  copy: Partial<Record<CopyKey, string>>;
  /** Runs once the workspace exists, for anything that plays on its own. */
  attach: (ws: Ws) => void;
  autoplay: Accessor<boolean>; step: (taskId: string) => void;
  title: { draft: (description: string) => string; fails: () => boolean };
  /** The next stop, confirmed or left hanging. */
  stopConfirmed: () => boolean;
  takeSaveFault: () => "conflict" | "timeout" | null;
  pairFault: () => "expired" | "connector" | "fingerprint" | null; clearPairFault: () => void;
  dictation: { spoken: string[]; fault: () => string | null };
  connect: {
    delayMs: number; run: (server: McpServer, forced: ConnectOutcome | null, now: number) => ConnectResult; templates: () => McpServer[];
    outcome: (forced: Accessor<ConnectOutcome | "auto">, set: (outcome: ConnectOutcome | "auto") => void, id: string) => JSX.Element;
  };
  touchConfig: (machineId: string, surface: SurfaceKey) => void;
  reconnect: (machineId: string) => void;
  reloadProfiles: (machineId: string) => void;
  newId: (kind: "machine" | "project") => string;
  unique: () => string;
};

/** The frame of a native window with no OS decoration. A browser or the demo has none, and the title band stays passive. */
export type WindowChrome = {
  maximized: Accessor<boolean>;
  minimize: () => void; toggleMaximize: () => void; close: () => void; startDragging: () => void;
  /** Asked on every close, however the owner asked for it. True keeps the window open. */
  guardClose: (guard: () => boolean) => void;
  /** Closes without asking, after the owner decided. */
  closeNow: () => void;
  /** Why the window could not close, while that still matters. */
  closeError?: Accessor<string | null>;
};

/** What a phone knows of the one machine its workspace shows. The phone has no machine of its own. */
export type PhoneMachineView = Omit<import("./phone-shell").PhoneMachine, "id">;
/**
 * A phone around the shared interface: no tabs, no window controls, a drawer for the sidebar, and touch sizes. The machine
 * sheet and the phone's own settings belong to the phone shell, so the workspace only asks it to open them.
 */
export type PhoneFrame = {
  kind: "phone";
  machine: Accessor<PhoneMachineView>;
  openMachines: () => void;
  openSettings: () => void;
  retry: () => void;
  /** Receives the workspace once it exists, and null when it goes. */
  attach: (workspace: PhoneWorkspace | null) => void;
};
/** What the phone shell may ask of the workspace it holds. */
export type PhoneWorkspace = {
  /** Closes the top-most menu, dialog or drawer, else goes back. False when there was nothing left to close or go back to. */
  back: () => boolean;
  /** Opens this machine's own settings (connections, orchestration, storage). */
  openMachineSettings: () => void;
};
/** Which shell the interface runs in. The desktop is the default and changes nothing. */
export type Shell = { kind: "desktop" } | PhoneFrame;

export type Ws = {
  /** The local host's explicit handoff for links in rendered text. */
  openResultTarget?: (target: import("./result-links").ResultTarget) => Promise<void>;
  /** The shell around the interface. A phone has no tab strip, terminal, editor or shortcut hints. */
  shell: Shell;
  /** A keyboard hint, or nothing where there is no keyboard to speak of. */
  keys: (hint: string) => string | undefined;
  notify?: import("./notification-target").Notify;
  /** Which sidebar rows changed since the owner last opened them. Absent where no workspace keeps track (a lone screen in a test). */
  unseen?: import("./unseen").Unseen;
  store: WorkspaceStore; router: Router; panels: Panels; theme: Theme;
  /** Open files and the owner's unsaved text, above every screen. */
  buffers: Buffers;
  /** Present only in the labelled demo entry. */
  demo: DemoSlot | undefined;
  /** Present only in a native window that draws its own frame. */
  chrome: WindowChrome | undefined;
  /** The machine in view; a route names it, Settings keeps the last one. */
  machine: Accessor<Machine>;
  /** Task previewed in the Inspector. Selecting never navigates. */
  selected: Accessor<string | null>; select: (taskId: string | null) => void;
  go: (to: Route | string, opts?: { replace?: boolean; force?: boolean }) => void;
  settings: SettingsState;
  /** Screens with nothing to inspect turn the Inspector off, with the reason on its toggle. */
  inspectable: Accessor<boolean>;
  reworkIntent: Accessor<{ taskId: string } | null>;
  returnForRework: (taskId: string) => void;
  dismissRework: () => void;
  /** Archive a Done task: straight to the Archive, with its work kept and no question. */
  archiveTask: (taskId: string) => void;
  run: (command: WorkspaceCommand) => Promise<CommandReceipt>;
  /** Runs `then` when the command was recorded. What it shows still comes from the next projection. */
  ok: (command: WorkspaceCommand, then: (receipt: CommandReceipt) => void) => void;
  /** Keeps the pressed control, or the one named, loading and closed to a second press until a whole multi-step action ends. */
  busy: <T>(work: Promise<T>, control?: HTMLElement | null) => Promise<T>;
  /** Whether the machine offers a capability. */
  can: (capability: string) => boolean;
  /** Why a command is off here, or nothing when the machine offers it. */
  gate: (type: WorkspaceCommand["type"]) => string | undefined;
  /** Attributes for a control that sends this command: off, with the reason, when the machine does not offer it. */
  needs: (type: WorkspaceCommand["type"]) => { "aria-disabled"?: "true"; "data-why"?: string };
  /** Why this computer cannot open things in other programs, or nothing. Being offline is its own reason. */
  noOpener: (capability: string, reason: CopyKey) => string | undefined;
  /** The name of the external editor control, for what this computer has. */
  externalLabel: () => string;
  t: (key: CopyKey) => string;
  /** A recorded engine, model and effort in the words the session uses. */
  route: (route: WorkerRoute | null) => string | null;
  newId: (kind: "machine" | "project") => string;
  unique: () => string;
  terminal?: TerminalPort;
  voice?: VoicePort;
  terminalAvailable: Accessor<boolean>; toggleTerminal: () => void;
  openSprintMenu: (sprint: Pick<Sprint, "id" | "projectId">, trigger: HTMLElement, point?: { x: number; y: number }) => void;
  openPalette: () => void; openHelp: () => void; openTaskMenu: (taskId: string, trigger: HTMLElement, point?: { x: number; y: number }) => void;
  /** Opens New task or sprint; a sprint id preselects that sprint for a new task. */
  newItem: (kind: "task" | "sprint", project?: string | null, sprint?: string | null) => void;
  /** The one Stop: the same path from the title band, the composer, a card, its menu and the keyboard. */
  stopTask: (taskId: string) => void;
  /** Opens the Close task dialog. Closing is never a single click. `refused` hears each refusal of the dialog's close, in plain words. */
  closeTask: (taskId: string, how?: "external" | "already_in_target", refused?: (message: string) => void) => void;
  /** Unsent composer text, per task and worker, so moving around never loses it. */
  drafts: Map<string, string>;
  localDrafts?: import("./local-drafts").LocalDrafts;
  /** The one open review or Git dialog. */
  dialog: Accessor<WsDialog | null>; open: (dialog: WsDialog) => void; closeDialog: () => void;
  /** Accepts this version, or asks first when any check did not pass. */
  accept: (taskId: string) => void;
  codeLeft: Accessor<"files" | "search">; setCodeLeft: (left: "files" | "search") => void;
  codeMode: Accessor<"file" | "inline" | "split">; setCodeMode: (mode: "file" | "inline" | "split") => void;
};
export const WsContext = createContext<Ws>();
export function useWs() {
  const ws = useContext(WsContext);
  if (!ws) throw new Error("Workspace context is missing");
  return ws;
}
