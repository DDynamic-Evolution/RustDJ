use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct TalkoverSettings {
    pub enabled: bool,
    pub reduction_db: f32,
    pub threshold_db: f32,
    pub release_ms: u32,
}

impl Default for TalkoverSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            reduction_db: 12.0,
            threshold_db: -36.0,
            release_ms: 450,
        }
    }
}

pub struct Talkover {
    gain: f32,
    last_voice: Option<Instant>,
    last_update: Option<Instant>,
}

impl Default for Talkover {
    fn default() -> Self {
        Self {
            gain: 1.0,
            last_voice: None,
            last_update: None,
        }
    }
}

impl Talkover {
    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn update(&mut self, settings: TalkoverSettings, mic_signal: Option<f32>, now: Instant) {
        let elapsed = self.last_update.replace(now).map_or(0.033, |last| {
            now.saturating_duration_since(last).as_secs_f32()
        });
        if !settings.enabled {
            self.gain = 1.0;
            self.last_voice = None;
            return;
        }
        if mic_signal.is_some_and(|peak| peak >= db_gain(settings.threshold_db.clamp(-60.0, -10.0)))
        {
            self.last_voice = Some(now);
        }
        let speaking = mic_signal.is_some()
            && self
                .last_voice
                .is_some_and(|last| now.saturating_duration_since(last).as_millis() < 220);
        let target = if speaking {
            db_gain(-settings.reduction_db.clamp(3.0, 24.0))
        } else {
            1.0
        };
        let time = if speaking {
            0.045
        } else {
            settings.release_ms.clamp(80, 2000) as f32 / 1000.0
        };
        self.gain += (target - self.gain) * (1.0 - (-elapsed / time).exp());
        if (target - self.gain).abs() < 0.001 {
            self.gain = target;
        }
    }
}

fn db_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

#[derive(Default)]
pub struct LimiterControl {
    enabled: AtomicBool,
    reduction: AtomicU32,
}

impl LimiterControl {
    pub fn set(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn take_reduction(&self) -> f32 {
        f32::from_bits(self.reduction.swap(0, Ordering::Relaxed))
    }
}

pub struct Limiter {
    gain: f32,
    release: f32,
}

impl Limiter {
    pub fn new(rate: u32) -> Self {
        Self {
            gain: 1.0,
            release: 1.0 - (-1.0 / (rate.max(1) as f32 * 0.100)).exp(),
        }
    }

    pub fn process(&mut self, control: &LimiterControl, frame: [f32; 2]) -> [f32; 2] {
        if !control.enabled.load(Ordering::Relaxed) {
            self.gain = 1.0;
            return frame;
        }
        // Link stereo channels to preserve their balance. Cap peaks immediately
        // at -1 dBFS without delaying the microphone.
        let frame = frame.map(|value| if value.is_finite() { value } else { 0.0 });
        let peak = frame[0].abs().max(frame[1].abs());
        let ceiling = 0.891_250_9;
        self.gain = (self.gain + (1.0 - self.gain) * self.release).min(ceiling / peak.max(ceiling));
        control
            .reduction
            .fetch_max((1.0 - self.gain).to_bits(), Ordering::Relaxed);
        frame.map(|value| value * self.gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn talkover_holds_between_words_then_releases() {
        let settings = TalkoverSettings {
            enabled: true,
            ..TalkoverSettings::default()
        };
        let mut duck = Talkover::default();
        let start = Instant::now();
        duck.update(settings, Some(0.0), start);
        duck.update(settings, Some(0.3), start + Duration::from_millis(120));
        assert!(duck.gain() < 0.35);
        duck.update(settings, Some(0.0), start + Duration::from_millis(260));
        assert!(duck.gain() < 0.35);
        duck.update(settings, None, start + Duration::from_secs(4));
        assert_eq!(duck.gain(), 1.0);
    }

    #[test]
    fn limiter_caps_stereo_preserving_balance() {
        let control = LimiterControl::default();
        let mut limiter = Limiter::new(48_000);
        assert_eq!(limiter.process(&control, [2.0, -0.5]), [2.0, -0.5]);
        control.set(true);
        let frame = limiter.process(&control, [2.0, -0.5]);
        assert!((frame[0] - db_gain(-1.0)).abs() < 0.0001);
        assert_eq!(frame[0] / frame[1], -4.0);
        assert!(control.take_reduction() > 0.5);
        assert!(limiter.process(&control, [0.5, 0.5])[0] < 0.3);
    }
}
