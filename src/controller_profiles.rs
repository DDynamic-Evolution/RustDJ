//! Independent model-specific MIDI tables; sources and limits: CONTROLLERS.md.
use crate::midi::{Action as A, Binding, Mode as M};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProfileId {
    #[default]
    RolandDj202,
    RolandDj505,
    PioneerDdj400,
    PioneerDdjFlx4,
    NumarkMixtrackPro3,
    NumarkMixtrackPlatinumFx,
    Generic,
}

impl ProfileId {
    pub const ALL: [Self; 7] = [
        Self::PioneerDdj400,
        Self::PioneerDdjFlx4,
        Self::RolandDj202,
        Self::RolandDj505,
        Self::NumarkMixtrackPro3,
        Self::NumarkMixtrackPlatinumFx,
        Self::Generic,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::RolandDj202 => "Roland DJ-202",
            Self::RolandDj505 => "Roland DJ-505",
            Self::PioneerDdj400 => "Pioneer DDJ-400",
            Self::PioneerDdjFlx4 => "Pioneer DDJ-FLX4",
            Self::NumarkMixtrackPro3 => "Numark Mixtrack (Pro) 3",
            Self::NumarkMixtrackPlatinumFx => "Numark Mixtrack Platinum FX",
            Self::Generic => "Generic MIDI / Learn",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::RolandDj202 => {
                "Four banks, eight hotcue/loop pads, mixer, filter/FX and LEDs. Existing mappings are preserved."
            }
            Self::RolandDj505 => {
                "Four banks, mixer and mode-dependent hotcue/loop pads. Select HOT CUE or LOOP first. TR-S and hardware effects are outside this profile."
            }
            Self::PioneerDdj400 | Self::PioneerDdjFlx4 => {
                "USB MIDI, two decks, 14-bit mixer/tempo, hotcues and beat-loop pads. Shared Beat FX, proprietary features and HID are not mapped."
            }
            Self::NumarkMixtrackPro3 => {
                "Two decks, mixer, tempo, four hotcues and four loop pads. This model has no hardware trim knob. SHIFT+hotcue clears it."
            }
            Self::NumarkMixtrackPlatinumFx => {
                "Four native banks, mixer, hotcue/loop modes and loop controls. Jog displays, fader cuts and effect paddles are not implemented."
            }
            Self::Generic => {
                "No assumed manufacturer mapping. Use MIDI Learn or import a RustDJ JSON profile. Audio routing is configured separately."
            }
        }
    }
    pub fn detect(port: &str) -> Option<Self> {
        let name: String = port
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .flat_map(char::to_lowercase)
            .collect();
        let models = [
            (Self::RolandDj202, "dj202"),
            (Self::RolandDj505, "dj505"),
            (Self::PioneerDdj400, "ddj400"),
            (Self::PioneerDdjFlx4, "ddjflx4"),
            (Self::NumarkMixtrackPro3, "mixtrackpro3"),
            (Self::NumarkMixtrackPro3, "mixtrack3"),
            (Self::NumarkMixtrackPlatinumFx, "mixtrackplatinumfx"),
        ];
        models.into_iter().find_map(|(profile, model)| {
            name.match_indices(model)
                .any(|(start, _)| {
                    !name
                        .as_bytes()
                        .get(start + model.len())
                        .is_some_and(u8::is_ascii_digit)
                })
                .then_some(profile)
        })
    }
    pub fn banked(self) -> bool {
        matches!(
            self,
            Self::RolandDj202 | Self::RolandDj505 | Self::NumarkMixtrackPlatinumFx
        )
    }
    pub fn matches(self, port: &str) -> bool {
        Self::detect(port) == Some(self)
    }
    pub fn defaults(self) -> Vec<Binding> {
        match self {
            Self::RolandDj202 => crate::midi::dj202_profile(),
            Self::RolandDj505 | Self::NumarkMixtrackPlatinumFx => banked(self),
            Self::PioneerDdj400 | Self::PioneerDdjFlx4 => pioneer(self),
            Self::NumarkMixtrackPro3 => mixtrack3(),
            Self::Generic => Vec::new(),
        }
    }
    pub fn initialization(self) -> &'static [&'static [u8]] {
        match self {
            Self::RolandDj202 => &[
                &[0xf0, 0, 0x20, 0x7f, 1, 0xf7],
                &[0xf0, 0, 0x20, 0x7f, 0, 0xf7],
            ],
            Self::RolandDj505 => &[
                &[0xf0, 0, 0x20, 0x7f, 0, 0xf7],
                &[0xf0, 0, 0x20, 0x7f, 1, 0xf7],
            ],
            Self::NumarkMixtrackPlatinumFx => &[&[0xf0, 0x7e, 0, 6, 1, 0xf7]],
            _ => &[],
        }
    }
}

