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
use std::sync::Arc;
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

/// Result from the built-in routed test tone. This is deliberately surfaced to
/// production builds so a real ASIO rig can tell us whether the callback is
/// alive and writing nonzero samples, even when the mixer remains silent.
#[derive(Debug, Clone, Serialize)]
pub struct AudioDebugReport {
    pub host: String,
    pub device: String,
    pub sample_format: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub pad_channels: (usize, usize),
    pub callback_calls: u64,
    pub frames_written: u64,
    pub nonzero_frames: u64,
    pub peak: f32,
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
    /// Short sine tone mixed directly onto the pad bus. Used for field
    /// diagnostics because it bypasses file decode and pad library state.
    StartDiagnosticTone(DiagnosticTone),
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
    RunOutputTest {
        reply: Sender<Result<AudioDebugReport, String>>,
    },
}

#[derive(Clone)]
struct StreamDebugInfo {
    host: String,
    device: String,
    sample_format: String,
    sample_rate: u32,
    channels: usize,
    pad_channels: (usize, usize),
}

struct DiagnosticTone {
    phase: f32,
    phase_inc: f32,
    frames_left: u64,
    total_frames: Arc<AtomicU64>,
    nonzero_frames: Arc<AtomicU64>,
    callback_calls: Arc<AtomicU64>,
    peak_bits: Arc<AtomicU32>,
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
        let has_active_t = has_active.clone();
        let build_in_progress_t = build_in_progress.clone();
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
                )
            })
            .expect("failed to spawn audio host thread");
        AudioEngine {
            tx,
            events_rx,
            has_active,
            build_in_progress,
        }
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

    pub fn run_output_test(&self) -> Result<AudioDebugReport, String> {
        if !self.has_active.load(Ordering::Relaxed) {
            return Err("audio output not ready - set or restore an output device first".into());
        }
        let (reply, reply_rx) = crossbeam_channel::bounded(1);
        self.tx
            .send(EngineCommand::RunOutputTest { reply })
            .map_err(|_| "audio host thread is gone".to_string())?;
        match reply_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => {
                Err("audio output test timed out - callback may not be running".into())
            }
            Err(RecvTimeoutError::Disconnected) => Err("audio host thread did not reply".into()),
        }
    }
}

/// Live stream + the producer end of the callback's command queue.
struct ActiveStream {
    _stream: cpal::Stream,
    cmd_tx: rtrb::Producer<PlayCommand>,
    out_rate: u32,
    /// Incremented once per real-time callback invocation. Lets the host thread
    /// confirm the driver actually started firing callbacks — ASIO devices can
    /// open "dead" (build + play succeed, but no callback ever runs), and the
    /// only cure is tearing the stream down and rebuilding it.
    heartbeat: Arc<AtomicU64>,
    debug: StreamDebugInfo,
}

const DEFAULT_CROSSFADE_MS: u32 = 2000;

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

/// Brief (channels, sample_rate) summary for device-list rendering.
/// Prefers an f32 config (matches the default `build_output_stream` path) and
/// falls back to whatever the default config reports.
fn best_output_summary(device: &cpal::Device) -> Option<(usize, u32)> {
    if let Ok(configs) = device.supported_output_configs() {
        // Highest channel count + a reasonable default sample rate.
        let mut best: Option<(u16, u32, SampleFormat)> = None;
        for cfg in configs {
            let channels = cfg.channels();
            let sr = cfg
                .max_sample_rate()
                .0
                .min(48_000)
                .max(cfg.min_sample_rate().0);
            let fmt = cfg.sample_format();
            // Prefer f32 over i32 when both are available, otherwise prefer
            // higher channel count.
            let candidate = (channels, sr, fmt);
            best = Some(match best {
                None => candidate,
                Some(cur) => {
                    let cur_f32 = matches!(cur.2, SampleFormat::F32);
                    let new_f32 = matches!(fmt, SampleFormat::F32);
                    if new_f32 && !cur_f32 {
                        candidate
                    } else if !new_f32 && cur_f32 {
                        cur
                    } else if channels > cur.0 {
                        candidate
                    } else {
                        cur
                    }
                }
            });
        }
        if let Some((ch, sr, _)) = best {
            return Some((ch as usize, sr));
        }
    }
    let cfg = device.default_output_config().ok()?;
    Some((cfg.channels() as usize, cfg.sample_rate().0))
}

