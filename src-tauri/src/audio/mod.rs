//! Real-time audio engine for looping worship pads with crossfade + channel routing.

mod decode;
mod engine;

pub use engine::{AudioDebugReport, AudioEngine, DeviceInfo, EngineEvent};
