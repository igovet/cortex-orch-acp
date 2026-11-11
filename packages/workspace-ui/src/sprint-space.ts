/*
 * How the sprint planning screen shares the window. One rule replaces any floating panel: below the plan's minimum
 * width, panels give way in a fixed order, and the panel the owner opened last stays. Nothing here is stored.
 */
export type PlanView = "list" | "graph";
/** The panel the owner opened last, which keeps its place when there is no room for everything. `plan` is the plan, chosen
 * over the conversation on a pane too narrow for both. */
export type Opened = "none" | "conv" | "sb" | "details" | "plan";

export const DETAILS_W = 300, CONV_MIN = 340, CONV_MAX = 400, CONV_WIDE = 520;
/** The plan pane never gets narrower than this, by view. */
export const minMain = (view: PlanView) => view === "graph" ? 640 : 420;

export type SpaceInput = {
  /** The window's width: the screen has the sidebar and the main area, and no inspector. */
  width: number; view: PlanView; full: boolean;
  sidebar: boolean; sidebarW: number; conv: boolean; details: boolean; opened: Opened;
};
export type Space = {
  sidebar: boolean; conv: boolean; details: boolean;
  /** Details takes the plan's place, with a way back, when even alone beside it the plan has no room. */
  replaced: boolean; convW: number; main: number;
  /** A pane too narrow for the conversation beside even the list, such as a phone: one of them at a time, the conversation
   * first, and each leads to the other. */
  swap: boolean;
};
/** Below this width the conversation and the plan take turns instead of standing side by side. */
export const SWAP_BELOW = 420 + CONV_MIN;

/**
 * Who stays and how wide. Without an owner's last choice the conversation gives way first, then the sidebar. The
 * panel opened last wins: showing the conversation or the sidebar closes Details first (the selection stays), and
 * opening Details lets the others give way. Full view in the Graph view keeps only the plan and Details.
 */
export function arrange(i: SpaceInput): Space {
  let sb = i.sidebar, conv = i.conv, det = i.details;
  const minW = minMain(i.view), full = i.full && i.view === "graph";
  if (!full && i.width < SWAP_BELOW) {
    // The conversation is the default reading path; the plan, or a task's details, replaces it when the owner opens them.
    const plan = i.opened === "plan" || i.opened === "details" || !i.conv;
    return { sidebar: false, conv: !plan, details: plan && det, replaced: plan && det, convW: i.width, main: plan ? i.width : 0, swap: true };
  }
  if (full) { sb = false; conv = false; }
  const avail = () => i.width - (sb ? i.sidebarW : 0) - (det ? DETAILS_W : 0);
  const convW = () => i.width >= 1800 ? CONV_WIDE : avail() - CONV_MAX >= minW ? CONV_MAX : CONV_MIN;
  const main = () => avail() - (conv ? convW() : 0);
  if (!full) for (let guard = 0; main() < minW && guard < 6; guard++) {
    if (i.opened === "conv") { if (det) det = false; else if (sb) sb = false; else if (conv) conv = false; else break; }
    else if (i.opened === "sb") { if (det) det = false; else if (conv) conv = false; else if (sb) sb = false; else break; }
    else if (conv) conv = false; else if (sb) sb = false; else break;
  }
  const m = main();
  return { sidebar: sb, conv, details: det, replaced: det && m < minW, convW: convW(), main: m, swap: false };
}

/** Whether the conversation can be shown: beside the plan, Full view or not, or in its place on a pane too narrow for both. */
export const convRoom = (width: number, view: PlanView) => width < SWAP_BELOW || width - CONV_MIN >= minMain(view);
export const noRoomWhy = (view: PlanView) => `Widen the window to ${minMain(view) + CONV_MIN} px to show the conversation`;

/** Inline graph in the List view: shown while its 12 px labels keep at least 11 px. An unmeasured pane counts as wide enough. */
export const INLINE_FLOOR = 0.92, INLINE_MAX_H = 320;
export function inlineScale(size: { width: number; height: number }, paneWidth: number) {
  const across = paneWidth > 0 ? paneWidth / size.width : 1;
  return Math.min(1, across, INLINE_MAX_H / size.height);
}

/**
 * The graph toolbar gives up items by tier as it narrows: under 760 px Layers and the minimap go to the overflow menu,
 * under 640 px Status too. A bar that still overflows takes the next tier.
 */
export const toolbarTier = (width: number): 0 | 2 | 3 => width < 640 ? 3 : width < 760 ? 2 : 0;
/** The minimap follows the canvas width until the owner first chooses. */
export const MAP_MIN_CANVAS = 760;
export const mapWanted = (pref: boolean | null, canvasWidth: number) => pref ?? canvasWidth >= MAP_MIN_CANVAS;
