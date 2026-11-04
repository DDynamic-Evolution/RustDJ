//! A bounded PCM loop, prepared off the audio thread and repeated without decoder seeks.
use crate::{beat::BeatGrid, track};
use rodio::{Source, source::SeekError};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};

/// Shared order for the eight on-screen loop buttons and DJ-202 loop pads.
pub const PRESETS: [(f32, &str); 8] = [
    (0.25, "¼"),
    (0.5, "½"),
    (1.0, "1"),
    (2.0, "2"),
    (4.0, "4"),
    (8.0, "8"),
    (16.0, "16"),
    (32.0, "32"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub start: Duration,
    pub end: Duration,
}

impl Range {
    pub fn validate(self, duration: Duration) -> Result<Self, String> {
        if self.end <= self.start || self.end - self.start < Duration::from_millis(10) {
            return Err("Loop OUT must be at least 10 ms after IN.".into());
        }
        if self.end > duration {
            return Err("The loop extends beyond the end of the track.".into());
        }
        if self.end - self.start > Duration::from_secs(120) {
            return Err("Manual loops cannot exceed 120 seconds.".into());
        }
        Ok(self)
    }

    pub fn beats(
        position: Duration,
        grid: BeatGrid,
        beats: f32,
        duration: Duration,
    ) -> Result<Self, String> {
        if !grid.bpm.is_finite()
            || !(20.0..=400.0).contains(&grid.bpm)
            || !grid.offset.is_finite()
            || !beats.is_finite()
            || !(0.125..=32.0).contains(&beats)
        {
            return Err(
                "Beat loops require a valid BPM value and a length from 1/8 to 32 beats.".into(),
            );
        }
        let period = grid.period();
        let start = (grid.offset
            + ((position.as_secs_f64() - grid.offset) / period).floor() * period)
            .max(0.0);
        Self {
            start: Duration::from_secs_f64(start),
            end: Duration::from_secs_f64(start + beats as f64 * period),
        }
        .validate(duration)
    }

    pub fn contains(self, position: Duration) -> bool {
        position >= self.start && position < self.end
    }

    pub fn distance(self, left: Duration, right: Duration) -> Duration {
        let distance = left.abs_diff(right);
        if self.contains(left) && self.contains(right) {
            distance.min((self.end - self.start).saturating_sub(distance))
        } else {
            distance
        }
    }
}

pub struct Buffer {
    pub range: Range,
    start: u64,
    samples: Arc<[f32]>,
    rate: u32,
    channels: u16,
}

impl Buffer {
    pub fn prepare(path: &Path, range: Range, cancelled: &AtomicBool) -> Result<Arc<Self>, String> {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Loop preparation cancelled.".into());
        }
        let mut decoder = track::open_decoder(path)?;
        let rate = decoder.sample_rate();
        let channels = decoder.channels();
        if rate == 0 || channels == 0 {
            return Err("Invalid loop audio format.".into());
        }
        let start = (range.start.as_secs_f64() * rate as f64).round() as u64 * channels as u64;
        let end = (range.end.as_secs_f64() * rate as f64).round() as u64 * channels as u64;
        let count = end.saturating_sub(start);
        if count == 0 || count > 16_000_000 {
            return Err("The loop is empty or needs more than 64 MB of audio memory.".into());
        }
        decoder
            .try_seek(Duration::from_secs_f64(
                start as f64 / rate as f64 / channels as f64,
            ))
            .map_err(|e| format!("Could not reach Loop IN: {e}"))?;
        let mut samples = Vec::with_capacity(count as usize);
        for sample in decoder.take(count as usize) {
            if samples.len().is_multiple_of(4096) && cancelled.load(Ordering::Relaxed) {
                return Err("Loop preparation cancelled.".into());
            }
            samples.push(sample);
        }
        if samples.len() != count as usize {
            return Err("Loop OUT is beyond the decodable audio data.".into());
        }
        let range = Range {
            start: Duration::from_secs_f64(start as f64 / rate as f64 / channels as f64),
            end: Duration::from_secs_f64(end as f64 / rate as f64 / channels as f64),
        };
        Ok(Arc::new(Self {
            range,
            start,
            samples: samples.into(),
            rate,
            channels,
        }))
    }
}

pub type Command = Option<Arc<Buffer>>;

