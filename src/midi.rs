//! MIDI input and a small DJ-202 profile. No device-specific code runs in the callback.
use crate::controller_profiles::ProfileId;
use eframe::egui;
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

/// The DJ-202 starts with split pad layers. Unlock all eight pads before
/// requesting its current positions/mode (see the controller author's mapping).
fn initialize_dj202(mut send: impl FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
    send(&[0xf0, 0x00, 0x20, 0x7f, 0x01, 0xf7])
        .map_err(|error| format!("Could not unlock DJ-202 pad layers: {error}"))?;
    send(&[0xf0, 0x00, 0x20, 0x7f, 0x00, 0xf7])
        .map_err(|error| format!("Could not request controller status: {error}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadMode {
    Hotcue,
    Loop,
    Other(u8),
}

impl PadMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hotcue => "HOT CUE",
            Self::Loop => "LOOP",
            Self::Other(0x01) => "CUE LOOP (select HOT CUE)",
            Self::Other(0x02) => "PITCH PLAY (select HOT CUE)",
            Self::Other(0x03) => "SPLIT HOT CUE (select HOT CUE)",
            Self::Other(0x11 | 0x13) => "ROLL (select LOOP)",
            Self::Other(0x12) => "MANUAL LOOP (select LOOP)",
            Self::Other(0x20..=0x22) => "SEQUENCER (select HOT CUE/LOOP)",
            Self::Other(0x30..=0x33) => "SAMPLER/SLICER (select HOT CUE/LOOP)",
            Self::Other(_) => "Other mode (select HOT CUE/LOOP)",
        }
    }
}

fn pad_mode_message([status, number, value]: [u8; 3]) -> Option<(usize, PadMode)> {
    if !(0x94..=0x97).contains(&status) || number != 0 || value > 127 {
        return None;
    }
    // Velocity is a mode ID here. In particular 0 means HOT CUE, not release.
    let mode = match value {
        0x00 => PadMode::Hotcue,
        0x10 => PadMode::Loop,
        other => PadMode::Other(other),
    };
    Some(((status - 0x94) as usize, mode))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    SelectDeck(usize),
    DeckLayer(usize),
    Play(usize),
    Cue(usize),
    SetCue(usize),
    Stop(usize),
    Sync(usize),
    Volume(usize),
    Tempo(usize),
    Jog(usize),
    Load(usize),
    Pfl(usize),
    EqLow(usize),
    EqMid(usize),
    EqHigh(usize),
    Gain(usize),
    Filter(usize),
    FxLevel(usize),
    FxToggle(usize, usize),
    CrossfaderToggle,
    Hotcue(usize, usize),
    ClearHotcue(usize, usize),
    LoopPreset(usize, u8),
    LoopPad(usize, usize),
    LoopToggle(usize),
    LoopIn(usize),
    LoopOut(usize),
    LoopExit(usize),
    Reloop(usize),
    LoopHalf(usize),
    LoopDouble(usize),
    Crossfader,
    Master,
    PhonesLevel,
    PhonesMix,
    Browse,
    Open,
}

impl Action {
    pub(crate) fn map_deck(self, map: impl FnOnce(usize) -> usize) -> Self {
        match self {
            Self::Play(d) => Self::Play(map(d)),
            Self::Cue(d) => Self::Cue(map(d)),
            Self::SetCue(d) => Self::SetCue(map(d)),
            Self::Stop(d) => Self::Stop(map(d)),
            Self::Sync(d) => Self::Sync(map(d)),
            Self::Volume(d) => Self::Volume(map(d)),
            Self::Tempo(d) => Self::Tempo(map(d)),
            Self::Jog(d) => Self::Jog(map(d)),
            Self::Load(d) => Self::Load(map(d)),
            Self::Pfl(d) => Self::Pfl(map(d)),
            Self::EqLow(d) => Self::EqLow(map(d)),
            Self::EqMid(d) => Self::EqMid(map(d)),
            Self::EqHigh(d) => Self::EqHigh(map(d)),
            Self::Gain(d) => Self::Gain(map(d)),
            Self::Filter(d) => Self::Filter(map(d)),
            Self::FxLevel(d) => Self::FxLevel(map(d)),
            Self::FxToggle(d, slot) => Self::FxToggle(map(d), slot),
            Self::Hotcue(d, slot) => Self::Hotcue(map(d), slot),
            Self::ClearHotcue(d, slot) => Self::ClearHotcue(map(d), slot),
            Self::LoopPad(d, slot) => Self::LoopPad(map(d), slot),
            Self::LoopPreset(d, beats) => Self::LoopPreset(map(d), beats),
            Self::LoopToggle(d) => Self::LoopToggle(map(d)),
            Self::LoopIn(d) => Self::LoopIn(map(d)),
            Self::LoopOut(d) => Self::LoopOut(map(d)),
            Self::LoopExit(d) => Self::LoopExit(map(d)),
            Self::Reloop(d) => Self::Reloop(map(d)),
            Self::LoopHalf(d) => Self::LoopHalf(map(d)),
            Self::LoopDouble(d) => Self::LoopDouble(map(d)),
            other => other,
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::DeckLayer(side) => {
                return if side == 0 {
                    "Controller: switch Deck 1/3".into()
                } else {
                    "Controller: switch Deck 2/4".into()
                };
            }
            Self::SelectDeck(d) => return format!("Select Deck {}", d + 1),
            Self::FxToggle(d, slot) => {
                return format!("{}: FX {}", d + 1, slot.saturating_add(1));
            }
            Self::LoopPad(d, slot) => {
                return format!(
                    "{}: Loop {} Beats",
                    d + 1,
                    crate::loops::PRESETS.get(slot).map_or("?", |p| p.1)
                );
            }
            Self::Hotcue(d, slot) => {
                return format!("{}: Hotcue {}", d + 1, slot + 1);
            }
            Self::ClearHotcue(d, slot) => {
                return format!("{}: Clear hotcue {}", d + 1, slot + 1);
            }
            Self::LoopPreset(d, beats) => {
                return format!("{}: {beats}-Beat-Loop", d + 1);
            }
            _ => {}
        }
        let (name, deck) = match self {
            Self::SelectDeck(_) | Self::DeckLayer(_) => unreachable!(),
            Self::Play(d) => ("Play/Pause", Some(d)),
            Self::Cue(d) => ("Cue", Some(d)),
            Self::SetCue(d) => ("Set cue", Some(d)),
            Self::Stop(d) => ("Stop", Some(d)),
            Self::Sync(d) => ("Sync", Some(d)),
            Self::Volume(d) => ("Level", Some(d)),
            Self::Tempo(d) => ("Tempo", Some(d)),
            Self::Jog(d) => ("Jog / Seek", Some(d)),
            Self::Load(d) => ("Load track", Some(d)),
            Self::Pfl(d) => ("Headphone CUE", Some(d)),
            Self::EqLow(d) => ("Low", Some(d)),
            Self::EqMid(d) => ("Mid", Some(d)),
            Self::EqHigh(d) => ("High", Some(d)),
            Self::Gain(d) => ("Gain / Trim", Some(d)),
            Self::Filter(d) => ("Filter LP / HP", Some(d)),
            Self::FxLevel(d) => ("FX Level", Some(d)),
            Self::FxToggle(..) => unreachable!(),
            Self::CrossfaderToggle => ("Toggle crossfader", None),
            Self::LoopToggle(d) => ("Toggle loop", Some(d)),
            Self::LoopIn(d) => ("Loop IN", Some(d)),
            Self::LoopOut(d) => ("Loop OUT", Some(d)),
            Self::LoopExit(d) => ("Exit loop", Some(d)),
            Self::Reloop(d) => ("Reloop", Some(d)),
            Self::LoopHalf(d) => ("Halve loop", Some(d)),
            Self::LoopDouble(d) => ("Double loop", Some(d)),
            Self::Hotcue(..) | Self::ClearHotcue(..) | Self::LoopPreset(..) | Self::LoopPad(..) => {
                unreachable!()
            }
            Self::Crossfader => ("Crossfader", None),
            Self::Master => ("Master Level", None),
            Self::PhonesLevel => ("Phones Level", None),
            Self::PhonesMix => ("Mixing: Cue / Master", None),
            Self::Browse => ("Browse selection", None),
            Self::Open => ("Browse open/load", None),
        };
        match deck {
            Some(d) => format!("{}: {name}", d + 1),
            None => name.into(),
        }
    }

