//! Ten octave-spaced peaking filters on the stereo master bus, before meters,
//! stream and recording. Parameters are shared; filter histories are per channel.
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

pub const FREQUENCIES: [f32; 10] = [
    31.25, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
pub const LABELS: [&str; 10] = [
    "31", "63", "125", "250", "500", "1k", "2k", "4k", "8k", "16k",
];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub preamp: f32,
    pub bands: [f32; 10],
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            preamp: 0.0,
            bands: [0.0; 10],
        }
    }
}

#[derive(Default)]
pub struct Params {
    enabled: AtomicBool,
    preamp: AtomicU32,
    bands: [AtomicU32; 10],
}
impl Params {
    pub fn set(&self, settings: Settings) {
        let finite = |v: f32| {
            if v.is_finite() {
                v.clamp(-12.0, 12.0)
            } else {
                0.0
            }
        };
        for (target, db) in self.bands.iter().zip(settings.bands) {
            target.store(finite(db).to_bits(), Ordering::Relaxed);
        }
        self.preamp
            .store(finite(settings.preamp).to_bits(), Ordering::Relaxed);
        self.enabled.store(settings.enabled, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
struct Coeff {
    b: [f64; 3],
    a: [f64; 2],
}
impl Coeff {
    fn make(frequency: f32, db: f64, rate: u32) -> Self {
        if db.abs() < 0.00001 || frequency as f64 >= rate as f64 * 0.45 {
            return Self {
                b: [1.0, 0.0, 0.0],
                a: [0.0; 2],
            };
        }
        let w = std::f64::consts::TAU * frequency as f64 / rate.max(1) as f64;
        let amplitude = 10.0_f64.powf(db / 40.0);
        let alpha = w.sin() / (2.0 * std::f64::consts::SQRT_2);
        let denominator = 1.0 + alpha / amplitude;
        Self {
            b: [
                (1.0 + alpha * amplitude) / denominator,
                -2.0 * w.cos() / denominator,
                (1.0 - alpha * amplitude) / denominator,
            ],
            a: [
                -2.0 * w.cos() / denominator,
                (1.0 - alpha / amplitude) / denominator,
            ],
        }
    }
}

pub struct Processor {
    params: Arc<Params>,
    rate: u32,
    frames: u64,
    db: [f64; 10],
    coeff: [Coeff; 10],
    states: [[[f64; 2]; 10]; 2],
    wet: f64,
    gain: f64,
}
impl Processor {
    pub fn new(params: Arc<Params>, rate: u32) -> Self {
        let db =
            std::array::from_fn(|i| f32::from_bits(params.bands[i].load(Ordering::Relaxed)) as f64);
        let coeff = std::array::from_fn(|i| Coeff::make(FREQUENCIES[i], db[i], rate));
        let wet = if params.enabled.load(Ordering::Acquire) {
            1.0
        } else {
            0.0
        };
        let gain =
            10.0_f64.powf(f32::from_bits(params.preamp.load(Ordering::Relaxed)) as f64 / 20.0);
        Self {
            params,
            rate,
            frames: 0,
            db,
            coeff,
            states: [[[0.0; 2]; 10]; 2],
            wet,
            gain,
        }
    }

    pub fn process(&mut self, value: f32, channel: usize) -> f32 {
        if channel == 0 {
            if self.frames.is_multiple_of(64) {
                let smooth = 1.0 - (-64.0 / (self.rate.max(1) as f64 * 0.02)).exp();
                let enabled = self.params.enabled.load(Ordering::Acquire);
                if enabled && self.wet == 0.0 {
                    self.states = [[[0.0; 2]; 10]; 2];
                }
                self.wet += ((if enabled { 1.0 } else { 0.0 }) - self.wet) * smooth;
                if !enabled && self.wet < 0.00001 {
                    self.wet = 0.0;
                }
                for (i, frequency) in FREQUENCIES.into_iter().enumerate() {
                    let target =
                        f32::from_bits(self.params.bands[i].load(Ordering::Relaxed)) as f64;
                    self.db[i] += (target - self.db[i]) * smooth;
                    self.coeff[i] = Coeff::make(frequency, self.db[i], self.rate);
                }
                let target = 10.0_f64
                    .powf(f32::from_bits(self.params.preamp.load(Ordering::Relaxed)) as f64 / 20.0);
                self.gain += (target - self.gain) * smooth;
            }
            self.frames = self.frames.wrapping_add(1);
        }
        if self.wet == 0.0 {
            return value;
        }
        let mut filtered = value as f64;
        for (coeff, state) in self.coeff.iter().zip(&mut self.states[channel]) {
            let next = coeff.b[0] * filtered + state[0];
            *state = [
                coeff.b[1] * filtered - coeff.a[0] * next + state[1],
                coeff.b[2] * filtered - coeff.a[1] * next,
            ];
            filtered = next;
        }
        let output = value as f64 + (filtered * self.gain - value as f64) * self.wet;
        if output.is_finite() {
            output as f32
        } else {
            0.0
        }
    }
}
