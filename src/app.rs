use crate::{
    audio::{Audio, Deck},
    beat::{BeatGrid, sync_plan},
    browser::{Browser, Event as BrowserEvent},
    decks::{COUNT, ORDER},
    dragdrop::{self, Origin, Target as DropTarget, Zones},
    dsp::{bus_gains_with_crossfader, knob_db},
    equalizer::{self, Settings as MasterEqSettings},
    loops::PRESETS as LOOP_PRESETS,
    midi::{self, Action, Midi, Mode},
    output::{self, DeviceInfo, Routing, Target},
    performance::Store as HotcueStore,
    playlist::{Item, Playlist},
    settings::{self, MixSettings, Settings},
    streamer::{Broadcaster, Phase, StreamSettings},
    track,
};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

const BG: Color32 = Color32::from_rgb(18, 19, 21);
const PANEL: Color32 = Color32::from_rgb(36, 38, 41);
const BORDER: Color32 = Color32::from_rgb(61, 64, 69);
const MUTED: Color32 = Color32::from_rgb(161, 151, 179);
const PURPLE: Color32 = Color32::from_rgb(174, 119, 255);
const LIGHT_PURPLE: Color32 = Color32::from_rgb(219, 186, 255);
const WARNING: Color32 = Color32::from_rgb(247, 101, 112);

struct Crossfader {
    position: f32,
    enabled: bool,
}

impl Crossfader {
    fn new(enabled: bool) -> Self {
        Self {
            position: 0.0,
            enabled,
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.position = 0.0;
        }
    }

    fn set_position(&mut self, position: f32) {
        if self.enabled {
            self.position = position.clamp(-1.0, 1.0);
        }
    }

    fn nudge(&mut self, amount: f32) {
        self.set_position(self.position + amount);
    }

    fn midi(&mut self, action: Action, value: f32) {
        match action {
            Action::Crossfader => self.set_position(value.clamp(0.0, 1.0) * 2.0 - 1.0),
            Action::CrossfaderToggle => self.set_enabled(!self.enabled),
            _ => {}
        }
    }
}

pub struct RustDj {
    decks: [Deck; COUNT],
    audio: Audio,
    crossfader: Crossfader,
    wave_window: f64,
    master: f32,
    phones: f32,
    mixing: f32,
    show_audio: bool,
    routing_edit: Routing,
    audio_devices: Vec<DeviceInfo>,
    device_scan: Option<Receiver<Result<Vec<DeviceInfo>, String>>>,
    audio_dirty: bool,
    library: Vec<Item>,
    playlist: Playlist,
    playlist_tab: bool,
    search: String,
    active: usize,
    notice: Option<String>,
    browser: Browser,
    midi: Midi,
    show_midi: bool,
    mixer_outputs: bool,
    synced: [Option<usize>; COUNT],
    taps: [Vec<Instant>; COUNT],
    hotcue_store: HotcueStore,
    bus_levels: [[f32; 2]; 2],
    broadcaster: Broadcaster,
    stream_settings: StreamSettings,
    stream_password: String,
    stream_password_error: Option<String>,
    show_streamer: bool,
    master_eq: MasterEqSettings,
    show_equalizer: bool,
}

impl RustDj {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = &cc.egui_ctx;
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BG;
        visuals.window_fill = PANEL;
        visuals.extreme_bg_color = Color32::from_rgb(18, 13, 24);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(44, 38, 57);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(65, 51, 83);
        visuals.selection.bg_fill = Color32::from_rgb(79, 51, 111);
        visuals.selection.stroke = Stroke::new(1.0_f32, PURPLE);
        visuals.hyperlink_color = LIGHT_PURPLE;
        ctx.set_visuals(visuals);
        ctx.style_mut(|style| {
            style.spacing.item_spacing = Vec2::new(6.0, 5.0);
            style.spacing.button_padding = Vec2::new(9.0, 5.0);
            style
                .text_styles
                .insert(egui::TextStyle::Body, FontId::proportional(12.0));
            style
                .text_styles
                .insert(egui::TextStyle::Button, FontId::proportional(12.0));
        });
        let (mut settings, notice) = settings::load();
        let migrated = settings.mapping_version < 6;
        if migrated && let Some(bindings) = settings.bindings.take() {
            settings.bindings = Some(midi::upgrade(bindings));
        }
        let audio = Audio::new(settings.routing.clone());
        audio.master_eq.set(settings.master_eq);
        let (stream_password, stream_password_error) =
            match crate::credentials::unprotect(&settings.streamer.protected_password) {
                Ok(password) => (password, None),
                Err(error) => (String::new(), Some(error)),
            };
        Self {
            decks: std::array::from_fn(|_| Deck::default()),
            audio,
            crossfader: Crossfader::new(settings.mix.crossfader_enabled),
            wave_window: 4.0,
            master: settings.mix.master.clamp(0.0, 1.0),
            phones: settings.mix.phones.clamp(0.0, 1.0),
            mixing: settings.mix.mixing.clamp(0.0, 1.0),
            show_audio: false,
            routing_edit: settings.routing,
            audio_devices: Vec::new(),
            device_scan: None,
            audio_dirty: migrated,
            library: Vec::new(),
            playlist: Playlist::default(),
            playlist_tab: false,
            search: String::new(),
            active: 0,
            notice,
            browser: Browser::new(settings.browser, ctx),
            midi: Midi::new(settings.bindings, settings.midi),
            show_midi: false,
            mixer_outputs: false,
            synced: [None; COUNT],
            taps: std::array::from_fn(|_| Vec::new()),
            hotcue_store: HotcueStore::default(),
            bus_levels: [[0.0; 2]; 2],
            broadcaster: Broadcaster::default(),
            stream_settings: settings.streamer,
            stream_password,
            stream_password_error,
            show_streamer: false,
            master_eq: settings.master_eq,
            show_equalizer: false,
        }
    }

    fn pick(&mut self, index: usize, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title(format!("Load music into Deck {}", index + 1))
            .add_filter("Music files", track::EXTENSIONS)
            .pick_file()
        {
            self.decks[index].load(path, ctx.clone());
            self.active = index;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        for (key, deck) in [
            (egui::Key::Num1, 0),
            (egui::Key::Num2, 1),
            (egui::Key::Num3, 2),
            (egui::Key::Num4, 3),
        ] {
            if ctx.input(|i| i.key_pressed(key)) {
                self.active = deck;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::A)) {
            self.active = 0;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::B)) {
            self.active = 1;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle_deck(self.active);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::S)) {
            self.sync(self.active);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::C)) {
            self.decks[self.active].go_to_cue(&self.audio);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            for deck in &mut self.decks {
                deck.stop(&self.audio);
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            self.crossfader.nudge(-0.05);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            self.crossfader.nudge(0.05);
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("RUST").size(21.0).strong());
            ui.label(RichText::new("DJ").size(21.0).strong().color(PURPLE));
            ui.label(RichText::new("0.19 / 4 DECKS").size(10.0).color(MUTED));
            ui.separator();
            let midi =
                controller_status(self.midi.connected.as_deref(), self.midi.prefs.auto_connect);
            ui.add_sized(
                [(ui.available_width() - 280.0).max(90.0), 24.0],
                egui::Label::new(RichText::new(&midi).size(12.0).strong().color(
                    if self.midi.connected.is_some() {
                        PURPLE
                    } else {
                        LIGHT_PURPLE
                    },
                ))
                .truncate(),
            )
            .on_hover_text(midi);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Audio / Outputs").clicked() {
                    self.show_audio = true;
                    self.refresh_audio_devices(ctx);
                }
                if ui.button("Controller / MIDI").clicked() {
                    self.show_midi = true;
                }
            });
        });
        ui.columns(2, |columns| {
            let master = if self.audio.stream.is_some() {
                self.audio.master_name.as_str()
            } else {
                self.audio
                    .routing
                    .master
                    .device
                    .as_deref()
                    .unwrap_or("Windows default")
            };
            let master = format!(
                "MASTER · {} · Channels {}/{}",
                master,
                self.audio.routing.master.pair * 2 + 1,
                self.audio.routing.master.pair * 2 + 2
            );
            columns[0]
                .add(egui::Label::new(RichText::new(&master).size(11.0).color(MUTED)).truncate())
                .on_hover_text(master);
            let phones =
                self.audio
                    .routing
                    .phones
                    .as_ref()
                    .map_or("PHONES · OFF".into(), |target| {
                        let name = self
                            .audio
                            .phones_name
                            .as_deref()
                            .or(target.device.as_deref())
                            .unwrap_or("Windows default");
                        format!(
                            "PHONES · {name} · Channels {}/{}",
                            target.pair * 2 + 1,
                            target.pair * 2 + 2
                        )
                    });
            if columns[1]
                .add(
                    egui::Button::new(RichText::new(&phones).size(11.0).color(LIGHT_PURPLE))
                        .frame(false)
                        .truncate(),
                )
                .on_hover_text(format!("{phones}\nClick to choose the headphone output"))
                .clicked()
            {
                self.show_audio = true;
                self.refresh_audio_devices(ctx);
            }
        });
    }

    fn library_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("SESSION / SIDELIST").strong());
            ui.label(
                RichText::new(format!("{} tracks", self.library.len()))
                    .small()
                    .color(MUTED),
            );
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search loaded tracks …")
                .desired_width(ui.available_width()),
        );
        ui.separator();
        let query = self.search.to_lowercase();
        let mut request = None;
        egui::ScrollArea::vertical()
            .id_salt("session-list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.library.is_empty() {
                    ui.label(RichText::new("Drag tracks here from the browser.").color(MUTED));
                    ui.label(
                        RichText::new(
                            "Select music on the left and load it into Deck 1, 3, 4 or 2.",
                        )
                        .small()
                        .color(MUTED),
                    );
                }
                for track in self
                    .library
                    .iter()
                    .filter(|t| t.title.to_lowercase().contains(&query))
                {
                    ui.horizontal(|ui| {
                        for index in ORDER {
                            if ui
                                .small_button(
                                    RichText::new((index + 1).to_string()).color(deck_color(index)),
                                )
                                .clicked()
                            {
                                request = Some((index, track.path.clone()));
                            }
                        }
                        let response = ui
                            .add_sized(
                                [(ui.available_width() - 55.0).max(60.0), 23.0],
                                egui::Label::new(&track.title)
                                    .truncate()
                                    .sense(Sense::click_and_drag()),
                            )
                            .on_hover_text(format!(
                                "{}\n{}\n{}",
                                track.title,
                                track.summary.as_ref().map_or("—".into(), |s| clock(
                                    Duration::from_millis(s.duration_ms)
                                )),
                                track.path.display()
                            ));
                        dragdrop::source(&response, track.clone(), Origin::Session);
                        if response.double_clicked() {
                            request = Some((self.active, track.path.clone()));
                        }
                        ui.label(
                            RichText::new(
                                track
                                    .summary
                                    .as_ref()
                                    .and_then(|s| s.bpm())
                                    .map_or("—".into(), |b| format!("{b:.1}")),
                            )
                            .small()
                            .color(MUTED),
                        );
                    });
                    ui.separator();
                }
            });
        if let Some((index, path)) = request {
            self.decks[index].load(path, ctx.clone());
        }
    }
}

