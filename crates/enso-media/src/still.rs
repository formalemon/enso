//! One picture out of a file: a video's poster frame, a little way in (a
//! first frame is often black), and the cover a song carries.

use std::{ffi::OsString, path::Path, time::Duration};

use crate::{Error, Media, ffmpeg, playback::fit};

/// A picture: R, G, B, A (opaque), `width × height`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A frame of `path`'s video, at most `side` pixels on its longer side.
pub fn poster(path: &Path, media: &Media, side: u32) -> Result<Still, Error> {
    let video = media
        .video
        .as_ref()
        .ok_or_else(|| Error::new("no pictures in it"))?;
    let (width, height) = fit(video.width, video.height, (side, side));
    // A second in, or a tenth of the way for something shorter.
    let at = media.duration.map_or(Duration::ZERO, |duration| {
        (duration / 10).min(Duration::from_secs(1))
    });
    let rgba = ffmpeg::output(
        "ffmpeg",
        Vec::<OsString>::from([
            "-ss".into(),
            ffmpeg::seconds(at).into(),
            "-i".into(),
            ffmpeg::input(path),
            "-map".into(),
            "0:V:0".into(),
            "-frames:v".into(),
            "1".into(),
            "-vf".into(),
            format!("scale={width}:{height}:flags=bicubic").into(),
            "-pix_fmt".into(),
            "rgba".into(),
            "-f".into(),
            "rawvideo".into(),
            "-".into(),
        ]),
    )?;
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(Error::new("ffmpeg made no picture of it"));
    }
    Ok(Still {
        width,
        height,
        rgba,
    })
}

/// The cover `path` carries, as it carries it: a JPEG's or a PNG's bytes.
pub fn cover(path: &Path) -> Result<Vec<u8>, Error> {
    let bytes = ffmpeg::output(
        "ffmpeg",
        Vec::<OsString>::from([
            "-i".into(),
            ffmpeg::input(path),
            "-map".into(),
            "0:v:0".into(),
            "-frames:v".into(),
            "1".into(),
            "-c:v".into(),
            "copy".into(),
            "-f".into(),
            "image2pipe".into(),
            "-".into(),
        ]),
    )?;
    if bytes.is_empty() {
        return Err(Error::new("no cover in it"));
    }
    Ok(bytes)
}
