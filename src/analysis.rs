use crate::{
    database::{self, Database, Fingerprint, Summary},
    track::{self, Track},
};
use eframe::egui;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};

pub fn cached(
    db: &Database,
    path: PathBuf,
    cancelled: &AtomicBool,
    force: bool,
) -> Result<Track, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Analysis cancelled.".into());
    }
    let before = Fingerprint::read(&path)?;
    if !force
        && let Some(mut track) = db.load_track(&path, &before)?
        && Fingerprint::read(&path)? == before
    {
        fill_tags(db, &mut track, &before)?;
        return Ok(track);
    }
    let track = track::analyze(path.clone(), cancelled)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Analysis cancelled.".into());
    }
    if Fingerprint::read(&path)? != before {
        return Err("The file changed during analysis. Please analyze it again.".into());
    }
    db.save(&track, &before)?;
    Ok(track)
}

fn fill_tags(db: &Database, track: &mut Track, identity: &Fingerprint) -> Result<(), String> {
    if track.tags.is_none() {
        let tags = track::read_tags(&track.path);
        if &Fingerprint::read(&track.path)? != identity {
            return Err("The file changed while reading its tags.".into());
        }
        db.save_tags(&track.path, identity, &tags)?;
        track.tags = Some(tags);
    }
    Ok(())
}

/// A missing/unwritable database must not prevent audio playback.
pub fn load(path: PathBuf, cancelled: &AtomicBool) -> Result<Track, String> {
    let before = Fingerprint::read(&path)?;
    let (db, mut warning) = match Database::open(&database::default_path()) {
        Ok(db) => (Some(db), None),
        Err(error) => (None, Some(error)),
    };
    if let Some(db) = &db {
        match db.load_track(&path, &before) {
            Ok(Some(mut track))
                if !cancelled.load(Ordering::Relaxed) && Fingerprint::read(&path)? == before =>
            {
                if let Err(error) = fill_tags(db, &mut track, &before) {
                    track.cache_warning = Some(format!("Tags were not saved: {error}"));
                }
                return Ok(track);
            }
            Ok(_) => {}
            Err(error) => warning = Some(error),
        }
    }
    let mut track = track::analyze(path.clone(), cancelled)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Analysis cancelled.".into());
    }
    if Fingerprint::read(&path)? != before {
        return Err("The file changed during analysis.".into());
    }
    if let Some(db) = &db
        && let Err(error) = db.save(&track, &before)
    {
        warning = Some(error);
    }
    track.cache_warning = warning.map(|e| format!("Analysis was not saved to the database: {e}"));
    Ok(track)
}

fn analyze_summary(
    db: &Database,
    path: &Path,
    cancelled: &AtomicBool,
    force: bool,
) -> Result<(Summary, bool), String> {
    if !force {
        let identity = Fingerprint::read(path)?;
        if let database::Lookup::Fresh(mut summary) = db.lookup(path, &identity)? {
            if summary.tags.is_none() {
                let tags = track::read_tags(path);
                if Fingerprint::read(path)? != identity {
                    return Err("The file changed while reading its tags.".into());
                }
                if cancelled.load(Ordering::Relaxed) {
                    return Err("Analysis cancelled.".into());
                }
                db.save_tags(path, &identity, &tags)?;
                summary.tags = Some(tags);
            }
            return Ok((summary, true));
        }
    }
    cached(db, path.to_path_buf(), cancelled, force)
        .map(|track| (Summary::from_track(&track), track.from_cache))
}

pub enum Event {
    Discovered {
        total: usize,
        errors: Vec<String>,
        failures: usize,
    },
    Started(PathBuf),
    Completed {
        path: PathBuf,
        result: Result<Summary, String>,
        reused: bool,
    },
    Finished {
        cancelled: bool,
    },
    Fatal(String),
}

pub struct Job {
    pub receiver: Receiver<Event>,
    pub total: usize,
    pub discovering: bool,
    pub completed: usize,
    pub reused: usize,
    pub failures: usize,
    pub current: Option<PathBuf>,
    pub errors: Vec<String>,
    cancelled: Arc<AtomicBool>,
}
enum Batch {
    Files(Vec<PathBuf>, bool),
    Folder(PathBuf),
}

