use crate::beat::BeatGrid;
pub use crate::output::Audio;
use crate::track::{self, Track};
use crate::{
    database::{self, Database, Fingerprint},
    dsp::{EqParams, Equalizer},
    effects::{Effects, Params as EffectParams},
    loops::{self, Buffer as LoopBuffer, Range as LoopRange},
    output::ChannelPair,
    performance::{Hotcues, Snapshot},
};
use rodio::{OutputStream, Sink, Source, source::SeekError};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    time::{Duration, Instant},
};

struct LoadJob {
    receiver: Receiver<Result<Loaded, String>>,
    cancelled: Arc<AtomicBool>,
}

struct Loaded {
    track: Track,
    cues: Hotcues,
    identity: Fingerprint,
}

struct LoopJob {
    receiver: Receiver<Result<Arc<LoopBuffer>, String>>,
    cancelled: Arc<AtomicBool>,
    resume_after_eof: bool,
}
impl Drop for LoopJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for LoadJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

pub struct Deck {
    pub track: Option<Track>,
    pub volume: f32,
    pub speed: f32,
    pub cue: Duration,
    pub error: Option<String>,
    pub meter: f32,
    pub channel_meter: f32,
    pub grid: Option<BeatGrid>,
    pub eq: [f32; 3],
    pub gain: f32,
    pub filter: f32,
    pub fx_enabled: [bool; 3],
    pub fx_level: f32,
    fx_params: Arc<EffectParams>,
    pub pfl: bool,
    pub phones_meter: f32,
    pub ui_cue_down: bool,
    pub hotcues: Hotcues,
    pub hotcues_dirty: bool,
    pub loop_beats: f32,
    pub loop_range: Option<LoopRange>,
    pub loop_in: Option<Duration>,
    pub loop_active: bool,
    pub loop_auto: bool,
    hotcue_identity: Option<Fingerprint>,
    loop_job: Option<LoopJob>,
    loop_buffer: Option<Arc<LoopBuffer>>,
    eq_params: Arc<EqParams>,
    cue_latch: CueLatch,
    sink: Option<Playback>,
    job: Option<LoadJob>,
}

