//! Spectral chroma + major/minor profile correlation, not a guarantee of tonality.
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const N: usize = 4096;
const MAJOR: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct MusicalKey {
    pub pitch: u8,
    pub minor: bool,
    pub confidence: f32,
}

impl MusicalKey {
    pub fn note(self) -> &'static str {
        [
            "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
        ][self.pitch as usize % 12]
    }
    pub fn camelot(self) -> String {
        let index = self.pitch as usize % 12;
        let number = if self.minor {
            [5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10][index]
        } else {
            [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1][index]
        };
        format!("{number}{}", if self.minor { 'A' } else { 'B' })
    }
    pub fn label(self) -> String {
        format!(
            "{}{} / {}",
            self.note(),
            if self.minor { "m" } else { "" },
            self.camelot()
        )
    }
    pub fn sort_index(self) -> u16 {
        self.pitch as u16 * 2 + u16::from(self.minor)
    }
}

fn classify(chroma: [f64; 12]) -> Option<MusicalKey> {
    let mean = chroma.iter().sum::<f64>() / 12.0;
    let variance = chroma.iter().map(|v| (v - mean).powi(2)).sum::<f64>();
    if variance < 1e-10 {
        return None;
    }
    let mut scores = Vec::with_capacity(24);
    for (minor, profile) in [(false, MAJOR), (true, MINOR)] {
        let average = profile.iter().sum::<f64>() / 12.0;
        let denominator =
            (variance * profile.iter().map(|v| (v - average).powi(2)).sum::<f64>()).sqrt();
        for tonic in 0..12 {
            let score = (0..12)
                .map(|i| (chroma[(tonic + i) % 12] - mean) * (profile[i] - average))
                .sum::<f64>()
                / denominator;
            scores.push((score, tonic as u8, minor));
        }
    }
    scores.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (score, pitch, minor) = scores[0];
    if score < 0.5 || score - scores[1].0 < 0.018 {
        return None;
    }
    Some(MusicalKey {
        pitch,
        minor,
        confidence: score.clamp(0.0, 1.0) as f32,
    })
}

pub struct KeyAnalyzer {
    rate: u32,
    target_rate: u32,
    duration: Option<f64>,
    frames: u64,
    phase: u32,
    sum: f64,
    count: u32,
    low: [f64; 2],
    alpha: f64,
    samples: Vec<f32>,
    window: Vec<f32>,
    spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    fft: Arc<dyn Fft<f32>>,
    chroma: [f64; 12],
    windows: u32,
    active: bool,
}

