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
        && let Some(track) = db.load_track(&path, &before)?
        && Fingerprint::read(&path)? == before
    {
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

/// A missing/unwritable database must not prevent audio playback.
pub fn load(path: PathBuf, cancelled: &AtomicBool) -> Result<Track, String> {
    let before = Fingerprint::read(&path)?;
    let (db, mut warning) = match Database::open(&database::default_path()) {
        Ok(db) => (Some(db), None),
        Err(error) => (None, Some(error)),
    };
    if let Some(db) = &db {
        match db.load_track(&path, &before) {
            Ok(Some(track))
                if !cancelled.load(Ordering::Relaxed) && Fingerprint::read(&path)? == before =>
            {
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

pub enum Event {
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
    pub completed: usize,
    pub reused: usize,
    pub failures: usize,
    pub current: Option<PathBuf>,
    pub errors: Vec<String>,
    cancelled: Arc<AtomicBool>,
}
impl Job {
    pub fn start(paths: Vec<PathBuf>, force: bool, database: &Path, ctx: &egui::Context) -> Self {
        let mut seen = HashSet::new();
        let paths: Vec<_> = paths
            .into_iter()
            .filter(|p| seen.insert(p.clone()))
            .collect();
        let total = paths.len();
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
            for path in paths {
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                if sender.send(Event::Started(path.clone())).is_err() {
                    return;
                }
                ctx.request_repaint();
                let result = cached(&db, path.clone(), &worker_cancel, force);
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                let reused = result.as_ref().is_ok_and(|t| t.from_cache);
                let result = result.map(|t| Summary::from_track(&t));
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
                Event::Started(_) => {}
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