/// Fade-in used when a pad is auto-restored onto a freshly-rebuilt stream
/// (device or channel switch). Deliberately short — the device reopen already
/// introduced a brief gap, so we want the pad back quickly rather than easing
/// it in over the full crossfade duration.
const RESTORE_FADE_MS: u32 = 200;

/// Brief pause after releasing the old stream before opening the new one. Some
/// ASIO drivers don't fully relinquish the device synchronously when their
/// stream is dropped; this gives the driver a moment to settle so the re-open
/// lands cleanly instead of racing a half-closed driver. Only applied when we
/// actually dropped a prior stream, so a first-time open stays instant.
const ASIO_RELEASE_SETTLE: Duration = Duration::from_millis(75);

/// How long to let a *cold* ASIO stream actually run before tearing it down to
/// prime the driver (see the cold-open prime in `build_stream_live`). The manual
/// cure — "change a channel, then change back" — leaves the first stream running
/// for a beat before the reopen, and some virtual ASIO drivers (notably
/// Voicemeeter's Virtual ASIO) only start routing audio on a reopen that follows
/// a stream that was genuinely running. Reproduce that dwell time here.
const COLD_OPEN_WARMUP: Duration = Duration::from_millis(600);

/// Settle after releasing the warmed-up cold stream, before the priming reopen.
/// Longer than `ASIO_RELEASE_SETTLE` because virtual ASIO drivers can take a
/// little longer to fully relinquish the device than hardware ones.
const COLD_OPEN_REOPEN_SETTLE: Duration = Duration::from_millis(250);

/// Spawn a decoder for `path`, push it onto the active stream as a fading-in
/// pad voice, and start a watcher tied to a fresh pad generation. Shared by the
/// `Play` command and the auto-restore that runs after a stream rebuild so both
/// paths produce identical voice + watcher wiring.
fn start_pad_voice(
    act: &mut ActiveStream,
    path: PathBuf,
    fade_in_ms: u32,
    pad_generation: &Arc<AtomicU64>,
    notify_tx: &Sender<HostNotify>,
) {
    let dec = decode::spawn(path, act.out_rate, true, false);
    let voice = Voice {
        consumer: dec.consumer,
        stop: dec.stop,
        ended: dec.ended.clone(),
        bus: VoiceBus::Pad,
        gain: 0.0,
        target: 1.0,
        step: fade_step(fade_in_ms, act.out_rate),
        remove_when_silent: false,
    };
    let _ = act.cmd_tx.push(PlayCommand::Crossfade(voice));
    // Bump the pad generation, then spawn a watcher tied to this generation. If
    // the decoder later exits for any reason — natural crossfade-stop, explicit
    // Stop, or an error mid-set — the host will see NotifyPadEnded. The host
    // only emits EngineEvent::PadEnded when the generation still matches: a
    // crossfade or Stop bumps the generation first, so those legitimate exits
    // stay quiet.
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
            // Skip post if the pad has already been replaced (saves a no-op
            // trip through the host loop).
            if watcher_gen_arc.load(Ordering::Relaxed) == my_gen {
                let _ = watcher_notify.send(HostNotify::PadEnded(my_gen));
            }
        })
        .ok();
}

