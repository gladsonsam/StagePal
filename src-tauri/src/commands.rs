//! Playback control. `*_logic` free functions are shared by the Tauri commands
//! (desktop) and axum handlers (phone remote). Each mutation persists and
//! broadcasts NowPlaying.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::audio::{AudioDebugReport, AudioEngine, DeviceInfo};
use crate::cues::{self, Synthesizer, VoiceInfo};
use crate::library;
use crate::model::{now_unix_ms, Key, NowPlaying, Preset, QuickCue, Settings};
use crate::state::CoreState;

/// TTS synthesizer as Tauri state. Trait-object so the backend can be swapped.
pub struct CueSynth(pub Box<dyn Synthesizer>);

// --- Shared logic (Tauri commands + web server) ---

/// Emit NowPlaying to the desktop window and the broadcast bus.
pub fn emit_now(app: &AppHandle, core: &CoreState) {
    let now = core.snapshot();
    let _ = app.emit("now-playing", now.clone());
    let _ = core.tx.send(now);
}

fn resolve_file(settings: &Settings, key: Key) -> Option<PathBuf> {
    settings
        .active_preset()
        .and_then(|p| p.files.get(&key).cloned())
}

pub fn set_volume_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
    volume: f32,
) -> Result<(), String> {
    engine.set_volume(volume)?;
    core.settings.lock().unwrap().master_volume = volume;
    core.now.lock().unwrap().volume = volume;
    emit_now(app, core);
    core.save()
}

pub fn play_key_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
    synth: &dyn Synthesizer,
    key: &str,
) -> Result<(), String> {
    let k = Key::parse(key).ok_or_else(|| format!("unknown key '{key}'"))?;

    // Re-pressing the playing key deselects it (fade out and stop).
    let already_playing = {
        let n = core.now.lock().unwrap();
        n.playing && n.key == Some(k)
    };
    if already_playing {
        return stop_logic(app, core, engine);
    }

    let (path, active, speak_key) = {
        let s = core.settings.lock().unwrap();
        (
            resolve_file(&s, k),
            s.active_preset.clone(),
            s.cues.speak_key_on_change,
        )
    };

    match path {
        Some(p) => engine.play(p)?,
        None => engine.play_synth(k.freq())?,
    };
    {
        let mut n = core.now.lock().unwrap();
        n.key = Some(k);
        n.playing = true;
        n.preset = active;
    }
    emit_now(app, core);

    // Best-effort auto-announce; a synth hiccup mustn't fail the press. Fixed
    // slow rate (-3) so a single letter gets enough airtime.
    if speak_key {
        let text = format!("Key of {}", k.spoken());
        if let Err(e) = cue_speak_logic(app, core, engine, synth, &text, None, Some(-3)) {
            eprintln!("[cue] auto key-announcement failed: {e}");
        }
    }
    Ok(())
}

pub fn stop_logic(app: &AppHandle, core: &CoreState, engine: &AudioEngine) -> Result<(), String> {
    engine.stop()?;
    {
        let mut n = core.now.lock().unwrap();
        n.playing = false;
        n.key = None;
    }
    emit_now(app, core);
    Ok(())
}

pub fn set_preset_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
    id: &str,
) -> Result<(), String> {
    {
        let mut s = core.settings.lock().unwrap();
        if !s.presets.iter().any(|p| p.id == id) {
            return Err(format!("no preset '{id}'"));
        }
        s.active_preset = Some(id.to_string());
    }
    core.save()?;

    // If a key is playing, crossfade into the same key of the new preset —
    // falling through to the generated pad when the new bank has no file.
    let current_key = core.now.lock().unwrap().key;
    if let Some(k) = current_key {
        let path = {
            let s = core.settings.lock().unwrap();
            resolve_file(&s, k)
        };
        match path {
            Some(p) => engine.play(p)?,
            None => engine.play_synth(k.freq())?,
        }
    }
    core.now.lock().unwrap().preset = Some(id.to_string());
    emit_now(app, core);
    Ok(())
}

pub fn set_crossfade_logic(core: &CoreState, engine: &AudioEngine, ms: u32) -> Result<(), String> {
    let ms = ms.clamp(100, 15_000);
    engine.set_crossfade(ms)?;
    core.settings.lock().unwrap().crossfade_ms = ms;
    core.save()
}

