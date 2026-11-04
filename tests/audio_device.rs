//! Opt-in integration test: uses the real default audio device at zero output gain.
use rustdj::{
    audio::{Audio, Deck},
    track::Track,
};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

fn await_load(deck: &mut Deck, audio: &Audio) -> Option<Track> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while deck.loading() && Instant::now() < deadline {
        if let Some(track) = deck.poll(audio) {
            return Some(track);
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!deck.loading(), "Loading timed out");
    None
}

#[test]
#[ignore = "requires a Windows audio output device; runs silently"]
fn two_decks_play_pause_seek_cue_and_recover_from_failed_load() {
    let audio = Audio::default();
    assert!(audio.stream.is_some(), "{:?}", audio.error);
    eprintln!(
        "Audio config: {:?}",
        audio.stream.as_ref().unwrap().config()
    );
    let ctx = eframe::egui::Context::default();
    let mut decks = [Deck::default(), Deck::default()];
    for (i, deck) in decks.iter_mut().enumerate() {
        let file = if i == 0 { "demo-a.wav" } else { "demo-b.wav" };
        deck.load(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(file),
            ctx.clone(),
        );
        assert!(await_load(deck, &audio).is_some());
        assert!(!deck.playing());
        deck.apply_mix(0.0);
        deck.toggle(&audio);
        deck.apply_mix(0.0);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while decks
        .iter()
        .any(|d| d.position() < Duration::from_millis(100))
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(20));
    }
    for deck in &mut decks {
        assert!(deck.playing());
        assert!(
            deck.position() >= Duration::from_millis(100),
            "Audio clock did not advance: {:?}",
            deck.position()
        );
        deck.toggle(&audio);
        assert!(!deck.playing());
        let paused = deck.position();
        thread::sleep(Duration::from_millis(80));
        assert!(deck.position().abs_diff(paused) < Duration::from_millis(40));
        deck.seek(Duration::from_secs(3), &audio);
        assert!(deck.position().abs_diff(Duration::from_secs(3)) < Duration::from_millis(100));
        deck.cue = Duration::from_secs(2);
        deck.go_to_cue(&audio);
        assert!(deck.position().abs_diff(deck.cue) < Duration::from_millis(100));
        deck.stop(&audio);
        assert!(deck.position() < Duration::from_millis(100));
        let old_path = deck.track.as_ref().unwrap().path.clone();
        deck.load(old_path.with_extension("missing"), ctx.clone());
        assert!(await_load(deck, &audio).is_none());
        assert!(deck.error.is_some());
        assert_eq!(deck.track.as_ref().unwrap().path, old_path);
        // Reaching the end must allow a clean restart.
        deck.seek(Duration::from_millis(7900), &audio);
        deck.toggle(&audio);
        deck.apply_mix(0.0);
        thread::sleep(Duration::from_millis(220));
        assert!(!deck.playing());
        deck.toggle(&audio);
        deck.apply_mix(0.0);
        thread::sleep(Duration::from_millis(80));
        assert!(deck.playing());
        assert!(deck.position() < Duration::from_secs(1));
        deck.stop(&audio);
    }
}