fn host_thread(
    rx: Receiver<EngineCommand>,
    notify_rx: Receiver<HostNotify>,
    notify_tx: Sender<HostNotify>,
    events: crossbeam_channel::Sender<EngineEvent>,
    has_active: Arc<AtomicBool>,
    build_in_progress: Arc<AtomicBool>,
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
    // Path of the pad currently playing (looping), if any. Tracked so that when
    // the stream is rebuilt for a device/channel switch — which drops the old
    // stream and every voice it carried — we can re-spawn the same pad on the
    // new stream and playback simply continues instead of going silent.
    let mut current_pad: Option<PathBuf> = None;
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
                    // Release the current stream BEFORE opening the next one.
                    // ASIO drivers are single-client: holding the old stream
                    // open while building the new one makes the re-open produce
                    // no audio, which is why a channel/device change needed a
                    // toggle away-and-back to come alive. Dropping first frees
                    // the driver so the new stream opens hot and outputs
                    // immediately. Trade-off: if the new open fails we no longer
                    // fall back to the old stream — but a silent "switched but
                    // dead" device is exactly the bug we're fixing, and the
                    // error is surfaced to the UI either way.
                    let had_stream = active.take().is_some();
                    has_active.store(false, Ordering::Relaxed);
                    if had_stream {
                        std::thread::sleep(ASIO_RELEASE_SETTLE);
                    }
                    build_in_progress.store(true, Ordering::Relaxed);
                    // `!had_stream` == cold open: no live stream preceded this,
                    // so the new one must be primed (see build_stream_live).
                    let result = build_stream_live(
                        &host,
                        &device,
                        channels,
                        master,
                        cue_volume,
                        click_init,
                        !had_stream,
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
                            active = Some(stream);
                            has_active.store(true, Ordering::Relaxed);
                            // Re-spawn the pad that was playing before the
                            // rebuild so a device/channel switch is seamless —
                            // the user doesn't have to manually re-trigger it.
                            if let (Some(act), Some(path)) = (active.as_mut(), current_pad.clone()) {
                                start_pad_voice(
                                    act,
                                    path,
                                    RESTORE_FADE_MS,
                                    &pad_generation,
                                    &notify_tx,
                                );
                            }
                            let _ = reply.send(Ok(()));
                        }
                        Err(e) => {
                            active = None;
                            has_active.store(false, Ordering::Relaxed);
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
                    let next = ChannelLayout {
                        click: channels,
                        ..last_channels
                    };
                    // Release the current stream BEFORE opening the next one.
                    // ASIO drivers are single-client: holding the old stream
                    // open while building the new one makes the re-open produce
                    // no audio, which is why a channel/device change needed a
                    // toggle away-and-back to come alive. Dropping first frees
                    // the driver so the new stream opens hot and outputs
                    // immediately. Trade-off: if the new open fails we no longer
                    // fall back to the old stream — but a silent "switched but
                    // dead" device is exactly the bug we're fixing, and the
                    // error is surfaced to the UI either way.
                    let had_stream = active.take().is_some();
                    has_active.store(false, Ordering::Relaxed);
                    if had_stream {
                        std::thread::sleep(ASIO_RELEASE_SETTLE);
                    }
                    build_in_progress.store(true, Ordering::Relaxed);
                    let result = build_stream_live(
                        &last_host,
                        &last_device,
                        next,
                        master,
                        cue_volume,
                        click_init,
                        !had_stream,
                    );
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
                            active = Some(stream);
                            has_active.store(true, Ordering::Relaxed);
                            // Re-spawn the pad that was playing before the
                            // rebuild so a device/channel switch is seamless —
                            // the user doesn't have to manually re-trigger it.
                            if let (Some(act), Some(path)) = (active.as_mut(), current_pad.clone()) {
                                start_pad_voice(
                                    act,
                                    path,
                                    RESTORE_FADE_MS,
                                    &pad_generation,
                                    &notify_tx,
                                );
                            }
                            let _ = reply.send(Ok(()));
                        }
                        Err(e) => {
                            active = None;
                            has_active.store(false, Ordering::Relaxed);
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
                    let next = ChannelLayout {
                        cue: channels,
                        ..last_channels
                    };
                    // Release the current stream BEFORE opening the next one.
                    // ASIO drivers are single-client: holding the old stream
                    // open while building the new one makes the re-open produce
                    // no audio, which is why a channel/device change needed a
                    // toggle away-and-back to come alive. Dropping first frees
                    // the driver so the new stream opens hot and outputs
                    // immediately. Trade-off: if the new open fails we no longer
                    // fall back to the old stream — but a silent "switched but
                    // dead" device is exactly the bug we're fixing, and the
                    // error is surfaced to the UI either way.
                    let had_stream = active.take().is_some();
                    has_active.store(false, Ordering::Relaxed);
                    if had_stream {
                        std::thread::sleep(ASIO_RELEASE_SETTLE);
                    }
                    build_in_progress.store(true, Ordering::Relaxed);
                    let result = build_stream_live(
                        &last_host,
                        &last_device,
                        next,
                        master,
                        cue_volume,
                        click_init,
                        !had_stream,
                    );
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
                            active = Some(stream);
                            has_active.store(true, Ordering::Relaxed);
                            // Re-spawn the pad that was playing before the
                            // rebuild so a device/channel switch is seamless —
                            // the user doesn't have to manually re-trigger it.
                            if let (Some(act), Some(path)) = (active.as_mut(), current_pad.clone()) {
                                start_pad_voice(
                                    act,
                                    path,
                                    RESTORE_FADE_MS,
                                    &pad_generation,
                                    &notify_tx,
                                );
                            }
                            let _ = reply.send(Ok(()));
                        }
                        Err(e) => {
                            active = None;
                            has_active.store(false, Ordering::Relaxed);
                            last_host.clear();
                            last_device.clear();
                            let _ = reply.send(Err(e));
                        }
                    }
                }
                EngineCommand::Play(path) => {
                    if let Some(act) = active.as_mut() {
                        // Remember the pad so a later device/channel switch can
                        // re-spawn it on the rebuilt stream and keep it playing.
                        current_pad = Some(path.clone());
                        start_pad_voice(act, path, crossfade_ms, &pad_generation, &notify_tx);
                    } else {
                        eprintln!("[audio] Play ignored: no output device configured");
                    }
                }
                EngineCommand::Stop => {
                    // User stopped the pad — don't auto-restore it on the next
                    // stream rebuild.
                    current_pad = None;
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
                        let _ = act
                            .cmd_tx
                            .push(PlayCommand::SetClickDuckActive(pref && cue_active));
                    }
                }
                EngineCommand::RunOutputTest { reply } => {
                    let Some(act) = active.as_mut() else {
                        let _ = reply.send(Err(
                            "audio output not ready - set an output device first".into(),
                        ));
                        continue;
                    };

                    let frames = (act.out_rate as f32 * 1.5) as u64;
                    let callback_calls = Arc::new(AtomicU64::new(0));
                    let total_frames = Arc::new(AtomicU64::new(0));
                    let nonzero_frames = Arc::new(AtomicU64::new(0));
                    let peak_bits = Arc::new(AtomicU32::new(0.0f32.to_bits()));
                    let tone = DiagnosticTone {
                        phase: 0.0,
                        phase_inc: std::f32::consts::TAU * 440.0 / act.out_rate as f32,
                        frames_left: frames,
                        total_frames: total_frames.clone(),
                        nonzero_frames: nonzero_frames.clone(),
                        callback_calls: callback_calls.clone(),
                        peak_bits: peak_bits.clone(),
                    };

                    if act
                        .cmd_tx
                        .push(PlayCommand::StartDiagnosticTone(tone))
                        .is_err()
                    {
                        let _ = reply.send(Err("audio callback command queue is full".into()));
                        continue;
                    }

                    let info = act.debug.clone();
                    std::thread::Builder::new()
                        .name("audio-debug-report".into())
                        .spawn(move || {
                            std::thread::sleep(Duration::from_millis(1800));
                            let report = AudioDebugReport {
                                host: info.host,
                                device: info.device,
                                sample_format: info.sample_format,
                                sample_rate: info.sample_rate,
                                channels: info.channels,
                                pad_channels: info.pad_channels,
                                callback_calls: callback_calls.load(Ordering::Relaxed),
                                frames_written: total_frames.load(Ordering::Relaxed),
                                nonzero_frames: nonzero_frames.load(Ordering::Relaxed),
                                peak: f32::from_bits(peak_bits.load(Ordering::Relaxed)),
                            };
                            let _ = reply.send(Ok(report));
                        })
                        .ok();
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
                    // The pad is no longer playing (file moved, share dropped),
                    // so drop it from auto-restore — re-spawning on the next
                    // device switch would just fail the same way.
                    current_pad = None;
                    let _ = events.send(EngineEvent::PadEnded);
                }
            }
        }
    }
}

