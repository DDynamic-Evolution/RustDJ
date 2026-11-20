use crate::{beat::BeatGrid, key::MusicalKey, track::Track};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    fs::Metadata,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const ANALYSIS_VERSION: i64 = 2;

pub fn default_path() -> PathBuf {
    if let Some(base) = std::env::var_os("RUSTDJ_DATA_DIR") {
        return PathBuf::from(base).join("library.sqlite3");
    }
    #[cfg(target_os = "linux")]
    return crate::settings::linux_data_path("XDG_DATA_HOME", ".local/share", "library.sqlite3");

    #[cfg(not(target_os = "linux"))]
    return crate::settings::config_dir().join("library.sqlite3");
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub bytes: i64,
    pub modified_ns: String,
}
impl Fingerprint {
    pub fn read(path: &Path) -> Result<Self, String> {
        Self::from_metadata(&std::fs::metadata(path).map_err(|e| e.to_string())?)
    }
    pub fn from_metadata(metadata: &Metadata) -> Result<Self, String> {
        let modified = metadata.modified().map_err(|e| e.to_string())?;
        Ok(Self {
            bytes: metadata.len().try_into().map_err(|_| "File is too large")?,
            modified_ns: match modified.duration_since(UNIX_EPOCH) {
                Ok(d) => d.as_nanos().to_string(),
                Err(e) => format!("-{}", e.duration().as_nanos()),
            },
        })
    }
}

