use crate::{
    audio::{Audio, Deck},
    autodj::{AutoDj, Control as AutoDjControl},
    beat::{BeatGrid, sync_plan},
    broadcast_mix::{Talkover, TalkoverSettings},
    browser::{Browser, Event as BrowserEvent},
    controller_profiles::{ProfileFile, ProfileId},
    database::{self, Database, HistoryEntry},
    decks::{COUNT, Layout as DeckLayout},
    dragdrop::{self, Origin, Target as DropTarget, Zones},
    dsp::{bus_gains_with_crossfader, knob_db},
    equalizer::{self, Settings as MasterEqSettings},
    inputs::{self, InputSettings, LiveInput},
    jingles::{self, Pads},
    loops::PRESETS as LOOP_PRESETS,
    midi::{self, Action, Midi, Mode},
    output::{self, DeviceInfo, Routing, Target},
    performance::Store as HotcueStore,
    playlist::{Item, Playlist},
    settings::{self, MixSettings, Settings},
    streamer::{Broadcaster, ConnectionSecurity, Phase, Status, StreamProfile, StreamSettings},
    track,
};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const BG: Color32 = Color32::from_rgb(18, 19, 21);
const PANEL: Color32 = Color32::from_rgb(36, 38, 41);
const BORDER: Color32 = Color32::from_rgb(61, 64, 69);
const MUTED: Color32 = Color32::from_rgb(161, 151, 179);
const PURPLE: Color32 = Color32::from_rgb(174, 119, 255);
const LIGHT_PURPLE: Color32 = Color32::from_rgb(219, 186, 255);
const AUX_INPUT_COUNT: usize = 2;
const MIC: usize = AUX_INPUT_COUNT;
const INPUT_COUNT: usize = AUX_INPUT_COUNT + 1;
const WARNING: Color32 = Color32::from_rgb(247, 101, 112);

#[derive(Clone, Copy, PartialEq, Eq)]
enum LibraryTab {
    Session,
    Playlist,
    AutoDj,
}

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

#[cfg(target_os = "linux")]
enum KeyringRequest {
    Load,
    Save(String),
    Remove,
    DeleteProfile,
}

#[cfg(target_os = "linux")]
enum KeyringUpdate {
    Loaded(String),
    Saved,
    Removed,
    ProfileDeleted,
}

#[cfg(target_os = "linux")]
impl KeyringRequest {
    fn spawn(self, id: u64) -> Receiver<Result<KeyringUpdate, String>> {
        use crate::credentials::linux as keyring;

        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let update = match self {
                Self::Load => keyring::load(id).map(KeyringUpdate::Loaded),
                Self::Save(password) => keyring::save(id, password).map(|()| KeyringUpdate::Saved),
                Self::Remove => keyring::clear(id).map(|()| KeyringUpdate::Removed),
                Self::DeleteProfile => keyring::load(id).and_then(|password| {
                    if !password.is_empty() {
                        keyring::clear(id)?;
                    }
                    Ok(KeyringUpdate::ProfileDeleted)
                }),
            };
            let _ = sender.send(update);
        });
        receiver
    }
}

struct GridEditor {
    deck: usize,
    path: PathBuf,
    grid: BeatGrid,
    error: Option<String>,
}