fn add(out: &mut Vec<Binding>, status: u8, number: u8, mode: M, action: A) {
    out.push(Binding {
        status,
        number,
        mode,
        action,
    });
}
fn note(out: &mut Vec<Binding>, status: u8, number: u8, action: A) {
    add(
        out,
        status,
        number,
        if matches!(action, A::Cue(_)) {
            M::Gate
        } else {
            M::Button
        },
        action,
    );
}
fn cc14(out: &mut Vec<Binding>, status: u8, number: u8, lsb: u8, action: A) {
    add(out, status, number, M::Absolute14 { lsb }, action);
}

fn pioneer(model: ProfileId) -> Vec<Binding> {
    let mut out = Vec::new();
    for d in 0..2 {
        let n = 0x90 + d as u8;
        let cc = 0xb0 + d as u8;
        for (number, action) in [
            (0x0b, A::Play(d)),
            (0x0c, A::Cue(d)),
            (0x58, A::Sync(d)),
            (0x54, A::Pfl(d)),
            (0x48, A::SetCue(d)),
            (0x10, A::LoopIn(d)),
            (0x11, A::LoopOut(d)),
            (
                0x4d,
                if model == ProfileId::PioneerDdj400 {
                    A::LoopToggle(d)
                } else {
                    A::LoopPreset(d, 4)
                },
            ),
            (0x50, A::LoopExit(d)),
            (0x51, A::LoopHalf(d)),
            (0x53, A::LoopDouble(d)),
        ] {
            note(&mut out, n, number, action);
        }
        if model == ProfileId::PioneerDdj400 {
            note(&mut out, n, 0x14, A::LoopPreset(d, 4));
        }
        for (number, action) in [
            (0, A::Tempo(d)),
            (4, A::Gain(d)),
            (7, A::EqHigh(d)),
            (11, A::EqMid(d)),
            (15, A::EqLow(d)),
            (19, A::Volume(d)),
        ] {
            cc14(&mut out, cc, number, number + 32, action);
        }
        for jog in [0x21, 0x22, 0x23, 0x29] {
            add(&mut out, cc, jog, M::Relative64, A::Jog(d));
        }
        cc14(&mut out, 0xb6, 0x17 + d as u8, 0x37 + d as u8, A::Filter(d));
        note(&mut out, 0x96, 0x46 + d as u8, A::Load(d));
        let pad = 0x97 + d as u8 * 2;
        for slot in 0..8 {
            note(&mut out, pad, slot as u8, A::Hotcue(d, slot));
            note(&mut out, pad + 1, slot as u8, A::ClearHotcue(d, slot));
            note(&mut out, pad, 0x60 + slot as u8, A::LoopPad(d, slot));
        }
    }
    cc14(&mut out, 0xb6, 0x1f, 0x3f, A::Crossfader);
    cc14(&mut out, 0xb6, 0x0c, 0x2c, A::PhonesMix);
    cc14(&mut out, 0xb6, 0x0d, 0x2d, A::PhonesLevel);
    cc14(&mut out, 0xb6, 0x08, 0x28, A::Master);
    add(&mut out, 0xb6, 0x40, M::RelativeTwos, A::Browse);
    note(&mut out, 0x96, 0x41, A::Open);
    out
}