/// Pick a usable output config. ASIO's `supported_output_configs()` in cpal
/// synthesizes one config for every channel count from 1..=device outputs, so
/// grabbing the first matching format can accidentally open a 1-channel stream
/// on a 32-out mixer. Prefer the driver's current/default config, which is what
/// DAWs typically use and what fixed-rate devices like the DL32S report as
/// 32ch @ 48 kHz.
fn pick_supported(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
    let fallback = device
        .default_output_config()
        .map_err(|e| format!("default config: {e}"))?;
    if matches!(
        fallback.sample_format(),
        SampleFormat::F32 | SampleFormat::I32
    ) {
        return Ok(fallback);
    }

    let configs: Vec<_> = device
        .supported_output_configs()
        .map_err(|e| format!("supported configs: {e}"))?
        .collect();

    // Fallback for hosts whose default config is odd but supported ranges are
    // richer. Prefer the most channels, then 48 kHz if present, then f32/i32.
    let mut candidates: Vec<_> = configs
        .iter()
        .filter(|c| matches!(c.sample_format(), SampleFormat::F32 | SampleFormat::I32))
        .map(|c| c.with_max_sample_rate())
        .collect();
    candidates.sort_by_key(|c| {
        let fmt_score = match c.sample_format() {
            SampleFormat::F32 => 2,
            SampleFormat::I32 => 1,
            _ => 0,
        };
        let rate_score = u8::from(c.sample_rate().0 == 48_000);
        (c.channels(), rate_score, fmt_score)
    });

    if let Some(c) = candidates.pop() {
        Ok(c)
    } else {
        Err(format!(
            "device sample format {:?} is not supported (need f32 or i32)",
            fallback.sample_format()
        ))
    }
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
/// driver during a cold-open prime (see `build_stream_live`): every bus
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

fn build_stream(
    host_label: &str,
    device_name: &str,
    channels: ChannelLayout,
    master: f32,
    cue_volume: f32,
    click_init: ClickInit,
) -> Result<ActiveStream, String> {
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
    let (pad_l, pad_r) = channels.pad;
    let (click_l, click_r) = channels.click;
    let (cue_l, cue_r) = channels.cue;

    if pad_l >= total_channels || pad_r >= total_channels {
        return Err(format!(
            "pad channel pair ({pad_l},{pad_r}) out of range; device has {total_channels} channels"
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
    eprintln!(
        "[audio] opening {host_label} '{device_name}' format={sample_format:?} rate={out_rate} channels={total_channels} pad=({pad_l},{pad_r}) click=({click_l},{click_r}) cue=({cue_l},{cue_r})"
    );

    // Lock-free queue: host thread → real-time callback.
    let (cmd_tx, cmd_rx) = RingBuffer::<PlayCommand>::new(64);

    // Callback liveness counter (see `ActiveStream::heartbeat`).
    let heartbeat = Arc::new(AtomicU64::new(0));

    let err_fn = |err| eprintln!("[audio] stream error: {err}");
    let stream = match sample_format {
        SampleFormat::F32 => {
            let cb = build_callback_f32(layout, master, cue_volume, click_gen, cmd_rx, heartbeat.clone());
            device
                .build_output_stream(&config, cb, err_fn, None)
                .map_err(|e| format!("build output stream (f32): {e}"))?
        }
        SampleFormat::I32 => {
            let cb = build_callback_i32(layout, master, cue_volume, click_gen, cmd_rx, heartbeat.clone());
            device
                .build_output_stream(&config, cb, err_fn, None)
                .map_err(|e| format!("build output stream (i32): {e}"))?
        }
        other => return Err(format!("unsupported sample format {other:?}")),
    };

    stream.play().map_err(|e| format!("start stream: {e}"))?;

    Ok(ActiveStream {
        _stream: stream,
        cmd_tx,
        out_rate,
        heartbeat,
        debug: StreamDebugInfo {
            host: host_label.to_string(),
            device: device_name.to_string(),
            sample_format: format!("{sample_format:?}"),
            sample_rate: out_rate,
            channels: total_channels,
            pad_channels: channels.pad,
        },
    })
}

/// How long to wait for the freshly-built stream's callback to fire at least
/// once before declaring it dead. A healthy driver fires within a few ms; ASIO
/// devices that opened dead never fire at all.
const LIVENESS_POLL_STEP: Duration = Duration::from_millis(25);
const LIVENESS_POLL_STEPS: u32 = 16; // 16 × 25 ms = up to 400 ms

/// Settle delays (ms) before each successive reopen when a stream comes up
/// dead. Increasing backoff: some ASIO drivers wake on a near-immediate
/// teardown+rebuild, others only relinquish/restart the device after real
/// wall-clock time has passed since the failed open. The schedule's length also
/// sets how many reopen attempts we make on top of the initial open.
const DEAD_STREAM_REOPEN_BACKOFF_MS: [u64; 4] = [150, 400, 900, 1600];

/// Wait until the stream's callback has fired (driver is genuinely running), or
/// give up after the poll budget. Returns true if at least one callback ran.
fn stream_is_live(act: &ActiveStream) -> bool {
    let start = act.heartbeat.load(Ordering::Relaxed);
    for _ in 0..LIVENESS_POLL_STEPS {
        std::thread::sleep(LIVENESS_POLL_STEP);
        if act.heartbeat.load(Ordering::Relaxed) > start {
            return true;
        }
    }
    false
}

/// Build a stream and confirm the driver actually started firing callbacks,
/// reopening if it came up dead. This is the generic cure for the "no signal
/// until I change a channel" symptom: an ASIO device can accept the stream and
/// report `play()` success yet never run a single callback, so audio is silent
/// until the stream is torn down and rebuilt. Each retry releases the prior
/// attempt (single-client drivers can't be double-opened) and waits the ASIO
/// settle interval before reopening.
///
/// `cold_open` is true when this is the first stream of the session for the
/// driver — i.e. the host thread held no live stream beforehand (boot
/// auto-restore, or selecting a device when none was open). See the cold-open
/// prime below for why that case needs special handling.
fn build_stream_live(
    host_label: &str,
    device_name: &str,
    channels: ChannelLayout,
    master: f32,
    cue_volume: f32,
    click_init: ClickInit,
    cold_open: bool,
) -> Result<ActiveStream, String> {
    let mut stream = build_stream(host_label, device_name, channels, master, cue_volume, click_init)?;

    // A *cold* first open of an ASIO driver is the unreliable one. The driver
    // accepts the stream and fires its data callback (so `heartbeat` ticks and
    // `stream_is_live` returns true), yet routes no audio to the hardware until
    // its buffers are torn down and re-prepared — it comes up "alive-but-silent".
    // That is the "no signal until I change a channel" bug: a manual channel
    // change works only because it drops a prior running stream and reopens, so
    // its open is the *second* (warm) one. On boot there is no prior stream, so
    // the cold open is trusted as-is and stays silent.
    //
    // Replicate, in code, the exact sequence that works by hand ("go to channel
    // 1, then back to the appropriate channel"). A same-config reopen is not
    // enough for Voicemeeter's Virtual ASIO — the routing must actually change
    // and change back:
    //   1. let the initial (silent) cold stream run so the driver fully starts,
    //   2. reopen on a decoy routing, let it run,
    //   3. reopen on the real routing — the open that finally routes audio.
    // Nothing is audible during 1–2: at a cold open no pad/cue voice exists yet
    // and the click boots disabled. Gated on a genuinely cold open AND ASIO, so
    // WASAPI is untouched and a warm channel change never pays this cost.
    if cold_open && host_label.eq_ignore_ascii_case("ASIO") {
        let decoy = decoy_layout(channels);
        eprintln!(
            "[audio] priming cold ASIO open of '{device_name}': real -> decoy {decoy:?} -> real"
        );
        let _ = stream_is_live(&stream);
        std::thread::sleep(COLD_OPEN_WARMUP);
        drop(stream);
        std::thread::sleep(COLD_OPEN_REOPEN_SETTLE);

        // The decoy build can legitimately fail (e.g. a 1-channel device); if it
        // does, skip the wiggle and just reopen on the real layout below.
        match build_stream(host_label, device_name, decoy, master, cue_volume, click_init) {
            Ok(decoy_stream) => {
                let _ = stream_is_live(&decoy_stream);
                std::thread::sleep(COLD_OPEN_WARMUP);
                drop(decoy_stream);
                std::thread::sleep(COLD_OPEN_REOPEN_SETTLE);
            }
            Err(e) => eprintln!("[audio] cold-open decoy reopen skipped: {e}"),
        }

        stream =
            build_stream(host_label, device_name, channels, master, cue_volume, click_init)?;
    }

    if stream_is_live(&stream) {
        return Ok(stream);
    }
    let attempts = DEAD_STREAM_REOPEN_BACKOFF_MS.len();
    for (i, &settle_ms) in DEAD_STREAM_REOPEN_BACKOFF_MS.iter().enumerate() {
        eprintln!(
            "[audio] '{device_name}' opened but no callbacks fired — reopening after {settle_ms}ms (attempt {}/{attempts})",
            i + 1
        );
        // Drop releases the dead stream (and its driver claim) before we retry.
        drop(stream);
        std::thread::sleep(Duration::from_millis(settle_ms));
        stream = build_stream(host_label, device_name, channels, master, cue_volume, click_init)?;
        if stream_is_live(&stream) {
            eprintln!("[audio] '{device_name}' is now live after reopen");
            return Ok(stream);
        }
    }
    // Still no heartbeat after retries. Hand back the last stream anyway rather
    // than dropping audio entirely — better a possibly-silent stream the user
    // can re-poke than a hard failure, and a few drivers under-report.
    eprintln!("[audio] '{device_name}' still reported no callbacks after reopen attempts");
    Ok(stream)
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
    osc_phase: f32, // radians, wraps at 2π
    osc_freq: f32,  // Hz
    osc_env: f32,   // current envelope, decays per sample
    osc_decay: f32, // per-sample env multiplier
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

        let s =
            self.osc_phase.sin() * self.osc_env * self.volume * self.enable_ramp * self.duck_ramp;
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
fn record_peak(bits: &AtomicU32, value: f32) {
    let value = value.abs();
    let mut cur = bits.load(Ordering::Relaxed);
    while value > f32::from_bits(cur) {
        match bits.compare_exchange_weak(cur, value.to_bits(), Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(next) => cur = next,
        }
    }
}

#[inline]
fn add_diagnostic_tone(mix: &mut MixedFrame, diag: &mut Option<DiagnosticTone>) {
    let Some(tone) = diag.as_mut() else {
        return;
    };
    if tone.frames_left == 0 {
        *diag = None;
        return;
    }

    let value = tone.phase.sin() * 0.35;
    tone.phase += tone.phase_inc;
    if tone.phase >= std::f32::consts::TAU {
        tone.phase -= std::f32::consts::TAU;
    }
    tone.frames_left -= 1;

    mix.pad_l += value;
    mix.pad_r += value;
    tone.total_frames.fetch_add(1, Ordering::Relaxed);
    if value != 0.0 {
        tone.nonzero_frames.fetch_add(1, Ordering::Relaxed);
    }
    record_peak(&tone.peak_bits, value);
}

#[inline]
fn drain_commands(
    voices: &mut Vec<Voice>,
    master: &mut f32,
    cue_volume: &mut f32,
    click: &mut ClickGen,
    diagnostic: &mut Option<DiagnosticTone>,
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
            PlayCommand::StartDiagnosticTone(tone) => *diagnostic = Some(tone),
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
    heartbeat: Arc<AtomicU64>,
) -> impl FnMut(&mut [f32], &cpal::OutputCallbackInfo) + Send + 'static {
    let mut voices: Vec<Voice> = Vec::with_capacity(4);
    let mut master = master_init;
    let mut cue_volume = cue_volume_init;
    let mut click = click_init;
    let mut diagnostic: Option<DiagnosticTone> = None;

    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        heartbeat.fetch_add(1, Ordering::Relaxed);
        drain_commands(
            &mut voices,
            &mut master,
            &mut cue_volume,
            &mut click,
            &mut diagnostic,
            &mut cmd_rx,
        );
        if let Some(tone) = diagnostic.as_ref() {
            tone.callback_calls.fetch_add(1, Ordering::Relaxed);
        }

        for frame in data.chunks_mut(layout.total) {
            let mut m = mix_one_frame(&mut voices, master, cue_volume);
            add_diagnostic_tone(&mut m, &mut diagnostic);
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
    heartbeat: Arc<AtomicU64>,
) -> impl FnMut(&mut [i32], &cpal::OutputCallbackInfo) + Send + 'static {
    let mut voices: Vec<Voice> = Vec::with_capacity(4);
    let mut master = master_init;
    let mut cue_volume = cue_volume_init;
    let mut click = click_init;
    let mut diagnostic: Option<DiagnosticTone> = None;

    // i32 full-scale. Headroom of 1 sample on the negative side avoids wrap.
    const SCALE: f32 = 2_147_483_520.0;

    // Scratch buffer reused per frame so we can compose the f32 sum and then
    // convert; sized for the worst-case channel count we'll ever see.
    let mut scratch = [0.0f32; 64];

    move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
        heartbeat.fetch_add(1, Ordering::Relaxed);
        drain_commands(
            &mut voices,
            &mut master,
            &mut cue_volume,
            &mut click,
            &mut diagnostic,
            &mut cmd_rx,
        );
        if let Some(tone) = diagnostic.as_ref() {
            tone.callback_calls.fetch_add(1, Ordering::Relaxed);
        }

        for frame in data.chunks_mut(layout.total) {
            let mut m = mix_one_frame(&mut voices, master, cue_volume);
            add_diagnostic_tone(&mut m, &mut diagnostic);
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
