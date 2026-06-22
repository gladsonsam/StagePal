//! Headless smoke test for the audio engine.
//!   cargo run --example audio_smoke
//!
//! Lists output devices, writes a 220 Hz stereo test tone (44.1 kHz, so the
//! resampler runs when the device is at 48 kHz), then drives the real
//! AudioEngine: set_output → play (crossfade-in) → stop (fade-out).
//! If your default output is audible you should hear ~3 s of a low tone.

use std::f32::consts::PI;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use stagepal_lib::audio::AudioEngine;

fn main() {
    println!("== output devices ==");
    let devices = AudioEngine::list_devices();
    for d in &devices {
        println!(
            "  {}{}  channels={} sample_rate={}",
            d.name,
            if d.is_default { " (default)" } else { "" },
            d.channels,
            d.default_sample_rate
        );
    }

    let Some(dev) = devices.iter().find(|d| d.is_default).cloned() else {
        eprintln!("no default output device; aborting");
        return;
    };

    let tone = std::env::temp_dir().join("pad_smoke_tone.wav");
    write_test_wav(&tone, 44_100, 3.0, 220.0).expect("write test wav");
    println!("\nwrote test tone: {tone:?}");

    let engine = AudioEngine::new();
    if let Err(e) = engine.set_output(&dev.host, &dev.name, (0, 1), (2, 3), (4, 5)) {
        eprintln!("set_output failed: {e}");
        return;
    }
    engine.set_volume(0.4).unwrap();
    engine.play(tone.clone()).unwrap();

    println!("playing (with 2 s crossfade-in) to '{}' ch (0,1)...", dev.name);
    std::thread::sleep(Duration::from_secs(3));

    println!("stop (fade out)...");
    engine.stop().unwrap();
    std::thread::sleep(Duration::from_millis(2500));

    let _ = std::fs::remove_file(&tone);
    println!("done — no errors.");
}

/// Write a minimal 16-bit PCM stereo WAV with a sine tone.
fn write_test_wav(path: &PathBuf, rate: u32, secs: f32, freq: f32) -> std::io::Result<()> {
    let channels: u16 = 2;
    let bits: u16 = 16;
    let n_frames = (rate as f32 * secs) as u32;
    let data_len = n_frames * channels as u32 * (bits / 8) as u32;
    let byte_rate = rate * channels as u32 * (bits / 8) as u32;
    let block_align = channels * (bits / 8);

    let mut f = File::create(path)?;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?; // PCM fmt chunk size
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&bits.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;

    for i in 0..n_frames {
        let t = i as f32 / rate as f32;
        let s = (2.0 * PI * freq * t).sin() * 0.5;
        let v = (s * i16::MAX as f32) as i16;
        f.write_all(&v.to_le_bytes())?; // L
        f.write_all(&v.to_le_bytes())?; // R
    }
    Ok(())
}