fn mixtrack3() -> Vec<Binding> {
    let mut out = Vec::new();
    for d in 0..2 {
        let n = 0x91 + d as u8;
        let cc = 0xb1 + d as u8;
        for (number, action) in [
            (1, A::Play(d)),
            (3, A::Cue(d)),
            (2, A::Sync(d)),
            (0x13, A::LoopIn(d)),
            (0x14, A::LoopOut(d)),
            (0x15, A::LoopToggle(d)),
            (0x16, A::LoopHalf(d)),
        ] {
            note(&mut out, n, number, action);
        }
        for (number, action) in [
            (1, A::EqHigh(d)),
            (2, A::EqMid(d)),
            (3, A::EqLow(d)),
            (4, A::Filter(d)),
            (5, A::Volume(d)),
        ] {
            add(&mut out, cc, number, M::Absolute7, action);
        }
        add(
            &mut out,
            cc,
            0x18,
            M::Absolute14Inverted { lsb: 0x38 },
            A::Tempo(d),
        );
        add(&mut out, cc, 0x11, M::RelativeTwos, A::Jog(d));
        note(&mut out, 0x90, 0x10 + d as u8, A::Load(d));
        note(&mut out, 0x90, 0x0e + d as u8, A::Pfl(d));
        for slot in 0..4 {
            note(&mut out, n, 0x1b + slot as u8, A::Hotcue(d, slot));
            // The four physical loop pads select the central 1/2/4/8 lengths.
            note(&mut out, n, 0x17 + slot as u8, A::LoopPad(d, slot + 2));
        }
        for slot in 0..3 {
            note(&mut out, n, 7 + slot as u8, A::FxToggle(d, slot));
        }
    }
    for (number, action) in [
        (7, A::Crossfader),
        (8, A::PhonesLevel),
        (9, A::PhonesMix),
        (10, A::Master),
    ] {
        add(&mut out, 0xb0, number, M::Absolute7, action);
    }
    add(&mut out, 0xb0, 0x17, M::RelativeTwos, A::Browse);
    note(&mut out, 0x90, 0x12, A::Open);
    out
}

fn banked(model: ProfileId) -> Vec<Binding> {
    let mut out = Vec::new();
    for d in 0..4 {
        let n = 0x90 + d as u8;
        let cc = 0xb0 + d as u8;
        let pad = 0x94 + d as u8;
        for (number, action) in [
            (0, A::Play(d)),
            (1, A::Cue(d)),
            (2, A::Sync(d)),
            (0x1b, A::Pfl(d)),
        ] {
            note(&mut out, n, number, action);
        }
        for (number, lsb, action) in [
            (0x16, 0x34, A::Gain(d)),
            (0x17, 0x35, A::EqHigh(d)),
            (0x18, 0x36, A::EqMid(d)),
            (0x19, 0x37, A::EqLow(d)),
            (0x1c, 0x3a, A::Volume(d)),
        ] {
            if model == ProfileId::RolandDj505 {
                cc14(&mut out, cc, number, lsb, action);
            } else {
                add(&mut out, cc, number, M::Absolute7, action);
            }
        }
        add(
            &mut out,
            cc,
            0x09,
            if model == ProfileId::RolandDj505 {
                M::Absolute14 { lsb: 0x3b }
            } else {
                M::Absolute14Inverted { lsb: 0x29 }
            },
            A::Tempo(d),
        );
        add(&mut out, cc, 0x1a, M::Absolute7, A::Filter(d));
        add(
            &mut out,
            cc,
            6,
            if model == ProfileId::NumarkMixtrackPlatinumFx {
                M::RelativeTwos
            } else {
                M::Relative64
            },
            A::Jog(d),
        );
        note(&mut out, 0x9f, 2 + d as u8, A::Load(d));
        for slot in 0..8 {
            note(&mut out, pad, 0x14 + slot as u8, A::Hotcue(d, slot));
            note(&mut out, pad, 0x1c + slot as u8, A::ClearHotcue(d, slot));
        }
        for (number, action) in [
            (0x34, A::LoopHalf(d)),
            (0x35, A::LoopDouble(d)),
            (0x38, A::LoopIn(d)),
            (0x39, A::LoopOut(d)),
            (0x40, A::LoopPreset(d, 4)),
            (0x33, A::LoopToggle(d)),
        ] {
            if model == ProfileId::NumarkMixtrackPlatinumFx && matches!(number, 0x38 | 0x39 | 0x33)
            {
                continue;
            }
            note(&mut out, pad, number, action);
        }
    }
    for d in 0..2 {
        note(&mut out, 0x90 + d as u8, 8, A::DeckLayer(d));
    }
    if model == ProfileId::RolandDj505 {
        for side in 0..2 {
            for slot in 0..3 {
                note(
                    &mut out,
                    0x98 + side as u8,
                    slot as u8,
                    A::FxToggle(side, slot),
                );
            }
            for number in [0, 3] {
                add(
                    &mut out,
                    0xb8 + side as u8,
                    number,
                    M::Absolute7,
                    A::FxLevel(side),
                );
            }
        }
    }
    if model == ProfileId::RolandDj505 {
        cc14(&mut out, 0xbf, 8, 0x58, A::Crossfader);
    } else {
        add(&mut out, 0xbf, 8, M::Absolute7, A::Crossfader);
    }
    add(&mut out, 0xbf, 0, M::RelativeTwos, A::Browse);
    note(&mut out, 0x9f, 7, A::Open);
    if model == ProfileId::NumarkMixtrackPlatinumFx {
        add(&mut out, 0xbf, 0x0c, M::Absolute7, A::PhonesLevel);
        add(&mut out, 0xbf, 0x0d, M::Absolute7, A::PhonesMix);
    }
    out
}

