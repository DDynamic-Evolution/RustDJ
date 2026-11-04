use crate::{
    analysis::{self, Job},
    database::{self, Database, Fingerprint, Lookup, Summary},
    dragdrop::{self, Origin},
    playlist::Item,
    track,
};
use eframe::egui::{self, RichText, Vec2};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub bytes: u64,
    pub analysis: Option<Summary>,
    pub stale: bool,
    pub analysis_error: Option<String>,
    identity: Option<Fingerprint>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BrowserSettings {
    pub folder: PathBuf,
    pub favorites: Vec<PathBuf>,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let music = home.join("Music");
        Self {
            folder: if music.is_dir() { music.clone() } else { home },
            favorites: vec![music].into_iter().filter(|p| p.is_dir()).collect(),
        }
    }
}

struct Listing {
    entries: Vec<Entry>,
    database_error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Name,
    Bpm,
    Key,
    Duration,
}

pub struct Browser {
    pub settings: BrowserSettings,
    pub dirty: bool,
    pub selected: Option<usize>,
    pub error: Option<String>,
    entries: Vec<Entry>,
    filtered: Vec<usize>,
    path_edit: String,
    search: String,
    receiver: Option<Receiver<Result<Listing, String>>>,
    database: PathBuf,
    database_error: Option<String>,
    job: Option<Job>,
    queued: HashSet<PathBuf>,
    analysis_notice: Option<String>,
    analysis_details: String,
    sort: Sort,
    descending: bool,
    drives: Vec<PathBuf>,
    drive_receiver: Receiver<Vec<PathBuf>>,
    scroll_selection: bool,
}

pub enum Event {
    Load(usize, PathBuf),
    Playlist(PathBuf),
}

pub fn scan(path: &Path) -> Result<Vec<Entry>, String> {
    let directory = std::fs::read_dir(path).map_err(|e| format!("Could not read folder: {e}"))?;
    let mut entries = Vec::new();
    for entry in directory {
        let entry = entry.map_err(|e| format!("Could not read directory entry: {e}"))?;
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() || (metadata.is_file() && track::supported(&path)) {
            entries.push(Entry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path,
                directory: metadata.is_dir(),
                bytes: metadata.len(),
                identity: if metadata.is_file() {
                    Fingerprint::from_metadata(&metadata).ok()
                } else {
                    None
                },
                analysis: None,
                stale: false,
                analysis_error: None,
            });
        }
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

fn option_cmp<T: Ord>(a: Option<T>, b: Option<T>, descending: bool) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => {
            let order = a.cmp(&b);
            if descending { order.reverse() } else { order }
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    }
}

fn scan_cached(path: &Path, database: &Path) -> Result<Listing, String> {
    let mut entries = scan(path)?;
    let mut database_error = None;
    match Database::open(database) {
        Ok(db) => {
            for entry in &mut entries {
                if let Some(identity) = &entry.identity {
                    match db.lookup(&entry.path, identity) {
                        Ok(Lookup::Fresh(summary)) => entry.analysis = Some(summary),
                        Ok(Lookup::Stale) => entry.stale = true,
                        Ok(Lookup::Missing) => {}
                        Err(error) => database_error = Some(error),
                    }
                }
            }
        }
        Err(error) => database_error = Some(error),
    }
    Ok(Listing {
        entries,
        database_error,
    })
}

impl Browser {
    pub fn new(settings: BrowserSettings, ctx: &egui::Context) -> Self {
        let (sender, drive_receiver) = mpsc::channel();
        let drive_ctx = ctx.clone();
        std::thread::spawn(move || {
            #[cfg(windows)]
            let drives = (b'A'..=b'Z')
                .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
                .filter(|p| p.is_dir())
                .collect();
            #[cfg(not(windows))]
            let drives = vec![PathBuf::from("/")];
            let _ = sender.send(drives);
            drive_ctx.request_repaint();
        });
        let mut browser = Self {
            settings,
            dirty: false,
            selected: None,
            error: None,
            entries: Vec::new(),
            filtered: Vec::new(),
            path_edit: String::new(),
            search: String::new(),
            receiver: None,
            database: database::default_path(),
            database_error: None,
            job: None,
            queued: HashSet::new(),
            analysis_notice: None,
            analysis_details: String::new(),
            sort: Sort::Name,
            descending: false,
            drives: Vec::new(),
            drive_receiver,
            scroll_selection: false,
        };
        browser.navigate(browser.settings.folder.clone(), ctx);
        browser.dirty = false;
        browser
    }