fn path_key(path: &Path) -> String {
    let path = std::fs::canonicalize(path)
        .unwrap_or_else(|_| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
    let key = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    let key = key.to_lowercase();
    key
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub duration_ms: u64,
    pub grid: Option<BeatGrid>,
    pub key: Option<MusicalKey>,
}
impl Summary {
    pub fn from_track(track: &Track) -> Self {
        Self {
            duration_ms: track.duration.as_millis() as u64,
            grid: track.grid,
            key: track.key,
        }
    }
    pub fn bpm(&self) -> Option<f32> {
        self.grid.map(|grid| grid.bpm)
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    summary: Summary,
    duration_ns: u64,
    sample_rate: u32,
    channels: u16,
    envelope_step: f64,
}
impl Stored {
    fn valid(&self) -> bool {
        self.sample_rate > 0
            && self.channels > 0
            && self.duration_ns > 0
            && self.envelope_step.is_finite()
            && self.envelope_step > 0.0
            && self.summary.grid.is_none_or(|g| {
                g.bpm.is_finite()
                    && (20.0..=400.0).contains(&g.bpm)
                    && g.offset.is_finite()
                    && g.confidence.is_finite()
            })
            && self
                .summary
                .key
                .is_none_or(|k| k.pitch < 12 && k.confidence.is_finite())
    }
}

#[derive(Clone, Debug)]
pub enum Lookup {
    Missing,
    Stale,
    Fresh(Summary),
}

pub struct HistoryEntry {
    pub local_time: String,
    pub utc_time: String,
    pub title: String,
    pub path: String,
    pub deck: usize,
}

pub struct Database {
    connection: Connection,
}
impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Database folder: {e}"))?;
        }
        let connection = Connection::open(path).map_err(|e| format!("Music database: {e}"))?;
        connection
            .busy_timeout(Duration::from_secs(3))
            .map_err(|e| e.to_string())?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if version > 1 {
            return Err("The music database was created by a newer version of RustDJ.".into());
        }
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS analyses (
                path TEXT PRIMARY KEY, file_size INTEGER NOT NULL, modified_ns TEXT NOT NULL,
                analysis_version INTEGER NOT NULL, result_json TEXT NOT NULL,
                waveform BLOB NOT NULL, envelope BLOB NOT NULL, analysed_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS hotcues (
                path TEXT PRIMARY KEY, file_size INTEGER NOT NULL, modified_ns TEXT NOT NULL,
                cues_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS colour_waveforms (
                path TEXT PRIMARY KEY, file_size INTEGER NOT NULL, modified_ns TEXT NOT NULL,
                version INTEGER NOT NULL, frames INTEGER NOT NULL, step REAL NOT NULL, data BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS beatgrid_corrections (
                path TEXT PRIMARY KEY, file_size INTEGER NOT NULL, modified_ns TEXT NOT NULL,
                bpm REAL NOT NULL, anchor REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS play_history (
                id INTEGER PRIMARY KEY, played_at INTEGER NOT NULL, title TEXT NOT NULL,
                path TEXT NOT NULL, deck INTEGER NOT NULL
            );",
            )
            .map_err(|e| format!("Could not initialize music database: {e}"))?;
        if version == 0 {
            connection
                .pragma_update(None, "user_version", 1)
                .map_err(|e| e.to_string())?;
        }
        Ok(Self { connection })
    }

    pub fn grid_correction(
        &self,
        path: &Path,
        file: &Fingerprint,
    ) -> Result<Option<BeatGrid>, String> {
        self.connection.query_row(
            "SELECT bpm, anchor FROM beatgrid_corrections WHERE path=?1 AND file_size=?2 AND modified_ns=?3",
            params![path_key(path), file.bytes, file.modified_ns],
            |row| Ok(BeatGrid { bpm: row.get(0)?, offset: row.get(1)?, confidence: 1.0 }),
        ).optional().map_err(|error| error.to_string())
    }

    pub fn save_grid_correction(
        &self,
        path: &Path,
        file: &Fingerprint,
        grid: Option<BeatGrid>,
    ) -> Result<(), String> {
        if let Some(grid) = grid {
            if !(40.0..=250.0).contains(&grid.bpm) || !grid.offset.is_finite() || grid.offset < 0.0
            {
                return Err("Enter a valid tempo and first beat.".into());
            }
            self.connection.execute(
                "INSERT INTO beatgrid_corrections VALUES(?1,?2,?3,?4,?5) ON CONFLICT(path) DO UPDATE SET
                 file_size=excluded.file_size,modified_ns=excluded.modified_ns,bpm=excluded.bpm,anchor=excluded.anchor",
                params![path_key(path), file.bytes, file.modified_ns, grid.bpm, grid.offset],
            ).map_err(|error| error.to_string())?;
        } else {
            self.connection.execute(
                "DELETE FROM beatgrid_corrections WHERE path=?1 AND file_size=?2 AND modified_ns=?3",
                params![path_key(path), file.bytes, file.modified_ns],
            ).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn log_play(
        &self,
        title: &str,
        path: &Path,
        deck: usize,
        timestamp: i64,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO play_history(played_at,title,path,deck) VALUES(?1,?2,?3,?4)",
                params![timestamp, title, path.to_string_lossy(), deck],
            )
            .map_err(|error| format!("Could not save play history: {error}"))?;
        Ok(())
    }

    pub fn history(&self, limit: i64) -> Result<Vec<HistoryEntry>, String> {
        let mut query = self
            .connection
            .prepare(
                "SELECT datetime(played_at,'unixepoch','localtime'),
                    strftime('%Y-%m-%dT%H:%M:%SZ',played_at,'unixepoch'),title,path,deck
             FROM play_history ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|error| error.to_string())?;
        let rows = query
            .query_map([limit], |row| {
                Ok(HistoryEntry {
                    local_time: row.get(0)?,
                    utc_time: row.get(1)?,
                    title: row.get(2)?,
                    path: row.get(3)?,
                    deck: row.get(4)?,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<_, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn lookup(&self, path: &Path, identity: &Fingerprint) -> Result<Lookup, String> {
        let row: Option<(i64, String, i64, String)> = self.connection.query_row(
            "SELECT file_size, modified_ns, analysis_version, result_json FROM analyses WHERE path=?1",
            [path_key(path)], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        ).optional().map_err(|e| e.to_string())?;
        let Some((size, modified, version, json)) = row else {
            return Ok(Lookup::Missing);
        };
        if size != identity.bytes || modified != identity.modified_ns || version != ANALYSIS_VERSION
        {
            return Ok(Lookup::Stale);
        }
        match serde_json::from_str::<Stored>(&json) {
            Ok(stored) if stored.valid() => Ok(Lookup::Fresh(stored.summary)),
            _ => Ok(Lookup::Stale),
        }
    }

    pub fn load_track(&self, path: &Path, identity: &Fingerprint) -> Result<Option<Track>, String> {
        if !matches!(self.lookup(path, identity)?, Lookup::Fresh(_)) {
            return Ok(None);
        }
        let (json, overview, envelope): (String, Vec<u8>, Vec<u8>) = self.connection.query_row(
            "SELECT result_json,waveform,envelope FROM analyses WHERE path=?1 AND length(envelope)<=134217728",
            [path_key(path)], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        ).optional().map_err(|e| e.to_string())?.unwrap_or_default();
        let Ok(stored) = serde_json::from_str::<Stored>(&json) else {
            return Ok(None);
        };
        let (Some(waveform), Some(envelope)) = (decode_peaks(&overview), decode_peaks(&envelope))
        else {
            return Ok(None);
        };
        if waveform.is_empty() || envelope.is_empty() || !stored.valid() {
            return Ok(None);
        }
        let coloured: Option<(usize, f64, Vec<u8>)> = self.connection.query_row(
            "SELECT frames,step,data FROM colour_waveforms WHERE path=?1 AND file_size=?2 AND modified_ns=?3 AND version=1 AND length(data)<=67108864",
            params![path_key(path), identity.bytes, identity.modified_ns], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        ).optional().map_err(|e| e.to_string())?;
        let Some((count, step, bytes)) = coloured else {
            return Ok(None);
        };
        let Some(detailed) = crate::waveform::Waveform::decode(&bytes, count, step) else {
            return Ok(None);
        };
        Ok(Some(Track {
            path: path.to_path_buf(),
            title: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            duration: Duration::from_nanos(stored.duration_ns),
            waveform,
            sample_rate: stored.sample_rate,
            channels: stored.channels,
            grid: stored.summary.grid,
            key: stored.summary.key,
            envelope: Arc::from(envelope),
            envelope_step: stored.envelope_step,
            from_cache: true,
            cache_warning: None,
            detailed: Arc::new(detailed),
        }))
    }

    pub fn save(&self, track: &Track, identity: &Fingerprint) -> Result<(), String> {
        let stored = Stored {
            summary: Summary::from_track(track),
            duration_ns: track
                .duration
                .as_nanos()
                .try_into()
                .map_err(|_| "Track is too long")?,
            sample_rate: track.sample_rate,
            channels: track.channels,
            envelope_step: track.envelope_step,
        };
        let json = serde_json::to_string(&stored).map_err(|e| e.to_string())?;
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        self.connection.execute("INSERT INTO analyses(path,file_size,modified_ns,analysis_version,result_json,waveform,envelope,analysed_at)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(path) DO UPDATE SET
            file_size=excluded.file_size,modified_ns=excluded.modified_ns,analysis_version=excluded.analysis_version,
            result_json=excluded.result_json,waveform=excluded.waveform,envelope=excluded.envelope,analysed_at=excluded.analysed_at",
            params![path_key(&track.path), identity.bytes, identity.modified_ns, ANALYSIS_VERSION, json, encode_peaks(&track.waveform), encode_peaks(&track.envelope), time]
        ).map_err(|e| format!("Analysis was not saved: {e}"))?;
        self.connection.execute(
            "INSERT INTO colour_waveforms(path,file_size,modified_ns,version,frames,step,data) VALUES(?1,?2,?3,1,?4,?5,?6)
            ON CONFLICT(path) DO UPDATE SET file_size=excluded.file_size,modified_ns=excluded.modified_ns,
            version=excluded.version,frames=excluded.frames,step=excluded.step,data=excluded.data",
            params![path_key(&track.path), identity.bytes, identity.modified_ns, track.detailed.frames().len() as i64, track.detailed.step, track.detailed.encode()?]
        ).map_err(|e| format!("Color waveform was not saved: {e}"))?;
        Ok(())
    }

    pub fn count(&self) -> Result<u64, String> {
        self.connection
            .query_row("SELECT count(*) FROM analyses", [], |row| row.get(0))
            .map_err(|e| e.to_string())
    }

    pub fn hotcues(
        &self,
        path: &Path,
        identity: &Fingerprint,
    ) -> Result<crate::performance::Hotcues, String> {
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT cues_json FROM hotcues WHERE path=?1 AND file_size=?2 AND modified_ns=?3",
                params![path_key(path), identity.bytes, identity.modified_ns],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or([None; 8]))
    }

    pub fn save_hotcues(
        &self,
        path: &Path,
        identity: &Fingerprint,
        cues: crate::performance::Hotcues,
    ) -> Result<(), String> {
        let json = serde_json::to_string(&cues).map_err(|e| e.to_string())?;
        self.connection.execute(
            "INSERT INTO hotcues(path,file_size,modified_ns,cues_json) VALUES(?1,?2,?3,?4)
            ON CONFLICT(path) DO UPDATE SET file_size=excluded.file_size,modified_ns=excluded.modified_ns,cues_json=excluded.cues_json",
            params![path_key(path), identity.bytes, identity.modified_ns, json]
        ).map_err(|e| format!("Hotcues were not saved: {e}"))?;
        Ok(())
    }
}

fn encode_peaks(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
fn decode_peaks(bytes: &[u8]) -> Option<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| {
            let value = f32::from_le_bytes(*bytes);
            (value.is_finite() && (0.0..=1.0).contains(&value)).then_some(value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    #[test]
    fn saved_grid_and_play_history_survive_database_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.sqlite3");
        let track = directory.path().join("track.mp3");
        let identity = Fingerprint {
            bytes: 44,
            modified_ns: "1".into(),
        };
        let grid = BeatGrid {
            bpm: 128.25,
            offset: 2.175,
            confidence: 1.0,
        };
        {
            let db = Database::open(&path).unwrap();
            db.save_grid_correction(&track, &identity, Some(grid))
                .unwrap();
            db.log_play("Track", &track, 2, 1_762_000_000).unwrap();
        }
        let db = Database::open(&path).unwrap();
        let restored = db.grid_correction(&track, &identity).unwrap().unwrap();
        assert_eq!((restored.bpm, restored.offset), (grid.bpm, grid.offset));
        let different = Fingerprint {
            bytes: 45,
            ..identity.clone()
        };
        assert!(db.grid_correction(&track, &different).unwrap().is_none());
        db.save_grid_correction(&track, &identity, None).unwrap();
        assert!(db.grid_correction(&track, &identity).unwrap().is_none());
        let history = db.history(10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].deck, 2);
        assert_eq!(history[0].title, "Track");
        assert!(history[0].utc_time.ends_with('Z'));
    }

    #[test]
    fn sqlite_persists_full_analysis_and_invalidates_changed_files_and_versions() {
        let temporary = tempfile::tempdir().unwrap();
        let file = temporary.path().join("track.wav");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo-a.wav"),
            &file,
        )
        .unwrap();
        let identity = Fingerprint::read(&file).unwrap();
        let track = crate::track::analyze(file.clone(), &AtomicBool::new(false)).unwrap();
        let db_path = temporary.path().join("library.sqlite3");
        {
            let db = Database::open(&db_path).unwrap();
            assert!(matches!(
                db.lookup(&file, &identity).unwrap(),
                Lookup::Missing
            ));
            db.save(&track, &identity).unwrap();
        }
        let db = Database::open(&db_path).unwrap();
        let loaded = db.load_track(&file, &identity).unwrap().unwrap();
        assert!(loaded.from_cache);
        assert_eq!(loaded.duration, track.duration);
        assert_eq!(loaded.envelope.as_ref(), track.envelope.as_ref());
        assert_eq!(loaded.key, track.key);
        assert_eq!(db.count().unwrap(), 1);
        let changed = Fingerprint {
            bytes: identity.bytes + 1,
            ..identity.clone()
        };
        assert!(matches!(db.lookup(&file, &changed).unwrap(), Lookup::Stale));
        let changed = Fingerprint {
            modified_ns: "different".into(),
            ..identity.clone()
        };
        assert!(db.load_track(&file, &changed).unwrap().is_none());
        db.connection
            .execute("UPDATE analyses SET analysis_version=0", [])
            .unwrap();
        assert!(matches!(
            db.lookup(&file, &identity).unwrap(),
            Lookup::Stale
        ));
    }
}