/// Assign (move) a file to a key. Any file already on the key returns to the
/// unmapped pile; the incoming file is moved, not duplicated.
pub fn assign_key_logic(
    core: &CoreState,
    preset_id: &str,
    key: Key,
    path: PathBuf,
) -> Result<(), String> {
    {
        let mut s = core.settings.lock().unwrap();
        let preset = s
            .presets
            .iter_mut()
            .find(|p| p.id == preset_id)
            .ok_or_else(|| format!("no preset '{preset_id}'"))?;

        // Detach the incoming file from wherever it currently lives.
        preset.unmapped.retain(|p| p != &path);
        preset.files.retain(|_, v| v != &path);

        // Park the key's previous file back in the unmapped pile.
        if let Some(prev) = preset.files.insert(key, path) {
            if !preset.unmapped.contains(&prev) {
                preset.unmapped.push(prev);
            }
        }
        preset.unmapped.sort();
    }
    core.save()
}

/// Unassign a key, returning its file to the unmapped pile.
pub fn clear_key_logic(core: &CoreState, preset_id: &str, key: Key) -> Result<(), String> {
    {
        let mut s = core.settings.lock().unwrap();
        let preset = s
            .presets
            .iter_mut()
            .find(|p| p.id == preset_id)
            .ok_or_else(|| format!("no preset '{preset_id}'"))?;
        if let Some(path) = preset.files.remove(&key) {
            if !preset.unmapped.contains(&path) {
                preset.unmapped.push(path);
                preset.unmapped.sort();
            }
        }
    }
    core.save()
}