fn discover(folder: PathBuf, cancelled: &AtomicBool) -> (Vec<PathBuf>, Vec<String>, usize) {
    let mut files = Vec::new();
    let mut directories = vec![folder];
    let mut errors = Vec::new();
    let mut failures = 0;
    let mut record_error = |path: &Path, error: std::io::Error| {
        failures += 1;
        if errors.len() < 30 {
            errors.push(format!("{}: {error}", path.display()));
        }
    };
    while let Some(path) = directories.pop() {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let listing = match std::fs::read_dir(&path) {
            Ok(listing) => listing,
            Err(error) => {
                record_error(&path, error);
                continue;
            }
        };
        for entry in listing {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    record_error(&path, error);
                    continue;
                }
            };
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => directories.push(entry.path()),
                // Skip symlinks inside the tree, including links back to an ancestor.
                Ok(kind) if kind.is_file() && track::supported(&entry.path()) => {
                    files.push(entry.path())
                }
                Ok(_) => {}
                Err(error) => record_error(&entry.path(), error),
            }
        }
    }
    files.sort();
    (files, errors, failures)
}

impl Job {
    pub fn start(paths: Vec<PathBuf>, force: bool, database: &Path, ctx: &egui::Context) -> Self {
        let mut seen = HashSet::new();
        let paths: Vec<_> = paths
            .into_iter()
            .filter(|p| seen.insert(p.clone()))
            .collect();
        Self::launch(Batch::Files(paths, force), database, ctx)
    }

    pub fn start_folder(path: PathBuf, database: &Path, ctx: &egui::Context) -> Self {
        Self::launch(Batch::Folder(path), database, ctx)
    }

