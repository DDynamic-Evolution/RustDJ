use rodio::Source;
use rustdj::track::{analyze, open_decoder};
use std::{path::PathBuf, sync::atomic::AtomicBool, time::Duration};

fn demo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/demo-a.wav")
}

#[test]
fn real_wav_analysis_produces_correct_metadata_and_waveform() {
    let track = analyze(demo(), &AtomicBool::new(false)).unwrap();
    assert_eq!(track.duration, Duration::from_secs(8));
    assert_eq!(track.sample_rate, 44100);
    assert_eq!(track.channels, 2);
    assert!(!track.waveform.is_empty());
    assert!(track.waveform.len() <= 900);
    assert!(
        track
            .waveform
            .iter()
            .all(|p| p.is_finite() && (0.0..=1.0).contains(p))
    );
    assert!(track.waveform.iter().any(|p| *p > 0.1));
}

#[test]
fn decoder_can_seek_and_still_produce_audio() {
    let mut decoder = open_decoder(&demo()).unwrap();
    decoder.try_seek(Duration::from_secs(4)).unwrap();
    assert!(decoder.take(4410).any(|sample| sample.abs() > 0.01));
}

#[test]
fn cancelled_analysis_stops_and_missing_files_report_error() {
    assert!(analyze(demo(), &AtomicBool::new(true)).is_err());
    assert!(analyze(demo().with_extension("missing"), &AtomicBool::new(false)).is_err());
}

#[test]
fn real_mp3_and_flac_decode_analyze_and_seek() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
    for file in ["demo-a.mp3", "demo-b.flac"] {
        let path = root.join(file);
        let track = analyze(path.clone(), &AtomicBool::new(false)).unwrap();
        assert!(
            track.duration.abs_diff(Duration::from_secs(8)) < Duration::from_millis(100),
            "{file}: {:?}",
            track.duration
        );
        assert!(track.waveform.iter().any(|p| *p > 0.1));
        let grid = track.grid.expect("Demo beat grid should be detected");
        assert!((grid.bpm - 120.0).abs() < 1.0, "{file}: {grid:?}");
        let mut decoder = open_decoder(&path).unwrap();
        decoder.try_seek(Duration::from_secs(4)).unwrap();
        assert!(
            decoder.take(4410).any(|sample| sample.abs() > 0.01),
            "{file}"
        );
    }
}