/// Initial payload for a freshly-loaded phone remote.
#[derive(Serialize)]
pub struct PresetBrief {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct Info {
    pub keys: Vec<&'static str>,
    pub presets: Vec<PresetBrief>,
    pub active_preset: Option<String>,
    pub mapped_keys: Vec<String>,
    /// Key → file name (no path), so the phone can label each pad.
    pub files: std::collections::HashMap<String, String>,
    /// Saved quick cues, so the phone renders its grid from one fetch.
    pub cues_quick: Vec<QuickCue>,
    pub now: NowPlaying,
}

pub fn build_info(core: &CoreState) -> Info {
    let now = core.snapshot();
    let s = core.settings.lock().unwrap();
    let presets = s
        .presets
        .iter()
        .map(|p| PresetBrief {
            id: p.id.clone(),
            name: p.name.clone(),
        })
        .collect();
    let active = s.active_preset();
    let mapped_keys = active
        .map(|p| p.files.keys().map(|k| k.as_str().to_string()).collect())
        .unwrap_or_default();
    let files = active
        .map(|p| {
            p.files
                .iter()
                .map(|(k, path)| {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (k.as_str().to_string(), name)
                })
                .collect()
        })
        .unwrap_or_default();
    let cues_quick = s.cues.quick.clone();
    Info {
        keys: Key::ALL.iter().map(|k| k.as_str()).collect(),
        presets,
        active_preset: s.active_preset.clone(),
        mapped_keys,
        files,
        cues_quick,
        now,
    }
}

// --- Tauri command wrappers (desktop UI) ---

#[tauri::command]
pub fn get_settings(core: State<'_, CoreState>) -> Settings {
    core.settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn get_state(core: State<'_, CoreState>) -> NowPlaying {
    core.snapshot()
}

#[tauri::command]
pub fn list_audio_devices() -> Vec<DeviceInfo> {
    AudioEngine::list_devices()
}

#[tauri::command]
pub fn set_audio_output(
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    host: String,
    device: String,
    channel_left: usize,
    channel_right: usize,
) -> Result<(), String> {
    // Resolve pad/click/cue routing. Switching devices restores that device's
    // saved channels (else requested pad pair + current click/cue); editing
    // the current device honours the requested pad pair, leaving click/cue.
    let ((pad_l, pad_r), (click_l, click_r), (cue_l, cue_r)) = {
        let s = core.settings.lock().unwrap();
        let switching =
            s.output_host != host || s.output_device.as_deref() != Some(device.as_str());
        let saved = if switching {
            s.device_routes.get(&Settings::device_key(&host, &device)).copied()
        } else {
            None
        };
        match saved {
            Some(r) => (
                (r.pad_left, r.pad_right),
                (r.click_left, r.click_right),
                (r.cue_left, r.cue_right),
            ),
            None => (
                (channel_left, channel_right),
                (s.click.channel_left, s.click.channel_right),
                (s.cues.channel_left, s.cues.channel_right),
            ),
        }
    };
    engine.set_output(
        &host,
        &device,
        (pad_l, pad_r),
        (click_l, click_r),
        (cue_l, cue_r),
    )?;
    {
        let mut s = core.settings.lock().unwrap();
        s.output_host = host;
        s.output_device = Some(device);
        s.channel_left = pad_l;
        s.channel_right = pad_r;
        s.click.channel_left = click_l;
        s.click.channel_right = click_r;
        s.cues.channel_left = cue_l;
        s.cues.channel_right = cue_r;
        // Remember this routing so reselecting the device restores it.
        s.remember_current_route();
    }
    core.save()
}

#[tauri::command]
pub fn run_audio_output_test(engine: State<'_, AudioEngine>) -> Result<AudioDebugReport, String> {
    engine.run_output_test()
}

#[tauri::command]
pub fn set_volume(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    volume: f32,
) -> Result<(), String> {
    set_volume_logic(&app, core.inner(), engine.inner(), volume)
}

#[tauri::command]
pub fn scan_library(
    core: State<'_, CoreState>,
    folder: String,
    name: Option<String>,
) -> Result<Preset, String> {
    // Preset id is the folder path. Scan runs OUTSIDE the Settings lock so
    // other commands aren't blocked; the merge happens under the lock so
    // concurrent edits during the scan aren't clobbered.
    let folder_path = std::path::PathBuf::from(&folder);
    let id = folder_path.to_string_lossy().to_string();

    let preset_name = {
        let s = core.settings.lock().unwrap();
        name.clone().or_else(|| {
            s.presets
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone())
        })
    };
    let fresh = library::scan_preset(&folder_path, preset_name)?;

    let preset = {
        let mut s = core.settings.lock().unwrap();
        let merged = match s.presets.iter().find(|p| p.id == id) {
            // Merge against the latest snapshot so mid-scan edits survive.
            Some(current) => library::merge_scan(current, fresh),
            None => fresh,
        };
        s.presets.retain(|p| p.id != merged.id);
        s.presets.push(merged.clone());
        // Auto-activate the first real folder a user imports (the active bank
        // otherwise defaults to the built-in generated-pads bank).
        let active_is_builtin = s
            .active_preset
            .as_deref()
            .map(|id| id == crate::model::BUILTIN_SYNTH_ID)
            .unwrap_or(true);
        if active_is_builtin {
            s.active_preset = Some(merged.id.clone());
        }
        merged
    };
    core.save()?;
    Ok(preset)
}

#[tauri::command]
pub fn assign_key(
    core: State<'_, CoreState>,
    id: String,
    key: String,
    path: String,
) -> Result<(), String> {
    let k = Key::parse(&key).ok_or_else(|| format!("unknown key '{key}'"))?;
    assign_key_logic(core.inner(), &id, k, PathBuf::from(path))
}

#[tauri::command]
pub fn clear_key(core: State<'_, CoreState>, id: String, key: String) -> Result<(), String> {
    let k = Key::parse(&key).ok_or_else(|| format!("unknown key '{key}'"))?;
    clear_key_logic(core.inner(), &id, k)
}

#[tauri::command]
pub fn rename_preset(core: State<'_, CoreState>, id: String, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("name cannot be empty".into());
    }
    {
        let mut s = core.settings.lock().unwrap();
        let preset = s
            .presets
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no preset '{id}'"))?;
        preset.name = name;
    }
    core.save()
}

#[tauri::command]
pub fn set_crossfade(
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    ms: u32,
) -> Result<(), String> {
    set_crossfade_logic(core.inner(), engine.inner(), ms)
}

#[tauri::command]
pub fn remove_preset(core: State<'_, CoreState>, id: String) -> Result<(), String> {
    if id == crate::model::BUILTIN_SYNTH_ID {
        return Err("the Generated Pads bank is built in and can't be removed".into());
    }
    {
        let mut s = core.settings.lock().unwrap();
        s.presets.retain(|p| p.id != id);
        if s.active_preset.as_deref() == Some(id.as_str()) {
            s.active_preset = s.presets.first().map(|p| p.id.clone());
        }
    }
    core.save()
}

#[tauri::command]
pub fn set_preset(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    id: String,
) -> Result<(), String> {
    set_preset_logic(&app, core.inner(), engine.inner(), &id)
}

