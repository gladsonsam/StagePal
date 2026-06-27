//! Generates a looping pad from a root frequency — no audio file required.
//! Chain: detuned tri/saw oscillators -> drive -> low-pass -> EQ -> chorus ->
//! reverb. Voicing is a key-neutral power chord (sub, root, 5th, octave).
//!
//! The returned `Decoder` matches `decode::spawn`, so the engine treats
//! file-based and synth pads identically.

use std::f32::consts::{PI, TAU};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rtrb::{Producer, RingBuffer};

use super::decode::Decoder;

/// Spawn a synth thread feeding interleaved-stereo f32 at `out_rate`.
pub fn spawn(root_hz: f32, out_rate: u32) -> Decoder {
    let capacity = (out_rate as usize * 2 * 2).max(16384);
    let (producer, consumer) = RingBuffer::<f32>::new(capacity);
    let stop = Arc::new(AtomicBool::new(false));
    let ended = Arc::new(AtomicBool::new(false));

    let stop_thread = stop.clone();
    let ended_thread = ended.clone();
    std::thread::Builder::new()
        .name("synth-pad".into())
        .spawn(move || {
            render_loop(root_hz, out_rate, producer, &stop_thread);
            ended_thread.store(true, Ordering::Relaxed);
        })
        .expect("failed to spawn synth thread");

    Decoder { consumer, stop, ended }
}

/// Chord layers: (frequency ratio, amplitude, detune scale, is_sine).
const LAYERS: [(f32, f32, f32, bool); 4] = [
    (0.5, 0.50, 0.30, true),  // sub
    (1.0, 1.00, 1.00, false), // root
    (1.5, 0.50, 1.00, false), // 5th
    (2.0, 0.28, 1.05, false), // octave
];

const UNISON: usize = 3;
const SUB_UNISON: usize = 2;
const DETUNE_CENTS: f32 = 5.0;
/// Triangle↔saw blend for tone oscillators (0 = triangle, 1 = saw).
const SAW_MIX: f32 = 0.22;

const CHUNK: usize = 256;
const MASTER: f32 = 0.45;
const DRY_MIX: f32 = 0.62;
const WET_MIX: f32 = 0.50;
const DRIVE: f32 = 0.15;

const EQ_HZ: f32 = 430.0;
const EQ_Q: f32 = 0.9;
const EQ_GAIN_DB: f32 = -3.5;

const CHORUS_BASE_MS: f32 = 16.0;
const CHORUS_DEPTH_MS: f32 = 1.6;
const CHORUS_RATES_HZ: [f32; CHORUS_VOICES] = [0.18, 0.24, 0.31];

#[inline]
fn cents(c: f32) -> f32 {
    (c / 1200.0 * std::f32::consts::LN_2).exp()
}

// --- Oscillator (sine, or triangle/saw blend) ---

struct Osc {
    phase: f32,
    inc: f32,
    sine: bool,
    saw_mix: f32,
}

impl Osc {
    fn new(freq: f32, sr: f32, phase0: f32, sine: bool, saw_mix: f32) -> Self {
        Osc { phase: phase0, inc: freq / sr, sine, saw_mix }
    }

    /// `mult` applies the slow pitch-drift LFO.
    #[inline]
    fn next(&mut self, mult: f32) -> f32 {
        let dt = self.inc * mult;
        let t = self.phase;
        let out = if self.sine {
            (t * TAU).sin()
        } else {
            let tri = 1.0 - 2.0 * (2.0 * t - 1.0).abs();
            let saw = (2.0 * t - 1.0) - poly_blep(t, dt);
            tri * (1.0 - self.saw_mix) + saw * self.saw_mix
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        out
    }
}

/// PolyBLEP residual to band-limit a sawtooth's wrap discontinuity.
#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

struct VoiceOsc {
    osc: Osc,
    amp: f32,
}

// --- State-variable low-pass (Cytomic TPT, stable at all cutoffs) ---

struct Svf {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl Svf {
    fn new() -> Self {
        Svf { ic1: 0.0, ic2: 0.0, a1: 0.0, a2: 0.0, a3: 0.0 }
    }

