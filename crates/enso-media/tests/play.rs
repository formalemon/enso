//! Playing real files, made on the spot by ffmpeg (skipped without it).

#![allow(clippy::unwrap_used)]

use std::{
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use enso_media::{Playback, probe, still};

fn made(name: &str, args: &[&str]) -> Option<PathBuf> {
    if !enso_media::available() {
        eprintln!("no ffmpeg here: skipped");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("enso-media-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(name);
    let made = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y"])
        .args(args)
        .arg(&path)
        .status()
        .ok()?;
    made.success().then_some(path)
}

fn video(name: &str) -> Option<PathBuf> {
    made(
        name,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=25",
            "-t",
            "1.2",
            "-pix_fmt",
            "yuv420p",
        ],
    )
}

fn play_to_the_end(playback: &mut Playback, within: Duration) -> Vec<Duration> {
    let start = Instant::now();
    let mut shown = Vec::new();
    playback.play(Instant::now());
    while !playback.has_ended() && start.elapsed() < within {
        let now = Instant::now();
        if let Some(frame) = playback.frame(now)
            && shown.last() != Some(&frame.at)
        {
            shown.push(frame.at);
        }
        thread::sleep(
            playback
                .due(now)
                .unwrap_or(Duration::from_millis(5))
                .min(Duration::from_millis(20)),
        );
    }
    shown
}

#[test]
fn a_video_plays_in_time_and_stops_at_its_end() {
    let Some(path) = video("moving.mp4") else {
        return;
    };
    let media = probe(&path).unwrap();
    let video = media.video.clone().unwrap();
    assert_eq!((video.width, video.height), (320, 240));
    assert!(media.audio.is_none());
    let mut playback = Playback::open(&path, media, (160, 120));
    // Paused, the first frame comes anyway.
    let start = Instant::now();
    while playback.frame(Instant::now()).is_none() && start.elapsed() < Duration::from_secs(5) {
        thread::sleep(Duration::from_millis(5));
    }
    let first = playback.frame(Instant::now()).unwrap();
    assert_eq!((first.width, first.height), (160, 120));
    assert_eq!(first.rgba.len(), 160 * 120 * 4);
    let begun = Instant::now();
    let shown = play_to_the_end(&mut playback, Duration::from_secs(10));
    let took = begun.elapsed();
    assert!(playback.has_ended(), "it ended");
    // In time: about as long as it lasts, its frames in order.
    assert!(
        took > Duration::from_millis(900) && took < Duration::from_secs(4),
        "{took:?}"
    );
    assert!(shown.len() > 15, "{} frames shown", shown.len());
    assert!(shown.windows(2).all(|pair| pair[0] < pair[1]));
    // Played again from the end: from the start.
    playback.play(Instant::now());
    assert!(playback.position(Instant::now()) < Duration::from_millis(200));
}

#[test]
fn seeking_shows_the_frame_there_even_paused() {
    let Some(path) = video("seek.mp4") else {
        return;
    };
    let media = probe(&path).unwrap();
    let mut playback = Playback::open(&path, media, (160, 120));
    playback.seek(Duration::from_millis(800), Instant::now());
    let start = Instant::now();
    let at = loop {
        if let Some(frame) = playback.frame(Instant::now()) {
            break frame.at;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(at, Duration::from_millis(800));
    assert_eq!(
        playback.position(Instant::now()),
        Duration::from_millis(800)
    );
}

#[test]
fn a_poster_comes_out_its_own_shape() {
    let Some(path) = video("poster.mp4") else {
        return;
    };
    let media = probe(&path).unwrap();
    let poster = still::poster(&path, &media, 100).unwrap();
    assert_eq!((poster.width, poster.height), (100, 74));
    assert!(still::cover(&path).is_ok() || still::cover(Path::new("/nonexistent")).is_err());
}

#[test]
fn sound_plays_to_its_end() {
    let Some(path) = made(
        "tone.ogg",
        &["-f", "lavfi", "-i", "sine=frequency=440", "-t", "0.6"],
    ) else {
        return;
    };
    let media = probe(&path).unwrap();
    assert!(media.is_sound());
    let mut playback = Playback::open(&path, media, (0, 0));
    // Not out loud.
    playback.set_muted(true);
    let begun = Instant::now();
    play_to_the_end(&mut playback, Duration::from_secs(10));
    assert!(playback.has_ended(), "{:?}", playback.trouble());
    assert!(
        begun.elapsed() >= Duration::from_millis(500),
        "{:?}",
        begun.elapsed()
    );
}
