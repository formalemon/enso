//! One file playing: its sound heard, its pictures shown as they fall due
//! by the clock the sound keeps (or, with no sound, a clock of their own).
//! Seeking starts both again from the new point; a new size starts the
//! pictures again where they are.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::{
    Error, Media,
    clock::Shared,
    frames::{Frame, Frames, Next},
    sound::Sound,
};

/// While playing with no frame in hand, look again this soon.
const POLL: Duration = Duration::from_millis(8);
/// Sound only: the timeline moves on, a few times a second.
const TICK: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct Playback {
    path: PathBuf,
    media: Media,
    timing: Shared,
    sound: Option<Sound>,
    frames: Option<Frames>,
    /// The size pictures are decoded at.
    size: (u32, u32),
    shown: Option<Frame>,
    next: Option<Frame>,
    /// The pictures have all been decoded.
    pictures_ended: bool,
    /// Just started or moved: the first frame shows whenever it comes.
    fresh: bool,
    playing: bool,
    ended: bool,
    muted: bool,
    /// Why there's no sound, when there should be.
    trouble: Option<Error>,
}

impl Playback {
    /// `path`, which holds `media`, ready to play from its start: paused,
    /// its first frame on its way, decoded `size` pixels.
    pub fn open(path: &Path, media: Media, size: (u32, u32)) -> Self {
        let now = Instant::now();
        let mut playback = Self {
            path: path.to_path_buf(),
            media,
            timing: Shared::new(Duration::ZERO, now),
            sound: None,
            frames: None,
            size,
            shown: None,
            next: None,
            pictures_ended: false,
            fresh: true,
            playing: false,
            ended: false,
            muted: false,
            trouble: None,
        };
        playback.restart(Duration::ZERO, now);
        playback
    }

