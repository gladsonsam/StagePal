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

export async function cueAdd(label: string, text: string): Promise<QuickCue> {
  const r = await fetch("/api/cue/add", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label, text }),
  });
  if (!r.ok) throw new Error(await r.text());
  return r.json() as Promise<QuickCue>;
}

export async function cueUpdate(
  id: string,
  label: string,
  text: string,
): Promise<void> {
  const r = await fetch(`/api/cue/update/${encodeURIComponent(id)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label, text }),
  });
  if (!r.ok) throw new Error(await r.text());
}

export async function cueRemove(id: string): Promise<void> {
  await fetch(`/api/cue/remove/${encodeURIComponent(id)}`, { method: "POST" });
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
