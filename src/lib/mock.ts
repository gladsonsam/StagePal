// Dev-only stand-in for the Tauri backend so the UI can run (and be designed) in
// a plain browser via `npm run dev`. This module is dynamically imported only
// when running outside Tauri in a dev build, so it's excluded from production
// bundles. It keeps just enough in-memory state to exercise every screen.

import type { DeviceInfo, NowPlaying, Preset, ServerUrl, Settings } from "./ipc";

const FOLDER = "C:/Pads/Aurora Pads";

const settings: Settings = {
  output_host: "WASAPI",
  output_device: "Speakers (Realtek High Definition Audio)",
  channel_left: 0,
  channel_right: 1,
  crossfade_ms: 2000,
  master_volume: 0.8,
  active_preset: FOLDER,
  server_port: 7777,
  click: {
    bpm: 90,
    beats_per_bar: 4,
    accent: true,
    volume: 0.8,
    channel_left: 2,
    channel_right: 3,
  },
  cues: {
    voice: null,
    rate: 0,
    volume: 0.95,
    channel_left: 4,
    channel_right: 5,
    duck_click: false,
    speak_key_on_change: false,
    quick: [
      { id: "q-verse-2", label: "Verse 2", text: "Verse two." },
      { id: "q-bridge", label: "Bridge", text: "Bridge coming up." },
      { id: "q-hold", label: "Hold", text: "Hold on this chord." },
      { id: "q-tag", label: "Tag", text: "One more time." },
    ],
  },
  presets: [
    {
      id: FOLDER,
      name: "Aurora Pads",
      folder: FOLDER,
      files: {
        C: `${FOLDER}/C.wav`,
        D: `${FOLDER}/D Pad.wav`,
        E: `${FOLDER}/E.wav`,
        G: `${FOLDER}/G.wav`,
        A: `${FOLDER}/A.wav`,
      },
      unmapped: [`${FOLDER}/Bright Swell.wav`, `${FOLDER}/Low Drone 02.wav`],
    },
    {
      id: "C:/Pads/Ambient Set",
      name: "Ambient Set",
      folder: "C:/Pads/Ambient Set",
      files: Object.fromEntries(
        ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"].map((k) => [
          k,
          `C:/Pads/Ambient Set/${k}.mp3`,
        ]),
      ) as Preset["files"],
      unmapped: [],
    },
  ],
};

const now: NowPlaying = {
  key: null,
  preset: settings.active_preset,
  volume: settings.master_volume,
  playing: false,
  click: {
    enabled: false,
    bpm: settings.click.bpm,
    beats_per_bar: settings.click.beats_per_bar,
    volume: settings.click.volume,
    accent: settings.click.accent,
    started_at_ms: null,
  },
  cue: {
    speaking: false,
    label: null,
  },
};

const devices: DeviceInfo[] = [
  {
    host: "WASAPI",
    name: "Speakers (Realtek High Definition Audio)",
    channels: 2,
    default_sample_rate: 48000,
    is_default: true,
  },
  {
    host: "WASAPI",
    name: "Headphones (USB Audio)",
    channels: 2,
    default_sample_rate: 48000,
    is_default: false,
  },
  {
    host: "ASIO",
    name: "Focusrite USB ASIO",
    channels: 18,
    default_sample_rate: 48000,
    is_default: false,
  },
];

const listeners = new Set<(n: NowPlaying) => void>();
const emit = () => listeners.forEach((cb) => cb({ ...now }));

