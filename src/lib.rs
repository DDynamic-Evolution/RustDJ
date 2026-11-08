pub mod analysis;
pub mod app;
pub mod audio;
pub mod beat;
pub mod browser;
pub mod capture;
pub mod controller_profiles;
pub mod credentials;
pub mod database;
pub mod decks;
pub mod dragdrop;
pub mod dsp;
pub mod effects;
pub mod equalizer;
pub mod key;
pub mod loops;
pub mod midi;
pub mod output;
pub mod performance;
pub mod playlist;
pub mod settings;
mod stream_transport;
pub mod streamer;
pub mod track;
pub mod waveform;

#[cfg(test)]
mod release11_tests {
    use super::*;
    use std::{
        path::Path,
        sync::{Arc, atomic::AtomicBool},
        time::Duration,
    };

    #[test]
    fn release11_waveform_bands_detail_extrema_and_database_reuse() {
        for (frequency, band) in [(80.0, 0), (1000.0, 1), (10000.0, 2)] {
            let mut builder = waveform::Builder::new(48000, 2);
            for frame in 0..48000 {
                let value =
                    0.45 * (std::f32::consts::TAU * frequency * frame as f32 / 48000.0).sin();
                builder.push(value);
                builder.push(-value);
            }
            let wave = builder.finish();
            assert!(wave.step <= 0.00101);
            assert_eq!(wave.frames().len(), 1000);
            let merged = wave.sample(0.2, 0.8).unwrap();
            assert_eq!(
                merged
                    .bands
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, v)| *v)
                    .unwrap()
                    .0,
                band,
                "{frequency}: {:?}",
                merged.bands
            );
            assert!(merged.min < -14000 && merged.max > 14000);
            let encoded = wave.encode().unwrap();
            let restored =
                waveform::Waveform::decode(&encoded, wave.frames().len(), wave.step).unwrap();
            assert_eq!(restored.frames(), wave.frames());
            assert_eq!(restored.sample(0.2, 0.8), Some(merged));
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("track.flac");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo-b.flac"),
            &path,
        )
        .unwrap();
        let identity = database::Fingerprint::read(&path).unwrap();
        let db_path = temp.path().join("library.sqlite3");
        let track = track::analyze(path.clone(), &AtomicBool::new(false)).unwrap();
        let mut cues = [None; 8];
        cues[2] = Some(Duration::from_millis(1234));
        {
            let db = database::Database::open(&db_path).unwrap();
            db.save_hotcues(&path, &identity, cues).unwrap();
            db.save(&track, &identity).unwrap();
        }
        let db = database::Database::open(&db_path).unwrap();
        let loaded = db.load_track(&path, &identity).unwrap().unwrap();
        assert!(loaded.from_cache);
        assert_eq!(loaded.detailed.frames(), track.detailed.frames());
        assert_eq!(db.hotcues(&path, &identity).unwrap(), cues);
        rusqlite::Connection::open(&db_path)
            .unwrap()
            .execute("DELETE FROM colour_waveforms", [])
            .unwrap();
        assert!(matches!(
            db.lookup(&path, &identity).unwrap(),
            database::Lookup::Fresh(_)
        ));
        assert!(db.load_track(&path, &identity).unwrap().is_none());
        assert_eq!(db.hotcues(&path, &identity).unwrap(), cues);
    }

    #[test]
    fn release11_audio_filter_effects_midi_and_disabled_crossfader() {
        fn render(data: Vec<f32>, filter: f32, enabled: [bool; 3]) -> Vec<f32> {
            let params = Arc::new(effects::Params::default());
            params.set(filter, 1.0, enabled, 1.0, 120.0);
            effects::Effects::new(rodio::buffer::SamplesBuffer::new(2, 48000, data), params)
                .collect()
        }
        fn tone(frequency: f32) -> Vec<f32> {
            (0..48000)
                .flat_map(|i| {
                    [
                        (std::f32::consts::TAU * frequency * i as f32 / 48000.0).sin() * 0.2,
                        0.0,
                    ]
                })
                .collect()
        }
        let data = tone(1000.0);
        assert_eq!(render(data.clone(), 0.0, [false; 3]), data);
        let energy = |v: &[f32]| {
            v.iter()
                .skip(24000)
                .step_by(2)
                .map(|s| (*s as f64).powi(2))
                .sum::<f64>()
                .sqrt()
        };
        let low = tone(40.0);
        let high = tone(15000.0);
        assert!(energy(&render(high.clone(), -1.0, [false; 3])) < energy(&high) * 0.02);
        assert!(energy(&render(low.clone(), 1.0, [false; 3])) < energy(&low) * 0.02);
        assert!(energy(&render(low.clone(), -1.0, [false; 3])) > energy(&low) * 0.8);
        assert!(energy(&render(high.clone(), 1.0, [false; 3])) > energy(&high) * 0.8);
        for slot in 0..3 {
            let mut impulse = vec![0.0; 48000];
            impulse[0] = 0.5;
            let mut enabled = [false; 3];
            enabled[slot] = true;
            let output = render(impulse, 0.0, enabled);
            assert!(output.iter().all(|v| v.is_finite() && v.abs() < 2.0));
            assert!(output.iter().skip(1).step_by(2).all(|v| *v == 0.0));
            assert!(
                output
                    .iter()
                    .skip(128)
                    .step_by(2)
                    .map(|v| v.abs())
                    .sum::<f32>()
                    > 0.01,
                "effect {slot}"
            );
        }
        for cross in [-1.0, 0.0, 1.0] {
            let gains = dsp::bus_gains_with_crossfader(
                [0.8, 0.6],
                [true, false],
                cross,
                0.5,
                0.4,
                0.0,
                false,
            );
            assert_eq!(gains.master, [0.4, 0.3]);
            assert_eq!(gains.phones, [0.4, 0.0]);
        }
        let old: settings::MixSettings = serde_json::from_str("{}").unwrap();
        assert!(old.crossfader_enabled);
        let disabled = settings::MixSettings {
            crossfader_enabled: false,
            ..old
        };
        let stored = serde_json::to_string(&disabled).unwrap();
        assert!(
            !serde_json::from_str::<settings::MixSettings>(&stored)
                .unwrap()
                .crossfader_enabled
        );
        let profile = midi::dj202_profile();
        let mut mapper = midi::Mapper::default();
        for deck in 0..2 {
            for slot in 0..3 {
                assert_eq!(
                    mapper.decode([0x98 + deck as u8, slot as u8, 127], &profile),
                    vec![(midi::Action::FxToggle(deck, slot), 1.0)]
                );
                assert!(
                    mapper
                        .decode([0x98 + deck as u8, slot as u8, 0], &profile)
                        .is_empty()
                );
            }
            assert_eq!(
                mapper.decode([0xb0 + deck as u8, 0x1a, 127], &profile),
                vec![(midi::Action::Filter(deck), 1.0)]
            );
            assert_eq!(
                mapper.decode([0xb8 + deck as u8, 0, 127], &profile),
                vec![(midi::Action::FxLevel(deck), 1.0)]
            );
        }
        let old: Vec<_> = profile
            .into_iter()
            .filter(|b| {
                !matches!(
                    b.action,
                    midi::Action::Filter(_) | midi::Action::FxLevel(_) | midi::Action::FxToggle(..)
                )
            })
            .collect();
        let upgraded = midi::upgrade(old);
        assert!(
            upgraded
                .iter()
                .any(|b| b.action == midi::Action::FxToggle(1, 2))
        );
        assert_eq!(midi::upgrade(upgraded.clone()).len(), upgraded.len());
    }
}