    fn launch(batch: Batch, database: &Path, ctx: &egui::Context) -> Self {
        let (total, discovering) = match &batch {
            Batch::Files(paths, _) => (paths.len(), false),
            Batch::Folder(_) => (0, true),
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let database = database.to_path_buf();
        let ctx = ctx.clone();
        let (sender, receiver) = mpsc::sync_channel(64);
        std::thread::spawn(move || {
            let db = match Database::open(&database) {
                Ok(db) => db,
                Err(error) => {
                    let _ = sender.send(Event::Fatal(error));
                    ctx.request_repaint();
                    return;
                }
            };
            let (paths, force) = match batch {
                Batch::Files(paths, force) => (paths, force),
                Batch::Folder(folder) => {
                    let root = std::fs::canonicalize(&folder).unwrap_or(folder);
                    let (paths, errors, failures) = discover(root, &worker_cancel);
                    if sender
                        .send(Event::Discovered {
                            total: paths.len(),
                            errors,
                            failures,
                        })
                        .is_err()
                    {
                        return;
                    }
                    ctx.request_repaint();
                    (paths, false)
                }
            };
            for path in paths {
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                if sender.send(Event::Started(path.clone())).is_err() {
                    return;
                }
                ctx.request_repaint();
                let result = analyze_summary(&db, &path, &worker_cancel, force);
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                let reused = result.as_ref().is_ok_and(|(_, reused)| *reused);
                let result = result.map(|(summary, _)| summary);
                if sender
                    .send(Event::Completed {
                        path,
                        result,
                        reused,
                    })
                    .is_err()
                {
                    return;
                }
                ctx.request_repaint();
            }
            let _ = sender.send(Event::Finished {
                cancelled: worker_cancel.load(Ordering::Relaxed),
            });
            ctx.request_repaint();
        });
        Self {
            receiver,
            total,
            discovering,
            completed: 0,
            reused: 0,
            failures: 0,
            current: None,
            errors: Vec::new(),
            cancelled,
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn cancelling(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn collection_folder_scan_indexes_nested_file_tags_and_bpm() {
        fn syncsafe(size: usize) -> [u8; 4] {
            [21, 14, 7, 0].map(|shift| ((size >> shift) & 127) as u8)
        }
        let workspace = tempfile::tempdir().unwrap();
        let folder = workspace.path().join("Music");
        let nested = folder.join("Deep").join("House");
        std::fs::create_dir_all(&nested).unwrap();
        let path = nested.join("fixture_01.mp3");
        let source =
            std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo-a.mp3")).unwrap();
        let source_start = if source.starts_with(b"ID3") {
            10 + source[6..10]
                .iter()
                .fold(0, |size, byte| (size << 7) + usize::from(*byte))
        } else {
            0
        };
        let mut frames = Vec::new();
        for (key, text) in [
            (b"TIT2", "Moonlight Signal"),
            (b"TPE1", "Nördliche Welle"),
            (b"TCON", "Deep_Test"),
        ] {
            frames.extend_from_slice(key);
            frames.extend_from_slice(&syncsafe(text.len() + 1));
            frames.extend_from_slice(&[0, 0, 3]);
            frames.extend_from_slice(text.as_bytes());
        }
        let mut file = b"ID3\x04\x00\x00".to_vec();
        file.extend_from_slice(&syncsafe(frames.len()));
        file.extend_from_slice(&frames);
        file.extend_from_slice(&source[source_start..]);
        std::fs::write(&path, file).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&folder, nested.join("Cycle")).unwrap();

        let database = workspace.path().join("test.sqlite3");
        let job = Job::start_folder(folder, &database, &egui::Context::default());
        loop {
            match job
                .receiver
                .recv_timeout(std::time::Duration::from_secs(12))
                .unwrap()
            {
                Event::Discovered {
                    total, failures, ..
                } => {
                    assert_eq!((total, failures), (1, 0));
                }
                Event::Completed { result, .. } => assert!(result.unwrap().bpm().is_some()),
                Event::Finished { cancelled } => {
                    assert!(!cancelled);
                    break;
                }
                Event::Fatal(error) => panic!("{error}"),
                Event::Started(_) => {}
            }
        }

        let db = Database::open(&database).unwrap();
        let tracks = db.search_collection("NÖRD signal DEEP_", 10).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].path, path);
        assert!((tracks[0].summary.bpm().unwrap() - 120.0).abs() < 1.0);
        assert_eq!(db.search_collection("ture_01", 10).unwrap().len(), 1);
        assert!(db.search_collection("missing", 10).unwrap().is_empty());
    }

    use super::*;
    #[test]
    fn cached_results_skip_decode_and_cancellation_never_writes_a_record() {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(&directory.path().join("library.sqlite3")).unwrap();
        let audio = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo-b.flac");
        let first = cached(&db, audio.clone(), &AtomicBool::new(false), false).unwrap();
        assert!(!first.from_cache);
        let second = cached(&db, audio.clone(), &AtomicBool::new(false), false).unwrap();
        assert!(second.from_cache);
        assert_eq!(second.envelope.as_ref(), first.envelope.as_ref());
        assert!(cached(&db, audio, &AtomicBool::new(true), true).is_err());
        assert_eq!(db.count().unwrap(), 1);
    }

    #[test]
    fn background_queue_continues_after_bad_audio_and_reuses_persisted_results() {
        let directory = tempfile::tempdir().unwrap();
        let bad = directory.path().join("broken.mp3");
        std::fs::write(&bad, b"This is not audio").unwrap();
        let good = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo-a.mp3");
        let database = directory.path().join("library.sqlite3");
        let ctx = egui::Context::default();
        let job = Job::start(
            vec![bad.clone(), good.clone(), good.clone()],
            false,
            &database,
            &ctx,
        );
        assert_eq!(job.total, 2);
        let mut completed = Vec::new();
        loop {
            match job
                .receiver
                .recv_timeout(std::time::Duration::from_secs(20))
                .unwrap()
            {
                Event::Completed {
                    path,
                    result,
                    reused,
                } => completed.push((path, result, reused)),
                Event::Finished { cancelled } => {
                    assert!(!cancelled);
                    break;
                }
                Event::Fatal(error) => panic!("{error}"),
                Event::Started(_) | Event::Discovered { .. } => {}
            }
        }
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0].0, bad);
        assert!(completed[0].1.is_err());
        assert_eq!(completed[1].0, good);
        assert!(completed[1].1.is_ok());
        assert!(!completed[1].2);
        let db = Database::open(&database).unwrap();
        assert_eq!(db.count().unwrap(), 1);
        assert!(
            cached(&db, good, &AtomicBool::new(false), false)
                .unwrap()
                .from_cache
        );
    }
}
