//! Core shared types: keys, presets, settings, and live NowPlaying state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Unix-epoch millis, so clients predict the click beat without a per-beat broadcast.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The 12 chromatic roots.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    #[serde(rename = "C")]
    C,
    #[serde(rename = "C#")]
    Cs,
    #[serde(rename = "D")]
    D,
    #[serde(rename = "D#")]
    Ds,
    #[serde(rename = "E")]
    E,
    #[serde(rename = "F")]
    F,
    #[serde(rename = "F#")]
    Fs,
    #[serde(rename = "G")]
    G,
    #[serde(rename = "G#")]
    Gs,
    #[serde(rename = "A")]
    A,
    #[serde(rename = "A#")]
    As,
    #[serde(rename = "B")]
    B,
}

impl Key {
    pub const ALL: [Key; 12] = [
        Key::C,
        Key::Cs,
        Key::D,
        Key::Ds,
        Key::E,
        Key::F,
        Key::Fs,
        Key::G,
        Key::Gs,
        Key::A,
        Key::As,
        Key::B,
    ];

    /// Canonical display string, e.g. "C#".
    pub fn as_str(self) -> &'static str {
        match self {
            Key::C => "C",
            Key::Cs => "C#",
            Key::D => "D",
            Key::Ds => "D#",
            Key::E => "E",
            Key::F => "F",
            Key::Fs => "F#",
            Key::G => "G",
            Key::Gs => "G#",
            Key::A => "A",
            Key::As => "A#",
            Key::B => "B",
        }
    }

    /// Fundamental frequency in the 3rd octave (C3 = 130.81 Hz).
    pub fn freq(self) -> f32 {
        match self {
            Key::C  => 130.81,
            Key::Cs => 138.59,
            Key::D  => 146.83,
            Key::Ds => 155.56,
            Key::E  => 164.81,
            Key::F  => 174.61,
            Key::Fs => 185.00,
            Key::G  => 196.00,
            Key::Gs => 207.65,
            Key::A  => 220.00,
            Key::As => 233.08,
            Key::B  => 246.94,
        }
    }

    /// TTS spelling — SAPI reads "#" as "hash", so spell sharps out.
    pub fn spoken(self) -> &'static str {
        match self {
            Key::C => "C",
            Key::Cs => "C sharp",
            Key::D => "D",
            Key::Ds => "D sharp",
            Key::E => "E",
            Key::F => "F",
            Key::Fs => "F sharp",
            Key::G => "G",
            Key::Gs => "G sharp",
            Key::A => "A",
            Key::As => "A sharp",
            Key::B => "B",
        }
    }

    /// Parse a key from an API/UI string (sharps and flats).
    pub fn parse(s: &str) -> Option<Key> {
        let norm = s.trim().to_lowercase();
        Key::ALL.into_iter().find(|k| k.aliases().contains(&norm.as_str()))
    }

    /// Lowercase spellings to recognise this key in a filename stem.
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Key::C => &["c"],
            Key::Cs => &["c#", "cs", "csharp", "db", "dflat"],
            Key::D => &["d"],
            Key::Ds => &["d#", "ds", "dsharp", "eb", "eflat"],
            Key::E => &["e"],
            Key::F => &["f"],
            Key::Fs => &["f#", "fs", "fsharp", "gb", "gflat"],
            Key::G => &["g"],
            Key::Gs => &["g#", "gs", "gsharp", "ab", "aflat"],
            Key::A => &["a"],
            Key::As => &["a#", "as", "asharp", "bb", "bflat"],
            Key::B => &["b"],
        }
    }
}

/// Reserved id of the built-in "Generated Pads" bank. It maps no files, so
/// every key falls through to the on-the-fly synth. Always present; can't be
/// removed.
pub const BUILTIN_SYNTH_ID: &str = "__builtin_synth__";

/// A set of pad files (one per key) in one folder.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub folder: PathBuf,
    /// Key → audio file path. May be missing some keys.
    pub files: HashMap<Key, PathBuf>,
    /// Files whose key couldn't be auto-determined (or lost a conflict), for
    /// manual assignment. `serde(default)` keeps pre-field settings loadable.
    #[serde(default)]
    pub unmapped: Vec<PathBuf>,
}

impl Preset {
    /// The built-in synth bank: no folder, no files — pure generated pads.
    pub fn builtin_synth() -> Preset {
        Preset {
            id: BUILTIN_SYNTH_ID.to_string(),
            name: "Generated Pads".to_string(),
            folder: PathBuf::new(),
            files: HashMap::new(),
            unmapped: Vec::new(),
        }
    }

    pub fn is_builtin_synth(&self) -> bool {
        self.id == BUILTIN_SYNTH_ID
    }
}

/// Saved pad/click/cue routing for one device, so reselecting it restores the
/// channels last picked instead of snapping back to 1/2.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct DeviceRoute {
    pub pad_left: usize,
    pub pad_right: usize,
    pub click_left: usize,
    pub click_right: usize,
    pub cue_left: usize,
    pub cue_right: usize,
}

/// Persisted application settings.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    /// cpal host label ("WASAPI" or "ASIO"). Defaults to WASAPI for pre-ASIO settings.
    #[serde(default = "default_host")]
    pub output_host: String,
    pub output_device: Option<String>,
    pub channel_left: usize,
    pub channel_right: usize,
    pub crossfade_ms: u32,
    pub master_volume: f32,
    pub presets: Vec<Preset>,
    pub active_preset: Option<String>,
    pub server_port: u16,
    /// Click-track config. `serde(default)` keeps pre-click settings loadable.
    #[serde(default)]
    pub click: ClickSettings,
    /// TTS cue config. `serde(default)` keeps pre-cues settings loadable.
    #[serde(default)]
    pub cues: CueSettings,
    /// Per-device routing memory, keyed by `device_key(host, device)`.
    #[serde(default)]
    pub device_routes: HashMap<String, DeviceRoute>,
}

