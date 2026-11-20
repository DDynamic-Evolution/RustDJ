use crate::{
    browser::BrowserSettings,
    decks::Layout,
    midi::{Binding, MidiPrefs},
    output::Routing,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub browser: BrowserSettings,
    pub deck_layout: Layout,
    pub bindings: Option<Vec<Binding>>,
    pub routing: Routing,
    pub mix: MixSettings,
    pub mapping_version: u32,
    pub midi: MidiPrefs,
    pub streamer: crate::streamer::StreamSettings,
    pub stream_profiles: Vec<crate::streamer::StreamProfile>,
    pub active_stream_profile: u64,
    pub stream_profile_serial: u64,
    pub inputs: [crate::inputs::InputSettings; 2],
    pub microphone: crate::inputs::InputSettings,
    pub talkover: crate::broadcast_mix::TalkoverSettings,
    pub master_limiter: bool,
    pub jingles: crate::jingles::Settings,
    pub master_eq: crate::equalizer::Settings,
    pub autodj: Vec<PathBuf>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct MixSettings {
    pub master: f32,
    pub phones: f32,
    pub mixing: f32,
    pub crossfader_enabled: bool,
}
impl Default for MixSettings {
    fn default() -> Self {
        Self {
            master: 0.7,
            phones: 0.4,
            mixing: 0.0,
            crossfader_enabled: true,
        }
    }
}

fn path() -> PathBuf {
    #[cfg(target_os = "linux")]
    return linux_data_path("XDG_CONFIG_HOME", ".config", "settings.json");

    #[cfg(not(target_os = "linux"))]
    return config_dir().join("settings.json");
}

pub(crate) fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("RustDJ")
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_data_path(variable: &str, default_dir: &str, file: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.as_ref().map(|path| path.join(default_dir)))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("RustDJ")
        .join(file);

    let legacy = config_dir().join(file);
    if !path.exists() && legacy.is_file() {
        return legacy;
    }
    path
}

pub fn load() -> (Settings, Option<String>) {
    load_from(&path())
}

pub(crate) fn load_from(path: &Path) -> (Settings, Option<String>) {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(settings) => (settings, None),
            Err(error) => (
                Settings::default(),
                Some(format!("Could not read settings: {error}")),
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(error) => (
            Settings::default(),
            Some(format!("Could not read settings: {error}")),
        ),
    }
}

pub fn save(settings: &Settings) -> Result<(), String> {
    save_to(&path(), settings)
}

pub(crate) fn save_to(path: &Path, settings: &Settings) -> Result<(), String> {
    let parent = path.parent().ok_or("No settings folder")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| format!("Settings were not saved: {e}"))
}