impl RustDj {
    fn mix_gains(&self) -> crate::dsp::BusGains<COUNT> {
        bus_gains_with_crossfader(
            self.decks.each_ref().map(|d| d.volume),
            self.decks.each_ref().map(|d| d.pfl),
            self.crossfader.position,
            self.master,
            self.phones,
            self.mixing,
            self.crossfader.enabled,
        )
    }

    fn sync(&mut self, follower: usize) {
        if self.synced[follower].is_some() {
            self.synced[follower] = None;
            return;
        }
        let loaded =
            std::array::from_fn(|i| self.decks[i].track.is_some() && self.synced[i].is_none());
        let playing = self.decks.each_ref().map(|d| d.playing());
        let Some(master) =
            crate::decks::sync_reference(follower, loaded, playing, self.mix_gains().master)
        else {
            self.notice = Some("Load another track to use Sync.".into());
            return;
        };
        self.synced[follower] = Some(master);
        match self.sync_plan(follower) {
            Ok(plan) => {
                self.decks[follower].speed = plan.speed;
                self.decks[follower].seek(plan.position, &self.audio);
                self.notice = Some(format!(
                    "Deck {} follows Deck {}. Tempo lock enabled.",
                    follower + 1,
                    master + 1
                ));
            }
            Err(error) => {
                self.synced[follower] = None;
                self.notice = Some(error);
            }
        }
    }

    fn sync_plan(&self, follower: usize) -> Result<crate::beat::SyncPlan, String> {
        let master = self.synced[follower].ok_or("No Sync reference selected.")?;
        let follower_grid = self.decks[follower]
            .grid
            .ok_or("The deck to synchronize has no BPM value.")?;
        let master_grid = self.decks[master]
            .grid
            .ok_or("The reference deck has no BPM value.")?;
        if self.decks[follower].track.is_none() || self.decks[master].track.is_none() {
            return Err("Load a track into both decks to use Sync.".into());
        }
        sync_plan(
            follower_grid,
            self.decks[follower].position(),
            master_grid,
            self.decks[master].position(),
            self.decks[master].speed,
        )
    }

    fn align_synced(&mut self, deck: usize) {
        if self.synced[deck].is_some()
            && let Ok(plan) = self.sync_plan(deck)
        {
            self.decks[deck].speed = plan.speed;
            self.decks[deck].seek(plan.position, &self.audio);
        }
    }

    fn toggle_deck(&mut self, deck: usize) {
        self.decks[deck].toggle(&self.audio);
        if self.decks[deck].playing() {
            self.align_synced(deck);
        }
    }

    fn tap_bpm(&mut self, deck: usize) {
        let now = Instant::now();
        let taps = &mut self.taps[deck];
        if taps
            .last()
            .is_some_and(|last| now.duration_since(*last).as_secs_f32() > 2.0)
        {
            taps.clear();
        }
        taps.push(now);
        if taps.len() > 8 {
            taps.remove(0);
        }
        if taps.len() >= 3 {
            let interval = now.duration_since(taps[0]).as_secs_f32() / (taps.len() - 1) as f32;
            let bpm = (60.0 / interval).clamp(40.0, 250.0);
            self.decks[deck].grid = Some(BeatGrid {
                bpm,
                offset: self.decks[deck]
                    .position()
                    .as_secs_f64()
                    .rem_euclid(60.0 / bpm as f64),
                confidence: 0.0,
            });
        }
    }

    fn midi_actions(&mut self, ctx: &egui::Context) {
        for (action, value) in self.midi.poll() {
            // Ignore out-of-range deck IDs in user-edited mapping files.
            let deck = match action {
                Action::SelectDeck(d)
                | Action::Play(d)
                | Action::Cue(d)
                | Action::SetCue(d)
                | Action::Stop(d)
                | Action::Sync(d)
                | Action::Volume(d)
                | Action::Tempo(d)
                | Action::Jog(d)
                | Action::Load(d)
                | Action::Pfl(d)
                | Action::EqLow(d)
                | Action::EqMid(d)
                | Action::EqHigh(d)
                | Action::Gain(d)
                | Action::Filter(d)
                | Action::FxLevel(d)
                | Action::FxToggle(d, _)
                | Action::Hotcue(d, _)
                | Action::ClearHotcue(d, _)
                | Action::LoopPreset(d, _)
                | Action::LoopPad(d, _)
                | Action::LoopToggle(d)
                | Action::LoopIn(d)
                | Action::LoopOut(d)
                | Action::LoopExit(d)
                | Action::Reloop(d)
                | Action::LoopHalf(d)
                | Action::LoopDouble(d) => Some(d),
                _ => None,
            };
            if deck.is_some_and(|d| d >= COUNT) {
                continue;
            }
            match action {
                Action::SelectDeck(d) => self.active = d,
                Action::DeckLayer(_) => {}
                Action::Play(d) => self.toggle_deck(d),
                Action::Cue(d) => self.decks[d].cue_button(value > 0.0, &self.audio),
                Action::SetCue(d) => self.decks[d].cue = self.decks[d].position(),
                Action::Stop(d) => self.decks[d].stop(&self.audio),
                Action::Sync(d) => self.sync(d),
                Action::Volume(d) => self.decks[d].volume = value.clamp(0.0, 1.0),
                Action::Tempo(d) => {
                    self.decks[d].speed = 1.0 + (value - 0.5) * 0.32;
                    if self.synced[d].is_some() {
                        self.synced[d] = None;
                    }
                }
                Action::Jog(d) => {
                    if self.synced[d].is_some() {
                        self.synced[d] = None;
                    }
                    let position =
                        (self.decks[d].position().as_secs_f64() + value as f64 * 0.015).max(0.0);
                    self.decks[d].seek(Duration::from_secs_f64(position), &self.audio);
                }
                Action::Load(d) => {
                    if let Some(path) = self.browser.selected_path() {
                        self.decks[d].load(path, ctx.clone());
                    }
                }
                Action::Crossfader | Action::CrossfaderToggle => {
                    self.crossfader.midi(action, value)
                }
                Action::Master => self.master = value.clamp(0.0, 1.0),
                Action::PhonesLevel => self.phones = value.clamp(0.0, 1.0),
                Action::PhonesMix => self.mixing = value.clamp(0.0, 1.0),
                Action::Pfl(d) => self.decks[d].pfl = !self.decks[d].pfl,
                Action::EqLow(d) => self.decks[d].eq[0] = knob_db(value),
                Action::EqMid(d) => self.decks[d].eq[1] = knob_db(value),
                Action::EqHigh(d) => self.decks[d].eq[2] = knob_db(value),
                Action::Gain(d) => self.decks[d].gain = knob_db(value),
                Action::Filter(d) => {
                    let v = value * 2.0 - 1.0;
                    self.decks[d].filter = if v.abs() < 0.015 { 0.0 } else { v };
                }
                Action::FxLevel(d) => self.decks[d].fx_level = value.clamp(0.0, 1.0),
                Action::FxToggle(d, slot) => {
                    if let Some(on) = self.decks[d].fx_enabled.get_mut(slot) {
                        *on = !*on;
                    }
                }
                Action::Hotcue(d, slot) => {
                    self.synced[d] = None;
                    self.decks[d].hotcue(slot, &self.audio);
                }
                Action::ClearHotcue(d, slot) => self.decks[d].clear_hotcue(slot),
                Action::LoopPreset(d, beats) => {
                    self.synced[d] = None;
                    self.decks[d].beat_loop(beats as f32, &self.audio);
                }
                Action::LoopPad(d, slot) => {
                    if let Some((beats, _)) = LOOP_PRESETS.get(slot) {
                        self.synced[d] = None;
                        self.decks[d].beat_loop(*beats, &self.audio);
                    }
                }
                Action::LoopToggle(d) => {
                    if self.decks[d].loop_active || self.decks[d].loop_pending() {
                        self.decks[d].exit_loop();
                    } else {
                        let beats = self.decks[d].loop_beats;
                        self.decks[d].beat_loop(beats, &self.audio);
                    }
                }
                Action::LoopIn(d) => self.decks[d].set_loop_in(),
                Action::LoopOut(d) => {
                    self.synced[d] = None;
                    self.decks[d].set_loop_out(&self.audio);
                }
                Action::LoopExit(d) => self.decks[d].exit_loop(),
                Action::Reloop(d) => {
                    self.synced[d] = None;
                    self.decks[d].reloop(&self.audio);
                }
                Action::LoopHalf(d) => self.decks[d].resize_loop(0.5, &self.audio),
                Action::LoopDouble(d) => self.decks[d].resize_loop(2.0, &self.audio),
                Action::Browse => {
                    self.browser.step_selection(value.clamp(-8.0, 8.0) as i32);
                }
                Action::Open => {
                    if let Some(BrowserEvent::Load(d, path)) =
                        self.browser.activate(self.active, ctx)
                    {
                        self.decks[d].load(path, ctx.clone());
                    }
                }
            }
        }
    }

