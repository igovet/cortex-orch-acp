import { createSignal, type Accessor } from "solid-js";
import type { WindowChrome } from "../../../packages/workspace-ui/src";

type NativeWindow = {
  minimize(): Promise<void>; toggleMaximize(): Promise<void>; close(): Promise<void>; startDragging(): Promise<void>;
  isMaximized(): Promise<boolean>; onResized(handler: () => void): Promise<() => void>;
  onCloseRequested(handler: (event: { preventDefault(): void }) => void): Promise<() => void>;
};

/** The undecorated window's own controls. A refused call changes nothing on screen, so it has nothing to report. */
export function createWindowChrome(native: NativeWindow, finishClose: () => Promise<void>): WindowChrome & { closeError: Accessor<string | null> } {
  const [maximized, setMaximized] = createSignal(false);
  const [closeError, setCloseError] = createSignal<string | null>(null);
  const refresh = () => { native.isMaximized().then(setMaximized, () => {}); };
  refresh();
  native.onResized(refresh).catch(() => {});
  const quiet = (call: () => Promise<void>) => () => { call().catch(() => {}); };
  // The title band's button and the window manager's close arrive here alike, so nothing bypasses the guard.
  let guard: (() => boolean) | undefined;
  let closing = false;
  const finish = () => {
    if (closing) return;
    closing = true;
    setCloseError(null);
    finishClose().catch(() => setCloseError("The window could not close safely. Check pending saves, then try again.")).finally(() => { closing = false; });
  };
  native.onCloseRequested(event => {
    event.preventDefault();
    if (!guard?.()) finish();
  }).catch(() => {});
  return {
    maximized, closeError,
    minimize: quiet(() => native.minimize()), toggleMaximize: quiet(() => native.toggleMaximize()),
    close: quiet(() => native.close()), startDragging: quiet(() => native.startDragging()),
    guardClose(next) { guard = next; }, closeNow: finish,
  };
}
