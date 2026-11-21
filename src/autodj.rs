use crate::{
    audio::{Audio, Deck, LoadJob},
    decks::COUNT,
    dragdrop::{self, Music, Origin, Selection, Target, Zones},
    playlist::Item,
    track,
};
use eframe::egui;
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

struct Run {
    deck: usize,
    index: usize,
    load_serial: u64,
    loading: bool,
    next: Option<LoadJob>,
}

pub enum Control {
    Start(usize),
    Next,
    Stop,
}

#[derive(Default)]
pub struct AutoDj {
    paths: Vec<PathBuf>,
    run: Option<Run>,
    selected_deck: usize,
    selection: Selection,
    finished: bool,
    error: Option<String>,
}

impl AutoDj {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            ..Self::default()
        }
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> bool {
        let before = self.paths.len();
        self.paths.extend(paths);
        let changed = before != self.paths.len();
        if changed && self.run.is_none() {
            self.reset_status();
        }
        changed
    }

    pub fn active_deck(&self) -> Option<usize> {
        self.run.as_ref().map(|run| run.deck)
    }

    fn reset_status(&mut self) {
        self.finished = false;
        self.error = None;
    }

    pub fn insert(&mut self, paths: Vec<PathBuf>, before: Option<usize>) -> bool {
        let Some(before) = before.filter(|_| self.run.is_none()) else {
            return self.add(paths);
        };
        if paths.is_empty() {
            return false;
        }
        let before = before.min(self.paths.len());
        self.selection.select_range(before..before + paths.len());
        self.paths.splice(before..before, paths);
        self.reset_status();
        true
    }

    pub fn move_to(&mut self, range: Range<usize>, before: Option<usize>) -> bool {
        if self.run.is_some() {
            return false;
        }
        let Some(moved) = dragdrop::move_range(&mut self.paths, range, before) else {
            return false;
        };
        self.selection.select_range(moved);
        self.reset_status();
        true
    }

    pub fn start(&mut self, deck: usize, target: &mut Deck, ctx: &egui::Context) -> bool {
        if self.run.is_some() || self.paths.is_empty() || target.playing() || target.loading() {
            return false;
        }
        self.reset_status();
        self.load(deck, 0, target, ctx, None);
        true
    }

    fn load(
        &mut self,
        deck: usize,
        index: usize,
        target: &mut Deck,
        ctx: &egui::Context,
        prepared: Option<LoadJob>,
    ) {
        let Some(path) = self.paths.get(index) else {
            self.run = None;
            self.finished = true;
            return;
        };
        if let Some(job) = prepared {
            target.load_prepared(job);
        } else {
            target.load(path.clone(), ctx.clone());
        }
        self.run = Some(Run {
            deck,
            index,
            load_serial: target.load_serial(),
            loading: true,
            next: None,
        });
    }

    fn release_deck(run: &Run, target: &mut Deck, audio: &Audio) {
        if run.load_serial == target.load_serial() {
            if run.loading {
                target.cancel_load();
            } else {
                target.stop(audio);
            }
        }
    }

    pub fn stop(&mut self, decks: &mut [Deck; COUNT], audio: &Audio) -> bool {
        let Some(run) = self.run.take() else {
            return false;
        };
        Self::release_deck(&run, &mut decks[run.deck], audio);
        self.reset_status();
        true
    }

    pub fn next(&mut self, decks: &mut [Deck; COUNT], audio: &Audio, ctx: &egui::Context) {
        if let Some(run) = self.run.take() {
            let target = &mut decks[run.deck];
            if run.load_serial == target.load_serial() {
                Self::release_deck(&run, target, audio);
                self.load(run.deck, run.index + 1, target, ctx, run.next);
            }
        }
    }

    pub fn on_loaded(&mut self, index: usize, deck: &mut Deck, audio: &Audio) {
        if let Some(run) = &mut self.run
            && run.loading
            && run.deck == index
            && run.load_serial == deck.load_serial()
            && deck.play(audio)
        {
            run.loading = false;
        }
    }

    pub fn update(&mut self, decks: &mut [Deck; COUNT], ctx: &egui::Context) {
        let Some(run) = &mut self.run else {
            return;
        };
        let deck = &mut decks[run.deck];
        if deck.load_serial() != run.load_serial {
            self.error = Some(format!(
                "AutoDJ stopped after a manual load on Deck {}.",
                run.deck + 1
            ));
            self.run = None;
            return;
        }
        if run.loading && !deck.loading() {
            let title = file_title(&self.paths[run.index]);
            let detail = deck.error.as_deref().unwrap_or("Could not start playback.");
            self.error = Some(format!("Skipped {title}. {detail}"));
        } else if run.loading || !deck.reached_end() {
            if !run.loading
                && run.next.is_none()
                && let Some(path) = self.paths.get(run.index + 1)
            {
                run.next = Some(Deck::prepare_load(path.clone(), ctx.clone(), true));
            }
            return;
        }
        let (deck_index, next_index, prepared) = (run.deck, run.index + 1, run.next.take());
        self.load(deck_index, next_index, deck, ctx, prepared);
    }

    pub fn controls(
        &mut self,
        ui: &mut egui::Ui,
        deck_order: &[usize],
        decks: &[Deck; COUNT],
        audio_ready: bool,
        master_gains: &[f32; COUNT],
    ) -> Option<Control> {
        let mut action = None;
        let idle = self.run.is_none();
        if !deck_order.contains(&self.selected_deck) {
            self.selected_deck = deck_order[0];
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(idle, |ui| {
                egui::ComboBox::from_id_salt("autodj-deck")
                    .width(62.0)
                    .selected_text(format!("Deck {}", self.selected_deck + 1))
                    .show_ui(ui, |ui| {
                        for &deck in deck_order {
                            ui.selectable_value(
                                &mut self.selected_deck,
                                deck,
                                format!("Deck {}", deck + 1),
                            );
                        }
                    });
            });
            let deck = &decks[self.selected_deck];
            let busy = deck.playing() || deck.loading();
            let disabled_hint = if !audio_ready {
                "Connect a master output to start AutoDJ."
            } else if busy && idle {
                "Stop this deck or choose another deck to start AutoDJ."
            } else {
                "Add tracks to the AutoDJ playlist."
            };
            if ui
                .add_enabled(
                    idle && audio_ready && !busy && !self.paths.is_empty(),
                    egui::Button::new("Start AutoDJ"),
                )
                .on_disabled_hover_text(disabled_hint)
                .clicked()
            {
                action = Some(Control::Start(self.selected_deck));
            }
            if ui.add_enabled(!idle, egui::Button::new("Stop")).clicked() {
                action = Some(Control::Stop);
            }
            if ui.add_enabled(!idle, egui::Button::new("Next")).clicked() {
                action = Some(Control::Next);
            }
        });
        let status = if let Some(run) = &self.run {
            let deck = &decks[run.deck];
            let phase = if run.loading {
                "Loading"
            } else if deck.playing() {
                "Playing"
            } else {
                "Paused"
            };
            let title = if run.loading {
                file_title(&self.paths[run.index])
            } else {
                deck.track.as_ref().map_or_else(
                    || file_title(&self.paths[run.index]),
                    |track| track.title.clone(),
                )
            };
            format!(
                "{phase} {}/{} · Deck {} · {title}",
                run.index + 1,
                self.paths.len(),
                run.deck + 1
            )
        } else if self.finished {
            String::from("Playlist finished")
        } else {
            format!("{} tracks", self.paths.len())
        };
        ui.add(egui::Label::new(&status).truncate())
            .on_hover_text(status);
        if self
            .active_deck()
            .is_some_and(|deck| master_gains[deck] <= 0.0001)
        {
            ui.colored_label(egui::Color32::LIGHT_RED, "Deck muted in the master mix.")
                .on_hover_text("Check the deck fader, crossfader and master volume.");
        }
        if let Some(error) = &self.error {
            ui.add(
                egui::Label::new(egui::RichText::new(error).color(egui::Color32::LIGHT_RED))
                    .truncate(),
            )
            .on_hover_text(error);
        }
        action
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        browser_selection: Vec<PathBuf>,
        playlist: &[Item],
        drops: &mut Zones,
    ) -> bool {
        let current = self.run.as_ref().map(|run| run.index);
        let idle = current.is_none();
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !browser_selection.is_empty(),
                    egui::Button::new("Add selection"),
                )
                .on_hover_text("Add the selected tracks from the music browser.")
                .clicked()
            {
                changed |= self.add(browser_selection);
            }
            if ui.button("Files …").clicked()
                && let Some(files) = rfd::FileDialog::new()
                    .add_filter("Audio files", track::EXTENSIONS)
                    .pick_files()
            {
                changed |= self.add(files.into_iter().filter(|path| track::supported(path)));
            }
            if ui
                .add_enabled(!playlist.is_empty(), egui::Button::new("Add playlist"))
                .on_hover_text("Append the tracks from the Playlist tab in their current order.")
                .clicked()
            {
                changed |= self.add(playlist.iter().map(|item| item.path.clone()));
            }
        });
        let range = self.selection.range().filter(|_| idle);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    range.as_ref().is_some_and(|range| range.start > 0),
                    egui::Button::new("Move up"),
                )
                .clicked()
                && let Some(range) = &range
            {
                changed |= self.move_to(range.clone(), Some(range.start - 1));
            }
            if ui
                .add_enabled(
                    range
                        .as_ref()
                        .is_some_and(|range| range.end < self.paths.len()),
                    egui::Button::new("Move down"),
                )
                .clicked()
                && let Some(range) = &range
            {
                changed |= self.move_to(range.clone(), Some(range.end + 1));
            }
            if ui
                .add_enabled(range.is_some(), egui::Button::new("Remove"))
                .clicked()
                && let Some(range) = range
            {
                self.paths.drain(range);
                self.selection = Selection::default();
                self.reset_status();
                changed = true;
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show_rows(ui, 22.0, self.paths.len(), |ui, rows| {
            for index in rows {
                let path = &self.paths[index];
                let title = if current == Some(index) {
                    egui::RichText::new(format!("▶ {}", file_title(path))).strong()
                } else {
                    egui::RichText::new(file_title(path))
                };
                let response = ui
                    .add_sized(
                        [ui.available_width(), 22.0],
                        egui::Button::selectable(self.selection.contains(index), title)
                            .sense(egui::Sense::click_and_drag())
                            .truncate(),
                    )
                    .on_hover_text(path.display().to_string())
                    .on_hover_cursor(egui::CursorIcon::Grab);
                let extend = ui.input(|input| input.modifiers.shift);
                if response.clicked() {
                    self.selection.click(index, extend);
                }
                if response.drag_started_by(egui::PointerButton::Primary) {
                    self.selection.start_drag(index, extend);
                    let range = self.selection.range().unwrap();
                    let items = self.paths[range.clone()]
                        .iter()
                        .map(|path| Item::new(path.clone(), None))
                        .collect();
                    dragdrop::source_many(
                        &response,
                        Music {
                            items,
                            origin: Origin::AutoDj(range),
                        },
                    );
                }
                if idle {
                    drops.list_row(ui, response.rect, |at| Target::AutoDj(Some(at)), index);
                }
            }
        });
        changed
    }
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