    fn controller_ui(&mut self, ctx: &egui::Context) {
        egui::Window::new("Controller / MIDI").open(&mut self.show_midi).default_width(650.0).resizable(true).show(ctx, |ui| {
            ui.label("The DJ-202 connects automatically on startup and when plugged in.");
            if ui.checkbox(&mut self.midi.prefs.auto_connect, "Connect automatically and reconnect after USB disconnection").changed() { self.midi.dirty = true; }
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Preferred: {}", self.midi.prefs.preferred_port.as_deref().unwrap_or("Detect DJ-202 automatically"))).small());
                if ui.small_button("Select DJ-202 automatically").clicked() { self.midi.prefs.preferred_port = None; self.midi.prefs.auto_connect = true; self.midi.dirty = true; self.midi.refresh(); }
            });
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("midi-port").selected_text(self.midi.ports.get(self.midi.selected_port).map_or("No MIDI ports", String::as_str)).width(280.0).show_ui(ui, |ui| {
                    for (index, name) in self.midi.ports.iter().enumerate() { ui.selectable_value(&mut self.midi.selected_port, index, name); }
                });
                if ui.button("Refresh").clicked() { self.midi.refresh(); }
                if self.midi.connected.is_some() {
                    if ui.button("Disconnect").clicked() { self.midi.disconnect(); }
                } else if ui.add_enabled(!self.midi.ports.is_empty(), egui::Button::new("Connect")).clicked() { self.midi.connect(ctx); }
            });
            ui.label(RichText::new(self.midi.connected.as_deref().unwrap_or("No controller connected")).color(PURPLE));
            if let Some(error) = &self.midi.error { ui.colored_label(WARNING, error); }
            ui.horizontal(|ui| {
                if ui.button("Load DJ-202 profile").clicked() { self.midi.bindings = midi::dj202_profile(); self.midi.dirty = true; }
                ui.checkbox(&mut self.midi.dj202_leds, "DJ-202 transport/pad LEDs");
            });
            ui.label(RichText::new("Profile: Decks 1–4, mixer, filter/FX, transport, browser and all eight hotcue/loop pads. DECK 3/4 switches the controller banks.").small().color(MUTED));
            ui.label("Press HOT CUE or LOOP on the DJ-202, then use pads 1–8.");
            ui.label(RichText::new("HOT CUE: empty = set, assigned = jump/play; SHIFT+pad = clear. LOOP: ¼ · ½ · 1 · 2 · 4 · 8 · 16 · 32 beats; press again to exit. PARAM −/+ halves/doubles the loop.").small());
            for (deck, mode) in self.midi.pad_modes.iter().enumerate() {
                ui.label(RichText::new(format!("Deck {} · Pads: {}", deck + 1, mode.map_or("Select HOT CUE/LOOP on the controller", midi::PadMode::label))).small().color(PURPLE));
            }
            ui.separator();
            ui.label(RichText::new("MIDI-LEARN").strong());
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("midi-action").selected_text(self.midi.learn_action.label()).show_ui(ui, |ui| {
                    for action in Action::all() { ui.selectable_value(&mut self.midi.learn_action, action, action.label()); }
                });
                egui::ComboBox::from_id_salt("midi-mode").selected_text(match self.midi.learn_mode { Mode::Relative64 => "Encoder: center 64", Mode::RelativeTwos => "Encoder: 1 / 127", _ => "Button / fader (7-bit)" }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.midi.learn_mode, Mode::Absolute7, "Button / fader (7-bit)");
                    ui.selectable_value(&mut self.midi.learn_mode, Mode::Relative64, "Encoder: center 64");
                    ui.selectable_value(&mut self.midi.learn_mode, Mode::RelativeTwos, "Encoder: 1 / 127");
                });
                if self.midi.learning.is_some() {
                    if ui.button("Cancel").clicked() { self.midi.learning = None; }
                } else if ui.add_enabled(self.midi.connected.is_some(), egui::Button::new("Learn")).clicked() { self.midi.learning = Some(self.midi.learn_action); }
            });
            if let Some(action) = self.midi.learning { ui.colored_label(PURPLE, format!("Move or press a control for {} …", action.label())); }
            if let Some(bytes) = self.midi.last_message { ui.monospace(format!("Last MIDI message: {:02X} {:02X} {:02X}", bytes[0], bytes[1], bytes[2])); }
            if let Some(action) = self.midi.last_action { ui.label(format!("Last controller action: {}", action.label())); }
            ui.label(RichText::new("Custom mappings and browser favorites are saved locally.").small().color(MUTED));
            let mut remove = None;
            egui::ScrollArea::vertical().id_salt("bindings").max_height(260.0).show(ui, |ui| {
                for (index, binding) in self.midi.bindings.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("x").clicked() { remove = Some(index); }
                        ui.monospace(format!("{:02X} {:02X}", binding.status, binding.number));
                        ui.label(binding.action.label());
                        ui.label(RichText::new(format!("{:?}", binding.mode)).small().color(MUTED));
                    });
                }
            });
            if let Some(index) = remove { self.midi.bindings.remove(index); self.midi.dirty = true; }
        });
    }
}

impl RustDj {
    fn equalizer_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.show_equalizer;
        let before = self.master_eq;
        egui::Window::new("10-Band Master Equalizer").open(&mut open)
            .default_pos(Pos2::new(200.0, 100.0)).default_width(740.0).resizable(true).vscroll(true).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.master_eq.enabled, "Enable equalizer");
                    if ui.button("Flat / Reset").clicked() { self.master_eq.preamp = 0.0; self.master_eq.bands = [0.0; 10]; }
                });
                ui.label("Applies to the master, SHOUTcast stream and MP3 recording. Settings are saved automatically.");
                ui.horizontal(|ui| { ui.label("Preamp"); ui.add(egui::Slider::new(&mut self.master_eq.preamp, -12.0..=12.0).suffix(" dB")); });
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for (band, label) in equalizer::LABELS.into_iter().enumerate() {
                            ui.allocate_ui_with_layout(Vec2::new(58.0, 230.0), egui::Layout::top_down(egui::Align::Center), |ui| {
                                ui.label(RichText::new(format!("{label} Hz")).color(if band < 5 { PURPLE } else { LIGHT_PURPLE }));
                                ui.spacing_mut().slider_width = 170.0;
                                ui.add(egui::Slider::new(&mut self.master_eq.bands[band], -12.0..=12.0).vertical().show_value(false));
                                ui.add(egui::DragValue::new(&mut self.master_eq.bands[band]).range(-12.0..=12.0).speed(0.1).fixed_decimals(1).suffix(" dB"));
                            });
                        }
                    });
                });
            });
        self.show_equalizer = open;
        if before != self.master_eq {
            self.audio.master_eq.set(self.master_eq);
            self.audio_dirty = true;
        }
    }

    fn streamer_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.show_streamer;
        let before = self.stream_settings.clone();
        let stream_busy = self.broadcaster.stream_busy();
        let recording_busy = self.broadcaster.recording_busy();
        let master_ready = self.audio.stream.is_some();
        let mut password_changed = false;
        egui::Window::new("SHOUTcast / MP3 Recording")
            .open(&mut open).default_width(660.0).default_height(720.0)
            .default_pos(Pos2::new(200.0, 64.0))
            .max_height((ctx.content_rect().height() - 100.0).max(320.0))
            .resizable(true).vscroll(true).show(ctx, |ui| {
                ui.label(RichText::new("MASTER → STREAM / MP3").strong().color(PURPLE));
                ui.label("Only the master is streamed and recorded, including master level, EQ, filters and effects.");
                ui.label(RichText::new(format!("Source: {} · Stereo · MP3 44.1 kHz", self.audio.master_name)).small().color(MUTED));
                ui.separator();
                ui.add_enabled_ui(!stream_busy, |ui| {
                    ui.spacing_mut().text_edit_width = 360.0;
                    egui::Grid::new("shoutcast-config").num_columns(2).spacing([14.0, 7.0]).show(ui, |ui| {
                        ui.label("Server / IP"); ui.text_edit_singleline(&mut self.stream_settings.host).on_hover_text("Hostname or IP address without http:// or a path"); ui.end_row();
                        ui.label("Source port"); ui.add(egui::DragValue::new(&mut self.stream_settings.source_port).range(1..=u16::MAX)); ui.end_row();
                        ui.label("Metadata/admin port"); ui.add(egui::DragValue::new(&mut self.stream_settings.admin_port).range(1..=u16::MAX)); ui.end_row();
                        ui.label("Stream ID (DNAS 2)"); ui.add(egui::DragValue::new(&mut self.stream_settings.stream_id).range(1..=u32::MAX)); ui.end_row();
                        ui.label("Source password"); password_changed = ui.add(egui::TextEdit::singleline(&mut self.stream_password).password(true)).on_hover_text("Saved using Windows encryption for your user account and restored on the next startup.").changed(); ui.end_row();
                        ui.label("Station name"); ui.text_edit_singleline(&mut self.stream_settings.station); ui.end_row();
                        ui.label("Genre (optional)"); ui.text_edit_singleline(&mut self.stream_settings.genre); ui.end_row();
                        ui.label("Station URL (optional)"); ui.text_edit_singleline(&mut self.stream_settings.url); ui.end_row();
                    });
                });
                if let Some(error) = &self.stream_password_error { ui.colored_label(WARNING, error); }
                ui.horizontal(|ui| {
                    ui.label("MP3 bitrate for new sessions");
                    egui::ComboBox::from_id_salt("stream-bitrate").selected_text(format!("{} kbit/s", self.stream_settings.bitrate)).show_ui(ui, |ui| {
                        for bitrate in [128, 192, 256, 320] { ui.selectable_value(&mut self.stream_settings.bitrate, bitrate, format!("{bitrate} kbit/s")); }
                    });
                });
                ui.label(RichText::new("SHOUTcast 1-compatible source connection, including DNAS 2 with a stream ID. Common ports: source 8001, metadata 8000; use your provider's settings.").small().color(MUTED));
                ui.separator();
                ui.checkbox(&mut self.stream_settings.send_tags, "Send track titles to the server");
                ui.add_enabled_ui(self.stream_settings.send_tags, |ui| {
                    ui.checkbox(&mut self.stream_settings.automatic_title, "Automatic: title of the playing deck with the highest master contribution");
                    if !self.stream_settings.automatic_title {
                        ui.horizontal(|ui| { ui.label("Manual title"); ui.text_edit_singleline(&mut self.stream_settings.manual_title); });
                    }
                });
                ui.horizontal(|ui| {
                    if stream_busy {
                        if ui.button("Stop stream").clicked() { self.broadcaster.stop_stream(); }
                    } else if ui.add_enabled(master_ready, egui::Button::new("Start stream").fill(PURPLE.gamma_multiply(0.5))).clicked()
                        && let Err(error) = self.broadcaster.start_stream(self.audio.capture.clone(), self.stream_settings.clone(), self.stream_password.clone())
                    { self.broadcaster.stream_status.phase = Phase::Error; self.broadcaster.stream_status.message = error; }
                    let status = &self.broadcaster.stream_status;
                    ui.label(RichText::new(format!("STREAM · {} · {:.2} MB", status.label(), status.bytes as f64 / 1_000_000.0)).color(if status.phase == Phase::Error { WARNING } else { PURPLE }));
                });
                if !self.broadcaster.stream_status.message.is_empty() { ui.label(&self.broadcaster.stream_status.message); }
                if self.stream_settings.send_tags && let Some(error) = &self.broadcaster.stream_status.metadata_error { ui.colored_label(WARNING, error); }
                ui.separator();
                ui.label(RichText::new("MP3 RECORDING").strong().color(LIGHT_PURPLE));
                ui.add_enabled_ui(!recording_busy, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Folder");
                        let mut directory = self.stream_settings.recording_dir.to_string_lossy().into_owned();
                        if ui.text_edit_singleline(&mut directory).changed() { self.stream_settings.recording_dir = directory.into(); }
                        if ui.button("Browse …").clicked()
                            && let Some(directory) = rfd::FileDialog::new().set_title("MP3 recording folder").pick_folder()
                        { self.stream_settings.recording_dir = directory; }
                    });
                });
                ui.horizontal(|ui| {
                    if recording_busy {
                        if ui.button("Stop recording").clicked() { self.broadcaster.stop_recording(); }
                    } else if ui.add_enabled(master_ready, egui::Button::new("Start MP3 recording")).clicked()
                        && let Err(error) = self.broadcaster.start_recording(self.audio.capture.clone(), &self.stream_settings)
                    { self.broadcaster.recording_status.phase = Phase::Error; self.broadcaster.recording_status.message = error; }
                    let status = &self.broadcaster.recording_status;
                    ui.label(RichText::new(format!("REC · {} · {} · {:.2} MB", status.label(), clock(Duration::from_secs_f64(status.seconds)), status.bytes as f64 / 1_000_000.0)).color(if status.phase == Phase::Error { WARNING } else { LIGHT_PURPLE }));
                });
                if !self.broadcaster.recording_status.message.is_empty() { ui.label(&self.broadcaster.recording_status.message); }
                if let Some(path) = &self.broadcaster.recording_status.path { ui.label(RichText::new(path.display().to_string()).small()); }
                ui.label(RichText::new("Streaming and recording can run independently. New MP3 files have unique names. Stop finalizes the recording. Stop both sessions before changing audio outputs.").small().color(MUTED));
                if !master_ready { ui.colored_label(WARNING, "Select an available master output under Audio / Outputs first."); }
            });
        self.show_streamer = open;
        if password_changed {
            match crate::credentials::protect(&self.stream_password) {
                Ok(blob) => {
                    self.stream_settings.protected_password = blob;
                    self.stream_password_error = None;
                }
                Err(error) => self.stream_password_error = Some(error),
            }
        }
        if self.stream_settings != before {
            self.audio_dirty = true;
        }
    }

    fn refresh_audio_devices(&mut self, ctx: &egui::Context) {
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(output::devices());
            ctx.request_repaint();
        });
        self.device_scan = Some(receiver);
    }

    fn apply_audio(&mut self, routing: Routing) {
        if self.broadcaster.busy() {
            self.notice =
                Some("Stop streaming and recording before changing audio outputs.".into());
            return;
        }
        match Audio::open(routing) {
            Ok(audio) => {
                audio.master_eq.set(self.master_eq);
                let mut errors = Vec::new();
                for deck in &mut self.decks {
                    if let Err(error) = deck.rebind(&audio) {
                        deck.disconnect();
                        deck.error = Some(error.clone());
                        errors.push(error);
                    }
                }
                self.audio = audio;
                self.routing_edit = self.audio.routing.clone();
                self.audio_dirty = true;
                self.notice = Some(if errors.is_empty() {
                    "Audio outputs applied; playback positions preserved.".into()
                } else {
                    errors.join("; ")
                });
            }
            Err(error) => self.notice = Some(format!("Outputs were not changed: {error}")),
        }
    }

    fn audio_ui(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.device_scan {
            match receiver.try_recv() {
                Ok(result) => {
                    self.device_scan = None;
                    match result {
                        Ok(devices) => self.audio_devices = devices,
                        Err(error) => self.notice = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.device_scan = None;
                    self.notice = Some("Could not list audio devices.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut open = self.show_audio;
        let (mut apply, mut refresh) = (false, false);
        egui::Window::new("Audio / Outputs").open(&mut open).default_width(680.0).resizable(true).show(ctx, |ui| {
            ui.label("Choose separate outputs for the master speakers and headphone monitoring.");
            ui.horizontal(|ui| {
                if ui.button("Refresh devices").clicked() { refresh = true; }
                if self.device_scan.is_some() { ui.spinner(); ui.label("Searching for devices …"); }
            });
            ui.separator();
            ui.label(RichText::new("MASTER OUTPUT").strong().color(PURPLE));
            target_ui(ui, "master-device", &mut self.routing_edit.master, &self.audio_devices);
            ui.add_space(8.0);
            ui.label(RichText::new("PHONES · MONITORING OUTPUT").strong().color(LIGHT_PURPLE));
            let choice_width = ui.available_width().min(580.0);
            refresh |= phones_device_ui(ui, "phones-device", &mut self.routing_edit.phones, &self.audio_devices, choice_width);
            if let Some(target) = &mut self.routing_edit.phones {
                channel_pair_ui(ui, "phones-device", target, &self.audio_devices, false);
            }
            ui.add_space(8.0);
            ui.label("Two devices: channels 1/2 on each. One multichannel device: e.g. master 1/2 and phones 3/4. Overlapping channel pairs are blocked.");
            ui.label(RichText::new("Bluetooth and different sound cards may have different latencies. Missing devices are not replaced with another output.").small().color(MUTED));
            ui.separator();
            ui.label(format!("Active master: {} — Channels {}/{}", self.audio.master_name, self.audio.routing.master.pair * 2 + 1, self.audio.routing.master.pair * 2 + 2));
            ui.label(format!("Active phones: {}", self.audio.phones_name.as_deref().unwrap_or("Off")));
            if let Some(error) = &self.audio.error { ui.colored_label(WARNING, error); }
            ui.horizontal(|ui| {
                if ui.button("Apply").clicked() { apply = true; }
                if ui.button("Reset selection").clicked() { self.routing_edit = self.audio.routing.clone(); }
            });
            if let Some(notice) = &self.notice { ui.label(RichText::new(notice).small().color(MUTED)); }
            ui.separator();
            ui.label("Monitoring: enable the deck's PHONES CUE, turn MIXING toward Cue and raise HEADPHONES level. Play the track or hold transport CUE to preview it.");
        });
        self.show_audio = open;
        if refresh {
            self.refresh_audio_devices(ctx);
        }
        if apply {
            self.apply_audio(self.routing_edit.clone());
        }
    }
}

fn target_ui(ui: &mut egui::Ui, id: &str, target: &mut Target, devices: &[DeviceInfo]) {
    let old = target.device.clone();
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width().min(580.0))
        .wrap_mode(egui::TextWrapMode::Truncate)
        .selected_text(target.device.as_deref().unwrap_or("Windows system default"))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut target.device, None, "Windows system default");
            for device in devices {
                ui.selectable_value(
                    &mut target.device,
                    Some(device.name.clone()),
                    format!("{} (up to {} channels)", device.name, device.channels),
                );
            }
        });
    if old != target.device {
        target.pair = 0;
    }
    channel_pair_ui(ui, id, target, devices, false);
}

