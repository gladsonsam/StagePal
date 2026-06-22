// REST + WebSocket transport to the Rust server (src-tauri/src/server.rs).
// POSTs are fire-and-forget; the server broadcasts new state back over /ws.

import type { NowPlaying } from "../shared/types";

export interface PresetBrief {
  id: string;
  name: string;
}

export interface QuickCue {
  id: string;
  label: string;
  text: string;
}

export interface Info {
  keys: string[];
  presets: PresetBrief[];
  active_preset: string | null;
  mapped_keys: string[];
  /** Active preset's key → file basename, for pad labels. */
  files: Record<string, string>;
  cues_quick: QuickCue[];
  now: NowPlaying;
}

export function post(path: string, body?: unknown): Promise<void> {
  return fetch(path, {
    method: "POST",
    headers: body ? { "Content-Type": "application/json" } : {},
    body: body ? JSON.stringify(body) : undefined,
  })
    .then(() => undefined)
    .catch(() => undefined);
}

export async function fetchInfo(): Promise<Info> {
  const r = await fetch("/api/info");
  return (await r.json()) as Info;
}