impl KeyAnalyzer {
    pub fn new(rate: u32, duration: Option<f64>) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(N);
        let scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
        Self {
            rate,
            target_rate: rate.min(11025),
            duration,
            frames: 0,
            phase: 0,
            sum: 0.0,
            count: 0,
            low: [0.0; 2],
            alpha: 1.0
                - (-std::f64::consts::TAU * 3500.0_f64.min(rate as f64 * 0.35) / rate as f64).exp(),
            samples: Vec::with_capacity(N),
            window: (0..N)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (N - 1) as f32).cos())
                .collect(),
            spectrum: vec![Complex32::default(); N],
            scratch,
            fft,
            chroma: [0.0; 12],
            windows: 0,
            active: false,
        }
    }

    pub fn push(&mut self, sample: f32) {
        let seconds = self.frames as f64 / self.rate as f64;
        self.frames += 1;
        let active = self
            .duration
            .filter(|d| d.is_finite() && *d > 120.0)
            .map_or(seconds < 120.0, |d| seconds.rem_euclid(d / 4.0) < 30.0);
        if !active {
            if self.active {
                self.samples.clear();
                self.sum = 0.0;
                self.count = 0;
                self.low = [0.0; 2];
            }
            self.active = false;
            return;
        }
        self.active = true;
        self.low[0] += self.alpha * (sample as f64 - self.low[0]);
        self.low[1] += self.alpha * (self.low[0] - self.low[1]);
        self.sum += self.low[1];
        self.count += 1;
        self.phase += self.target_rate;
        if self.phase >= self.rate {
            self.phase -= self.rate;
            self.samples.push((self.sum / self.count as f64) as f32);
            self.sum = 0.0;
            self.count = 0;
            if self.samples.len() == N {
                self.process();
                self.samples.drain(..N / 2);
            }
        }
    }

    fn process(&mut self) {
        for i in 0..N {
            self.spectrum[i] = Complex32::new(self.samples[i] * self.window[i], 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.spectrum, &mut self.scratch);
        let maximum = self.spectrum[1..N / 2]
            .iter()
            .map(|c| c.norm_sqr())
            .fold(0.0_f32, f32::max);
        if maximum < 1e-5 {
            return;
        }
        let mut chroma = [0.0; 12];
        for bin in 2..N / 2 - 1 {
            let [a, b, c] = [
                self.spectrum[bin - 1].norm_sqr(),
                self.spectrum[bin].norm_sqr(),
                self.spectrum[bin + 1].norm_sqr(),
            ];
            if b < maximum * 0.0025 || b <= a || b < c {
                continue;
            }
            let [a, b_log, c] = [a.max(1e-20).ln(), b.max(1e-20).ln(), c.max(1e-20).ln()];
            let denominator = a - 2.0 * b_log + c;
            let delta = if denominator.abs() > 1e-10 {
                (0.5 * (a - c) / denominator).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            let frequency = (bin as f32 + delta) * self.target_rate as f32 / N as f32;
            if !(55.0..=4000.0).contains(&frequency) {
                continue;
            }
            let midi = 69.0 + 12.0 * (frequency / 440.0).log2();
            let distance = (midi - midi.round()).abs();
            if distance > 0.4 {
                continue;
            }
            let pitch = (midi.round() as i32).rem_euclid(12) as usize;
            chroma[pitch] += b.sqrt() as f64 * (1.0 - distance as f64 / 0.5)
                / (frequency as f64 / 220.0).powf(0.2);
        }
        let total = chroma.iter().sum::<f64>();
        if total > 1e-8 {
            for (target, value) in self.chroma.iter_mut().zip(chroma) {
                *target += value / total;
            }
            self.windows += 1;
        }
    }

    pub fn finish(self) -> Option<MusicalKey> {
        if self.windows < 3 {
            None
        } else {
            classify(self.chroma)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_major_minor_profiles_and_camelot_labels_are_consistent() {
        for minor in [false, true] {
            for tonic in 0..12 {
                let profile = if minor { MINOR } else { MAJOR };
                let chroma = std::array::from_fn(|pitch| profile[(pitch + 12 - tonic) % 12]);
                let key = classify(chroma).unwrap();
                assert_eq!((key.pitch, key.minor), (tonic as u8, minor));
            }
        }
        assert_eq!(
            MusicalKey {
                pitch: 9,
                minor: true,
                confidence: 1.0
            }
            .label(),
            "Am / 8A"
        );
        assert_eq!(
            MusicalKey {
                pitch: 0,
                minor: false,
                confidence: 1.0
            }
            .label(),
            "C / 8B"
        );
        assert!(classify([1.0; 12]).is_none());
    }

    #[test]
    fn estimates_real_synthesized_chord_progressions_and_rejects_silence() {
        for (minor, chords) in [
            (
                false,
                [[60, 64, 67], [65, 69, 72], [67, 71, 74], [60, 64, 67]],
            ),
            (
                true,
                [[57, 60, 64], [62, 65, 69], [64, 67, 71], [57, 60, 64]],
            ),
        ] {
            let mut analysis = KeyAnalyzer::new(22050, Some(8.0));
            for i in 0..22050 * 8 {
                let time = i as f32 / 22050.0;
                let chord = chords[i / (22050 * 2)];
                let sample: f32 = chord
                    .into_iter()
                    .enumerate()
                    .map(|(j, note)| {
                        let hz = 440.0 * 2.0_f32.powf((note as f32 - 69.0) / 12.0);
                        (std::f32::consts::TAU * hz * time).sin() * if j == 0 { 0.15 } else { 0.07 }
                    })
                    .sum();
                analysis.push(sample);
            }
            let key = analysis.finish().unwrap();
            assert_eq!(
                (key.pitch, key.minor),
                (if minor { 9 } else { 0 }, minor),
                "{key:?}"
            );
        }
        let mut silence = KeyAnalyzer::new(44100, Some(8.0));
        for _ in 0..44100 * 8 {
            silence.push(0.0);
        }
        assert!(silence.finish().is_none());
    }
}