/// None = off; Some(None) = system default; Some(Some(name)) = explicit device.
/// Keep a selected device's channel pair until the device itself changes.
fn phones_device_ui(
    ui: &mut egui::Ui,
    id: &str,
    target: &mut Option<Target>,
    devices: &[DeviceInfo],
    width: f32,
) -> bool {
    let mut choice = target.as_ref().map(|target| target.device.clone());
    let old = choice.clone();
    let label = match &choice {
        None => "Off – no headphone output",
        Some(None) => "Windows system default",
        Some(Some(name)) => name,
    }
    .to_owned();
    let mut refresh = false;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .wrap_mode(egui::TextWrapMode::Truncate)
        .selected_text(&label)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut choice, None, "Off – no headphone output");
            ui.selectable_value(&mut choice, Some(None), "Windows system default");
            for device in devices {
                ui.selectable_value(
                    &mut choice,
                    Some(Some(device.name.clone())),
                    format!("{} (up to {} channels)", device.name, device.channels),
                );
            }
            ui.separator();
            if ui.button("Refresh devices …").clicked() {
                refresh = true;
                ui.close();
            }
        })
        .response
        .on_hover_text(label);
    if choice != old {
        *target = choice.map(|device| Target { device, pair: 0 });
    }
    refresh
}

fn channel_pair_ui(
    ui: &mut egui::Ui,
    id: &str,
    target: &mut Target,
    devices: &[DeviceInfo],
    compact: bool,
) {
    let channels = target
        .device
        .as_ref()
        .and_then(|name| devices.iter().find(|d| &d.name == name))
        .map_or(8, |d| d.channels);
    egui::ComboBox::from_id_salt((id, "channels"))
        .width(if compact { 52.0 } else { 140.0 })
        .selected_text(if compact {
            format!("{}/{}", target.pair * 2 + 1, target.pair * 2 + 2)
        } else {
            format!("Channels {} / {}", target.pair * 2 + 1, target.pair * 2 + 2)
        })
        .show_ui(ui, |ui| {
            for pair in 0..(channels / 2).clamp(1, 16) {
                ui.selectable_value(
                    &mut target.pair,
                    pair,
                    format!("Channels {} / {}", pair * 2 + 1, pair * 2 + 2),
                );
            }
        });
}