pub struct RustDj {
    logo: egui::TextureHandle,
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
    library_tab: LibraryTab,
    autodj: AutoDj,
    search: String,
    active: usize,
    deck_layout: DeckLayout,
    library_view: bool,
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
    stream_profiles: Vec<StreamProfile>,
    active_stream_profile: usize,
    stream_profile_serial: u64,
    confirm_profile_delete: bool,
    input_settings: [InputSettings; INPUT_COUNT],
    live_inputs: [Option<LiveInput>; INPUT_COUNT],
    input_errors: [Option<String>; INPUT_COUNT],
    mic_peak: f32,
    talkover_settings: TalkoverSettings,
    talkover: Talkover,
    master_limiter: bool,
    limiter_reduction: f32,
    jingles: Pads,
    show_jingles: bool,
    show_performance: bool,
    show_history: bool,
    history: Vec<HistoryEntry>,
    history_refresh: Instant,
    audible_since: [Option<Instant>; COUNT],
    history_logged: [bool; COUNT],
    grid_editor: Option<GridEditor>,
    input_devices: Vec<inputs::DeviceInfo>,
    input_scan: Option<Receiver<Result<Vec<inputs::DeviceInfo>, String>>>,
    input_scan_error: Option<String>,
    show_inputs: bool,
    stream_password: String,
    stream_password_error: Option<String>,
    #[cfg(target_os = "linux")]
    keyring_task: Option<Receiver<Result<KeyringUpdate, String>>>,
    #[cfg(target_os = "linux")]
    keyring_notice: Option<&'static str>,
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
        let migrated = settings.mapping_version < 7;
        if settings.mapping_version < 6
            && let Some(bindings) = settings.bindings.take()
        {
            settings.bindings = Some(midi::upgrade(bindings));
        }
        let audio = Audio::new(settings.routing.clone());
        audio.master_eq.set(settings.master_eq);
        audio.limiter.set(settings.master_limiter);
        if settings.stream_profiles.is_empty() {
            settings.stream_profiles.push(StreamProfile {
                id: 0,
                name: "Default".into(),
                settings: settings.streamer.clone(),
            });
        }
        let active_stream_profile = settings.stream_profiles.iter()
            .position(|profile| profile.id == settings.active_stream_profile)
            .unwrap_or(0);
        let active_profile = &settings.stream_profiles[active_stream_profile];
        let stream_profile_serial = settings.stream_profiles.iter()
            .map(|profile| profile.id)
            .max()
            .unwrap_or(0)
            .max(settings.stream_profile_serial);
        let mut stream_settings = active_profile.settings.clone();
        stream_settings.recording_dir = settings.streamer.recording_dir;
        #[cfg(target_os = "linux")]
        let (stream_password, stream_password_error) = (String::new(), None);
        #[cfg(not(target_os = "linux"))]
        let (stream_password, stream_password_error) =
            match crate::credentials::unprotect(&stream_settings.protected_password) {
                Ok(password) => (password, None),
                Err(error) => (String::new(), Some(error)),
            };
        Self {
            logo: crate::branding::logo(ctx),
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
            library_tab: LibraryTab::Session,
            autodj: AutoDj::new(settings.autodj),
            search: String::new(),
            active: 0,
            deck_layout: settings.deck_layout,
            library_view: false,
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
            stream_settings,
            active_stream_profile,
            stream_profile_serial,
            confirm_profile_delete: false,
            input_settings: {
                let [first, second] = settings.inputs;
                [first, second, settings.microphone]
            },
            live_inputs: std::array::from_fn(|_| None),
            input_errors: std::array::from_fn(|_| None),
            mic_peak: 0.0,
            talkover_settings: settings.talkover,
            talkover: Talkover::default(),
            master_limiter: settings.master_limiter,
            limiter_reduction: 0.0,
            jingles: Pads::new(settings.jingles),
            show_jingles: false,
            show_performance: false,
            show_history: false,
            history: Vec::new(),
            history_refresh: Instant::now(),
            audible_since: [None; COUNT],
            history_logged: [false; COUNT],
            grid_editor: None,
            input_devices: Vec::new(),
            input_scan: None,
            input_scan_error: None,
            show_inputs: false,
            stream_password,
            stream_password_error,
            #[cfg(target_os = "linux")]
            keyring_task: Some(KeyringRequest::Load.spawn(active_profile.id)),
            #[cfg(target_os = "linux")]
            keyring_notice: None,
            stream_profiles: settings.stream_profiles,
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

    fn load_dropped_tracks(
        &mut self,
        first_deck: usize,
        paths: impl IntoIterator<Item = PathBuf>,
        ctx: &egui::Context,
    ) {
        let order = self.deck_layout.order();
        let slot = order.iter().position(|&deck| deck == first_deck).unwrap_or(0);
        let targets = order.iter().cycle().skip(slot).take(order.len());
        for (path, &target) in paths.into_iter().zip(targets) {
            self.decks[target].load(path, ctx.clone());
            self.active = target;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || self.browser.confirmation_open() {
            return;
        }
        for (key, deck) in [
            (egui::Key::Num1, 0),
            (egui::Key::Num2, 1),
            (egui::Key::Num3, 2),
            (egui::Key::Num4, 3),
        ] {
            if self.deck_layout.contains(deck) && ctx.input(|i| i.key_pressed(key)) {
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
            for &index in self.deck_layout.order() {
                self.stop_deck(index);
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            self.crossfader.nudge(-0.05);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            self.crossfader.nudge(0.05);
        }
    }

    fn set_deck_layout(&mut self, layout: DeckLayout) {
        if self.deck_layout == layout {
            return;
        }
        if self.autodj.active_deck().is_some_and(|deck| !layout.contains(deck)) {
            self.autodj.stop(&mut self.decks, &self.audio);
        }
        self.deck_layout = layout;
        if !layout.contains(self.active) {
            self.active = layout.order()[0];
        }
        for (deck, reference) in self.synced.iter_mut().enumerate() {
            if !layout.contains(deck) || reference.is_some_and(|index| !layout.contains(index)) {
                *reference = None;
            }
        }
        self.audio_dirty = true;
    }

    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::new((self.logo.id(), Vec2::new(120.0, 40.0))).alt_text("RustDJ"));
            ui.label(
                RichText::new(env!("CARGO_PKG_VERSION"))
                    .size(10.0)
                    .color(MUTED),
            );
            ui.separator();
            let hidden_decks_playing = self
                .decks
                .iter()
                .enumerate()
                .any(|(index, deck)| !DeckLayout::Two.contains(index) && deck.playing());
            if ui
                .add_enabled(
                    !hidden_decks_playing,
                    egui::Button::new("2 Decks").selected(self.deck_layout == DeckLayout::Two),
                )
                .on_disabled_hover_text("Pause Decks 3 and 4 before switching to 2 Decks.")
                .clicked()
            {
                self.set_deck_layout(DeckLayout::Two);
            }
            if ui
                .selectable_label(self.deck_layout == DeckLayout::Four, "4 Decks")
                .clicked()
            {
                self.set_deck_layout(DeckLayout::Four);
            }
            if ui
                .selectable_label(self.library_view, "Library")
                .on_hover_text("Expand the music browser. Click again to show the decks and mixer.")
                .clicked()
            {
                self.library_view = !self.library_view;
            }
            if ui.button("Jingles").clicked() {
                self.show_jingles = true;
            }
            if ui.button("History").clicked() {
                self.show_history = true;
                self.history_refresh = Instant::now();
            }
            if ui.add(egui::Button::new("Talkover").selected(self.talkover_settings.enabled)).clicked() {
                self.show_performance = true;
            }
            ui.separator();
            let midi =
                controller_status(self.midi.connected.as_deref(), self.midi.prefs.auto_connect);
            ui.add_sized(
                [(ui.available_width() - 390.0).max(90.0), 24.0],
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
                let inputs_active = self.live_inputs[..AUX_INPUT_COUNT].iter().filter(|input| input.is_some()).count();
                let label = if inputs_active == 0 {
                    "Audio Inputs".into()
                } else {
                    format!("Audio Inputs · {inputs_active}/2")
                };
                if ui.add(egui::Button::new(label).selected(inputs_active != 0 || self.live_inputs[MIC].is_some())).clicked() {
                    self.show_inputs = true;
                    self.refresh_input_devices(ctx);
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
                    .unwrap_or("System default")
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
                            .unwrap_or("System default");
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
        ui.separator();
        ui.columns(2, |columns| {
            columns[0].horizontal(|ui| {
                self.stream_control_ui(ui);
                let status = &self.broadcaster.stream_status;
                broadcast_status_ui(ui, "STREAM", status, "LIVE", PURPLE);
            });
            columns[1].horizontal(|ui| {
                self.recording_control_ui(ui);
                let status = &self.broadcaster.recording_status;
                broadcast_status_ui(ui, "REC", status, "RECORDING", LIGHT_PURPLE);
            });
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
                            "Select music on the left and load it into a deck.",
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
                        for &index in self.deck_layout.order() {
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
            std::array::from_fn(|deck| {
                if self.deck_layout.contains(deck) {
                    self.decks[deck].volume * self.talkover.gain()
                } else {
                    0.0
                }
            }),
            std::array::from_fn(|deck| self.deck_layout.contains(deck) && self.decks[deck].pfl),
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
        let loaded = std::array::from_fn(|i| {
            self.deck_layout.contains(i) && self.decks[i].track.is_some() && self.synced[i].is_none()
        });
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

    fn stop_deck(&mut self, deck: usize) {
        if self.autodj.active_deck() == Some(deck) {
            self.autodj.stop(&mut self.decks, &self.audio);
        } else {
            self.decks[deck].stop(&self.audio);
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

    fn flush_jogs(&mut self, jogs: &mut [f32; COUNT]) {
        for (d, ticks) in jogs.iter_mut().enumerate() {
            let ticks = std::mem::take(ticks);
            if ticks != 0.0 {
                self.synced[d] = None;
                if let Some(position) = self.decks[d].jog(ticks, Instant::now()) {
                    self.decks[d].seek(position, &self.audio);
                }
            }
        }
    }

    fn midi_actions(&mut self, ctx: &egui::Context) {
        let mut jogs = [0.0; COUNT];
        for (action, value) in self.midi.poll() {
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
            if deck.is_some_and(|d| !self.deck_layout.contains(d)) {
                continue;
            }
            if let Action::Jog(d) = action {
                jogs[d] += value;
                continue;
            }
            // Apply pending turns before a transport action can read/change position.
            self.flush_jogs(&mut jogs);
            match action {
                Action::SelectDeck(d) => self.active = d,
                Action::DeckLayer(_) => {}
                Action::Play(d) => self.toggle_deck(d),
                Action::Cue(d) => self.decks[d].cue_button(value > 0.0, &self.audio),
                Action::SetCue(d) => self.decks[d].cue = self.decks[d].position(),
                Action::Stop(d) => self.stop_deck(d),
                Action::Sync(d) => self.sync(d),
                Action::Volume(d) => self.decks[d].volume = value.clamp(0.0, 1.0),
                Action::Tempo(d) => {
                    self.decks[d].speed = 1.0 + (value - 0.5) * 0.32;
                    if self.synced[d].is_some() {
                        self.synced[d] = None;
                    }
                }
                Action::Jog(_) => unreachable!(),
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
                Action::Jingle(pad) if value > 0.0 => self.play_jingle(pad),
                Action::Jingle(_) => {}
                Action::Open => {
                    if let Some(BrowserEvent::Load(d, path)) =
                        self.browser.activate(self.active, ctx)
                    {
                        self.decks[d].load(path, ctx.clone());
                    }
                }
            }
        }
        self.flush_jogs(&mut jogs);
    }

    fn controller_ui(&mut self, ctx: &egui::Context) {
        egui::Window::new("Controller / MIDI").open(&mut self.show_midi).default_width(700.0).resizable(true).vscroll(true).show(ctx, |ui| {
            let mut choice=self.midi.prefs.profile;
            ui.horizontal(|ui| {
                ui.label("Controller profile");
                egui::ComboBox::from_id_salt("controller-profile").selected_text(choice.map_or("Automatic (known models)",ProfileId::label)).show_ui(ui,|ui| {
                    ui.selectable_value(&mut choice,None,"Automatic (known models)");
                    for profile in ProfileId::ALL { ui.selectable_value(&mut choice,Some(profile),profile.label()); }
                });
            });
            if choice!=self.midi.prefs.profile { self.midi.choose_profile(choice,ctx); }
            ui.label(RichText::new(format!("Active profile: {}",self.midi.active_profile.label())).strong().color(PURPLE));
            ui.label(self.midi.active_profile.description());
            ui.label(RichText::new("Model-specific USB MIDI mappings. Hardware verification is pending; HID, DVS and proprietary displays are not implied.").small().color(MUTED));
            if ui.checkbox(&mut self.midi.prefs.auto_connect, "Connect automatically and reconnect after USB disconnection").changed() { self.midi.dirty = true; }
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Preferred: {}", self.midi.prefs.preferred_port.as_deref().unwrap_or("Detect a supported model"))).small());
                if ui.small_button("Auto-detect device").clicked() { self.midi.prefs.preferred_port = None; self.midi.prefs.auto_connect = true; self.midi.dirty = true; self.midi.refresh(); }
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
                if ui.button("Reset profile").clicked() { self.midi.reset_profile(); }
                if ui.button("Import JSON …").clicked() && let Some(path)=rfd::FileDialog::new().set_title("Import RustDJ controller profile").add_filter("RustDJ MIDI profile",&["json"]).pick_file() {
                    match ProfileFile::read(&path) {
                        Ok(file)=>{self.midi.choose_profile(Some(file.profile),ctx);self.midi.bindings=file.bindings;self.midi.save_current_profile();self.midi.dirty=true;},
                        Err(error)=>self.midi.error=Some(error),
                    }
                }
                if ui.button("Export JSON …").clicked() && let Some(path)=rfd::FileDialog::new().set_title("Export RustDJ controller profile").set_file_name("RustDJ-controller.json").add_filter("RustDJ MIDI profile",&["json"]).save_file() {
                    let file=ProfileFile{schema_version:1,profile:self.midi.active_profile,bindings:self.midi.bindings.clone()};
                    if let Err(error)=file.write(&path) { self.midi.error=Some(error); }
                }
                if ui.checkbox(&mut self.midi.dj202_leds, "MIDI LEDs (supported controls)").changed() {self.midi.dirty=true;}
            });
            ui.label(RichText::new("Each model keeps its own learned mappings. Reset restores only the active profile. Unknown devices use Generic MIDI / Learn.").small().color(MUTED));
            ui.label("Select the appropriate HOT CUE / LOOP mode before using performance pads.");
            ui.label(RichText::new("HOT CUE: empty = set, assigned = jump/play; SHIFT+pad = clear. LOOP: ¼ · ½ · 1 · 2 · 4 · 8 · 16 · 32 beats; press again to exit. PARAM −/+ halves/doubles the loop.").small());
            for (deck, mode) in self.midi.pad_modes.iter().enumerate() {
                if self.midi.active_profile.banked() { ui.label(RichText::new(format!("Deck {} · Pads: {}", deck + 1, mode.map_or("Select HOT CUE/LOOP on the controller", midi::PadMode::label))).small().color(PURPLE)); }
            }
            ui.separator();
            ui.label(RichText::new("MIDI-LEARN").strong());
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("midi-action").selected_text(self.midi.learn_action.label()).show_ui(ui, |ui| {
                    for action in Action::all() { ui.selectable_value(&mut self.midi.learn_action, action, action.label()); }
                });
                egui::ComboBox::from_id_salt("midi-mode").selected_text(match self.midi.learn_mode { Mode::Absolute7Inverted => "Inverted fader (7-bit)", Mode::Relative64 => "Encoder: center 64", Mode::RelativeTwos => "Encoder: 1 / 127", _ => "Button / fader (7-bit)" }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.midi.learn_mode, Mode::Absolute7, "Button / fader (7-bit)");
                    ui.selectable_value(&mut self.midi.learn_mode, Mode::Absolute7Inverted, "Inverted fader (7-bit)");
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

    fn stream_control_ui(&mut self, ui: &mut egui::Ui) {
        if self.broadcaster.stream_busy() {
            let stopping = self.broadcaster.stream_status.phase == Phase::Stopping;
            if ui.add_enabled(!stopping, egui::Button::new("Stop stream")).clicked() {
                self.broadcaster.stop_stream();
                self.broadcaster.poll();
            }
            return;
        }

        let label = match self.stream_settings.security {
            ConnectionSecurity::Tls => "Start TLS stream",
            ConnectionSecurity::Plaintext => "Start unencrypted stream",
        };
        let button = egui::Button::new(label).fill(PURPLE.gamma_multiply(0.5));
        let master_ready = self.audio.stream.is_some();
        let keyring_ready = self.keyring_ready();
        let disabled_hint = if keyring_ready {
            "Select an available master output under Audio / Outputs first."
        } else {
            "Waiting for the system keyring."
        };
        if ui
            .add_enabled(master_ready && keyring_ready, button)
            .on_disabled_hover_text(disabled_hint)
            .on_hover_text(format!("Stream profile: {}", self.stream_profiles[self.active_stream_profile].name))
            .clicked()
            && let Err(message) = self.broadcaster.start_stream(
                self.audio.capture.clone(),
                self.stream_settings.clone(),
                self.stream_password.clone(),
            )
        {
            self.broadcaster.stream_status = Status {
                phase: Phase::Error,
                message,
                ..Status::default()
            };
        }
    }

    fn recording_control_ui(&mut self, ui: &mut egui::Ui) {
        if self.broadcaster.recording_busy() {
            let stopping = self.broadcaster.recording_status.phase == Phase::Stopping;
            if ui
                .add_enabled(!stopping, egui::Button::new("Stop recording"))
                .clicked()
            {
                self.broadcaster.stop_recording();
                self.broadcaster.poll();
            }
            return;
        }

        if ui
            .add_enabled(
                self.audio.stream.is_some(),
                egui::Button::new("Start MP3 recording"),
            )
            .on_disabled_hover_text("Select an available master output under Audio / Outputs first.")
            .clicked()
            && let Err(message) = self
                .broadcaster
                .start_recording(self.audio.capture.clone(), &self.stream_settings)
        {
            self.broadcaster.recording_status = Status {
                phase: Phase::Error,
                message,
                ..Status::default()
            };
        }
    }

    fn keyring_ready(&self) -> bool {
        #[cfg(target_os = "linux")]
        return self.keyring_task.is_none();

        #[cfg(not(target_os = "linux"))]
        true
    }

    #[cfg(target_os = "linux")]
    fn poll_keyring(&mut self) {
        let Some(task) = &self.keyring_task else {
            return;
        };
        let result = match task.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The keyring operation stopped unexpectedly.".into())
            }
        };
        self.keyring_task = None;
        self.stream_password_error = None;
        self.keyring_notice = match result {
            Ok(KeyringUpdate::Loaded(password)) => {
                self.stream_password = password;
                if self.stream_password.is_empty() {
                    None
                } else {
                    Some("Password loaded from the system keyring.")
                }
            }
            Ok(KeyringUpdate::Saved) => Some("Password saved in the system keyring."),
            Ok(KeyringUpdate::ProfileDeleted) => {
                self.delete_stream_profile();
                return;
            }
            Ok(KeyringUpdate::Removed) => {
                self.stream_password.clear();
                Some("Saved password removed from the system keyring.")
            }
            Err(error) => {
                self.stream_password_error = Some(error);
                None
            }
        };
    }

    #[cfg(target_os = "linux")]
    fn keyring_controls_ui(&mut self, ui: &mut egui::Ui) {
        if !self.keyring_ready() {
            ui.label("Waiting for the system keyring …");
            return;
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.stream_password.is_empty(), egui::Button::new("Save password"))
                .clicked()
            {
                self.keyring_task = Some(KeyringRequest::Save(self.stream_password.clone()).spawn(self.stream_profile_id()));
            }
            if ui.button("Remove saved password").clicked() {
                self.keyring_task = Some(KeyringRequest::Remove.spawn(self.stream_profile_id()));
            }
            if self.stream_password_error.is_some() && ui.button("Reload saved password").clicked() {
                self.keyring_task = Some(KeyringRequest::Load.spawn(self.stream_profile_id()));
            }
        });
        if let Some(notice) = self.keyring_notice {
            ui.label(notice);
        }
    }

    fn stream_profile_id(&self) -> u64 {
        self.stream_profiles[self.active_stream_profile].id
    }

    fn select_stream_profile(&mut self, index: usize) {
        if index == self.active_stream_profile {
            return;
        }
        self.stream_profiles[self.active_stream_profile].settings = self.stream_settings.clone();
        self.restore_stream_profile(index);
    }

    fn restore_stream_profile(&mut self, index: usize) {
        self.active_stream_profile = index;
        let recording_dir = self.stream_settings.recording_dir.clone();
        self.stream_settings = self.stream_profiles[index].settings.clone();
        self.stream_settings.recording_dir = recording_dir;
        self.stream_password.clear();
        self.stream_password_error = None;
        self.broadcaster.stream_status = Status::default();
        self.confirm_profile_delete = false;
        self.audio_dirty = true;
        #[cfg(target_os = "linux")]
        {
            self.keyring_notice = None;
            self.keyring_task = Some(KeyringRequest::Load.spawn(self.stream_profile_id()));
        }
        #[cfg(not(target_os = "linux"))]
        match crate::credentials::unprotect(&self.stream_settings.protected_password) {
            Ok(password) => self.stream_password = password,
            Err(error) => self.stream_password_error = Some(error),
        }
    }

    fn delete_stream_profile(&mut self) {
        self.stream_profiles.remove(self.active_stream_profile);
        let index = self.active_stream_profile.min(self.stream_profiles.len() - 1);
        self.restore_stream_profile(index);
    }

    fn stream_profile_ui(&mut self, ui: &mut egui::Ui) {
        let mut index = self.active_stream_profile;
        ui.horizontal(|ui| {
            ui.label("Stream profile");
            egui::ComboBox::from_id_salt("stream-profile")
                .selected_text(&self.stream_profiles[index].name)
                .show_ui(ui, |ui| {
                    for (item, profile) in self.stream_profiles.iter().enumerate() {
                        ui.selectable_value(&mut index, item, &profile.name);
                    }
                });
            if ui.button("New profile").clicked()
                && let Some(id) = self.stream_profile_serial.checked_add(1)
            {
                self.stream_profile_serial = id;
                self.stream_profiles.push(StreamProfile {
                    id,
                    name: format!("Stream {id}"),
                    settings: StreamSettings::default(),
                });
                index = self.stream_profiles.len() - 1;
            }
            if ui.add_enabled(self.stream_profiles.len() > 1, egui::Button::new("Delete profile")).clicked() {
                self.confirm_profile_delete = true;
            }
        });
        self.select_stream_profile(index);
        let profile = &mut self.stream_profiles[self.active_stream_profile];
        ui.horizontal(|ui| {
            ui.label("Profile name");
            if ui.text_edit_singleline(&mut profile.name).changed() {
                self.audio_dirty = true;
            }
        });
        if self.confirm_profile_delete {
            ui.horizontal(|ui| {
                ui.label("Delete this profile and its saved password?");
                if ui.button("Delete").clicked() {
                    self.confirm_profile_delete = false;
                    #[cfg(target_os = "linux")]
                    {
                        self.keyring_task = Some(KeyringRequest::DeleteProfile.spawn(self.stream_profile_id()));
                    }
                    #[cfg(not(target_os = "linux"))]
                    self.delete_stream_profile();
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_profile_delete = false;
                }
            });
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
                ui.add_enabled_ui(!stream_busy && self.keyring_ready(), |ui| self.stream_profile_ui(ui));
                ui.add_enabled_ui(!stream_busy, |ui| {
                    ui.spacing_mut().text_edit_width = 360.0;
                    egui::Grid::new("shoutcast-config").num_columns(2).spacing([14.0, 7.0]).show(ui, |ui| {
                        ui.label("Transport security");
                        egui::ComboBox::from_id_salt("stream-security").selected_text(self.stream_settings.security.label()).show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.stream_settings.security, ConnectionSecurity::Tls, ConnectionSecurity::Tls.label());
                            ui.selectable_value(&mut self.stream_settings.security, ConnectionSecurity::Plaintext, ConnectionSecurity::Plaintext.label());
                        }); ui.end_row();
                        ui.label("Server / IP"); ui.text_edit_singleline(&mut self.stream_settings.host).on_hover_text("Hostname or IP address without http:// or a path"); ui.end_row();
                        ui.label("Source port"); ui.add(egui::DragValue::new(&mut self.stream_settings.source_port).range(1..=u16::MAX)); ui.end_row();
                        ui.label("Metadata/admin port"); ui.add(egui::DragValue::new(&mut self.stream_settings.admin_port).range(1..=u16::MAX)); ui.end_row();
                        ui.label("Stream ID (DNAS 2)"); ui.add(egui::DragValue::new(&mut self.stream_settings.stream_id).range(1..=u32::MAX)); ui.end_row();
                        ui.label("Source password");
                        password_changed = ui.add_enabled(
                            self.keyring_ready(),
                            egui::TextEdit::singleline(&mut self.stream_password).password(true),
                        ).on_hover_text(crate::credentials::PASSWORD_HINT).changed();
                        ui.end_row();
                        ui.label("Station name"); ui.text_edit_singleline(&mut self.stream_settings.station); ui.end_row();
                        ui.label("Genre (optional)"); ui.text_edit_singleline(&mut self.stream_settings.genre); ui.end_row();
                        ui.label("Station URL (optional)"); ui.text_edit_singleline(&mut self.stream_settings.url); ui.end_row();
                    });
                    #[cfg(target_os = "linux")]
                    {
                        if password_changed {
                            self.keyring_notice = None;
                        }
                        self.keyring_controls_ui(ui);
                    }
                });
                ui.add_enabled_ui(!stream_busy, |ui| {
                    ui.checkbox(&mut self.stream_settings.reconnect, "Reconnect after a connection drop")
                        .on_hover_text("Retries a lost live connection up to five times.");
                });
                if self.stream_settings.security == ConnectionSecurity::Plaintext {
                    ui.colored_label(WARNING, "UNENCRYPTED: the source password and metadata password are exposed on the network. Use only on a trusted local or separately encrypted connection.");
                } else {
                    ui.label(RichText::new("TLS protects both source login/audio and HTTPS metadata. The certificate and hostname are verified. Use your provider's TLS-enabled ports; there is no plaintext fallback.").small().color(MUTED));
                }
                if let Some(error) = &self.stream_password_error { ui.colored_label(WARNING, error); }
                ui.horizontal(|ui| {
                    ui.label("MP3 bitrate for new sessions");
                    egui::ComboBox::from_id_salt("stream-bitrate").selected_text(format!("{} kbit/s", self.stream_settings.bitrate)).show_ui(ui, |ui| {
                        for bitrate in [128, 192, 256, 320] { ui.selectable_value(&mut self.stream_settings.bitrate, bitrate, format!("{bitrate} kbit/s")); }
                    });
                });
                ui.label(RichText::new("SHOUTcast 1-compatible source protocol, including DNAS 2 stream IDs. TLS requires a compatible source endpoint or TLS proxy. Legacy ports 8001/8000 may not support TLS; use your provider's settings.").small().color(MUTED));
                ui.separator();
                ui.checkbox(&mut self.stream_settings.send_tags, "Send track titles to the server");
                ui.add_enabled_ui(self.stream_settings.send_tags, |ui| {
                    ui.checkbox(&mut self.stream_settings.automatic_title, "Automatic: title of the playing deck with the highest master contribution");
                    if !self.stream_settings.automatic_title {
                        ui.horizontal(|ui| { ui.label("Manual title"); ui.text_edit_singleline(&mut self.stream_settings.manual_title); });
                    }
                });
                ui.horizontal(|ui| {
                    self.stream_control_ui(ui);
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
                    self.recording_control_ui(ui);
                    let status = &self.broadcaster.recording_status;
                    ui.label(RichText::new(format!("REC · {} · {} · {:.2} MB", status.label(), clock(Duration::from_secs_f64(status.seconds)), status.bytes as f64 / 1_000_000.0)).color(if status.phase == Phase::Error { WARNING } else { LIGHT_PURPLE }));
                });
                if !self.broadcaster.recording_status.message.is_empty() { ui.label(&self.broadcaster.recording_status.message); }
                if let Some(path) = &self.broadcaster.recording_status.path { ui.label(RichText::new(path.display().to_string()).small()); }
                ui.label(RichText::new("Streaming and recording can run independently. New MP3 files have unique names. Stop finalizes the recording. Stop both sessions before changing audio outputs.").small().color(MUTED));
                if !master_ready { ui.colored_label(WARNING, "Select an available master output under Audio / Outputs first."); }
            });
        self.show_streamer = open;
        if password_changed && cfg!(windows) {
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

    fn refresh_input_devices(&mut self, ctx: &egui::Context) {
        if self.input_scan.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(inputs::devices());
            ctx.request_repaint();
        });
        self.input_scan = Some(receiver);
        self.input_scan_error = None;
    }

    fn start_input(&mut self, index: usize) {
        self.input_errors[index] = match LiveInput::open(&self.input_settings[index], &self.audio) {
            Ok(input) => {
                self.live_inputs[index] = Some(input);
                None
            }
            Err(error) => Some(error),
        };
    }

    fn stop_input(&mut self, index: usize) {
        self.live_inputs[index] = None;
        if index == MIC {
            self.mic_peak = 0.0;
        }
    }

    fn update_inputs(&mut self, meter_decay: f32) {
        self.mic_peak *= meter_decay;
        let mic_peak = self.live_inputs[MIC].as_ref()
            .filter(|input| !input.disconnected())
            .map(|input| input.take_peak() * self.input_settings[MIC].level.clamp(0.0, 1.0));
        self.mic_peak = self.mic_peak.max(mic_peak.unwrap_or(0.0));
        self.talkover.update(self.talkover_settings, mic_peak, Instant::now());
        for (index, live) in self.live_inputs.iter_mut().enumerate() {
            let Some(input) = live else { continue };
            if input.disconnected() {
                *live = None;
                if index == MIC {
                    self.mic_peak = 0.0;
                }
                self.input_errors[index] = Some("Audio input disconnected. Reconnect it and enable the input again.".into());
                continue;
            }
            let mut gain = self.input_settings[index].level.clamp(0.0, 1.0);
            if index != MIC {
                gain *= self.talkover.gain();
            }
            input.set_gain(gain * self.master, gain * self.phones * self.mixing);
        }
    }

    fn inputs_ui(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.input_scan {
            match receiver.try_recv() {
                Ok(result) => {
                    self.input_scan = None;
                    match result {
                        Ok(devices) => self.input_devices = devices,
                        Err(error) => self.input_scan_error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.input_scan = None;
                    self.input_scan_error = Some("Could not list audio inputs.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut open = self.show_inputs;
        egui::Window::new("Audio Inputs").open(&mut open).default_width(570.0).max_height(ctx.content_rect().height() - 70.0).vscroll(true).show(ctx, |ui| {
            ui.label("Enabled inputs feed the master, stream and recording.");
            ui.horizontal(|ui| {
                if ui.button("Refresh devices").clicked() {
                    self.refresh_input_devices(ctx);
                }
                if self.input_scan.is_some() {
                    ui.spinner();
                }
            });
            if let Some(error) = &self.input_scan_error {
                ui.colored_label(WARNING, error);
            }
            for index in 0..self.input_settings.len() {
                ui.separator();
                ui.push_id(index, |ui| {
                    let active = self.live_inputs[index].is_some();
                    ui.horizontal(|ui| {
                        if index == MIC {
                            ui.strong("Microphone");
                        } else {
                            ui.strong(format!("Input {}", index + 1));
                        }
                        if active {
                            if ui.button("Disable input").clicked() {
                                self.stop_input(index);
                            } else if let Some(input) = &self.live_inputs[index] {
                                let peak = if index == MIC { self.mic_peak } else { input.take_peak() };
                                ui.label(format!("ACTIVE · {}", peak_text(peak)));
                            }
                        } else if ui.button("Enable input").clicked() {
                            self.start_input(index);
                        }
                    });
                    let settings = &mut self.input_settings[index];
                    let original_device = settings.device.clone();
                    ui.add_enabled_ui(!active, |ui| {
                        egui::ComboBox::from_id_salt("device")
                            .width(ui.available_width().min(510.0))
                            .selected_text(settings.device.as_deref().unwrap_or("System default input"))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut settings.device, None, "System default input");
                                for device in &self.input_devices {
                                    ui.selectable_value(&mut settings.device, Some(device.name.clone()), &device.name);
                                }
                            });
                        if original_device != settings.device {
                            settings.channel = 0;
                            self.audio_dirty = true;
                        }
                        let channels = self.input_devices.iter()
                            .find(|device| Some(device.name.as_str()) == settings.device.as_deref())
                            .map_or(32, |device| device.channels);
                        ui.horizontal(|ui| {
                            let mut stereo = settings.stereo;
                            if ui.add_enabled(channels > 1 || stereo, egui::Checkbox::new(&mut stereo, "Stereo pair")).changed() {
                                settings.stereo = stereo;
                                self.audio_dirty = true;
                            }
                            let max_first = channels.saturating_sub(u16::from(settings.stereo)).max(1);
                            let mut channel = settings.channel.saturating_add(1);
                            ui.label(if settings.stereo { "First channel" } else { "Channel" });
                            if ui.add(egui::DragValue::new(&mut channel).range(1..=max_first)).changed() {
                                settings.channel = channel - 1;
                                self.audio_dirty = true;
                            }
                        });
                    });
                    if ui.add(egui::Slider::new(&mut settings.level, 0.0..=1.0).text("Input level")).changed() {
                        self.audio_dirty = true;
                    }
                    if let Some(error) = &self.input_errors[index] {
                        ui.colored_label(WARNING, error);
                    }
                });
            }
        });
        self.show_inputs = open;
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

    fn play_jingle(&mut self, pad: usize) {
        if let Err(error) = self.jingles.play(pad, &self.audio, self.master) {
            self.notice = Some(error);
        }
    }

    fn jingles_ui(&mut self, ctx: &egui::Context) {
        if !self.show_jingles {
            return;
        }
        let mut open = self.show_jingles;
        egui::Window::new("Jingles")
            .open(&mut open)
            .default_width(495.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Slider::new(&mut self.jingles.settings.level, 0.0..=1.0)
                                .text("Jingle level"),
                        )
                        .changed()
                    {
                        self.audio_dirty = true;
                    }
                    if ui.button("Stop all").clicked() {
                        self.jingles.stop_all();
                    }
                });
                for pad in 0..jingles::COUNT {
                    ui.push_id(pad, |ui| {
                        ui.separator();
                        ui.horizontal(|ui| {
                            let file = self.jingles.settings.paths[pad].as_deref();
                            let label = file
                                .and_then(|path| path.file_stem())
                                .map(|name| name.to_string_lossy().into_owned());
                            if ui
                                .add_enabled(
                                    file.is_some(),
                                    egui::Button::new(format!("Play {}", pad + 1))
                                        .selected(self.jingles.playing(pad)),
                                )
                                .clicked()
                            {
                                self.play_jingle(pad);
                            }
                            if ui
                                .add_enabled(self.jingles.playing(pad), egui::Button::new("Stop"))
                                .clicked()
                            {
                                self.jingles.stop(pad);
                            }
                            if ui.button("Choose file …").clicked()
                                && let Some(path) = rfd::FileDialog::new()
                                    .add_filter("Audio", track::EXTENSIONS)
                                    .pick_file()
                            {
                                self.jingles.stop(pad);
                                self.jingles.settings.paths[pad] = Some(path);
                                self.audio_dirty = true;
                            }
                            if ui
                                .add_enabled(
                                    self.jingles.settings.paths[pad].is_some(),
                                    egui::Button::new("Clear"),
                                )
                                .clicked()
                            {
                                self.jingles.stop(pad);
                                self.jingles.settings.paths[pad] = None;
                                self.audio_dirty = true;
                            }
                            ui.add(
                                egui::Label::new(label.as_deref().unwrap_or("Empty pad"))
                                    .truncate(),
                            );
                        });
                    });
                }
            });
        self.show_jingles = open;
    }

    fn talkover_ui(&mut self, ctx: &egui::Context) {
        egui::Window::new("Talkover")
            .open(&mut self.show_performance)
            .default_width(385.0)
            .show(ctx, |ui| {
                let settings = &mut self.talkover_settings;
                self.audio_dirty |= ui
                    .checkbox(&mut settings.enabled, "Lower music while speaking")
                    .changed();
                ui.add_enabled_ui(settings.enabled, |ui| {
                    self.audio_dirty |= ui
                        .add(
                            egui::Slider::new(&mut settings.threshold_db, -60.0..=-10.0)
                                .text("Mic threshold")
                                .suffix(" dBFS"),
                        )
                        .changed();
                    self.audio_dirty |= ui
                        .add(
                            egui::Slider::new(&mut settings.reduction_db, 3.0..=24.0)
                                .text("Music reduction")
                                .suffix(" dB"),
                        )
                        .changed();
                    self.audio_dirty |= ui
                        .add(
                            egui::Slider::new(&mut settings.release_ms, 80..=2000)
                                .text("Return time")
                                .suffix(" ms"),
                        )
                        .changed();
                });
                if settings.enabled && self.talkover.gain() < 0.99 {
                    ui.colored_label(
                        LIGHT_PURPLE,
                        format!("Talkover · {:.1} dB", peak_db(self.talkover.gain())),
                    );
                }
            });
    }

    fn log_audible_tracks(&mut self, gains: &crate::dsp::BusGains<COUNT>, now: Instant) {
        for (index, deck) in self.decks.iter().enumerate() {
            if self.history_logged[index] {
                continue;
            }
            if !deck.program_playing() || gains.master[index] < 0.001 {
                self.audible_since[index] = None;
                continue;
            }
            let started = *self.audible_since[index].get_or_insert(now);
            let elapsed = now.saturating_duration_since(started);
            if elapsed >= Duration::from_secs(2)
                && let Some(track) = &deck.track
            {
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .saturating_sub(elapsed)
                    .as_secs() as i64;
                self.hotcue_store.play(
                    track.title.clone(),
                    track.path.clone(),
                    index + 1,
                    timestamp,
                );
                self.history_logged[index] = true;
            }
        }
    }

    fn history_ui(&mut self, ctx: &egui::Context) {
        if !self.show_history {
            return;
        }
        let now = Instant::now();
        if now >= self.history_refresh {
            self.history_refresh = now + Duration::from_secs(2);
            match Database::open(&database::default_path()).and_then(|db| db.history(500)) {
                Ok(entries) => self.history = entries,
                Err(error) => self.notice = Some(error),
            }
        }
        egui::Window::new("Set history")
            .open(&mut self.show_history)
            .default_width(580.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Export CSV …").clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .set_file_name("RustDJ-set-history.csv")
                            .add_filter("CSV", &["csv"])
                            .save_file()
                    {
                        self.notice = Some(match export_history(&path) {
                            Ok(()) => format!("Set history saved to {}", path.display()),
                            Err(error) => error,
                        });
                    }
                    ui.label("Last 500 plays");
                });
                ui.separator();
                egui::ScrollArea::vertical().max_height(410.0).show_rows(
                    ui,
                    19.0,
                    self.history.len(),
                    |ui, rows| {
                        for entry in &self.history[rows] {
                            ui.horizontal(|ui| {
                                ui.monospace(&entry.local_time);
                                ui.label(format!("Deck {}", entry.deck));
                                ui.add(egui::Label::new(&entry.title).truncate())
                                    .on_hover_text(&entry.path);
                            });
                        }
                    },
                );
                if self.history.is_empty() {
                    ui.label("Tracks appear after two seconds in the master mix.");
                }
            });
    }

    fn open_grid_editor(&mut self, index: usize) {
        let deck = &self.decks[index];
        if let Some(track) = &deck.track {
            let mut grid = deck.grid.unwrap_or(BeatGrid {
                bpm: 120.0,
                offset: 0.0,
                confidence: 1.0,
            });
            grid.bpm = grid.bpm.clamp(40.0, 250.0);
            self.grid_editor = Some(GridEditor {
                deck: index,
                path: track.path.clone(),
                grid,
                error: None,
            });
        }
    }

    fn grid_editor_ui(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.grid_editor.take() else {
            return;
        };
        let deck = &mut self.decks[editor.deck];
        let Some(track) = deck
            .track
            .as_ref()
            .filter(|track| track.path == editor.path)
        else {
            return;
        };
        let duration = track.duration.as_secs_f64();
        let last = (duration - 0.001).max(0.0);
        let title = track.title.clone();
        let mut open = true;
        let mut apply = None;
        egui::Window::new("Beatgrid editor")
            .open(&mut open)
            .default_width(540.0)
            .show(ctx, |ui| {
                ui.heading(format!("Deck {} · {title}", editor.deck + 1));
                let position = deck.position().as_secs_f64();
                ui.label(format!("Playhead {}", clock(deck.position())));
                ui.horizontal(|ui| {
                    ui.label("Tempo");
                    ui.add(
                        egui::DragValue::new(&mut editor.grid.bpm)
                            .range(40.0..=250.0)
                            .speed(0.01)
                            .fixed_decimals(2)
                            .suffix(" BPM"),
                    );
                    ui.label("First beat");
                    ui.add(
                        egui::DragValue::new(&mut editor.grid.offset)
                            .range(0.0..=last)
                            .speed(0.001)
                            .fixed_decimals(3)
                            .suffix(" s"),
                    );
                });
                if !editor.grid.bpm.is_finite() {
                    editor.grid.bpm = 120.0;
                }
                if !editor.grid.offset.is_finite() {
                    editor.grid.offset = 0.0;
                }
                ui.horizontal(|ui| {
                    if ui.button("Set from playhead").clicked() {
                        editor.grid.offset = position.min(last);
                    }
                    if ui.button("−10 ms").clicked() {
                        editor.grid.offset = (editor.grid.offset - 0.01).max(0.0);
                    }
                    if ui.button("+10 ms").clicked() {
                        editor.grid.offset = (editor.grid.offset + 0.01).min(last);
                    }
                    if ui.button("Go to first beat").clicked() {
                        deck.seek(Duration::from_secs_f64(editor.grid.offset), &self.audio);
                    }
                });
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
                let painter = ui.painter().with_clip_rect(rect);
                painter.rect_filled(rect, 3.0, BG);
                let first = editor.grid.offset;
                let period = editor.grid.period();
                let start = first - period;
                let end = first + period * 3.0;
                let middle = rect.left() + rect.width() * 0.25;
                if let Some(track) = &deck.track {
                    let step = (end - start) / rect.width().max(1.0) as f64;
                    for column in 0..rect.width() as usize {
                        let time = start + column as f64 * step;
                        if let Some(frame) = track.detailed.sample(time, time + step) {
                            spectral_column(
                                &painter,
                                rect,
                                rect.left() + column as f32,
                                frame,
                                false,
                            );
                        }
                    }
                }
                for beat in 0..4 {
                    let x = rect.left() + rect.width() * beat as f32 * 0.25;
                    painter.vline(
                        x,
                        rect.y_range(),
                        Stroke::new(1.0_f32, if beat == 1 { LIGHT_PURPLE } else { BORDER }),
                    );
                }
                painter.circle_filled(Pos2::new(middle, rect.top() + 4.0), 3.0, LIGHT_PURPLE);
                ui.horizontal(|ui| {
                    if ui.button("Save beatgrid").clicked() {
                        apply = Some(Some(editor.grid));
                    }
                    if ui.button("Use detected grid").clicked() {
                        apply = Some(None);
                    }
                });
                if let Some(error) = &editor.error {
                    ui.colored_label(WARNING, error);
                }
            });
        if let Some(grid) = apply {
            match deck.save_grid(grid) {
                Ok(()) => {
                    for (index, deck) in self.decks.iter_mut().enumerate() {
                        if let Some(track) = deck
                            .track
                            .as_ref()
                            .filter(|track| track.path == editor.path)
                        {
                            deck.grid = grid.or(track.grid);
                            deck.exit_loop();
                            self.synced[index] = None;
                            for follower in &mut self.synced {
                                if *follower == Some(index) {
                                    *follower = None;
                                }
                            }
                        }
                    }
                    self.notice = Some("Beatgrid updated.".into());
                    open = false;
                }
                Err(error) => editor.error = Some(error),
            }
        }
        if open {
            self.grid_editor = Some(editor);
        }
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
                audio.limiter.set(self.master_limiter);
                self.jingles.stop_all();
                let autodj_was_running = self.autodj.stop(&mut self.decks, &self.audio);
                let mut errors = Vec::new();
                for deck in &mut self.decks {
                    if let Err(error) = deck.rebind(&audio) {
                        deck.disconnect();
                        deck.error = Some(error.clone());
                        errors.push(error);
                    }
                }
                self.audio = audio;
                for index in 0..self.live_inputs.len() {
                    if self.live_inputs[index].take().is_some() {
                        self.start_input(index);
                    }
                }
                self.routing_edit = self.audio.routing.clone();
                self.audio_dirty = true;
                if autodj_was_running {
                    errors.push("AutoDJ stopped after changing audio outputs.".into());
                }
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
        .selected_text(target.device.as_deref().unwrap_or("System default"))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut target.device, None, "System default");
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
        Some(None) => "System default",
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
            ui.selectable_value(&mut choice, Some(None), "System default");
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
        let meter_decay = 0.86_f32.powf(ctx.input(|i| i.unstable_dt.max(0.0)) / 0.033);
        let levels_before = (
            self.master,
            self.phones,
            self.mixing,
            self.crossfader.enabled,
        );
        self.limiter_reduction = (self.limiter_reduction * meter_decay)
            .max(self.audio.limiter.take_reduction());
        let (master_peaks, phones_peaks) = self.audio.levels();
        for (bus, peaks) in [master_peaks, phones_peaks].into_iter().enumerate() {
            for (channel, value) in peaks.into_iter().enumerate() {
                self.bus_levels[bus][channel] = value
                    .max(self.bus_levels[bus][channel] * meter_decay)
                    .min(8.0);
            }
        }
        if let Some(error) = self.hotcue_store.error() {
            self.notice = Some(error);
        }
        self.broadcaster.poll();
        #[cfg(target_os = "linux")]
        self.poll_keyring();
        self.browser.poll();
        self.midi.tick(ctx);
        let mut loaded_this_frame = [None; COUNT];
        for (index, deck) in self.decks.iter_mut().enumerate() {
            if let Some(track) = deck.poll(&self.audio) {
                loaded_this_frame[index] = Some(deck.load_serial());
                self.browser.on_track(&track);
                self.playlist.on_track(&track);
                self.synced[index] = None;
                for follower in &mut self.synced {
                    if *follower == Some(index) {
                        *follower = None;
                    }
                }
                self.taps[index].clear();
                self.audible_since[index] = None;
                self.history_logged[index] = false;
                upsert_session(&mut self.library, Item::from_track(&track));
            }
        }
        self.midi_actions(ctx);
        self.shortcuts(ctx);
        egui::TopBottomPanel::top("app-header")
            .frame(egui::Frame::new().fill(BG).inner_margin(6.0))
            .show(ctx, |ui| {
                self.header(ui, ctx);
            });
        let deck_order = self.deck_layout.order();
        egui::TopBottomPanel::bottom("footer").frame(egui::Frame::new().fill(PANEL).inner_margin(4.0)).show(ctx, |ui| {
            if let Some(error) = &self.audio.error {
                ui.add(egui::Label::new(RichText::new(error).small().color(WARNING)).truncate()).on_hover_text(error);
            } else if let Some(notice) = &self.notice {
                ui.add(egui::Label::new(RichText::new(notice).small().color(MUTED)).truncate()).on_hover_text(notice);
            } else {
                let keys = match self.deck_layout {
                    DeckLayout::Two => "1 / 2",
                    DeckLayout::Four => "1 / 2 / 3 / 4",
                };
                ui.label(RichText::new(format!("{keys}  Select deck     SPACE  Play/Pause     S  Sync     C  Cue     ESC  Stop     Left / Right  Crossfader")).size(10.0).color(MUTED));
            }
        });
        if !self.library_view {
            egui::TopBottomPanel::top("beat-waveforms")
                .resizable(true)
                .default_height(114.0)
                .height_range(100.0..=180.0)
                .frame(egui::Frame::new().fill(BG).inner_margin(5.0))
                .show(ctx, |ui| {
                    self.waveforms_ui(ui);
                });
        }
        let mut bounds = [Rect::NOTHING; COUNT];
        let mut drops = Zones::default();
        let mut responses = [DeckResponse::default(); COUNT];
        if !self.library_view {
            egui::TopBottomPanel::top("console")
                .frame(egui::Frame::new().fill(PANEL).inner_margin(5.0))
                .show(ctx, |ui| {
                    let (mixer_width, deck_width) = console_dimensions(ui.available_width(), self.deck_layout);
                    ui.horizontal_top(|ui| {
                        for (slot, &index) in deck_order.iter().enumerate() {
                            if slot == deck_order.len() / 2 {
                                ui.allocate_ui_with_layout(
                                    Vec2::new(mixer_width, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| self.mixer_ui(ui, mixer_width),
                                );
                            }
                            let card = ui.allocate_ui_with_layout(
                                Vec2::new(deck_width, 0.0),
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
        }
        for &index in deck_order {
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
            if response.stopped {
                self.stop_deck(index);
            }
            if response.edit_grid {
                self.open_grid_editor(index);
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
                    let tab = &mut self.library_tab;
                    let session = ui.selectable_value(tab, LibraryTab::Session, "SESSION");
                    drops.add(DropTarget::Session, session.rect, ui.layer_id());
                    let playlist = ui.selectable_value(tab, LibraryTab::Playlist, "PLAYLIST");
                    drops.add(DropTarget::Playlist(None), playlist.rect, ui.layer_id());
                    let autodj = ui.selectable_value(tab, LibraryTab::AutoDj, "AutoDJ");
                    drops.add(DropTarget::AutoDj(None), autodj.rect, ui.layer_id());
                    if playlist.clicked() {
                        self.playlist.refresh(ctx);
                    }
                });
                ui.separator();
                drops.add(
                    match self.library_tab {
                        LibraryTab::Session => DropTarget::Session,
                        LibraryTab::Playlist => DropTarget::Playlist(None),
                        LibraryTab::AutoDj => DropTarget::AutoDj(None),
                    },
                    ui.available_rect_before_wrap(),
                    ui.layer_id(),
                );
                match self.library_tab {
                    LibraryTab::Session => self.library_ui(ui, ctx),
                    LibraryTab::Playlist => {
                        if let Some((index, path)) =
                            self.playlist
                                .ui(ui, ctx, self.active, deck_order, &mut drops)
                        {
                            self.decks[index].load(path, ctx.clone());
                            self.active = index;
                        }
                    }
                    LibraryTab::AutoDj => {
                        let gains = self.mix_gains();
                        let control = self.autodj.controls(
                            ui,
                            deck_order,
                            &self.decks,
                            self.audio.stream.is_some() && self.audio.bus(false).is_some(),
                            &gains.master,
                        );
                        self.audio_dirty |= self.autodj.ui(
                            ui,
                            self.browser.selected_paths(),
                            &self.playlist.items,
                            &mut drops,
                        );
                        match control {
                            Some(AutoDjControl::Start(deck)) => {
                                if self.autodj.start(deck, &mut self.decks[deck], ctx) {
                                    self.active = deck;
                                    self.library_view = false;
                                }
                            }
                            Some(AutoDjControl::Next) => {
                                self.autodj.next(&mut self.decks, &self.audio, ctx);
                            }
                            Some(AutoDjControl::Stop) => {
                                self.autodj.stop(&mut self.decks, &self.audio);
                            }
                            None => {}
                        }
                    }
                }
            });
        let deck_loading = self.decks.iter().any(Deck::loading);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PANEL).inner_margin(6.0))
            .show(ctx, |ui| {
                match self
                    .browser
                    .ui(ui, self.active, deck_order, ctx, deck_loading)
                {
                    Some(BrowserEvent::Load(index, path)) => {
                        self.decks[index].load(path, ctx.clone());
                        self.active = index;
                    }
                    Some(BrowserEvent::Playlist(paths)) => {
                        self.playlist.add(paths, ctx);
                        self.library_tab = LibraryTab::Playlist;
                    }
                    Some(BrowserEvent::AutoDj(paths)) => {
                        self.audio_dirty |= self.autodj.add(paths);
                        self.library_tab = LibraryTab::AutoDj;
                    }
                    None => {}
                }
            });
        self.controller_ui(ctx);
        self.audio_ui(ctx);
        self.inputs_ui(ctx);
        self.update_inputs(meter_decay);
        self.jingles_ui(ctx);
        self.talkover_ui(ctx);
        self.history_ui(ctx);
        self.grid_editor_ui(ctx);
        self.jingles.update(self.master);
        self.playlist.confirm_changes(ctx);
        if let Some((target, music)) = drops.finish(ctx) {
            match target {
                DropTarget::Deck(index) => {
                    self.load_dropped_tracks(index, music.items.into_iter().map(|item| item.path), ctx);
                }
                DropTarget::Session => {
                    for item in music.items {
                        upsert_session(&mut self.library, item);
                    }
                    self.library_tab = LibraryTab::Session;
                }
                DropTarget::Playlist(index) => {
                    if let Origin::Playlist(range) = music.origin {
                        self.playlist.move_to(range, index);
                    } else {
                        self.playlist.insert_many(music.items, index, ctx);
                    }
                    self.library_tab = LibraryTab::Playlist;
                }
                DropTarget::AutoDj(index) => {
                    self.audio_dirty |= if let Origin::AutoDj(range) = music.origin {
                        self.autodj.move_to(range, index)
                    } else {
                        self.autodj.insert(music.items.into_iter().map(|item| item.path).collect(), index)
                    };
                    self.library_tab = LibraryTab::AutoDj;
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
                    self.load_dropped_tracks(index, external_music, ctx);
                }
                Some(DropTarget::Session) => {
                    for path in external_music {
                        upsert_session(&mut self.library, Item::new(path, None));
                    }
                    self.library_tab = LibraryTab::Session;
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
                    self.library_tab = LibraryTab::Playlist;
                }
                Some(DropTarget::AutoDj(index)) => {
                    self.audio_dirty |= self.autodj.insert(external_music, index);
                    self.library_tab = LibraryTab::AutoDj;
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
        let now = Instant::now();
        for (index, deck) in self.decks.iter_mut().enumerate() {
            deck.apply_buses_at(gains.master[index], gains.phones[index], meter_decay, now);
            if loaded_this_frame[index] == Some(deck.load_serial()) {
                self.autodj.on_loaded(index, deck, &self.audio);
            }
            if let Some(snapshot) = deck.take_hotcue_changes() {
                self.hotcue_store.save(snapshot);
            }
        }
        self.autodj.update(&mut self.decks, ctx);
        self.log_audible_tracks(&gains, now);
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
                if !self.deck_layout.contains(index) {
                    return midi::DeckLeds::default();
                }
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
            self.midi.save_current_profile();
            self.stream_profiles[self.active_stream_profile].settings = self.stream_settings.clone();
            let mut legacy = self.stream_profiles.iter()
                .find(|profile| profile.id == 0)
                .map_or_else(StreamSettings::default, |profile| profile.settings.clone());
            legacy.recording_dir = self.stream_settings.recording_dir.clone();
            let settings = Settings {
                browser: self.browser.settings.clone(),
                deck_layout: self.deck_layout,
                bindings: Some(self.midi.bindings.clone()),
                routing: self.audio.routing.clone(),
                mix: MixSettings {
                    master: self.master,
                    phones: self.phones,
                    mixing: self.mixing,
                    crossfader_enabled: self.crossfader.enabled,
                },
                mapping_version: 7,
                midi: self.midi.prefs.clone(),
                streamer: legacy,
                stream_profiles: self.stream_profiles.clone(),
                active_stream_profile: self.stream_profile_id(),
                stream_profile_serial: self.stream_profile_serial,
                inputs: [self.input_settings[0].clone(), self.input_settings[1].clone()],
                microphone: self.input_settings[MIC].clone(),
                talkover: self.talkover_settings,
                master_limiter: self.master_limiter,
                jingles: self.jingles.settings.clone(),
                master_eq: self.master_eq,
                autodj: self.autodj.paths().to_vec(),
            };
            if let Err(error) = settings::save(&settings) {
                self.notice = Some(error);
            }
            self.browser.dirty = false;
            self.midi.dirty = false;
            self.audio_dirty = false;
        }
        if self.decks.iter().any(Deck::playing) {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
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

fn export_history(path: &std::path::Path) -> Result<(), String> {
    use std::io::{BufWriter, Write};

    fn csv(value: &str) -> String {
        let escaped = value.replace('"', r#""""#);
        let prefix = if value.trim_start().starts_with(['=', '+', '-', '@']) {
            "'"
        } else {
            ""
        };
        format!(r#""{prefix}{escaped}""#)
    }

    let entries = Database::open(&database::default_path())?.history(-1)?;
    let mut file = BufWriter::new(std::fs::File::create(path).map_err(|error| error.to_string())?);
    writeln!(file, "played_at_utc,title,deck,path").map_err(|error| error.to_string())?;
    for entry in entries.iter().rev() {
        writeln!(
            file,
            "{},{},{},{}",
            entry.utc_time,
            csv(&entry.title),
            entry.deck,
            csv(&entry.path)
        )
        .map_err(|error| error.to_string())?;
    }
    file.flush().map_err(|error| error.to_string())
}

fn broadcast_status_ui(ui: &mut egui::Ui, name: &str, status: &Status, active: &str, color: Color32) {
    let phase = if status.phase == Phase::Running {
        active
    } else {
        status.label()
    };
    let mut text = format!("{name} · {phase}");
    if status.seconds > 0.0 {
        let total_seconds = status.seconds as u64;
        let (minutes, seconds) = (total_seconds / 60, total_seconds % 60);
        text.push_str(&format!(" · {minutes:02}:{seconds:02}"));
    }
    let color = match status.phase {
        Phase::Error | Phase::Reconnecting => {
            text.push_str(" · ");
            text.push_str(&status.message);
            WARNING
        }
        Phase::Idle | Phase::Done => MUTED,
        _ => color,
    };
    let response = ui.add(egui::Label::new(RichText::new(&text).strong().color(color)).truncate());
    let mut details = text;
    if !matches!(status.phase, Phase::Error | Phase::Reconnecting) && !status.message.is_empty() {
        details.push('\n');
        details.push_str(&status.message);
    }
    if let Some(path) = &status.path {
        details.push('\n');
        details.push_str(&path.to_string_lossy());
    }
    if let Some(error) = &status.metadata_error {
        details.push('\n');
        details.push_str(error);
    }
    response.on_hover_text(details);
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
    edit_grid: bool,
    sync: bool,
    tap: bool,
    toggled: bool,
    stopped: bool,
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
                    if ui.add_enabled(deck.track.is_some(), egui::Button::new("Beatgrid editor …")).clicked() {
                        response.edit_grid = true;
                        ui.close();
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
                    response.stopped = true;
                }
            });
            response.performance_changed |=
                deck_performance_ui(ui, deck, audio, color, width - 18.0);
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

fn deck_performance_ui(
    ui: &mut egui::Ui,
    deck: &mut Deck,
    audio: &Audio,
    color: Color32,
    width: f32,
) -> bool {
    const EFFECT_WIDTH: f32 = 91.0;
    const GAP: f32 = 6.0;

    // Leave breathing room at the right edge for the widgets' own padding.
    let pads_width = width - EFFECT_WIDTH - GAP - 8.0;
    let side_by_side = pads_width >= 176.0;
    let bank_width = if side_by_side {
        (pads_width - GAP) / 2.0
    } else {
        pads_width
    };
    let pad_width = (bank_width - 9.0) / 4.0;

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        ui.allocate_ui_with_layout(
            Vec2::new(EFFECT_WIDTH, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(3.0, 3.0);
                ui.horizontal(|ui| {
                    fx_knob(ui, "FILTER", &mut deck.filter, -1.0, 1.0, 0.0, color);
                    fx_knob(ui, "FX LEVEL", &mut deck.fx_level, 0.0, 1.0, 0.25, color);
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding.x = 2.0;
                    for (slot, (label, name)) in [("FLG", "Flanger"), ("REV", "Reverb"), ("ECHO", "Echo")]
                        .into_iter().enumerate()
                    {
                        if ui.add(egui::Button::new(label).small().selected(deck.fx_enabled[slot]))
                            .on_hover_text(name)
                            .clicked()
                        {
                            deck.fx_enabled[slot] = !deck.fx_enabled[slot];
                        }
                    }
                });
            },
        );
        let layout = if side_by_side {
            egui::Layout::left_to_right(egui::Align::Min)
        } else {
            egui::Layout::top_down(egui::Align::Min)
        };
        ui.allocate_ui_with_layout(Vec2::new(pads_width, 0.0), layout, |ui| {
            let mut changed = false;
            for (title, hotcues) in [("HOTCUES", true), ("LOOPS · BEATS", false)] {
                ui.allocate_ui_with_layout(
                    Vec2::new(bank_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(3.0, 2.0);
                        ui.label(RichText::new(title).size(9.0).color(MUTED));
                        let enabled = deck.track.is_some() && (hotcues || audio.stream.is_some());
                        for row in 0..2 {
                            ui.horizontal(|ui| {
                                for slot in row * 4..row * 4 + 4 {
                                    let (beats, loop_label) = LOOP_PRESETS[slot];
                                    let saved = deck.hotcues[slot].is_some();
                                    let (label, selected, hint) = if hotcues {
                                        ((slot + 1).to_string(), saved,
                                         "Empty: set. Assigned: jump. Shift-click or right-click to clear.")
                                    } else {
                                        (loop_label.to_owned(), deck.loop_auto
                                            && (deck.loop_active || deck.loop_pending())
                                            && (deck.loop_beats - beats).abs() < 0.001,
                                         "Start the beat loop. Press again to exit.")
                                    };
                                    let button = ui.add_enabled(
                                        enabled,
                                        egui::Button::new(label).small().selected(selected)
                                            .min_size(Vec2::new(pad_width, 17.0)),
                                    ).on_hover_text(hint);
                                    if hotcues {
                                        if button.secondary_clicked()
                                            || (button.clicked() && ui.input(|i| i.modifiers.shift))
                                        {
                                            deck.clear_hotcue(slot);
                                        } else if button.clicked() {
                                            deck.hotcue(slot, audio);
                                            changed |= saved;
                                        }
                                    } else if button.clicked() {
                                        deck.beat_loop(beats, audio);
                                        changed = true;
                                    }
                                }
                            });
                        }
                    },
                );
            }
            changed
        }).inner
    }).inner
}

fn knob_ui(ui: &mut egui::Ui, label: &str, value: &mut f32, color: Color32) {
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.spacing_mut().button_padding = Vec2::new(3.0, 1.0);
        ui.spacing_mut().interact_size.y = 15.0;
        ui.label(RichText::new(label).size(9.0).color(MUTED));
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(24.0, 16.0), Sense::click_and_drag());
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
        ui.painter().circle_filled(center, 7.0, BG);
        ui.painter()
            .circle_stroke(center, 7.0, Stroke::new(1.0_f32, BORDER));
        ui.painter().line_segment(
            [
                center + Vec2::angled(angle) * 3.0,
                center + Vec2::angled(angle) * 6.0,
            ],
            Stroke::new(2.0_f32, color),
        );
        ui.add(
            egui::DragValue::new(value)
                .range(-24.0..=12.0)
                .speed(0.2)
                .fixed_decimals(1)
                ,
        );
    });
}

fn channel_strip(ui: &mut egui::Ui, deck: &mut Deck, color: Color32, index: usize) {
    ui.vertical_centered(|ui| {
        ui.visuals_mut().selection.bg_fill = color.gamma_multiply(0.35);
        ui.visuals_mut().selection.stroke = Stroke::new(1.0_f32, color);
        ui.spacing_mut().item_spacing = Vec2::new(3.0, 2.0);
        ui.label(RichText::new((index + 1).to_string()).strong().color(color));
        ui.columns(2, |columns| {
            for (band, label) in [(2, "HIGH"), (1, "MID"), (0, "LOW")] {
                knob_ui(&mut columns[0], label, &mut deck.eq[band], color);
            }
            knob_ui(&mut columns[1], "GAIN", &mut deck.gain, color);
            columns[1].vertical_centered(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().slider_width = 48.0;
                    ui.add_sized(
                        [18.0, 48.0],
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
                if ui.small_button("↺").on_hover_text("Reset gain and EQ").clicked() {
                    deck.eq = [0.0; 3];
                    deck.gain = 0.0;
                }
            });
        });
    });
}

fn strip_meter_ui(ui: &mut egui::Ui, level: f32, color: Color32) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(14.0, 48.0), Sense::hover());
    response.on_hover_text(format!("Channel level: {}", peak_text(level)));
    let db = peak_db(level);
    for segment in 0..24 {
        let threshold = -60.0 + segment as f32 / 23.0 * 66.0;
        let bottom = rect.bottom() - segment as f32 * rect.height() / 24.0;
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
                Pos2::new(rect.left(), bottom - rect.height() / 24.0 + 1.0),
                Vec2::new(rect.width(), rect.height() / 24.0 - 1.0),
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
    fn mic_mixer_ui(&mut self, ui: &mut egui::Ui) {
        let active = self.live_inputs[MIC].is_some();
        let (text, color) = if self.input_errors[MIC].is_some() {
            ("MIC ERR", WARNING)
        } else if active {
            ("MIC ON", LIGHT_PURPLE)
        } else {
            ("MIC OFF", MUTED)
        };
        let hint = self.input_errors[MIC]
            .as_deref()
            .unwrap_or("Toggle the microphone. Choose its device under Audio Inputs.");
        if ui
            .add_sized(
                [62.0, 17.0],
                egui::Button::new(RichText::new(text).color(color)).small().selected(active),
            )
            .on_hover_text(hint)
            .clicked()
        {
            if active {
                self.stop_input(MIC);
            } else {
                self.start_input(MIC);
            }
        }
        ui.spacing_mut().slider_width = 54.0;
        let level = &mut self.input_settings[MIC].level;
        if ui
            .add(egui::Slider::new(level, 0.0..=1.0).show_value(false))
            .on_hover_text(format!("Microphone level: {:.0}%", *level * 100.0))
            .changed()
        {
            self.audio_dirty = true;
        }
        let display = peak_text(self.mic_peak);
        let fill = if self.mic_peak >= 1.0 { WARNING } else { LIGHT_PURPLE };
        let visible = ((peak_db(self.mic_peak) + 60.0) / 60.0).clamp(0.0, 1.0);
        let (rect, response) = ui.allocate_exact_size(Vec2::new(42.0, 8.0), Sense::hover());
        ui.painter().rect_filled(rect, 2.0, BG);
        if visible > 0.0 {
            ui.painter().rect_filled(
                Rect::from_min_size(rect.min, Vec2::new(rect.width() * visible, rect.height())),
                2.0,
                fill,
            );
        }
        response.on_hover_text(format!("Microphone: {display}"));
    }

    fn mixer_ui(&mut self, ui: &mut egui::Ui, width: f32) {
        let (mut apply_phones, mut refresh_devices, mut show_audio) = (false, false, false);
        card().show(ui, |ui| {
            ui.set_width(width - 18.0);
            ui.spacing_mut().item_spacing.y = 3.0;
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
                let limiting = self.master_limiter && self.limiter_reduction >= 0.01;
                let label = if limiting { format!("LIM {:.1}", peak_db(1.0 - self.limiter_reduction)) } else { "Limiter".into() };
                let color = if limiting { WARNING } else { ui.visuals().text_color() };
                if ui.add(egui::Button::new(RichText::new(label).color(color)).small().selected(self.master_limiter))
                    .on_hover_text("Master limiter: -1 dBFS peak ceiling for speakers, stream and recording.")
                    .clicked() {
                    self.master_limiter = !self.master_limiter;
                    self.audio.limiter.set(self.master_limiter);
                    self.audio_dirty = true;
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
                let order = self.deck_layout.order();
                ui.columns(order.len(), |columns| {
                    for (column, &index) in columns.iter_mut().zip(order) {
                        channel_strip(column, &mut self.decks[index], deck_color(index), index);
                    }
                });
            }
            ui.separator();
            output_levels_ui(ui, &mut self.master, &mut self.phones, &mut self.mixing);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (index, label, color) in [(0, "MASTER", PURPLE), (1, "PHONES", LIGHT_PURPLE)] {
                    let [left, right] = self.bus_levels[index];
                    let text = RichText::new(format!("{label} {}", peak_text(left.max(right))))
                        .size(10.0)
                        .color(color);
                    ui.add_sized([91.0, 17.0], egui::Label::new(text).truncate());
                    ui.separator();
                }
                self.mic_mixer_ui(ui);
            });
            ui.separator();
            ui.horizontal(|ui| {
                let mut enabled = self.crossfader.enabled;
                if ui.checkbox(&mut enabled, "Crossfader").changed() {
                    self.crossfader.set_enabled(enabled);
                }
                let (left, right) = match self.deck_layout {
                    DeckLayout::Two => ("1", "2"),
                    DeckLayout::Four => ("1/3", "4/2"),
                };
                ui.label(RichText::new(left).color(PURPLE));
                ui.spacing_mut().slider_width = (ui.available_width() - 28.0).max(52.0);
                let mut position = self.crossfader.position;
                if ui.add_enabled(
                    self.crossfader.enabled,
                    egui::Slider::new(&mut position, -1.0..=1.0).show_value(false),
                ).on_hover_text("OFF: centered; MIDI and keyboard movements are ignored.").changed() {
                    self.crossfader.set_position(position);
                }
                ui.label(RichText::new(right).color(LIGHT_PURPLE));
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
        let order = self.deck_layout.order();
        let spacing = ui.spacing().item_spacing.y * (order.len() - 1) as f32;
        let lane_height = ((ui.available_height() - spacing) / order.len() as f32).max(17.0);
        for &index in order {
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
                let step = window / pixels as f64;
                // Keep the waveform bins anchored in track time as they scroll.
                let left = position - window * 0.5;
                let first = (left / step).floor();
                for column in 0..=pixels {
                    let start = (first + column as f64) * step;
                    if let Some(frame) = track.detailed.sample(start, start + step) {
                        let x = rect.left()
                            + (((start + step * 0.5 - left) / window) as f32 * rect.width());
                        spectral_column(&painter, rect, x, frame, x < rect.center().x);
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
            let delta = response.drag_delta().x;
            if response.dragged() && delta != 0.0 {
                let time = (position - delta as f64 / rect.width() as f64 * window).max(0.0);
                deck.seek(Duration::from_secs_f64(time), &self.audio);
            }
        }
    }
}

pub(crate) fn console_dimensions(width: f32, layout: DeckLayout) -> (f32, f32) {
    let mixer = (width * 0.32).clamp(400.0, 480.0);
    let decks = layout.order().len() as f32;
    (mixer, (width - mixer - 6.0 * decks) / decks)
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
            meter_ui(ui, levels[0], color, "L", 74.0);
            meter_ui(ui, levels[1], color, "R", 74.0);
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
            for layout in [DeckLayout::Two, DeckLayout::Four] {
                let (mixer, deck) = console_dimensions(width, layout);
                let deck_count = layout.order().len() as f32;
                assert!(mixer >= 400.0 && deck >= 200.0);
                assert!((mixer + (deck + 6.0) * deck_count - width).abs() < 0.01);
            }
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
