//! Text-to-speech "cues" — quick spoken messages fired from the phone remote
//! to talk over IEMs without yelling across stage.
//!
//! `Synthesizer` is the abstraction; `SapiSynth` is the v1 Windows impl over
//! PowerShell. Swap in another `Synthesizer` (COM, macOS/Linux) at construction.

mod sapi;
mod synth;

pub use sapi::{temp_wav_path as sapi_temp_wav_path, SapiSynth};
pub use synth::{Synthesizer, VoiceInfo};
