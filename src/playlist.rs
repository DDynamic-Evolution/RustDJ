use crate::{
    database::{self, Database, Fingerprint, Lookup, Summary},
    dragdrop::{self, Origin, Zones},
    track::{self, Track},
};
use eframe::egui::{self, RichText};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub title: String,
    pub summary: Option<Summary>,
    pub available: Option<bool>,
}

impl Item {
    pub fn new(path: PathBuf, title: Option<String>) -> Self {
        let title = title.filter(|t| !t.is_empty()).unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        Self {
            path,
            title,
            summary: None,
            available: None,
        }
    }

    pub fn from_track(track: &Track) -> Self {
        Self {
            path: track.path.clone(),
            title: track.title.clone(),
            summary: Some(Summary::from_track(track)),
            available: Some(true),
        }
    }
}

fn read_playlist(path: &Path) -> Result<(Vec<Item>, usize), String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| format!("Could not read playlist: {e}"))?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("The playlist exceeds 16 MB.".into());
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "The playlist is not UTF-8 encoded. Please save it as M3U8.")?
        .trim_start_matches('\u{feff}');
    let base = std::path::absolute(path).map_err(|e| e.to_string())?;
    let base = base.parent().ok_or("The playlist has no parent folder")?;
    let mut items = Vec::new();
    let mut title = None;
    let mut skipped = 0;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with("#EXT-X-") {
            return Err(
                "HLS streams are not supported. Open a local music playlist instead.".into(),
            );
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            title = info.split_once(',').map(|(_, name)| name.trim().to_owned());
        } else if !line.starts_with('#') {
            let label = title.take();
            let file = Path::new(line);
            if line.contains("://") || line.contains('\0') || !track::supported(file) {
                skipped += 1;
                continue;
            }
            let file = if file.is_absolute() {
                file.to_path_buf()
            } else {
                base.join(file)
            };
            items.push(Item::new(file, label));
        }
    }
    if items.is_empty() && skipped > 0 {
        return Err("This playlist contains no supported local music files.".into());
    }
    Ok((items, skipped))
}

fn write_playlist(path: &Path, items: &[Item]) -> Result<(), String> {
    let mut text = String::from("#EXTM3U\n");
    for item in items {
        let absolute = std::path::absolute(&item.path).map_err(|e| e.to_string())?;
        let file = absolute
            .to_str()
            .ok_or("The file path cannot be saved as UTF-8.")?;
        if file.contains(['\r', '\n', '\0']) {
            return Err("A file path contains invalid characters.".into());
        }
        let duration = item
            .summary
            .as_ref()
            .map_or(-1, |s| (s.duration_ms / 1000) as i64);
        text.push_str(&format!(
            "#EXTINF:{duration},{}\n{file}\n",
            item.title.replace(['\r', '\n'], " ")
        ));
    }
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    let parent = absolute.parent().ok_or("No playlist folder")?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        ".rustdj-playlist-{}-{stamp}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| format!("Could not create temporary playlist: {e}"))?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &absolute)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("Playlist was not saved: {error}"));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Pending {
    New,
    Open,
    Close,
}

type Metadata = Vec<(PathBuf, bool, Option<Summary>)>;

#[derive(Default)]
pub struct Playlist {
    pub items: Vec<Item>,
    pub modified: bool,
    pub path: Option<PathBuf>,
    selected: Option<usize>,
    error: Option<String>,
    notice: Option<String>,
    metadata: Option<Receiver<Metadata>>,
    pending: Option<Pending>,
    allow_close: bool,
}

impl Playlist {
    pub fn insert(&mut self, item: Item, index: Option<usize>, ctx: &egui::Context) {
        self.insert_many(vec![item], index, ctx);
    }

    fn insert_items(&mut self, items: Vec<Item>, index: Option<usize>) -> bool {
        if items.is_empty() {
            return false;
        }
        let index = index.unwrap_or(self.items.len()).min(self.items.len());
        self.selected = Some(index + items.len() - 1);
        self.items.splice(index..index, items);
        self.modified = true;
        self.error = None;
        self.notice = None;
        true
    }

    pub fn insert_many(&mut self, items: Vec<Item>, index: Option<usize>, ctx: &egui::Context) {
        if self.insert_items(items, index) {
            self.refresh(ctx);
        }
    }

    pub fn move_to(&mut self, from: usize, before: Option<usize>) {
        if from >= self.items.len() {
            return;
        }
        let before = before.unwrap_or(self.items.len()).min(self.items.len());
        let destination = before - usize::from(before > from);
        if destination != from {
            let item = self.items.remove(from);
            self.items.insert(destination, item);
            self.selected = Some(destination);
            self.modified = true;
        }
    }

