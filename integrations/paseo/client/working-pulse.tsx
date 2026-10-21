import { useEffect, useRef, useState, type ReactNode } from "react";
import { AccessibilityInfo, Animated, Platform, View } from "react-native";
import { setWorkerCardPulse, WORKER_PULSE_MIN_OPACITY, WORKER_PULSE_PERIOD_MS } from "./worker-card-pulse.ts";

type Props = { active: boolean; children: ReactNode };

/**
 * Wraps a worker's small indicator (marker or status icon) and makes only it
 * breathe while `active`. Decorative: status stays in the icon and text, so
 * nothing depends on the motion, and the wrapper adds no accessible name.
 */
export function WorkingPulse({ active, children }: Props) {
  return Platform.OS === "web" ? <WebPulse active={active}>{children}</WebPulse> : <NativePulse active={active}>{children}</NativePulse>;
}

function WebPulse({ active, children }: Props) {
  const ref = useRef<unknown>(null);
  useEffect(() => {
    const element = ref.current as HTMLElement | null;
    if (!element || typeof element.setAttribute !== "function") return;
    setWorkerCardPulse(element, active);
    // Settling, unmounting or a selection that remounts the indicator
    // removes the attribute, which stops the CSS animation.
    return () => setWorkerCardPulse(element, false);
  }, [active]);
  return <View ref={ref as never} style={{ alignItems: "center", justifyContent: "center" }}>{children}</View>;
}

type ReduceMotionInfo = {
  isReduceMotionEnabled?: () => Promise<boolean>;
  addEventListener?: (event: "reduceMotionChanged", handler: (enabled: boolean) => void) => { remove?: () => void } | undefined;
};

/** The platform's reduce-motion setting, followed while mounted. */
function useReducedMotion(): boolean {
  const [reduce, setReduce] = useState(false);
  useEffect(() => {
    const info = AccessibilityInfo as unknown as ReduceMotionInfo | undefined;
    if (!info || typeof info.isReduceMotionEnabled !== "function") return;
    let live = true;
    info.isReduceMotionEnabled().then((enabled) => { if (live) setReduce(Boolean(enabled)); }, () => {});
    const subscription = typeof info.addEventListener === "function"
      ? info.addEventListener("reduceMotionChanged", (enabled) => { if (live) setReduce(Boolean(enabled)); })
      : undefined;
    return () => { live = false; subscription?.remove?.(); };
  }, []);
  return reduce;
}

function NativePulse({ active, children }: Props) {
  const reduceMotion = useReducedMotion();
  const opacity = useRef(new Animated.Value(1)).current;
  const animate = active && !reduceMotion;
  useEffect(() => {
    if (!animate) { opacity.setValue(1); return; }
    // Native-driver opacity: no JavaScript work or React state per frame.
    const loop = Animated.loop(Animated.sequence([
      Animated.timing(opacity, { toValue: WORKER_PULSE_MIN_OPACITY, duration: WORKER_PULSE_PERIOD_MS / 2, useNativeDriver: true }),
      Animated.timing(opacity, { toValue: 1, duration: WORKER_PULSE_PERIOD_MS / 2, useNativeDriver: true }),
    ]));
    loop.start();
    return () => { loop.stop(); opacity.setValue(1); };
  }, [animate, opacity]);
  return <Animated.View style={{ opacity, alignItems: "center", justifyContent: "center" }}>{children}</Animated.View>;
}
