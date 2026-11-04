//! RBJ/W3C biquad EQ: low shelf 200 Hz, mid peak 1 kHz, high shelf 4 kHz.
use rodio::{Source, source::SeekError};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

pub struct EqParams {
    db: [AtomicU32; 3],
    speed: AtomicU32,
    gain_db: AtomicU32,
}
impl Default for EqParams {
    fn default() -> Self {
        Self {
            db: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            speed: AtomicU32::new(1.0_f32.to_bits()),
            gain_db: AtomicU32::new(0.0_f32.to_bits()),
        }
    }
}
impl EqParams {
    pub fn set_gain(&self, db: f32) {
        self.gain_db
            .store(db.clamp(-24.0, 12.0).to_bits(), Ordering::Relaxed);
    }
    fn gain(&self) -> f64 {
        10.0_f64.powf(f32::from_bits(self.gain_db.load(Ordering::Relaxed)) as f64 / 20.0)
    }
    pub fn set_speed(&self, speed: f32) {
        self.speed
            .store(speed.clamp(0.5, 2.0).to_bits(), Ordering::Relaxed);
    }
    fn rate(&self, source_rate: u32) -> u32 {
        (source_rate as f32 * f32::from_bits(self.speed.load(Ordering::Relaxed))).max(1000.0) as u32
    }
    pub fn set(&self, db: [f32; 3]) {
        for (target, value) in self.db.iter().zip(db) {
            target.store(value.clamp(-24.0, 12.0).to_bits(), Ordering::Relaxed);
        }
    }
    fn get(&self) -> [f64; 3] {
        std::array::from_fn(|i| f32::from_bits(self.db[i].load(Ordering::Relaxed)) as f64)
    }
}

#[derive(Clone, Copy)]
struct Coeff {
    b: [f64; 3],
    a: [f64; 2],
}
impl Coeff {
    fn make(band: usize, db: f64, rate: u32) -> Self {
        if db.abs() < 0.00001 {
            return Self {
                b: [1.0, 0.0, 0.0],
                a: [0.0; 2],
            };
        }
        let frequency = [200.0_f64, 1000.0, 4000.0][band].min(rate as f64 * 0.4);
        let omega = std::f64::consts::TAU * frequency / rate as f64;
        let a = 10.0_f64.powf(db / 40.0);
        let c = omega.cos();
        let s = omega.sin();
        let alpha = s / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let beta = 2.0 * a.sqrt() * alpha;
        let (b, denom) = match band {
            0 => (
                [
                    a * ((a + 1.0) - (a - 1.0) * c + beta),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                    a * ((a + 1.0) - (a - 1.0) * c - beta),
                ],
                [
                    (a + 1.0) + (a - 1.0) * c + beta,
                    -2.0 * ((a - 1.0) + (a + 1.0) * c),
                    (a + 1.0) + (a - 1.0) * c - beta,
                ],
            ),
            1 => (
                [1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a],
                [1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a],
            ),
            _ => (
                [
                    a * ((a + 1.0) + (a - 1.0) * c + beta),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                    a * ((a + 1.0) + (a - 1.0) * c - beta),
                ],
                [
                    (a + 1.0) - (a - 1.0) * c + beta,
                    2.0 * ((a - 1.0) - (a + 1.0) * c),
                    (a + 1.0) - (a - 1.0) * c - beta,
                ],
            ),
        };
        Self {
            b: b.map(|v| v / denom[0]),
            a: [denom[1] / denom[0], denom[2] / denom[0]],
        }
    }
}

#[derive(Clone, Copy, Default)]
struct State {
    z: [f64; 2],
}
impl State {
    fn sample(&mut self, x: f64, c: Coeff) -> f64 {
        let y = c.b[0] * x + self.z[0];
        self.z = [c.b[1] * x - c.a[0] * y + self.z[1], c.b[2] * x - c.a[1] * y];
        y
    }
}

pub struct Equalizer<S> {
    inner: S,
    params: Arc<EqParams>,
    coeff: [Coeff; 3],
    states: Vec<[State; 3]>,
    current_db: [f64; 3],
    target_db: [f64; 3],
    cursor: usize,
    frames: u64,
    gain: f64,
}
impl<S: Source<Item = f32>> Equalizer<S> {
    pub fn new(inner: S, params: Arc<EqParams>) -> Self {
        let db = params.get();
        let coeff =
            std::array::from_fn(|i| Coeff::make(i, db[i], params.rate(inner.sample_rate())));
        let states = vec![[State::default(); 3]; inner.channels() as usize];
        let gain = params.gain();
        Self {
            inner,
            params,
            coeff,
            states,
            current_db: db,
            target_db: db,
            cursor: 0,
            frames: 0,
            gain,
        }
    }
}
impl<S: Source<Item = f32>> Iterator for Equalizer<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let mut value = self.inner.next()? as f64;
        if self.cursor == 0 {
            // Update coefficients every 64 frames; smooth controls over ~20 ms.
            if self.frames.is_multiple_of(64) {
                self.target_db = self.params.get();
                let smooth = 1.0 - (-64.0 / (self.inner.sample_rate() as f64 * 0.02)).exp();
                self.gain += (self.params.gain() - self.gain) * smooth;
                for band in 0..3 {
                    self.current_db[band] +=
                        (self.target_db[band] - self.current_db[band]) * smooth;
                    self.coeff[band] = Coeff::make(
                        band,
                        self.current_db[band],
                        self.params.rate(self.inner.sample_rate()),
                    );
                }
            }
            self.frames += 1;
        }
        value *= self.gain;
        for band in 0..3 {
            value = self.states[self.cursor][band].sample(value, self.coeff[band]);
        }
        self.cursor = (self.cursor + 1) % self.states.len();
        Some(value as f32)
    }
}
impl<S: Source<Item = f32>> Source for Equalizer<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)?;
        self.states.fill([State::default(); 3]);
        self.cursor = 0;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BusGains<const N: usize = 2> {
    pub master: [f32; N],
    pub phones: [f32; N],
}