pub(crate) fn basic_leds(
    profile: ProfileId,
    decks: [crate::midi::DeckLeds; 4],
    synced: [bool; 4],
) -> Vec<[u8; 3]> {
    let mut messages = Vec::new();
    let count = if profile.banked() { 4 } else { 2 };
    if profile == ProfileId::Generic {
        return messages;
    }
    for d in 0..count {
        let (status, play, sync, pfl_status, pfl) = match profile {
            ProfileId::PioneerDdj400 | ProfileId::PioneerDdjFlx4 => {
                (0x90 + d as u8, 0x0b, 0x58, 0x90 + d as u8, 0x54)
            }
            ProfileId::NumarkMixtrackPro3 => (0x91 + d as u8, 1, 2, 0x90, 0x0e + d as u8),
            _ => (0x90 + d as u8, 0, 2, 0x90 + d as u8, 0x1b),
        };
        for (st, n, on) in [
            (status, play, decks[d].playing),
            (status, sync, synced[d]),
            (pfl_status, pfl, decks[d].pfl),
        ] {
            messages.push([st, n, if on { 127 } else { 0 }]);
        }
    }
    messages
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileFile {
    pub schema_version: u32,
    pub profile: ProfileId,
    pub bindings: Vec<Binding>,
}

impl ProfileFile {
    pub fn read(path: &std::path::Path) -> Result<Self, String> {
        let file =
            std::fs::File::open(path).map_err(|e| format!("Could not read MIDI profile: {e}"))?;
        let mut limited = std::io::Read::take(file, 1_048_577);
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut limited, &mut data).map_err(|e| e.to_string())?;
        if data.len() > 1_048_576 {
            return Err("MIDI profile exceeds 1 MB.".into());
        }
        let file: Self = serde_json::from_slice(&data)
            .map_err(|e| format!("Invalid RustDJ MIDI profile: {e}"))?;
        file.validate()?;
        Ok(file)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.bindings.len() > 4096 {
            return Err("Unsupported MIDI profile version or too many bindings.".into());
        }
        let mut addresses = std::collections::BTreeSet::new();
        for binding in &self.bindings {
            let note = binding.status & 0xf0 == 0x90;
            let cc = binding.status & 0xf0 == 0xb0;
            if binding.number > 127
                || !(note || cc)
                || (note && !matches!(binding.mode, M::Button | M::Gate))
                || binding
                    .mode
                    .lsb()
                    .is_some_and(|v| v > 127 || v == binding.number)
            {
                return Err("Invalid MIDI address or input mode.".into());
            }
            for number in std::iter::once(binding.number).chain(binding.mode.lsb()) {
                if !addresses.insert((binding.status, number)) {
                    return Err("MIDI profile contains overlapping addresses.".into());
                }
            }
            let mut valid = true;
            binding.action.map_deck(|d| {
                valid = d < 4;
                d
            });
            valid &= match binding.action {
                A::DeckLayer(s) => s < 2,
                A::FxToggle(_, s) => s < 3,
                A::Hotcue(_, s) | A::ClearHotcue(_, s) | A::LoopPad(_, s) => s < 8,
                A::LoopPreset(_, b) => (1..=32).contains(&b),
                _ => true,
            };
            if !valid {
                return Err("MIDI profile contains an out-of-range deck, pad or effect.".into());
            }
        }
        Ok(())
    }
    pub fn write(&self, path: &std::path::Path) -> Result<(), String> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| format!("Could not save MIDI profile: {e}"))
    }
}
