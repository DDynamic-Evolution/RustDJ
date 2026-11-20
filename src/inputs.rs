//! Live audio inputs mixed into the existing output buses.
use crate::output::{Audio, ChannelPair};
use rodio::{
    Source,
    cpal::{
        self, FromSample, SampleFormat, SizedSample, SupportedStreamConfig,
        traits::{DeviceTrait, HostTrait, StreamTrait},
    },
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct InputSettings {
    pub device: Option<String>,
    /// Zero-based first selected input channel.
    pub channel: u16,
    pub stereo: bool,
    pub level: f32,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self {
            device: None,
            channel: 0,
            stereo: false,
            level: 0.7,
        }
    }
}

pub struct DeviceInfo {
    pub name: String,
    pub channels: u16,
}

pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    let host = cpal::default_host();
    Ok(host
        .input_devices()
        .map_err(|e| e.to_string())?
        .filter_map(|device| {
            let name = device.name().ok()?;
            let channels = device
                .supported_input_configs()
                .ok()?
                .map(|config| config.channels())
                .max()?;
            Some(DeviceInfo { name, channels })
        })
        .collect())
}

const CAPACITY: usize = 8192;

struct Frames {
    samples: Box<[AtomicU64]>,
    write: AtomicUsize,
    read: AtomicUsize,
}

impl Default for Frames {
    fn default() -> Self {
        Self {
            samples: (0..CAPACITY).map(|_| AtomicU64::new(0)).collect(),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        }
    }
}

impl Frames {
    fn push(&self, frame: [f32; 2]) {
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if write.wrapping_sub(read) == CAPACITY {
            return;
        }
        let packed = u64::from(frame[0].to_bits()) | (u64::from(frame[1].to_bits()) << 32);
        self.samples[write % CAPACITY].store(packed, Ordering::Relaxed);
        self.write.store(write.wrapping_add(1), Ordering::Release);
    }

    fn next_frame(&self, rate: u32) -> [f32; 2] {
        let mut read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        let queued = write.wrapping_sub(read);
        if queued == 0 {
            return [0.0; 2];
        }
        // Keep the delay bounded when the two device clocks differ.
        let delay = (rate as usize / 20).clamp(1, CAPACITY / 2);
        if queued > delay * 2 {
            read = write.wrapping_sub(delay);
        }
        let packed = self.samples[read % CAPACITY].load(Ordering::Relaxed);
        self.read.store(read.wrapping_add(1), Ordering::Release);
        [
            f32::from_bits(packed as u32),
            f32::from_bits((packed >> 32) as u32),
        ]
    }
}

#[derive(Default)]
struct BusInput {
    frames: Frames,
    gain: AtomicU32,
}

#[derive(Default)]
struct Shared {
    master: BusInput,
    phones: Option<BusInput>,
    disconnected: AtomicBool,
    stopped: AtomicBool,
    peak: AtomicU32,
}

struct InputSource {
    shared: Arc<Shared>,
    phones: bool,
    rate: u32,
    next_right: Option<f32>,
}

impl Iterator for InputSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some(right) = self.next_right.take() {
            return Some(right);
        }
        if self.shared.stopped.load(Ordering::Relaxed)
            || self.shared.disconnected.load(Ordering::Relaxed)
        {
            return None;
        }
        let bus = if self.phones {
            self.shared.phones.as_ref()?
        } else {
            &self.shared.master
        };
        let gain = f32::from_bits(bus.gain.load(Ordering::Relaxed));
        let [left, right] = bus.frames.next_frame(self.rate);
        self.next_right = Some(right * gain);
        Some(left * gain)
    }
}

impl Source for InputSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub struct LiveInput {
    _stream: cpal::Stream,
    shared: Arc<Shared>,
}

