import { useEffect, useRef, useState } from "react";
import type { NowPlaying } from "../../shared/types";
import { fetchInfo, type Info } from "../api";

const DEFAULT_NOW: NowPlaying = {
  key: null,
  preset: null,
  volume: 0.8,
  playing: false,
  click: {
    enabled: false,
    bpm: 90,
    beats_per_bar: 4,
    volume: 0.8,
    accent: true,
    started_at_ms: null,
  },
  cue: {
    speaking: false,
    label: null,
  },
};

export type ConnState = "connected" | "reconnecting";

interface RemoteState {
  info: Info | null;
  now: NowPlaying;
  conn: ConnState;
  refreshInfo: () => Promise<void>;
}

// Fetches /api/info, then mirrors NowPlaying WS broadcasts into state.
// Auto-reconnects on close/error and whenever the page becomes visible again.
export function useRemoteState(): RemoteState {
  const [info, setInfo] = useState<Info | null>(null);
  const [now, setNow] = useState<NowPlaying>(DEFAULT_NOW);
  const [conn, setConn] = useState<ConnState>("reconnecting");

  // Refs let the visibility handler reach into the effect's live state.
  const wsRef = useRef<WebSocket | null>(null);
  const timerRef = useRef<number | null>(null);
  const deadRef = useRef(false);
  const connectRef = useRef<(() => void) | null>(null);

  function refreshInfo(): Promise<void> {
    return fetchInfo()
      .then((i) => {
        setInfo(i);
      })
      .catch(() => {});
  }

  useEffect(() => {
    function connect() {
      if (deadRef.current) return;
      if (timerRef.current != null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      const proto = location.protocol === "https:" ? "wss" : "ws";
      const ws = new WebSocket(`${proto}://${location.host}/ws`);
      wsRef.current = ws;
      ws.onopen = () => {
        setConn("connected");
        // Refresh info so cue list and presets are up-to-date after reconnect.
        fetchInfo()
          .then((i) => setInfo(i))
          .catch(() => {});
      };
      ws.onclose = () => {
        if (deadRef.current) return;
        setConn("reconnecting");
        timerRef.current = window.setTimeout(connect, 1500);
      };
      // onerror always fires before onclose; closing here triggers the retry.
      ws.onerror = () => ws.close();
      ws.onmessage = (e) => {
        try {
          setNow(normalize(JSON.parse(e.data) as NowPlaying));
        } catch {
          /* ignore malformed frame */
        }
      };
    }

    connectRef.current = connect;
    fetchInfo()
      .then((i) => {
        setInfo(i);
        setNow(normalize(i.now));
      })
      .catch(() => {});
    connect();

    return () => {
      deadRef.current = true;
      if (timerRef.current != null) window.clearTimeout(timerRef.current);
      wsRef.current?.close();
    };
  }, []);

  // When the user switches back to this tab, reconnect and refresh data.
  useEffect(() => {
    function onVisibilityChange() {
      if (document.visibilityState !== "visible") return;
      fetchInfo()
        .then((i) => setInfo(i))
        .catch(() => {});
      const ws = wsRef.current;
      const gone =
        !ws ||
        ws.readyState === WebSocket.CLOSED ||
        ws.readyState === WebSocket.CLOSING;
      if (gone) {
        if (timerRef.current != null) {
          window.clearTimeout(timerRef.current);
          timerRef.current = null;
        }
        connectRef.current?.();
      }
    }
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () =>
      document.removeEventListener("visibilitychange", onVisibilityChange);
  }, []);

  return { info, now, conn, refreshInfo };
}

/** Backfill click/cue in case an older backend omits them. */
function normalize(n: NowPlaying): NowPlaying {
  let out = n;
  if (!out.click) out = { ...out, click: DEFAULT_NOW.click };
  if (!out.cue) out = { ...out, cue: DEFAULT_NOW.cue };
  return out;
}
