//! Signed, frequency-coloured waveform bins and peak-preserving resolution levels.
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use std::io::{Read, Write};

pub const MAX_FRAMES: usize = 2_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub min: i16,
    pub max: i16,
    pub bands: [u8; 3],
}
impl Frame {
    pub fn bounds(self) -> (f32, f32) {
        (self.min as f32 / 32767.0, self.max as f32 / 32767.0)
    }
    pub fn merge(self, other: Self) -> Self {
        let weight = |f: Self| {
            (f.min.unsigned_abs().max(f.max.unsigned_abs()) as f64)
                .powi(2)
                .max(1.0)
        };
        let (a, b) = (weight(self), weight(other));
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
            bands: std::array::from_fn(|i| {
                ((self.bands[i] as f64 * a + other.bands[i] as f64 * b) / (a + b)).round() as u8
            }),
        }
    }
}

#[derive(Debug)]
pub struct Waveform {
    pub step: f64,
    levels: Vec<Vec<Frame>>,
}
impl Waveform {
    pub fn new(frames: Vec<Frame>, step: f64) -> Self {
        let mut levels = vec![frames];
        while levels.last().is_some_and(|l| l.len() > 1) {
            let next = levels
                .last()
                .unwrap()
                .chunks(2)
                .map(|p| if p.len() == 2 { p[0].merge(p[1]) } else { p[0] })
                .collect();
            levels.push(next);
        }
        Self { step, levels }
    }
    pub fn frames(&self) -> &[Frame] {
        &self.levels[0]
    }
    pub fn sample(&self, start: f64, end: f64) -> Option<Frame> {
        if !start.is_finite() || !end.is_finite() || end <= 0.0 || end <= start {
            return None;
        }
        let mut first = (start.max(0.0) / self.step).floor() as usize;
        let last = (end / self.step).ceil().min(self.frames().len() as f64) as usize;
        if first >= last {
            return None;
        }
        let mut result: Option<Frame> = None;
        while first < last {
            let remaining = last - first;
            let size = (usize::BITS - 1 - remaining.leading_zeros()).min(if first == 0 {
                usize::BITS - 1
            } else {
                first.trailing_zeros()
            });
            let frame = self.levels[size as usize][first >> size];
            result = Some(result.map_or(frame, |r| r.merge(frame)));
            first += 1 << size;
        }
        result
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        for frame in self.frames() {
            encoder
                .write_all(&frame.min.to_le_bytes())
                .map_err(|e| e.to_string())?;
            encoder
                .write_all(&frame.max.to_le_bytes())
                .map_err(|e| e.to_string())?;
            encoder.write_all(&frame.bands).map_err(|e| e.to_string())?;
        }
        encoder.finish().map_err(|e| e.to_string())
    }
    pub fn decode(bytes: &[u8], count: usize, step: f64) -> Option<Self> {
        if count == 0 || count > MAX_FRAMES || !step.is_finite() || step <= 0.0 {
            return None;
        }
        let mut raw = Vec::new();
        ZlibDecoder::new(bytes)
            .take((count * 7 + 1) as u64)
            .read_to_end(&mut raw)
            .ok()?;
        if raw.len() != count * 7 {
            return None;
        }
        let frames: Option<Vec<_>> = raw
            .as_chunks::<7>()
            .0
            .iter()
            .map(|b| {
                let (min, max) = (
                    i16::from_le_bytes([b[0], b[1]]),
                    i16::from_le_bytes([b[2], b[3]]),
                );
                (min <= max && min != i16::MIN).then_some(Frame {
                    min,
                    max,
                    bands: [b[4], b[5], b[6]],
                })
            })
            .collect();
        Some(Self::new(frames?, step))
    }
}

#[derive(Clone, Copy)]
pub struct Filter {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}
impl Filter {
    pub fn retune(&mut self, rate: f64, frequency: f64, high: bool) {
        let z = self.z;
        *self = Self::new(rate, frequency, high);
        self.z = z;
    }
    pub fn clear(&mut self) {
        self.z = [0.0; 2];
    }
    pub fn new(rate: f64, frequency: f64, high: bool) -> Self {
        let rate = rate.max(1.0);
        let omega =
            std::f64::consts::TAU * frequency.clamp(10.0_f64.min(rate * 0.45), rate * 0.45) / rate;
        let (s, c) = omega.sin_cos();
        let alpha = s / std::f64::consts::SQRT_2;
        let b = if high {
            [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0]
        } else {
            [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0]
        };
        let denominator = 1.0 + alpha;
        Self {
            b: b.map(|v| v / denominator),
            a: [-2.0 * c / denominator, (1.0 - alpha) / denominator],
            z: [0.0; 2],
        }
    }
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z = [
            self.b[1] * x - self.a[0] * y + self.z[1],
            self.b[2] * x - self.a[1] * y,
        ];
        y
    }
}

pub struct Builder {
    filters: Vec<[Filter; 4]>,
    frames: Vec<Frame>,
    bucket: usize,
    count: usize,
    cursor: usize,
    min: f32,
    max: f32,
    energy: [f64; 3],
    step: f64,
}
impl Builder {
    pub fn new(rate: u32, channels: u16) -> Self {
        let filters = [
            Filter::new(rate as f64, 250.0, false),
            Filter::new(rate as f64, 250.0, true),
            Filter::new(rate as f64, 4000.0, false),
            Filter::new(rate as f64, 4000.0, true),
        ];
        let frames = (rate as usize / 1000).max(1);
        Self {
            filters: vec![filters; channels as usize],
            frames: Vec::new(),
            bucket: frames * channels as usize,
            count: 0,
            cursor: 0,
            min: 1.0,
            max: -1.0,
            energy: [0.0; 3],
            step: frames as f64 / rate as f64,
        }
    }
    pub fn push(&mut self, sample: f32) {
        let sample = if sample.is_finite() { sample } else { 0.0 };
        self.min = self.min.min(sample.clamp(-1.0, 1.0));
        self.max = self.max.max(sample.clamp(-1.0, 1.0));
        let filters = &mut self.filters[self.cursor];
        let low = filters[0].process(sample as f64);
        let mid = filters[1].process(sample as f64);
        let mid = filters[2].process(mid);
        let high = filters[3].process(sample as f64);
        for (energy, value) in self.energy.iter_mut().zip([low, mid, high]) {
            *energy += value * value;
        }
        self.cursor = (self.cursor + 1) % self.filters.len();
        self.count += 1;
        if self.count >= self.bucket {
            self.flush();
        }
    }
    fn flush(&mut self) {
        if self.count == 0 {
            return;
        }
        let total = self.energy.iter().sum::<f64>();
        self.frames.push(Frame {
            min: (self.min * 32767.0).round() as i16,
            max: (self.max * 32767.0).round() as i16,
            bands: self.energy.map(|v| {
                if total > 1e-14 {
                    (v / total * 255.0).round() as u8
                } else {
                    0
                }
            }),
        });
        self.count = 0;
        self.min = 1.0;
        self.max = -1.0;
        self.energy = [0.0; 3];
        if self.frames.len() >= MAX_FRAMES {
            self.frames = self.frames.chunks(2).map(|p| p[0].merge(p[1])).collect();
            self.bucket *= 2;
            self.step *= 2.0;
        }
    }
    pub fn finish(mut self) -> Waveform {
        self.flush();
        Waveform::new(self.frames, self.step)
    }
}
