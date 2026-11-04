//! Deck filter and three stereo-independent, smoothed audio effects.
use crate::waveform::Filter;
use rodio::{Source, source::SeekError};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU32, Ordering},
    },
    time::Duration,
};

pub struct Params {
    filter: AtomicU32,
    level: AtomicU32,
    enabled: AtomicU8,
    speed: AtomicU32,
    bpm: AtomicU32,
}
impl Default for Params {
    fn default() -> Self {
        Self {
            filter: AtomicU32::new(0),
            level: AtomicU32::new(0.25_f32.to_bits()),
            enabled: AtomicU8::new(0),
            speed: AtomicU32::new(1.0_f32.to_bits()),
            bpm: AtomicU32::new(120.0_f32.to_bits()),
        }
    }
}
impl Params {
    pub fn set(&self, filter: f32, level: f32, enabled: [bool; 3], speed: f32, bpm: f32) {
        self.filter
            .store(filter.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
        self.level
            .store(level.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.enabled.store(
            enabled
                .iter()
                .enumerate()
                .fold(0, |m, (i, on)| m | (u8::from(*on) << i)),
            Ordering::Relaxed,
        );
        self.speed
            .store(speed.clamp(0.5, 2.0).to_bits(), Ordering::Relaxed);
        self.bpm
            .store(bpm.clamp(40.0, 250.0).to_bits(), Ordering::Relaxed);
    }
}

struct Delay {
    samples: Vec<f64>,
    index: usize,
    valid: usize,
}
impl Delay {
    fn new(count: usize) -> Self {
        Self {
            samples: vec![0.0; count.max(2)],
            index: 0,
            valid: 0,
        }
    }
    fn read(&self, delay: f64) -> f64 {
        let delay = delay.clamp(1.0, (self.samples.len() - 1) as f64);
        if self.valid < delay.ceil() as usize {
            return 0.0;
        }
        let whole = delay.floor() as usize;
        let first = (self.index + self.samples.len() - whole) % self.samples.len();
        let second = (first + self.samples.len() - 1) % self.samples.len();
        self.samples[first] * (1.0 - delay.fract()) + self.samples[second] * delay.fract()
    }
    fn push(&mut self, value: f64) {
        self.samples[self.index] = value;
        self.index = (self.index + 1) % self.samples.len();
        self.valid = (self.valid + 1).min(self.samples.len());
    }
    fn reset(&mut self) {
        self.index = 0;
        self.valid = 0;
    }
}
struct Channel {
    filter: Filter,
    flange: Delay,
    echo: Delay,
    comb: [Delay; 4],
    damping: [f64; 4],
}
pub struct Effects<S> {
    inner: S,
    params: Arc<Params>,
    channels: Vec<Channel>,
    cursor: usize,
    frames: u64,
    filter: f64,
    mix: [f64; 3],
    enabled: u8,
    phase: f64,
    rate: f64,
    echo_frames: f64,
}
impl<S: Source<Item = f32>> Effects<S> {
    pub fn new(inner: S, params: Arc<Params>) -> Self {
        let rate = inner.sample_rate() as f64;
        let channels = (0..inner.channels())
            .map(|_| Channel {
                filter: Filter::new(rate, 20000.0, false),
                flange: Delay::new((rate * 0.03) as usize),
                echo: Delay::new((rate * 1.6) as usize),
                comb: std::array::from_fn(|_| Delay::new((rate * 0.12) as usize)),
                damping: [0.0; 4],
            })
            .collect();
        Self {
            inner,
            params,
            channels,
            cursor: 0,
            frames: 0,
            filter: 0.0,
            mix: [0.0; 3],
            enabled: 0,
            phase: 0.0,
            rate,
            echo_frames: rate * 0.25,
        }
    }
}
impl<S: Source<Item = f32>> Iterator for Effects<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let input = self.inner.next()? as f64;
        if self.cursor == 0 {
            if self.frames.is_multiple_of(64) {
                let speed = f32::from_bits(self.params.speed.load(Ordering::Relaxed)) as f64;
                let rate = self.inner.sample_rate() as f64 * speed;
                let smooth = 1.0 - (-64.0 / (self.inner.sample_rate() as f64 * 0.02)).exp();
                self.filter += (f32::from_bits(self.params.filter.load(Ordering::Relaxed)) as f64
                    - self.filter)
                    * smooth;
                let level = f32::from_bits(self.params.level.load(Ordering::Relaxed)) as f64;
                let enabled = self.params.enabled.load(Ordering::Relaxed);
                for slot in 0..3 {
                    self.mix[slot] += ((if enabled & (1 << slot) != 0 {
                        level
                    } else {
                        0.0
                    }) - self.mix[slot])
                        * smooth;
                    if enabled & !self.enabled & (1 << slot) != 0 {
                        for channel in &mut self.channels {
                            match slot {
                                0 => channel.flange.reset(),
                                1 => {
                                    for c in &mut channel.comb {
                                        c.reset();
                                    }
                                    channel.damping = [0.0; 4];
                                }
                                _ => channel.echo.reset(),
                            }
                        }
                    }
                }
                self.enabled = enabled;
                let high = self.filter > 0.0;
                let cutoff = if high {
                    20.0 * 500.0_f64.powf(self.filter)
                } else {
                    20000.0 * 0.004_f64.powf(-self.filter)
                };
                for channel in &mut self.channels {
                    channel.filter.retune(rate, cutoff, high);
                }
                self.rate = rate;
                self.echo_frames = self.inner.sample_rate() as f64 * 30.0
                    / f32::from_bits(self.params.bpm.load(Ordering::Relaxed)) as f64;
            }
            self.frames += 1;
            self.phase = (self.phase + 0.25 / self.rate).fract();
        }
        let channel = &mut self.channels[self.cursor];
        let mut value = input;
        if self.filter.abs() > 0.015 {
            value = channel.filter.process(value);
        }
        if self.mix[0] > 0.00001 {
            let delayed = channel.flange.read(
                self.rate
                    * (0.002 + 0.004 * (0.5 + 0.5 * (self.phase * std::f64::consts::TAU).sin())),
            );
            channel.flange.push(value + delayed * 0.25);
            value = value * (1.0 - self.mix[0] * 0.5) + delayed * self.mix[0] * 0.5;
        }
        if self.mix[1] > 0.00001 {
            let mut wet = 0.0;
            for (i, time) in [0.0297, 0.0371, 0.0411, 0.0437].into_iter().enumerate() {
                let delayed = channel.comb[i].read(self.rate * time);
                channel.damping[i] += 0.25 * (delayed - channel.damping[i]);
                channel.comb[i].push(value + channel.damping[i] * [0.69, 0.72, 0.73, 0.75][i]);
                wet += delayed * 0.25;
            }
            value = value * (1.0 - self.mix[1] * 0.4) + wet * self.mix[1] * 0.4;
        }
        if self.mix[2] > 0.00001 {
            let delayed = channel.echo.read(self.echo_frames);
            channel.echo.push(value + delayed * 0.45);
            value += delayed * self.mix[2] * 0.55;
        }
        self.cursor = (self.cursor + 1) % self.channels.len();
        Some(value as f32)
    }
}
impl<S: Source<Item = f32>> Source for Effects<S> {
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
    fn try_seek(&mut self, at: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(at)?;
        for channel in &mut self.channels {
            channel.filter.clear();
            channel.flange.reset();
            channel.echo.reset();
            for c in &mut channel.comb {
                c.reset();
            }
            channel.damping = [0.0; 4];
        }
        self.cursor = 0;
        Ok(())
    }
}