    pub fn navigate(&mut self, path: PathBuf, ctx: &egui::Context) {
        let path = if path.is_absolute() {
            path
        } else {
            self.settings.folder.join(path)
        };
        let path = std::path::absolute(&path).unwrap_or(path);
        self.settings.folder = path.clone();
        self.path_edit = path.to_string_lossy().into_owned();
        self.selected = None;
        self.entries.clear();
        self.filtered.clear();
        self.error = None;
        self.dirty = true;
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        let database = self.database.clone();
        std::thread::spawn(move || {
            let _ = sender.send(scan_cached(&path, &database));
            ctx.request_repaint();
        });
        self.receiver = Some(receiver);
    }

    pub fn poll(&mut self) {
        self.poll_analysis();
        if let Ok(drives) = self.drive_receiver.try_recv() {
            self.drives = drives;
        }
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.receiver = None;
                    match result {
                        Ok(listing) => {
                            self.entries = listing.entries;
                            self.database_error = listing.database_error;
                            self.refilter();
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.error = Some("Folder scan stopped unexpectedly.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    fn refilter(&mut self) {
        let selected = self
            .selected
            .and_then(|row| self.filtered.get(row))
            .and_then(|i| self.entries.get(*i))
            .map(|e| e.path.clone());
        let search = self.search.to_lowercase();
        self.filtered = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.name.to_lowercase().contains(&search)
                    || e.analysis.as_ref().is_some_and(|a| {
                        a.key
                            .is_some_and(|k| k.label().to_lowercase().contains(&search))
                    })
            })
            .map(|(i, _)| i)
            .collect();
        let descending = self.descending;
        self.filtered.sort_by(|left, right| {
            let a = &self.entries[*left];
            let b = &self.entries[*right];
            b.directory
                .cmp(&a.directory)
                .then_with(|| match self.sort {
                    Sort::Name => {
                        let order = a.name.to_lowercase().cmp(&b.name.to_lowercase());
                        if descending { order.reverse() } else { order }
                    }
                    Sort::Bpm => option_cmp(
                        a.analysis
                            .as_ref()
                            .and_then(|a| a.bpm())
                            .map(|b| (b * 1000.0) as u32),
                        b.analysis
                            .as_ref()
                            .and_then(|a| a.bpm())
                            .map(|b| (b * 1000.0) as u32),
                        descending,
                    ),
                    Sort::Key => option_cmp(
                        a.analysis
                            .as_ref()
                            .and_then(|a| a.key)
                            .map(|k| k.sort_index()),
                        b.analysis
                            .as_ref()
                            .and_then(|a| a.key)
                            .map(|k| k.sort_index()),
                        descending,
                    ),
                    Sort::Duration => option_cmp(
                        a.analysis.as_ref().map(|a| a.duration_ms),
                        b.analysis.as_ref().map(|a| a.duration_ms),
                        descending,
                    ),
                })
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.selected = selected.and_then(|path| {
            self.filtered
                .iter()
                .position(|i| self.entries[*i].path == path)
        });
    }

    pub fn on_track(&mut self, track: &track::Track) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.path == track.path)
        {
            entry.analysis = Some(Summary::from_track(track));
            entry.stale = false;
            entry.analysis_error = track.cache_warning.clone();
            self.refilter();
        }
    }

    fn start_analysis(&mut self, paths: Vec<PathBuf>, force: bool, ctx: &egui::Context) {
        if self.job.is_some() || paths.is_empty() {
            return;
        }
        self.queued = paths.iter().cloned().collect();
        self.analysis_notice = None;
        self.analysis_details.clear();
        self.job = Some(Job::start(paths, force, &self.database, ctx));
    }

    fn poll_analysis(&mut self) {
        let events: Vec<_> = self
            .job
            .as_ref()
            .map_or(Vec::new(), |job| job.receiver.try_iter().collect());
        let mut finished = false;
        let mut changed = false;
        for event in events {
            let Some(job) = &mut self.job else {
                break;
            };
            match event {
                analysis::Event::Started(path) => job.current = Some(path),
                analysis::Event::Completed {
                    path,
                    result,
                    reused,
                } => {
                    job.current = None;
                    job.completed += 1;
                    job.reused += usize::from(reused);
                    self.queued.remove(&path);
                    match result {
                        Ok(summary) => {
                            if let Some(entry) = self.entries.iter_mut().find(|e| e.path == path) {
                                entry.analysis = Some(summary);
                                entry.stale = false;
                                entry.analysis_error = None;
                                changed = true;
                            }
                        }
                        Err(error) => {
                            job.failures += 1;
                            job.errors.push(format!("{}: {error}", path.display()));
                            if let Some(entry) = self.entries.iter_mut().find(|e| e.path == path) {
                                entry.analysis_error = Some(error);
                            }
                        }
                    }
                }
                analysis::Event::Finished { cancelled } => {
                    self.analysis_notice = Some(format!(
                        "{}: {} / {} files, {} from database, {} errors",
                        if cancelled {
                            "Cancelled"
                        } else {
                            "Analysis complete"
                        },
                        job.completed,
                        job.total,
                        job.reused,
                        job.failures
                    ));
                    self.analysis_details = job.errors.join("\n");
                    finished = true;
                }
                analysis::Event::Fatal(error) => {
                    self.database_error = Some(error);
                    finished = true;
                }
            }
        }
        if changed {
            self.refilter();
        }
        if finished {
            self.job = None;
            self.queued.clear();
        }
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        let entry = self.entries.get(*self.filtered.get(self.selected?)?)?;
        (!entry.directory).then(|| entry.path.clone())
    }

    pub fn step_selection(&mut self, amount: i32) {
        if !self.filtered.is_empty() {
            self.scroll_selection = true;
            self.selected = Some(match self.selected {
                Some(index) => {
                    (index as i32 + amount).clamp(0, self.filtered.len() as i32 - 1) as usize
                }
                None => {
                    if amount < 0 {
                        self.filtered.len() - 1
                    } else {
                        0
                    }
                }
            });
        }
    }

    pub fn activate(&mut self, deck: usize, ctx: &egui::Context) -> Option<Event> {
        let entry = self.entries.get(*self.filtered.get(self.selected?)?)?;
        let path = entry.path.clone();
        if entry.directory {
            self.navigate(path, ctx);
            None
        } else {
            Some(Event::Load(deck, path))
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, active: usize, ctx: &egui::Context) -> Option<Event> {
        self.poll();
        let mut navigation = None;
        let mut event = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.settings.folder.parent().is_some(),
                    egui::Button::new("Up"),
                )
                .clicked()
            {
                navigation = self.settings.folder.parent().map(Path::to_path_buf);
            }
            if ui.button("Folder …").clicked() {
                navigation = rfd::FileDialog::new()
                    .set_directory(&self.settings.folder)
                    .pick_folder();
            }
            if ui.button("Refresh").clicked() {
                navigation = Some(self.settings.folder.clone());
            }
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search music / folders …")
                        .desired_width(220.0),
                )
                .changed()
            {
                self.refilter();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        self.selected_path().is_some(),
                        egui::Button::new("+ Playlist"),
                    )
                    .clicked()
                {
                    event = self.selected_path().map(Event::Playlist);
                }
                for deck in crate::decks::ORDER.into_iter().rev() {
                    if ui
                        .add_enabled(
                            self.selected_path().is_some(),
                            egui::Button::new(
                                RichText::new((deck + 1).to_string())
                                    .color(crate::decks::COLORS[deck]),
                            ),
                        )
                        .on_hover_text(format!("Load into Deck {}", deck + 1))
                        .clicked()
                    {
                        event = self.selected_path().map(|p| Event::Load(deck, p));
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.path_edit)
                    .desired_width((ui.available_width() - 155.0).max(100.0)),
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                navigation = Some(PathBuf::from(&self.path_edit));
            }
            if ui.button("Go").clicked() {
                navigation = Some(PathBuf::from(&self.path_edit));
            }
            if ui.small_button("Favorite +").clicked()
                && !self.settings.favorites.contains(&self.settings.folder)
            {
                self.settings.favorites.push(self.settings.folder.clone());
                self.dirty = true;
            }
        });
        ui.horizontal(|ui| {
            let selected = self.selected_path();
            let ready = selected.as_ref().is_some_and(|path| {
                self.entries
                    .iter()
                    .any(|e| &e.path == path && e.analysis.is_some())
            });
            if ui
                .add_enabled(
                    self.job.is_none() && selected.is_some(),
                    egui::Button::new(if ready {
                        "Reanalyze selection"
                    } else {
                        "Analyze selection"
                    }),
                )
                .clicked()
            {
                self.start_analysis(vec![selected.unwrap()], ready, ctx);
            }
            let missing: Vec<_> = self
                .entries
                .iter()
                .filter(|e| !e.directory && e.analysis.is_none())
                .map(|e| e.path.clone())
                .collect();
            if ui
                .add_enabled(
                    self.job.is_none() && !missing.is_empty(),
                    egui::Button::new(format!("Analyze missing ({})", missing.len())),
                )
                .on_hover_text(
                    "Analyze new or changed files in the current folder; excludes subfolders",
                )
                .clicked()
            {
                self.start_analysis(missing, false, ctx);
            }
            ui.label(RichText::new("SQLite database").small().weak())
                .on_hover_text(self.database.display().to_string());
        });
        if let Some(job) = &self.job {
            ui.horizontal(|ui| {
                ui.add(
                    egui::ProgressBar::new(job.completed as f32 / job.total.max(1) as f32)
                        .desired_width(160.0)
                        .text(format!("{} / {}", job.completed, job.total)),
                );
                if ui.button("Cancel").clicked() {
                    job.cancel();
                }
                let status = if job.cancelling() {
                    "Cancelling …".into()
                } else {
                    job.current.as_ref().map_or("Waiting …".into(), |p| {
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    })
                };
                ui.add(egui::Label::new(status).truncate());
            });
        } else if let Some(notice) = &self.analysis_notice {
            ui.label(RichText::new(notice).small())
                .on_hover_text(&self.analysis_details);
        }
        if let Some(error) = &self.database_error {
            ui.add(
                egui::Label::new(RichText::new(error).color(egui::Color32::LIGHT_RED)).truncate(),
            )
            .on_hover_text(error);
        }
        ui.separator();
        let height = (ui.available_height() - 6.0).max(90.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(155.0, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_max_width(155.0);
                    egui::ScrollArea::vertical()
                        .id_salt("folders")
                        .max_height(height)
                        .show(ui, |ui| {
                            ui.label(RichText::new("FAVORITES").small().strong());
                            let mut remove = None;
                            for (index, folder) in self.settings.favorites.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    let name = folder
                                        .file_name()
                                        .unwrap_or(folder.as_os_str())
                                        .to_string_lossy();
                                    if ui
                                        .add_sized(
                                            [117.0, 24.0],
                                            egui::Button::new(name)
                                                .selected(*folder == self.settings.folder)
                                                .truncate(),
                                        )
                                        .on_hover_text(folder.display().to_string())
                                        .clicked()
                                    {
                                        navigation = Some(folder.clone());
                                    }
                                    if ui.small_button("x").clicked() {
                                        remove = Some(index);
                                    }
                                });
                            }
                            if let Some(index) = remove {
                                self.settings.favorites.remove(index);
                                self.dirty = true;
                            }
                            ui.separator();
                            ui.label(RichText::new("DRIVES").small().strong());
                            for root in &self.drives {
                                if ui.small_button(root.display().to_string()).clicked() {
                                    navigation = Some(root.clone());
                                }
                            }
                            ui.separator();
                            ui.label(RichText::new("FOLDERS").small().strong());
                            let ancestors: Vec<_> = self.settings.folder.ancestors().collect();
                            for (depth, folder) in ancestors.into_iter().rev().enumerate() {
                                let name = folder
                                    .file_name()
                                    .unwrap_or(folder.as_os_str())
                                    .to_string_lossy();
                                ui.horizontal(|ui| {
                                    ui.add_space((depth.min(5) * 7) as f32);
                                    if ui
                                        .add(
                                            egui::Button::new(format!("- {name}"))
                                                .selected(folder == self.settings.folder)
                                                .truncate(),
                                        )
                                        .clicked()
                                    {
                                        navigation = Some(folder.to_path_buf());
                                    }
                                });
                            }
                            for entry in self.entries.iter().filter(|entry| entry.directory) {
                                if ui
                                    .add(
                                        egui::Button::new(format!("  + {}", entry.name)).truncate(),
                                    )
                                    .on_hover_text(entry.path.display().to_string())
                                    .clicked()
                                {
                                    navigation = Some(entry.path.clone());
                                }
                            }
                        });
                },
            );
            ui.separator();
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let mut sort = None;
                    if ui.add_sized([(ui.available_width() - 432.0).max(80.0), 18.0], egui::Button::new("TITLE / FILENAME").frame(false)).clicked() { sort = Some(Sort::Name); }
                    for (label, width, key) in [("LENGTH", 58.0, Sort::Duration), ("BPM", 55.0, Sort::Bpm), ("KEY", 100.0, Sort::Key)] {
                        if ui.add_sized([width, 18.0], egui::Button::new(label).frame(false)).clicked() { sort = Some(key); }
                    }
                    ui.add_sized([78.0, 18.0], egui::Label::new(RichText::new("ANALYSIS").small()));
                    ui.add_sized([48.0, 18.0], egui::Label::new(RichText::new("FORMAT").small()));
                    ui.add_sized([57.0, 18.0], egui::Label::new(RichText::new("SIZE").small()));
                    if let Some(sort) = sort { if self.sort == sort { self.descending = !self.descending; } else { self.sort = sort; self.descending = false; } self.refilter(); }
                });
                if self.receiver.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading folder …");
                    });
                }
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if self.filtered.is_empty() && self.receiver.is_none() && self.error.is_none() {
                    ui.label("No matching music files or folders.");
                }
                let mut activate = false;
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt("music-files")
                    .max_height(height)
                    .auto_shrink([false, false]);
                if self.scroll_selection {
                    scroll = scroll.vertical_scroll_offset(
                        (self.selected.unwrap_or(0) as f32 * (27.0 + ui.spacing().item_spacing.y)
                            - height * 0.5)
                            .max(0.0),
                    );
                    self.scroll_selection = false;
                }
                scroll.show_rows(ui, 27.0, self.filtered.len(), |ui, range| {
                    for row in range {
                        let entry = &self.entries[self.filtered[row]];
                        let row_response = ui.horizontal(|ui| {
                            let name = if entry.directory {
                                format!("[Folder]  {}", entry.name)
                            } else {
                                entry.name.clone()
                            };
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new((ui.available_width() - 432.0).max(80.0), 27.0),
                                egui::Sense::hover(),
                            );
                            if self.selected == Some(row) {
                                ui.painter()
                                    .rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
                            } else if row % 2 == 0 {
                                ui.painter().rect_filled(
                                    rect,
                                    0.0,
                                    egui::Color32::from_white_alpha(5),
                                );
                            }
                            let color = ui.visuals().text_color();
                            let galley = ui.painter().layout_no_wrap(
                                name,
                                egui::FontId::proportional(12.0),
                                color,
                            );
                            let position =
                                rect.left_center() + Vec2::new(5.0, -galley.size().y * 0.5);
                            ui.painter()
                                .with_clip_rect(rect)
                                .galley(position, galley, color);
                            let summary = entry.analysis.as_ref();
                            let duration = summary.map_or("—".into(), |a| { let seconds = a.duration_ms / 1000; format!("{}:{:02}", seconds / 60, seconds % 60) });
                            let bpm = summary.and_then(|a| a.bpm()).map_or("—".into(), |b| format!("{b:.1}"));
                            let key = summary.and_then(|a| a.key);
                            ui.add_sized([58.0, 27.0], egui::Label::new(duration));
                            ui.add_sized([55.0, 27.0], egui::Label::new(bpm));
                            ui.add_sized([100.0, 27.0], egui::Label::new(key.map_or("—".into(), |k| k.label())))
                                .on_hover_text(key.map_or("No reliable key detected, or the file has not been analyzed yet.".into(), |k| format!("Estimated key: {}. Profile match {:.0}%; not guaranteed.", k.label(), k.confidence * 100.0)));
                            let working = self.job.as_ref().is_some_and(|job| job.current.as_ref() == Some(&entry.path));
                            let state = if entry.directory { "" } else if working { "Analyzing …" } else if self.queued.contains(&entry.path) { "Queued" } else if entry.analysis_error.is_some() { "Error" } else if summary.is_some() { "Analyzed" } else if entry.stale { "Changed" } else { "Pending" };
                            ui.add_sized([78.0, 27.0], egui::Label::new(RichText::new(state).small().color(if entry.analysis_error.is_some() { egui::Color32::LIGHT_RED } else { ui.visuals().weak_text_color() }))).on_hover_text(entry.analysis_error.as_deref().unwrap_or(state));
                            ui.add_sized([48.0, 27.0], egui::Label::new(if entry.directory { "DIR".into() } else { entry.path.extension().unwrap_or_default().to_string_lossy().to_uppercase() }));
                            ui.add_sized([57.0, 27.0], egui::Label::new(if entry.directory { "".into() } else { format!("{:.1} MB", entry.bytes as f64 / 1_000_000.0) }));
                        });
                        let response = ui.interact(
                            row_response.response.rect.intersect(ui.clip_rect()),
                            ui.id().with(("music-row", &entry.path)),
                            if entry.directory { egui::Sense::click() } else { egui::Sense::click_and_drag() },
                        ).on_hover_text(entry.path.display().to_string());
                        if response.clicked() || response.drag_started() { self.selected = Some(row); }
                        if response.double_clicked() { self.selected = Some(row); activate = true; }
                        if !entry.directory {
                            let mut item = Item::new(entry.path.clone(), None);
                            item.summary = entry.analysis.clone(); item.available = Some(true);
                            dragdrop::source(&response, item, Origin::Browser);
                        }
                    }
                });
                if activate {
                    event = self.activate(active, ctx);
                }
            });
        });
        if let Some(path) = navigation {
            self.navigate(path, ctx);
        }
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_filters_non_audio_and_recognizes_real_mp3_flac() {
        let entries = scan(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.name == "demo-a.mp3" && e.bytes > 0)
        );
        assert!(
            entries
                .iter()
                .any(|e| e.name == "demo-b.flac" && e.bytes > 0)
        );
        assert!(
            entries
                .iter()
                .all(|e| e.directory || track::supported(&e.path))
        );
        assert!(scan(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("does-not-exist")).is_err());
    }

    #[test]
    fn folder_listing_restores_persisted_metadata_and_survives_database_failure() {
        let temporary = tempfile::tempdir().unwrap();
        let audio = temporary.path().join("music");
        std::fs::create_dir(&audio).unwrap();
        let file = audio.join("test.flac");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo-b.flac"),
            &file,
        )
        .unwrap();
        let database = temporary.path().join("library.sqlite3");
        let listing = scan_cached(&audio, &database).unwrap();
        assert!(listing.database_error.is_none());
        assert!(listing.entries[0].analysis.is_none());
        {
            let db = Database::open(&database).unwrap();
            crate::analysis::cached(
                &db,
                file.clone(),
                &std::sync::atomic::AtomicBool::new(false),
                false,
            )
            .unwrap();
        }
        let listing = scan_cached(&audio, &database).unwrap();
        assert!(
            listing.entries[0]
                .analysis
                .as_ref()
                .unwrap()
                .bpm()
                .is_some()
        );
        std::fs::write(&file, b"changed").unwrap();
        let listing = scan_cached(&audio, &database).unwrap();
        assert!(listing.entries[0].stale);
        assert!(listing.entries[0].analysis.is_none());
        let blocked = temporary.path().join("not-a-directory");
        std::fs::write(&blocked, b"blocked").unwrap();
        let listing = scan_cached(&audio, &blocked.join("library.sqlite3")).unwrap();
        assert!(listing.database_error.is_some());
        assert_eq!(listing.entries.len(), 1);
    }
}