#[tauri::command]
pub fn play_key(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    synth: State<'_, CueSynth>,
    key: String,
) -> Result<(), String> {
    play_key_logic(&app, core.inner(), engine.inner(), synth.0.as_ref(), &key)
}

#[tauri::command]
pub fn stop(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
) -> Result<(), String> {
    stop_logic(&app, core.inner(), engine.inner())
}

#[derive(Serialize)]
pub struct ServerUrl {
    pub ip: Option<String>,
    pub host: String,
    pub port: u16,
}

#[tauri::command]
pub fn server_url(core: State<'_, CoreState>) -> ServerUrl {
    let port = core.settings.lock().unwrap().server_port;
    ServerUrl {
        ip: crate::server::local_ipv4().map(|ip| ip.to_string()),
        host: crate::server::mdns_host(),
        port,
    }
}

// --- Click track ---

pub fn set_click_enabled_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
    enabled: bool,
) -> Result<(), String> {
    engine.set_click_enabled(enabled)?;
    {
        let mut n = core.now.lock().unwrap();
        n.click.enabled = enabled;
        n.click.started_at_ms = if enabled { Some(now_unix_ms()) } else { None };
    }
    emit_now(app, core);
    Ok(())
}

pub fn set_click_bpm_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
    bpm: f32,
) -> Result<(), String> {
    let bpm = bpm.clamp(20.0, 400.0);
    engine.set_click_bpm(bpm)?;
    {
        let mut s = core.settings.lock().unwrap();
        s.click.bpm = bpm;
    }
    core.now.lock().unwrap().click.bpm = bpm;
    emit_now(app, core);
    core.save()
}

pub fn set_click_beats_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
    beats: u32,
) -> Result<(), String> {
    let beats = beats.clamp(1, 32);
    engine.set_click_beats(beats)?;
    {
        let mut s = core.settings.lock().unwrap();
        s.click.beats_per_bar = beats;
    }
    {
        let mut n = core.now.lock().unwrap();
        n.click.beats_per_bar = beats;
        // Realign so clients restart from beat 1 on the next tick.
        if n.click.enabled {
            n.click.started_at_ms = Some(now_unix_ms());
        }
    }
    emit_now(app, core);
    core.save()
}

pub fn set_click_accent_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
    accent: bool,
) -> Result<(), String> {
    engine.set_click_accent(accent)?;
    {
        let mut s = core.settings.lock().unwrap();
        s.click.accent = accent;
    }
    core.now.lock().unwrap().click.accent = accent;
    emit_now(app, core);
    core.save()
}

pub fn set_click_volume_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
    volume: f32,
) -> Result<(), String> {
    let volume = volume.clamp(0.0, 1.0);
    engine.set_click_volume(volume)?;
    {
        let mut s = core.settings.lock().unwrap();
        s.click.volume = volume;
    }
    core.now.lock().unwrap().click.volume = volume;
    emit_now(app, core);
    core.save()
}

pub fn set_click_channels_logic(
    core: &CoreState,
    engine: &AudioEngine,
    channel_left: usize,
    channel_right: usize,
) -> Result<(), String> {
    engine.set_click_channels((channel_left, channel_right))?;
    {
        let mut s = core.settings.lock().unwrap();
        s.click.channel_left = channel_left;
        s.click.channel_right = channel_right;
        s.remember_current_route();
    }
    core.save()
}

#[tauri::command]
pub fn set_click_enabled(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    enabled: bool,
) -> Result<(), String> {
    set_click_enabled_logic(&app, core.inner(), engine.inner(), enabled)
}

#[tauri::command]
pub fn set_click_bpm(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    bpm: f32,
) -> Result<(), String> {
    set_click_bpm_logic(&app, core.inner(), engine.inner(), bpm)
}

#[tauri::command]
pub fn set_click_beats(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    beats: u32,
) -> Result<(), String> {
    set_click_beats_logic(&app, core.inner(), engine.inner(), beats)
}

#[tauri::command]
pub fn set_click_accent(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    accent: bool,
) -> Result<(), String> {
    set_click_accent_logic(&app, core.inner(), engine.inner(), accent)
}

#[tauri::command]
pub fn set_click_volume(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    volume: f32,
) -> Result<(), String> {
    set_click_volume_logic(&app, core.inner(), engine.inner(), volume)
}