    pub fn media(&self) -> &Media {
        &self.media
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// It played to its end, and stopped there.
    pub fn has_ended(&self) -> bool {
        self.ended
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// What went wrong with the sound or the pictures, if anything did.
    pub fn trouble(&self) -> Option<&Error> {
        self.trouble.as_ref()
    }

    /// Where it is, now.
    pub fn position(&self, now: Instant) -> Duration {
        let at = self.timing.lock().clock.now(now);
        match self.media.duration {
            Some(duration) => at.min(duration),
            None => at,
        }
    }

    pub fn play(&mut self, now: Instant) {
        if self.playing {
            return;
        }
        if self.ended {
            self.ended = false;
            self.playing = true;
            self.restart(Duration::ZERO, now);
            return;
        }
        self.playing = true;
        match &self.sound {
            // The clock starts when the sound is heard.
            Some(sound) => sound.resume(),
            None => self.timing.lock().clock.run(true, now),
        }
    }

    pub fn pause(&mut self, now: Instant) {
        if !self.playing {
            return;
        }
        self.playing = false;
        self.timing.lock().clock.run(false, now);
        if let Some(sound) = &self.sound {
            sound.pause();
        }
    }

    pub fn toggle(&mut self, now: Instant) {
        if self.playing {
            self.pause(now);
        } else {
            self.play(now);
        }
    }

    /// Go to `to`, playing on if it was playing.
    pub fn seek(&mut self, to: Duration, now: Instant) {
        let to = match self.media.duration {
            Some(duration) => to.min(duration.saturating_sub(Duration::from_millis(100))),
            None => to,
        };
        self.ended = false;
        self.restart(to, now);
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        if let Some(sound) = &self.sound {
            sound.mute(muted);
        }
    }

    /// Decode the pictures `size` pixels from here on.
    pub fn set_size(&mut self, size: (u32, u32), now: Instant) {
        if size == self.size || size.0 < 2 || size.1 < 2 {
            return;
        }
        self.size = size;
        let at = self.position(now);
        self.start_pictures(at);
    }

    /// The size pictures are decoded at.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Bring the pictures up to `now`, and say which to show.
    pub fn frame(&mut self, now: Instant) -> Option<&Frame> {
        let at = self.position(now);
        if let Some(frames) = &self.frames {
            loop {
                if self.next.is_none() {
                    match frames.next() {
                        Next::Frame(frame) => self.next = Some(frame),
                        Next::Waiting => break,
                        Next::Ended => {
                            self.pictures_ended = true;
                            break;
                        }
                    }
                }
                let due = self
                    .next
                    .as_ref()
                    .is_some_and(|next| self.fresh || next.at <= at);
                if !due {
                    break;
                }
                self.fresh = false;
                if let Some(old) = std::mem::replace(&mut self.shown, self.next.take()) {
                    frames.recycle(old);
                }
            }
        }
        self.check_end(now);
        self.shown.as_ref()
    }

    /// How long until something changes on its own: the next frame due,
    /// or the timeline moving on. `None` when nothing will.
    pub fn due(&self, now: Instant) -> Option<Duration> {
        let waiting = self.fresh && self.frames.is_some();
        if !self.playing {
            return waiting.then_some(POLL);
        }
        if self.frames.is_none() {
            return Some(TICK);
        }
        let at = self.position(now);
        match &self.next {
            Some(next) => Some(next.at.saturating_sub(at).max(Duration::from_millis(1))),
            None if self.pictures_ended => Some(TICK),
            None => Some(POLL),
        }
    }

    fn check_end(&mut self, now: Instant) {
        if !self.playing {
            return;
        }
        let (sound_ended, sound_lost) = {
            let timing = self.timing.lock();
            (timing.sound_ended, timing.sound_lost)
        };
        let sound_done = self.sound.is_none() || sound_ended || sound_lost;
        let pictures_done = self.frames.is_none() || (self.pictures_ended && self.next.is_none());
        let past = self.media.duration.is_some_and(|duration| {
            self.timing.lock().clock.now(now) >= duration + Duration::from_secs(1)
        });
        // With the sound lost, the pictures (or the length) say when.
        let done = if self.sound.is_some() && !sound_lost {
            sound_done && pictures_done
        } else {
            pictures_done && (self.frames.is_some() || past)
        };
        if done || past {
            self.playing = false;
            self.ended = true;
            {
                let mut timing = self.timing.lock();
                timing.clock.run(false, now);
                if let Some(duration) = self.media.duration {
                    timing.clock.set(duration, now);
                }
            }
            // Played out: let go of the decoders and the sound's stream.
            // The last frame stays on show.
            self.sound = None;
            self.frames = None;
        }
    }

    /// Start the sound and the pictures again from `at`.
    fn restart(&mut self, at: Duration, now: Instant) {
        // The old ones stop before the new ones start.
        self.sound = None;
        self.timing = Shared::new(at, now);
        if self.media.audio.is_some() {
            let flags = (self.playing, self.muted, self.media.video.is_some());
            match Sound::start(&self.path, at, flags, self.timing.clone()) {
                Ok(sound) => self.sound = Some(sound),
                Err(err) => self.trouble = Some(err),
            }
        }
        if self.sound.is_none() && self.playing {
            self.timing.lock().clock.run(true, now);
        }
        self.start_pictures(at);
    }

    fn start_pictures(&mut self, at: Duration) {
        self.frames = None;
        self.next = None;
        self.pictures_ended = false;
        let Some(video) = &self.media.video else {
            return;
        };
        self.fresh = true;
        match Frames::start(&self.path, at, self.size, video.rate.min(60.0)) {
            Ok(frames) => self.frames = Some(frames),
            Err(err) => self.trouble = Some(err),
        }
    }
}

/// The size to decode a `width × height` video at to fill `area` (output
/// pixels) without spilling: its own shape, never larger than itself.
pub fn fit(width: u32, height: u32, area: (u32, u32)) -> (u32, u32) {
    let scale = (area.0 as f32 / width.max(1) as f32)
        .min(area.1 as f32 / height.max(1) as f32)
        .min(1.0);
    let even = |side: f32| ((side.round() as u32) & !1).max(2);
    (even(width as f32 * scale), even(height as f32 * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_without_spilling_or_growing() {
        assert_eq!(fit(1920, 1080, (960, 1000)), (960, 540));
        assert_eq!(fit(1080, 1920, (1000, 960)), (540, 960));
        assert_eq!(fit(640, 360, (1920, 1080)), (640, 360));
    }
}
