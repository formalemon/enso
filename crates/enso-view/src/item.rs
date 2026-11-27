//! What's read for the viewer, off its thread: a picture, decoded, with
//! what its camera says of it; a video, as ffprobe reads it, with a frame
//! of it to show while it's still. And the thumbnails along the foot.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
};

use enso_fs::picture::Details;
use enso_media::Media;
use enso_ui::Image;

use crate::{album, photo::Photo};

/// One thing in the album, read.
#[derive(Debug)]
pub enum Item {
    Picture {
        photo: Arc<Photo>,
        details: Details,
    },
    Video {
        media: Media,
        /// A frame from a little way in.
        poster: Option<Arc<Photo>>,
    },
}

impl Item {
    /// What to draw of it while still: the picture, or the video's frame.
    pub fn still(&self) -> Option<&Arc<Photo>> {
        match self {
            Self::Picture { photo, .. } => Some(photo),
            Self::Video { poster, .. } => poster.as_ref(),
        }
    }

    pub fn is_video(&self) -> bool {
        matches!(self, Self::Video { .. })
    }

    /// Its size in its own pixels, as shown.
    pub fn size(&self) -> Option<(u32, u32)> {
        match self {
            Self::Picture { photo, .. } => Some(photo.size()),
            Self::Video { media, .. } => media
                .video
                .as_ref()
                .map(|video| (video.width, video.height)),
        }
    }
}

/// The longest side of a video's still frame.
const POSTER: u32 = 1920;

/// Read `path`.
pub fn read(path: &Path) -> Result<Item, String> {
    if album::is_video(path) {
        if !enso_media::available() {
            return Err("videos need ffmpeg, which isn't installed".to_owned());
        }
        let media = enso_media::probe(path).map_err(|err| err.to_string())?;
        let poster = enso_media::still::poster(path, &media, POSTER)
            .ok()
            .map(|still| Arc::new(Photo::new(still.width, still.height, still.rgba)));
        return Ok(Item::Video { media, poster });
    }
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    let pixels = enso_fs::picture::decode(&bytes)
        .map_err(|err| err.to_string())?
        .premultiplied();
    Ok(Item::Picture {
        photo: Arc::new(Photo::new(pixels.width, pixels.height, pixels.rgba)),
        details: enso_fs::picture::details(&bytes),
    })
}

/// Makes thumbnails for the strip along the foot, nearest first, as the
/// file manager's grid has them (and from the same cache).
#[derive(Debug)]
pub struct Thumbnailer {
    ask: mpsc::Sender<Vec<PathBuf>>,
}

impl Thumbnailer {
    /// `done` is told each one made, and says whether anyone's listening.
    pub fn start(done: impl Fn(PathBuf, Option<Image>) -> bool + Send + 'static) -> Option<Self> {
        let (ask, asked) = mpsc::channel::<Vec<PathBuf>>();
        thread::Builder::new()
            .name("enso-view-thumbs".to_owned())
            .spawn(move || {
                while let Ok(mut wanted) = asked.recv() {
                    // Only the latest wish counts.
                    while let Ok(later) = asked.try_recv() {
                        wanted = later;
                    }
                    for path in wanted {
                        let thumb =
                            enso_fs::thumbs::thumbnail(&path, enso_fs::thumbs::Size::Normal)
                                .and_then(|thumb| {
                                    Image::from_raw(
                                        thumb.width,
                                        thumb.height,
                                        thumb.width as usize * 4,
                                        true,
                                        &thumb.rgba,
                                    )
                                });
                        if !done(path, thumb) {
                            return;
                        }
                    }
                }
            })
            .ok()?;
        Some(Self { ask })
    }

    /// Make these, nearest first.
    pub fn want(&self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            let _ = self.ask.send(paths);
        }
    }
}
