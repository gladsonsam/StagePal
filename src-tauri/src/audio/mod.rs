//! Real-time audio engine: looping pads, crossfade, channel routing.

mod decode;
mod engine;
pub(super) mod synth;

pub use engine::{AudioDebugReport, AudioEngine, DeviceInfo, EngineEvent};
