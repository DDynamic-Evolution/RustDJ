//! Ordered background persistence of hotcues and play history.
use crate::database::{self, Database, Fingerprint};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

pub type Hotcues = [Option<Duration>; 8];

pub struct Snapshot {
    pub path: PathBuf,
    pub identity: Fingerprint,
    pub cues: Hotcues,
}

enum Event {
    Hotcues(Snapshot),
    Play {
        title: String,
        path: PathBuf,
        deck: usize,
        timestamp: i64,
    },
}

pub struct Store {
    sender: Option<Sender<Event>>,
    errors: Receiver<String>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Default for Store {
    fn default() -> Self {
        Self::at(database::default_path())
    }
}
impl Store {
    pub fn at(path: PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel::<Event>();
        let (errors, reports) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut db = None;
            for event in receiver {
                let result = (|| {
                    if db.is_none() {
                        db = Some(Database::open(&path)?);
                    }
                    let db = db.as_ref().unwrap();
                    match event {
                        Event::Hotcues(snapshot) => {
                            if Fingerprint::read(&snapshot.path)? != snapshot.identity {
                                return Err(
                                    "Hotcues were not saved: the music file changed.".into()
                                );
                            }
                            db.save_hotcues(&snapshot.path, &snapshot.identity, snapshot.cues)
                        }
                        Event::Play {
                            title,
                            path,
                            deck,
                            timestamp,
                        } => db.log_play(&title, &path, deck, timestamp),
                    }
                })();
                if let Err(error) = result {
                    let _ = errors.send(error);
                }
            }
        });
        Self {
            sender: Some(sender),
            errors: reports,
            worker: Some(worker),
        }
    }
}
impl Store {
    pub fn save(&self, snapshot: Snapshot) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(Event::Hotcues(snapshot));
        }
    }
    pub fn play(&self, title: String, path: PathBuf, deck: usize, timestamp: i64) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(Event::Play {
                title,
                path,
                deck,
                timestamp,
            });
        }
    }

    pub fn error(&self) -> Option<String> {
        self.errors.try_iter().last()
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.sender = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_hotcue_edits_flush_in_order_and_survive_restart_without_an_analysis_record() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("music.mp3");
        std::fs::write(&path, b"identity fixture").unwrap();
        let identity = Fingerprint::read(&path).unwrap();
        let db_path = temp.path().join("library.sqlite3");
        let mut cues = [None; 8];
        {
            let store = Store::at(db_path.clone());
            cues[0] = Some(Duration::from_millis(1234));
            store.save(Snapshot {
                path: path.clone(),
                identity: identity.clone(),
                cues,
            });
            cues[0] = None;
            cues[7] = Some(Duration::from_millis(4567));
            store.save(Snapshot {
                path: path.clone(),
                identity: identity.clone(),
                cues,
            });
        }
        let db = Database::open(&db_path).unwrap();
        assert_eq!(db.count().unwrap(), 0);
        assert_eq!(db.hotcues(&path, &identity).unwrap(), cues);
        let changed = Fingerprint {
            bytes: identity.bytes + 1,
            ..identity
        };
        assert_eq!(db.hotcues(&path, &changed).unwrap(), [None; 8]);
    }
}
