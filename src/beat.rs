//! Lightweight constant-tempo analysis on a 100 Hz RMS envelope.
//! Not a downbeat detector: grids can be corrected by the user.
use std::time::Duration;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct BeatGrid {
    pub bpm: f32,
    pub offset: f64,
    pub confidence: f32,
}

impl BeatGrid {
    pub fn period(self) -> f64 {
        60.0 / self.bpm as f64
    }
    pub fn phase(self, position: Duration) -> f64 {
        ((position.as_secs_f64() - self.offset) / self.period()).rem_euclid(1.0)
    }
}

pub fn detect(energy: &[f32], step: f64) -> Option<BeatGrid> {
    if energy.len() < 400 || !step.is_finite() || step <= 0.0 {
        return None;
    }
    // Bounded work, even for multi-hour recordings.
    let energy = &energy[..energy.len().min((120.0 / step) as usize)];
    let mut onsets = vec![0.0_f32; energy.len()];
    for i in 1..energy.len() {
        let start = i.saturating_sub(4);
        let baseline = energy[start..i].iter().sum::<f32>() / (i - start) as f32;
        onsets[i] = (energy[i] - baseline).max(0.0);
    }
    let maximum = onsets.iter().copied().fold(0.0_f32, f32::max);
    if maximum < 0.0001 || onsets.iter().filter(|v| **v > maximum * 0.3).count() < 4 {
        return None;
    }
    let power = onsets.iter().map(|v| v * v).sum::<f32>();
    let mut strong = Vec::new();
    for (i, onset) in onsets.iter().enumerate() {
        if *onset > maximum * 0.5
            && strong
                .last()
                .is_none_or(|last| (i - *last) as f64 * step > 0.2)
        {
            strong.push(i);
        }
    }
    let mut intervals: Vec<_> = strong
        .windows(2)
        .map(|pair| (pair[1] - pair[0]) as f64 * step)
        .collect();
    intervals.sort_by(f64::total_cmp);
    let interval_bpm = intervals
        .get(intervals.len() / 2)
        .map(|interval| {
            let mut bpm = 60.0 / interval;
            while bpm < 70.0 {
                bpm *= 2.0;
            }
            while bpm > 180.0 {
                bpm /= 2.0;
            }
            bpm
        })
        .unwrap_or(120.0);
    let correlation = |lag: f64| -> f32 {
        let lag_int = lag.floor() as usize;
        if lag_int + 1 >= onsets.len() {
            return 0.0;
        }
        let fraction = lag.fract() as f32;
        let count = onsets.len() - lag_int - 1;
        let sum: f32 = (0..count)
            .map(|i| {
                onsets[i]
                    * (onsets[i + lag_int] * (1.0 - fraction) + onsets[i + lag_int + 1] * fraction)
            })
            .sum();
        sum / power * onsets.len() as f32 / count as f32
    };
    let mut best = (0.0_f32, 120.0_f32);
    for candidate in 700..=1800 {
        let bpm = candidate as f32 / 10.0;
        let lag = 60.0 / bpm as f64 / step;
        let score =
            (correlation(lag) + 0.5 * correlation(2.0 * lag) + 0.25 * correlation(3.0 * lag))
                / 1.75;
        // Prefer strong-onset spacing when half-tempo candidates are equally periodic.
        let score = score
            * (1.0 + 0.02 * (-0.5 * ((bpm as f64 - interval_bpm) / 8.0).powi(2)).exp() as f32);
        if score > best.0 {
            best = (score, bpm);
        }
    }
    if best.0 < 0.16 {
        return None;
    }
    let period = 60.0 / best.1 as f64;
    let bins = (period / step).round() as usize;
    let mut phases = vec![0.0_f32; bins];
    for (i, onset) in onsets.iter().enumerate() {
        let bin =
            (((i as f64 * step).rem_euclid(period) / period * bins as f64).round() as usize) % bins;
        phases[bin] += onset;
    }
    let phase = phases
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?
        .0;
    let mut period = period;
    let mut offset = phase as f64 / bins as f64 * period;
    // Refine the coarse autocorrelation using weighted onset/beat regression.
    // This removes tempo quantization and phase drift on non-integer frame periods.
    for _ in 0..2 {
        let (mut w, mut x, mut y, mut xx, mut xy) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, onset) in onsets.iter().enumerate() {
            if *onset < maximum * 0.25 {
                continue;
            }
            let time = i as f64 * step;
            let beat = ((time - offset) / period).round();
            if (time - (offset + beat * period)).abs() > period * 0.18 {
                continue;
            }
            let weight = *onset as f64;
            w += weight;
            x += weight * beat;
            y += weight * time;
            xx += weight * beat * beat;
            xy += weight * beat * time;
        }
        let denominator = w * xx - x * x;
        if denominator <= 0.0 {
            break;
        }
        let refined = (w * xy - x * y) / denominator;
        if (refined / period - 1.0).abs() > 0.02 {
            break;
        }
        period = refined;
        offset = ((y - period * x) / w).rem_euclid(period);
    }
    Some(BeatGrid {
        bpm: (60.0 / period) as f32,
        offset,
        confidence: best.0.clamp(0.0, 1.0),
    })
}