impl eframe::App for RustDj {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let levels_before = (
            self.master,
            self.phones,
            self.mixing,
            self.crossfader.enabled,
        );
        let (master_peaks, phones_peaks) = self.audio.levels();
        for (bus, peaks) in [master_peaks, phones_peaks].into_iter().enumerate() {
            for (channel, value) in peaks.into_iter().enumerate() {
                self.bus_levels[bus][channel] =
                    value.max(self.bus_levels[bus][channel] * 0.86).min(8.0);
            }
        }
        if let Some(error) = self.hotcue_store.error() {
            self.notice = Some(error);
        }
        self.broadcaster.poll();
        self.browser.poll();
        self.midi.tick(ctx);
        for (index, deck) in self.decks.iter_mut().enumerate() {
            if let Some(track) = deck.poll(&self.audio) {
                self.browser.on_track(&track);
                self.playlist.on_track(&track);
                self.synced[index] = None;
                for follower in &mut self.synced {
                    if *follower == Some(index) {
                        *follower = None;
                    }
                }
                self.taps[index].clear();
                upsert_session(&mut self.library, Item::from_track(&track));
            }
        }
        self.midi_actions(ctx);
        self.shortcuts(ctx);
        egui::TopBottomPanel::top("app-header")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(6.0))
            .show(ctx, |ui| {
                self.header(ui, ctx);
            });
        egui::TopBottomPanel::bottom("footer").frame(egui::Frame::new().fill(PANEL).inner_margin(4.0)).show(ctx, |ui| {
            if let Some(error) = &self.audio.error {
                ui.add(egui::Label::new(RichText::new(error).small().color(WARNING)).truncate()).on_hover_text(error);
            } else if let Some(notice) = &self.notice {
                ui.add(egui::Label::new(RichText::new(notice).small().color(MUTED)).truncate()).on_hover_text(notice);
            } else {
                ui.label(RichText::new("1 / 2 / 3 / 4  Select deck     SPACE  Play/Pause     S  Sync     C  Cue     ESC  Stop     Left / Right  Crossfader").size(10.0).color(MUTED));
            }
        });
        egui::TopBottomPanel::top("waveforms")
            .resizable(true)
            .default_height(160.0)
            .height_range(120.0..=260.0)
            .frame(egui::Frame::new().fill(BG).inner_margin(5.0))
            .show(ctx, |ui| {
                self.waveforms_ui(ui);
            });
        let mut bounds = [Rect::NOTHING; COUNT];
        let mut drops = Zones::default();
        let mut responses = [DeckResponse::default(); COUNT];
        egui::TopBottomPanel::top("console")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(5.0))
            .show(ctx, |ui| {
                let (mixer_width, deck_width) = console_dimensions(ui.available_width());
                ui.horizontal_top(|ui| {
                    for (slot, index) in ORDER.into_iter().enumerate() {
                        if slot == 2 {
                            ui.allocate_ui_with_layout(
                                Vec2::new(mixer_width, 360.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| self.mixer_ui(ui, mixer_width),
                            );
                        }
                        let card = ui.allocate_ui_with_layout(
                            Vec2::new(deck_width, 320.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                responses[index] = deck_ui(
                                    ui,
                                    &mut self.decks[index],
                                    &self.audio,
                                    index,
                                    self.active == index,
                                    self.synced[index].is_some(),
                                    deck_width,
                                )
                                .inner;
                            },
                        );
                        bounds[index] = card.response.rect;
                        drops.add(DropTarget::Deck(index), card.response.rect, ui.layer_id());
                    }
                });
            });
        for index in 0..COUNT {
            if ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| bounds[index].contains(p))
            }) {
                self.active = index;
            }
            let response = responses[index];
            if response.open {
                self.pick(index, ctx);
            }
            if response.tempo_changed || response.performance_changed {
                self.synced[index] = None;
            }
            if response.sync {
                self.sync(index);
            }
            if response.tap {
                self.tap_bpm(index);
            }
            if response.toggled && self.decks[index].playing() {
                self.align_synced(index);
            }
        }
        egui::SidePanel::right("session")
            .resizable(true)
            .default_width(330.0)
            .width_range(240.0..=550.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(6.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let session = ui.selectable_value(&mut self.playlist_tab, false, "SESSION");
                    drops.add(DropTarget::Session, session.rect, ui.layer_id());
                    let playlist = ui.selectable_value(&mut self.playlist_tab, true, "PLAYLIST");
                    drops.add(DropTarget::Playlist(None), playlist.rect, ui.layer_id());
                    if playlist.clicked() {
                        self.playlist.refresh(ctx);
                    }
                });
                ui.separator();
                drops.add(
                    if self.playlist_tab {
                        DropTarget::Playlist(None)
                    } else {
                        DropTarget::Session
                    },
                    ui.available_rect_before_wrap(),
                    ui.layer_id(),
                );
                if self.playlist_tab {
                    if let Some((index, path)) = self.playlist.ui(ui, ctx, self.active, &mut drops)
                    {
                        self.decks[index].load(path, ctx.clone());
                        self.active = index;
                    }
                } else {
                    self.library_ui(ui, ctx);
                }
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PANEL).inner_margin(6.0))
            .show(ctx, |ui| match self.browser.ui(ui, self.active, ctx) {
                Some(BrowserEvent::Load(index, path)) => {
                    self.decks[index].load(path, ctx.clone());
                    self.active = index;
                }
                Some(BrowserEvent::Playlist(path)) => {
                    self.playlist.add(vec![path], ctx);
                    self.playlist_tab = true;
                }
                None => {}
            });
        self.controller_ui(ctx);
        self.audio_ui(ctx);
        self.playlist.confirm_changes(ctx);
        if let Some((target, music)) = drops.finish(ctx) {
            match target {
                DropTarget::Deck(index) => {
                    self.decks[index].load(music.item.path, ctx.clone());
                    self.active = index;
                }
                DropTarget::Session => {
                    upsert_session(&mut self.library, music.item);
                    self.playlist_tab = false;
                }
                DropTarget::Playlist(index) => {
                    if let Origin::Playlist(source) = music.origin {
                        self.playlist.move_to(source, index);
                    } else {
                        self.playlist.insert(music.item, index, ctx);
                    }
                    self.playlist_tab = true;
                }
            }
        }
        let external_target = drops.target(ctx);
        let mut external_music = Vec::new();
        for file in ctx.input(|i| i.raw.dropped_files.clone()) {
            if let Some(path) = file.path {
                if path.is_dir() {
                    self.browser.navigate(path, ctx);
                } else if track::supported(&path) {
                    external_music.push(path);
                } else {
                    self.notice = Some("Drop a music file or folder here.".into());
                }
            }
        }
        if !external_music.is_empty() {
            match external_target {
                Some(DropTarget::Deck(index)) => {
                    for (offset, path) in external_music.into_iter().take(COUNT).enumerate() {
                        let slot = ORDER.iter().position(|d| *d == index).unwrap_or(0);
                        let target = ORDER[(slot + offset) % COUNT];
                        self.decks[target].load(path, ctx.clone());
                        self.active = target;
                    }
                }
                Some(DropTarget::Session) => {
                    for path in external_music {
                        upsert_session(&mut self.library, Item::new(path, None));
                    }
                    self.playlist_tab = false;
                }
                Some(DropTarget::Playlist(index)) => {
                    self.playlist.insert_many(
                        external_music
                            .into_iter()
                            .map(|p| Item::new(p, None))
                            .collect(),
                        index,
                        ctx,
                    );
                    self.playlist_tab = true;
                }
                None => {}
            }
        }
        for follower in 0..COUNT {
            if self.synced[follower].is_some() {
                match self.sync_plan(follower) {
                    Ok(plan) => self.decks[follower].speed = plan.speed,
                    Err(error) => {
                        self.synced[follower] = None;
                        self.notice = Some(error);
                    }
                }
            }
        }
        let gains = self.mix_gains();
        for (index, deck) in self.decks.iter_mut().enumerate() {
            deck.apply_buses(gains.master[index], gains.phones[index]);
            if let Some(snapshot) = deck.take_hotcue_changes() {
                self.hotcue_store.save(snapshot);
            }
        }
        let dominant = (0..COUNT)
            .filter(|i| self.decks[*i].playing() && gains.master[*i] > 0.0001)
            .max_by(|a, b| gains.master[*a].total_cmp(&gains.master[*b]));
        let title = if self.stream_settings.automatic_title {
            dominant
                .and_then(|i| self.decks[i].track.as_ref().map(|t| t.title.as_str()))
                .unwrap_or(&self.stream_settings.station)
        } else {
            &self.stream_settings.manual_title
        };
        self.broadcaster
            .set_metadata(self.stream_settings.send_tags, title);
        self.streamer_ui(ctx);
        self.equalizer_ui(ctx);
        self.midi.feedback(
            std::array::from_fn(|index| {
                let deck = &self.decks[index];
                midi::DeckLeds {
                    effects: deck.fx_enabled,
                    playing: deck.playing(),
                    pfl: deck.pfl,
                    hotcues: deck.hotcues.map(|cue| cue.is_some()),
                    loops: std::array::from_fn(|pad| {
                        deck.loop_active
                            && deck.loop_auto
                            && (deck.loop_beats - LOOP_PRESETS[pad].0).abs() < 0.001
                    }),
                }
            }),
            self.synced.map(|master| master.is_some()),
        );
        if self.browser.dirty
            || self.midi.dirty
            || self.audio_dirty
            || levels_before
                != (
                    self.master,
                    self.phones,
                    self.mixing,
                    self.crossfader.enabled,
                )
        {
            let settings = Settings {
                browser: self.browser.settings.clone(),
                bindings: Some(self.midi.bindings.clone()),
                routing: self.audio.routing.clone(),
                mix: MixSettings {
                    master: self.master,
                    phones: self.phones,
                    mixing: self.mixing,
                    crossfader_enabled: self.crossfader.enabled,
                },
                mapping_version: 6,
                midi: self.midi.prefs.clone(),
                streamer: self.stream_settings.clone(),
                master_eq: self.master_eq,
            };
            if let Err(error) = settings::save(&settings) {
                self.notice = Some(error);
            }
            self.browser.dirty = false;
            self.midi.dirty = false;
            self.audio_dirty = false;
        }
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(3)
        .inner_margin(8.0)
}

fn upsert_session(items: &mut Vec<Item>, item: Item) {
    if let Some(existing) = items.iter_mut().find(|i| i.path == item.path) {
        if item.summary.is_some() {
            *existing = item;
        }
    } else {
        items.push(item);
    }
}

fn clock(time: Duration) -> String {
    let secs = time.as_secs();
    format!(
        "{:02}:{:02}.{:01}",
        secs / 60,
        secs % 60,
        time.subsec_millis() / 100
    )
}

#[derive(Clone, Copy, Default)]
struct DeckResponse {
    performance_changed: bool,
    open: bool,
    sync: bool,
    tap: bool,
    toggled: bool,
    tempo_changed: bool,
}

fn deck_color(index: usize) -> Color32 {
    crate::decks::COLORS[index]
}

