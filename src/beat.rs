//! Constant-tempo beat grids from frequency-band transients and periodicity.
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
    if !step.is_finite() || step <= 0.0 {
        return None;
    }
    let energy = &energy[..energy.len().min((1200.0 / step) as usize)];
    detect_onsets(&onset_strength(energy), step)
}

pub(crate) fn detect_bands(energy: &[Vec<f32>; 3], step: f64) -> Option<BeatGrid> {
    let mut onsets = vec![0.0; energy[0].len()];
    let maxima = energy
        .each_ref()
        .map(|band| band.iter().copied().fold(0.0_f32, f32::max));
    let loudest = maxima.into_iter().fold(0.0_f32, f32::max);
    for ((band, maximum), weight) in energy.iter().zip(maxima).zip([0.58, 0.30, 0.12]) {
        if maximum < 0.0001 {
            continue;
        }
        // A quiet band can support the beat without dominating the phase.
        let weight = weight * (3.0 * (maximum / loudest).sqrt()).min(1.0);
        for (total, onset) in onsets.iter_mut().zip(onset_strength(band)) {
            *total += onset * weight;
        }
    }
    detect_onsets(&onsets, step)
}

fn onset_strength(energy: &[f32]) -> Vec<f32> {
    let mut onsets = vec![0.0; energy.len()];
    let maximum = energy.iter().copied().fold(0.0_f32, f32::max);
    if maximum < 0.0001 {
        return onsets;
    }
    let mut previous = (energy[0] * 5.0 / maximum).ln_1p();
    let mut tail = 0.0_f32;
    for (onset, &rms) in onsets[1..].iter_mut().zip(&energy[1..]) {
        let value = (rms * 5.0 / maximum).ln_1p();
        let rise = (value - previous).max(0.0);
        *onset = (rise - tail * 0.5).max(0.0);
        tail = (tail * 0.55).max(rise);
        previous = previous * 0.5 + value * 0.5;
    }
    // Normalize each band by a strong typical transient instead of a single spike.
    let peak = onsets.iter().copied().fold(0.0_f32, f32::max);
    let mut accents: Vec<_> = onsets
        .iter()
        .copied()
        .filter(|v| *v > peak * 0.10)
        .collect();
    if !accents.is_empty() {
        let rank = (accents.len() - 1) * 9 / 10;
        let scale = *accents.select_nth_unstable_by(rank, f32::total_cmp).1;
        for onset in &mut onsets {
            *onset = (*onset / scale).min(1.5);
        }
    }
    onsets
}

fn detect_onsets(onsets: &[f32], step: f64) -> Option<BeatGrid> {
    if !step.is_finite() || step <= 0.0 || onsets.len() as f64 * step < 3.8 {
        return None;
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
            while bpm < 75.0 {
                bpm *= 2.0;
            }
            while bpm > 185.0 {
                bpm /= 2.0;
            }
            bpm
        })
        .unwrap_or(120.0);
    // Autocorrelate once per integer lag, then interpolate for fractional BPM.
    let max_lag = ((3.0 * 60.0 / 55.0 / step).ceil() as usize + 1).min(onsets.len() - 1);
    let correlations: Vec<f32> = (0..=max_lag)
        .map(|lag| {
            let tail = &onsets[lag..];
            let sum = onsets.iter().zip(tail).map(|(a, b)| a * b).sum::<f32>();
            sum / power * onsets.len() as f32 / tail.len() as f32
        })
        .collect();
    let correlation = |lag: f64| -> f32 {
        let index = lag.floor() as usize;
        let fraction = lag.fract() as f32;
        correlations
            .get(index)
            .zip(correlations.get(index + 1))
            .map_or(0.0, |(&a, &b)| a + fraction * (b - a))
    };
    let mut best = (0.0_f32, 120.0_f32);
    for candidate in 550..=2000 {
        let bpm = candidate as f32 / 10.0;
        let lag = 60.0 / bpm as f64 / step;
        let score =
            (correlation(lag) + 0.5 * correlation(2.0 * lag) + 0.25 * correlation(3.0 * lag))
                / 1.75;
        // Prefer strong-onset spacing when half-tempo candidates are equally periodic.
        let interval_prior = (-0.5 * ((bpm as f64 - interval_bpm) / 9.0).powi(2)).exp() as f32;
        let tempo_prior = (-0.5 * ((bpm / 120.0).log2() / 0.5).powi(2)).exp();
        let score = score * (1.0 + 0.07 * interval_prior + 0.035 * tempo_prior);
        if score > best.0 {
            best = (score, bpm);
        }
    }
    if best.0 < 0.16 {
        return None;
    }
    let period = 60.0 / best.1 as f64;
    let bins = ((period / step).round() as usize).max(1);
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
        return Err("Sync requires valid BPM values on both decks.".into());
    }
    let speed = master.bpm * master_speed / follower.bpm;
    if !(0.5..=2.0).contains(&speed) {
        return Err("Sync would exceed the 50–200% tempo range. Check the BPM values.".into());
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
    fn bass_pulse_anchors_grid_against_louder_offbeat_hats() {
        let mut energy = std::array::from_fn(|_| vec![0.0; 3600]);
        let period = 60.0 / 124.0;
        for beat in 0..70 {
            let time = 0.17 + beat as f64 * period;
            let kick = (time * 100.0).round() as usize;
            let hat = ((time + period * 0.5) * 100.0).round() as usize;
            if beat % 7 != 3 {
                energy[0][kick] = 0.22;
            }
            energy[1][kick] = 0.12;
            energy[2][hat] = 0.91;
        }
        let grid = detect_bands(&energy, 0.01).unwrap();
        assert!((grid.bpm - 124.0).abs() < 0.6, "{grid:?}");
        assert!((grid.offset - 0.17).abs() < 0.025, "{grid:?}");
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