/// The clock is in original interleaved samples, including loop wraparounds.
pub struct Transport<S> {
    inner: S,
    position: u64,
    clock: Arc<AtomicU64>,
    fault: Arc<AtomicBool>,
    receiver: Receiver<Command>,
    buffer: Command,
    decoder_dirty: bool,
}

impl<S: Source<Item = f32>> Transport<S> {
    pub fn new(inner: S, clock: Arc<AtomicU64>, fault: Arc<AtomicBool>) -> (Self, Sender<Command>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self {
                inner,
                position: 0,
                clock,
                fault,
                receiver,
                buffer: None,
                decoder_dirty: false,
            },
            sender,
        )
    }

    fn updates(&mut self) {
        for command in self.receiver.try_iter() {
            self.buffer = command.filter(|b| {
                b.rate == self.inner.sample_rate() && b.channels == self.inner.channels()
            });
        }
    }
}

impl<S: Source<Item = f32>> Iterator for Transport<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let channels = self.inner.channels() as u64;
        if self.position.is_multiple_of(channels) {
            self.updates();
        }
        let sample = if let Some(buffer) = &self.buffer {
            if self.position < buffer.start
                || self.position >= buffer.start + buffer.samples.len() as u64
            {
                self.position = buffer.start;
            }
            self.decoder_dirty = true;
            buffer.samples[(self.position - buffer.start) as usize]
        } else {
            if self.decoder_dirty {
                let at = Duration::from_secs_f64(
                    self.position as f64 / self.inner.sample_rate() as f64 / channels as f64,
                );
                if self.inner.try_seek(at).is_err() {
                    self.fault.store(true, Ordering::Relaxed);
                    return None;
                }
                self.decoder_dirty = false;
            }
            self.inner.next()?
        };
        self.position += 1;
        if let Some(buffer) = &self.buffer
            && self.position >= buffer.start + buffer.samples.len() as u64
        {
            self.position = buffer.start;
        }
        self.clock.store(self.position, Ordering::Relaxed);
        Some(sample)
    }
}