    pub fn all() -> Vec<Self> {
        let mut actions = vec![
            Self::DeckLayer(0),
            Self::DeckLayer(1),
            Self::Crossfader,
            Self::CrossfaderToggle,
            Self::Master,
            Self::PhonesLevel,
            Self::PhonesMix,
            Self::Browse,
            Self::Open,
        ];
        for d in 0..crate::decks::COUNT {
            actions.extend([
                Self::SelectDeck(d),
                Self::Play(d),
                Self::Cue(d),
                Self::SetCue(d),
                Self::Stop(d),
                Self::Sync(d),
                Self::Volume(d),
                Self::Tempo(d),
                Self::Jog(d),
                Self::Load(d),
                Self::Pfl(d),
                Self::EqLow(d),
                Self::EqMid(d),
                Self::EqHigh(d),
                Self::Gain(d),
                Self::Filter(d),
                Self::FxLevel(d),
                Self::LoopToggle(d),
                Self::LoopIn(d),
                Self::LoopOut(d),
                Self::LoopExit(d),
                Self::Reloop(d),
                Self::LoopHalf(d),
                Self::LoopDouble(d),
            ]);
            for slot in 0..8 {
                actions.push(Self::LoopPad(d, slot));
                actions.extend([Self::Hotcue(d, slot), Self::ClearHotcue(d, slot)]);
            }
            for beats in [1, 2, 4, 8] {
                actions.push(Self::LoopPreset(d, beats));
            }
            for slot in 0..3 {
                actions.push(Self::FxToggle(d, slot));
            }
        }
        actions
    }

