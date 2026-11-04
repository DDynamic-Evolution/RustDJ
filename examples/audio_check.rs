//! Silent Windows integration harness. Writes a local report instead of opening a console.
#![cfg_attr(windows, windows_subsystem = "windows")]
use rustdj::{
    audio::{Audio, Deck},
    output::{self, Routing, Target},
};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

fn wait_for(mut check: impl FnMut() -> bool, description: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if check() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(format!("Timed out: {description}"))
}

fn run() -> Result<String, String> {
    let initial = Audio::default();
    if let Some(error) = &initial.error {
        return Err(error.clone());
    }
    let master_name = initial.master_name.clone();
    drop(initial);
    let devices = output::devices()?;
    let phones = devices
        .iter()
        .find(|d| d.name != master_name && d.channels >= 2)
        .ok_or("No second output is available for this test.")?;
    let routing = Routing {
        master: Target {
            device: Some(master_name.clone()),
            pair: 0,
        },
        phones: Some(Target {
            device: Some(phones.name.clone()),
            pair: 0,
        }),
    };
    let audio = Audio::open(routing)?;
    if audio.phones().is_none() {
        return Err("Headphone output is missing.".into());
    }
    let context = eframe::egui::Context::default();
    let mut deck = Deck::default();
    deck.load(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/demo-a.mp3"),
        context,
    );
    wait_for(
        || {
            deck.poll(&audio);
            !deck.loading()
        },
        "load MP3",
    )?;
    if deck.track.is_none() {
        return Err(format!("File loading: {:?}", deck.error));
    }
    deck.apply_buses(0.0, 0.0); // Both hardware outputs stay silent throughout.
    deck.toggle(&audio);
    wait_for(
        || {
            deck.position() > Duration::from_millis(150)
                && deck
                    .phones_position()
                    .is_some_and(|p| p > Duration::from_millis(150))
        },
        "start both audio clocks",
    )?;
    deck.toggle(&audio);
    if deck.playing() {
        return Err("Pause failed.".into());
    }
    deck.speed = 1.25;
    deck.apply_buses(0.0, 0.0);
    deck.seek(Duration::from_secs(3), &audio);
    if deck.position().abs_diff(Duration::from_secs(3)) > Duration::from_millis(100)
        || deck
            .phones_position()
            .unwrap()
            .abs_diff(Duration::from_secs(3))
            > Duration::from_millis(100)
    {
        return Err(format!(
            "Seek at 125%: Master {:?}, Phones {:?}",
            deck.position(),
            deck.phones_position()
        ));
    }
    deck.cue = Duration::from_secs(1);
    deck.cue_button(true, &audio);
    deck.apply_buses(0.0, 0.0);
    wait_for(
        || deck.playing() && deck.position() > Duration::from_millis(1100),
        "hold Cue preview",
    )?;
    deck.cue_button(false, &audio);
    if deck.playing()
        || deck.position().abs_diff(deck.cue) > Duration::from_millis(100)
        || deck.phones_position().unwrap().abs_diff(deck.cue) > Duration::from_millis(100)
    {
        return Err("Releasing Cue did not reset both outputs.".into());
    }
    deck.hotcue(0, &audio);
    if deck.hotcues[0].is_none() {
        return Err("Setting a hotcue failed.".into());
    }
    deck.seek(Duration::from_millis(2500), &audio);
    deck.hotcue(1, &audio);
    deck.hotcue(0, &audio);
    deck.apply_buses(0.0, 0.0);
    wait_for(
        || deck.playing() && deck.position() > Duration::from_millis(1100),
        "jump to hotcue and play",
    )?;
    deck.beat_loop(1.0, &audio);
    wait_for(
        || {
            deck.poll(&audio);
            deck.loop_active && !deck.loop_pending()
        },
        "prepare beat loop",
    )?;
    let range = deck.loop_range.unwrap();
    let mut wraps = 0;
    let mut previous = deck.position();
    for _ in 0..130 {
        thread::sleep(Duration::from_millis(10));
        deck.poll(&audio);
        deck.apply_buses(0.0, 0.0);
        let position = deck.position();
        if position < previous {
            wraps += 1;
        }
        if !range.contains(position) || !range.contains(deck.phones_position().unwrap()) {
            return Err(format!(
                "Loop out of bounds: Master {position:?}, Phones {:?}",
                deck.phones_position()
            ));
        }
        previous = position;
    }
    if wraps < 2 {
        return Err(format!("Loop did not repeat: {wraps} wraps."));
    }
    deck.resize_loop(2.0, &audio);
    wait_for(
        || {
            deck.poll(&audio);
            deck.loop_active && !deck.loop_pending()
        },
        "double loop",
    )?;
    if (deck.loop_range.unwrap().end - deck.loop_range.unwrap().start)
        .abs_diff(Duration::from_secs(1))
        > Duration::from_millis(10)
    {
        return Err("The doubled loop has the wrong length.".into());
    }
    let end = deck.loop_range.unwrap().end;
    deck.exit_loop();
    wait_for(
        || deck.playing() && deck.position() > end + Duration::from_millis(50),
        "exit loop and continue playback",
    )?;
    deck.toggle(&audio);
    deck.seek(Duration::from_secs(3), &audio);
    deck.set_loop_in();
    deck.seek(Duration::from_millis(3500), &audio);
    deck.set_loop_out(&audio);
    wait_for(
        || {
            deck.poll(&audio);
            deck.loop_active && !deck.loop_pending()
        },
        "prepare manual IN/OUT loop",
    )?;
    deck.toggle(&audio);
    deck.apply_buses(0.0, 0.0);
    thread::sleep(Duration::from_millis(750));
    if !deck.loop_range.unwrap().contains(deck.position()) {
        return Err("The manual loop did not repeat.".into());
    }
    deck.reloop(&audio);
    deck.reloop(&audio);
    wait_for(
        || {
            deck.poll(&audio);
            deck.loop_active && !deck.loop_pending()
        },
        "Reloop",
    )?;
    deck.clear_hotcue(0);
    if deck.hotcues[0].is_some() || deck.hotcues[1].is_none() {
        return Err("Clearing a hotcue affected other pads.".into());
    }
    deck.stop(&audio);
    if deck.loop_active || deck.loop_pending() || deck.position() > Duration::from_millis(50) {
        return Err("Stop did not exit the loop.".into());
    }
    if Audio::open(Routing {
        master: Target::default(),
        phones: Some(Target::default()),
    })
    .is_ok()
    {
        return Err("Overlapping outputs were not blocked.".into());
    }
    Ok(format!(
        "PASS\nMaster: {master_name}\nPhones: {}\nBoth audio clocks, pause, seek at 125%, Cue hold/release, hotcue set/jump/clear, repeated beat loops on both outputs, double loop, exit loop, manual IN/OUT loop, reloop, stop and channel overlap protection.\nAll output levels were zero.\n",
        phones.name
    ))
}

fn main() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("work/audio-check.txt");
    let report = match run() {
        Ok(report) => report,
        Err(error) => format!("FAIL\n{error}\n"),
    };
    let _ = std::fs::write(path, report);
}