    pub fn add(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let items = paths
            .into_iter()
            .filter(|p| track::supported(p))
            .map(|path| Item::new(std::path::absolute(&path).unwrap_or(path), None))
            .collect();
        self.insert_many(items, None, ctx);
    }

    fn open(&mut self, path: PathBuf) -> Result<(), String> {
        let (items, skipped) = read_playlist(&path)?;
        self.items = items;
        self.path = Some(path);
        self.selected = None;
        self.modified = false;
        self.error = None;
        self.notice = (skipped > 0).then(|| format!("Skipped {skipped} unsupported entries."));
        self.metadata = None;
        Ok(())
    }

    fn save_to(&mut self, path: PathBuf) -> Result<(), String> {
        write_playlist(&path, &self.items)?;
        self.path = Some(path);
        self.modified = false;
        self.error = None;
        self.notice = Some(format!("Playlist saved: {} tracks.", self.items.len()));
        Ok(())
    }

    fn save_dialog(&mut self, save_as: bool) -> bool {
        let path = if save_as || self.path.is_none() {
            let mut dialog = rfd::FileDialog::new()
                .set_title("Save playlist")
                .add_filter("M3U8 playlist (UTF-8)", &["m3u8"])
                .set_file_name(
                    self.path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .unwrap_or("RustDJ-Playlist.m3u8".as_ref())
                        .to_string_lossy(),
                );
            if let Some(parent) = self.path.as_ref().and_then(|p| p.parent()) {
                dialog = dialog.set_directory(parent);
            }
            dialog.save_file().map(|p| {
                if p.extension().is_none() {
                    p.with_extension("m3u8")
                } else {
                    p
                }
            })
        } else {
            self.path.clone()
        };
        let Some(path) = path else {
            return false;
        };
        match self.save_to(path) {
            Ok(()) => true,
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }

    fn perform(&mut self, pending: Pending, ctx: &egui::Context) {
        match pending {
            Pending::New => {
                self.items.clear();
                self.selected = None;
                self.path = None;
                self.modified = false;
                self.error = None;
                self.notice = None;
                self.metadata = None;
            }
            Pending::Open => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Load playlist")
                    .add_filter("M3U/M3U8 playlist (UTF-8)", &["m3u8", "m3u"]);
                if let Some(parent) = self.path.as_ref().and_then(|p| p.parent()) {
                    dialog = dialog.set_directory(parent);
                }
                if let Some(path) = dialog.pick_file() {
                    match self.open(path) {
                        Ok(()) => self.refresh(ctx),
                        Err(error) => self.error = Some(error),
                    }
                }
            }
            Pending::Close => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn request(&mut self, pending: Pending, ctx: &egui::Context) {
        if self.modified {
            self.pending = Some(pending);
        } else {
            self.perform(pending, ctx);
        }
    }

    pub fn confirm_changes(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.modified && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }
        if let Some(pending) = self.pending {
            let mut choice = None;
            egui::Modal::new(egui::Id::new("playlist-unsaved")).show(ctx, |ui| {
                ui.set_max_width(470.0);
                ui.heading("Save playlist?");
                ui.label("The playlist has unsaved changes.");
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                ui.horizontal(|ui| {
                    if ui.button("Save and continue").clicked() {
                        choice = Some(0);
                    }
                    if ui.button("Discard").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(2);
                    }
                });
            });
            match choice {
                Some(0) if self.save_dialog(false) => {
                    self.pending = None;
                    self.perform(pending, ctx);
                }
                Some(1) => {
                    self.pending = None;
                    self.perform(pending, ctx);
                }
                Some(2) => self.pending = None,
                _ => {}
            }
        }
    }

