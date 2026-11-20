use crate::beat::{self, BeatGrid};
use crate::key::{KeyAnalyzer, MusicalKey};
use rodio::{Decoder, Source};
use serde::{Deserialize, Serialize};
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
use symphonia::core::{
    formats::FormatOptions,
    io::{MediaSourceStream, MediaSourceStreamOptions},
    meta::{MetadataOptions, MetadataRevision, StandardTagKey, Value},
    probe::Hint,
};

pub const EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "ogg", "m4a", "aac", "aiff"];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub values: Vec<String>,
}

impl Tags {
    fn read_revision(&mut self, revision: &MetadataRevision) {
        for tag in revision.tags() {
            if self.values.len() == 64 || matches!(tag.value, Value::Binary(_) | Value::Flag) {
                continue;
            }
            let value: String = tag.value.to_string().trim().chars().take(512).collect();
            if value.is_empty() {
                continue;
            }
            let primary = match tag.std_key {
                Some(StandardTagKey::TrackTitle) => Some(&mut self.title),
                Some(StandardTagKey::Artist) => Some(&mut self.artist),
                Some(StandardTagKey::Album) => Some(&mut self.album),
                Some(StandardTagKey::Genre) => Some(&mut self.genre),
                _ => None,
            };
            if let Some(field) = primary
                && field.is_empty()
            {
                *field = value.clone();
            }
            if !self.values.contains(&value) {
                self.values.push(value);
            }
        }
    }
}

pub fn read_tags(path: &Path) -> Tags {
    let mut tags = Tags::default();
    let Ok(file) = File::open(path) else {
        return tags;
    };
    let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }
    let options = MetadataOptions {
        limit_metadata_bytes: symphonia::core::meta::Limit::Maximum(256 * 1024),
        limit_visual_bytes: symphonia::core::meta::Limit::Maximum(4 * 1024 * 1024),
    };
    let Ok(mut probed) =
        symphonia::default::get_probe().format(&hint, stream, &FormatOptions::default(), &options)
    else {
        return tags;
    };
    if let Some(metadata) = probed.format.metadata().current() {
        tags.read_revision(metadata);
    }
    if let Some(probed_metadata) = probed.metadata.get()
        && let Some(metadata) = probed_metadata.current()
    {
        tags.read_revision(metadata);
    }
    tags
}

#[derive(Clone, Debug)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub tags: Option<Tags>,
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
    let file = File::open(path).map_err(|e| format!("Could not read file: {e}"))?;
    Decoder::try_from(file).map_err(|e| format!("Could not decode audio format: {e}"))
}

pub fn analyze(path: PathBuf, cancelled: &AtomicBool) -> Result<Track, String> {
    let decoder = open_decoder(&path)?;
    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels();
    if sample_rate == 0 || channels == 0 {
        return Err("Invalid audio sample rate or channel count.".into());
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
    let mut rhythm = [Vec::new(), Vec::new(), Vec::new()];
    let mut rhythm_energy = [0.0_f64; 3];
    let mut rhythm_frames = 0;
    let rhythm_step = 10.0 * (sample_rate / 1000).max(1) as f64 / sample_rate as f64;
    let rhythm_limit = (1200.0 / rhythm_step) as usize;
    let rhythm_samples = 10 * (sample_rate / 1000).max(1) as usize * channels as usize;
    let mut count = 0_u64;
    for sample in decoder {
        if let Some(energy) = detailed.push_with_energy(sample)
            && rhythm[0].len() < rhythm_limit
        {
            for (total, value) in rhythm_energy.iter_mut().zip(energy) {
                *total += value;
            }
            rhythm_frames += 1;
            if rhythm_frames == 10 {
                for (band, total) in rhythm.iter_mut().zip(rhythm_energy) {
                    band.push((total / rhythm_samples as f64).sqrt() as f32);
                }
                rhythm_frames = 0;
                rhythm_energy = [0.0; 3];
            }
        }
        if count.is_multiple_of(4096) && cancelled.load(Ordering::Relaxed) {
            return Err("Loading cancelled.".into());
        }
        peak = peak.max(sample.abs().min(1.0));
        count += 1;
        mono += sample as f64;
        if count.is_multiple_of(channels as u64) {
            key_analysis.push((mono / channels as f64) as f32);
            mono = 0.0;
        }
        if count.is_multiple_of(bucket_samples as u64) {
            peaks.push(peak);
            peak = 0.0;
        }
    }
    if count == 0 {
        return Err("The file contains no decodable audio data.".into());
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
    let grid = beat::detect_bands(&rhythm, rhythm_step);
    if cancelled.load(Ordering::Relaxed) {
        return Err("Loading cancelled.".into());
    }
    let key = key_analysis.finish();
    let tags = Some(read_tags(&path));
    Ok(Track {
        path,
        title,
        tags,
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
