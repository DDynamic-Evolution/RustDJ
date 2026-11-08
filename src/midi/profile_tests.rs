use super::*;
use crate::controller_profiles::ProfileFile;

#[test]
fn profiles21_documented_inputs_and_device_isolation() {
    for profile in ProfileId::ALL {
        let bindings = profile.defaults();
        ProfileFile {
            schema_version: 1,
            profile,
            bindings: bindings.clone(),
        }
        .validate()
        .unwrap();
        for binding in &bindings {
            let mut mapper = Mapper::for_profile(profile);
            let mut message = [binding.status, binding.number, 127];
            let expected = match binding.mode {
                Mode::Button | Mode::Gate => 1.0,
                Mode::Absolute7 => 1.0,
                Mode::Absolute7Inverted => 0.0,
                Mode::Absolute14 { lsb } | Mode::Absolute14Inverted { lsb } => {
                    assert!(mapper.decode(message, &bindings).is_empty());
                    message[1] = lsb;
                    if matches!(binding.mode, Mode::Absolute14Inverted { .. }) {
                        0.0
                    } else {
                        1.0
                    }
                }
                Mode::Relative64 => {
                    message[2] = 65;
                    1.0
                }
                Mode::RelativeTwos => {
                    message[2] = 1;
                    1.0
                }
            };
            let decoded = mapper.decode(message, &bindings);
            let action = if let Action::DeckLayer(side) = binding.action {
                Action::SelectDeck(side + 2)
            } else {
                binding.action
            };
            assert_eq!(
                decoded,
                vec![(action, expected)],
                "{} / {:?}",
                profile.label(),
                binding.action
            );
            if matches!(binding.mode, Mode::Button | Mode::Gate) {
                message[2] = 0;
                let release = mapper.decode(message, &bindings);
                if binding.mode == Mode::Gate {
                    assert_eq!(release, vec![(binding.action, 0.0)]);
                } else {
                    assert!(release.is_empty());
                }
            }
        }
    }
    for profile in [ProfileId::PioneerDdj400, ProfileId::PioneerDdjFlx4] {
        let mut midi = Midi::unconnected(
            None,
            MidiPrefs {
                profile: Some(profile),
                ..Default::default()
            },
        );
        midi.connected = Some(profile.label().into());
        let (sender, receiver) = mpsc::channel();
        midi.receiver = Some(receiver);
        // Pioneer pad 1 is Roland's deck-four mode message; it must not be eaten.
        sender.send([0x97, 0, 127]).unwrap();
        assert_eq!(midi.poll(), vec![(Action::Hotcue(0, 0), 1.0)]);
        sender.send([0x99, 0x67, 127]).unwrap();
        assert_eq!(midi.poll(), vec![(Action::LoopPad(1, 7), 1.0)]);
        assert_eq!(midi.pad_modes, [None; 4]);
        midi.mapper.decode([0xb6, 0x1f, 64], &midi.bindings);
        let center = midi.mapper.decode([0xb6, 0x3f, 0], &midi.bindings);
        assert!((center[0].1 - 0.5).abs() < 0.0001);
        assert!(profile.initialization().is_empty());
        let expected = if profile == ProfileId::PioneerDdj400 {
            Action::LoopToggle(0)
        } else {
            Action::LoopPreset(0, 4)
        };
        assert_eq!(
            midi.mapper.decode([0x90, 0x4d, 127], &midi.bindings),
            vec![(expected, 1.0)]
        );
    }
    for profile in [ProfileId::RolandDj505, ProfileId::NumarkMixtrackPlatinumFx] {
        let mut midi = Midi::unconnected(
            None,
            MidiPrefs {
                profile: Some(profile),
                ..Default::default()
            },
        );
        let (sender, receiver) = mpsc::channel();
        midi.receiver = Some(receiver);
        let value = if profile == ProfileId::NumarkMixtrackPlatinumFx {
            1
        } else {
            65
        };
        assert_eq!(
            midi.mapper.decode([0xb0, 6, value], &midi.bindings),
            vec![(Action::Jog(0), 1.0)]
        );
        for deck in [2, 0, 3, 1] {
            let pad = 0x94 + deck as u8;
            sender.send([pad, 0x14, 127]).unwrap();
            assert!(midi.poll().is_empty());
            sender.send([pad, 0, 127]).unwrap();
            assert!(midi.poll().is_empty());
            for slot in 0..8 {
                sender.send([pad, 0x14 + slot as u8, 127]).unwrap();
                assert_eq!(midi.poll(), vec![(Action::Hotcue(deck, slot), 1.0)]);
            }
            sender.send([pad, 0x0d, 127]).unwrap();
            assert!(midi.poll().is_empty());
            for slot in 0..8 {
                sender.send([pad, 0x14 + slot as u8, 127]).unwrap();
                assert_eq!(midi.poll(), vec![(Action::LoopPad(deck, slot), 1.0)]);
            }
            sender.send([pad, 0x0b, 127]).unwrap();
            assert!(midi.poll().is_empty());
            sender.send([pad, 0x14, 127]).unwrap();
            assert!(midi.poll().is_empty());
        }
    }
    let mut midi = Midi::unconnected(
        None,
        MidiPrefs {
            profile: Some(ProfileId::NumarkMixtrackPro3),
            ..Default::default()
        },
    );
    let (sender, receiver) = mpsc::channel();
    midi.receiver = Some(receiver);
    sender.send([0x91, 0x0b, 127]).unwrap();
    assert!(midi.poll().is_empty());
    sender.send([0x91, 0x1b, 127]).unwrap();
    assert_eq!(midi.poll(), vec![(Action::ClearHotcue(0, 0), 1.0)]);
    sender.send([0x81, 0x0b, 0]).unwrap();
    assert!(midi.poll().is_empty());
    sender.send([0x91, 0x1b, 127]).unwrap();
    assert_eq!(midi.poll(), vec![(Action::Hotcue(0, 0), 1.0)]);
    let mut generic = Mapper::for_profile(ProfileId::Generic);
    generic.decode([0x92, 0, 127], &dj202_profile());
    assert_eq!(
        generic.decode([0x98, 0, 127], &dj202_profile()),
        vec![(Action::FxToggle(0, 0), 1.0)]
    );
    assert!(ProfileId::Generic.initialization().is_empty());
    assert!(
        crate::controller_profiles::basic_leds(
            ProfileId::Generic,
            [DeckLeds::default(); 4],
            [false; 4]
        )
        .is_empty()
    );
    let mut decks = [DeckLeds::default(); 4];
    decks[0].playing = true;
    assert!(
        crate::controller_profiles::basic_leds(ProfileId::PioneerDdj400, decks, [false; 4])
            .contains(&[0x90, 0x0b, 127])
    );
}

