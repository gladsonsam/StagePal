// Typed wrappers over the Tauri backend commands + the now-playing event.
// One place for invoke() names and argument shapes.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Key, NowPlaying } from "../shared/types";

// In a dev build running outside Tauri (a plain browser), fall back to an
// in-memory mock so the UI is fully explorable. Production builds always run
// inside Tauri, where `import.meta.env.DEV` is false and this whole branch (and
// the dynamically-imported mock) is dropped from the bundle.
const inTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const useMock = import.meta.env.DEV && !inTauri;

function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (useMock) {
    return import("./mock").then((m) => m.mockInvoke<T>(cmd, args));
  }
  return tauriInvoke<T>(cmd, args);
}

export { ALL_KEYS } from "../shared/types";
export type { Key, ClickNow, CueNow, NowPlaying } from "../shared/types";

export interface DeviceInfo {
  /** cpal host label, e.g. "WASAPI" or "ASIO". */
  host: string;
  name: string;
  channels: number;
  default_sample_rate: number;
  is_default: boolean;
}

export interface AudioDebugReport {
  host: string;
  device: string;
  sample_format: string;
  sample_rate: number;
  channels: number;
  pad_channels: [number, number];
  callback_calls: number;
  frames_written: number;
  nonzero_frames: number;
  peak: number;
}

export interface Preset {
  id: string;
  name: string;
  folder: string;
  /** Key (e.g. "C#") → file path. */
  files: Partial<Record<Key, string>>;
  /** Audio files whose key couldn't be auto-detected — awaiting manual mapping. */
  unmapped: string[];
}

export interface ClickSettings {
  bpm: number;
  beats_per_bar: number;
  accent: boolean;
  volume: number;
  channel_left: number;
  channel_right: number;
}

export interface QuickCue {
  id: string;
  label: string;
  text: string;
}

export interface CueSettings {
  voice: string | null;
  rate: number;
  volume: number;
  channel_left: number;
  channel_right: number;
  duck_click: boolean;
  /** Speak "Key of X" automatically whenever a pad changes. */
  speak_key_on_change: boolean;
  quick: QuickCue[];
}

export interface Settings {
  output_host: string;
  output_device: string | null;
  channel_left: number;
  channel_right: number;
  crossfade_ms: number;
  master_volume: number;
  presets: Preset[];
  active_preset: string | null;
  server_port: number;
  click: ClickSettings;
  cues: CueSettings;
}

export interface ServerUrl {
  ip: string | null;
  host: string;
  port: number;
}

export const getSettings = () => invoke<Settings>("get_settings");
export const getState = () => invoke<NowPlaying>("get_state");
export const listAudioDevices = () =>
  invoke<DeviceInfo[]>("list_audio_devices");

export const setAudioOutput = (
  host: string,
  device: string,
  channelLeft: number,
  channelRight: number,
) =>
  invoke<void>("set_audio_output", { host, device, channelLeft, channelRight });

export const runAudioOutputTest = () =>
  invoke<AudioDebugReport>("run_audio_output_test");

export const setVolume = (volume: number) =>
  invoke<void>("set_volume", { volume });

export const scanLibrary = (folder: string, name?: string) =>
  invoke<Preset>("scan_library", { folder, name: name ?? null });

export const removePreset = (id: string) =>
  invoke<void>("remove_preset", { id });

export const setPreset = (id: string) => invoke<void>("set_preset", { id });

export const renamePreset = (id: string, name: string) =>
  invoke<void>("rename_preset", { id, name });

/** Map (or move) an audio file onto a key within a preset. */
export const assignKey = (id: string, key: Key, path: string) =>
  invoke<void>("assign_key", { id, key, path });

/** Unmap a key, returning its file to the unmapped pile. */
export const clearKey = (id: string, key: Key) =>
  invoke<void>("clear_key", { id, key });

/** Crossfade / fade-out duration in milliseconds. */
export const setCrossfade = (ms: number) =>
  invoke<void>("set_crossfade", { ms });

export const playKey = (key: Key) => invoke<void>("play_key", { key });

export const stopPads = () => invoke<void>("stop");

export const getServerUrl = () => invoke<ServerUrl>("server_url");

/** Start or stop the click. Independent of pad transport. */
export const setClickEnabled = (enabled: boolean) =>
  invoke<void>("set_click_enabled", { enabled });

export const setClickBpm = (bpm: number) =>
  invoke<void>("set_click_bpm", { bpm });

export const setClickBeats = (beats: number) =>
  invoke<void>("set_click_beats", { beats });

export const setClickAccent = (accent: boolean) =>
  invoke<void>("set_click_accent", { accent });

export const setClickVolume = (volume: number) =>
  invoke<void>("set_click_volume", { volume });

export const setClickChannels = (channelLeft: number, channelRight: number) =>
  invoke<void>("set_click_channels", { channelLeft, channelRight });

/* ── Cues ─────────────────────────────────────────────────────────── */

export interface VoiceInfo {
  id: string;
  name: string;
}

export const listVoices = () => invoke<VoiceInfo[]>("list_voices");

/** Speak free-form text. Renders TTS to a temp WAV then plays it on the cue bus. */
export const cueSpeak = (text: string) => invoke<void>("cue_speak", { text });

/** Speak a saved quick cue by id. */
export const cueSpeakQuick = (id: string) =>
  invoke<void>("cue_speak_quick", { id });

/** Cut any in-flight cue immediately. */
export const cueStop = () => invoke<void>("cue_stop");

export const cueAdd = (label: string, text: string) =>
  invoke<QuickCue>("cue_add", { label, text });

export const cueUpdate = (id: string, label: string, text: string) =>
  invoke<void>("cue_update", { id, label, text });

export const cueRemove = (id: string) => invoke<void>("cue_remove", { id });

export const cueMove = (id: string, toIndex: number) =>
  invoke<void>("cue_move", { id, toIndex });

export const setCueVoice = (voice: string | null) =>
  invoke<void>("set_cue_voice", { voice });

export const setCueRate = (rate: number) =>
  invoke<void>("set_cue_rate", { rate });

export const setCueVolume = (volume: number) =>
  invoke<void>("set_cue_volume", { volume });

export const setCueChannels = (channelLeft: number, channelRight: number) =>
  invoke<void>("set_cue_channels", { channelLeft, channelRight });

export const setCueDuckClick = (duck: boolean) =>
  invoke<void>("set_cue_duck_click", { duck });

export const setCueSpeakKey = (enabled: boolean) =>
  invoke<void>("set_cue_speak_key", { enabled });

/** Subscribe to live now-playing updates pushed from the backend. */
export const onNowPlaying = (
  cb: (n: NowPlaying) => void,
): Promise<UnlistenFn> => {
  if (useMock) {
    return import("./mock").then((m) => m.mockListen(cb));
  }
  return listen<NowPlaying>("now-playing", (e) => cb(e.payload));
};