impl<S: Source<Item = f32>> Source for Transport<S> {
    fn current_span_len(&self) -> Option<usize> {
        Some(1024)
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.updates();
        let samples = (position.as_secs_f64() * self.sample_rate() as f64).round() as u64
            * self.channels() as u64;
        if self
            .buffer
            .as_ref()
            .is_some_and(|b| samples >= b.start && samples < b.start + b.samples.len() as u64)
        {
            self.decoder_dirty = true;
        } else {
            self.inner.try_seek(position)?;
            self.decoder_dirty = false;
        }
        self.position = samples;
        self.clock.store(samples, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;

    #[test]
    fn loop_pads_eight_presets_produce_the_expected_quantized_lengths() {
        let grid = BeatGrid {
            bpm: 120.0,
            offset: 0.1,
            confidence: 1.0,
        };
        let expected = [0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
        for ((beats, _), seconds) in PRESETS.into_iter().zip(expected) {
            let range = Range::beats(
                Duration::from_millis(1370),
                grid,
                beats,
                Duration::from_secs(60),
            )
            .unwrap();
            assert!(range.start.abs_diff(Duration::from_millis(1100)) < Duration::from_micros(1));
            assert!(
                (range.end - range.start).abs_diff(Duration::from_secs_f64(seconds))
                    < Duration::from_micros(1)
            );
        }
        assert_eq!(PRESETS.len(), 8);
    }

    #[test]
    fn real_sinks_keep_short_audio_looping_past_eof_and_apply_gain_to_both_outputs() {
        use crate::dsp::{EqParams, Equalizer};
        let mut outputs = Vec::new();
        let mut sinks = Vec::new();
        let mut controls = Vec::new();
        let mut clocks = Vec::new();
        for _ in 0..2 {
            let data: Vec<_> = (0..192)
                .map(|i| if i % 2 == 0 { 0.2 } else { -0.2 })
                .collect();
            let clock = Arc::new(AtomicU64::new(0));
            let (source, control) = Transport::new(
                SamplesBuffer::new(2, 48000, data.clone()),
                clock.clone(),
                Arc::new(AtomicBool::new(false)),
            );
            let range = Range {
                start: Duration::from_secs_f64(16.0 / 48000.0),
                end: Duration::from_secs_f64(48.0 / 48000.0),
            };
            control
                .send(Some(Arc::new(Buffer {
                    range,
                    start: 32,
                    samples: data[32..96].to_vec().into(),
                    rate: 48000,
                    channels: 2,
                })))
                .unwrap();
            let eq = Arc::new(EqParams::default());
            eq.set_gain(6.0206);
            let (sink, output) = rodio::Sink::new();
            sink.append(Equalizer::new(source, eq));
            sink.play();
            outputs.push(output);
            sinks.push(sink);
            controls.push(control);
            clocks.push(clock);
        }
        for _ in 0..96000 {
            let a = outputs[0].next().unwrap();
            let b = outputs[1].next().unwrap();
            assert!((a - b).abs() < 0.00001);
            assert!((a.abs() - 0.4).abs() < 0.0001, "{a}");
        }
        assert!(sinks.iter().all(|s| !s.empty()));
        assert!(
            clocks
                .iter()
                .all(|c| (32..96).contains(&c.load(Ordering::Relaxed)))
        );
        for control in controls {
            control.send(None).unwrap();
        }
        for output in &mut outputs {
            for _ in 0..1000 {
                output.next();
            }
        }
        assert!(sinks.iter().all(|s| s.empty()));
    }

    #[test]
    fn sample_exact_stereo_loops_wrap_clocks_and_exit_at_the_current_media_position() {
        let samples: Vec<f32> = (0..200).map(|i| i as f32 / 200.0).collect();
        let clock = Arc::new(AtomicU64::new(0));
        let fault = Arc::new(AtomicBool::new(false));
        let (mut source, control) = Transport::new(
            SamplesBuffer::new(2, 100, samples.clone()),
            clock.clone(),
            fault.clone(),
        );
        let range = Range {
            start: Duration::from_millis(100),
            end: Duration::from_millis(150),
        };
        control
            .send(Some(Arc::new(Buffer {
                range,
                start: 20,
                samples: samples[20..30].to_vec().into(),
                rate: 100,
                channels: 2,
            })))
            .unwrap();
        for cycle in 0..100 {
            assert_eq!(
                source.by_ref().take(10).collect::<Vec<_>>(),
                samples[20..30],
                "cycle {cycle}"
            );
            assert_eq!(clock.load(Ordering::Relaxed), 20);
        }
        assert_eq!(source.next(), Some(samples[20]));
        assert_eq!(source.next(), Some(samples[21]));
        control.send(None).unwrap();
        assert_eq!(source.next(), Some(samples[22]));
        assert_eq!(source.next(), Some(samples[23]));
        assert!(!fault.load(Ordering::Relaxed));
        source.try_seek(Duration::from_millis(400)).unwrap();
        assert_eq!(source.next(), Some(samples[80]));
    }

    #[test]
    fn real_mp3_flac_loops_prepare_without_holding_an_open_decoder_and_cancel() {
        let range = Range {
            start: Duration::from_secs(1),
            end: Duration::from_secs(2),
        };
        for name in ["demo-a.mp3", "demo-b.flac"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(name);
            let buffer = Buffer::prepare(&path, range, &AtomicBool::new(false)).unwrap();
            assert_eq!(buffer.samples.len(), 88200);
            assert!(buffer.samples.iter().any(|v| v.abs() > 0.01));
            assert!(Buffer::prepare(&path, range, &AtomicBool::new(true)).is_err());
        }
    }

    #[test]
    fn beat_ranges_quantize_validate_track_end_and_measure_circular_drift() {
        let grid = BeatGrid {
            bpm: 120.0,
            offset: 0.1,
            confidence: 1.0,
        };
        let range = Range::beats(
            Duration::from_millis(1370),
            grid,
            4.0,
            Duration::from_secs(8),
        )
        .unwrap();
        assert!(range.start.abs_diff(Duration::from_millis(1100)) < Duration::from_micros(1));
        assert!(range.end.abs_diff(Duration::from_millis(3100)) < Duration::from_micros(1));
        assert_eq!(
            range.distance(Duration::from_millis(3090), Duration::from_millis(1110)),
            Duration::from_millis(20)
        );
        assert!(Range::beats(Duration::from_secs(7), grid, 8.0, Duration::from_secs(8)).is_err());
    }
}
