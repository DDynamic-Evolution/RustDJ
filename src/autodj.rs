use crate::{
    output::{Audio, ChannelPair},
    track,
};
use eframe::egui;
use rodio::{Decoder, Sink};
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

type Loaded = Result<(Decoder<BufReader<File>>, String), String>;

#[derive(Default)]
enum Playback {
    #[default]
    Stopped,
    Loading(usize, Receiver<Loaded>),
    Playing(usize, Sink, String),
}

#[derive(Default)]
pub struct AutoDj {
    paths: Vec<PathBuf>,
    playback: Playback,
    selected: Option<usize>,
    error: Option<String>,
    finished: bool,
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
        let changed = self.paths.len() != before;
        if changed {
            self.finished = false;
        }
        changed
    }

    fn current_index(&self) -> Option<usize> {
        match &self.playback {
            Playback::Stopped => None,
            Playback::Loading(index, _) | Playback::Playing(index, _, _) => Some(*index),
        }
    }

    pub fn current_title(&self) -> Option<&str> {
        match &self.playback {
            Playback::Playing(_, sink, title) if !sink.empty() => Some(title),
            _ => None,
        }
    }

    pub fn stop(&mut self) -> bool {
        let running = self.current_index().is_some();
        self.playback = Playback::Stopped;
        self.finished = false;
        running
    }

    fn load(&mut self, index: usize, ctx: &egui::Context) {
        let Some(path) = self.paths.get(index).cloned() else {
            self.playback = Playback::Stopped;
            self.finished = true;
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = track::open_decoder(&path).map(|decoder| {
                let tags = track::read_tags(&path);
                let title = match (tags.artist.is_empty(), tags.title.is_empty()) {
                    (_, true) => file_title(&path),
                    (true, false) => tags.title,
                    (false, false) => format!("{} · {}", tags.artist, tags.title),
                };
                (decoder, title)
            });
            let _ = sender.send(result);
            ctx.request_repaint();
        });
        self.playback = Playback::Loading(index, receiver);
    }

    pub fn update(&mut self, audio: &Audio, gain: f32, ctx: &egui::Context) {
        let (index, result) = match &self.playback {
            Playback::Stopped => return,
            Playback::Playing(index, sink, _) => {
                if sink.empty() {
                    self.load(index + 1, ctx);
                } else {
                    sink.set_volume(gain);
                }
                return;
            }
            Playback::Loading(index, receiver) => {
                let result = match receiver.try_recv() {
                    Ok(result) => result,
                    Err(TryRecvError::Empty) => return,
                    Err(TryRecvError::Disconnected) => Err("Audio loading stopped.".into()),
                };
                (*index, result)
            }
        };
        match result {
            Ok((decoder, title)) => {
                let (Some(output), Some(bus)) = (&audio.stream, audio.bus(false)) else {
                    self.stop();
                    self.error = Some("Connect a master output to use AutoDJ.".into());
                    return;
                };
                let sink = Sink::connect_new(bus);
                sink.set_volume(gain);
                sink.append(ChannelPair::new(
                    decoder,
                    output.config().channel_count(),
                    audio.routing.master.pair,
                ));
                self.playback = Playback::Playing(index, sink, title);
            }
            Err(error) => {
                self.error = Some(format!(
                    "Skipped {}. {error}",
                    file_title(&self.paths[index])
                ));
                self.load(index + 1, ctx);
            }
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        audio_ready: bool,
        browser_selection: Option<PathBuf>,
        playlist: &[crate::playlist::Item],
    ) -> bool {
        let current = self.current_index();
        let idle = current.is_none();
        let mut changed = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    idle && audio_ready && !self.paths.is_empty(),
                    egui::Button::new("Start AutoDJ"),
                )
                .on_disabled_hover_text("Add tracks and connect a master output to start AutoDJ.")
                .clicked()
            {
                self.error = None;
                self.finished = false;
                self.load(0, ctx);
            }
            if ui.add_enabled(!idle, egui::Button::new("Stop")).clicked() {
                self.stop();
            }
            if ui.add_enabled(!idle, egui::Button::new("Next")).clicked()
                && let Some(index) = current
            {
                self.load(index + 1, ctx);
            }
        });
        let status = match &self.playback {
            Playback::Stopped if self.finished => String::from("Playlist finished"),
            Playback::Stopped => format!("{} tracks", self.paths.len()),
            Playback::Loading(index, _) => format!("Loading {}", file_title(&self.paths[*index])),
            Playback::Playing(index, _, title) => {
                format!("{}/{} · {title}", index + 1, self.paths.len())
            }
        };
        ui.add(egui::Label::new(&status).truncate())
            .on_hover_text(status);
        if let Some(error) = &self.error {
            ui.add(
                egui::Label::new(egui::RichText::new(error).color(egui::Color32::LIGHT_RED))
                    .truncate(),
            )
            .on_hover_text(error);
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    browser_selection.is_some(),
                    egui::Button::new("Add selection"),
                )
                .on_hover_text("Add the selected track from the music browser.")
                .clicked()
                && let Some(path) = browser_selection
            {
                changed |= self.add([path]);
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
        if let Some(index) = self.selected {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(idle && index > 0, egui::Button::new("Move up"))
                    .clicked()
                {
                    self.paths.swap(index, index - 1);
                    self.selected = Some(index - 1);
                    changed = true;
                }
                if ui
                    .add_enabled(
                        idle && index + 1 < self.paths.len(),
                        egui::Button::new("Move down"),
                    )
                    .clicked()
                {
                    self.paths.swap(index, index + 1);
                    self.selected = Some(index + 1);
                    changed = true;
                }
                if ui.add_enabled(idle, egui::Button::new("Remove")).clicked() {
                    self.paths.remove(index);
                    self.selected = None;
                    changed = true;
                }
            });
        }
        ui.separator();
        egui::ScrollArea::vertical().show_rows(ui, 22.0, self.paths.len(), |ui, rows| {
            for index in rows {
                let path = &self.paths[index];
                let title = if current == Some(index) {
                    egui::RichText::new(format!("▶ {}", file_title(path))).strong()
                } else {
                    egui::RichText::new(file_title(path))
                };
                if ui
                    .add(egui::Button::selectable(self.selected == Some(index), title).truncate())
                    .on_hover_text(path.display().to_string())
                    .clicked()
                {
                    self.selected = Some(index);
                }
            }
        });
        if changed {
            self.finished = false;
        }
        changed
    }
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
