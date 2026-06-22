//! Windows SAPI impl via PowerShell + System.Speech.Synthesis.
//!
//! PowerShell over COM: zero new deps, ships on every Windows. Trait lets us
//! swap to a `windows`-crate COM impl later without touching the audio engine.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use super::synth::{Synthesizer, VoiceInfo};

/// Collision-free temp filenames within a process lifetime.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// CREATE_NO_WINDOW — stops a console flashing when GUI spawns powershell.exe.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct SapiSynth;

impl SapiSynth {
    pub fn new() -> Self {
        SapiSynth
    }
}

impl Default for SapiSynth {
    fn default() -> Self {
        SapiSynth::new()
    }
}

impl Synthesizer for SapiSynth {
    fn voices(&self) -> Result<Vec<VoiceInfo>, String> {
        // @() coerces a one-element result to an array (else JSON yields a bare object).
        let script = "Add-Type -AssemblyName System.Speech | Out-Null; \
            $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
            $voices = @($s.GetInstalledVoices() | ForEach-Object { \
                @{ id = $_.VoiceInfo.Name; name = $_.VoiceInfo.Name } \
            }); \
            $s.Dispose(); \
            ConvertTo-Json -Compress -InputObject $voices";

        let out = run_ps(script)?;
        if out.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str::<Vec<VoiceInfo>>(out.trim())
            .map_err(|e| format!("parse voices: {e} (raw: {out:?})"))
    }

    fn synth_to_wav(
        &self,
        text: &str,
        voice: Option<&str>,
        rate: i32,
        out: &Path,
    ) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("nothing to speak".into());
        }

        // Text via stdin to dodge PowerShell quoting pitfalls (apostrophes, newlines, non-ASCII).
        let rate = rate.clamp(-10, 10);
        let out_path = ps_escape(&out.to_string_lossy());
        let voice_line = match voice {
            Some(v) if !v.trim().is_empty() => {
                format!("$s.SelectVoice('{}');", ps_escape(v))
            }
            _ => String::new(),
        };

        let script = format!(
            "Add-Type -AssemblyName System.Speech | Out-Null; \
            $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
            $s.Rate = {rate}; \
            {voice_line} \
            $s.SetOutputToWaveFile('{out_path}'); \
            $reader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8); \
            $text = $reader.ReadToEnd(); \
            $s.Speak($text); \
            $s.Dispose()"
        );

        run_ps_stdin(&script, text.as_bytes()).map(|_| ())
    }
}

/// Temp path for a rendered cue WAV. Caller deletes after playback.
pub fn temp_wav_path() -> std::path::PathBuf {
    let pid = std::process::id();
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("stagepal-cue-{pid}-{n}.wav"))
}

/// Escape for a PowerShell single-quoted literal: double the apostrophe.
fn ps_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// Run a PowerShell script (no stdin), capture stdout. Errors carry stderr.
fn run_ps(script: &str) -> Result<String, String> {
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let output = cmd
        .output()
        .map_err(|e| format!("spawn powershell: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "powershell exited {} — {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Like `run_ps` but writes `stdin_bytes` to stdin, avoiding command-line quoting.
fn run_ps_stdin(script: &str, stdin_bytes: &[u8]) -> Result<String, String> {
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn powershell: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_bytes)
            .map_err(|e| format!("write stdin: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("await powershell: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "powershell exited {} — {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
