use crate::beat::{self, BeatGrid};
use crate::key::{KeyAnalyzer, MusicalKey};
use rodio::{Decoder, Source};
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub const EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "ogg", "m4a", "aac", "aiff"];

#[derive(Clone, Debug)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub duration: Duration,
    pub waveform: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub grid: Option<BeatGrid>,
    pub envelope: Arc<[f32]>,
    pub envelope_step: f64,
    pub key: Option<MusicalKey>,
    pub from_cache: bool,
    pub cache_warning: Option<String>,
    pub detailed: Arc<crate::waveform::Waveform>,
}

pub fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>, String> {
    let file = File::open(path).map_err(|e| format!("Datei nicht lesbar: {e}"))?;
    Decoder::try_from(file).map_err(|e| format!("Audioformat nicht lesbar: {e}"))
}

pub fn analyze(path: PathBuf, cancelled: &AtomicBool) -> Result<Track, String> {
    let decoder = open_decoder(&path)?;
    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels();
    if sample_rate == 0 || channels == 0 {
        return Err("Ungültige Audio-Abtastrate oder Kanalzahl.".into());
    }
    let mut key_analysis = KeyAnalyzer::new(
        sample_rate,
        decoder.total_duration().map(|d| d.as_secs_f64()),
    );
    let mut mono = 0.0_f64;
    let mut detailed = crate::waveform::Builder::new(sample_rate, channels);
    // Store only one peak per ~10 ms, not the decoded audio of an entire track.
    let bucket_samples = (sample_rate as usize / 100).max(1) * channels as usize;
    let mut peaks = Vec::new();
    let mut peak = 0.0_f32;
    let mut energy_sum = 0.0_f64;
    let mut energies = Vec::new();
    let mut count = 0_u64;
    for sample in decoder {
        detailed.push(sample);
        if count.is_multiple_of(4096) && cancelled.load(Ordering::Relaxed) {
            return Err("Laden abgebrochen.".into());
        }
        peak = peak.max(sample.abs().min(1.0));
        energy_sum += (sample as f64).powi(2);
        count += 1;
        mono += sample as f64;
        if count.is_multiple_of(channels as u64) {
            key_analysis.push((mono / channels as f64) as f32);
            mono = 0.0;
        }
        if count.is_multiple_of(bucket_samples as u64) {
            peaks.push(peak);
            if energies.len() < 12100 {
                energies.push((energy_sum / bucket_samples as f64).sqrt() as f32);
            }
            energy_sum = 0.0;
            peak = 0.0;
        }
    }
    if count == 0 {
        return Err("Die Datei enthält keine dekodierbaren Audiodaten.".into());
    }
    if !count.is_multiple_of(bucket_samples as u64) {
        peaks.push(peak);
    }
    let duration = Duration::from_secs_f64(count as f64 / channels as f64 / sample_rate as f64);
    let title = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let step = bucket_samples as f64 / channels as f64 / sample_rate as f64;
    let grid = beat::detect(&energies, step);
    if cancelled.load(Ordering::Relaxed) {
        return Err("Laden abgebrochen.".into());
    }
    let key = key_analysis.finish();
    Ok(Track {
        path,
        title,
        duration,
        waveform: reduce_peaks(&peaks, 900),
        sample_rate,
        channels,
        grid,
        envelope: peaks.into(),
        envelope_step: step,
        key,
        from_cache: false,
        cache_warning: None,
        detailed: Arc::new(detailed.finish()),
    })
}

fn reduce_peaks(peaks: &[f32], limit: usize) -> Vec<f32> {
    if peaks.len() <= limit {
        return peaks.to_vec();
    }
    (0..limit)
        .map(|i| {
            let start = i * peaks.len() / limit;
            let end = (i + 1) * peaks.len() / limit;
            peaks[start..end].iter().copied().fold(0.0, f32::max)
        })
        .collect()
}

pub fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| EXTENSIONS.iter().any(|ext| ext.eq_ignore_ascii_case(s)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_reduction_preserves_transient_peaks_and_tail() {
        let mut input = vec![0.1; 10001];
        input[777] = 0.95;
        input[10000] = 0.8;
        let reduced = reduce_peaks(&input, 900);
        assert_eq!(reduced.len(), 900);
        assert!(reduced.contains(&0.95));
        assert_eq!(reduced.last(), Some(&0.8));
    }
}
