//! `Synthesizer` trait — boundary between the app and the TTS engine. Impls
//! render text to a WAV the audio engine plays via its normal path, so the
//! engine needn't know TTS exists.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One installed system voice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceInfo {
    /// Selector passed to `synth_to_wav`. For SAPI, the voice Name string.
    pub id: String,
    /// UI label. May equal `id`.
    pub name: String,
}

pub trait Synthesizer: Send + Sync {
    /// Pickable voices; may be empty if none installed.
    fn voices(&self) -> Result<Vec<VoiceInfo>, String>;

    /// Render `text` to `out` as WAV. `voice` `None` = system default. `rate`
    /// is the SAPI scale -10..10 (0 = normal); impls may clamp.
    ///
    /// Sync on purpose: callers run it on a worker thread; async would force
    /// every impl into a runtime.
    fn synth_to_wav(
        &self,
        text: &str,
        voice: Option<&str>,
        rate: i32,
        out: &Path,
    ) -> Result<(), String>;
}