    fn continuous(self) -> bool {
        matches!(
            self,
            Self::Volume(_)
                | Self::Tempo(_)
                | Self::Crossfader
                | Self::Master
                | Self::PhonesLevel
                | Self::PhonesMix
                | Self::EqLow(_)
                | Self::EqMid(_)
                | Self::EqHigh(_)
                | Self::Gain(_)
                | Self::Filter(_)
                | Self::FxLevel(_)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Button,
    Gate,
    Absolute7,
    Absolute7Inverted,
    Absolute14 { lsb: u8 },
    Absolute14Inverted { lsb: u8 },
    Relative64,
    RelativeTwos,
}

impl Mode {
    pub fn lsb(self) -> Option<u8> {
        match self {
            Self::Absolute14 { lsb } | Self::Absolute14Inverted { lsb } => Some(lsb),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub status: u8,
    pub number: u8,
    pub mode: Mode,
    pub action: Action,
}

pub fn dj202_profile() -> Vec<Binding> {
    let mut bindings = Vec::new();
    for d in 0..crate::decks::COUNT {
        bindings.push(Binding {
            status: 0xb0 + d as u8,
            number: 0x1a,
            mode: Mode::Absolute7,
            action: Action::Filter(d),
        });
        if d < 2 {
            for number in [0x00, 0x03] {
                bindings.push(Binding {
                    status: 0xb8 + d as u8,
                    number,
                    mode: Mode::Absolute7,
                    action: Action::FxLevel(d),
                });
            }
            for slot in 0..3 {
                bindings.push(Binding {
                    status: 0x98 + d as u8,
                    number: slot as u8,
                    mode: Mode::Button,
                    action: Action::FxToggle(d, slot),
                });
            }
        }
        let status = 0x94 + d as u8;
        for slot in 0..8 {
            bindings.push(Binding {
                status,
                number: 0x11 + slot as u8,
                mode: Mode::Button,
                action: Action::LoopPad(d, slot),
            });
            bindings.push(Binding {
                status,
                number: 1 + slot as u8,
                mode: Mode::Button,
                action: Action::Hotcue(d, slot),
            });
            bindings.push(Binding {
                status,
                number: 9 + slot as u8,
                mode: Mode::Button,
                action: Action::ClearHotcue(d, slot),
            });
        }
        for (number, action) in [
            (0x41, Action::LoopHalf(d)),
            (0x42, Action::LoopDouble(d)),
            (0x49, Action::LoopHalf(d)),
            (0x4a, Action::LoopDouble(d)),
        ] {
            bindings.push(Binding {
                status,
                number,
                mode: Mode::Button,
                action,
            });
        }
        for (number, action) in [
            (0x00, Action::Play(d)),
            (0x01, Action::Cue(d)),
            (0x02, Action::Sync(d)),
            (0x04, Action::Stop(d)),
            (0x05, Action::SetCue(d)),
        ] {
            bindings.push(Binding {
                status: 0x90 + d as u8,
                number,
                mode: if matches!(action, Action::Cue(_)) {
                    Mode::Gate
                } else {
                    Mode::Button
                },
                action,
            });
        }
        bindings.push(Binding {
            status: 0x9f,
            number: 0x02 + d as u8,
            mode: Mode::Button,
            action: Action::Load(d),
        });
        bindings.push(Binding {
            status: 0xb0 + d as u8,
            number: 0x1c,
            mode: Mode::Absolute14 { lsb: 0x3a },
            action: Action::Volume(d),
        });
        bindings.push(Binding {
            status: 0xb0 + d as u8,
            number: 0x09,
            mode: Mode::Absolute14 { lsb: 0x3b },
            action: Action::Tempo(d),
        });
        bindings.push(Binding {
            status: 0xb0 + d as u8,
            number: 0x06,
            mode: Mode::Relative64,
            action: Action::Jog(d),
        });
        bindings.push(Binding {
            status: 0x90 + d as u8,
            number: 0x1b,
            mode: Mode::Button,
            action: Action::Pfl(d),
        });
        bindings.push(Binding {
            status: 0xb0 + d as u8,
            number: 0x16,
            mode: Mode::Absolute14 { lsb: 0x34 },
            action: Action::Gain(d),
        });
        for (number, lsb, action) in [
            (0x19, 0x37, Action::EqLow(d)),
            (0x18, 0x36, Action::EqMid(d)),
            (0x17, 0x35, Action::EqHigh(d)),
        ] {
            bindings.push(Binding {
                status: 0xb0 + d as u8,
                number,
                mode: Mode::Absolute14 { lsb },
                action,
            });
        }
    }
    for side in 0..2 {
        bindings.push(Binding {
            status: 0x90 + side as u8,
            number: 0x08,
            mode: Mode::Button,
            action: Action::DeckLayer(side),
        });
    }
    bindings.push(Binding {
        status: 0xbf,
        number: 0x08,
        mode: Mode::Absolute14 { lsb: 0x58 },
        action: Action::Crossfader,
    });
    bindings.push(Binding {
        status: 0xbf,
        number: 0x00,
        mode: Mode::RelativeTwos,
        action: Action::Browse,
    });
    bindings.push(Binding {
        status: 0x9f,
        number: 0x06,
        mode: Mode::Button,
        action: Action::Open,
    });
    bindings.push(Binding {
        status: 0xbf,
        number: 0x0c,
        mode: Mode::Absolute7,
        action: Action::PhonesLevel,
    });
    bindings.push(Binding {
        status: 0xbf,
        number: 0x0d,
        mode: Mode::Absolute7,
        action: Action::PhonesMix,
    });
    bindings
}

/// Extend old profiles without overwriting custom controller assignments.
pub fn upgrade(mut bindings: Vec<Binding>) -> Vec<Binding> {
    if bindings.is_empty() {
        return bindings;
    }
    for binding in &mut bindings {
        if (0x94..=0x97).contains(&binding.status)
            && (0x11..=0x18).contains(&binding.number)
            && binding.mode == Mode::Button
        {
            let d = (binding.status - 0x94) as usize;
            let slot = (binding.number - 0x11) as usize;
            let legacy = [
                Action::LoopPreset(d, 1),
                Action::LoopPreset(d, 2),
                Action::LoopPreset(d, 4),
                Action::LoopPreset(d, 8),
                Action::LoopIn(d),
                Action::LoopOut(d),
                Action::LoopExit(d),
                Action::Reloop(d),
            ];
            if binding.action == legacy[slot] {
                binding.action = Action::LoopPad(d, slot);
            }
        }
        if matches!(binding.action, Action::Cue(_)) {
            binding.mode = Mode::Gate;
        }
    }
    let original: Vec<_> = bindings.iter().map(|b| b.action).collect();
    for binding in dj202_profile() {
        if !original.contains(&binding.action)
            && !bindings.iter().any(|b| {
                b.status == binding.status
                    && (b.number == binding.number || b.mode.lsb() == Some(binding.number))
            })
        {
            bindings.push(binding);
        }
    }
    bindings
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct DeckLeds {
    pub effects: [bool; 3],
    pub playing: bool,
    pub pfl: bool,
    pub hotcues: [bool; 8],
    pub loops: [bool; 8],
}

fn all_led_messages<const N: usize>(
    decks: [DeckLeds; N],
    synced: [bool; N],
    layers: [usize; 2],
) -> Vec<[u8; 3]> {
    let mut messages = Vec::new();
    for (d, deck) in decks.into_iter().enumerate() {
        for (note, on) in [(0, deck.playing), (2, synced[d]), (0x1b, deck.pfl)] {
            messages.push([0x90 + d as u8, note, if on { 127 } else { 0 }]);
        }
        for slot in 0..8 {
            messages.push([
                0x94 + d as u8,
                slot as u8 + 1,
                if deck.hotcues[slot] { 127 } else { 0 },
            ]);
            messages.push([
                0x94 + d as u8,
                slot as u8 + 0x11,
                if deck.loops[slot] { 127 } else { 0 },
            ]);
        }
    }
    for (side, selected) in layers.into_iter().enumerate() {
        if let Some(deck) = decks.get(selected) {
            for slot in 0..3 {
                messages.push([
                    0x98 + side as u8,
                    slot as u8,
                    if deck.effects[slot] { 127 } else { 0 },
                ]);
            }
        }
        messages.push([0x90 + side as u8, 0x08, if selected >= 2 { 127 } else { 0 }]);
    }
    messages
}

#[cfg(test)]
fn led_messages<const N: usize>(decks: [DeckLeds; N], synced: Option<usize>) -> Vec<[u8; 3]> {
    all_led_messages(decks, std::array::from_fn(|i| synced == Some(i)), [0, 1])
}

pub struct Mapper {
    profile: ProfileId,
    msb: [[u8; 128]; 16],
    layers: [usize; 2],
    cue_holds: Vec<(u8, u8, usize)>,
}

impl Default for Mapper {
    fn default() -> Self {
        Self {
            profile: ProfileId::RolandDj202,
            msb: [[0; 128]; 16],
            layers: [0, 1],
            cue_holds: Vec::with_capacity(4),
        }
    }
}

impl Mapper {
    fn for_profile(profile: ProfileId) -> Self {
        Self {
            profile,
            ..Self::default()
        }
    }
    fn observe_deck(&mut self, [status, number, value]: [u8; 3]) {
        if !self.profile.banked() {
            return;
        }
        // Native bank channels identify the deck, including when returning to
        // 1/2. Never reinterpret them through the last software layer. Releases
        // must not switch banks (Cue may still belong to the previous deck).
        let deck = match status {
            0x90..=0x93 if value > 0 && number != 0x08 => status - 0x90,
            0x94..=0x97 if number == 0 || value > 0 => status - 0x94,
            0xb0..=0xb3 => status - 0xb0,
            _ => return,
        } as usize;
        self.layers[deck % 2] = deck;
    }

    pub fn decode(&mut self, message: [u8; 3], bindings: &[Binding]) -> Vec<(Action, f32)> {
        let [status, number, value] = message;
        if number > 127 || value > 127 {
            return Vec::new();
        }
        self.observe_deck(message);
        let channel = (status & 0x0f) as usize;
        if status & 0xf0 == 0xb0 {
            self.msb[channel][number as usize] = value;
        }
        let decoded: Vec<_> = bindings
            .iter()
            .filter_map(|binding| {
                if binding.number > 127 {
                    return None;
                }
                let note_off = status & 0xf0 == 0x80 && binding.status == (status | 0x10);
                if binding.status != status && !(binding.mode == Mode::Gate && note_off) {
                    return None;
                }
                let output = match binding.mode {
                    Mode::Gate
                        if number == binding.number && (status & 0xf0 == 0x90 || note_off) =>
                    {
                        if note_off || value == 0 {
                            0.0
                        } else {
                            1.0
                        }
                    }
                    Mode::Gate if number == binding.number && status & 0xf0 == 0xb0 => {
                        if value >= 64 { 1.0 } else { 0.0 }
                    }
                    Mode::Button
                        if number == binding.number && value > 0 && status & 0xf0 == 0x90 =>
                    {
                        1.0
                    }
                    Mode::Button
                        if number == binding.number && value >= 64 && status & 0xf0 == 0xb0 =>
                    {
                        1.0
                    }
                    Mode::Absolute7 | Mode::Absolute7Inverted
                        if number == binding.number && status & 0xf0 == 0xb0 =>
                    {
                        let value = value as f32 / 127.0;
                        if binding.mode == Mode::Absolute7Inverted {
                            1.0 - value
                        } else {
                            value
                        }
                    }
                    Mode::Absolute14 { lsb } | Mode::Absolute14Inverted { lsb }
                        if number == lsb && status & 0xf0 == 0xb0 =>
                    {
                        let raw = ((self.msb[channel][binding.number as usize] as u16) << 7)
                            | value as u16;
                        let max = if binding.action == Action::Crossfader
                            && self.profile == ProfileId::RolandDj202
                        {
                            16368.0
                        } else {
                            16383.0
                        };
                        let value = (raw as f32 / max).clamp(0.0, 1.0);
                        if matches!(binding.mode, Mode::Absolute14Inverted { .. }) {
                            1.0 - value
                        } else {
                            value
                        }
                    }
                    Mode::Relative64 if number == binding.number && status & 0xf0 == 0xb0 => {
                        (value as i16 - 64) as f32
                    }
                    Mode::RelativeTwos if number == binding.number && status & 0xf0 == 0xb0 => {
                        if value < 64 {
                            value as f32
                        } else {
                            (value as i16 - 128) as f32
                        }
                    }
                    _ => return None,
                };
                Some((binding.action, output))
            })
            .collect();
        decoded
            .into_iter()
            .map(|(action, value)| {
                if let Action::DeckLayer(side) = action
                    && side < 2
                {
                    self.layers[side] ^= 2;
                    return (Action::SelectDeck(self.layers[side]), value);
                }
                // Only shared hardware controls follow the selected bank. Native
                // transport/mixer/pad channels and learned deck targets stay exact.
                let shared = self.profile.banked()
                    && match action {
                        Action::FxToggle(side, slot) if side < 2 && slot < 3 => {
                            status == 0x98 + side as u8 && number == slot as u8
                        }
                        Action::FxLevel(side) if side < 2 => {
                            status == 0xb8 + side as u8 && matches!(number, 0 | 3)
                        }
                        Action::Load(side) if side < 2 => {
                            status == 0x9f && number == 2 + side as u8
                        }
                        _ => false,
                    };
                let action = if shared {
                    action.map_deck(|side| self.layers[side])
                } else {
                    action
                };
                let action = if let Action::Cue(deck) = action {
                    // The same physical Cue key can release on the new native deck
                    // channel after switching 1/3 or 2/4 while it is held.
                    let hold_channel = if self.profile.banked() && channel < 4 {
                        channel % 2
                    } else {
                        channel
                    } as u8;
                    if let Some(index) = self
                        .cue_holds
                        .iter()
                        .position(|hold| hold.0 == hold_channel && hold.1 == number)
                    {
                        let held = self.cue_holds[index].2;
                        if value == 0.0 {
                            self.cue_holds.remove(index);
                        }
                        Action::Cue(held)
                    } else {
                        if value > 0.0 {
                            self.cue_holds.push((hold_channel, number, deck));
                        }
                        Action::Cue(deck)
                    }
                } else {
                    action
                };
                (action, value)
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiPrefs {
    pub auto_connect: bool,
    pub preferred_port: Option<String>,
    pub profile: Option<ProfileId>,
    pub mappings: BTreeMap<ProfileId, Vec<Binding>>,
    pub led_feedback: bool,
}
impl Default for MidiPrefs {
    fn default() -> Self {
        Self {
            auto_connect: true,
            preferred_port: None,
            profile: None,
            mappings: BTreeMap::new(),
            led_feedback: true,
        }
    }
}

pub fn auto_port(ports: &[String], preferred: Option<&str>) -> Option<usize> {
    if let Some(name) = preferred {
        return ports.iter().position(|port| port == name);
    }
    ports.iter().position(|name| is_dj202(name))
}

fn is_dj202(name: &str) -> bool {
    name.to_ascii_lowercase()
        .replace(['-', ' '], "")
        .contains("dj202")
}

pub struct Midi {
    pub active_profile: ProfileId,
    pub bindings: Vec<Binding>,
    pub ports: Vec<String>,
    pub selected_port: usize,
    pub connected: Option<String>,
    pub error: Option<String>,
    pub last_message: Option<[u8; 3]>,
    pub last_action: Option<Action>,
    pub pad_modes: [Option<PadMode>; 4],
    pub learning: Option<Action>,
    pub dirty: bool,
    pub learn_action: Action,
    pub learn_mode: Mode,
    pub dj202_leds: bool,
    pub prefs: MidiPrefs,
    port_scan: Option<Receiver<Result<Vec<String>, String>>>,
    last_scan: Instant,
    last_retry: Option<Instant>,
    mapper: Mapper,
    input: Option<MidiInputConnection<()>>,
    output: Option<MidiOutputConnection>,
    receiver: Option<Receiver<[u8; 3]>>,
    last_leds: Option<([DeckLeds; 4], [bool; 4], [usize; 2])>,
    pending_actions: Vec<(Action, f32)>,
    shifted: [bool; 4],
}

impl Midi {
    pub fn new(bindings: Option<Vec<Binding>>, prefs: MidiPrefs) -> Self {
        let mut midi = Self::unconnected(bindings, prefs);
        midi.refresh();
        midi
    }

    fn unconnected(bindings: Option<Vec<Binding>>, mut prefs: MidiPrefs) -> Self {
        let active_profile = prefs
            .profile
            .or_else(|| {
                prefs
                    .preferred_port
                    .as_deref()
                    .map(|name| ProfileId::detect(name).unwrap_or(ProfileId::Generic))
            })
            .unwrap_or(ProfileId::RolandDj202);
        if prefs.mappings.is_empty()
            && let Some(old) = bindings.as_ref()
        {
            prefs.mappings.insert(active_profile, old.clone());
        }
        let bindings = prefs
            .mappings
            .get(&active_profile)
            .cloned()
            .unwrap_or_else(|| {
                if active_profile == ProfileId::RolandDj202 {
                    bindings.unwrap_or_else(dj202_profile)
                } else {
                    active_profile.defaults()
                }
            });
        let leds = prefs.led_feedback;
        Self {
            active_profile,
            bindings,
            ports: Vec::new(),
            selected_port: 0,
            connected: None,
            error: None,
            last_message: None,
            last_action: None,
            pad_modes: [None; 4],
            learning: None,
            dirty: false,
            learn_action: Action::Play(0),
            learn_mode: Mode::Absolute7,
            dj202_leds: leds,
            prefs,
            port_scan: None,
            last_scan: Instant::now(),
            last_retry: None,
            mapper: Mapper::for_profile(active_profile),
            input: None,
            output: None,
            receiver: None,
            last_leds: None,
            pending_actions: Vec::new(),
            shifted: [false; 4],
        }
    }

    pub fn save_current_profile(&mut self) {
        self.prefs
            .mappings
            .insert(self.active_profile, self.bindings.clone());
        self.prefs.led_feedback = self.dj202_leds;
    }

    fn release_cues(&mut self) {
        self.pending_actions.extend(
            self.mapper
                .cue_holds
                .drain(..)
                .map(|(_, _, d)| (Action::Cue(d), 0.0)),
        );
    }

    fn activate_profile(&mut self, profile: ProfileId) {
        if profile == self.active_profile {
            return;
        }
        self.save_current_profile();
        self.release_cues();
        self.active_profile = profile;
        self.bindings = self
            .prefs
            .mappings
            .get(&profile)
            .cloned()
            .unwrap_or_else(|| profile.defaults());
        self.mapper = Mapper::for_profile(profile);
        self.pad_modes = [None; 4];
        self.shifted = [false; 4];
        self.last_leds = None;
        self.learning = None;
        self.last_action = None;
        self.dirty = true;
    }

    pub fn choose_profile(&mut self, choice: Option<ProfileId>, ctx: &egui::Context) {
        let was_connected = self.connected.is_some();
        let profile = choice
            .or_else(|| self.connected.as_deref().and_then(ProfileId::detect))
            .or_else(|| {
                self.ports
                    .get(self.selected_port)
                    .and_then(|p| ProfileId::detect(p))
            })
            .unwrap_or(ProfileId::Generic);
        self.prefs.profile = choice;
        self.close_connection();
        self.activate_profile(profile);
        self.dirty = true;
        if was_connected
            && self.ports.get(self.selected_port).is_some_and(|p| {
                profile == ProfileId::Generic
                    || ProfileId::detect(p).is_none_or(|actual| actual == profile)
            })
        {
            self.connect(ctx);
        }
    }

    pub fn reset_profile(&mut self) {
        self.release_cues();
        self.bindings = self.active_profile.defaults();
        self.mapper = Mapper::for_profile(self.active_profile);
        self.pad_modes = [None; 4];
        self.shifted = [false; 4];
        self.learning = None;
        self.last_leds = None;
        self.dirty = true;
        self.save_current_profile();
    }

    fn automatic_port(&self) -> Option<usize> {
        if let Some(preferred) = self.prefs.preferred_port.as_ref() {
            if self.prefs.profile.is_some_and(|selected| {
                selected != ProfileId::Generic
                    && ProfileId::detect(preferred).is_some_and(|actual| selected != actual)
            }) {
                return None;
            }
            return self.ports.iter().position(|p| p == preferred);
        }
        let candidates: Vec<_> = self
            .ports
            .iter()
            .enumerate()
            .filter(|(_, name)| {
                ProfileId::detect(name)
                    .is_some_and(|p| self.prefs.profile.is_none_or(|selected| selected == p))
            })
            .map(|(i, _)| i)
            .collect();
        (candidates.len() == 1).then(|| candidates[0])
    }

    pub fn refresh(&mut self) {
        if self.port_scan.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = MidiInput::new("RustDJ ports")
                .map_err(|e| e.to_string())
                .map(|input| {
                    input
                        .ports()
                        .iter()
                        .filter_map(|p| input.port_name(p).ok())
                        .collect()
                });
            let _ = sender.send(result);
        });
        self.port_scan = Some(receiver);
        self.last_scan = Instant::now();
    }

    /// Discover hot-plugged devices without blocking the audio or UI thread.
    pub fn tick(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.port_scan {
            match receiver.try_recv() {
                Ok(result) => {
                    self.port_scan = None;
                    match result {
                        Ok(ports) => {
                            let selected = self.ports.get(self.selected_port).cloned();
                            self.ports = ports;
                            if self
                                .connected
                                .as_ref()
                                .is_some_and(|name| !self.ports.contains(name))
                            {
                                self.close_connection();
                            }
                            self.selected_port = selected
                                .as_ref()
                                .and_then(|name| self.ports.iter().position(|p| p == name))
                                .or_else(|| self.automatic_port())
                                .unwrap_or(0);
                        }
                        Err(error) => self.error = Some(format!("MIDI device scan: {error}")),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.port_scan = None;
                    self.error = Some("MIDI device scan stopped unexpectedly.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.last_scan.elapsed() >= Duration::from_secs(2) {
            self.refresh();
        }
        if self.prefs.auto_connect
            && self.connected.is_none()
            && self
                .last_retry
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(3))
            && let Some(index) = self.automatic_port()
        {
            self.selected_port = index;
            self.connect(ctx);
        }
    }

    pub fn connect(&mut self, ctx: &egui::Context) {
        self.close_connection();
        self.last_retry = Some(Instant::now());
        self.error = None;
        let result = (|| -> Result<(), String> {
            let mut input = MidiInput::new("RustDJ input").map_err(|e| e.to_string())?;
            input.ignore(Ignore::All);
            let ports = input.ports();
            let port = ports
                .get(self.selected_port)
                .ok_or("The MIDI port is no longer available. Refresh the list.")?;
            let name = input.port_name(port).map_err(|e| e.to_string())?;
            if self.ports.get(self.selected_port) != Some(&name) {
                return Err("The MIDI port list changed. Please refresh it.".into());
            }
            let profile = self
                .prefs
                .profile
                .unwrap_or_else(|| ProfileId::detect(&name).unwrap_or(ProfileId::Generic));
            self.activate_profile(profile);
            let (sender, receiver) = mpsc::sync_channel(512);
            let ctx = ctx.clone();
            self.input = Some(
                input
                    .connect(
                        port,
                        "RustDJ controller",
                        move |_, bytes, _| {
                            if bytes.len() == 3 && matches!(bytes[0] & 0xf0, 0x80 | 0x90 | 0xb0) {
                                let _ = sender.try_send([bytes[0], bytes[1], bytes[2]]);
                                ctx.request_repaint();
                            }
                        },
                        (),
                    )
                    .map_err(|e| format!("Could not open MIDI port: {e}"))?,
            );
            self.receiver = Some(receiver);
            // The output is also needed to unlock the pad layers, even with LEDs off.
            // Never send device-specific initialization to unrelated instruments.
            if self.active_profile.matches(&name) {
                let result = (|| -> Result<MidiOutputConnection, String> {
                    let output = MidiOutput::new("RustDJ LEDs").map_err(|e| e.to_string())?;
                    let ports = output.ports();
                    let matching: Vec<_> = ports
                        .iter()
                        .filter(|p| {
                            output
                                .port_name(p)
                                .is_ok_and(|n| self.active_profile.matches(&n))
                        })
                        .collect();
                    let port = ports
                        .iter()
                        .find(|p| output.port_name(p).is_ok_and(|n| n == name))
                        .or_else(|| (matching.len() == 1).then(|| matching[0]))
                        .ok_or("MIDI output is missing or ambiguous for the selected controller")?;
                    let mut connection = output
                        .connect(port, "RustDJ LEDs")
                        .map_err(|e| e.to_string())?;
                    if self.active_profile == ProfileId::RolandDj202 {
                        initialize_dj202(|message| {
                            connection.send(message).map_err(|e| e.to_string())
                        })?;
                    } else {
                        for message in self.active_profile.initialization() {
                            connection.send(message).map_err(|e| e.to_string())?;
                        }
                    }
                    Ok(connection)
                })();
                match result {
                    Ok(connection) => self.output = Some(connection),
                    Err(error) => {
                        self.error = Some(format!(
                            "MIDI input connected; pad unlock/LED output unavailable: {error}"
                        ))
                    }
                }
            }
            if self.prefs.preferred_port.as_ref() != Some(&name) {
                self.prefs.preferred_port = Some(name.clone());
                self.dirty = true;
            }
            self.connected = Some(name);
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    pub fn disconnect(&mut self) {
        self.prefs.auto_connect = false;
        self.dirty = true;
        self.close_connection();
    }

    fn close_connection(&mut self) {
        self.release_cues();
        self.input = None;
        self.output = None;
        self.receiver = None;
        self.connected = None;
        self.last_leds = None;
        self.pad_modes = [None; 4];
        self.last_message = None;
        self.last_action = None;
        self.learning = None;
        self.mapper = Mapper::for_profile(self.active_profile);
        self.shifted = [false; 4];
    }

    pub fn poll(&mut self) -> Vec<(Action, f32)> {
        let messages: Vec<_> = self
            .receiver
            .as_ref()
            .map_or(Vec::new(), |r| r.try_iter().take(512).collect());
        let mut actions = std::mem::take(&mut self.pending_actions);
        for message in messages {
            if matches!(message[0] & 0xf0, 0x80 | 0x90) {
                self.last_leds = None;
            }
            self.last_message = Some(message);
            if self.active_profile == ProfileId::RolandDj202
                && self.connected.as_deref().is_some_and(is_dj202)
                && let Some((deck, mode)) = pad_mode_message(message)
            {
                self.mapper.observe_deck(message);
                self.pad_modes[deck] = Some(mode);
                // Mode selection must not become a learned pad binding.
                continue;
            }
            let [status, number, value] = message;
            let note = status & 0xf0;
            if self.active_profile == ProfileId::NumarkMixtrackPro3
                && matches!(status & 0xf, 1 | 2)
                && matches!(note, 0x80 | 0x90)
                && number == 0x0b
            {
                self.shifted[(status & 0xf) as usize - 1] = note == 0x90 && value > 0;
                continue;
            }
            if matches!(
                self.active_profile,
                ProfileId::RolandDj505 | ProfileId::NumarkMixtrackPlatinumFx
            ) && (0x94..=0x97).contains(&status)
                && number <= 0x0f
            {
                if value > 0 {
                    let d = (status - 0x94) as usize;
                    self.mapper.layers[d % 2] = d;
                    self.pad_modes[d] = Some(match number {
                        0 => PadMode::Hotcue,
                        0x0d => PadMode::Loop,
                        n => PadMode::Other(n),
                    });
                }
                continue;
            }
            if let Some(action) = self.learning {
                if let Some(binding) = learn_binding(action, self.learn_mode, message) {
                    self.bindings.retain(|b| {
                        b.action != action
                            && !(b.status == binding.status
                                && (b.number == binding.number
                                    || b.mode.lsb() == Some(binding.number)))
                    });
                    self.bindings.push(binding);
                    self.learning = None;
                    self.dirty = true;
                }
                continue;
            }
            let mut decoded = self.mapper.decode(message, &self.bindings);
            if self.active_profile == ProfileId::NumarkMixtrackPro3 {
                for (action, _) in &mut decoded {
                    if let Action::Hotcue(d, s) = *action
                        && self.shifted.get(d).copied().unwrap_or(false)
                    {
                        *action = Action::ClearHotcue(d, s);
                    }
                }
            }
            if matches!(
                self.active_profile,
                ProfileId::RolandDj505 | ProfileId::NumarkMixtrackPlatinumFx
            ) && (0x94..=0x97).contains(&status)
                && (0x14..=0x23).contains(&number)
            {
                let mode = self.pad_modes[(status - 0x94) as usize];
                decoded.retain_mut(|(action, _)| match *action {
                    Action::Hotcue(d, s) => match mode {
                        Some(PadMode::Hotcue) => true,
                        Some(PadMode::Loop) => {
                            *action = Action::LoopPad(d, s);
                            true
                        }
                        _ => false,
                    },
                    Action::ClearHotcue(..) => mode == Some(PadMode::Hotcue),
                    _ => true,
                });
            }
            if let Some((action, _)) = decoded.last() {
                self.last_action = Some(*action);
            }
            actions.extend(decoded);
        }
        actions
    }

    pub fn feedback(&mut self, decks: [DeckLeds; 4], synced: [bool; 4]) {
        let state = (decks, synced, self.mapper.layers);
        if !self.dj202_leds
            || self.last_leds == Some(state)
            || !self
                .connected
                .as_deref()
                .is_some_and(|name| self.active_profile.matches(name))
        {
            return;
        }
        if let Some(output) = &mut self.output {
            let messages = if self.active_profile == ProfileId::RolandDj202 {
                all_led_messages(decks, synced, self.mapper.layers)
            } else {
                crate::controller_profiles::basic_leds(self.active_profile, decks, synced)
            };
            for message in messages {
                if let Err(error) = output.send(&message) {
                    self.error = Some(format!("LED output failed: {error}"));
                    return;
                }
            }
            self.last_leds = Some(state);
        }
    }
}

pub fn learn_binding(action: Action, learn_mode: Mode, message: [u8; 3]) -> Option<Binding> {
    let [status, number, value] = message;
    if number > 127 || value > 127 {
        return None;
    }
    let mode = match status & 0xf0 {
        0x90 if value > 0 && matches!(action, Action::Cue(_)) => Mode::Gate,
        0xb0 if matches!(action, Action::Cue(_)) => Mode::Gate,
        0x90 if value > 0 && !action.continuous() => Mode::Button,
        0xb0 if matches!(learn_mode, Mode::Relative64 | Mode::RelativeTwos) => learn_mode,
        0xb0 if action.continuous() => {
            if learn_mode == Mode::Absolute7Inverted {
                Mode::Absolute7Inverted
            } else {
                Mode::Absolute7
            }
        }
        0xb0 if value >= 64 => Mode::Button,
        _ => return None,
    };
    Some(Binding {
        status,
        number,
        mode,
        action,
    })
}

#[cfg(test)]
mod profile_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression18_native_deck_banks_switch_both_ways_without_redirecting_controls() {
        use crate::{
            decks::{COUNT, ORDER, sync_reference},
            dsp::bus_gains_with_crossfader,
        };
        assert_eq!(ORDER.map(|i| i + 1), [1, 3, 4, 2]);
        for width in [1268.0, 1668.0, 1908.0] {
            let (mixer, deck) = crate::app::console_dimensions(width);
            assert!(deck >= 200.0 && mixer >= 400.0);
            assert!((mixer + deck * 4.0 + 24.0 - width).abs() < 0.01);
        }
        let profile = dj202_profile();
        for deck in 0..COUNT {
            let mut mapper = Mapper::default();
            assert_eq!(
                mapper.decode([0x90 + deck as u8, 0, 127], &profile),
                vec![(Action::Play(deck), 1.0)]
            );
            assert_eq!(
                mapper.decode([0x9f, 2 + deck as u8, 127], &profile),
                vec![(Action::Load(deck), 1.0)]
            );
            for slot in 0..8 {
                assert_eq!(
                    mapper.decode([0x94 + deck as u8, 1 + slot as u8, 127], &profile),
                    vec![(Action::Hotcue(deck, slot), 1.0)]
                );
                assert_eq!(
                    mapper.decode([0x94 + deck as u8, 0x11 + slot as u8, 127], &profile),
                    vec![(Action::LoopPad(deck, slot), 1.0)]
                );
                assert!(
                    mapper
                        .decode([0x94 + deck as u8, 0x11 + slot as u8, 0], &profile)
                        .is_empty()
                );
            }
            mapper.decode([0xb0 + deck as u8, 0x1c, 127], &profile);
            assert_eq!(
                mapper.decode([0xb0 + deck as u8, 0x3a, 127], &profile),
                vec![(Action::Volume(deck), 1.0)]
            );
        }
        let mut mapper = Mapper::default();
        assert_eq!(
            mapper.decode([0x90, 1, 127], &profile),
            vec![(Action::Cue(0), 1.0)]
        );
        assert_eq!(
            mapper.decode([0x90, 8, 127], &profile),
            vec![(Action::SelectDeck(2), 1.0)]
        );
        assert!(mapper.decode([0x90, 8, 0], &profile).is_empty());
        assert_eq!(
            mapper.decode([0x82, 1, 0], &profile),
            vec![(Action::Cue(0), 0.0)]
        );
        assert_eq!(
            mapper.decode([0x92, 0, 127], &profile),
            vec![(Action::Play(2), 1.0)]
        );
        assert_eq!(
            mapper.decode([0x98, 1, 127], &profile),
            vec![(Action::FxToggle(2, 1), 1.0)]
        );
        assert_eq!(
            mapper.decode([0xb8, 0, 127], &profile),
            vec![(Action::FxLevel(2), 1.0)]
        );
        assert_eq!(
            mapper.decode([0x9f, 2, 127], &profile),
            vec![(Action::Load(2), 1.0)]
        );
        assert_eq!(
            mapper.decode([0x91, 8, 127], &profile),
            vec![(Action::SelectDeck(3), 1.0)]
        );
        assert_eq!(
            mapper.decode([0x99, 0, 127], &profile),
            vec![(Action::FxToggle(3, 0), 1.0)]
        );
        assert_eq!(
            mapper.decode([0x90, 8, 127], &profile),
            vec![(Action::SelectDeck(0), 1.0)]
        );
        // Native channels are authoritative in BOTH directions, irrespective
        // of the last software toggle, other side, or startup bank snapshot.
        for side in 0..2 {
            for deck in [side + 2, side, side + 2, side] {
                let note = 0x90 + deck as u8;
                let pad = 0x94 + deck as u8;
                let cc = 0xb0 + deck as u8;
                for (number, action) in [
                    (0, Action::Play(deck)),
                    (2, Action::Sync(deck)),
                    (0x1b, Action::Pfl(deck)),
                ] {
                    assert_eq!(
                        mapper.decode([note, number, 127], &profile),
                        vec![(action, 1.0)]
                    );
                    assert!(mapper.decode([note, number, 0], &profile).is_empty());
                }
                for (msb, lsb, action) in [
                    (0x1c, 0x3a, Action::Volume(deck)),
                    (0x16, 0x34, Action::Gain(deck)),
                    (0x17, 0x35, Action::EqHigh(deck)),
                    (0x18, 0x36, Action::EqMid(deck)),
                    (0x19, 0x37, Action::EqLow(deck)),
                    (0x09, 0x3b, Action::Tempo(deck)),
                ] {
                    assert!(mapper.decode([cc, msb, 127], &profile).is_empty());
                    assert_eq!(mapper.decode([cc, lsb, 127], &profile), vec![(action, 1.0)]);
                }
                assert_eq!(
                    mapper.decode([cc, 0x1a, 0], &profile),
                    vec![(Action::Filter(deck), 0.0)]
                );
                for slot in 0..8 {
                    assert_eq!(
                        mapper.decode([pad, 1 + slot, 127], &profile),
                        vec![(Action::Hotcue(deck, slot as usize), 1.0)]
                    );
                    assert_eq!(
                        mapper.decode([pad, 0x11 + slot, 127], &profile),
                        vec![(Action::LoopPad(deck, slot as usize), 1.0)]
                    );
                }
                assert_eq!(
                    mapper.decode([0x98 + side as u8, 0, 127], &profile),
                    vec![(Action::FxToggle(deck, 0), 1.0)]
                );
                assert_eq!(
                    mapper.decode([0xb8 + side as u8, 0, 127], &profile),
                    vec![(Action::FxLevel(deck), 1.0)]
                );
                assert_eq!(
                    mapper.decode([0x9f, 2 + side as u8, 127], &profile),
                    vec![(Action::Load(deck), 1.0)]
                );
                assert_eq!(
                    mapper.decode([note, 1, 127], &profile),
                    vec![(Action::Cue(deck), 1.0)]
                );
                let other = deck ^ 2;
                mapper.decode([0x90 + other as u8, 0, 127], &profile);
                assert_eq!(
                    mapper.decode([0x80 + other as u8, 1, 0], &profile),
                    vec![(Action::Cue(deck), 0.0)]
                );
                assert_eq!(mapper.layers[side], other);
            }
        }
        // A learned absolute target is not a physical-side alias.
        let custom_deck = [Binding {
            status: 0x90,
            number: 0,
            mode: Mode::Button,
            action: Action::Play(1),
        }];
        mapper.decode([0x92, 0, 127], &profile);
        assert_eq!(
            mapper.decode([0x90, 0, 127], &custom_deck),
            vec![(Action::Play(1), 1.0)]
        );
        let (sender, receiver) = mpsc::sync_channel(8);
        let mut midi = Midi::new(Some(profile.clone()), MidiPrefs::default());
        midi.connected = Some("DJ-202".into());
        midi.receiver = Some(receiver);
        for deck in [2, 0, 3, 1] {
            sender.send([0x94 + deck, 0, 0]).unwrap();
            assert!(midi.poll().is_empty());
            assert_eq!(midi.pad_modes[deck as usize], Some(PadMode::Hotcue));
            assert_eq!(midi.mapper.layers[deck as usize % 2], deck as usize);
        }
        let mut decks = [DeckLeds::default(); 4];
        decks[2].effects[1] = true;
        decks[3].hotcues[7] = true;
        let messages = all_led_messages(decks, [false, false, true, true], [2, 3]);
        assert!(messages.contains(&[0x98, 1, 127]));
        assert!(messages.contains(&[0x97, 8, 127]));
        assert!(messages.contains(&[0x92, 2, 127]));
        assert!(messages.contains(&[0x90, 8, 127]));
        let old: Vec<_> = profile
            .iter()
            .filter(|b| {
                !matches!(b.status, 0x92 | 0x93 | 0x96 | 0x97 | 0xb2 | 0xb3)
                    && !matches!(b.action, Action::DeckLayer(_) | Action::Load(2..))
            })
            .cloned()
            .collect();
        let upgraded = upgrade(old);
        assert!(upgraded.iter().any(|b| b.action == Action::Play(3)));
        assert!(upgraded.iter().any(|b| b.action == Action::DeckLayer(1)));
        assert_eq!(upgrade(upgraded.clone()).len(), upgraded.len());
        let custom = upgrade(vec![Binding {
            status: 0x92,
            number: 0,
            mode: Mode::Button,
            action: Action::Master,
        }]);
        assert_eq!(
            Mapper::default().decode([0x92, 0, 127], &custom),
            vec![(Action::Master, 1.0)]
        );
        assert_eq!(
            bus_gains_with_crossfader([0.2, 0.4, 0.6, 0.8], [true; 4], -1.0, 1.0, 0.8, 0.0, true)
                .master,
            [0.2, 0.0, 0.6, 0.0]
        );
        assert_eq!(
            bus_gains_with_crossfader([0.2, 0.4, 0.6, 0.8], [true; 4], 1.0, 1.0, 0.8, 0.0, true)
                .master,
            [0.0, 0.4, 0.0, 0.8]
        );
        let bypass =
            bus_gains_with_crossfader([0.2, 0.4, 0.6, 0.8], [true; 4], -1.0, 1.0, 0.8, 0.0, false);
        assert_eq!(bypass.master, [0.2, 0.4, 0.6, 0.8]);
        assert_eq!(bypass.phones, [0.4; 4]);
        assert_eq!(
            sync_reference(
                2,
                [true; 4],
                [false, true, false, true],
                [0.1, 0.3, 1.0, 0.7]
            ),
            Some(3)
        );
        assert_eq!(
            sync_reference(3, [false, false, false, true], [true; 4], [1.0; 4]),
            None
        );
    }

    #[test]
    fn controller12_unlocks_layers_and_tracks_modes_without_hijacking_midi_learn() {
        let mut sent = Vec::new();
        initialize_dj202(|bytes| {
            sent.push(bytes.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            sent,
            [
                vec![0xf0, 0, 0x20, 0x7f, 1, 0xf7],
                vec![0xf0, 0, 0x20, 0x7f, 0, 0xf7],
            ]
        );
        let mut attempts = 0;
        let error = initialize_dj202(|_| {
            attempts += 1;
            Err("Port getrennt".into())
        })
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert!(error.contains("pad layers"));

        let mut midi = Midi::unconnected(None, MidiPrefs::default());
        midi.connected = Some("Roland DJ-202".into());
        midi.learning = Some(Action::Hotcue(1, 7));
        let (sender, receiver) = mpsc::channel();
        midi.receiver = Some(receiver);
        sender.send([0x94, 0, 0x10]).unwrap();
        sender.send([0x95, 0, 0x00]).unwrap();
        assert!(midi.poll().is_empty());
        assert_eq!(
            midi.pad_modes,
            [Some(PadMode::Loop), Some(PadMode::Hotcue), None, None]
        );
        assert_eq!(midi.learning, Some(Action::Hotcue(1, 7)));
        sender.send([0x85, 0, 127]).unwrap();
        assert!(midi.poll().is_empty());
        assert_eq!(midi.pad_modes[1], Some(PadMode::Hotcue));
        sender.send([0x94, 0, 0x30]).unwrap();
        assert!(midi.poll().is_empty());
        assert!(
            midi.pad_modes[0]
                .unwrap()
                .label()
                .contains("select HOT CUE/LOOP")
        );
        assert_eq!(pad_mode_message([0x96, 0, 0]), Some((2, PadMode::Hotcue)));
        assert!(pad_mode_message([0x94, 0, 128]).is_none());
        // Learn only the actual pad, then verify its next press reaches the action.
        sender.send([0x95, 8, 127]).unwrap();
        assert!(midi.poll().is_empty());
        assert!(midi.learning.is_none());
        sender.send([0x95, 8, 0]).unwrap();
        sender.send([0x95, 8, 127]).unwrap();
        assert_eq!(midi.poll(), vec![(Action::Hotcue(1, 7), 1.0)]);
        assert_eq!(midi.last_action, Some(Action::Hotcue(1, 7)));
        midi.close_connection();
        assert_eq!(midi.pad_modes, [None; 4]);
        assert!(midi.last_action.is_none());

        // A generic controller's note 0 remains learnable.
        midi.connected = Some("MIDI keyboard".into());
        midi.learning = Some(Action::LoopToggle(0));
        let (sender, receiver) = mpsc::channel();
        midi.receiver = Some(receiver);
        sender.send([0x94, 0, 127]).unwrap();
        assert!(midi.poll().is_empty());
        assert!(midi.learning.is_none());
        sender.send([0x94, 0, 127]).unwrap();
        assert_eq!(midi.poll(), vec![(Action::LoopToggle(0), 1.0)]);
    }

    #[test]
    fn controller12_all_eight_hotcues_and_loops_on_both_decks_preserve_custom_maps() {
        let mut midi = Midi::unconnected(None, MidiPrefs::default());
        midi.connected = Some("DJ 202".into());
        let (sender, receiver) = mpsc::channel();
        midi.receiver = Some(receiver);
        assert_eq!(
            crate::loops::PRESETS.map(|p| p.0),
            [0.25, 0.5, 1., 2., 4., 8., 16., 32.]
        );
        for deck in 0..2 {
            let status = 0x94 + deck as u8;
            for slot in 0..8 {
                for (mode, note, action) in [
                    (0x00, 1 + slot as u8, Action::Hotcue(deck, slot)),
                    (0x00, 9 + slot as u8, Action::ClearHotcue(deck, slot)),
                    (0x10, 0x11 + slot as u8, Action::LoopPad(deck, slot)),
                ] {
                    sender.send([status, 0, mode]).unwrap();
                    sender.send([status, note, 127]).unwrap();
                    sender.send([status, note, 0]).unwrap();
                    sender.send([status - 0x10, note, 127]).unwrap();
                    assert_eq!(midi.poll(), vec![(action, 1.0)]);
                    assert_eq!(midi.last_action, Some(action));
                }
            }
            for (note, action) in [
                (0x41, Action::LoopHalf(deck)),
                (0x42, Action::LoopDouble(deck)),
            ] {
                sender.send([status, note, 127]).unwrap();
                sender.send([status, note, 0]).unwrap();
                assert_eq!(midi.poll(), vec![(action, 1.0)]);
            }
        }
        let mut decks = [DeckLeds::default(); 2];
        decks[0].hotcues = [true; 8];
        decks[1].loops[7] = true;
        let messages = led_messages(decks, None);
        for slot in 0..8 {
            assert!(messages.contains(&[0x94, 1 + slot, 127]));
            assert!(messages.contains(&[0x95, 1 + slot, 0]));
            assert!(messages.contains(&[0x95, 0x11 + slot, if slot == 7 { 127 } else { 0 }]));
        }
        // Fill missing stock assignments while retaining an intentionally remapped pad.
        let custom = Binding {
            status: 0x94,
            number: 0x11,
            mode: Mode::Button,
            action: Action::Stop(0),
        };
        let upgraded = upgrade(vec![custom]);
        let mut mapper = Mapper::default();
        assert_eq!(
            mapper.decode([0x94, 0x11, 127], &upgraded),
            vec![(Action::Stop(0), 1.0)]
        );
        assert_eq!(
            mapper.decode([0x95, 0x18, 127], &upgraded),
            vec![(Action::LoopPad(1, 7), 1.0)]
        );
        assert_eq!(
            mapper.decode([0x94, 8, 127], &upgraded),
            vec![(Action::Hotcue(0, 7), 1.0)]
        );
        assert_eq!(upgrade(upgraded.clone()).len(), upgraded.len());
        assert!(upgrade(Vec::new()).is_empty());
    }

    #[test]
    fn loop_pads_controller_migrates_and_decodes_all_eight_without_release_retrigger() {
        let profile = dj202_profile();
        let mut mapper = Mapper::default();
        for d in 0..2 {
            let status = 0x94 + d as u8;
            for slot in 0..8 {
                assert_eq!(
                    mapper.decode([status, slot as u8 + 1, 127], &profile),
                    vec![(Action::Hotcue(d, slot), 1.0)]
                );
                assert!(
                    mapper
                        .decode([status, slot as u8 + 1, 0], &profile)
                        .is_empty()
                );
                assert!(
                    mapper
                        .decode([status - 0x10, slot as u8 + 1, 64], &profile)
                        .is_empty()
                );
                assert_eq!(
                    mapper.decode([status, slot as u8 + 9, 127], &profile),
                    vec![(Action::ClearHotcue(d, slot), 1.0)]
                );
                assert_eq!(
                    mapper.decode([status, slot as u8 + 0x11, 127], &profile),
                    vec![(Action::LoopPad(d, slot), 1.0)]
                );
                assert!(
                    mapper
                        .decode([status, slot as u8 + 0x11, 0], &profile)
                        .is_empty()
                );
                assert!(
                    mapper
                        .decode([status - 0x10, slot as u8 + 0x11, 127], &profile)
                        .is_empty()
                );
            }
            for (note, action) in [(0x41, Action::LoopHalf(d)), (0x42, Action::LoopDouble(d))] {
                assert_eq!(
                    mapper.decode([status, note, 127], &profile),
                    vec![(action, 1.0)]
                );
                assert!(mapper.decode([status, note, 0], &profile).is_empty());
            }
        }
        let mut old = profile;
        for binding in &mut old {
            if let Action::LoopPad(d, slot) = binding.action {
                binding.action = [
                    Action::LoopPreset(d, 1),
                    Action::LoopPreset(d, 2),
                    Action::LoopPreset(d, 4),
                    Action::LoopPreset(d, 8),
                    Action::LoopIn(d),
                    Action::LoopOut(d),
                    Action::LoopExit(d),
                    Action::Reloop(d),
                ][slot];
            }
        }
        let upgraded = upgrade(old.clone());
        for d in 0..2 {
            for slot in 0..8 {
                assert_eq!(
                    mapper.decode([0x94 + d as u8, 0x11 + slot as u8, 127], &upgraded),
                    vec![(Action::LoopPad(d, slot), 1.0)]
                );
            }
        }
        assert_eq!(upgrade(upgraded.clone()).len(), upgraded.len());
        old.iter_mut()
            .find(|b| b.status == 0x94 && b.number == 0x15)
            .unwrap()
            .action = Action::Master;
        let custom = upgrade(old);
        assert_eq!(
            mapper.decode([0x94, 0x15, 127], &custom),
            vec![(Action::Master, 1.0)]
        );
        let mut decks = [DeckLeds::default(); 2];
        decks[0].loops[7] = true;
        assert!(led_messages(decks, None).contains(&[0x94, 0x18, 127]));
    }

    #[test]
    fn pad_led_feedback_uses_separate_banks_and_reflects_hotcue_and_loop_state() {
        let mut decks = [DeckLeds::default(); 2];
        decks[0].hotcues[3] = true;
        decks[1].loops[2] = true;
        let messages = led_messages(decks, None);
        assert!(messages.contains(&[0x94, 4, 127]));
        assert!(messages.contains(&[0x95, 4, 0]));
        assert!(messages.contains(&[0x95, 0x13, 127]));
        assert!(messages.contains(&[0x94, 0x13, 0]));
    }
    #[test]
    fn auto_connect_prefers_saved_device_and_ignores_unrelated_midi_ports() {
        let ports = vec![
            "MIDI keyboard".into(),
            "DJ-202".into(),
            "DJ-202 CTRL".into(),
        ];
        assert_eq!(auto_port(&ports, None), Some(1));
        assert_eq!(auto_port(&ports, Some("DJ-202 CTRL")), Some(2));
        assert_eq!(auto_port(&ports, Some("Offline controller")), None);
        assert_eq!(auto_port(&ports[..1], None), None);
        let prefs: MidiPrefs = serde_json::from_str("{}").unwrap();
        assert!(prefs.auto_connect);
        assert!(prefs.preferred_port.is_none());
    }

    #[test]
    fn gain_trim_uses_14_bit_midi_and_migrates_without_overwriting_custom_maps() {
        let profile = dj202_profile();
        let mut mapper = Mapper::default();
        assert!(mapper.decode([0xb1, 0x16, 64], &profile).is_empty());
        let action = mapper.decode([0xb1, 0x34, 0], &profile);
        assert_eq!(action[0].0, Action::Gain(1));
        assert_eq!(crate::dsp::knob_db(action[0].1), 0.0);
        let mut old = dj202_profile();
        old.retain(|b| !matches!(b.action, Action::Gain(_)));
        let upgraded = upgrade(old);
        assert_eq!(
            upgraded
                .iter()
                .filter(|b| matches!(b.action, Action::Gain(_)))
                .count(),
            4
        );
    }
    #[test]
    fn cue_releases_eq_pfl_and_phones_controls_decode_correctly() {
        let mut mapper = Mapper::default();
        let map = dj202_profile();
        assert_eq!(
            mapper.decode([0x90, 1, 127], &map),
            vec![(Action::Cue(0), 1.0)]
        );
        assert_eq!(
            mapper.decode([0x80, 1, 127], &map),
            vec![(Action::Cue(0), 0.0)]
        );
        assert_eq!(
            mapper.decode([0x90, 1, 0], &map),
            vec![(Action::Cue(0), 0.0)]
        );
        assert_eq!(
            mapper.decode([0x91, 0x1b, 127], &map),
            vec![(Action::Pfl(1), 1.0)]
        );
        assert!(mapper.decode([0x91, 0x1b, 0], &map).is_empty());
        mapper.decode([0xb0, 0x19, 64], &map);
        let values = mapper.decode([0xb0, 0x37, 0], &map);
        assert_eq!(values[0].0, Action::EqLow(0));
        assert_eq!(crate::dsp::knob_db(values[0].1), 0.0);
        assert_eq!(
            mapper.decode([0xbf, 0x0d, 127], &map),
            vec![(Action::PhonesMix, 1.0)]
        );
        assert_eq!(
            mapper.decode([0xbf, 0x0c, 0], &map),
            vec![(Action::PhonesLevel, 0.0)]
        );
        assert!(!map.iter().any(|b| b.action == Action::Master));
    }

    #[test]
    fn old_profiles_gain_mixer_functions_without_overwriting_custom_mappings() {
        let custom = Binding {
            status: 0xb4,
            number: 10,
            action: Action::PhonesMix,
            mode: Mode::Absolute7,
        };
        let old = vec![
            Binding {
                status: 0x90,
                number: 1,
                action: Action::Cue(0),
                mode: Mode::Button,
            },
            custom,
        ];
        let new = upgrade(old);
        assert!(
            new.iter()
                .any(|b| b.action == Action::Cue(0) && b.mode == Mode::Gate)
        );
        assert_eq!(
            new.iter().filter(|b| b.action == Action::PhonesMix).count(),
            1
        );
        assert!(
            new.iter()
                .any(|b| b.action == Action::PhonesMix && b.status == 0xb4)
        );
        assert!(new.iter().any(|b| b.action == Action::Pfl(1)));
    }
    #[test]
    fn dj202_transport_only_triggers_on_press_and_uses_separate_decks() {
        let mut mapper = Mapper::default();
        let map = dj202_profile();
        assert_eq!(
            mapper.decode([0x91, 0, 127], &map),
            vec![(Action::Play(1), 1.0)]
        );
        assert!(mapper.decode([0x91, 0, 0], &map).is_empty());
        assert!(mapper.decode([0x81, 0, 127], &map).is_empty());
        assert_eq!(
            mapper.decode([0x92, 0, 127], &map),
            vec![(Action::Play(2), 1.0)]
        );
        assert_eq!(
            mapper.decode([0x90, 2, 127], &map),
            vec![(Action::Sync(2), 1.0)]
        );
    }
    #[test]
    fn high_resolution_faders_combine_msb_lsb_without_double_triggering() {
        let mut mapper = Mapper::default();
        let map = dj202_profile();
        assert!(mapper.decode([0xb0, 0x1c, 127], &map).is_empty());
        assert_eq!(
            mapper.decode([0xb0, 0x3a, 127], &map),
            vec![(Action::Volume(0), 1.0)]
        );
        mapper.decode([0xbf, 8, 127], &map);
        assert_eq!(
            mapper.decode([0xbf, 0x58, 112], &map),
            vec![(Action::Crossfader, 1.0)]
        );
        assert_eq!(
            mapper.decode([0xbf, 0, 127], &map),
            vec![(Action::Browse, -1.0)]
        );
    }
    #[test]
    fn learn_roundtrips_and_invalid_messages_are_ignored() {
        assert!(learn_binding(Action::Play(0), Mode::Absolute7, [0x90, 1, 0]).is_none());
        let binding = learn_binding(Action::Volume(1), Mode::Absolute7, [0xb4, 10, 80]).unwrap();
        let saved = serde_json::to_string(&vec![binding]).unwrap();
        let bindings: Vec<Binding> = serde_json::from_str(&saved).unwrap();
        assert_eq!(
            Mapper::default().decode([0xb4, 10, 127], &bindings),
            vec![(Action::Volume(1), 1.0)]
        );
    }
}