fn deck_ui(
    ui: &mut egui::Ui,
    deck: &mut Deck,
    audio: &Audio,
    index: usize,
    active: bool,
    synced: bool,
    width: f32,
) -> egui::InnerResponse<DeckResponse> {
    let color = deck_color(index);
    card()
        .stroke(Stroke::new(
            1.0_f32,
            if active {
                color
            } else {
                color.gamma_multiply(0.55)
            },
        ))
        .show(ui, |ui| {
            ui.visuals_mut().selection.bg_fill = color.gamma_multiply(0.35);
            ui.visuals_mut().selection.stroke = Stroke::new(1.0_f32, color);
            ui.set_width(width - 18.0);
            ui.spacing_mut().item_spacing = Vec2::new(3.0, 4.0);
            ui.spacing_mut().button_padding = Vec2::new(3.0, 3.0);
            let mut response = DeckResponse::default();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("DECK {}", index + 1))
                        .strong()
                        .color(color),
                );
                if deck.pfl {
                    ui.label(RichText::new("PHONES").size(9.0).color(color));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Open").clicked() {
                        response.open = true;
                    }
                });
            });
            let title = deck
                .track
                .as_ref()
                .map_or("Drop music here", |t| t.title.as_str());
            ui.add(egui::Label::new(RichText::new(title).strong()).truncate())
                .on_hover_text(title);
            let duration = deck.track.as_ref().map_or(Duration::ZERO, |t| t.duration);
            let position = deck.position();
            let fraction = if duration.is_zero() {
                0.0
            } else {
                position.as_secs_f32() / duration.as_secs_f32()
            };
            if let Some(fraction) = waveform_ui(
                ui,
                deck.track.as_ref().map(|t| t.detailed.as_ref()),
                duration.as_secs_f64(),
                fraction,
            ) {
                deck.seek(duration.mul_f32(fraction), audio);
                response.performance_changed = true;
            }
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(clock(position))
                        .monospace()
                        .size(15.0)
                        .color(color),
                );
                let bpm = deck.grid.map_or("Set BPM".into(), |g| {
                    format!("{:.1} BPM", g.bpm * deck.speed)
                });
                ui.menu_button(bpm, |ui| {
                    if let Some(grid) = &mut deck.grid {
                        ui.add(
                            egui::DragValue::new(&mut grid.bpm)
                                .range(40.0..=250.0)
                                .speed(0.1)
                                .suffix(" BPM"),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("/2").clicked() {
                                grid.bpm = (grid.bpm / 2.0).max(40.0);
                            }
                            if ui.button("x2").clicked() {
                                grid.bpm = (grid.bpm * 2.0).min(250.0);
                            }
                        });
                        if ui.button("Set beat here").clicked() {
                            grid.offset = position.as_secs_f64().rem_euclid(grid.period());
                            ui.close();
                        }
                    } else if ui.button("Set 120 BPM").clicked() {
                        deck.grid = Some(BeatGrid {
                            bpm: 120.0,
                            offset: 0.0,
                            confidence: 0.0,
                        });
                    }
                    if ui.button("Tap BPM").clicked() {
                        response.tap = true;
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().slider_width = (width - 93.0).max(90.0);
                let mut percent = (deck.speed - 1.0) * 100.0;
                if ui
                    .add_enabled(
                        !synced,
                        egui::Slider::new(&mut percent, -50.0..=100.0).show_value(false),
                    )
                    .changed()
                {
                    deck.speed = 1.0 + percent / 100.0;
                    response.tempo_changed = true;
                }
                if ui
                    .small_button(format!("{percent:+.1}%"))
                    .on_hover_text("Reset tempo")
                    .clicked()
                {
                    deck.speed = 1.0;
                    response.tempo_changed = true;
                }
            });
            ui.horizontal(|ui| {
                let available = deck.track.is_some() && audio.stream.is_some();
                let cue = ui
                    .add_enabled(available, egui::Button::new("CUE"))
                    .on_hover_text("Hold to preview. Right-click to set the cue point.");
                if cue.secondary_clicked() {
                    deck.cue = position;
                }
                let down = cue.is_pointer_button_down_on();
                if down != deck.ui_cue_down {
                    deck.ui_cue_down = down;
                    deck.cue_button(down, audio);
                }
                if ui
                    .add_enabled(
                        available,
                        egui::Button::new(if deck.playing() { "PAUSE" } else { "PLAY" })
                            .fill(color.gamma_multiply(0.45)),
                    )
                    .clicked()
                {
                    deck.toggle(audio);
                    response.toggled = true;
                }
                if ui
                    .add_enabled(available, egui::Button::new("SYNC").selected(synced))
                    .clicked()
                {
                    response.sync = true;
                }
                if ui
                    .add_enabled(available, egui::Button::new("STOP"))
                    .clicked()
                {
                    deck.stop(audio);
                }
            });
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(50.0, 62.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        fx_knob(ui, "FILTER", &mut deck.filter, -1.0, 1.0, 0.0, color);
                    },
                );
                ui.allocate_ui_with_layout(
                    Vec2::new(50.0, 62.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        fx_knob(ui, "FX LEVEL", &mut deck.fx_level, 0.0, 1.0, 0.25, color);
                    },
                );
                ui.vertical(|ui| {
                    ui.set_width(50.0);
                    for (slot, label) in ["FLG", "REV", "ECHO"].into_iter().enumerate() {
                        if ui
                            .add(
                                egui::Button::new(label)
                                    .small()
                                    .selected(deck.fx_enabled[slot]),
                            )
                            .clicked()
                        {
                            deck.fx_enabled[slot] = !deck.fx_enabled[slot];
                        }
                    }
                });
            });
            let pad_width = ((width - 18.0 - 21.0) / 8.0).max(17.0);
            ui.label(RichText::new("HOTCUES").size(9.0).color(MUTED));
            ui.horizontal(|ui| {
                for slot in 0..8 {
                    let saved = deck.hotcues[slot];
                    let button = ui.add_enabled(
                        deck.track.is_some(),
                        egui::Button::new((slot + 1).to_string())
                            .small()
                            .selected(saved.is_some())
                            .min_size(Vec2::new(pad_width, 20.0)),
                    );
                    if button.secondary_clicked()
                        || (button.clicked() && ui.input(|i| i.modifiers.shift))
                    {
                        deck.clear_hotcue(slot);
                    } else if button.clicked() {
                        deck.hotcue(slot, audio);
                        response.performance_changed = saved.is_some();
                    }
                    button.on_hover_text("Empty: set; assigned: jump. Shift/right-click: clear.");
                }
            });
            ui.label(RichText::new("LOOPS · BEATS").size(9.0).color(MUTED));
            ui.horizontal(|ui| {
                for (beats, label) in LOOP_PRESETS {
                    let selected = deck.loop_auto
                        && (deck.loop_active || deck.loop_pending())
                        && (deck.loop_beats - beats).abs() < 0.001;
                    if ui
                        .add_enabled(
                            deck.track.is_some() && audio.stream.is_some(),
                            egui::Button::new(label)
                                .small()
                                .selected(selected)
                                .min_size(Vec2::new(pad_width, 20.0)),
                        )
                        .on_hover_text("Press again to exit the loop")
                        .clicked()
                    {
                        deck.beat_loop(beats, audio);
                        response.performance_changed = true;
                    }
                }
            });
            if deck.loading() {
                ui.label(RichText::new("Analyzing …").small().color(color));
            } else if let Some(error) = &deck.error {
                ui.add(egui::Label::new(RichText::new(error).small().color(WARNING)).truncate())
                    .on_hover_text(error);
            } else {
                ui.label(
                    RichText::new(format!(
                        "−{} · {}",
                        clock(duration.saturating_sub(position)),
                        if deck.loop_active {
                            "LOOP"
                        } else if deck.playing() {
                            "PLAY"
                        } else {
                            "READY"
                        }
                    ))
                    .size(9.0)
                    .color(MUTED),
                );
            }
            response
        })
}

fn knob_ui(ui: &mut egui::Ui, label: &str, value: &mut f32, color: Color32) {
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.spacing_mut().button_padding = Vec2::new(4.0, 2.0);
        ui.label(RichText::new(label).size(9.0).color(MUTED));
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(38.0, 32.0), Sense::click_and_drag());
        if response.dragged() {
            *value = (*value - response.drag_delta().y * 0.2 + response.drag_delta().x * 0.1)
                .clamp(-24.0, 12.0);
        }
        if response.double_clicked() {
            *value = 0.0;
        }
        response.on_hover_text("Drag or enter a value; double-click to reset to 0 dB.");
        let center = rect.center();
        let angle = -std::f32::consts::FRAC_PI_2
            + if *value < 0.0 {
                *value / 24.0
            } else {
                *value / 12.0
            } * 2.3;
        ui.painter().circle_filled(center, 15.0, BG);
        ui.painter()
            .circle_stroke(center, 15.0, Stroke::new(1.0_f32, BORDER));
        ui.painter().line_segment(
            [
                center + Vec2::angled(angle) * 5.0,
                center + Vec2::angled(angle) * 12.0,
            ],
            Stroke::new(2.0_f32, color),
        );
        ui.add(
            egui::DragValue::new(value)
                .range(-24.0..=12.0)
                .speed(0.2)
                .fixed_decimals(1)
                .suffix(" dB"),
        );
    });
}

fn channel_strip(ui: &mut egui::Ui, deck: &mut Deck, color: Color32, index: usize) {
    ui.vertical_centered(|ui| {
        ui.visuals_mut().selection.bg_fill = color.gamma_multiply(0.35);
        ui.visuals_mut().selection.stroke = Stroke::new(1.0_f32, color);
        ui.spacing_mut().item_spacing = Vec2::new(3.0, 3.0);
        ui.label(RichText::new((index + 1).to_string()).strong().color(color));
        ui.columns(2, |columns| {
            for (band, label) in [(2, "HIGH"), (1, "MID"), (0, "LOW")] {
                knob_ui(&mut columns[0], label, &mut deck.eq[band], color);
            }
            knob_ui(&mut columns[1], "GAIN", &mut deck.gain, color);
            columns[1].vertical_centered(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(2.0, 3.0);
                ui.label(RichText::new("LEVEL").size(8.0).color(MUTED));
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().slider_width = 100.0;
                    ui.add_sized(
                        [18.0, 100.0],
                        egui::Slider::new(&mut deck.volume, 0.0..=1.0)
                            .vertical()
                            .show_value(false),
                    );
                    strip_meter_ui(ui, deck.channel_meter, color);
                });
                ui.label(
                    RichText::new(format!("{} dB", peak_number(deck.channel_meter)))
                        .size(8.0)
                        .color(color),
                );
                if ui
                    .add(egui::Button::new("CUE").small().selected(deck.pfl))
                    .on_hover_text("Headphone monitoring before the channel fader and crossfader")
                    .clicked()
                {
                    deck.pfl = !deck.pfl;
                }
                if ui.small_button("Reset").clicked() {
                    deck.eq = [0.0; 3];
                    deck.gain = 0.0;
                }
            });
        });
    });
}

fn strip_meter_ui(ui: &mut egui::Ui, level: f32, color: Color32) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(14.0, 100.0), Sense::hover());
    response.on_hover_text(format!("Channel level: {}", peak_text(level)));
    let db = peak_db(level);
    for segment in 0..30 {
        let threshold = -60.0 + segment as f32 / 29.0 * 66.0;
        let bottom = rect.bottom() - segment as f32 * rect.height() / 30.0;
        let tint = if db < threshold {
            BORDER.gamma_multiply(0.45)
        } else if threshold >= 0.0 {
            WARNING
        } else if threshold >= -6.0 {
            Color32::from_rgb(247, 202, 85)
        } else {
            color
        };
        ui.painter().rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.left(), bottom - rect.height() / 30.0 + 1.0),
                Vec2::new(rect.width(), rect.height() / 30.0 - 1.0),
            ),
            1.0,
            tint,
        );
    }
}

const BAND_COLOURS: [Color32; 3] = [PURPLE, Color32::from_rgb(75, 212, 144), LIGHT_PURPLE];