/// Persisted click-track config. The live `enabled` flag is deliberately NOT
/// persisted — the app boots stopped so nobody is surprised by a live click.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClickSettings {
    pub bpm: f32,
    pub beats_per_bar: u32,
    pub accent: bool,
    pub volume: f32,
    pub channel_left: usize,
    pub channel_right: usize,
}

impl Default for ClickSettings {
    fn default() -> Self {
        ClickSettings {
            bpm: 90.0,
            beats_per_bar: 4,
            accent: true,
            volume: 0.8,
            channel_left: 2,
            channel_right: 3,
        }
    }
}

/// A labeled bit of text spoken with a tap. `id` is opaque and independent of
/// label so renames don't break phone-remote references.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct QuickCue {
    pub id: String,
    pub label: String,
    pub text: String,
}

/// Persisted TTS cue config. `voice` None = system default SAPI voice.
/// `rate` is the SAPI scale, -10..10.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CueSettings {
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub rate: i32,
    pub volume: f32,
    pub channel_left: usize,
    pub channel_right: usize,
    /// Drop the click bus ~12 dB while a cue speaks. Off by default.
    #[serde(default)]
    pub duck_click: bool,
    /// Auto-announce the new key on pad change (e.g. "Key of G"). Off by default.
    #[serde(default)]
    pub speak_key_on_change: bool,
    #[serde(default)]
    pub quick: Vec<QuickCue>,
}

impl Default for CueSettings {
    fn default() -> Self {
        CueSettings {
            voice: None,
            // SAPI default rate sounds rushed for short phrases; -1 is relaxed.
            rate: -1,
            volume: 0.95,
            // Cue bus defaults to channels 5/6 — usually free of the pad (1/2)
            // and click (3/4) pairs. Falls back to silent on fewer channels.
            channel_left: 4,
            channel_right: 5,
            duck_click: false,
            speak_key_on_change: false,
            quick: Vec::new(),
        }
    }
}

fn default_host() -> String {
    "WASAPI".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            output_host: default_host(),
            output_device: None,
            channel_left: 0,
            channel_right: 1,
            crossfade_ms: 2000,
            master_volume: 0.8,
            presets: Vec::new(),
            active_preset: None,
            server_port: 7777,
            click: ClickSettings::default(),
            cues: CueSettings::default(),
            device_routes: HashMap::new(),
        }
    }
}

impl Settings {
    pub fn active_preset(&self) -> Option<&Preset> {
        let id = self.active_preset.as_deref()?;
        self.presets.iter().find(|p| p.id == id)
    }

    /// Guarantee the built-in "Generated Pads" bank exists at the top of the
    /// list. Called on load so it's always a selectable option alongside any
    /// imported folders. Defaults the active bank to it on first run.
    pub fn ensure_builtin_synth(&mut self) {
        if !self.presets.iter().any(|p| p.is_builtin_synth()) {
            self.presets.insert(0, Preset::builtin_synth());
        }
        if self.active_preset.is_none() {
            self.active_preset = Some(BUILTIN_SYNTH_ID.to_string());
        }
    }

    /// Stable map key for per-device routing memory.
    pub fn device_key(host: &str, device: &str) -> String {
        format!("{host}::{device}")
    }

    /// Current pad/click/cue channels as a `DeviceRoute`.
    pub fn current_route(&self) -> DeviceRoute {
        DeviceRoute {
            pad_left: self.channel_left,
            pad_right: self.channel_right,
            click_left: self.click.channel_left,
            click_right: self.click.channel_right,
            cue_left: self.cues.channel_left,
            cue_right: self.cues.channel_right,
        }
    }

    /// Snapshot current routing into `device_routes` for the active device.
    /// No-op when no device is selected.
    pub fn remember_current_route(&mut self) {
        if let Some(device) = self.output_device.clone() {
            let key = Self::device_key(&self.output_host, &device);
            let route = self.current_route();
            self.device_routes.insert(key, route);
        }
    }
}

/// Live playback state, broadcast to every client.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NowPlaying {
    pub key: Option<Key>,
    pub preset: Option<String>,
    pub volume: f32,
    pub playing: bool,
    #[serde(default)]
    pub click: ClickNow,
    #[serde(default)]
    pub cue: CueNow,
}

impl Default for NowPlaying {
    fn default() -> Self {
        NowPlaying {
            key: None,
            preset: None,
            volume: 0.8,
            playing: false,
            click: ClickNow::default(),
            cue: CueNow::default(),
        }
    }
}

/// Live click-track state. `started_at_ms` lets clients predict the beat (no
/// per-beat broadcast); re-set on (re)start or signature change. `volume`/
/// `accent` mirror `ClickSettings` so clients see edits live over the WebSocket.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClickNow {
    pub enabled: bool,
    pub bpm: f32,
    pub beats_per_bar: u32,
    pub volume: f32,
    pub accent: bool,
    pub started_at_ms: Option<u64>,
}

impl Default for ClickNow {
    fn default() -> Self {
        ClickNow {
            enabled: false,
            bpm: 90.0,
            beats_per_bar: 4,
            volume: 0.8,
            accent: true,
            started_at_ms: None,
        }
    }
}

/// Live TTS cue state. `label` carries the quick cue's label so phones can
/// highlight the speaking button; None for free-form text speaks.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CueNow {
    pub speaking: bool,
    pub label: Option<String>,
}
