use crate::{
    output::{Audio, ChannelPair},
    track,
};
use eframe::egui;
use rodio::{Sink, mixer::Mixer};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

enum Playback {
    Loading(usize),
    Playing {
        index: usize,
        sink: Sink,
        title: String,
    },
    Stopped,
    Finished,
}

impl Playback {
    fn index(&self) -> Option<usize> {
        match self {
            Self::Loading(index) | Self::Playing { index, .. } => Some(*index),
            Self::Stopped | Self::Finished => None,
        }
    }
}

struct Run {
    paths: Vec<PathBuf>,
    playback: Playback,
    gain: f32,
    error: Option<String>,
}

struct Output {
    mixer: Mixer,
    channels: u16,
    pair: u16,
}

#[derive(Default)]
pub struct AutoDj {
    paths: Vec<PathBuf>,
    run: Option<Arc<Mutex<Run>>>,
    selected: Option<usize>,
}

impl AutoDj {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            run: None,
            selected: None,
        }
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> bool {
        let before = self.paths.len();
        self.paths.extend(paths);
        if before == self.paths.len() {
            return false;
        }
        if let Some(run) = &self.run {
            let mut run = run.lock().unwrap();
            if run.playback.index().is_some() {
                run.paths.extend_from_slice(&self.paths[before..]);
            } else {
                run.playback = Playback::Stopped;
                run.error = None;
            }
        }
        true
    }

    pub fn current_title(&self) -> Option<String> {
        let run = self.run.as_ref()?.lock().unwrap();
        match &run.playback {
            Playback::Playing { sink, title, .. } if !sink.empty() => Some(title.clone()),
            _ => None,
        }
    }

    pub fn stop(&mut self) -> bool {
        let Some(run) = self.run.take() else {
            return false;
        };
        let mut run = run.lock().unwrap();
        let running = run.playback.index().is_some();
        run.playback = Playback::Stopped;
        running
    }

    fn next(&mut self) {
        if let Some(run) = &self.run {
            let mut run = run.lock().unwrap();
            if let Some(index) = run.playback.index() {
                run.playback = Playback::Loading(index + 1);
            }
        }
    }

    fn start(&mut self, output: Output, gain: f32, ctx: &egui::Context) {
        self.stop();
        let run = Arc::new(Mutex::new(Run {
            paths: self.paths.clone(),
            playback: Playback::Loading(0),
            gain,
            error: None,
        }));
        let worker = Arc::clone(&run);
        let ctx = ctx.clone();
        thread::spawn(move || play_queue(worker, output, ctx));
        self.run = Some(run);
    }

    pub fn set_gain(&mut self, gain: f32) {
        if let Some(run) = &self.run {
            let mut run = run.lock().unwrap();
            run.gain = gain;
            if let Playback::Playing { sink, .. } = &run.playback {
                sink.set_volume(gain);
            }
        }
    }

    fn status(&self) -> (Option<usize>, String, Option<String>) {
        let Some(run) = &self.run else {
            return (None, format!("{} tracks", self.paths.len()), None);
        };
        let run = run.lock().unwrap();
        let status = match &run.playback {
            Playback::Finished => String::from("Playlist finished"),
            Playback::Stopped => format!("{} tracks", self.paths.len()),
            Playback::Loading(index) => run.paths.get(*index).map_or_else(
                || String::from("Finishing playlist …"),
                |path| {
                    format!(
                        "Loading {}/{} · {}",
                        index + 1,
                        run.paths.len(),
                        file_title(path)
                    )
                },
            ),
            Playback::Playing { index, title, .. } => {
                format!("Playing {}/{} · {title}", index + 1, run.paths.len())
            }
        };
        (run.playback.index(), status, run.error.clone())
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        audio: &Audio,
        gain: f32,
        browser_selection: Option<PathBuf>,
        playlist: &[crate::playlist::Item],
    ) -> bool {
        let (current, status, error) = self.status();
        let idle = current.is_none();
        let master = audio.stream.as_ref().zip(audio.bus(false));
        let mut changed = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    idle && master.is_some() && !self.paths.is_empty(),
                    egui::Button::new("Start AutoDJ"),
                )
                .on_disabled_hover_text("Add tracks and connect a master output to start AutoDJ.")
                .clicked()
                && let Some((output, mixer)) = master
            {
                self.start(
                    Output {
                        mixer: mixer.clone(),
                        channels: output.config().channel_count(),
                        pair: audio.routing.master.pair,
                    },
                    gain,
                    ctx,
                );
            }
            if ui.add_enabled(!idle, egui::Button::new("Stop")).clicked() {
                self.stop();
            }
            if ui.add_enabled(!idle, egui::Button::new("Next")).clicked() {
                self.next();
            }
        });
        ui.add(egui::Label::new(&status).truncate())
            .on_hover_text(status);
        if let Some(error) = &error {
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
                    self.run = None;
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
                    self.run = None;
                    changed = true;
                }
                if ui.add_enabled(idle, egui::Button::new("Remove")).clicked() {
                    self.paths.remove(index);
                    self.selected = None;
                    self.run = None;
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
        changed
    }
}

impl Drop for AutoDj {
    fn drop(&mut self) {
        self.stop();
    }
}

fn play_queue(run: Arc<Mutex<Run>>, output: Output, ctx: egui::Context) {
    loop {
        let (index, path) = {
            let mut state = run.lock().unwrap();
            let index = match &state.playback {
                Playback::Stopped | Playback::Finished => return,
                Playback::Loading(index) => *index,
                Playback::Playing { index, sink, .. } if sink.empty() => index + 1,
                Playback::Playing { .. } => {
                    drop(state);
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
            };
            let Some(path) = state.paths.get(index).cloned() else {
                state.playback = Playback::Finished;
                drop(state);
                ctx.request_repaint();
                return;
            };
            state.playback = Playback::Loading(index);
            (index, path)
        };
        ctx.request_repaint();

        let loaded = track::open_decoder(&path).map(|decoder| {
            let tags = track::read_tags(&path);
            let title = match (tags.artist.is_empty(), tags.title.is_empty()) {
                (_, true) => file_title(&path),
                (true, false) => tags.title,
                (false, false) => format!("{} · {}", tags.artist, tags.title),
            };
            (decoder, title)
        });

        let mut state = run.lock().unwrap();
        // Stop and Next can change the playback state while a file is opening.
        if !matches!(state.playback, Playback::Loading(pending) if pending == index) {
            continue;
        }
        match loaded {
            Ok((decoder, title)) => {
                let sink = Sink::connect_new(&output.mixer);
                sink.set_volume(state.gain);
                sink.append(ChannelPair::new(decoder, output.channels, output.pair));
                state.playback = Playback::Playing { index, sink, title };
            }
            Err(error) => {
                state.error = Some(format!("Skipped {}. {error}", file_title(&path)));
                state.playback = Playback::Loading(index + 1);
            }
        }
        drop(state);
        ctx.request_repaint();
    }
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