#[tauri::command]
pub fn set_click_channels(
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    channel_left: usize,
    channel_right: usize,
) -> Result<(), String> {
    set_click_channels_logic(core.inner(), engine.inner(), channel_left, channel_right)
}

// --- Cues (TTS) ---

/// Render text to a temp WAV and play it on the cue bus. Sets `speaking = true`
/// synchronously; the `false` flip arrives via the engine's `CueEnded` event.
/// `rate_override` bypasses the saved cue rate (used by auto key-announce).
pub fn cue_speak_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
    synth: &dyn Synthesizer,
    text: &str,
    label: Option<String>,
    rate_override: Option<i32>,
) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("nothing to speak".into());
    }
    let (voice, saved_rate) = {
        let s = core.settings.lock().unwrap();
        (s.cues.voice.clone(), s.cues.rate)
    };
    let rate = rate_override.unwrap_or(saved_rate).clamp(-10, 10);
    let out = cues::sapi_temp_wav_path();
    synth.synth_to_wav(trimmed, voice.as_deref(), rate, &out)?;
    engine.play_cue(out)?;
    {
        let mut n = core.now.lock().unwrap();
        n.cue.speaking = true;
        n.cue.label = label;
    }
    emit_now(app, core);
    Ok(())
}

pub fn cue_stop_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
) -> Result<(), String> {
    engine.stop_cue()?;
    {
        let mut n = core.now.lock().unwrap();
        n.cue.speaking = false;
        n.cue.label = None;
    }
    emit_now(app, core);
    Ok(())
}

pub fn cue_speak_quick_logic(
    app: &AppHandle,
    core: &CoreState,
    engine: &AudioEngine,
    synth: &dyn Synthesizer,
    id: &str,
) -> Result<(), String> {
    let cue = {
        let s = core.settings.lock().unwrap();
        s.cues
            .quick
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| format!("no quick cue '{id}'"))?
    };
    cue_speak_logic(app, core, engine, synth, &cue.text, Some(cue.label), None)
}

/// Short opaque id from the clock, stable across rename/edit.
fn new_cue_id() -> String {
    let n = now_unix_ms();
    format!("q-{n:x}")
}

pub fn cue_add_logic(core: &CoreState, label: String, text: String) -> Result<QuickCue, String> {
    let label = label.trim().to_string();
    if label.is_empty() {
        return Err("label cannot be empty".into());
    }
    let cue = QuickCue {
        id: new_cue_id(),
        label,
        text,
    };
    {
        let mut s = core.settings.lock().unwrap();
        s.cues.quick.push(cue.clone());
    }
    core.save()?;
    Ok(cue)
}

pub fn cue_update_logic(
    core: &CoreState,
    id: &str,
    label: String,
    text: String,
) -> Result<(), String> {
    let label = label.trim().to_string();
    if label.is_empty() {
        return Err("label cannot be empty".into());
    }
    {
        let mut s = core.settings.lock().unwrap();
        let c = s
            .cues
            .quick
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| format!("no quick cue '{id}'"))?;
        c.label = label;
        c.text = text;
    }
    core.save()
}

pub fn cue_remove_logic(core: &CoreState, id: &str) -> Result<(), String> {
    {
        let mut s = core.settings.lock().unwrap();
        s.cues.quick.retain(|c| c.id != id);
    }
    core.save()
}

/// Move cue `id` to `to_index` (clamped to the end).
pub fn cue_move_logic(core: &CoreState, id: &str, to_index: usize) -> Result<(), String> {
    {
        let mut s = core.settings.lock().unwrap();
        let Some(from) = s.cues.quick.iter().position(|c| c.id == id) else {
            return Err(format!("no quick cue '{id}'"));
        };
        let item = s.cues.quick.remove(from);
        let to = to_index.min(s.cues.quick.len());
        s.cues.quick.insert(to, item);
    }
    core.save()
}

pub fn set_cue_voice_logic(core: &CoreState, voice: Option<String>) -> Result<(), String> {
    {
        let mut s = core.settings.lock().unwrap();
        s.cues.voice = voice.filter(|v| !v.trim().is_empty());
    }
    core.save()
}

pub fn set_cue_rate_logic(core: &CoreState, rate: i32) -> Result<(), String> {
    {
        let mut s = core.settings.lock().unwrap();
        s.cues.rate = rate.clamp(-10, 10);
    }
    core.save()
}

