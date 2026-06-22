// Press-and-hold stepper with auto-repeat + acceleration. Each instance owns
// its own start anchor; taps within coalesceWindow accumulate off the last-sent
// target so multi-tap works before the upstream value catches up.

import type { KeyboardEvent, MouseEvent, PointerEvent } from "react";
import { useEffect, useRef } from "react";

interface HoldRepeatCfg {
  initialDelay?: number;
  startInterval?: number;
  minInterval?: number;
  speedupAfter?: number;
  speedupFactor?: number;
  /** Taps within this many ms accumulate off the last target, not `value`. */
  coalesceWindow?: number;
}

interface HoldRepeatProps {
  /** Spread onto the button. */
  onPointerDown: (e: PointerEvent<HTMLElement>) => void;
  onPointerUp: () => void;
  onPointerLeave: () => void;
  onPointerCancel: () => void;
  /** Keyboard activation (Space/Enter) → single step. */
  onClick: (e: MouseEvent<HTMLElement>) => void;
  /** Optional: lets parent add keyboard handling (e.g. arrow keys). */
  onKeyDown?: (e: KeyboardEvent<HTMLElement>) => void;
}

/** `step`: +1 for plus, -1 for minus. `apply`: receives the next target. */
export function useHoldRepeat(
  value: number,
  step: number,
  apply: (target: number) => void,
  cfg: HoldRepeatCfg = {},
): HoldRepeatProps {
  const {
    initialDelay = 350,
    startInterval = 110,
    minInterval = 22,
    speedupAfter = 700,
    speedupFactor = 0.7,
    coalesceWindow = 1500,
  } = cfg;

  // Mirror latest prop so the timer callback reads the current value at tick time.
  const valueRef = useRef(value);
  valueRef.current = value;
  const applyRef = useRef(apply);
  applyRef.current = apply;

  const timerRef = useRef<number | null>(null);
  // Last applied target; anchor for a follow-up tap before `value` catches up.
  const lastTargetRef = useRef<number | null>(null);
  const lastTargetAtRef = useRef(0);

  const stop = () => {
    if (timerRef.current != null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  };

  useEffect(() => () => stop(), []);

  const nowMs = () =>
    typeof performance !== "undefined" ? performance.now() : Date.now();

  // Anchor: last target if a recent tap is pending, else current value.
  const anchor = (): number => {
    const t = lastTargetRef.current;
    if (t != null && nowMs() - lastTargetAtRef.current < coalesceWindow) {
      return t;
    }
    return valueRef.current;
  };

  const fire = (target: number) => {
    lastTargetRef.current = target;
    lastTargetAtRef.current = nowMs();
    applyRef.current(target);
  };

  const oneStep = () => {
    fire(anchor() + step);
  };

  const start = () => {
    stop();
    const base = anchor();
    let n = 1;
    fire(base + step * n);
    let interval = startInterval;
    let elapsed = 0;
    const tick = () => {
      n += 1;
      fire(base + step * n);
      elapsed += interval;
      if (elapsed >= speedupAfter && interval > minInterval) {
        interval = Math.max(minInterval, Math.round(interval * speedupFactor));
        elapsed = 0;
      }
      timerRef.current = window.setTimeout(tick, interval);
    };
    timerRef.current = window.setTimeout(tick, initialDelay);
  };

  return {
    onPointerDown: (e) => {
      // Reject non-primary mouse buttons; let touch/pen through (button is 0).
      if (e.pointerType === "mouse" && e.button !== 0) return;
      e.preventDefault();
      e.currentTarget.setPointerCapture?.(e.pointerId);
      start();
    },
    onPointerUp: stop,
    onPointerLeave: stop,
    onPointerCancel: stop,
    // detail === 0 means keyboard activation; pointer clicks (detail >= 1) are
    // already handled by pointerdown.
    onClick: (e) => {
      if (e.detail === 0) oneStep();
    },
  };
}