struct Voice {
    sink: Sink,
    peak: Arc<AtomicU32>,
    media_samples: Arc<AtomicU64>,
    loop_control: Sender<loops::Command>,
    fault: Arc<AtomicBool>,
}
impl std::ops::Deref for Voice {
    type Target = Sink;
    fn deref(&self) -> &Sink {
        &self.sink
    }
}
struct Playback {
    master: Voice,
    phones: Option<Voice>,
    last_drift: Instant,
}
impl std::ops::Deref for Playback {
    type Target = Voice;
    fn deref(&self) -> &Voice {
        &self.master
    }
}
impl Playback {
    fn play(&self) {
        self.master.play();
        if let Some(phones) = &self.phones {
            phones.play();
        }
    }
    fn pause(&self) {
        self.master.pause();
        if let Some(phones) = &self.phones {
            phones.pause();
        }
    }
    fn set_speed(&self, speed: f32) {
        self.master.set_speed(speed);
        if let Some(phones) = &self.phones {
            phones.set_speed(speed);
        }
    }
    fn seek_media(&self, position: Duration) -> Result<(), SeekError> {
        self.master
            .try_seek(position.div_f32(self.master.speed()))?;
        if let Some(phones) = &self.phones {
            phones.try_seek(position.div_f32(phones.speed()))?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct CueLatch {
    held: bool,
    previewing: bool,
}
#[derive(Debug, PartialEq, Eq)]
enum CueOp {
    None,
    Return,
    Preview,
}
impl CueLatch {
    fn change(&mut self, pressed: bool, playing: bool) -> CueOp {
        if pressed == self.held {
            return CueOp::None;
        }
        self.held = pressed;
        if pressed {
            self.previewing = !playing;
            if playing {
                CueOp::Return
            } else {
                CueOp::Preview
            }
        } else if self.previewing {
            self.previewing = false;
            CueOp::Return
        } else {
            CueOp::None
        }
    }
}

fn make_voice(
    track: &Track,
    stream: &OutputStream,
    pair: u16,
    eq: Arc<EqParams>,
    mixer: &rodio::mixer::Mixer,
    fx_params: Arc<EffectParams>,
) -> Result<Voice, String> {
    let media_samples = Arc::new(AtomicU64::new(0));
    let fault = Arc::new(AtomicBool::new(false));
    let (transport, loop_control) = loops::Transport::new(
        track::open_decoder(&track.path)?,
        media_samples.clone(),
        fault.clone(),
    );
    let source = Effects::new(Equalizer::new(transport, eq), fx_params);
    let sink = Sink::connect_new(mixer);
    sink.pause();
    sink.set_volume(0.0);
    let peak = Arc::new(AtomicU32::new(0));
    let source = Metered {
        inner: source,
        peak: peak.clone(),
    };
    sink.append(ChannelPair::new(
        source,
        stream.config().channel_count(),
        pair,
    ));
    Ok(Voice {
        sink,
        peak,
        media_samples,
        loop_control,
        fault,
    })
}

impl Default for Deck {
    fn default() -> Self {
        Self {
            track: None,
            volume: 0.8,
            speed: 1.0,
            cue: Duration::ZERO,
            error: None,
            meter: 0.0,
            channel_meter: 0.0,
            grid: None,
            eq: [0.0; 3],
            gain: 0.0,
            filter: 0.0,
            fx_enabled: [false; 3],
            fx_level: 0.25,
            fx_params: Arc::new(EffectParams::default()),
            pfl: false,
            phones_meter: 0.0,
            ui_cue_down: false,
            hotcues: [None; 8],
            hotcues_dirty: false,
            hotcue_identity: None,
            loop_beats: 4.0,
            loop_range: None,
            loop_in: None,
            loop_active: false,
            loop_auto: false,
            loop_job: None,
            loop_buffer: None,
            eq_params: Arc::new(EqParams::default()),
            cue_latch: CueLatch::default(),
            sink: None,
            job: None,
        }
    }
}

impl Deck {
    pub fn load(&mut self, path: PathBuf, ctx: eframe::egui::Context) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_worker = cancelled.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                let mut track = crate::analysis::load(path, &cancel_worker)?;
                let identity = Fingerprint::read(&track.path)?;
                let cues = match Database::open(&database::default_path())
                    .and_then(|db| db.hotcues(&track.path, &identity))
                {
                    Ok(cues) => cues.map(|cue| cue.filter(|p| *p < track.duration)),
                    Err(error) => {
                        track.cache_warning = Some(format!("Hotcues were not loaded: {error}"));
                        [None; 8]
                    }
                };
                Ok(Loaded {
                    track,
                    cues,
                    identity,
                })
            })();
            let _ = sender.send(result);
            ctx.request_repaint();
        });
        self.job = Some(LoadJob {
            receiver,
            cancelled,
        });
        self.error = None;
    }

    pub fn loading(&self) -> bool {
        self.job.is_some()
    }

    /// Returns the successfully loaded track for the session library.
    pub fn poll(&mut self, audio: &Audio) -> Option<Track> {
        self.poll_loop(audio);
        let result = {
            let job = self.job.as_ref()?;
            match job.receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    Err("Audio loading stopped unexpectedly.".into())
                }
            }
        };
        self.job = None;
        match result {
            Ok(Loaded {
                track,
                cues,
                identity,
            }) => match self.make_sink(&track, audio) {
                Ok(sink) => {
                    self.exit_loop();
                    self.loop_range = None;
                    self.loop_in = None;
                    self.hotcues = cues;
                    self.hotcue_identity = Some(identity);
                    self.hotcues_dirty = false;
                    self.sink = sink;
                    self.cue = Duration::ZERO;
                    self.cue_latch = CueLatch::default();
                    self.ui_cue_down = false;
                    self.speed = 1.0;
                    self.meter = 0.0;
                    self.channel_meter = 0.0;
                    self.phones_meter = 0.0;
                    self.grid = track.grid;
                    self.error = track.cache_warning.clone();
                    self.track = Some(track.clone());
                    return Some(track);
                }
                Err(error) => self.error = Some(error),
            },
            Err(error) => self.error = Some(error),
        }
        None
    }

    fn make_sink(&self, track: &Track, audio: &Audio) -> Result<Option<Playback>, String> {
        let Some(stream) = &audio.stream else {
            return Ok(None);
        };
        self.eq_params.set(self.eq);
        self.eq_params.set_gain(self.gain);
        self.eq_params.set_speed(self.speed);
        let master = make_voice(
            track,
            stream,
            audio.routing.master.pair,
            self.eq_params.clone(),
            audio.bus(false).ok_or("Master mixing bus is missing.")?,
            self.fx_params.clone(),
        )?;
        let phones = match (audio.phones(), &audio.routing.phones) {
            (Some(stream), Some(target)) => Some(make_voice(
                track,
                stream,
                target.pair,
                self.eq_params.clone(),
                audio.bus(true).ok_or("Headphone mixing bus is missing.")?,
                self.fx_params.clone(),
            )?),
            _ => None,
        };
        Ok(Some(Playback {
            master,
            phones,
            last_drift: Instant::now(),
        }))
    }

    pub fn rebind(&mut self, audio: &Audio) -> Result<(), String> {
        let position = self.position();
        let playing = self.playing();
        if let Some(track) = &self.track {
            let playback = self.make_sink(track, audio)?;
            self.sink = playback;
            self.seek(position, audio);
            self.send_loop(self.loop_buffer.clone());
            if playing && let Some(sink) = &self.sink {
                sink.play();
            }
        }
        Ok(())
    }

    pub fn playing(&self) -> bool {
        self.sink
            .as_ref()
            .is_some_and(|sink| !sink.is_paused() && !sink.empty())
    }

    pub fn position(&self) -> Duration {
        let Some(track) = &self.track else {
            return Duration::ZERO;
        };
        match &self.sink {
            Some(sink) if sink.empty() => track.duration,
            Some(sink) => Duration::from_secs_f64(
                sink.media_samples.load(Ordering::Relaxed) as f64
                    / track.sample_rate as f64
                    / track.channels as f64,
            )
            .min(track.duration),
            None => Duration::ZERO,
        }
    }

    /// Source-time diagnostic for the separately clocked headphone path.
    pub fn phones_position(&self) -> Option<Duration> {
        let track = self.track.as_ref()?;
        let phones = self.sink.as_ref()?.phones.as_ref()?;
        Some(
            Duration::from_secs_f64(
                phones.media_samples.load(Ordering::Relaxed) as f64
                    / track.sample_rate as f64
                    / track.channels as f64,
            )
            .min(track.duration),
        )
    }

    pub fn toggle(&mut self, audio: &Audio) {
        if self.cue_latch.previewing {
            self.cue_latch.previewing = false;
            return;
        }
        if self.playing() {
            if let Some(sink) = &self.sink {
                sink.pause();
            }
            return;
        }
        if !self.ensure_source(audio) {
            return;
        }
        if let Some(sink) = &self.sink {
            sink.play();
        }
    }

    fn ensure_source(&mut self, audio: &Audio) -> bool {
        if audio.stream.is_none() {
            self.error = Some("No audio output. Reconnect audio.".into());
            return false;
        }
        let Some(track) = &self.track else {
            return false;
        };
        if self.sink.as_ref().is_none_or(|sink| sink.empty()) {
            match self.make_sink(track, audio) {
                Ok(sink) => self.sink = sink,
                Err(error) => {
                    self.error = Some(error);
                    return false;
                }
            }
        }
        true
    }

    pub fn seek(&mut self, position: Duration, audio: &Audio) {
        if (self.loop_active || self.loop_pending())
            && self
                .loop_range
                .is_some_and(|range| !range.contains(position))
        {
            self.exit_loop();
        }
        if !self.ensure_source(audio) {
            return;
        }
        let duration = self.track.as_ref().map_or(Duration::ZERO, |t| t.duration);
        // Keep the decoder alive when the user clicks the far right edge.
        let target = position.min(duration.saturating_sub(Duration::from_millis(1)));
        if let Some(sink) = &self.sink
            && let Err(error) = sink.seek_media(target)
        {
            self.error = Some(format!("Could not set playback position: {error}"));
        }
    }

    pub fn stop(&mut self, audio: &Audio) {
        self.exit_loop();
        self.cue_latch.previewing = false;
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        self.seek(Duration::ZERO, audio);
    }

    pub fn go_to_cue(&mut self, audio: &Audio) {
        self.exit_loop();
        self.cue_latch.previewing = false;
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        self.seek(self.cue, audio);
    }

    /// Transport CUE: return/pause while playing; hold-to-preview while paused.
    pub fn cue_button(&mut self, pressed: bool, audio: &Audio) {
        match self.cue_latch.change(pressed, self.playing()) {
            CueOp::Return => self.go_to_cue(audio),
            CueOp::Preview => {
                self.go_to_cue(audio);
                self.cue_latch.previewing = true;
                if let Some(sink) = &self.sink {
                    sink.play();
                }
            }
            CueOp::None => {}
        }
    }

    /// Compatibility helper: the ordinary program bus, with cue output silent.
    pub fn apply_mix(&mut self, gain: f32) {
        self.apply_buses(self.volume * gain, 0.0);
    }

    pub fn apply_buses(&mut self, master_gain: f32, phones_gain: f32) {
        self.fx_params.set(
            self.filter,
            self.fx_level,
            self.fx_enabled,
            self.speed,
            self.grid.map_or(120.0, |g| g.bpm),
        );
        self.eq_params.set(self.eq);
        self.eq_params.set_gain(self.gain);
        self.eq_params.set_speed(self.speed);
        if let Some(sink) = &mut self.sink {
            if sink.master.fault.swap(false, Ordering::Relaxed)
                || sink
                    .phones
                    .as_ref()
                    .is_some_and(|p| p.fault.swap(false, Ordering::Relaxed))
            {
                self.error = Some("Loop transport could not restore the decoder position.".into());
            }
            sink.master.set_volume(master_gain);
            sink.set_speed(self.speed);
            if let Some(phones) = &sink.phones {
                phones.set_volume(phones_gain);
            }
            let channel_peak = f32::from_bits(sink.master.peak.swap(0, Ordering::Relaxed));
            self.channel_meter = channel_peak.max(self.channel_meter * 0.86).min(8.0);
            self.meter = (channel_peak * master_gain).max(self.meter * 0.86).min(2.0);
            self.phones_meter = (sink
                .phones
                .as_ref()
                .map_or(0.0, |p| f32::from_bits(p.peak.swap(0, Ordering::Relaxed)))
                * phones_gain)
                .max(self.phones_meter * 0.86)
                .min(2.0);
            // Separate devices have independent clocks. Keep headphone transport near
            // the program source; never seek/correct the public master signal.
            if sink.last_drift.elapsed() > Duration::from_secs(1) {
                sink.last_drift = Instant::now();
                if let Some(phones) = &sink.phones
                    && !sink.master.is_paused()
                    && !phones.is_paused()
                    && !sink.master.empty()
                    && !phones.empty()
                {
                    let master = sink.master.media_samples.load(Ordering::Relaxed);
                    let cue = phones.media_samples.load(Ordering::Relaxed);
                    if let Some(track) = &self.track {
                        let samples_per_second = track.sample_rate as f64 * track.channels as f64;
                        let master_position =
                            Duration::from_secs_f64(master as f64 / samples_per_second);
                        let phones_position =
                            Duration::from_secs_f64(cue as f64 / samples_per_second);
                        let distance = if self.loop_active {
                            self.loop_range
                                .map_or(master_position.abs_diff(phones_position), |r| {
                                    r.distance(master_position, phones_position)
                                })
                        } else {
                            master_position.abs_diff(phones_position)
                        };
                        if distance > Duration::from_millis(80) {
                            let position =
                                Duration::from_secs_f64(master as f64 / samples_per_second);
                            if let Err(error) = phones.try_seek(position.div_f32(phones.speed())) {
                                self.error =
                                    Some(format!("Headphone synchronization failed: {error}"));
                            }
                        }
                    }
                }
            }
        } else {
            self.channel_meter *= 0.86;
            self.meter *= 0.86;
            self.phones_meter *= 0.86;
        }
    }

    pub fn disconnect(&mut self) {
        self.sink = None;
    }

    pub fn hotcue(&mut self, slot: usize, audio: &Audio) {
        if slot >= 8 || self.track.is_none() {
            return;
        }
        if let Some(position) = self.hotcues[slot] {
            self.exit_loop();
            self.cue_latch = CueLatch::default();
            self.ui_cue_down = false;
            self.seek(position, audio);
            if let Some(sink) = &self.sink {
                sink.play();
            }
        } else {
            let end = self
                .track
                .as_ref()
                .unwrap()
                .duration
                .saturating_sub(Duration::from_millis(1));
            self.hotcues[slot] = Some(self.position().min(end));
            self.hotcues_dirty = true;
        }
    }

    pub fn clear_hotcue(&mut self, slot: usize) {
        if let Some(cue) = self.hotcues.get_mut(slot) {
            *cue = None;
            self.hotcues_dirty = true;
        }
    }

    pub fn take_hotcue_changes(&mut self) -> Option<Snapshot> {
        if !std::mem::take(&mut self.hotcues_dirty) {
            return None;
        }
        Some(Snapshot {
            path: self.track.as_ref()?.path.clone(),
            identity: self.hotcue_identity.clone()?,
            cues: self.hotcues,
        })
    }

    pub fn loop_pending(&self) -> bool {
        self.loop_job.is_some()
    }

    fn send_loop(&self, buffer: Option<Arc<LoopBuffer>>) {
        if let Some(sink) = &self.sink {
            let _ = sink.master.loop_control.send(buffer.clone());
            if let Some(phones) = &sink.phones {
                let _ = phones.loop_control.send(buffer);
            }
        }
    }

    pub fn exit_loop(&mut self) {
        self.loop_job = None;
        self.loop_active = false;
        self.loop_buffer = None;
        self.send_loop(None);
    }

    fn prepare_loop(&mut self, range: LoopRange, audio: &Audio) {
        let Some(track) = &self.track else {
            return;
        };
        let range = match range.validate(track.duration) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let path = track.path.clone();
        self.exit_loop();
        if !self.ensure_source(audio) {
            return;
        }
        self.loop_range = Some(range);
        self.loop_in = Some(range.start);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(LoopBuffer::prepare(&path, range, &worker_cancel));
        });
        self.loop_job = Some(LoopJob {
            receiver,
            cancelled,
            resume_after_eof: self.playing(),
        });
        self.error = None;
    }

    fn poll_loop(&mut self, audio: &Audio) {
        let Some(job) = &self.loop_job else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Loop preparation stopped unexpectedly.".into()),
        };
        let resume_after_eof = job.resume_after_eof;
        self.loop_job = None;
        match result {
            Ok(buffer) => {
                let ended = self.sink.as_ref().is_none_or(|sink| sink.empty());
                if !self.ensure_source(audio) {
                    return;
                }
                self.loop_range = Some(buffer.range);
                self.loop_buffer = Some(buffer.clone());
                self.loop_active = true;
                self.send_loop(Some(buffer.clone()));
                if !buffer.range.contains(self.position()) {
                    self.seek(buffer.range.start, audio);
                }
                if ended
                    && resume_after_eof
                    && let Some(sink) = &self.sink
                {
                    sink.play();
                }
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn beat_loop(&mut self, beats: f32, audio: &Audio) {
        if self.loop_auto
            && (self.loop_active || self.loop_pending())
            && (self.loop_beats - beats).abs() < 0.001
        {
            self.exit_loop();
            return;
        }
        let Some(track) = &self.track else {
            return;
        };
        let Some(grid) = self.grid else {
            self.error = Some("Analyze or set BPM before using beat loops.".into());
            return;
        };
        let position = if self.loop_active || self.loop_pending() {
            self.loop_range.map_or(self.position(), |r| r.start)
        } else {
            self.position()
        };
        match LoopRange::beats(position, grid, beats, track.duration) {
            Ok(range) => {
                self.loop_beats = beats;
                self.loop_auto = true;
                self.prepare_loop(range, audio);
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn resize_loop(&mut self, factor: f32, audio: &Audio) {
        let beats = (self.loop_beats * factor).clamp(0.125, 32.0);
        if self.loop_active || self.loop_pending() {
            if !self.loop_auto
                && let Some(range) = self.loop_range
            {
                let length = (range.end - range.start).mul_f32(factor.clamp(0.5, 2.0));
                self.prepare_loop(
                    LoopRange {
                        start: range.start,
                        end: range.start + length,
                    },
                    audio,
                );
                return;
            }
            if (beats - self.loop_beats).abs() > 0.001 {
                self.beat_loop(beats, audio);
            }
        } else {
            self.loop_beats = beats;
        }
    }

    pub fn set_loop_in(&mut self) {
        if self.track.is_some() {
            let position = self.position();
            self.exit_loop();
            self.loop_in = Some(position);
            self.loop_range = None;
            self.loop_auto = false;
        }
    }

    pub fn set_loop_out(&mut self, audio: &Audio) {
        if let Some(start) = self.loop_in {
            self.loop_auto = false;
            self.prepare_loop(
                LoopRange {
                    start,
                    end: self.position(),
                },
                audio,
            );
        } else {
            self.error = Some("Set Loop IN first.".into());
        }
    }

    pub fn reloop(&mut self, audio: &Audio) {
        if self.loop_active || self.loop_pending() {
            self.exit_loop();
        } else if let Some(range) = self.loop_range {
            self.prepare_loop(range, audio);
        }
    }
}

/// Equal-power crossfader; use master headroom for correlated sources.
pub fn crossfader_gains(position: f32) -> [f32; 2] {
    let angle = (position.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    [angle.cos().max(0.0), angle.sin()]
}

struct Metered<S> {
    inner: S,
    peak: Arc<AtomicU32>,
}

impl<S: Source<Item = f32>> Iterator for Metered<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;
        self.peak
            .fetch_max(sample.abs().to_bits(), Ordering::Relaxed);
        Some(sample)
    }
}

impl<S: Source<Item = f32>> Source for Metered<S> {
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
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression18_visible_output_sliders_control_four_decks_and_persist_levels() {
        use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, Rect, Vec2};
        fn sliders(
            ctx: &egui::Context,
            width: f32,
            values: &mut [f32; 3],
            events: Vec<Event>,
        ) -> [Rect; 3] {
            let mut rects = [Rect::NOTHING; 3];
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 100.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let [master, phones, mixing] = values;
                        rects = crate::app::output_levels_ui(ui, master, phones, mixing)
                            .map(|r| r.rect);
                    });
                },
            );
            rects
        }
        let mut values = [0.7, 0.4, 0.0];
        for width in [340.0, 390.0, 450.0] {
            let ctx = egui::Context::default();
            let rects = sliders(&ctx, width, &mut values, vec![]);
            for (index, fraction) in [0.25, 0.65, 0.75].into_iter().enumerate() {
                let rect = rects[index];
                assert!(rect.left() >= 0.0 && rect.right() <= width && rect.height() >= 14.0);
                if index > 0 {
                    assert!(rects[index - 1].right() < rect.left());
                }
                let pos = Pos2::new(rect.left() + rect.width() * fraction, rect.center().y);
                sliders(
                    &ctx,
                    width,
                    &mut values,
                    vec![
                        Event::PointerMoved(pos),
                        Event::PointerButton {
                            pos,
                            button: PointerButton::Primary,
                            pressed: true,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                );
                sliders(
                    &ctx,
                    width,
                    &mut values,
                    vec![Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    }],
                );
                assert!(
                    (values[index] - fraction).abs() < 0.07,
                    "slider {index}: {}",
                    values[index]
                );
            }
        }
        let temp = tempfile::Builder::new()
            .prefix("output18-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let settings_file = temp.path().join("settings.json");
        let saved = crate::settings::Settings {
            mix: crate::settings::MixSettings {
                master: values[0],
                phones: values[1],
                mixing: values[2],
                crossfader_enabled: false,
            },
            ..Default::default()
        };
        crate::settings::save_to(&settings_file, &saved).unwrap();
        let (restored, error) = crate::settings::load_from(&settings_file);
        assert!(error.is_none());
        assert_eq!(
            [
                restored.mix.master,
                restored.mix.phones,
                restored.mix.mixing
            ],
            values
        );
        fn voice(eq: Arc<EqParams>) -> (Voice, impl Source<Item = f32>) {
            let (sink, output) = Sink::new();
            sink.pause();
            let peak = Arc::new(AtomicU32::new(0));
            let clock = Arc::new(AtomicU64::new(0));
            let fault = Arc::new(AtomicBool::new(false));
            let (transport, loop_control) = loops::Transport::new(
                rodio::buffer::SamplesBuffer::new(2, 48000, vec![0.25; 200000]),
                clock.clone(),
                fault.clone(),
            );
            let source = Equalizer::new(transport, eq);
            sink.append(ChannelPair::new(
                Metered {
                    inner: source,
                    peak: peak.clone(),
                },
                2,
                0,
            ));
            (
                Voice {
                    sink,
                    peak,
                    media_samples: clock,
                    loop_control,
                    fault,
                },
                output,
            )
        }
        fn amplitude(output: &mut impl Iterator<Item = f32>) -> f32 {
            for _ in 0..2048 {
                output.next().unwrap();
            }
            output.take(2048).map(f32::abs).sum::<f32>() / 2048.0
        }
        let mut decks: [Deck; 4] = std::array::from_fn(|_| Deck::default());
        let mut outputs = Vec::new();
        for deck in &mut decks {
            let (master, master_output) = voice(deck.eq_params.clone());
            let (phones, phones_output) = voice(deck.eq_params.clone());
            deck.sink = Some(Playback {
                master,
                phones: Some(phones),
                last_drift: Instant::now(),
            });
            deck.sink.as_ref().unwrap().play();
            outputs.push((master_output, phones_output));
        }
        for [master, phones, mixing] in [
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.7, 0.0, 0.0],
            [0.3, 0.5, 0.0],
            values,
            [0.0, 0.6, 1.0],
        ] {
            let gains = crate::dsp::bus_gains_with_crossfader(
                [0.2, 0.4, 0.6, 0.8],
                [true, false, true, false],
                0.0,
                master,
                phones,
                mixing,
                false,
            );
            for (index, (deck, (master_output, phones_output))) in
                decks.iter_mut().zip(&mut outputs).enumerate()
            {
                deck.apply_buses(gains.master[index], gains.phones[index]);
                let actual_master = amplitude(master_output);
                let actual_phones = amplitude(phones_output);
                assert!((actual_master - 0.25 * gains.master[index]).abs() < 0.0001);
                assert!((actual_phones - 0.25 * gains.phones[index]).abs() < 0.0001);
                if master == 0.0 {
                    assert_eq!(actual_master, 0.0);
                }
                if phones == 0.0 {
                    assert_eq!(actual_phones, 0.0);
                }
                // Monitoring the master stays audible at master level zero.
                if mixing == 1.0 {
                    assert!(actual_phones > 0.0);
                }
            }
        }
    }

    #[test]
    fn cue_hold_previews_and_release_returns_but_play_latches() {
        let mut cue = CueLatch::default();
        assert_eq!(cue.change(true, false), CueOp::Preview);
        assert_eq!(cue.change(true, true), CueOp::None);
        assert_eq!(cue.change(false, true), CueOp::Return);
        assert!(!cue.previewing);
        assert_eq!(cue.change(true, true), CueOp::Return);
        assert_eq!(cue.change(false, false), CueOp::None);
        assert_eq!(cue.change(true, false), CueOp::Preview);
        // Play while holding Cue latches normal playback.
        cue.previewing = false;
        assert_eq!(cue.change(false, true), CueOp::None);
    }

    #[test]
    fn crossfader_has_correct_endpoints_and_constant_power() {
        for i in -100..=100 {
            let [a, b] = crossfader_gains(i as f32 / 100.0);
            assert!((a * a + b * b - 1.0).abs() < 0.00001);
        }
        assert!(crossfader_gains(-1.0)[1] < 0.00001);
        assert!(crossfader_gains(1.0)[0] < 0.00001);
        assert_eq!(crossfader_gains(-2.0), crossfader_gains(-1.0));
    }

    #[test]
    fn metering_passes_audio_and_forwards_seeking() {
        let peak = Arc::new(AtomicU32::new(0));
        let mut source = Metered {
            inner: rodio::buffer::SamplesBuffer::new(1, 100, vec![0.2, -0.7, 0.3]),
            peak: peak.clone(),
        };
        assert_eq!(source.next(), Some(0.2));
        assert_eq!(source.next(), Some(-0.7));
        assert_eq!(f32::from_bits(peak.load(Ordering::Relaxed)), 0.7);
        source.try_seek(Duration::ZERO).unwrap();
        assert_eq!(source.next(), Some(0.2));
    }

    #[test]
    fn media_clock_stays_in_source_time_when_speed_changes_and_seeks() {
        let clock = Arc::new(AtomicU64::new(0));
        let (source, _control) = loops::Transport::new(
            rodio::buffer::SamplesBuffer::new(1, 100, vec![0.1; 1000]),
            clock.clone(),
            Arc::new(AtomicBool::new(false)),
        );
        let mut source = source.speed(2.0);
        for _ in 0..100 {
            source.next();
        }
        assert_eq!(clock.load(Ordering::Relaxed), 100);
        source.set_factor(0.5);
        for _ in 0..50 {
            source.next();
        }
        assert_eq!(clock.load(Ordering::Relaxed), 150);
        source.try_seek(Duration::from_secs_f32(3.0 / 0.5)).unwrap();
        assert_eq!(clock.load(Ordering::Relaxed), 300);
        source.next();
        assert_eq!(clock.load(Ordering::Relaxed), 301);
    }
}
