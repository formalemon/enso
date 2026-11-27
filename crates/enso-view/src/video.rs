//! The video on show: its playback, the controls over it, and what the
//! desktop is told of it (the now-playing card lists it among the players,
//! and the media keys reach it).

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use enso_media::{
    Media, Playback,
    announce::{Announcer, Asked, Playing, Status},
};

/// How a video is fitted: to fill `area` (output pixels) without spilling,
/// enlarged if it's smaller, as players do. Even sides, as decoders like.
pub fn fit(width: u32, height: u32, area: (f32, f32)) -> (u32, u32) {
    let scale = (area.0 / width.max(1) as f32).min(area.1 / height.max(1) as f32);
    let even = |side: f32| ((side.floor() as u32) & !1).max(2);
    (even(width as f32 * scale), even(height as f32 * scale))
}

#[derive(Debug)]
pub struct Video {
    pub path: PathBuf,
    pub playback: Playback,
    /// The timeline's thumb, held, `0..=1` of the way.
    pub held: Option<f32>,
    /// It has been played since it came on show: the big button is gone.
    pub started: bool,
    announcer: Option<Announcer>,
    /// What the desktop was last told: whether it played, and where.
    told: Option<(Status, Duration)>,
}

impl Video {
    pub fn open(path: &Path, media: Media, size: (u32, u32)) -> Self {
        Self {
            path: path.to_path_buf(),
            playback: Playback::open(path, media, size),
            held: None,
            started: false,
            announcer: None,
            told: None,
        }
    }

    pub fn media(&self) -> &Media {
        self.playback.media()
    }

    pub fn toggle(&mut self, now: Instant) {
        self.started = true;
        self.playback.toggle(now);
    }

    pub fn play(&mut self, now: Instant) {
        self.started = true;
        self.playback.play(now);
    }

    /// Move `by` on (or back), from where it is.
    pub fn seek_by(&mut self, by: Duration, forward: bool, now: Instant) {
        let at = self.playback.position(now);
        let to = if forward {
            at + by
        } else {
            at.saturating_sub(by)
        };
        self.seek_to(to, now);
    }

    pub fn seek_to(&mut self, to: Duration, now: Instant) {
        self.playback.seek(to, now);
        if let Some(announcer) = &self.announcer {
            announcer.seeked(to);
        }
        self.told = None;
    }

    /// Where, as a fraction of the way through, with the thumb held where it
    /// is held.
    pub fn fraction(&self, now: Instant) -> f32 {
        if let Some(held) = self.held {
            return held;
        }
        match self.media().duration {
            Some(length) if !length.is_zero() => {
                self.playback.position(now).as_secs_f32() / length.as_secs_f32()
            }
            _ => 0.0,
        }
    }

    /// Tell the desktop how it stands, if that changed: what it is, whether
    /// it plays. Becomes a player on the bus the first time it plays;
    /// `asked` is given what the desktop asks of it.
    pub fn announce(
        &mut self,
        (can_previous, can_next): (bool, bool),
        now: Instant,
        asked: impl Fn(Asked) + Send + Sync + 'static,
    ) {
        if !self.started {
            return;
        }
        if self.announcer.is_none() {
            match Announcer::start("enso_view", "Viewer", "enso-view", asked) {
                Ok(announcer) => self.announcer = Some(announcer),
                Err(err) => {
                    eprintln!("enso-view: {err}");
                    // Not again for this one.
                    self.started = false;
                    return;
                }
            }
        }
        let status = if self.playback.is_playing() {
            Status::Playing
        } else {
            Status::Paused
        };
        let position = self.playback.position(now);
        if self.told.is_some_and(|(told, _)| told == status) {
            return;
        }
        self.told = Some((status, position));
        let media = self.media();
        let title = media.tags.title.clone().unwrap_or_else(|| {
            self.path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        let playing = Playing {
            title,
            artist: media.tags.artist.clone(),
            album: None,
            art: None,
            length: media.duration,
            position,
            status,
            can_next,
            can_previous,
            track: 1,
        };
        if let Some(announcer) = &self.announcer {
            announcer.tell(playing);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_fills_its_stage_its_own_shape() {
        assert_eq!(fit(1920, 1080, (960.0, 1000.0)), (960, 540));
        // Smaller than the stage: enlarged to it.
        assert_eq!(fit(640, 360, (1280.0, 1000.0)), (1280, 720));
        assert_eq!(fit(1080, 1920, (1000.0, 960.0)), (540, 960));
    }
}
