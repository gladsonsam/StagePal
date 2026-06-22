//! Shared core state: persisted settings + live "now playing", plus a broadcast
//! bus so all clients (desktop + phones) see state changes.

use std::path::PathBuf;
use std::sync::Mutex;

use tokio::sync::broadcast;

use crate::model::{ClickNow, CueNow, NowPlaying, Settings};

pub struct CoreState {
    pub settings: Mutex<Settings>,
    pub now: Mutex<NowPlaying>,
    /// NowPlaying snapshots, consumed by the phone WebSocket.
    pub tx: broadcast::Sender<NowPlaying>,
    config_path: PathBuf,
}

impl CoreState {
    /// Load settings, or defaults if missing/corrupt.
    pub fn load(config_path: PathBuf) -> Self {
        let settings = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default();

        let now = NowPlaying {
            volume: settings.master_volume,
            preset: settings.active_preset.clone(),
            click: ClickNow {
                enabled: false,
                bpm: settings.click.bpm,
                beats_per_bar: settings.click.beats_per_bar,
                volume: settings.click.volume,
                accent: settings.click.accent,
                started_at_ms: None,
            },
            cue: CueNow::default(),
            ..Default::default()
        };

        let (tx, _rx) = broadcast::channel(64);

        CoreState {
            settings: Mutex::new(settings),
            now: Mutex::new(now),
            tx,
            config_path,
        }
    }

    /// Persist settings to disk.
    pub fn save(&self) -> Result<(), String> {
        let settings = self.settings.lock().unwrap();
        if let Some(dir) = self.config_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create config dir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(&*settings).map_err(|e| e.to_string())?;
        std::fs::write(&self.config_path, json).map_err(|e| format!("write settings: {e}"))
    }

    /// Clone of live playback state.
    pub fn snapshot(&self) -> NowPlaying {
        self.now.lock().unwrap().clone()
    }
}