pub fn bus_gains<const N: usize>(
    volumes: [f32; N],
    cue: [bool; N],
    crossfader: f32,
    master: f32,
    phones: f32,
    mixing: f32,
) -> BusGains<N> {
    bus_gains_with_crossfader(volumes, cue, crossfader, master, phones, mixing, true)
}

pub fn bus_gains_with_crossfader<const N: usize>(
    volumes: [f32; N],
    cue: [bool; N],
    crossfader: f32,
    master: f32,
    phones: f32,
    mixing: f32,
    enabled: bool,
) -> BusGains<N> {
    let cross = if enabled {
        crate::audio::crossfader_gains(crossfader)
    } else {
        [1.0; 2]
    };
    let cue_count = cue.iter().filter(|enabled| **enabled).count().max(1);
    let cue_scale = 1.0 / (cue_count as f32).sqrt();
    let mixing = mixing.clamp(0.0, 1.0);
    let program: [f32; N] = std::array::from_fn(|i| volumes[i] * cross[i % 2]);
    BusGains {
        master: program.map(|g| g * master.clamp(0.0, 1.0)),
        phones: std::array::from_fn(|i| {
            phones.clamp(0.0, 1.0)
                * ((1.0 - mixing) * if cue[i] { cue_scale } else { 0.0 } + mixing * program[i])
        }),
    }
}

/// MIDI knob midpoint is exactly flat (value 64); range -24 to +12 dB.
pub fn knob_db(value: f32) -> f32 {
    let center = 0.5;
    if (value - center).abs() <= 0.005 {
        return 0.0;
    }
    if value <= center {
        -24.0 * (1.0 - value / center)
    } else {
        12.0 * (value - center) / (1.0 - center)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gain_changes_audio_before_eq_and_stereo_routing() {
        let params = Arc::new(EqParams::default());
        params.set_gain(6.0);
        let data = vec![0.1, -0.2, 0.3, -0.4];
        let actual: Vec<_> = Equalizer::new(
            rodio::buffer::SamplesBuffer::new(2, 48000, data.clone()),
            params,
        )
        .collect();
        let gain = 10.0_f32.powf(6.0 / 20.0);
        for (actual, sample) in actual.into_iter().zip(data) {
            assert!((actual - sample * gain).abs() < 0.00001);
        }
    }
    fn rms(frequency: f32, db: [f32; 3]) -> f32 {
        let params = Arc::new(EqParams::default());
        params.set(db);
        let samples: Vec<_> = (0..48000)
            .map(|i| (std::f32::consts::TAU * frequency * i as f32 / 48000.0).sin() * 0.1)
            .collect();
        let source = Equalizer::new(rodio::buffer::SamplesBuffer::new(1, 48000, samples), params);
        (source.skip(12000).map(|v| v * v).sum::<f32>() / 36000.0).sqrt()
    }
    #[test]
    fn eq_has_real_bass_mid_treble_cut_and_boost() {
        for (frequency, band) in [(40.0, 0), (1000.0, 1), (12000.0, 2)] {
            let flat = rms(frequency, [0.0; 3]);
            let mut cut = [0.0; 3];
            cut[band] = -24.0;
            let mut boost = [0.0; 3];
            boost[band] = 12.0;
            assert!(rms(frequency, cut) < flat * 0.13, "band {band}");
            assert!(rms(frequency, boost) > flat * 3.5, "band {band}");
        }
        assert!(knob_db(64.0 / 127.0).abs() < 0.00001);
    }
    #[test]
    fn flat_stereo_eq_does_not_change_samples_or_leak_between_channels() {
        let data = vec![0.2, 0.0, -0.5, 0.0, 0.8, 0.0];
        let actual: Vec<_> = Equalizer::new(
            rodio::buffer::SamplesBuffer::new(2, 48000, data.clone()),
            Arc::new(EqParams::default()),
        )
        .collect();
        assert_eq!(actual, data);
    }
    #[test]
    fn pre_fader_cue_is_independent_from_channel_crossfader_and_master() {
        let gains = bus_gains([0.0, 1.0], [true, false], 1.0, 0.0, 0.4, 0.0);
        assert_eq!(gains.master, [0.0, 0.0]);
        assert_eq!(gains.phones, [0.4, 0.0]);
        let program = bus_gains([0.8, 0.6], [true, false], 0.0, 0.7, 0.5, 1.0);
        assert!((program.phones[0] - program.master[0] * 0.5 / 0.7).abs() < 0.00001);
        assert_eq!(
            bus_gains([1.0; 2], [true; 2], 0.0, 0.7, 0.0, 0.5).phones,
            [0.0; 2]
        );
    }
}
