// StagePal — Tauri backend entrypoint.

pub mod audio;
mod commands;
pub mod cues;
pub mod library;
pub mod model;
mod server;
mod state;

use audio::AudioEngine;
use cues::SapiSynth;
use state::CoreState;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, WindowEvent,
};

/// Show and focus the main window (boots hidden).
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Re-apply saved audio settings on boot, except click `enabled` (boots stopped).
/// Runs on a background thread so a slow ASIO open doesn't pin the UI; until it
/// returns `has_active` is false and presses surface "audio output not ready".
fn restore_audio(app: &tauri::AppHandle) {
    let core = app.state::<CoreState>();
    let engine = app.state::<AudioEngine>();

    let (
        host,
        device,
        pad_channels,
        click_channels,
        cue_channels,
        volume,
        cue_volume,
        crossfade_ms,
        click_bpm,
        click_beats,
        click_accent,
        click_volume,
        duck_click,
    ) = {
        let s = core.settings.lock().unwrap();
        (
            s.output_host.clone(),
            s.output_device.clone(),
            (s.channel_left, s.channel_right),
            (s.click.channel_left, s.click.channel_right),
            (s.cues.channel_left, s.cues.channel_right),
            s.master_volume,
            s.cues.volume,
            s.crossfade_ms,
            s.click.bpm,
            s.click.beats_per_bar,
            s.click.accent,
            s.click.volume,
            s.cues.duck_click,
        )
    };

    let _ = engine.set_volume(volume);
    let _ = engine.set_cue_volume(cue_volume);
    let _ = engine.set_crossfade(crossfade_ms);
    let _ = engine.set_click_bpm(click_bpm);
    let _ = engine.set_click_beats(click_beats);
    let _ = engine.set_click_accent(click_accent);
    let _ = engine.set_click_volume(click_volume);
    let _ = engine.set_duck_click(duck_click);
    if let Some(device) = device {
        if let Err(e) =
            engine.set_output(&host, &device, pad_channels, click_channels, cue_channels)
        {
            eprintln!("[boot] could not restore audio output '{device}' on {host}: {e}");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    // Single instance must be the FIRST plugin registered.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }));
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(AudioEngine::new())
        .manage(commands::CueSynth(Box::new(SapiSynth::new())))
        .setup(|app| {
            // Load persisted settings into CoreState.
            let config_path = app
                .path()
                .app_config_dir()
                .map(|d| d.join("settings.json"))
                .unwrap_or_else(|_| std::path::PathBuf::from("settings.json"));
            app.manage(CoreState::load(config_path));

            // Restore audio on a background thread so a hung driver can't block
            // setup() from returning — the window must show regardless.
            let app_for_restore = app.handle().clone();
            std::thread::Builder::new()
                .name("restore-audio".into())
                .spawn(move || restore_audio(&app_for_restore))
                .ok();

            // Pre-warm SAPI: the first System.Speech call costs ~2-4 s cold, so
            // a throwaway voice listing loads OS caches off-thread.
            let app_for_warm = app.handle().clone();
            std::thread::Builder::new()
                .name("sapi-prewarm".into())
                .spawn(move || {
                    let synth = app_for_warm.state::<commands::CueSynth>();
                    if let Err(e) = synth.0.voices() {
                        eprintln!("[boot] sapi prewarm: {e}");
                    }
                })
                .ok();

            // Bridge engine events to NowPlaying broadcasts.
            let events = app.state::<AudioEngine>().events();
            let app_for_events = app.handle().clone();
            std::thread::Builder::new()
                .name("engine-events".into())
                .spawn(move || {
                    while let Ok(ev) = events.recv() {
                        match ev {
                            audio::EngineEvent::CueStarted => {
                                // commands.rs already flipped speaking=true.
                            }
                            audio::EngineEvent::CueEnded => {
                                let core = app_for_events.state::<CoreState>();
                                {
                                    let mut n = core.now.lock().unwrap();
                                    n.cue.speaking = false;
                                    n.cue.label = None;
                                }
                                commands::emit_now(&app_for_events, core.inner());
                            }
                            audio::EngineEvent::PadEnded => {
                                // Pad exited unexpectedly (file moved/share
                                // dropped). Reset so the next press restarts
                                // rather than hitting the deselect path.
                                let core = app_for_events.state::<CoreState>();
                                {
                                    let mut n = core.now.lock().unwrap();
                                    n.playing = false;
                                    n.key = None;
                                }
                                commands::emit_now(&app_for_events, core.inner());
                            }
                        }
                    }
                })
                .ok();

            // Start the phone-remote web server.
            let port = app
                .state::<CoreState>()
                .settings
                .lock()
                .unwrap()
                .server_port;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                server::serve(handle, port).await;
            });

            // Launch on system startup.
            #[cfg(desktop)]
            {
                use tauri_plugin_autostart::MacosLauncher;
                app.handle().plugin(tauri_plugin_autostart::init(
                    MacosLauncher::LaunchAgent,
                    None,
                ))?;

                // Enable launch-on-login (release only, so dev doesn't register
                // the debug binary). enable() is idempotent.
                #[cfg(not(debug_assertions))]
                {
                    use tauri_plugin_autostart::ManagerExt;
                    let autostart = app.handle().autolaunch();
                    if !autostart.is_enabled().unwrap_or(false) {
                        if let Err(e) = autostart.enable() {
                            eprintln!("[autostart] could not enable launch-on-login: {e}");
                        }
                    }
                }
            }

            // System tray.
            let open_item = MenuItem::with_id(app, "open", "Open Settings", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_item, &quit_item])?;

            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("StagePal")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        // Hide to tray instead of quitting on close.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::get_state,
            commands::list_audio_devices,
            commands::set_audio_output,
            commands::run_audio_output_test,
            commands::set_volume,
            commands::scan_library,
            commands::remove_preset,
            commands::set_preset,
            commands::rename_preset,
            commands::assign_key,
            commands::clear_key,
            commands::set_crossfade,
            commands::play_key,
            commands::stop,
            commands::server_url,
            commands::set_click_enabled,
            commands::set_click_bpm,
            commands::set_click_beats,
            commands::set_click_accent,
            commands::set_click_volume,
            commands::set_click_channels,
            commands::list_voices,
            commands::cue_speak,
            commands::cue_speak_quick,
            commands::cue_stop,
            commands::cue_add,
            commands::cue_update,
            commands::cue_remove,
            commands::cue_move,
            commands::set_cue_voice,
            commands::set_cue_rate,
            commands::set_cue_volume,
            commands::set_cue_channels,
            commands::set_cue_duck_click,
            commands::set_cue_speak_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