pub fn set_cue_volume_logic(
    core: &CoreState,
    engine: &AudioEngine,
    volume: f32,
) -> Result<(), String> {
    let v = volume.clamp(0.0, 1.0);
    engine.set_cue_volume(v)?;
    {
        let mut s = core.settings.lock().unwrap();
        s.cues.volume = v;
    }
    core.save()
}

pub fn set_cue_channels_logic(
    core: &CoreState,
    engine: &AudioEngine,
    channel_left: usize,
    channel_right: usize,
) -> Result<(), String> {
    engine.set_cue_channels((channel_left, channel_right))?;
    {
        let mut s = core.settings.lock().unwrap();
        s.cues.channel_left = channel_left;
        s.cues.channel_right = channel_right;
        s.remember_current_route();
    }
    core.save()
}

pub fn set_cue_duck_click_logic(
    core: &CoreState,
    engine: &AudioEngine,
    duck: bool,
) -> Result<(), String> {
    engine.set_duck_click(duck)?;
    {
        let mut s = core.settings.lock().unwrap();
        s.cues.duck_click = duck;
    }
    core.save()
}

pub fn set_cue_speak_key_logic(core: &CoreState, enabled: bool) -> Result<(), String> {
    {
        let mut s = core.settings.lock().unwrap();
        s.cues.speak_key_on_change = enabled;
    }
    core.save()
}

#[tauri::command]
pub fn list_voices(synth: State<'_, CueSynth>) -> Result<Vec<VoiceInfo>, String> {
    synth.0.voices()
}

#[tauri::command]
pub fn cue_speak(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    synth: State<'_, CueSynth>,
    text: String,
) -> Result<(), String> {
    cue_speak_logic(
        &app,
        core.inner(),
        engine.inner(),
        synth.0.as_ref(),
        &text,
        None,
        None,
    )
}

#[tauri::command]
pub fn cue_speak_quick(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    synth: State<'_, CueSynth>,
    id: String,
) -> Result<(), String> {
    cue_speak_quick_logic(&app, core.inner(), engine.inner(), synth.0.as_ref(), &id)
}

#[tauri::command]
pub fn cue_stop(
    app: AppHandle,
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
) -> Result<(), String> {
    cue_stop_logic(&app, core.inner(), engine.inner())
}

#[tauri::command]
pub fn cue_add(
    core: State<'_, CoreState>,
    label: String,
    text: String,
) -> Result<QuickCue, String> {
    cue_add_logic(core.inner(), label, text)
}

#[tauri::command]
pub fn cue_update(
    core: State<'_, CoreState>,
    id: String,
    label: String,
    text: String,
) -> Result<(), String> {
    cue_update_logic(core.inner(), &id, label, text)
}

#[tauri::command]
pub fn cue_remove(core: State<'_, CoreState>, id: String) -> Result<(), String> {
    cue_remove_logic(core.inner(), &id)
}

#[tauri::command]
pub fn cue_move(core: State<'_, CoreState>, id: String, to_index: usize) -> Result<(), String> {
    cue_move_logic(core.inner(), &id, to_index)
}

#[tauri::command]
pub fn set_cue_voice(core: State<'_, CoreState>, voice: Option<String>) -> Result<(), String> {
    set_cue_voice_logic(core.inner(), voice)
}

#[tauri::command]
pub fn set_cue_rate(core: State<'_, CoreState>, rate: i32) -> Result<(), String> {
    set_cue_rate_logic(core.inner(), rate)
}

#[tauri::command]
pub fn set_cue_volume(
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    volume: f32,
) -> Result<(), String> {
    set_cue_volume_logic(core.inner(), engine.inner(), volume)
}

#[tauri::command]
pub fn set_cue_channels(
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    channel_left: usize,
    channel_right: usize,
) -> Result<(), String> {
    set_cue_channels_logic(core.inner(), engine.inner(), channel_left, channel_right)
}

#[tauri::command]
pub fn set_cue_duck_click(
    core: State<'_, CoreState>,
    engine: State<'_, AudioEngine>,
    duck: bool,
) -> Result<(), String> {
    set_cue_duck_click_logic(core.inner(), engine.inner(), duck)
}

#[tauri::command]
pub fn set_cue_speak_key(core: State<'_, CoreState>, enabled: bool) -> Result<(), String> {
    set_cue_speak_key_logic(core.inner(), enabled)
}