export function mockListen(cb: (n: NowPlaying) => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export async function mockInvoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  const a = args as Record<string, any>;
  const preset = () => settings.presets.find((p) => p.id === a.id);
  switch (cmd) {
    case "get_settings":
      return structuredClone(settings) as T;
    case "get_state":
      return { ...now } as T;
    case "list_audio_devices":
      return devices as T;
    case "server_url":
      return { ip: "192.168.1.42", host: "studio-pc", port: 7777 } as ServerUrl as T;
    case "set_volume":
      now.volume = a.volume;
      settings.master_volume = a.volume;
      emit();
      return undefined as T;
    case "play_key": {
      if (now.playing && now.key === a.key) {
        now.playing = false;
        now.key = null;
        emit();
      } else {
        now.key = a.key;
        now.playing = true;
        emit();
        if (settings.cues.speak_key_on_change) {
          const spoken = String(a.key).replace("#", " sharp");
          const label = `Key of ${spoken}`;
          now.cue.speaking = true;
          now.cue.label = label;
          emit();
          setTimeout(() => {
            now.cue.speaking = false;
            now.cue.label = null;
            emit();
          }, 1200);
        }
      }
      return undefined as T;
    }
    case "stop":
      now.playing = false;
      now.key = null;
      emit();
      return undefined as T;
    case "set_preset":
      settings.active_preset = a.id;
      now.preset = a.id;
      emit();
      return undefined as T;
    case "set_audio_output":
      settings.output_host = a.host;
      settings.output_device = a.device;
      settings.channel_left = a.channelLeft;
      settings.channel_right = a.channelRight;
      return undefined as T;
    case "set_crossfade":
      settings.crossfade_ms = a.ms;
      return undefined as T;
    case "rename_preset": {
      const p = preset();
      if (p) p.name = a.name;
      return undefined as T;
    }
    case "remove_preset":
      settings.presets = settings.presets.filter((p) => p.id !== a.id);
      if (settings.active_preset === a.id)
        settings.active_preset = settings.presets[0]?.id ?? null;
      return undefined as T;
    case "assign_key": {
      const p = preset();
      if (p) {
        p.unmapped = p.unmapped.filter((x) => x !== a.path);
        for (const k of Object.keys(p.files)) if (p.files[k as keyof typeof p.files] === a.path) delete p.files[k as keyof typeof p.files];
        const prev = p.files[a.key as keyof typeof p.files];
        if (prev && !p.unmapped.includes(prev)) p.unmapped.push(prev);
        p.files[a.key as keyof typeof p.files] = a.path;
        p.unmapped.sort();
      }
      return undefined as T;
    }
    case "clear_key": {
      const p = preset();
      if (p) {
        const prev = p.files[a.key as keyof typeof p.files];
        if (prev) {
          delete p.files[a.key as keyof typeof p.files];
          if (!p.unmapped.includes(prev)) p.unmapped.push(prev);
          p.unmapped.sort();
        }
      }
      return undefined as T;
    }
    case "scan_library":
      return (preset() ?? settings.presets[0]) as T;
    case "set_click_enabled":
      now.click.enabled = !!a.enabled;
      now.click.started_at_ms = now.click.enabled ? Date.now() : null;
      emit();
      return undefined as T;
    case "set_click_bpm":
      settings.click.bpm = a.bpm;
      now.click.bpm = a.bpm;
      emit();
      return undefined as T;
    case "set_click_beats":
      settings.click.beats_per_bar = a.beats;
      now.click.beats_per_bar = a.beats;
      if (now.click.enabled) now.click.started_at_ms = Date.now();
      emit();
      return undefined as T;
    case "set_click_accent":
      settings.click.accent = !!a.accent;
      now.click.accent = !!a.accent;
      emit();
      return undefined as T;
    case "set_click_volume":
      settings.click.volume = a.volume;
      now.click.volume = a.volume;
      emit();
      return undefined as T;
    case "set_click_channels":
      settings.click.channel_left = a.channelLeft;
      settings.click.channel_right = a.channelRight;
      return undefined as T;
    case "list_voices":
      return [
        { id: "Microsoft David Desktop", name: "Microsoft David Desktop" },
        { id: "Microsoft Zira Desktop", name: "Microsoft Zira Desktop" },
      ] as T;
    case "cue_speak":
    case "cue_speak_quick": {
      const label =
        cmd === "cue_speak_quick"
          ? settings.cues.quick.find((q) => q.id === a.id)?.label ?? null
          : null;
      now.cue.speaking = true;
      now.cue.label = label;
      emit();
      // Fake a ~1.2s spoken length so the UI flips back automatically.
      setTimeout(() => {
        now.cue.speaking = false;
        now.cue.label = null;
        emit();
      }, 1200);
      return undefined as T;
    }
    case "cue_stop":
      now.cue.speaking = false;
      now.cue.label = null;
      emit();
      return undefined as T;
    case "cue_add": {
      const id = `q-${Date.now().toString(16)}`;
      const cue = { id, label: a.label, text: a.text };
      settings.cues.quick.push(cue);
      return cue as T;
    }
    case "cue_update": {
      const c = settings.cues.quick.find((q) => q.id === a.id);
      if (c) {
        c.label = a.label;
        c.text = a.text;
      }
      return undefined as T;
    }
    case "cue_remove":
      settings.cues.quick = settings.cues.quick.filter((q) => q.id !== a.id);
      return undefined as T;
    case "cue_move": {
      const from = settings.cues.quick.findIndex((q) => q.id === a.id);
      if (from < 0) return undefined as T;
      const [item] = settings.cues.quick.splice(from, 1);
      const to = Math.min(a.toIndex, settings.cues.quick.length);
      settings.cues.quick.splice(to, 0, item);
      return undefined as T;
    }
    case "set_cue_voice":
      settings.cues.voice = a.voice ?? null;
      return undefined as T;
    case "set_cue_rate":
      settings.cues.rate = a.rate;
      return undefined as T;
    case "set_cue_volume":
      settings.cues.volume = a.volume;
      return undefined as T;
    case "set_cue_channels":
      settings.cues.channel_left = a.channelLeft;
      settings.cues.channel_right = a.channelRight;
      return undefined as T;
    case "set_cue_duck_click":
      settings.cues.duck_click = !!a.duck;
      return undefined as T;
    case "set_cue_speak_key":
      settings.cues.speak_key_on_change = !!a.enabled;
      return undefined as T;
    default:
      return undefined as T;
  }
}