fn spectral_column(
    painter: &egui::Painter,
    rect: Rect,
    x: f32,
    frame: crate::waveform::Frame,
    dim: bool,
) {
    let (min, max) = frame.bounds();
    if min.abs().max(max.abs()) < 0.00003 {
        return;
    }
    let anchor = 0.0_f32.clamp(min, max);
    let total = frame.bands.iter().map(|v| *v as f32).sum::<f32>().max(1.0);
    let shade = if dim { 0.48 } else { 0.95 };
    let y = |v: f32| rect.center().y - v * rect.height() * 0.46;
    let strongest = frame
        .bands
        .iter()
        .enumerate()
        .max_by_key(|(_, value)| *value)
        .map_or(0, |(i, _)| i);
    painter.line_segment(
        [Pos2::new(x, y(min)), Pos2::new(x, y(max))],
        Stroke::new(1.0_f32, BAND_COLOURS[strongest].gamma_multiply(shade)),
    );
    for end in [min, max] {
        let mut before = anchor;
        let mut fraction = 0.0;
        for (band, colour) in BAND_COLOURS.iter().enumerate() {
            fraction += frame.bands[band] as f32 / total;
            let after = anchor + (end - anchor) * fraction;
            if (y(before) - y(after)).abs() > 0.15 {
                painter.line_segment(
                    [Pos2::new(x, y(before)), Pos2::new(x, y(after))],
                    Stroke::new(1.0_f32, colour.gamma_multiply(shade)),
                );
            }
            before = after;
        }
    }
}

fn waveform_ui(
    ui: &mut egui::Ui,
    waveform: Option<&crate::waveform::Waveform>,
    duration: f64,
    fraction: f32,
) -> Option<f32> {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 36.0),
        Sense::click_and_drag(),
    );
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 3.0, Color32::from_rgb(19, 14, 26));
    if let Some(waveform) = waveform {
        let pixels = rect.width().ceil().max(1.0) as usize;
        for x in 0..pixels {
            let start = x as f64 / pixels as f64 * duration;
            let end = (x + 1) as f64 / pixels as f64 * duration;
            if let Some(frame) = waveform.sample(start, end) {
                spectral_column(
                    &painter,
                    rect,
                    rect.left() + x as f32 + 0.5,
                    frame,
                    x as f32 / (pixels as f32) < fraction,
                );
            }
        }
        let marker = rect.left() + fraction.clamp(0.0, 1.0) * rect.width();
        painter.line_segment(
            [
                Pos2::new(marker, rect.top()),
                Pos2::new(marker, rect.bottom()),
            ],
            Stroke::new(1.5_f32, Color32::WHITE),
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "DROP MUSIC HERE",
            FontId::proportional(11.0),
            MUTED,
        );
    }
    if waveform.is_some() && (response.clicked() || response.dragged()) {
        response
            .interact_pointer_pos()
            .map(|p| ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0))
    } else {
        None
    }
}

fn fx_knob(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    min: f32,
    max: f32,
    reset: f32,
    colour: Color32,
) {
    ui.vertical_centered(|ui| {
        ui.set_width(38.0);
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(RichText::new(label).size(8.0).color(MUTED));
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(34.0, 29.0), Sense::click_and_drag());
        if response.dragged() {
            *value = (*value - response.drag_delta().y * (max - min) * 0.006).clamp(min, max);
        }
        if response.double_clicked() {
            *value = reset;
        }
        response.on_hover_text("Drag to adjust; double-click to reset.");
        let angle = -std::f32::consts::FRAC_PI_2 + ((*value - min) / (max - min) * 2.0 - 1.0) * 2.3;
        ui.painter().circle_filled(rect.center(), 13.0, BG);
        ui.painter()
            .circle_stroke(rect.center(), 13.0, Stroke::new(1.0_f32, BORDER));
        ui.painter().line_segment(
            [
                rect.center() + Vec2::angled(angle) * 4.0,
                rect.center() + Vec2::angled(angle) * 11.0,
            ],
            Stroke::new(2.0_f32, colour),
        );
        let text = if min < 0.0 {
            if value.abs() < 0.015 {
                "OFF".into()
            } else {
                format!(
                    "{} {:.0}",
                    if *value < 0.0 { "LP" } else { "HP" },
                    value.abs() * 100.0
                )
            }
        } else {
            format!("{:.0}%", *value * 100.0)
        };
        ui.label(RichText::new(text).size(8.0).color(colour));
    });
}

impl RustDj {
    fn mixer_ui(&mut self, ui: &mut egui::Ui, width: f32) {
        let (mut apply_phones, mut refresh_devices, mut show_audio) = (false, false, false);
        card().show(ui, |ui| {
            ui.set_width(width - 18.0);
            ui.set_min_height(308.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.mixer_outputs, false, "MIXER");
                if ui.selectable_value(&mut self.mixer_outputs, true, "MASTER / PHONES").clicked() {
                    refresh_devices = true;
                }
                if ui.add(egui::Button::new("EQ").small().selected(self.master_eq.enabled)).on_hover_text("10-Band Master Equalizer").clicked() { self.show_equalizer = true; }
                let active = self.broadcaster.busy();
                let label = if self.broadcaster.recording_busy() { "STREAMER / REC" } else { "STREAMER" };
                if ui.add(egui::Button::new(label).small().selected(active)).on_hover_text("Stream the master to SHOUTcast and record MP3").clicked() {
                    self.show_streamer = true;
                }
            });
            ui.separator();
            if self.mixer_outputs {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new("PHONES").size(10.0).color(LIGHT_PURPLE));
                    let choice_width = (ui.available_width() - 152.0).max(80.0);
                    refresh_devices |= phones_device_ui(ui, "mixer-phones-device", &mut self.routing_edit.phones, &self.audio_devices, choice_width);
                    if let Some(target) = &mut self.routing_edit.phones {
                        channel_pair_ui(ui, "mixer-phones-device", target, &self.audio_devices, true);
                    } else {
                        ui.add_space(52.0);
                    }
                    let pending = self.routing_edit.phones != self.audio.routing.phones;
                    apply_phones = ui.add_enabled(pending || self.audio.error.is_some(), egui::Button::new("Apply").small())
                        .on_hover_text("Apply and save the headphone output. The master output stays unchanged.").clicked();
                    show_audio = ui.small_button("…").on_hover_text("Configure all audio outputs").clicked();
                });
                ui.horizontal_top(|ui| {
                    stereo_meter_ui(ui, self.bus_levels[0], "MASTER", PURPLE);
                    stereo_meter_ui(ui, self.bus_levels[1], "PHONES", LIGHT_PURPLE);
                });
            } else {
                ui.columns(COUNT, |columns| {
                    for (slot, index) in ORDER.into_iter().enumerate() {
                        channel_strip(&mut columns[slot], &mut self.decks[index], deck_color(index), index);
                    }
                });
            }
            ui.separator();
            output_levels_ui(ui, &mut self.master, &mut self.phones, &mut self.mixing);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "MASTER {}",
                        peak_text(self.bus_levels[0][0].max(self.bus_levels[0][1]))
                    ))
                    .size(11.0)
                    .color(PURPLE),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "PHONES {}",
                        peak_text(self.bus_levels[1][0].max(self.bus_levels[1][1]))
                    ))
                    .size(11.0)
                    .color(LIGHT_PURPLE),
                );
            });
            ui.separator();
            ui.horizontal(|ui| {
                let mut enabled = self.crossfader.enabled;
                if ui.checkbox(&mut enabled, "CROSSFADER").changed() {
                    self.crossfader.set_enabled(enabled);
                }
                ui.label(
                    RichText::new(if self.crossfader.enabled { "ON" } else { "OFF" })
                        .strong()
                        .color(if self.crossfader.enabled { PURPLE } else { MUTED }),
                );
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("1/3").color(PURPLE));
                ui.spacing_mut().slider_width = width - 74.0;
                let mut position = self.crossfader.position;
                if ui.add_enabled(
                    self.crossfader.enabled,
                    egui::Slider::new(&mut position, -1.0..=1.0).show_value(false),
                ).on_hover_text("OFF: centered; MIDI and keyboard movements are ignored.").changed() {
                    self.crossfader.set_position(position);
                }
                ui.label(RichText::new("4/2").color(LIGHT_PURPLE));
            });
            if self.bus_levels.iter().flatten().any(|peak| *peak >= 1.0) {
                ui.colored_label(WARNING, "CLIP · Reduce output level");
            }
        });
        if refresh_devices || show_audio {
            self.refresh_audio_devices(ui.ctx());
        }
        if show_audio {
            self.show_audio = true;
        }
        if apply_phones {
            let master_draft = self.routing_edit.master.clone();
            let mut routing = self.audio.routing.clone();
            routing.phones = self.routing_edit.phones.clone();
            self.apply_audio(routing);
            self.routing_edit.master = master_draft;
        }
    }

    fn waveforms_ui(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.horizontal(|ui| {
            for (band, label) in ["LOW <250 Hz", "MID", "HIGH >4 kHz"]
                .into_iter()
                .enumerate()
            {
                ui.label(RichText::new(label).size(10.0).color(BAND_COLOURS[band]));
            }
            ui.separator();
            if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                self.wave_window = (self.wave_window * 2.0).min(16.0);
            }
            ui.label(RichText::new(format!("{} s", self.wave_window)).small());
            if ui.small_button("+").on_hover_text("More detail").clicked() {
                self.wave_window = (self.wave_window / 2.0).max(1.0);
            }
        });
        let lane_height = ((ui.available_height() - 6.0) / COUNT as f32).max(22.0);
        for index in ORDER {
            let deck = &mut self.decks[index];
            let colour = deck_color(index);
            let position = deck.position().as_secs_f64();
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), lane_height), Sense::drag());
            let painter = ui.painter().with_clip_rect(rect);
            painter.rect_filled(rect, 0.0, Color32::from_rgb(11, 12, 14));
            let window = self.wave_window * deck.speed as f64;
            if let Some(track) = &deck.track {
                if deck.loop_active
                    && let Some(range) = deck.loop_range
                {
                    let left = rect.center().x
                        + ((range.start.as_secs_f64() - position) / window) as f32 * rect.width();
                    let right = rect.center().x
                        + ((range.end.as_secs_f64() - position) / window) as f32 * rect.width();
                    painter.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(left, rect.top()),
                            Pos2::new(right, rect.bottom()),
                        )
                        .intersect(rect),
                        0.0,
                        colour.gamma_multiply(0.08),
                    );
                }
                let pixels = rect.width().ceil().max(1.0) as usize;
                for x in 0..pixels {
                    let start = position + (x as f64 / pixels as f64 - 0.5) * window;
                    let end = start + window / pixels as f64;
                    if let Some(frame) = track.detailed.sample(start, end) {
                        spectral_column(
                            &painter,
                            rect,
                            rect.left() + x as f32 + 0.5,
                            frame,
                            x < pixels / 2,
                        );
                    }
                }
                if let Some(grid) = deck.grid {
                    let subdivision = if self.wave_window <= 4.0 { 4 } else { 1 };
                    let period = grid.period() / subdivision as f64;
                    let first = ((position - window / 2.0 - grid.offset) / period).floor() as i64;
                    for tick in first..first + (window / period).ceil() as i64 + 2 {
                        let time = grid.offset + tick as f64 * period;
                        let x =
                            rect.center().x + ((time - position) / window) as f32 * rect.width();
                        painter.line_segment(
                            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                            Stroke::new(
                                1.0_f32,
                                Color32::from_white_alpha(
                                    if tick.rem_euclid(subdivision * 4) == 0 {
                                        80
                                    } else if tick.rem_euclid(subdivision) == 0 {
                                        40
                                    } else {
                                        13
                                    },
                                ),
                            ),
                        );
                    }
                }
            }
            painter.line_segment(
                [
                    Pos2::new(rect.center().x, rect.top()),
                    Pos2::new(rect.center().x, rect.bottom()),
                ],
                Stroke::new(1.5_f32, Color32::WHITE),
            );
            painter.text(
                rect.left_top() + Vec2::new(7.0, 4.0),
                egui::Align2::LEFT_TOP,
                index + 1,
                FontId::proportional(12.0),
                colour,
            );
            if response.dragged() {
                let time = (position
                    - response.drag_delta().x as f64 / rect.width() as f64 * window)
                    .max(0.0);
                deck.seek(Duration::from_secs_f64(time), &self.audio);
            }
        }
    }
}

