use rodio::{
    OutputStream, OutputStreamBuilder, Source,
    cpal::{
        self,
        traits::{DeviceTrait, HostTrait},
    },
    mixer::{self, Mixer, MixerSource},
    source::{SeekError, UniformSourceIterator},
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
struct StereoPeaks {
    values: [AtomicU32; 2],
}

impl StereoPeaks {
    fn take(&self) -> [f32; 2] {
        std::array::from_fn(|i| f32::from_bits(self.values[i].swap(0, Ordering::Relaxed)))
    }
}

struct MeteredBus {
    source: MixerSource,
    peaks: Arc<StereoPeaks>,
    channel: u16,
    pair: u16,
    capture: Option<Arc<crate::capture::MasterCapture>>,
    frame: [f32; 2],
    equalizer: Option<crate::equalizer::Processor>,
}
impl Iterator for MeteredBus {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let mut value = self.source.next()?;
        if let Some(eq) = &mut self.equalizer
            && (self.pair * 2..self.pair * 2 + 2).contains(&self.channel)
        {
            value = eq.process(value, (self.channel - self.pair * 2) as usize);
        }
        if value.is_finite() && (self.pair * 2..self.pair * 2 + 2).contains(&self.channel) {
            self.peaks.values[(self.channel - self.pair * 2) as usize]
                .fetch_max(value.abs().to_bits(), Ordering::Relaxed);
        }
        if let Some(capture) = &self.capture {
            if self.channel == self.pair * 2 {
                self.frame[0] = value;
            } else if self.channel == self.pair * 2 + 1 {
                self.frame[1] = value;
                capture.push(self.frame);
            }
        }
        self.channel = (self.channel + 1) % self.source.channels();
        Some(value)
    }
}
impl Source for MeteredBus {
    fn current_span_len(&self) -> Option<usize> {
        self.source.current_span_len()
    }
    fn channels(&self) -> u16 {
        self.source.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
    fn try_seek(&mut self, at: Duration) -> Result<(), SeekError> {
        self.source.try_seek(at)
    }
}

fn metered_bus(
    channels: u16,
    rate: u32,
    pair: u16,
    peaks: Arc<StereoPeaks>,
) -> (Mixer, MeteredBus) {
    let (mixer, source) = mixer::mixer(channels, rate);
    // Keep the submix connected when no deck has been loaded yet.
    mixer.add(rodio::source::Zero::new(channels, rate));
    (
        mixer,
        MeteredBus {
            source,
            peaks,
            channel: 0,
            pair,
            capture: None,
            frame: [0.0; 2],
            equalizer: None,
        },
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Target {
    /// None selects the system default, never an arbitrary fallback device.
    pub device: Option<String>,
    /// Zero-based stereo pair: 0 = channels 1/2, 1 = 3/4.
    pub pair: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Routing {
    pub master: Target,
    /// None explicitly disables headphone output.
    pub phones: Option<Target>,
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub channels: u16,
}

pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    let host = cpal::default_host();
    host.output_devices()
        .map_err(|e| e.to_string())?
        .map(|device| {
            let name = device.name().map_err(|e| e.to_string())?;
            let channels = device
                .supported_output_configs()
                .map_err(|e| e.to_string())?
                .map(|c| c.channels())
                .max()
                .unwrap_or(2);
            Ok(DeviceInfo { name, channels })
        })
        .collect()
}

pub struct Audio {
    pub stream: Option<OutputStream>,
    phones_stream: Option<OutputStream>,
    shared: bool,
    pub error: Option<String>,
    pub routing: Routing,
    pub master_name: String,
    pub phones_name: Option<String>,
    master_bus: Option<Mixer>,
    phones_bus: Option<Mixer>,
    master_peaks: Arc<StereoPeaks>,
    phones_peaks: Arc<StereoPeaks>,
    pub capture: Arc<crate::capture::MasterCapture>,
    pub master_eq: Arc<crate::equalizer::Params>,
}

impl Default for Audio {
    fn default() -> Self {
        Self::new(Routing::default())
    }
}

fn resolve(target: &Target) -> Result<(cpal::Device, String), String> {
    let host = cpal::default_host();
    let device = match &target.device {
        Some(name) => host
            .output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().is_ok_and(|n| &n == name))
            .ok_or_else(|| format!("Audiogerät nicht gefunden: {name}"))?,
        None => host
            .default_output_device()
            .ok_or("Kein Standard-Audioausgang verfügbar.")?,
    };
    let name = device.name().map_err(|e| e.to_string())?;
    Ok((device, name))
}

fn open(device: cpal::Device, required_channels: u16) -> Result<OutputStream, String> {
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let mut builder =
        OutputStreamBuilder::from_device(device.clone()).map_err(|e| e.to_string())?;
    if config.channels() < required_channels {
        let range = device
            .supported_output_configs()
            .map_err(|e| e.to_string())?
            .filter(|c| c.channels() >= required_channels)
            .min_by_key(|c| c.channels())
            .ok_or_else(|| {
                format!(
                    "Gerät bietet kein Kanalpaar {}/{}.",
                    required_channels - 1,
                    required_channels
                )
            })?;
        let rate = 48000_u32.clamp(range.min_sample_rate().0, range.max_sample_rate().0);
        builder = builder.with_supported_config(&range.with_sample_rate(cpal::SampleRate(rate)));
    }
    let mut stream = builder
        .open_stream()
        .map_err(|e| format!("Audioausgang kann nicht geöffnet werden: {e}"))?;
    stream.log_on_drop(false);
    Ok(stream)
}

pub fn validate_pairs(
    same_device: bool,
    master_pair: u16,
    phones_pair: Option<u16>,
) -> Result<(), String> {
    if master_pair > 15 || phones_pair.is_some_and(|pair| pair > 15) {
        return Err("Ungültiges Kanalpaar.".into());
    }
    if same_device && phones_pair == Some(master_pair) {
        return Err("Master und Kopfhörer dürfen auf demselben Gerät nicht dasselbe Kanalpaar verwenden. Anderes Gerät oder z. B. Kanäle 3/4 wählen.".into());
    }
    Ok(())
}

impl Audio {
    pub fn new(routing: Routing) -> Self {
        match Self::open(routing.clone()) {
            Ok(audio) => audio,
            Err(error) => Self {
                stream: None,
                phones_stream: None,
                shared: false,
                error: Some(error),
                routing,
                master_name: "Offline".into(),
                phones_name: None,
                master_bus: None,
                phones_bus: None,
                master_peaks: Arc::default(),
                phones_peaks: Arc::default(),
                capture: Arc::new(crate::capture::MasterCapture::new(44100)),
                master_eq: Arc::default(),
            },
        }
    }

    pub fn open(routing: Routing) -> Result<Self, String> {
        let (master_device, master_name) = resolve(&routing.master)?;
        let phones = routing.phones.as_ref().map(resolve).transpose()?;
        let shared = phones
            .as_ref()
            .is_some_and(|(_, name)| *name == master_name);
        validate_pairs(
            shared,
            routing.master.pair,
            routing.phones.as_ref().map(|p| p.pair),
        )?;
        let channels = (routing.master.pair + 1) * 2;
        let channels = if shared {
            channels.max((routing.phones.as_ref().unwrap().pair + 1) * 2)
        } else {
            channels
        };
        let stream = open(master_device, channels)?;
        let phones_name = phones.as_ref().map(|(_, name)| name.clone());
        let phones_stream = if shared {
            None
        } else {
            phones
                .map(|(device, _)| open(device, (routing.phones.as_ref().unwrap().pair + 1) * 2))
                .transpose()?
        };
        let master_peaks = Arc::new(StereoPeaks::default());
        let phones_peaks = Arc::new(StereoPeaks::default());
        let capture = Arc::new(crate::capture::MasterCapture::new(
            stream.config().sample_rate(),
        ));
        let (master_bus, mut master_source) = metered_bus(
            stream.config().channel_count(),
            stream.config().sample_rate(),
            routing.master.pair,
            master_peaks.clone(),
        );
        master_source.capture = Some(capture.clone());
        let master_eq = Arc::new(crate::equalizer::Params::default());
        master_source.equalizer = Some(crate::equalizer::Processor::new(
            master_eq.clone(),
            stream.config().sample_rate(),
        ));
        stream.mixer().add(master_source);
        let phones_output = if shared {
            Some(&stream)
        } else {
            phones_stream.as_ref()
        };
        let phones_bus = match (phones_output, &routing.phones) {
            (Some(output), Some(target)) => {
                let (bus, source) = metered_bus(
                    output.config().channel_count(),
                    output.config().sample_rate(),
                    target.pair,
                    phones_peaks.clone(),
                );
                output.mixer().add(source);
                Some(bus)
            }
            _ => None,
        };
        Ok(Self {
            stream: Some(stream),
            phones_stream,
            shared,
            error: None,
            routing,
            master_name,
            phones_name,
            master_bus: Some(master_bus),
            phones_bus,
            master_peaks,
            phones_peaks,
            capture,
            master_eq,
        })
    }

    pub fn phones(&self) -> Option<&OutputStream> {
        if self.shared {
            self.stream.as_ref()
        } else {
            self.phones_stream.as_ref()
        }
    }

    pub fn bus(&self, phones: bool) -> Option<&Mixer> {
        if phones {
            self.phones_bus.as_ref()
        } else {
            self.master_bus.as_ref()
        }
    }
    pub fn levels(&self) -> ([f32; 2], [f32; 2]) {
        (self.master_peaks.take(), self.phones_peaks.take())
    }
}

/// Maps stereo to an explicit channel pair. Other output channels stay silent.
pub struct ChannelPair<S: Source> {
    inner: UniformSourceIterator<S>,
    channels: u16,
    pair: u16,
    cursor: u16,
    frame: [f32; 2],
}

impl<S: Source<Item = f32>> ChannelPair<S> {
    pub fn new(inner: S, channels: u16, pair: u16) -> Self {
        assert!(channels >= 2 && pair < channels / 2);
        let rate = inner.sample_rate();
        Self {
            inner: UniformSourceIterator::new(inner, 2, rate),
            channels,
            pair,
            cursor: 0,
            frame: [0.0; 2],
        }
    }
}

impl<S: Source<Item = f32>> Iterator for ChannelPair<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.cursor == 0 {
            self.frame = [self.inner.next()?, self.inner.next().unwrap_or(0.0)];
        }
        let value = if self.cursor / 2 == self.pair {
            self.frame[(self.cursor % 2) as usize]
        } else {
            0.0
        };
        self.cursor = (self.cursor + 1) % self.channels;
        Some(value)
    }
}

