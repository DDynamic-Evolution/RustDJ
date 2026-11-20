use crate::{
    output::{Audio, ChannelPair},
    track,
};
use rodio::Sink;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const COUNT: usize = 4;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub paths: [Option<PathBuf>; COUNT],
    pub level: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            paths: std::array::from_fn(|_| None),
            level: 0.65,
        }
    }
}

#[derive(Default)]
pub struct Pads {
    pub settings: Settings,
    voices: [Option<Sink>; COUNT],
}

impl Pads {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn play(&mut self, index: usize, audio: &Audio, master: f32) -> Result<(), String> {
        let path = self
            .settings
            .paths
            .get(index)
            .and_then(Option::as_ref)
            .ok_or("Choose an audio file for this jingle pad.")?;
        let output = audio
            .stream
            .as_ref()
            .ok_or("Connect a master output first.")?;
        let bus = audio
            .bus(false)
            .ok_or("The master mixing bus is unavailable.")?;
        let decoder = track::open_decoder(path)?;
        self.stop(index);
        let sink = Sink::connect_new(bus);
        sink.set_volume(self.settings.level.clamp(0.0, 1.0) * master);
        sink.append(ChannelPair::new(
            decoder,
            output.config().channel_count(),
            audio.routing.master.pair,
        ));
        self.voices[index] = Some(sink);
        Ok(())
    }

    pub fn playing(&self, index: usize) -> bool {
        self.voices[index]
            .as_ref()
            .is_some_and(|sink| !sink.empty())
    }

    pub fn stop(&mut self, index: usize) {
        if let Some(sink) = self.voices[index].take() {
            sink.stop();
        }
    }

    pub fn stop_all(&mut self) {
        for index in 0..COUNT {
            self.stop(index);
        }
    }

    pub fn update(&mut self, master: f32) {
        for voice in &mut self.voices {
            match voice {
                Some(sink) if !sink.empty() => {
                    sink.set_volume(self.settings.level.clamp(0.0, 1.0) * master)
                }
                _ => *voice = None,
            }
        }
    }
}
