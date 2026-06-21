//! Audio engine: owns the output device stream and mixes up to two looping
//! "voices" with an equal-gain crossfade, routing the stereo mix to two chosen
//! output channels of a (possibly multichannel) device.
//!
//! Threading model:
//!   - `AudioEngine` (Send + Sync) is the control handle stored in Tauri state.
//!     It sends `EngineCommand`s to a host thread.
//!   - The host thread owns the cpal `Stream` (which is `!Send`) and keeps it
//!     alive. On `SetOutput` it (re)builds the stream.
//!   - The real-time callback owns all playback state (voices, gains, master
//!     volume) and drains `PlayCommand`s from a lock-free queue each call. It
//!     never locks, allocates, or touches the filesystem.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{HostId, SampleFormat};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use rtrb::{Consumer, RingBuffer};
use serde::Serialize;

/// Upper bound on how long the caller waits for a stream rebuild. ASIO opens
/// can be slow but should never legitimately exceed this; a broken driver that
/// hangs longer would otherwise freeze the UI permanently. Note: when a prior
/// build_stream is mid-flight the host thread is single-threaded, so a queued
/// request may time out before the host even starts on it — the error message
/// is intentionally vague about whose fault it is.
const STREAM_BUILD_TIMEOUT: Duration = Duration::from_secs(30);

fn await_stream_reply(rx: &Receiver<Result<(), String>>, busy: bool) -> Result<(), String> {
    match rx.recv_timeout(STREAM_BUILD_TIMEOUT) {
        Ok(r) => r,
        Err(RecvTimeoutError::Timeout) => {
            if busy {
                // A previous device open is still running; the host thread
                // hasn't even reached this request yet. Don't blame the device
                // the user just picked — they may still get a successful open
                // once the prior call returns.
                Err("audio output is still opening a previous device — please wait or restart the app".into())
            } else {
                Err("audio device took too long to open — driver may be hung".into())
            }
        }
        Err(RecvTimeoutError::Disconnected) => Err("audio host thread did not reply".into()),
    }
}

use super::decode;

/// Info about an output device, surfaced to the UI for device/channel pickers.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    /// "WASAPI" or "ASIO" — the cpal host that owns this device.
    pub host: String,
    pub name: String,
    pub channels: usize,
    pub default_sample_rate: u32,
    /// True if this is the default output device on the default host.
    pub is_default: bool,
}

/// Describes the stream that is actually open right now — surfaced to the UI so
/// the Settings page can show "ASIO Focusrite · 18 ch · 48000 Hz · i32" and the
/// routing pickers reflect the device's real channel count instead of a guess.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveOutput {
    pub host: String,
    pub device: String,
    pub channels: usize,
    pub sample_rate: u32,
    /// "f32" or "i32" — which callback path is feeding the device.
    pub sample_format: String,
    /// Negotiated callback buffer size in frames, learned from the first
    /// callback (cpal doesn't report it up front). `None` until audio flows.
    pub buffer_frames: Option<u32>,
}

/// Which mixed-output bus a voice routes into. Pads and cues mix into separate
/// stereo pairs so a tech can park them on different IEM auxes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum VoiceBus {
    Pad,
    Cue,
}

/// One source (looping pad or one-shot cue) plus its current gain ramp.
struct Voice {
    consumer: Consumer<f32>,
    stop: Arc<AtomicBool>,
    /// Set true by the decoder thread the moment it has stopped producing.
    /// One-shots use this to drop themselves once their tail has drained.
    ended: Arc<AtomicBool>,
    bus: VoiceBus,
    gain: f32,
    target: f32,
    /// Per-frame gain increment magnitude.
    step: f32,
    /// Drop this voice once it has faded to silence.
    remove_when_silent: bool,
}