#[test]
fn profiles21_saved_profiles_legacy_migration_import_and_cue_release() {
    let legacy = vec![Binding {
        status: 0xb4,
        number: 70,
        mode: Mode::Absolute7,
        action: Action::Master,
    }];
    let mut midi = Midi::unconnected(Some(legacy.clone()), MidiPrefs::default());
    for (port, profile) in [
        ("My MIDI keyboard", ProfileId::Generic),
        ("Numark Mixtrack Pro 3", ProfileId::NumarkMixtrackPro3),
    ] {
        let old = Midi::unconnected(
            Some(legacy.clone()),
            MidiPrefs {
                preferred_port: Some(port.into()),
                ..Default::default()
            },
        );
        assert_eq!(old.active_profile, profile);
        assert_eq!(
            serde_json::to_string(&old.bindings).unwrap(),
            serde_json::to_string(&legacy).unwrap()
        );
    }
    midi.activate_profile(ProfileId::PioneerDdj400);
    midi.prefs.profile = Some(ProfileId::PioneerDdj400);
    let (sender, receiver) = mpsc::channel();
    midi.receiver = Some(receiver);
    midi.learning = Some(Action::Master);
    midi.learn_mode = Mode::Absolute7Inverted;
    sender.send([0xb4, 60, 100]).unwrap();
    assert!(midi.poll().is_empty());
    assert!(midi.learning.is_none());
    assert!(
        midi.bindings
            .iter()
            .any(|b| b.action == Action::Master && b.mode == Mode::Absolute7Inverted)
    );
    sender.send([0x90, 0x0c, 127]).unwrap();
    assert_eq!(midi.poll(), vec![(Action::Cue(0), 1.0)]);
    let pioneer = midi.bindings.clone();
    midi.activate_profile(ProfileId::RolandDj202);
    assert_eq!(midi.poll(), vec![(Action::Cue(0), 0.0)]);
    assert_eq!(
        serde_json::to_string(&midi.bindings).unwrap(),
        serde_json::to_string(&legacy).unwrap()
    );
    midi.activate_profile(ProfileId::PioneerDdj400);
    midi.save_current_profile();
    let directory = tempfile::Builder::new()
        .prefix("profiles21-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap();
    let settings = crate::settings::Settings {
        midi: midi.prefs.clone(),
        bindings: Some(midi.bindings.clone()),
        mapping_version: 7,
        ..Default::default()
    };
    let settings_path = directory.path().join("settings.json");
    crate::settings::save_to(&settings_path, &settings).unwrap();
    let (loaded, error) = crate::settings::load_from(&settings_path);
    assert!(error.is_none());
    let mut restored = Midi::unconnected(loaded.bindings, loaded.midi);
    assert_eq!(restored.active_profile, ProfileId::PioneerDdj400);
    assert_eq!(
        serde_json::to_string(&restored.bindings).unwrap(),
        serde_json::to_string(&pioneer).unwrap()
    );
    restored.activate_profile(ProfileId::Generic);
    restored.save_current_profile();
    restored.activate_profile(ProfileId::RolandDj202);
    restored.activate_profile(ProfileId::Generic);
    assert!(restored.bindings.is_empty());
    let path = directory.path().join("controller.json");
    let file = ProfileFile {
        schema_version: 1,
        profile: ProfileId::PioneerDdj400,
        bindings: pioneer,
    };
    file.write(&path).unwrap();
    let mut imported = ProfileFile::read(&path).unwrap();
    assert_eq!(imported.profile, file.profile);
    assert_eq!(
        serde_json::to_string(&imported.bindings).unwrap(),
        serde_json::to_string(&file.bindings).unwrap()
    );
    imported.schema_version = 2;
    assert!(imported.validate().is_err());
    imported.schema_version = 1;
    imported.bindings.push(imported.bindings[0].clone());
    assert!(imported.validate().is_err());
    imported.bindings.pop();
    imported.bindings[0].action = Action::Play(8);
    assert!(imported.validate().is_err());
    std::fs::write(&path, vec![b' '; 1_048_577]).unwrap();
    assert!(ProfileFile::read(&path).is_err());
    let mut auto = Midi::unconnected(None, MidiPrefs::default());
    auto.ports = vec!["Unrelated keyboard".into(), "PIONEER DDJ-FLX4 MIDI".into()];
    assert_eq!(auto.automatic_port(), Some(1));
    auto.ports.push("Roland DJ-202".into());
    assert_eq!(auto.automatic_port(), None);
    auto.prefs.profile = Some(ProfileId::RolandDj202);
    assert_eq!(auto.automatic_port(), Some(2));
    auto.prefs.preferred_port = Some("Missing port".into());
    assert_eq!(auto.automatic_port(), None);
    assert_eq!(ProfileId::detect("DDJ-4000"), None);
    assert_eq!(ProfileId::detect("Numark Mixtrack Platinum (no FX)"), None);
    assert_eq!(
        ProfileId::detect("MIXTRACK PRO 3"),
        Some(ProfileId::NumarkMixtrackPro3)
    );
    assert_eq!(ProfileId::detect("DJ-808"), None);
}