impl<S: Source<Item = f32>> Source for ChannelPair<S> {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(position)?;
        self.cursor = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourdeck17_master_equalizer_response_capture_recording_and_saved_settings() {
        use crate::{
            equalizer::{FREQUENCIES, Params, Processor, Settings as EqSettings},
            streamer::{Broadcaster, Phase, StreamSettings},
        };
        use std::{path::Path, thread, time::Instant};
        fn rms(samples: &[f32]) -> f32 {
            (samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32).sqrt()
        }
        for (band, frequency) in FREQUENCIES.into_iter().enumerate() {
            let params = Arc::new(Params::default());
            let mut settings = EqSettings {
                enabled: true,
                ..EqSettings::default()
            };
            settings.bands[band] = 6.0;
            params.set(settings);
            let mut eq = Processor::new(params.clone(), 48000);
            let mut input = Vec::new();
            let mut output = Vec::new();
            for i in 0..48000 {
                let sample = (std::f32::consts::TAU * frequency * i as f32 / 48000.0).sin() * 0.02;
                let filtered = eq.process(sample, 0);
                assert_eq!(eq.process(0.0, 1), 0.0);
                if i >= 24000 {
                    input.push(sample);
                    output.push(filtered);
                }
            }
            assert!(
                (rms(&output) / rms(&input) - 10.0_f32.powf(6.0 / 20.0)).abs() < 0.03,
                "band {band}"
            );
            settings.enabled = false;
            params.set(settings);
            for _ in 0..24000 {
                assert!(eq.process(0.1, 0).is_finite());
                eq.process(0.0, 1);
            }
            assert_eq!(eq.process(0.1234, 0), 0.1234);
        }
        let directory = tempfile::Builder::new()
            .prefix("fourdeck17-")
            .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("work"))
            .unwrap();
        let mut saved = crate::settings::Settings::default();
        saved.master_eq.enabled = true;
        saved.master_eq.preamp = -3.0;
        saved.master_eq.bands[5] = 6.0;
        let settings_path = directory.path().join("settings.json");
        crate::settings::save_to(&settings_path, &saved).unwrap();
        let (restored, error) = crate::settings::load_from(&settings_path);
        assert!(error.is_none());
        assert_eq!(restored.master_eq, saved.master_eq);
        let params = Arc::new(Params::default());
        params.set(restored.master_eq);
        let capture = Arc::new(crate::capture::MasterCapture::new(48000));
        let (master, mut source) = metered_bus(4, 48000, 1, Arc::default());
        source.capture = Some(capture.clone());
        source.equalizer = Some(Processor::new(params, 48000));
        let gains =
            crate::dsp::bus_gains_with_crossfader([0.8; 4], [false; 4], 0.0, 0.5, 1.0, 0.0, false);
        for gain in gains.master {
            let samples: Vec<_> = (0..48000)
                .flat_map(|i| {
                    let signal =
                        (std::f32::consts::TAU * 1000.0 * i as f32 / 48000.0).sin() * 0.02 * gain;
                    [0.8, -0.8, signal, 0.0]
                })
                .collect();
            master.add(rodio::buffer::SamplesBuffer::new(4, 48000, samples));
        }
        let mut broadcaster = Broadcaster::default();
        let stream_settings = StreamSettings {
            recording_dir: directory.path().into(),
            ..StreamSettings::default()
        };
        broadcaster
            .start_recording(capture.clone(), &stream_settings)
            .unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while broadcaster.recording_status.phase != Phase::Running {
            broadcaster.poll();
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(2));
        }
        capture.stream.begin();
        let rendered: Vec<_> = source.by_ref().take(48000 * 4).collect();
        capture.stream.stop();
        let mut captured = Vec::new();
        assert_eq!(capture.stream.drain(&mut captured, 48000), 48000);
        for (output, frame) in rendered
            .as_chunks::<4>()
            .0
            .iter()
            .zip(captured.as_chunks::<2>().0)
        {
            assert_eq!(&output[2..], frame);
            assert_eq!(frame[1], 0.0);
        }
        let left: Vec<_> = captured.iter().skip(24000).step_by(2).copied().collect();
        let expected_rms = gains.master.iter().sum::<f32>()
            * 0.02
            * 10.0_f32.powf(3.0 / 20.0)
            * std::f32::consts::FRAC_1_SQRT_2;
        assert!((rms(&left) - expected_rms).abs() < 0.001);
        broadcaster.stop_recording();
        let end = Instant::now() + Duration::from_secs(5);
        while broadcaster.recording_busy() {
            broadcaster.poll();
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(broadcaster.recording_status.phase, Phase::Done);
        let decoder =
            crate::track::open_decoder(broadcaster.recording_status.path.as_ref().unwrap())
                .unwrap();
        assert_eq!(decoder.sample_rate(), 44100);
        assert_eq!(decoder.channels(), 2);
        let decoded: Vec<_> = decoder.collect();
        let left: Vec<_> = decoded
            .iter()
            .skip(12000)
            .take(50000)
            .step_by(2)
            .copied()
            .collect();
        assert!((rms(&left) - expected_rms).abs() < 0.006);
        let legacy: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.master_eq, EqSettings::default());
    }

    #[test]
    fn streamer16_master_capture_records_real_mp3_without_phones_or_channel_leakage() {
        use crate::{
            capture::{MasterCapture, PcmBuffer},
            streamer::{Broadcaster, Phase, StreamSettings},
        };
        use std::{path::Path, thread, time::Instant};
        fn wait(mut condition: impl FnMut() -> bool) {
            let end = Instant::now() + Duration::from_secs(5);
            while !condition() {
                assert!(Instant::now() < end, "recording worker timeout");
                thread::sleep(Duration::from_millis(2));
            }
        }
        let directory = tempfile::Builder::new()
            .prefix("streamer16-")
            .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("work"))
            .unwrap();
        let settings = StreamSettings {
            recording_dir: directory.path().into(),
            ..StreamSettings::default()
        };
        let capture = Arc::new(MasterCapture::new(48000));
        let (master, mut master_source) = metered_bus(4, 48000, 1, Arc::default());
        master_source.capture = Some(capture.clone());
        let (phones, mut phones_source) = metered_bus(4, 48000, 0, Arc::default());
        let gains = crate::dsp::bus_gains_with_crossfader(
            [0.8, 0.6],
            [false, true],
            0.0,
            0.5,
            1.0,
            0.0,
            true,
        );
        let mut a = Vec::new();
        let mut b = Vec::new();
        for i in 0..48000 {
            let left =
                (std::f32::consts::TAU * 1000.0 * i as f32 / 48000.0).sin() * gains.master[0];
            let right =
                (std::f32::consts::TAU * 2000.0 * i as f32 / 48000.0).sin() * gains.master[1];
            a.extend_from_slice(&[0.8, -0.8, left, 0.0]);
            b.extend_from_slice(&[0.0, 0.0, 0.0, right]);
        }
        master.add(rodio::buffer::SamplesBuffer::new(4, 48000, a));
        master.add(rodio::buffer::SamplesBuffer::new(4, 48000, b));
        phones.add(rodio::buffer::SamplesBuffer::new(
            4,
            48000,
            vec![0.95; 48000 * 4],
        ));
        let mut broadcaster = Broadcaster::default();
        broadcaster
            .start_recording(capture.clone(), &settings)
            .unwrap();
        wait(|| {
            broadcaster.poll();
            broadcaster.recording_status.phase == Phase::Running
        });
        capture.stream.begin();
        for _ in 0..48000 * 4 {
            master_source.next().unwrap();
            phones_source.next().unwrap();
        }
        capture.stream.stop();
        let mut captured = Vec::new();
        assert_eq!(capture.stream.drain(&mut captured, 48000), 48000);
        for (i, frame) in captured.as_chunks::<2>().0.iter().enumerate() {
            let expected_left =
                (std::f32::consts::TAU * 1000.0 * i as f32 / 48000.0).sin() * gains.master[0];
            let expected_right =
                (std::f32::consts::TAU * 2000.0 * i as f32 / 48000.0).sin() * gains.master[1];
            assert!((frame[0] - expected_left).abs() < 0.00001);
            assert!((frame[1] - expected_right).abs() < 0.00001);
        }
        broadcaster.stop_recording();
        wait(|| {
            broadcaster.poll();
            !broadcaster.recording_busy()
        });
        assert_eq!(
            broadcaster.recording_status.phase,
            Phase::Done,
            "{:?}",
            broadcaster.recording_status
        );
        assert!((broadcaster.recording_status.seconds - 1.0).abs() < 0.00001);
        let path = broadcaster.recording_status.path.clone().unwrap();
        let decoder = crate::track::open_decoder(&path).unwrap();
        assert_eq!(decoder.channels(), 2);
        assert_eq!(decoder.sample_rate(), 44100);
        let decoded: Vec<_> = decoder.collect();
        assert!(decoded.len() > 44100 * 2);
        for (channel, expected) in [gains.master[0], gains.master[1]].into_iter().enumerate() {
            let samples: Vec<_> = decoded
                .iter()
                .skip(6000 + channel)
                .take(60000)
                .step_by(2)
                .copied()
                .collect();
            let rms =
                (samples.iter().map(|v| v.powi(2)).sum::<f32>() / samples.len() as f32).sqrt();
            assert!(
                (rms - expected * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.025,
                "channel {channel}: {rms}"
            );
        }
        let original = std::fs::read(&path).unwrap();
        broadcaster
            .start_recording(capture.clone(), &settings)
            .unwrap();
        wait(|| {
            broadcaster.poll();
            broadcaster.recording_status.phase == Phase::Running
        });
        let next_path = broadcaster.recording_status.path.clone().unwrap();
        for _ in 0..4410 {
            capture.push([0.1, -0.1]);
        }
        drop(broadcaster); // Application close must flush the second recording as well.
        assert_ne!(path, next_path);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(crate::track::open_decoder(&next_path).unwrap().count() > 1000);

        let buffer = PcmBuffer::default();
        buffer.begin();
        for _ in 0..(1 << 17) + 5 {
            buffer.push([f32::NAN, 2.0]);
        }
        assert_eq!(buffer.dropped(), 5);
        buffer.stop();
        buffer.push([0.3, 0.4]);
        let mut pcm = Vec::new();
        assert_eq!(buffer.drain(&mut pcm, 1 << 17), 1 << 17);
        assert!(
            pcm.as_chunks::<2>()
                .0
                .iter()
                .all(|frame| *frame == [0.0, 1.0])
        );
        buffer.begin();
        assert_eq!(buffer.dropped(), 0);
        assert_eq!(buffer.drain(&mut pcm, 100), 0);
        buffer.stop();
    }

    #[test]
    fn console_meter_stereo_taps_measure_the_actual_sum_and_remain_connected_while_idle() {
        let peaks = Arc::new(StereoPeaks::default());
        let (bus, mut source) = metered_bus(4, 48000, 1, peaks.clone());
        assert!(source.by_ref().take(40).all(|v| v == 0.0));
        bus.add(rodio::buffer::SamplesBuffer::new(
            4,
            48000,
            [0.9, 0.8, 0.1, -0.2].repeat(32),
        ));
        bus.add(rodio::buffer::SamplesBuffer::new(
            4,
            48000,
            [-0.3, -0.2, 0.2, -0.3].repeat(32),
        ));
        let rendered: Vec<_> = source.by_ref().take(128).collect();
        assert_eq!(rendered.len(), 128);
        for frame in rendered.as_chunks::<4>().0 {
            assert!((frame[2] - 0.3).abs() < 0.00001);
            assert!((frame[3] + 0.5).abs() < 0.00001);
        }
        let [left, right] = peaks.take();
        assert!((left - 0.3).abs() < 0.00001 && (right - 0.5).abs() < 0.00001);
        assert_eq!(peaks.take(), [0.0; 2]);
        assert!(source.by_ref().take(1024).all(|v| v == 0.0));
        bus.add(rodio::buffer::SamplesBuffer::new(
            4,
            48000,
            [0.0, 0.0, 1.2, -0.4].repeat(16),
        ));
        for _ in 0..64 {
            source.next().unwrap();
        }
        let [left, right] = peaks.take();
        assert!((left - 1.2).abs() < 0.00001 && (right - 0.4).abs() < 0.00001);
    }
    #[test]
    fn master_and_cue_have_no_channel_leakage() {
        let input = rodio::buffer::SamplesBuffer::new(2, 48000, vec![0.2, 0.4, 0.6, 0.8]);
        assert_eq!(
            ChannelPair::new(input.clone(), 4, 1).collect::<Vec<_>>(),
            vec![0.0, 0.0, 0.2, 0.4, 0.0, 0.0, 0.6, 0.8]
        );
        assert_eq!(
            ChannelPair::new(input, 4, 0).collect::<Vec<_>>(),
            vec![0.2, 0.4, 0.0, 0.0, 0.6, 0.8, 0.0, 0.0]
        );
        assert!(validate_pairs(true, 0, Some(0)).is_err());
        assert!(validate_pairs(true, 0, Some(1)).is_ok());
        assert!(validate_pairs(false, 0, Some(0)).is_ok());
    }

    #[test]
    fn mapped_channel_seek_preserves_stereo_pair_and_position() {
        let input = rodio::buffer::SamplesBuffer::new(2, 100, [0.2, 0.4].repeat(200));
        let mut source = ChannelPair::new(input, 4, 1);
        source.next(); // Seek while part way through an interleaved output frame.
        source.try_seek(Duration::from_secs(1)).unwrap();
        assert_eq!(source.take(4).collect::<Vec<_>>(), vec![0.0, 0.0, 0.2, 0.4]);
    }
}
