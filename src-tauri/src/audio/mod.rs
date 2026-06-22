//! Real-time audio engine: looping pads, crossfade, channel routing.

mod decode;
mod engine;

pub use engine::{AudioDebugReport, AudioEngine, DeviceInfo, EngineEvent};
