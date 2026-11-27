//! Sound and moving pictures for the enso suite: what a media file holds,
//! its frames and its sound, played in time.
//!
//! Decoding is ffmpeg's. A video's pictures in the codecs of the day
//! (H.264, HEVC, VP9, AV1) have no decoder in pure Rust, and ffmpeg is on
//! any desktop that plays video already: Firefox depends on it. It is run as
//! a program, the way the file manager runs `git` — one for the pictures and
//! one for the sound, each from a point in the file on, the pictures at the
//! size the window wants. The sound goes to PipeWire through its PulseAudio
//! socket, in pure Rust, and keeps the clock the pictures follow.
//!
//! - [`probe`] — what a file holds: how long, its picture, its sound, the
//!   names it carries (title, artist, album).
//! - [`Playback`] — one file playing: its frames as they fall due, its
//!   sound, pausing, seeking, the end.
//! - [`still`] — one picture out of a file: a video's poster frame, the
//!   cover a song carries.
//! - [`announce`] (the `mpris` feature) — telling the desktop what's
//!   playing, and taking its buttons.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod clock;
mod ffmpeg;
mod frames;
mod playback;
pub mod probe;
mod sound;
pub mod still;

#[cfg(feature = "mpris")]
pub mod announce;

pub use ffmpeg::available;
pub use frames::Frame;
pub use playback::Playback;
pub use probe::{Media, Tags, probe};

use std::fmt;

/// Why something couldn't be played or read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// `3:07`, or `1:02:45` for something longer than an hour.
pub fn clock(duration: std::time::Duration) -> String {
    let total = duration.as_secs();
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}