    fn set_cutoff(&mut self, fc: f32, q: f32, sr: f32) {
        let g = (PI * (fc / sr).clamp(0.0001, 0.49)).tan();
        let k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    #[inline]
    fn low(&mut self, v0: f32) -> f32 {
        let v3 = v0 - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2
    }
}

// --- Biquad (RBJ peaking EQ) ---

struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn peaking(f0: f32, sr: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = TAU * f0 / sr;
        let cos = w0.cos();
        let alpha = w0.sin() / (2.0 * q);
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos;
        let a2 = 1.0 - alpha / a;
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

// --- Ensemble chorus (modulated delay voices) ---

const CHORUS_VOICES: usize = 3;

struct Chorus {
    buf: Vec<f32>,
    widx: usize,
    lfo: [f32; CHORUS_VOICES],
    inc: [f32; CHORUS_VOICES],
    /// Base delay and modulation depth, in samples.
    base: f32,
    depth: f32,
}

impl Chorus {
    fn new(sr: f32, side: usize) -> Self {
        let base = CHORUS_BASE_MS / 1000.0 * sr;
        let depth = CHORUS_DEPTH_MS / 1000.0 * sr;
        let len = (base + depth) as usize + 4;
        // Voices spread ~120° apart; right channel offset for stereo width.
        let side_phase = if side == 0 { 0.0 } else { PI * 0.5 };
        let mut lfo = [0.0f32; CHORUS_VOICES];
        let mut inc = [0.0f32; CHORUS_VOICES];
        for i in 0..CHORUS_VOICES {
            lfo[i] = (i as f32 * (TAU / CHORUS_VOICES as f32) + side_phase) % TAU;
            inc[i] = TAU * CHORUS_RATES_HZ[i] / sr;
        }
        Chorus { buf: vec![0.0; len], widx: 0, lfo, inc, base, depth }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let n = self.buf.len();
        self.buf[self.widx] = x;

        let mut wet = 0.0;
        for i in 0..CHORUS_VOICES {
            let d = self.base + self.depth * self.lfo[i].sin();
            let mut rp = self.widx as f32 - d;
            if rp < 0.0 {
                rp += n as f32;
            }
            let i0 = rp.floor() as usize % n;
            let frac = rp - rp.floor();
            let i1 = (i0 + 1) % n;
            wet += self.buf[i0] * (1.0 - frac) + self.buf[i1] * frac;

            self.lfo[i] += self.inc[i];
            if self.lfo[i] >= TAU {
                self.lfo[i] -= TAU;
            }
        }
        wet /= CHORUS_VOICES as f32;

        self.widx += 1;
        if self.widx >= n {
            self.widx = 0;
        }
        x * 0.7 + wet * 0.5
    }
}

#[inline]
fn drive(x: f32, amt: f32) -> f32 {
    (x * (1.0 + amt)).tanh()
}

// --- Freeverb (8 parallel combs -> 4 series allpasses per channel) ---

const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
    damp1: f32,
    damp2: f32,
    feedback: f32,
}

impl Comb {
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let out = self.buf[self.idx];
        self.store = out * self.damp2 + self.store * self.damp1;
        self.buf[self.idx] = input + self.store * self.feedback;
        self.idx += 1;
        if self.idx >= self.buf.len() {
            self.idx = 0;
        }
        out
    }
}

struct Allpass {
    buf: Vec<f32>,
    idx: usize,
    feedback: f32,
}

impl Allpass {
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let bufout = self.buf[self.idx];
        let out = -input + bufout;
        self.buf[self.idx] = input + bufout * self.feedback;
        self.idx += 1;
        if self.idx >= self.buf.len() {
            self.idx = 0;
        }
        out
    }
}

struct Reverb {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
    input_gain: f32,
}

impl Reverb {
    fn new(sr: f32, spread: usize, feedback: f32, damp1: f32, input_gain: f32) -> Self {
        let scale = sr / 44100.0;
        let size = |n: usize| ((n as f32 * scale) as usize).max(1) + spread;
        let combs = COMB_TUNING
            .iter()
            .map(|&n| Comb {
                buf: vec![0.0; size(n)],
                idx: 0,
                store: 0.0,
                damp1,
                damp2: 1.0 - damp1,
                feedback,
            })
            .collect();
        let allpasses = ALLPASS_TUNING
            .iter()
            .map(|&n| Allpass { buf: vec![0.0; size(n)], idx: 0, feedback: 0.5 })
            .collect();
        Reverb { combs, allpasses, input_gain }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let inp = input * self.input_gain;
        let mut out = 0.0;
        for c in &mut self.combs {
            out += c.process(inp);
        }
        for a in &mut self.allpasses {
            out = a.process(out);
        }
        out
    }
}

// --- Per-channel voice stack + filter + reverb ---

struct Channel {
    oscs: Vec<VoiceOsc>,
    /// Cascaded for a 24 dB/oct rolloff.
    svf1: Svf,
    svf2: Svf,
    eq: Biquad,
    chorus: Chorus,
    reverb: Reverb,
}

impl Channel {
    fn new(root_hz: f32, sr: f32, side: usize) -> Self {
        let mut oscs = Vec::new();
        for &(ratio, amp, detune_scale, sine) in LAYERS.iter() {
            let n = if sine { SUB_UNISON } else { UNISON };
            let layer_freq = root_hz * ratio;
            for i in 0..n {
                let frac = if n == 1 {
                    0.0
                } else {
                    (i as f32 / (n as f32 - 1.0)) * 2.0 - 1.0
                };
                // Offset L vs R detune + phase to decorrelate the two sides.
                let side_off = if side == 0 { 0.0 } else { 1.7 };
                let detune = frac * DETUNE_CENTS * detune_scale + side_off;
                let freq = layer_freq * cents(detune);
                let phase0 = ((i as f32 * 0.6180339 + side as f32 * 0.37) % 1.0).abs();
                oscs.push(VoiceOsc {
                    osc: Osc::new(freq, sr, phase0, sine, SAW_MIX),
                    amp: amp / n as f32,
                });
            }
        }
        Channel {
            oscs,
            svf1: Svf::new(),
            svf2: Svf::new(),
            eq: Biquad::peaking(EQ_HZ, sr, EQ_Q, EQ_GAIN_DB),
            chorus: Chorus::new(sr, side),
            reverb: Reverb::new(sr, side * STEREO_SPREAD, 0.84, 0.38, 0.015),
        }
    }