pub(crate) fn console_dimensions(width: f32) -> (f32, f32) {
    let mixer = (width * 0.32).clamp(400.0, 480.0);
    (mixer, (width - mixer - 24.0) * 0.25)
}

fn controller_status(port: Option<&str>, auto: bool) -> String {
    match port {
        Some(port) => format!("CONTROLLER ON · {port}"),
        None => format!(
            "CONTROLLER OFF{}",
            if auto { " · auto-detecting" } else { "" }
        ),
    }
}

fn peak_db(level: f32) -> f32 {
    if level.is_finite() && level > 0.000001 {
        20.0 * level.log10()
    } else {
        f32::NEG_INFINITY
    }
}

fn peak_number(level: f32) -> String {
    let db = peak_db(level);
    if db.is_finite() {
        format!("{db:+.1}")
    } else {
        "−∞".into()
    }
}

fn peak_text(level: f32) -> String {
    format!("{} dBFS", peak_number(level))
}

pub(crate) fn output_levels_ui(
    ui: &mut egui::Ui,
    master: &mut f32,
    phones: &mut f32,
    mixing: &mut f32,
) -> [egui::Response; 3] {
    let mut responses = Vec::with_capacity(3);
    ui.columns(3, |columns| {
        for (column, (label, value, color, hint)) in columns.iter_mut().zip([
            (
                "MASTER",
                master,
                PURPLE,
                "Volume of the master, stream and recording",
            ),
            (
                "HEADPHONES",
                phones,
                LIGHT_PURPLE,
                "Volume of the selected headphone output",
            ),
            (
                "CUE / MASTER",
                mixing,
                MUTED,
                "Left: cue monitoring; right: master in headphones",
            ),
        ]) {
            column.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.spacing_mut().slider_width = ui.available_width();
                ui.label(RichText::new(label).size(9.0).strong().color(color));
                responses.push(
                    ui.add(egui::Slider::new(value, 0.0..=1.0).show_value(false))
                        .on_hover_text(hint),
                );
                ui.label(
                    RichText::new(format!("{:.0} %", *value * 100.0))
                        .size(9.0)
                        .color(color),
                );
            });
        }
    });
    responses.try_into().unwrap()
}

fn stereo_meter_ui(ui: &mut egui::Ui, levels: [f32; 2], label: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.set_width(102.0);
        ui.label(RichText::new(label).strong().color(color));
        ui.horizontal_top(|ui| {
            meter_ui(ui, levels[0], color, "L", 172.0);
            meter_ui(ui, levels[1], color, "R", 172.0);
        });
    });
}

fn meter_ui(ui: &mut egui::Ui, level: f32, color: Color32, label: &str, height: f32) {
    ui.vertical_centered(|ui| {
        ui.set_width(48.0);
        ui.label(RichText::new(label).strong().color(color));
        let (rect, response) = ui.allocate_exact_size(Vec2::new(48.0, height), Sense::hover());
        response.on_hover_text("Digital peak level in dBFS; levels at or above 0 dBFS may clip.");
        let painter = ui.painter();
        let db = peak_db(level);
        for i in 0..30 {
            let bottom = rect.bottom() - i as f32 * height / 30.0;
            let segment = Rect::from_min_size(
                Pos2::new(rect.left() + 23.0, bottom - height / 30.0 + 1.0),
                Vec2::new(24.0, height / 30.0 - 1.0),
            );
            let threshold = -60.0 + i as f32 / 29.0 * 66.0;
            let tint = if db < threshold {
                BORDER.gamma_multiply(0.45)
            } else if threshold >= 0.0 {
                Color32::from_rgb(255, 83, 93)
            } else if threshold >= -6.0 {
                Color32::from_rgb(247, 202, 85)
            } else {
                color
            };
            painter.rect_filled(segment, 1.0, tint);
        }
        for mark in [-48.0, -24.0, -12.0, -6.0, 0.0, 6.0] {
            let y = rect.bottom() - (mark + 60.0) / 66.0 * height;
            painter.text(
                Pos2::new(rect.left() + 20.0, y),
                egui::Align2::RIGHT_CENTER,
                format!("{mark:.0}"),
                FontId::monospace(8.0),
                MUTED,
            );
        }
        ui.label(
            RichText::new(peak_number(level))
                .monospace()
                .size(11.0)
                .color(if level >= 1.0 { WARNING } else { color }),
        );
        ui.label(RichText::new("dBFS").size(8.0).color(MUTED));
    });
}

#[cfg(test)]
mod crossfader_tests {
    use super::*;

    #[test]
    fn crossfader15_off_centers_and_ignores_midi_and_keyboard_movements() {
        let profile = midi::dj202_profile();
        let mut mapper = midi::Mapper::default();
        for initial in [-1.0, -0.4, 0.7, 1.0] {
            let mut crossfader = Crossfader::new(true);
            crossfader.set_position(initial);
            assert_eq!(crossfader.position, initial);
            crossfader.set_enabled(false);
            assert_eq!(crossfader.position, 0.0);
            for raw in [0_u16, 16368, 4096, 12288, 8192] {
                assert!(
                    mapper
                        .decode([0xbf, 8, (raw >> 7) as u8], &profile)
                        .is_empty()
                );
                let actions = mapper.decode([0xbf, 0x58, (raw & 127) as u8], &profile);
                assert_eq!(actions.len(), 1);
                for (action, value) in actions {
                    assert_eq!(action, Action::Crossfader);
                    crossfader.midi(action, value);
                }
                crossfader.nudge(-0.05);
                crossfader.nudge(0.05);
                crossfader.set_position(1.0);
                assert_eq!(crossfader.position, 0.0);
                assert!(!crossfader.enabled);
            }
            crossfader.set_enabled(true);
            assert_eq!(crossfader.position, 0.0);
            crossfader.midi(Action::Crossfader, 1.0);
            assert_eq!(crossfader.position, 1.0);
            crossfader.midi(Action::CrossfaderToggle, 1.0);
            assert!(!crossfader.enabled);
            assert_eq!(crossfader.position, 0.0);
            crossfader.midi(Action::CrossfaderToggle, 1.0);
            assert!(crossfader.enabled);
            assert_eq!(crossfader.position, 0.0);
            crossfader.nudge(-0.05);
            assert_eq!(crossfader.position, -0.05);
        }
    }

    #[test]
    fn crossfader15_saved_off_restarts_centered_and_preserves_audio_bypass() {
        let saved = MixSettings {
            crossfader_enabled: false,
            ..MixSettings::default()
        };
        let json = serde_json::to_string(&saved).unwrap();
        let restored: MixSettings = serde_json::from_str(&json).unwrap();
        let mut crossfader = Crossfader::new(restored.crossfader_enabled);
        crossfader.midi(Action::Crossfader, 0.0);
        crossfader.midi(Action::Crossfader, 1.0);
        assert_eq!(crossfader.position, 0.0);
        assert!(!crossfader.enabled);
        let gains = bus_gains_with_crossfader(
            [0.8, 0.6],
            [true, false],
            crossfader.position,
            0.5,
            0.4,
            0.0,
            crossfader.enabled,
        );
        assert_eq!(gains.master, [0.4, 0.3]);
        assert_eq!(gains.phones, [0.4, 0.0]);
        crossfader.set_enabled(true);
        let gains = bus_gains_with_crossfader(
            [0.8, 0.6],
            [true, false],
            crossfader.position,
            0.5,
            0.4,
            0.0,
            crossfader.enabled,
        );
        let center_gain = std::f32::consts::FRAC_1_SQRT_2;
        assert!((gains.master[0] - 0.4 * center_gain).abs() < 0.00001);
        assert!((gains.master[1] - 0.3 * center_gain).abs() < 0.00001);
        assert_eq!(gains.phones, [0.4, 0.0]);
        let old: MixSettings = serde_json::from_str("{}").unwrap();
        let legacy = Crossfader::new(old.crossfader_enabled);
        assert!(legacy.enabled);
        assert_eq!(legacy.position, 0.0);
    }
}

#[cfg(test)]
mod console_meter_tests {
    use super::*;
    #[test]
    fn console_meter_layout_status_and_numeric_levels_cover_supported_sizes() {
        for width in [1268.0, 1668.0, 1908.0] {
            let (mixer, deck) = console_dimensions(width);
            assert!(mixer >= 400.0 && deck >= 200.0);
            assert!((mixer + deck * 4.0 + 24.0 - width).abs() < 0.01);
        }
        assert!(controller_status(Some("DJ-202"), false).contains("ON · DJ-202"));
        assert!(controller_status(None, true).contains("OFF"));
        assert!(peak_text(0.0).contains("−∞"));
        assert!((peak_db(0.5) + 6.0206).abs() < 0.0001);
        assert_eq!(peak_text(1.0), "+0.0 dBFS");
        assert!((peak_db(2.0) - 6.0206).abs() < 0.0001);
        assert_eq!(peak_db(f32::NAN), f32::NEG_INFINITY);
    }
}