impl LiveInput {
    pub fn open(settings: &InputSettings, audio: &Audio) -> Result<Self, String> {
        let output = audio
            .stream
            .as_ref()
            .ok_or("Select a master output before enabling inputs.")?;
        let host = cpal::default_host();
        let device = match &settings.device {
            Some(name) => host
                .input_devices()
                .map_err(|e| e.to_string())?
                .find(|device| device.name().is_ok_and(|candidate| &candidate == name))
                .ok_or_else(|| format!("Input device not found: {name}"))?,
            None => host
                .default_input_device()
                .ok_or("No default audio input is available.")?,
        };
        let needed = settings
            .channel
            .checked_add(if settings.stereo { 2 } else { 1 })
            .ok_or("Invalid input channel.")?;
        let config = input_config(&device, needed)?;
        let shared = Arc::new(Shared {
            phones: audio.bus(true).map(|_| BusInput::default()),
            ..Shared::default()
        });
        let stream = match config.sample_format() {
            SampleFormat::I8 => open_stream::<i8>,
            SampleFormat::I16 => open_stream::<i16>,
            SampleFormat::I24 => open_stream::<cpal::I24>,
            SampleFormat::I32 => open_stream::<i32>,
            SampleFormat::I64 => open_stream::<i64>,
            SampleFormat::U8 => open_stream::<u8>,
            SampleFormat::U16 => open_stream::<u16>,
            SampleFormat::U32 => open_stream::<u32>,
            SampleFormat::U64 => open_stream::<u64>,
            SampleFormat::F32 => open_stream::<f32>,
            SampleFormat::F64 => open_stream::<f64>,
            format => return Err(format!("Unsupported input sample format: {format}")),
        }(&device, &config, settings, shared.clone())?;
        stream
            .play()
            .map_err(|e| format!("Could not start audio input: {e}"))?;
        for (phones, channels, pair) in [
            (
                false,
                output.config().channel_count(),
                audio.routing.master.pair,
            ),
            (
                true,
                audio.phones().map_or(0, |out| out.config().channel_count()),
                audio.routing.phones.as_ref().map_or(0, |out| out.pair),
            ),
        ] {
            if let Some(bus) = audio.bus(phones) {
                let source = InputSource {
                    shared: shared.clone(),
                    phones,
                    rate: config.sample_rate().0,
                    next_right: None,
                };
                bus.add(ChannelPair::new(source, channels, pair));
            }
        }
        Ok(Self {
            _stream: stream,
            shared,
        })
    }

    pub fn set_gain(&self, master: f32, phones: f32) {
        self.shared
            .master
            .gain
            .store(master.to_bits(), Ordering::Relaxed);
        if let Some(bus) = &self.shared.phones {
            bus.gain.store(phones.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.shared.peak.swap(0, Ordering::Relaxed))
    }

    pub fn disconnected(&self) -> bool {
        self.shared.disconnected.load(Ordering::Relaxed)
    }
}

impl Drop for LiveInput {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Relaxed);
    }
}

fn input_config(device: &cpal::Device, needed: u16) -> Result<SupportedStreamConfig, String> {
    if let Ok(config) = device.default_input_config()
        && config.channels() >= needed
    {
        return Ok(config);
    }
    device
        .supported_input_configs()
        .map_err(|e| e.to_string())?
        .filter(|config| config.channels() >= needed)
        .min_by_key(|config| config.channels())
        .map(|config| {
            let rate = cpal::SampleRate(
                48_000_u32.clamp(config.min_sample_rate().0, config.max_sample_rate().0),
            );
            config.with_sample_rate(rate)
        })
        .ok_or_else(|| format!("Input device does not provide channel {needed}."))
}

fn open_stream<T>(
    device: &cpal::Device,
    config: &SupportedStreamConfig,
    settings: &InputSettings,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels());
    let left = usize::from(settings.channel);
    let right = left + usize::from(settings.stereo);
    let error_state = shared.clone();
    device
        .build_input_stream(
            &config.config(),
            move |data: &[T], _| {
                for frame in data.chunks_exact(channels) {
                    let samples = [
                        frame[left].to_sample::<f32>(),
                        frame[right].to_sample::<f32>(),
                    ]
                    .map(|x| {
                        if x.is_finite() {
                            x.clamp(-1.0, 1.0)
                        } else {
                            0.0
                        }
                    });
                    shared.peak.fetch_max(
                        samples[0].abs().max(samples[1].abs()).to_bits(),
                        Ordering::Relaxed,
                    );
                    shared.master.frames.push(samples);
                    if let Some(phones) = &shared.phones {
                        phones.frames.push(samples);
                    }
                }
            },
            move |_| error_state.disconnected.store(true, Ordering::Relaxed),
            None,
        )
        .map_err(|e| format!("Could not open audio input: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_source_preserves_stereo_and_skips_stale_frames() {
        let shared = Arc::new(Shared::default());
        shared
            .master
            .gain
            .store(0.5_f32.to_bits(), Ordering::Relaxed);
        let mut source = InputSource {
            shared: shared.clone(),
            phones: false,
            rate: 48_000,
            next_right: None,
        };
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.next(), Some(0.0));
        shared.master.frames.push([0.8, -0.4]);
        assert_eq!(source.next(), Some(0.4));
        assert_eq!(source.next(), Some(-0.2));

        for index in 0..CAPACITY {
            shared.master.frames.push([index as f32, 0.0]);
        }
        assert_eq!(source.next(), Some((CAPACITY - 2400) as f32 * 0.5));
        assert_eq!(source.next(), Some(0.0));
        shared.stopped.store(true, Ordering::Relaxed);
        assert_eq!(source.next(), None);
    }
}