    #[inline]
    fn process(&mut self, drift: f32, breath: f32) -> f32 {
        let mut sig = 0.0;
        for v in &mut self.oscs {
            sig += v.osc.next(drift) * v.amp;
        }
        sig /= TOTAL_AMP;
        sig = drive(sig, DRIVE);
        sig = self.svf2.low(self.svf1.low(sig));
        sig = self.eq.process(sig) * breath;
        let body = self.chorus.process(sig);
        let wet = self.reverb.process(body);
        body * DRY_MIX + wet * WET_MIX
    }
}

/// Sum of layer amplitudes, for post-mix normalization.
const TOTAL_AMP: f32 = 2.28;

// --- Render loop ---

const FILT_BASE_HZ: f32 = 1050.0;
const FILT_DEPTH_HZ: f32 = 280.0;
const FILT_Q: f32 = 0.6;
const FILT_LFO_HZ: f32 = 0.07;
const AMP_LFO_HZ: f32 = 0.15;
const AMP_DEPTH: f32 = 0.05;
const DRIFT_LFO_HZ: f32 = 0.09;
const DRIFT_DEPTH: f32 = 0.0009;

#[inline]
fn soft_clip(x: f32) -> f32 {
    x.tanh()
}

fn render_loop(root_hz: f32, out_rate: u32, mut producer: Producer<f32>, stop: &AtomicBool) {
    let sr = out_rate as f32;
    let mut l = Channel::new(root_hz, sr, 0);
    let mut r = Channel::new(root_hz, sr, 1);

    let filt_inc = TAU * FILT_LFO_HZ / sr;
    let amp_inc = TAU * AMP_LFO_HZ / sr;
    let drift_inc = TAU * DRIFT_LFO_HZ / sr;
    let mut filt_phase = 0.0f32;
    let mut amp_phase = 0.0f32;
    let mut drift_phase = 0.0f32;

    loop {
        if stop.load(Ordering::Relaxed) || producer.is_abandoned() {
            return;
        }

        // Filter coefficients update once per chunk (tan is expensive).
        let cutoff = FILT_BASE_HZ + FILT_DEPTH_HZ * filt_phase.sin();
        l.svf1.set_cutoff(cutoff, FILT_Q, sr);
        l.svf2.set_cutoff(cutoff, FILT_Q, sr);
        r.svf1.set_cutoff(cutoff, FILT_Q, sr);
        r.svf2.set_cutoff(cutoff, FILT_Q, sr);

        for _ in 0..CHUNK {
            let breath = 1.0 + AMP_DEPTH * amp_phase.sin();
            let drift_l = 1.0 + DRIFT_DEPTH * drift_phase.sin();
            let drift_r = 1.0 + DRIFT_DEPTH * (drift_phase + 2.2).sin();

            let lo = soft_clip(l.process(drift_l, breath) * MASTER);
            let ro = soft_clip(r.process(drift_r, breath) * MASTER);

            filt_phase += filt_inc;
            if filt_phase >= TAU {
                filt_phase -= TAU;
            }
            amp_phase += amp_inc;
            if amp_phase >= TAU {
                amp_phase -= TAU;
            }
            drift_phase += drift_inc;
            if drift_phase >= TAU {
                drift_phase -= TAU;
            }

            for s in [lo, ro] {
                loop {
                    if producer.push(s).is_ok() {
                        break;
                    }
                    if stop.load(Ordering::Relaxed) || producer.is_abandoned() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
}