impl Drop for Voice {
    fn drop(&mut self) {
        // Tell the decoder thread to exit.
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Commands sent into the real-time callback via a lock-free queue.
enum PlayCommand {
    /// Fade in this new pad voice while fading out all existing pad voices.
    Crossfade(Voice),
    /// Drop in a new one-shot cue voice. Replaces any prior cue voice.
    PushCue(Voice),
    /// Fade all pad voices out (Stop button), at the given per-frame gain step.
    FadeOutAll(f32),
    /// Stop any in-flight cue.
    StopCue(f32),
    SetMaster(f32),
    SetCueVolume(f32),
    SetClickEnabled(bool),
    SetClickBpm(f32),
    SetClickBeats(u32),
    SetClickAccent(bool),
    SetClickVolume(f32),
    /// Toggle the cue→click ducking ramp (true while a cue is speaking AND
    /// the user has duck_click enabled).
    SetClickDuckActive(bool),
}

/// Initial click configuration handed to the audio callback when it's built.
/// All subsequent edits flow through `PlayCommand::SetClick*` so the RT thread
/// only ever updates itself; channel changes go through `SetOutput` instead
/// since they require rebuilding the cpal stream.
#[derive(Clone, Copy, Debug)]
pub struct ClickInit {
    pub enabled: bool,
    pub bpm: f32,
    pub beats_per_bar: u32,
    pub accent: bool,
    pub volume: f32,
}

/// High-level commands from the control handle to the host thread.
enum EngineCommand {
    SetOutput {
        host: String,
        device: String,
        pad_channels: (usize, usize),
        click_channels: (usize, usize),
        cue_channels: (usize, usize),
        reply: Sender<Result<(), String>>,
    },
    /// Change just the click channel pair; rebuilds the stream using the last
    /// known device + pad channels + cue channels + master volume + click state.
    SetClickChannels {
        channels: (usize, usize),
        reply: Sender<Result<(), String>>,
    },
    /// Change just the cue channel pair; rebuilds the stream similarly.
    SetCueChannels {
        channels: (usize, usize),
        reply: Sender<Result<(), String>>,
    },
    Play(PathBuf),
    Stop,
    PlayCue(PathBuf),
    StopCue,
    SetVolume(f32),
    SetCueVolume(f32),
    SetCrossfade(u32),
    SetClickEnabled(bool),
    SetClickBpm(f32),
    SetClickBeats(u32),
    SetClickAccent(bool),
    SetClickVolume(f32),
    /// Persisted "duck the click while a cue speaks" toggle.
    SetDuckClick(bool),
}

/// Internal messages sent from watcher threads back into the host thread.
/// Kept separate from `EngineCommand` so the public command channel can
/// disconnect cleanly when `AudioEngine` is dropped — if host_thread held a
/// clone of the public Sender, the channel would stay open forever.
enum HostNotify {
    /// Posted by the cue-watcher thread when a cue finishes naturally. Carries
    /// the cue generation so the loop can ignore stale watchers (cue replaced
    /// or stopped before this fired). Routing it through the host (not
    /// straight to the upstream `events` channel) is what lets the loop clear
    /// `cue_active` and lift the click duck before broadcasting CueEnded.
    CueEnded(u64),
    /// Posted by the pad-watcher thread when a pad decoder exits. The host
    /// only emits a `PadEnded` event if the generation still matches — that
    /// way crossfades and explicit Stop calls (which also flip the decoder's
    /// `ended` flag) don't fire a redundant pad-ended event.
    PadEnded(u64),
}

/// Events the engine pushes upstream so commands.rs can broadcast NowPlaying
/// edges (cue/pad started/ended) without polling.
#[derive(Clone, Copy, Debug)]
pub enum EngineEvent {
    CueStarted,
    CueEnded,
    /// The currently-playing pad voice exited unexpectedly (decoder errored —
    /// file moved, share dropped). Lets commands.rs flip `now.playing` back
    /// to false so the "is this key playing?" check doesn't wedge.
    PadEnded,
}

/// Control handle. Cheap to clone-share via Tauri state.
pub struct AudioEngine {
    tx: Sender<EngineCommand>,
    /// Events from the engine to anyone who wants them (one consumer at a
    /// time; commands.rs owns the receive side).
    events_rx: crossbeam_channel::Receiver<EngineEvent>,
    /// True once a stream is up. Read synchronously by `play` / `play_cue`
    /// so the caller gets an honest error (instead of a silent drop) when the
    /// engine is still booting or after a device-open failure.
    has_active: Arc<AtomicBool>,
    /// True while the host thread is mid-`build_stream`. Used to attribute
    /// timeouts correctly when a second SetOutput is queued behind a hung one.
    build_in_progress: Arc<AtomicBool>,
    /// Snapshot of the currently-open stream (None when no device is up or the
    /// last open failed). Set by the host thread, read by `active_output`.
    active_output: Arc<Mutex<Option<ActiveOutput>>>,
    /// Live callback buffer size in frames, written by the RT callback on each
    /// invocation (0 = no audio has flowed yet). Read into `ActiveOutput`.
    buffer_frames: Arc<AtomicU32>,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (notify_tx, notify_rx) = crossbeam_channel::unbounded::<HostNotify>();
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let has_active = Arc::new(AtomicBool::new(false));
        let build_in_progress = Arc::new(AtomicBool::new(false));
        let active_output = Arc::new(Mutex::new(None));
        let buffer_frames = Arc::new(AtomicU32::new(0));
        let has_active_t = has_active.clone();
        let build_in_progress_t = build_in_progress.clone();
        let active_output_t = active_output.clone();
        let buffer_frames_t = buffer_frames.clone();
        std::thread::Builder::new()
            .name("audio-host".into())
            .spawn(move || {
                host_thread(
                    rx,
                    notify_rx,
                    notify_tx,
                    events_tx,
                    has_active_t,
                    build_in_progress_t,
                    active_output_t,
                    buffer_frames_t,
                )
            })
            .expect("failed to spawn audio host thread");
        AudioEngine {
            tx,
            events_rx,
            has_active,
            build_in_progress,
            active_output,
            buffer_frames,
        }
    }

    /// Snapshot of the stream that is currently open (device, channels, sample
    /// rate, format, live buffer size), or `None` when nothing is open.
    pub fn active_output(&self) -> Option<ActiveOutput> {
        let mut info = self.active_output.lock().unwrap().clone();
        if let Some(ref mut o) = info {
            let bf = self.buffer_frames.load(Ordering::Relaxed);
            o.buffer_frames = (bf > 0).then_some(bf);
        }
        info
    }

    /// Borrow the upstream event stream. Cloneable (crossbeam Receiver),
    /// but the audio host only ever sends one of each event so only one
    /// listener should typically drain it.
    pub fn events(&self) -> crossbeam_channel::Receiver<EngineEvent> {
        self.events_rx.clone()
    }

    /// Enumerate output devices across every cpal host available on this
    /// platform (WASAPI always; ASIO when its driver is installed and the
    /// `asio` feature was compiled in).
    pub fn list_devices() -> Vec<DeviceInfo> {
        let default_host = cpal::default_host();
        let default_device_name = default_host
            .default_output_device()
            .and_then(|d| d.name().ok());
        let default_host_id = default_host.id();

        let mut out = Vec::new();
        for host_id in cpal::available_hosts() {
            let Ok(host) = cpal::host_from_id(host_id) else {
                continue;
            };
            let host_label = host_id_label(host_id);
            let Ok(devices) = host.output_devices() else {
                continue;
            };
            for device in devices {
                let Ok(name) = device.name() else { continue };
                // Some ASIO drivers list defaults that won't actually open;
                // skip a device entirely if no output config is reachable.
                let (channels, sample_rate) = match best_output_summary(&device) {
                    Some(t) => t,
                    None => continue,
                };
                let is_default = host_id == default_host_id
                    && default_device_name.as_deref() == Some(name.as_str());
                out.push(DeviceInfo {
                    host: host_label.to_string(),
                    name,
                    channels,
                    default_sample_rate: sample_rate,
                    is_default,
                });
            }
        }
        out
    }

    /// Produce a full human-readable audio report — every host, every device,
    /// its default config and all supported (format, channels, sample-rate)
    /// combinations — and write it to the log. Returned as a string so the UI
    /// can show or copy it. This is the artifact to ask a user for when a device
    /// won't open: it captures exactly what cpal sees on their machine.
    pub fn diagnostics() -> String {
        let mut report = String::new();
        report.push_str(&format!(
            "StagePal audio diagnostics (v{})\n",
            env!("CARGO_PKG_VERSION")
        ));
        let default_host = cpal::default_host();
        let default_dev = default_host
            .default_output_device()
            .and_then(|d| d.name().ok());
        report.push_str(&format!(
            "default host: {} | default output: {}\n",
            host_id_label(default_host.id()),
            default_dev.as_deref().unwrap_or("(none)")
        ));
        report.push_str(&format!(
            "hosts available: {}\n",
            cpal::available_hosts()
                .iter()
                .map(|h| host_id_label(*h))
                .collect::<Vec<_>>()
                .join(", ")
        ));

        for host_id in cpal::available_hosts() {
            let label = host_id_label(host_id);
            report.push_str(&format!("\n== {label} ==\n"));
            let host = match cpal::host_from_id(host_id) {
                Ok(h) => h,
                Err(e) => {
                    report.push_str(&format!("  ! could not open host: {e}\n"));
                    continue;
                }
            };
            let devices = match host.output_devices() {
                Ok(d) => d,
                Err(e) => {
                    report.push_str(&format!("  ! could not enumerate devices: {e}\n"));
                    continue;
                }
            };
            let mut any = false;
            for device in devices {
                any = true;
                let name = device.name().unwrap_or_else(|_| "(unnamed)".into());
                report.push_str(&format!("  • {name}\n"));
                match device.default_output_config() {
                    Ok(c) => report.push_str(&format!(
                        "      default: {} ch, {} Hz, {:?}\n",
                        c.channels(),
                        c.sample_rate().0,
                        c.sample_format()
                    )),
                    Err(e) => report.push_str(&format!("      default config error: {e}\n")),
                }
                match device.supported_output_configs() {
                    Ok(cfgs) => {
                        // Collapse the (often large) ASIO list to: per format,
                        // the max channel count and the set of sample rates.
                        let cfgs: Vec<_> = cfgs.collect();
                        if cfgs.is_empty() {
                            report.push_str("      supported: (none reported)\n");
                        }
                        for fmt in [
                            SampleFormat::F32,
                            SampleFormat::I32,
                            SampleFormat::I16,
                            SampleFormat::U16,
                        ] {
                            let matching: Vec<_> =
                                cfgs.iter().filter(|c| c.sample_format() == fmt).collect();
                            if matching.is_empty() {
                                continue;
                            }
                            let max_ch =
                                matching.iter().map(|c| c.channels()).max().unwrap_or(0);
                            let mut rates: Vec<u32> = matching
                                .iter()
                                .map(|c| c.max_sample_rate().0)
                                .collect();
                            rates.sort_unstable();
                            rates.dedup();
                            report.push_str(&format!(
                                "      supported {fmt:?}: up to {max_ch} ch @ {rates:?} Hz\n"
                            ));
                        }
                    }
                    Err(e) => report.push_str(&format!("      supported configs error: {e}\n")),
                }
            }
            if !any {
                report.push_str("  (no output devices)\n");
            }
        }

        crate::linfo!("audio diagnostics requested:\n{report}");
        report
    }

    pub fn set_output(
        &self,
        host: &str,
        device: &str,
        pad_channels: (usize, usize),
        click_channels: (usize, usize),
        cue_channels: (usize, usize),
    ) -> Result<(), String> {
        let (reply, reply_rx) = crossbeam_channel::bounded(1);
        let busy = self.build_in_progress.load(Ordering::Relaxed);
        self.tx
            .send(EngineCommand::SetOutput {
                host: host.to_string(),
                device: device.to_string(),
                pad_channels,
                click_channels,
                cue_channels,
                reply,
            })
            .map_err(|_| "audio host thread is gone".to_string())?;
        await_stream_reply(&reply_rx, busy)
    }

    pub fn set_click_channels(&self, channels: (usize, usize)) -> Result<(), String> {
        let (reply, reply_rx) = crossbeam_channel::bounded(1);
        let busy = self.build_in_progress.load(Ordering::Relaxed);
        self.tx
            .send(EngineCommand::SetClickChannels { channels, reply })
            .map_err(|_| "audio host thread is gone".to_string())?;
        await_stream_reply(&reply_rx, busy)
    }

    pub fn set_cue_channels(&self, channels: (usize, usize)) -> Result<(), String> {
        let (reply, reply_rx) = crossbeam_channel::bounded(1);
        let busy = self.build_in_progress.load(Ordering::Relaxed);
        self.tx
            .send(EngineCommand::SetCueChannels { channels, reply })
            .map_err(|_| "audio host thread is gone".to_string())?;
        await_stream_reply(&reply_rx, busy)
    }

    pub fn set_click_enabled(&self, enabled: bool) -> Result<(), String> {
        self.tx
            .send(EngineCommand::SetClickEnabled(enabled))
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn set_click_bpm(&self, bpm: f32) -> Result<(), String> {
        self.tx
            .send(EngineCommand::SetClickBpm(bpm))
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn set_click_beats(&self, beats: u32) -> Result<(), String> {
        self.tx
            .send(EngineCommand::SetClickBeats(beats))
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn set_click_accent(&self, accent: bool) -> Result<(), String> {
        self.tx
            .send(EngineCommand::SetClickAccent(accent))
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn set_click_volume(&self, volume: f32) -> Result<(), String> {
        self.tx
            .send(EngineCommand::SetClickVolume(volume))
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn play(&self, path: PathBuf) -> Result<(), String> {
        if !self.has_active.load(Ordering::Relaxed) {
            return Err("audio output not ready — set or restore an output device first".into());
        }
        self.tx
            .send(EngineCommand::Play(path))
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn stop(&self) -> Result<(), String> {
        self.tx
            .send(EngineCommand::Stop)
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn play_cue(&self, path: PathBuf) -> Result<(), String> {
        if !self.has_active.load(Ordering::Relaxed) {
            // Caller (e.g. cue_speak_logic) just synthesized this WAV to
            // %TEMP%; if we don't take ownership of it, it will leak —
            // decode::spawn would normally delete it after playback, but we
            // never get that far. Best-effort delete here.
            let _ = std::fs::remove_file(&path);
            return Err("audio output not ready — set or restore an output device first".into());
        }
        self.tx
            .send(EngineCommand::PlayCue(path))
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn stop_cue(&self) -> Result<(), String> {
        self.tx
            .send(EngineCommand::StopCue)
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn set_duck_click(&self, duck: bool) -> Result<(), String> {
        self.tx
            .send(EngineCommand::SetDuckClick(duck))
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn set_volume(&self, volume: f32) -> Result<(), String> {
        self.tx
            .send(EngineCommand::SetVolume(volume.clamp(0.0, 1.0)))
            .map_err(|_| "audio host thread is gone".to_string())
    }

    pub fn set_cue_volume(&self, volume: f32) -> Result<(), String> {
        self.tx
            .send(EngineCommand::SetCueVolume(volume.clamp(0.0, 1.0)))
            .map_err(|_| "audio host thread is gone".to_string())
    }

    /// Set the crossfade/fade-out duration (milliseconds) used for subsequent
    /// `play`/`stop` calls.
    pub fn set_crossfade(&self, ms: u32) -> Result<(), String> {
        self.tx
            .send(EngineCommand::SetCrossfade(ms))
            .map_err(|_| "audio host thread is gone".to_string())
    }
}

/// Live stream + the producer end of the callback's command queue.
struct ActiveStream {
    _stream: cpal::Stream,
    cmd_tx: rtrb::Producer<PlayCommand>,
    out_rate: u32,
    /// What this stream actually opened as, for the UI / diagnostics.
    info: ActiveOutput,
}

const DEFAULT_CROSSFADE_MS: u32 = 2000;

/// How long to let a *cold* ASIO stream actually run before tearing it down to
/// prime the driver (see `build_stream_primed`). The manual cure — "change a
/// channel, then change back" — leaves a stream running for a beat before the
/// reopen, and some virtual ASIO drivers (notably Voicemeeter's Virtual ASIO)
/// only start routing audio on a reopen that follows a genuinely-running stream.
const COLD_OPEN_WARMUP: Duration = Duration::from_millis(600);

/// Settle after releasing a warmed-up cold stream, before the next open. Virtual
/// ASIO drivers can take a moment to fully relinquish the device.
const COLD_OPEN_REOPEN_SETTLE: Duration = Duration::from_millis(250);

/// Per-frame gain step so a 0→1 (or 1→0) ramp completes in `ms` at `rate`.
fn fade_step(ms: u32, rate: u32) -> f32 {
    let frames = (ms.max(1) as f32 / 1000.0 * rate as f32).max(1.0);
    1.0 / frames
}

fn host_id_label(id: HostId) -> &'static str {
    // cpal's HostId names are stable strings ("WASAPI", "ASIO", ...).
    // Centralising the conversion lets us match on them in `set_output`.
    id.name()
}

fn host_from_label(label: &str) -> Result<cpal::Host, String> {
    for id in cpal::available_hosts() {
        if host_id_label(id).eq_ignore_ascii_case(label) {
            return cpal::host_from_id(id).map_err(|e| format!("open host '{label}': {e}"));
        }
    }
    Err(format!("audio host '{label}' is not available"))
}

/// Brief (channels, sample_rate) summary for device-list rendering. Mirrors
/// `pick_supported` so the dropdown shows the channel count and rate the device
/// will *actually* open with — not a different probe that could disagree (which
/// is exactly how the old ASIO bug hid: the list showed 18 ch while the stream
/// opened mono). Returns `None` if no f32/i32 config is reachable, so such a
/// device is dropped from the picker instead of looking selectable-but-broken.
fn best_output_summary(device: &cpal::Device) -> Option<(usize, u32)> {
    let cfg = pick_supported(device).ok()?;
    Some((cfg.channels() as usize, cfg.sample_rate().0))
}

#[allow(clippy::too_many_arguments)]
fn host_thread(
    rx: Receiver<EngineCommand>,
    notify_rx: Receiver<HostNotify>,
    notify_tx: Sender<HostNotify>,
    events: crossbeam_channel::Sender<EngineEvent>,
    has_active: Arc<AtomicBool>,
    build_in_progress: Arc<AtomicBool>,
    active_output: Arc<Mutex<Option<ActiveOutput>>>,
    buffer_frames: Arc<AtomicU32>,
) {
    let mut active: Option<ActiveStream> = None;
    let mut master: f32 = 0.8;
    let mut cue_volume: f32 = 1.0;
    let mut crossfade_ms: u32 = DEFAULT_CROSSFADE_MS;
    // Last known device + channel layout, so SetClickChannels / SetCueChannels
    // can re-issue build_stream with the right context.
    let mut last_host: String = String::new();
    let mut last_device: String = String::new();
    let mut last_channels = ChannelLayout {
        pad: (0, 1),
        click: (2, 3),
        cue: (4, 5),
    };
    // Click state shadow — re-applied on every stream rebuild. `enabled` is
    // deliberately not seeded from settings on boot (worship leaders shouldn't
    // be surprised by a live click on launch); the desktop/remote toggle it on.
    let mut click_enabled: bool = false;
    let mut click_bpm: f32 = 90.0;
    let mut click_beats: u32 = 4;
    let mut click_accent: bool = true;
    let mut click_volume: f32 = 0.8;
    // Cue duck preference and the live "is a cue currently speaking" flag,
    // tracked here so we can update the click ducking ramp when either changes.
    let mut duck_click_pref: bool = false;
    let mut cue_active: bool = false;
    // Monotonic id for the in-flight cue. Each PushCue / StopCue claims a new
    // value; the cue-watcher thread captures its id at spawn time and only
    // fires CueEnded if it still matches. This stops a stale watcher (from a
    // replaced or stopped cue) from clearing the "speaking" badge while a
    // newer cue is still audible.
    let cue_generation = Arc::new(AtomicU64::new(0));
    // Same idea for pads — incremented on each Play / Stop / Crossfade /
    // stream rebuild, so a pad watcher only reports its decoder's exit if
    // the voice it was tracking is still the current pad.
    let pad_generation = Arc::new(AtomicU64::new(0));

    // Receive on both the public command channel and the internal notify
    // channel. When AudioEngine is dropped, the public channel disconnects
    // and we exit; the notify channel stays open (host owns the sender) but
    // that's fine — there's nothing left to drive it.
    loop {
        use crossbeam_channel::Select;
        let mut sel = Select::new();
        let cmd_idx = sel.recv(&rx);
        let notify_idx = sel.recv(&notify_rx);
        let oper = sel.select();
        let i = oper.index();
        if i == cmd_idx {
            let cmd = match oper.recv(&rx) {
                Ok(c) => c,
                Err(_) => break,
            };
            match cmd {
            EngineCommand::SetOutput {
                host,
                device,
                pad_channels,
                click_channels,
                cue_channels,
                reply,
            } => {
                let click_init = ClickInit {
                    enabled: click_enabled,
                    bpm: click_bpm,
                    beats_per_bar: click_beats,
                    accent: click_accent,
                    volume: click_volume,
                };
                let channels = ChannelLayout {
                    pad: pad_channels,
                    click: click_channels,
                    cue: cue_channels,
                };
                // Cold open == the host thread holds no live stream yet (boot
                // auto-restore, or first device pick). Those need the ASIO prime;
                // a warm reselect already has a running stream and doesn't.
                let cold_open = active.is_none();
                build_in_progress.store(true, Ordering::Relaxed);
                let result = build_stream_primed(
                    &host,
                    &device,
                    channels,
                    master,
                    cue_volume,
                    click_init,
                    &buffer_frames,
                    cold_open,
                );
                build_in_progress.store(false, Ordering::Relaxed);
                match result {
                    Ok(stream) => {
                        // Replacing the old stream drops it and every voice it
                        // was carrying. Invalidate any orphan pad/cue watcher,
                        // flush playing-state immediately (don't wait for the
                        // ~300ms watcher tail), and re-apply the cue duck on
                        // the freshly-built stream so a mid-cue rebuild
                        // doesn't slam the click back to full volume.
                        pad_generation.fetch_add(1, Ordering::Relaxed);
                        cue_generation.fetch_add(1, Ordering::Relaxed);
                        if cue_active {
                            cue_active = false;
                            let _ = events.send(EngineEvent::CueEnded);
                        }
                        last_host = host;
                        last_device = device;
                        last_channels = channels;
                        *active_output.lock().unwrap() = Some(stream.info.clone());
                        active = Some(stream);
                        has_active.store(true, Ordering::Relaxed);
                        let _ = reply.send(Ok(()));
                    }
                    Err(e) => {
                        crate::lerror!("[audio] set_output failed: {e}");
                        active = None;
                        has_active.store(false, Ordering::Relaxed);
                        *active_output.lock().unwrap() = None;
                        // The device that just failed isn't worth retrying for
                        // subsequent channel-only edits — clear `last_*` so the
                        // next SetClickChannels/SetCueChannels short-circuits
                        // with an honest "no device" response instead of
                        // re-running the same failure under the hood.
                        last_host.clear();
                        last_device.clear();
                        let _ = reply.send(Err(e));
                    }
                }
            }
            EngineCommand::SetClickChannels { channels, reply } => {
                if last_device.is_empty() {
                    let _ = reply.send(Ok(()));
                    continue;
                }
                let click_init = ClickInit {
                    enabled: click_enabled,
                    bpm: click_bpm,
                    beats_per_bar: click_beats,
                    accent: click_accent,
                    volume: click_volume,
                };
                let next = ChannelLayout { click: channels, ..last_channels };
                build_in_progress.store(true, Ordering::Relaxed);
                let result = build_stream(&last_host, &last_device, next, master, cue_volume, click_init, &buffer_frames);
                build_in_progress.store(false, Ordering::Relaxed);
                match result {
                    Ok(stream) => {
                        pad_generation.fetch_add(1, Ordering::Relaxed);
                        cue_generation.fetch_add(1, Ordering::Relaxed);
                        if cue_active {
                            cue_active = false;
                            let _ = events.send(EngineEvent::CueEnded);
                        }
                        last_channels = next;
                        *active_output.lock().unwrap() = Some(stream.info.clone());
                        active = Some(stream);
                        has_active.store(true, Ordering::Relaxed);
                        let _ = reply.send(Ok(()));
                    }
                    Err(e) => {
                        crate::lerror!("[audio] re-open for channel change failed: {e}");
                        active = None;
                        has_active.store(false, Ordering::Relaxed);
                        *active_output.lock().unwrap() = None;
                        last_host.clear();
                        last_device.clear();
                        let _ = reply.send(Err(e));
                    }
                }
            }
            EngineCommand::SetCueChannels { channels, reply } => {
                if last_device.is_empty() {
                    let _ = reply.send(Ok(()));
                    continue;
                }
                let click_init = ClickInit {
                    enabled: click_enabled,
                    bpm: click_bpm,
                    beats_per_bar: click_beats,
                    accent: click_accent,
                    volume: click_volume,
                };
                let next = ChannelLayout { cue: channels, ..last_channels };
                build_in_progress.store(true, Ordering::Relaxed);
                let result = build_stream(&last_host, &last_device, next, master, cue_volume, click_init, &buffer_frames);
                build_in_progress.store(false, Ordering::Relaxed);
                match result {
                    Ok(stream) => {
                        pad_generation.fetch_add(1, Ordering::Relaxed);
                        cue_generation.fetch_add(1, Ordering::Relaxed);
                        if cue_active {
                            cue_active = false;
                            let _ = events.send(EngineEvent::CueEnded);
                        }
                        last_channels = next;
                        *active_output.lock().unwrap() = Some(stream.info.clone());
                        active = Some(stream);
                        has_active.store(true, Ordering::Relaxed);
                        let _ = reply.send(Ok(()));
                    }
                    Err(e) => {
                        crate::lerror!("[audio] re-open for channel change failed: {e}");
                        active = None;
                        has_active.store(false, Ordering::Relaxed);
                        *active_output.lock().unwrap() = None;
                        last_host.clear();
                        last_device.clear();
                        let _ = reply.send(Err(e));
                    }
                }
            }
            EngineCommand::Play(path) => {
                if let Some(act) = active.as_mut() {
                    let dec = decode::spawn(path, act.out_rate, true, false);
                    let voice = Voice {
                        consumer: dec.consumer,
                        stop: dec.stop,
                        ended: dec.ended.clone(),
                        bus: VoiceBus::Pad,
                        gain: 0.0,
                        target: 1.0,
                        step: fade_step(crossfade_ms, act.out_rate),
                        remove_when_silent: false,
                    };
                    let _ = act.cmd_tx.push(PlayCommand::Crossfade(voice));
                    // Bump the pad generation, then spawn a watcher tied to
                    // this generation. If the decoder later exits for any
                    // reason — natural crossfade-stop, explicit Stop, or an
                    // error mid-set — the host will see NotifyPadEnded.
                    // The host only emits EngineEvent::PadEnded when the
                    // generation still matches: a crossfade or Stop bumps the
                    // generation first, so those legitimate exits stay quiet.
                    let my_gen = pad_generation.fetch_add(1, Ordering::Relaxed) + 1;
                    let watcher_ended = dec.ended;
                    let watcher_notify = notify_tx.clone();
                    let watcher_gen_arc = pad_generation.clone();
                    std::thread::Builder::new()
                        .name("pad-watcher".into())
                        .spawn(move || {
                            while !watcher_ended.load(Ordering::Relaxed) {
                                std::thread::sleep(Duration::from_millis(200));
                            }
                            // Skip post if the pad has already been replaced
                            // (saves a no-op trip through the host loop).
                            if watcher_gen_arc.load(Ordering::Relaxed) == my_gen {
                                let _ = watcher_notify.send(HostNotify::PadEnded(my_gen));
                            }
                        })
                        .ok();
                } else {
                    eprintln!("[audio] Play ignored: no output device configured");
                }
            }
            EngineCommand::Stop => {
                if let Some(act) = active.as_mut() {
                    // Invalidate the current pad watcher so its eventual
                    // PadEnded post is ignored — the user initiated the stop,
                    // they don't need a redundant "pad ended" event.
                    pad_generation.fetch_add(1, Ordering::Relaxed);
                    let step = fade_step(crossfade_ms, act.out_rate);
                    let _ = act.cmd_tx.push(PlayCommand::FadeOutAll(step));
                }
            }
            EngineCommand::PlayCue(path) => {
                let Some(act) = active.as_mut() else {
                    eprintln!("[audio] PlayCue ignored: no output device configured");
                    // The synthesized temp WAV would normally be deleted by
                    // decode::spawn's thread on exit (delete_on_exit=true).
                    // We never reach that path here, so clean up directly
                    // instead of leaking the file in %TEMP%.
                    let _ = std::fs::remove_file(&path);
                    continue;
                };
                let dec = decode::spawn(path, act.out_rate, false, true);
                let voice = Voice {
                    consumer: dec.consumer,
                    stop: dec.stop,
                    ended: dec.ended.clone(),
                    bus: VoiceBus::Cue,
                    gain: 1.0,
                    target: 1.0,
                    // Short fade-in/out so cue start/stop never click. Not a
                    // pad-style crossfade — voice is a one-shot.
                    step: fade_step(50, act.out_rate),
                    remove_when_silent: false,
                };
                let _ = act.cmd_tx.push(PlayCommand::PushCue(voice));
                cue_active = true;
                let my_gen = cue_generation.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = events.send(EngineEvent::CueStarted);
                if duck_click_pref {
                    let _ = act.cmd_tx.push(PlayCommand::SetClickDuckActive(true));
                }
                // Watcher: when the decoder signals "ended", give the audio
                // buffer ~300 ms to drain, then post a notification back to
                // the engine loop. Routing through the internal notify
                // channel (not straight to `events`) is what lets the loop
                // clear `cue_active` and lift the click duck before
                // broadcasting CueEnded.
                let watcher_ended = dec.ended;
                let watcher_notify = notify_tx.clone();
                std::thread::Builder::new()
                    .name("cue-watcher".into())
                    .spawn(move || {
                        while !watcher_ended.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        std::thread::sleep(Duration::from_millis(300));
                        let _ = watcher_notify.send(HostNotify::CueEnded(my_gen));
                    })
                    .ok();
            }
            EngineCommand::StopCue => {
                if let Some(act) = active.as_mut() {
                    let step = fade_step(50, act.out_rate);
                    let _ = act.cmd_tx.push(PlayCommand::StopCue(step));
                    if cue_active {
                        cue_active = false;
                        // Invalidate the current watcher before firing — we're
                        // emitting CueEnded ourselves and don't want a delayed
                        // duplicate from the watcher.
                        cue_generation.fetch_add(1, Ordering::Relaxed);
                        let _ = events.send(EngineEvent::CueEnded);
                    }
                    if duck_click_pref {
                        let _ = act.cmd_tx.push(PlayCommand::SetClickDuckActive(false));
                    }
                }
            }
            EngineCommand::SetVolume(v) => {
                master = v;
                if let Some(act) = active.as_mut() {
                    let _ = act.cmd_tx.push(PlayCommand::SetMaster(v));
                }
            }
            EngineCommand::SetCueVolume(v) => {
                cue_volume = v;
                if let Some(act) = active.as_mut() {
                    let _ = act.cmd_tx.push(PlayCommand::SetCueVolume(v));
                }
            }
            EngineCommand::SetCrossfade(ms) => {
                crossfade_ms = ms.max(1);
            }
            EngineCommand::SetClickEnabled(en) => {
                click_enabled = en;
                if let Some(act) = active.as_mut() {
                    let _ = act.cmd_tx.push(PlayCommand::SetClickEnabled(en));
                }
            }
            EngineCommand::SetClickBpm(bpm) => {
                click_bpm = bpm;
                if let Some(act) = active.as_mut() {
                    let _ = act.cmd_tx.push(PlayCommand::SetClickBpm(bpm));
                }
            }
            EngineCommand::SetClickBeats(b) => {
                click_beats = b;
                if let Some(act) = active.as_mut() {
                    let _ = act.cmd_tx.push(PlayCommand::SetClickBeats(b));
                }
            }
            EngineCommand::SetClickAccent(a) => {
                click_accent = a;
                if let Some(act) = active.as_mut() {
                    let _ = act.cmd_tx.push(PlayCommand::SetClickAccent(a));
                }
            }
            EngineCommand::SetClickVolume(v) => {
                click_volume = v;
                if let Some(act) = active.as_mut() {
                    let _ = act.cmd_tx.push(PlayCommand::SetClickVolume(v));
                }
            }
            EngineCommand::SetDuckClick(pref) => {
                duck_click_pref = pref;
                if let Some(act) = active.as_mut() {
                    let _ = act.cmd_tx.push(PlayCommand::SetClickDuckActive(pref && cue_active));
                }
            }
            }
        } else if i == notify_idx {
            let n = match oper.recv(&notify_rx) {
                Ok(n) => n,
                // Host owns a sender, so this branch shouldn't fire — but if
                // it ever does, treat it the same as the command-side close.
                Err(_) => break,
            };
            match n {
                HostNotify::CueEnded(gen) => {
                    // Stale watcher (cue was replaced or stopped before this
                    // fired) — the newer path already handled state, so ignore.
                    if cue_generation.load(Ordering::Relaxed) != gen || !cue_active {
                        continue;
                    }
                    cue_active = false;
                    if duck_click_pref {
                        if let Some(act) = active.as_mut() {
                            let _ = act.cmd_tx.push(PlayCommand::SetClickDuckActive(false));
                        }
                    }
                    let _ = events.send(EngineEvent::CueEnded);
                }
                HostNotify::PadEnded(gen) => {
                    // Stale (a crossfade/stop/rebuild already bumped the
                    // generation) — that path either replaced the voice with
                    // a fresh one or intentionally stopped it.
                    if pad_generation.load(Ordering::Relaxed) != gen {
                        continue;
                    }
                    // Same generation: the decoder exited unexpectedly. Bump
                    // the generation so subsequent state changes don't fire
                    // again for the same voice.
                    pad_generation.fetch_add(1, Ordering::Relaxed);
                    let _ = events.send(EngineEvent::PadEnded);
                }
            }
        }
    }
}

/// Pick a usable supported config: prefer f32 (matches WASAPI default), fall
/// back to i32 (typical for ASIO drivers). Anything else is rejected.
/// Choose the stream config to open a device with.
///
/// The device's *default* config is the right answer for almost every device:
/// it carries the full physical channel count, the driver's current sample rate
/// (so we don't force an ASIO clock change that can fail or glitch other apps),
/// and the native sample format. We only fall back to scanning `supported_output_configs`
/// when the default's format isn't one we can feed (we render f32/i32).
///
/// History: the old code took the *first* matching config from
/// `supported_output_configs()`. That worked on WASAPI (whose first config is the
/// stereo mix format) but broke every ASIO device — cpal's ASIO backend
/// enumerates configs starting at **1 channel**, so the first match was mono and
/// the pad bus on channels (0,1) was rejected as "out of range". Selecting by max
/// channel count (and preferring the native default) is what makes ASIO work.
fn pick_supported(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
    // 1. Prefer the driver's own default — full channels, native rate + format.
    if let Ok(cfg) = device.default_output_config() {
        if matches!(cfg.sample_format(), SampleFormat::F32 | SampleFormat::I32) {
            return Ok(cfg);
        }
    }

    // 2. Otherwise scan: among f32 (preferred) then i32 ranges, take the one
    //    with the MOST channels so multi-out routing has room to land. Pin the
    //    rate to the device default when that rate is offered, else max.
    let configs: Vec<_> = device
        .supported_output_configs()
        .map_err(|e| format!("supported configs: {e}"))?
        .collect();
    let default_rate = device.default_output_config().ok().map(|c| c.sample_rate());

    let pick = |fmt: SampleFormat| -> Option<cpal::SupportedStreamConfig> {
        let range = configs
            .iter()
            .filter(|c| c.sample_format() == fmt)
            .max_by_key(|c| c.channels())?;
        // Use the device's default rate if this range covers it, else its max.
        let cfg = match default_rate {
            Some(r) if r >= range.min_sample_rate() && r <= range.max_sample_rate() => {
                range.clone().with_sample_rate(r)
            }
            _ => range.clone().with_max_sample_rate(),
        };
        Some(cfg)
    };

    pick(SampleFormat::F32)
        .or_else(|| pick(SampleFormat::I32))
        .ok_or_else(|| {
            "device exposes no f32/i32 output config — cpal can only feed those formats"
                .to_string()
        })
}

/// A stereo bus's routing: physical channel indexes on the output device.
/// `l == r` means mono (mix sums to -6 dB; see `write_pair`).
#[derive(Clone, Copy, Debug)]
struct BusRouting {
    /// `None` when the configured channel is out of range for the device —
    /// degrades silently rather than rejecting the switch.
    l: Option<usize>,
    r: Option<usize>,
}

impl BusRouting {
    fn new(l: usize, r: usize, total_channels: usize) -> Self {
        BusRouting {
            l: (l < total_channels).then_some(l),
            r: (r < total_channels).then_some(r),
        }
    }
}

/// Where each of the three buses lands on the device. Computed once when the
/// stream is built and handed to the callback.
#[derive(Clone, Copy, Debug)]
struct BusLayout {
    total: usize,
    pad: BusRouting,
    click: BusRouting,
    cue: BusRouting,
}

/// User-facing channel pairs for the three buses. Held by the host thread so
/// stream rebuilds (e.g. SetClickChannels) reuse the rest of the configuration.
#[derive(Clone, Copy, Debug)]
struct ChannelLayout {
    pad: (usize, usize),
    click: (usize, usize),
    cue: (usize, usize),
}

/// A deliberately-different, always-valid routing used to "wiggle" an ASIO
/// driver during a cold-open prime (see `build_stream_primed`): every bus
/// collapsed onto a single channel. Channel 0 exists on every device; if the
/// real pad pair is already there we use channel 1 instead, so the routing the
/// driver sees genuinely changes (the prime relies on the layout differing).
fn decoy_layout(real: ChannelLayout) -> ChannelLayout {
    let ch = if real.pad == (0, 0) { 1 } else { 0 };
    ChannelLayout {
        pad: (ch, ch),
        click: (ch, ch),
        cue: (ch, ch),
    }
}

/// Like `build_stream`, but on a *cold* ASIO open it replays the manual cure for
/// the "no signal until I change a channel" bug.
///
/// A cold first open of an ASIO driver routinely comes up "alive-but-silent":
/// the stream opens and `play()` succeeds, yet no audio reaches the hardware
/// until the routing is torn down and actually changed. A manual channel change
/// cures it only because it reopens after a running stream; on boot there is no
/// prior stream, so the cold open is trusted as-is and stays silent. Reproduce,
/// in code, the exact sequence that works by hand ("go to channel 1, then back
/// to the appropriate channel"): open on the real routing and let it run, reopen
/// on a decoy routing and let it run, then reopen on the real routing — the open
/// that finally routes audio. Nothing is audible during the first two steps: at
/// a cold open no pad/cue voice exists yet and the click boots disabled.
///
/// `cold_open` is true only when the host thread held no live stream beforehand
/// (boot auto-restore, or selecting a device when none was open). Warm reopens
/// and every WASAPI open fall straight through to a plain `build_stream`, so
/// they pay none of this cost.
#[allow(clippy::too_many_arguments)]
fn build_stream_primed(
    host_label: &str,
    device_name: &str,
    channels: ChannelLayout,
    master: f32,
    cue_volume: f32,
    click_init: ClickInit,
    buffer_frames: &Arc<AtomicU32>,
    cold_open: bool,
) -> Result<ActiveStream, String> {
    if !(cold_open && host_label.eq_ignore_ascii_case("ASIO")) {
        return build_stream(
            host_label,
            device_name,
            channels,
            master,
            cue_volume,
            click_init,
            buffer_frames,
        );
    }

    let decoy = decoy_layout(channels);
    crate::linfo!(
        "[audio] priming cold ASIO open of '{device_name}': real -> decoy {decoy:?} -> real"
    );

    let warm = build_stream(
        host_label,
        device_name,
        channels,
        master,
        cue_volume,
        click_init,
        buffer_frames,
    )?;
    std::thread::sleep(COLD_OPEN_WARMUP);
    drop(warm);
    std::thread::sleep(COLD_OPEN_REOPEN_SETTLE);

    // The decoy build can legitimately fail (e.g. a 1-channel device); if it
    // does, skip the wiggle and just reopen on the real layout below.
    match build_stream(
        host_label,
        device_name,
        decoy,
        master,
        cue_volume,
        click_init,
        buffer_frames,
    ) {
        Ok(decoy_stream) => {
            std::thread::sleep(COLD_OPEN_WARMUP);
            drop(decoy_stream);
            std::thread::sleep(COLD_OPEN_REOPEN_SETTLE);
        }
        Err(e) => crate::lerror!("[audio] cold-open decoy reopen skipped: {e}"),
    }

    build_stream(
        host_label,
        device_name,
        channels,
        master,
        cue_volume,
        click_init,
        buffer_frames,
    )
}

fn build_stream(
    host_label: &str,
    device_name: &str,
    channels: ChannelLayout,
    master: f32,
    cue_volume: f32,
    click_init: ClickInit,
    buffer_frames: &Arc<AtomicU32>,
) -> Result<ActiveStream, String> {
    crate::linfo!(
        "[audio] opening {host_label}/{device_name} — pad={:?} click={:?} cue={:?}",
        channels.pad,
        channels.click,
        channels.cue
    );
    // A fresh open hasn't produced a callback yet; clear the stale buffer size
    // so `active_output` doesn't report the previous device's value.
    buffer_frames.store(0, Ordering::Relaxed);

    let host = host_from_label(host_label)?;
    let device = host
        .output_devices()
        .map_err(|e| format!("enumerate devices: {e}"))?
        .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
        .ok_or_else(|| format!("output device '{device_name}' not found on {host_label}"))?;

    let supported = pick_supported(&device)?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();
    let total_channels = config.channels as usize;
    let out_rate = config.sample_rate.0;
    crate::linfo!(
        "[audio] {device_name}: selected {total_channels} ch @ {out_rate} Hz, {sample_format:?}"
    );
    let (pad_l, pad_r) = channels.pad;
    let (click_l, click_r) = channels.click;
    let (cue_l, cue_r) = channels.cue;

    if pad_l >= total_channels || pad_r >= total_channels {
        // With a healthy multi-out device this can't happen (pad defaults to
        // 0/1). It only fires if the driver opened with <2 outputs — usually an
        // ASIO control-panel set to mono. Say so instead of a bare index error.
        return Err(format!(
            "pad channels ({pad_l},{pad_r}) don't fit — {device_name} opened with only \
             {total_channels} output channel(s). Open the device's ASIO control panel and \
             enable at least 2 outputs."
        ));
    }
    // Click and cue channels are allowed to be out of range — the callback
    // simply doesn't write to them. Lets us silently degrade when switching
    // to a 2-channel device without rejecting the device switch entirely.
    let layout = BusLayout {
        total: total_channels,
        pad: BusRouting {
            l: Some(pad_l),
            r: Some(pad_r),
        },
        click: BusRouting::new(click_l, click_r, total_channels),
        cue: BusRouting::new(cue_l, cue_r, total_channels),
    };

    let click_gen = ClickGen::new(out_rate, click_init);

    // Lock-free queue: host thread → real-time callback.
    let (cmd_tx, cmd_rx) = RingBuffer::<PlayCommand>::new(64);

    let err_fn = |err| crate::lerror!("[audio] stream runtime error: {err}");
    let bf = buffer_frames.clone();
    let stream = match sample_format {
        SampleFormat::F32 => {
            let cb = build_callback_f32(layout, master, cue_volume, click_gen, cmd_rx, bf);
            device
                .build_output_stream(&config, cb, err_fn, None)
                .map_err(|e| format!("build output stream (f32): {e}"))?
        }
        SampleFormat::I32 => {
            let cb = build_callback_i32(layout, master, cue_volume, click_gen, cmd_rx, bf);
            device
                .build_output_stream(&config, cb, err_fn, None)
                .map_err(|e| format!("build output stream (i32): {e}"))?
        }
        other => return Err(format!("unsupported sample format {other:?}")),
    };

    stream
        .play()
        .map_err(|e| format!("start stream: {e}"))?;

    crate::linfo!("[audio] stream live on {host_label}/{device_name}");
    Ok(ActiveStream {
        _stream: stream,
        cmd_tx,
        out_rate,
        info: ActiveOutput {
            host: host_label.to_string(),
            device: device_name.to_string(),
            channels: total_channels,
            sample_rate: out_rate,
            sample_format: match sample_format {
                SampleFormat::F32 => "f32".to_string(),
                SampleFormat::I32 => "i32".to_string(),
                other => format!("{other:?}"),
            },
            buffer_frames: None,
        },
    })
}

/// Synthesized click. Lives entirely inside the real-time callback: no
/// allocation, no I/O, just integer counters and a windowed sine ping per beat.
/// A 20 ms equal-rate ramp on enable/disable keeps toggling click-free.
struct ClickGen {
    sample_rate: f32,
    bpm: f32,
    beats_per_bar: u32,
    accent: bool,
    volume: f32,
    samples_per_beat: f32,
    samples_since_beat: f32,
    beat_index: u32,
    // Active ping voice (one at a time — at 300 BPM the prior ping is long gone
    // before the next one fires).
    osc_phase: f32,    // radians, wraps at 2π
    osc_freq: f32,     // Hz
    osc_env: f32,      // current envelope, decays per sample
    osc_decay: f32,    // per-sample env multiplier
    // Enable ramp: ~20 ms equal-rate fade in/out to suppress click-on-toggle.
    enable_ramp: f32,
    enable_target: f32,
    enable_step: f32,
    // Cue-duck ramp: drops the click by ~12 dB while a cue is speaking so the
    // voice cuts through. Independent of `enable_ramp` and `volume` so the
    // user's click volume isn't disturbed.
    duck_ramp: f32,
    duck_target: f32,
    duck_step: f32,
}

/// Linear multiplier corresponding to a -12 dB drop (≈0.25). Hardcoded — the
/// duck amount isn't surfaced to the UI yet; the user only toggles whether
/// ducking happens at all.
const CUE_DUCK_GAIN: f32 = 0.251_188_64;

impl ClickGen {
    fn new(sample_rate: u32, init: ClickInit) -> Self {
        let sr = sample_rate as f32;
        let bpm = init.bpm.clamp(20.0, 400.0);
        let beats = init.beats_per_bar.clamp(1, 32);
        let enable_target = if init.enabled { 1.0 } else { 0.0 };
        ClickGen {
            sample_rate: sr,
            bpm,
            beats_per_bar: beats,
            accent: init.accent,
            volume: init.volume.clamp(0.0, 1.0),
            samples_per_beat: 60.0 / bpm * sr,
            // Re-arm so the first beat fires immediately on enable.
            samples_since_beat: 60.0 / bpm * sr,
            beat_index: 0,
            osc_phase: 0.0,
            osc_freq: 1500.0,
            osc_env: 0.0,
            osc_decay: env_decay_per_sample(sr),
            enable_ramp: enable_target,
            enable_target,
            enable_step: 1.0 / (0.020 * sr).max(1.0),
            duck_ramp: 1.0,
            duck_target: 1.0,
            // ~60 ms ducking ramp — fast enough to dip before the cue's first
            // syllable, slow enough not to thump on enable.
            duck_step: (1.0 - CUE_DUCK_GAIN) / (0.060 * sr).max(1.0),
        }
    }

    fn set_bpm(&mut self, bpm: f32) {
        self.bpm = bpm.clamp(20.0, 400.0);
        self.samples_per_beat = 60.0 / self.bpm * self.sample_rate;
    }
    fn set_beats(&mut self, b: u32) {
        self.beats_per_bar = b.clamp(1, 32);
        if self.beat_index >= self.beats_per_bar {
            self.beat_index = 0;
        }
    }
    fn set_accent(&mut self, a: bool) {
        self.accent = a;
    }
    fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
    }
    fn set_enabled(&mut self, en: bool) {
        self.enable_target = if en { 1.0 } else { 0.0 };
        if en {
            // Re-arm beat 1 to fire immediately so the user hears the click
            // start the moment they press play.
            self.beat_index = 0;
            self.samples_since_beat = self.samples_per_beat;
            self.osc_env = 0.0;
        }
    }
    fn set_duck_active(&mut self, active: bool) {
        self.duck_target = if active { CUE_DUCK_GAIN } else { 1.0 };
    }

    #[inline]
    fn next_sample(&mut self) -> f32 {
        if self.enable_ramp < self.enable_target {
            self.enable_ramp = (self.enable_ramp + self.enable_step).min(self.enable_target);
        } else if self.enable_ramp > self.enable_target {
            self.enable_ramp = (self.enable_ramp - self.enable_step).max(self.enable_target);
        }
        if self.duck_ramp < self.duck_target {
            self.duck_ramp = (self.duck_ramp + self.duck_step).min(self.duck_target);
        } else if self.duck_ramp > self.duck_target {
            self.duck_ramp = (self.duck_ramp - self.duck_step).max(self.duck_target);
        }

        // Fully off: skip the oscillator math entirely.
        if self.enable_ramp <= 0.0 && self.enable_target <= 0.0 {
            return 0.0;
        }

        if self.samples_since_beat >= self.samples_per_beat {
            self.samples_since_beat -= self.samples_per_beat;
            let is_one = self.beat_index == 0 && self.accent;
            self.osc_freq = if is_one { 2000.0 } else { 1500.0 };
            self.osc_phase = 0.0;
            self.osc_env = if is_one { 1.0 } else { 0.7 };
            self.beat_index = (self.beat_index + 1) % self.beats_per_bar.max(1);
        }

        let s = self.osc_phase.sin()
            * self.osc_env
            * self.volume
            * self.enable_ramp
            * self.duck_ramp;
        self.osc_phase += std::f32::consts::TAU * self.osc_freq / self.sample_rate;
        if self.osc_phase >= std::f32::consts::TAU {
            self.osc_phase -= std::f32::consts::TAU;
        }
        self.osc_env *= self.osc_decay;
        self.samples_since_beat += 1.0;

        s
    }
}

/// Per-sample envelope multiplier so a fresh ping decays from 1.0 to ~0.001
/// over 30 ms. Independent of BPM (the envelope IS the click).
fn env_decay_per_sample(sample_rate: f32) -> f32 {
    let frames = (sample_rate * 0.030).max(1.0);
    0.001_f32.powf(1.0 / frames)
}

/// One frame of mixed audio, separated by bus so the writer can route each to
/// its own channel pair.
#[derive(Default, Clone, Copy)]
struct MixedFrame {
    pad_l: f32,
    pad_r: f32,
    cue_l: f32,
    cue_r: f32,
}

/// Pull one f32 stereo frame from the active voices, splitting pad voices from
/// cue voices and applying master volume only to pads. Cues are louder by
/// default (set in CueSettings::volume on the host side), and we don't want
/// the pad's master volume to also attenuate the spoken voice.
#[inline]
fn mix_one_frame(voices: &mut Vec<Voice>, master: f32, cue_volume: f32) -> MixedFrame {
    let mut m = MixedFrame::default();

    for v in voices.iter_mut() {
        if v.gain < v.target {
            v.gain = (v.gain + v.step).min(v.target);
        } else if v.gain > v.target {
            v.gain = (v.gain - v.step).max(v.target);
        }

        if v.consumer.slots() >= 2 {
            let l = v.consumer.pop().unwrap_or(0.0);
            let r = v.consumer.pop().unwrap_or(0.0);
            match v.bus {
                VoiceBus::Pad => {
                    m.pad_l += l * v.gain;
                    m.pad_r += r * v.gain;
                }
                VoiceBus::Cue => {
                    m.cue_l += l * v.gain;
                    m.cue_r += r * v.gain;
                }
            }
        } else if v.ended.load(Ordering::Relaxed) {
            // Decoder is done and the ring is empty — fade the voice out so it
            // drops without a click. Applies to both cues (one-shot reached
            // EOF) and pads (decoder hit an error like the file going missing
            // mid-set — otherwise the voice would sit at full gain producing
            // silence and wedge the "is this key playing?" check).
            v.target = 0.0;
            v.remove_when_silent = true;
        }
    }

    voices.retain(|v| !(v.remove_when_silent && v.gain <= 0.0));

    m.pad_l *= master;
    m.pad_r *= master;
    m.cue_l *= cue_volume;
    m.cue_r *= cue_volume;
    m
}

#[inline]
fn drain_commands(
    voices: &mut Vec<Voice>,
    master: &mut f32,
    cue_volume: &mut f32,
    click: &mut ClickGen,
    cmd_rx: &mut rtrb::Consumer<PlayCommand>,
) {
    while let Ok(cmd) = cmd_rx.pop() {
        match cmd {
            PlayCommand::Crossfade(v) => {
                for old in voices.iter_mut() {
                    if old.bus == VoiceBus::Pad {
                        old.target = 0.0;
                        old.remove_when_silent = true;
                    }
                }
                voices.push(v);
            }
            PlayCommand::PushCue(v) => {
                // Replace any in-flight cue immediately — last press wins; the
                // band shouldn't have to wait through "Verse 2" before "Bridge"
                // can speak. Old cue's decoder thread will exit (Voice::Drop
                // flips stop).
                for old in voices.iter_mut() {
                    if old.bus == VoiceBus::Cue {
                        old.target = 0.0;
                        old.remove_when_silent = true;
                    }
                }
                voices.push(v);
            }
            PlayCommand::FadeOutAll(step) => {
                for v in voices.iter_mut() {
                    if v.bus == VoiceBus::Pad {
                        v.target = 0.0;
                        v.step = step;
                        v.remove_when_silent = true;
                    }
                }
            }
            PlayCommand::StopCue(step) => {
                for v in voices.iter_mut() {
                    if v.bus == VoiceBus::Cue {
                        v.target = 0.0;
                        v.step = step;
                        v.remove_when_silent = true;
                    }
                }
            }
            PlayCommand::SetMaster(m) => *master = m,
            PlayCommand::SetCueVolume(v) => *cue_volume = v,
            PlayCommand::SetClickEnabled(en) => click.set_enabled(en),
            PlayCommand::SetClickBpm(bpm) => click.set_bpm(bpm),
            PlayCommand::SetClickBeats(b) => click.set_beats(b),
            PlayCommand::SetClickAccent(a) => click.set_accent(a),
            PlayCommand::SetClickVolume(v) => click.set_volume(v),
            PlayCommand::SetClickDuckActive(active) => click.set_duck_active(active),
        }
    }
}

/// Sum one stereo bus into the given channel slots of `frame`.
///   - Both indexes present and equal → user picked mono; sum at -6 dB so
///     correlated material doesn't clip.
///   - Both indexes present and different → route stereo as-is.
///   - One present → mono input; write to whichever exists. Cues are also
///     reasonably "mono" (SAPI renders single-channel WAVs) so duplicating
///     the same sample into both channels of a stereo cue pair is the
///     correct behavior — handled by the caller passing l == r samples.
///   - Neither present → silently dropped (e.g. configured channels are out
///     of range for the current device).
///
/// Channel collisions across buses (cue and click on the same channel, say)
/// sum naturally because we always `+=`.
#[inline]
fn write_pair(frame: &mut [f32], bus: BusRouting, l: f32, r: f32) {
    match (bus.l, bus.r) {
        (Some(li), Some(ri)) if li == ri => {
            if li < frame.len() {
                frame[li] += 0.5 * (l + r);
            }
        }
        (Some(li), Some(ri)) => {
            if li < frame.len() {
                frame[li] += l;
            }
            if ri < frame.len() {
                frame[ri] += r;
            }
        }
        (Some(i), None) | (None, Some(i)) => {
            if i < frame.len() {
                frame[i] += 0.5 * (l + r);
            }
        }
        (None, None) => {}
    }
}

/// Write pad + click + cue into the right slots of `frame`. Click is mono;
/// it's expanded by passing the same sample as l/r.
#[inline]
fn write_frame_f32(frame: &mut [f32], layout: &BusLayout, mix: &MixedFrame, click: f32) {
    for sample in frame.iter_mut() {
        *sample = 0.0;
    }
    write_pair(frame, layout.pad, mix.pad_l, mix.pad_r);
    write_pair(frame, layout.click, click, click);
    write_pair(frame, layout.cue, mix.cue_l, mix.cue_r);
}

fn build_callback_f32(
    layout: BusLayout,
    master_init: f32,
    cue_volume_init: f32,
    click_init: ClickGen,
    mut cmd_rx: rtrb::Consumer<PlayCommand>,
    buffer_frames: Arc<AtomicU32>,
) -> impl FnMut(&mut [f32], &cpal::OutputCallbackInfo) + Send + 'static {
    let mut voices: Vec<Voice> = Vec::with_capacity(4);
    let mut master = master_init;
    let mut cue_volume = cue_volume_init;
    let mut click = click_init;

    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        // Publish the negotiated buffer size (frames) for diagnostics. Cheap,
        // RT-safe relaxed store; the value only changes if the driver resizes.
        let frames = (data.len() / layout.total.max(1)) as u32;
        buffer_frames.store(frames, Ordering::Relaxed);
        drain_commands(&mut voices, &mut master, &mut cue_volume, &mut click, &mut cmd_rx);

        for frame in data.chunks_mut(layout.total) {
            let m = mix_one_frame(&mut voices, master, cue_volume);
            let c = click.next_sample();
            write_frame_f32(frame, &layout, &m, c);
        }
    }
}

fn build_callback_i32(
    layout: BusLayout,
    master_init: f32,
    cue_volume_init: f32,
    click_init: ClickGen,
    mut cmd_rx: rtrb::Consumer<PlayCommand>,
    buffer_frames: Arc<AtomicU32>,
) -> impl FnMut(&mut [i32], &cpal::OutputCallbackInfo) + Send + 'static {
    let mut voices: Vec<Voice> = Vec::with_capacity(4);
    let mut master = master_init;
    let mut cue_volume = cue_volume_init;
    let mut click = click_init;

    // i32 full-scale. Headroom of 1 sample on the negative side avoids wrap.
    const SCALE: f32 = 2_147_483_520.0;

    // Scratch buffer reused per frame so we can compose the f32 sum and then
    // convert; sized for the worst-case channel count we'll ever see.
    let mut scratch = [0.0f32; 64];

    move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
        let frames = (data.len() / layout.total.max(1)) as u32;
        buffer_frames.store(frames, Ordering::Relaxed);
        drain_commands(&mut voices, &mut master, &mut cue_volume, &mut click, &mut cmd_rx);

        for frame in data.chunks_mut(layout.total) {
            let m = mix_one_frame(&mut voices, master, cue_volume);
            let c = click.next_sample();

            let n = frame.len().min(scratch.len());
            let scratch = &mut scratch[..n];
            write_frame_f32(scratch, &layout, &m, c);

            for (out, v) in frame.iter_mut().zip(scratch.iter()) {
                let clipped = v.clamp(-1.0, 1.0);
                *out = (clipped * SCALE) as i32;
            }
        }
    }
}