#[derive(Debug)]
pub struct SyncPlan {
    pub speed: f32,
    pub position: Duration,
}

pub fn sync_plan(
    follower: BeatGrid,
    position: Duration,
    master: BeatGrid,
    master_position: Duration,
    master_speed: f32,
) -> Result<SyncPlan, String> {
    if !follower.bpm.is_finite()
        || !master.bpm.is_finite()
        || !master_speed.is_finite()
        || follower.bpm <= 0.0
        || master.bpm <= 0.0
        || master_speed <= 0.0
    {
        return Err("Für Sync sind gültige BPM auf beiden Decks erforderlich.".into());
    }
    let speed = master.bpm * master_speed / follower.bpm;
    if !(0.5..=2.0).contains(&speed) {
        return Err("Sync würde den Tempobereich von 50–200 % überschreiten. BPM prüfen.".into());
    }
    let phase_delta =
        (master.phase(master_position) - follower.phase(position) + 0.5).rem_euclid(1.0) - 0.5;
    let mut seconds = position.as_secs_f64() + phase_delta * follower.period();
    if seconds < 0.0 {
        seconds += follower.period();
    }
    Ok(SyncPlan {
        speed,
        position: Duration::from_secs_f64(seconds.max(0.0)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_constant_tempo_and_rejects_silence() {
        for bpm in [90.0, 120.0, 128.0, 150.0] {
            let mut energy = vec![0.0; 4000];
            for beat in 0..((40.0 * bpm / 60.0) as usize) {
                let index = ((beat as f64 * 60.0 / bpm + 0.13) * 100.0).round() as usize;
                if index < energy.len() {
                    energy[index] = 0.9;
                }
            }
            let grid = detect(&energy, 0.01).unwrap();
            assert!((grid.bpm as f64 - bpm).abs() < 0.6, "{bpm}: {grid:?}");
            assert!((grid.offset - 0.13).abs() < 0.04, "{bpm}: {grid:?}");
        }
        assert!(detect(&vec![0.0; 4000], 0.01).is_none());
        assert!(detect(&vec![0.5; 4000], 0.01).is_none());
    }

    #[test]
    fn sync_matches_effective_tempo_and_nearest_beat_phase() {
        let a = BeatGrid {
            bpm: 120.0,
            offset: 0.1,
            confidence: 1.0,
        };
        let b = BeatGrid {
            bpm: 128.0,
            offset: 0.2,
            confidence: 1.0,
        };
        let target = Duration::from_secs_f64(7.23);
        let plan = sync_plan(b, Duration::from_secs_f64(12.8), a, target, 1.1).unwrap();
        assert!((b.bpm * plan.speed - 132.0).abs() < 0.001);
        assert!((b.phase(plan.position) - a.phase(target)).abs() < 0.0001);
        assert!((plan.position.as_secs_f64() - 12.8).abs() < b.period() / 2.0);
        assert!(sync_plan(b, Duration::ZERO, a, Duration::ZERO, 8.0).is_err());
    }
}
