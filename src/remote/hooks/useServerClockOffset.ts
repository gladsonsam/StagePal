import { useEffect, useState } from "react";

// Estimates serverNow - clientNow (ms) from /api/time, keeping the lowest-RTT
// sample. Without it, phone beat-dots drift by device/host clock skew.
export function useServerClockOffset(): number {
  const [offset, setOffset] = useState(0);

  useEffect(() => {
    let cancelled = false;

    const sample = async (): Promise<{ offset: number; rtt: number } | null> => {
      const t0 = Date.now();
      try {
        const r = await fetch("/api/time", { cache: "no-store" });
        const t1 = Date.now();
        const serverNow = Number(await r.json());
        if (!Number.isFinite(serverNow)) return null;
        const rtt = t1 - t0;
        // Assume the response was stamped at the RTT midpoint.
        return { offset: serverNow - (t0 + rtt / 2), rtt };
      } catch {
        return null;
      }
    };

    const run = async () => {
      let best: { offset: number; rtt: number } | null = null;
      for (let i = 0; i < 5; i++) {
        const s = await sample();
        if (cancelled) return;
        if (s && (!best || s.rtt < best.rtt)) best = s;
      }
      if (!cancelled && best) setOffset(best.offset);
    };

    run();
    // Resample on focus — phones adjust clocks, drifting dots after suspend.
    const onFocus = () => {
      run();
    };
    window.addEventListener("focus", onFocus);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", onFocus);
    };
  }, []);

  return offset;
}
