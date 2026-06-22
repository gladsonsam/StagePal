// Types shared between desktop (Tauri IPC) and remote (REST/WS).
// Keep dependency-free so the phone remote bundle never imports @tauri-apps/api.

export type Key =
  | "C" | "C#" | "D" | "D#" | "E" | "F"
  | "F#" | "G" | "G#" | "A" | "A#" | "B";

export const ALL_KEYS: Key[] = [
  "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

export interface ClickNow {
  enabled: boolean;
  bpm: number;
  beats_per_bar: number;
  /** Broadcast so the phone remote sees desktop edits. */
  volume: number;
  accent: boolean;
  /** unix-epoch ms when click last (re)started; null when stopped. */
  started_at_ms: number | null;
}

/** Live TTS cue state. `label` is the quick cue's label, or null for free-form. */
export interface CueNow {
  speaking: boolean;
  label: string | null;
}

export interface NowPlaying {
  key: Key | null;
  preset: string | null;
  volume: number;
  playing: boolean;
  click: ClickNow;
  cue: CueNow;
}
