//! Core data types shared across the app: musical keys, pad presets, persisted
//! settings, and the live "now playing" state broadcast to all clients.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Wall-clock unix-epoch milliseconds. Used so connected clients can predict
/// the current click beat locally without a per-beat broadcast.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The 12 chromatic roots. Designed to extend later with major/minor quality.
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

    /// TTS-friendly spelling — SAPI says "#" as "hash", so spell sharps out.
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

    /// Parse a key from an API/UI string (accepts sharps and flats).
    pub fn parse(s: &str) -> Option<Key> {
        let norm = s.trim().to_lowercase();
        Key::ALL.into_iter().find(|k| k.aliases().contains(&norm.as_str()))
    }

    /// Lowercase spellings used to recognise this key in a filename stem.
    /// Includes sharp, flat, and "sharp"/"flat" word forms.
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

/// A set of pad files (one per key) living in one folder.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub folder: PathBuf,
    /// Key → audio file path. May be missing some keys.
    pub files: HashMap<Key, PathBuf>,
    /// Audio files found in the folder whose key could not be determined
    /// automatically (or that lost a same-key conflict). Surfaced to the UI so
    /// the user can assign them to a key by hand. `#[serde(default)]` keeps old
    /// settings files (written before this field existed) loadable.
    #[serde(default)]
    pub unmapped: Vec<PathBuf>,
}

/// Saved pad/click/cue channel routing for one output device. Remembered per
/// device so returning to a previously-used interface restores the exact
/// channels you last picked there instead of snapping back to 1/2.
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
    /// cpal host label ("WASAPI" or "ASIO"). Defaulted to WASAPI on Windows so
    /// older settings files (written before ASIO support) keep working.
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
    /// Click-track config. `#[serde(default)]` keeps settings files written
    /// before the click feature loadable.
    #[serde(default)]
    pub click: ClickSettings,
    /// TTS cue config. `#[serde(default)]` keeps pre-cues settings files loadable.
    #[serde(default)]
    pub cues: CueSettings,
    /// Per-device channel routing memory, keyed by `device_key(host, device)`.
    /// Lets the app restore the channels last used on each interface rather
    /// than resetting to 1/2 every time the device is reselected.
    #[serde(default)]
    pub device_routes: HashMap<String, DeviceRoute>,
}

/// Persisted click-track configuration. The live `enabled` flag is intentionally
/// NOT persisted — the app boots with the click stopped so a worship leader
/// isn't surprised by a live click on launch.
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

/// One saved "quick cue" — a labeled bit of text the band can speak with a tap.
/// `id` is a short opaque string (uuid-ish), independent of label so renames
/// don't break references from the phone remote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct QuickCue {
    pub id: String,
    pub label: String,
    pub text: String,
}

/// Persisted TTS cue config. `voice` of `None` means use the system default
/// SAPI voice. `rate` is the SAPI rate scale, -10..10.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CueSettings {
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub rate: i32,
    pub volume: f32,
    pub channel_left: usize,
    pub channel_right: usize,
    /// Drop the click bus ~12 dB while a cue is speaking. Off by default.
    #[serde(default)]
    pub duck_click: bool,
    /// Auto-announce the new key whenever a pad changes (e.g. "Key of G").
    /// Triggered from `play_key_logic` so desktop and phone-remote presses
    /// both fire it. Off by default.
    #[serde(default)]
    pub speak_key_on_change: bool,
    #[serde(default)]
    pub quick: Vec<QuickCue>,
}

impl Default for CueSettings {
    fn default() -> Self {
        CueSettings {
            voice: None,
            // SAPI default rate sounds rushed for short phrases like the auto
            // key-announcement. -1 reads as relaxed without sounding sluggish.
            rate: -1,
            volume: 0.95,
            // Default the cue bus to channels 5/6 — most multi-out interfaces
            // have at least 6 outs and these are commonly free of the pad pair
            // (1/2) and the click pair (3/4). Falls back to silent if the
            // device has fewer channels, like the click bus does.
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

    /// Stable map key for per-device routing memory.
    pub fn device_key(host: &str, device: &str) -> String {
        format!("{host}::{device}")
    }

    /// The current pad/click/cue channels as a `DeviceRoute`.
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

    /// Snapshot the current routing into `device_routes` under the active
    /// device, so reselecting it later restores these channels. No-op when no
    /// device is selected.
    pub fn remember_current_route(&mut self) {
        if let Some(device) = self.output_device.clone() {
            let key = Self::device_key(&self.output_host, &device);
            let route = self.current_route();
            self.device_routes.insert(key, route);
        }
    }
}

/// Live playback state, broadcast to every connected client (desktop + phones).
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

/// Live click-track state. `started_at_ms` lets clients predict the current
/// beat locally (no per-beat broadcast) — re-set whenever the click is
/// (re)started or its time signature changes.
///
/// `volume` and `accent` mirror their persisted counterparts in `ClickSettings`
/// so connected clients (e.g. the phone remote) see those edits live over the
/// WebSocket — broadcasting them is cheaper than asking the remote to refetch
/// `/api/info` after every toggle.
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

/// Live TTS cue state. `speaking` flips true the moment a cue starts and back
/// to false when it finishes (or is stopped). `label` carries the saved quick
/// cue's label so phones can highlight which button is currently speaking; it
/// is None for free-form text speaks.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CueNow {
    pub speaking: bool,
    pub label: Option<String>,
}