    pub fn refresh(&mut self, ctx: &egui::Context) {
        let paths: Vec<_> = self.items.iter().map(|i| i.path.clone()).collect();
        if paths.is_empty() {
            self.metadata = None;
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let db = Database::open(&database::default_path()).ok();
            let results = paths
                .into_iter()
                .map(|path| {
                    let available = path.is_file();
                    let summary = Fingerprint::read(&path).ok().and_then(|id| {
                        match db.as_ref()?.lookup(&path, &id).ok()? {
                            Lookup::Fresh(summary) => Some(summary),
                            _ => None,
                        }
                    });
                    (path, available, summary)
                })
                .collect();
            let _ = sender.send(results);
            ctx.request_repaint();
        });
        self.metadata = Some(receiver);
    }

    fn poll(&mut self) {
        if let Some(receiver) = &self.metadata {
            match receiver.try_recv() {
                Ok(results) => {
                    self.metadata = None;
                    for (path, available, summary) in results {
                        for item in self.items.iter_mut().filter(|i| i.path == path) {
                            item.available = Some(available);
                            item.summary = summary.clone();
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.metadata = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    pub fn on_track(&mut self, track: &Track) {
        for item in self.items.iter_mut().filter(|item| item.path == track.path) {
            item.summary = Some(Summary::from_track(track));
            item.available = Some(true);
        }
    }

    fn move_selected(&mut self, direction: i32) {
        let Some(index) = self.selected else {
            return;
        };
        let next = index as i32 + direction;
        if (0..self.items.len() as i32).contains(&next) {
            self.items.swap(index, next as usize);
            self.selected = Some(next as usize);
            self.modified = true;
        }
    }

    fn remove_selected(&mut self) {
        if let Some(index) = self.selected.filter(|i| *i < self.items.len()) {
            self.items.remove(index);
            self.modified = true;
            self.selected = (!self.items.is_empty()).then(|| index.min(self.items.len() - 1));
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        active: usize,
        drops: &mut Zones,
    ) -> Option<(usize, PathBuf)> {
        self.poll();
        ui.horizontal_wrapped(|ui| {
            if ui.button("New").clicked() {
                self.request(Pending::New, ctx);
            }
            if ui.button("Load …").clicked() {
                self.request(Pending::Open, ctx);
            }
            if ui.button("Save").clicked() {
                self.save_dialog(false);
            }
            if ui.button("Save as …").clicked() {
                self.save_dialog(true);
            }
        });
        let name = self.path.as_ref().map_or("New playlist".into(), |p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "{}{} · {} tracks",
                    name,
                    if self.modified { " *" } else { "" },
                    self.items.len()
                ))
                .strong(),
            )
            .truncate(),
        )
        .on_hover_text(
            self.path
                .as_ref()
                .map_or("Not saved yet".into(), |p| p.display().to_string()),
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("+ Files …").clicked()
                && let Some(paths) = rfd::FileDialog::new()
                    .add_filter("Music files", track::EXTENSIONS)
                    .pick_files()
            {
                self.add(paths, ctx);
            }
            if ui.small_button("Refresh metadata").clicked() {
                self.refresh(ctx);
            }
            if self.metadata.is_some() {
                ui.spinner();
            }
        });
        ui.horizontal(|ui| {
            let selected = self.selected.filter(|i| *i < self.items.len());
            if ui
                .add_enabled(selected.is_some_and(|i| i > 0), egui::Button::new("Up"))
                .clicked()
            {
                self.move_selected(-1);
            }
            if ui
                .add_enabled(
                    selected.is_some_and(|i| i + 1 < self.items.len()),
                    egui::Button::new("Down"),
                )
                .clicked()
            {
                self.move_selected(1);
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Remove"))
                .clicked()
            {
                self.remove_selected();
            }
        });
        if let Some(error) = &self.error {
            ui.add(
                egui::Label::new(RichText::new(error).color(egui::Color32::LIGHT_RED)).truncate(),
            )
            .on_hover_text(error);
        } else if let Some(notice) = &self.notice {
            ui.add(egui::Label::new(RichText::new(notice).small()).truncate())
                .on_hover_text(notice);
        }
        ui.separator();
        let mut load = None;
        egui::ScrollArea::vertical()
            .id_salt("playlist-items")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.items.is_empty() {
                    ui.label("Drag music here from the browser.");
                    ui.label(
                        RichText::new("Or use “+ Files …” to add multiple tracks.")
                            .small()
                            .weak(),
                    );
                }
                for (index, item) in self.items.iter().enumerate() {
                    let row = ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            for deck in crate::decks::ORDER {
                                let label = (deck + 1).to_string();
                                if ui
                                    .add_enabled(
                                        item.available != Some(false),
                                        egui::Button::new(
                                            RichText::new(label).color(crate::decks::COLORS[deck]),
                                        )
                                        .small(),
                                    )
                                    .clicked()
                                {
                                    load = Some((deck, item.path.clone()));
                                }
                            }
                            let response = ui
                                .add_sized(
                                    [(ui.available_width() - 4.0).max(40.0), 24.0],
                                    egui::Button::new(format!("{}. {}", index + 1, item.title))
                                        .selected(self.selected == Some(index))
                                        .sense(egui::Sense::click_and_drag())
                                        .truncate(),
                                )
                                .on_hover_text(item.path.display().to_string());
                            dragdrop::source(&response, item.clone(), Origin::Playlist(index));
                            if response.clicked() {
                                self.selected = Some(index);
                            }
                            if response.double_clicked() && item.available != Some(false) {
                                load = Some((active, item.path.clone()));
                            }
                        });
                        let summary = item.summary.as_ref();
                        let bpm = summary
                            .and_then(|s| s.bpm())
                            .map_or("— BPM".into(), |b| format!("{b:.1} BPM"));
                        let key = summary
                            .and_then(|s| s.key)
                            .map_or("Key —".into(), |k| k.label());
                        let length = summary.map_or("".into(), |s| {
                            format!(
                                " · {}:{:02}",
                                s.duration_ms / 60000,
                                s.duration_ms / 1000 % 60
                            )
                        });
                        ui.label(
                            RichText::new(if item.available == Some(false) {
                                "File missing · Check the path".into()
                            } else {
                                format!("{bpm} · {key}{length}")
                            })
                            .small()
                            .color(if item.available == Some(false) {
                                egui::Color32::LIGHT_RED
                            } else {
                                ui.visuals().weak_text_color()
                            }),
                        );
                        ui.separator();
                    });
                    drops.playlist_row(ui, row.response.rect, index);
                }
            });
        load
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> tempfile::TempDir {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("work/playlist-tests");
        std::fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }

    #[test]
    fn save_reopen_preserves_unicode_order_duplicates_and_missing_files() {
        let temp = directory();
        let first = temp.path().join("Ärger #1.flac");
        let second = temp.path().join("Noch nicht da.mp3");
        let mut playlist = Playlist {
            items: vec![
                Item::new(first.clone(), Some("Grüße, Welt".into())),
                Item::new(second.clone(), None),
                Item::new(first.clone(), None),
            ],
            selected: Some(1),
            ..Default::default()
        };
        playlist.move_selected(-1);
        let target = temp.path().join("Mein Set.m3u8");
        playlist.save_to(target.clone()).unwrap();
        assert!(!playlist.modified);
        let mut loaded = Playlist::default();
        loaded.open(target.clone()).unwrap();
        assert_eq!(
            loaded.items.iter().map(|i| &i.path).collect::<Vec<_>>(),
            vec![&second, &first, &first]
        );
        assert_eq!(loaded.items[1].title, "Grüße, Welt");
        loaded.selected = Some(0);
        loaded.remove_selected();
        loaded.save_to(target.clone()).unwrap();
        let (again, _) = read_playlist(&target).unwrap();
        assert_eq!(again.len(), 2);
        assert_eq!(again[0].path, first);
    }

    #[test]
    fn imports_bom_crlf_relative_paths_comments_and_skips_remote_entries() {
        let temp = directory();
        let file = temp.path().join("set.m3u8");
        std::fs::write(&file, "\u{feff}#EXTM3U\r\n# Comment\r\n#EXTINF:123,Ein Titel\r\nmusic/song.mp3\r\nhttps://example.invalid/live.mp3\r\ncover.png\r\nmusic/next.flac\r\n").unwrap();
        let (items, skipped) = read_playlist(&file).unwrap();
        assert_eq!(skipped, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].path, temp.path().join("music/song.mp3"));
        assert_eq!(items[0].title, "Ein Titel");
        assert_eq!(items[1].title, "next");
    }

    #[test]
    fn failed_load_or_save_keeps_the_current_playlist_and_dirty_state() {
        let temp = directory();
        let invalid = temp.path().join("bad.m3u");
        std::fs::write(&invalid, [0xff, 0xfe, 0x00]).unwrap();
        let mut playlist = Playlist::default();
        playlist
            .items
            .push(Item::new(temp.path().join("track.mp3"), None));
        playlist.modified = true;
        assert!(playlist.open(invalid).is_err());
        assert_eq!(playlist.items.len(), 1);
        assert!(playlist.modified);
        assert!(playlist.path.is_none());
        let invalid_target = temp.path().join("missing-directory/set.m3u8");
        assert!(playlist.save_to(invalid_target).is_err());
        assert!(playlist.modified);
        assert!(playlist.path.is_none());
        let stream = temp.path().join("hls.m3u8");
        std::fs::write(&stream, "#EXTM3U\n#EXT-X-TARGETDURATION:10\nsegment.aac\n").unwrap();
        assert!(read_playlist(&stream).is_err());
    }

    #[test]
    fn inserting_between_items_and_drag_reordering_preserve_the_saved_order() {
        let temp = directory();
        let a = temp.path().join("A.mp3");
        let b = temp.path().join("B.flac");
        let c = temp.path().join("C.mp3");
        let mut playlist = Playlist::default();
        playlist.insert_items(
            vec![Item::new(a.clone(), None), Item::new(c.clone(), None)],
            None,
        );
        playlist.insert_items(vec![Item::new(b.clone(), None)], Some(1));
        assert!(playlist.modified);
        playlist.move_to(0, None);
        playlist.move_to(1, Some(0));
        let file = temp.path().join("dragged.m3u8");
        playlist.save_to(file.clone()).unwrap();
        let (items, _) = read_playlist(&file).unwrap();
        assert_eq!(
            items.iter().map(|i| &i.path).collect::<Vec<_>>(),
            vec![&c, &b, &a]
        );
        assert_eq!(items.len(), 3);
    }
}
