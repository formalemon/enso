//! A video's pictures, decoded by ffmpeg from a point on, at the size the
//! window shows them, a few ahead of when they're wanted. ffmpeg keeps an
//! even pace (frames repeated or dropped to it), so each frame's time is
//! where it started plus its count over the rate.

use std::{
    io::Read,
    path::Path,
    sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
    thread,
    time::Duration,
};

use crate::{Error, ffmpeg};

/// How many frames are decoded ahead of the one shown.
const AHEAD: usize = 3;

/// One picture of a video: R, G, B, A (opaque), `width × height`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// When in the file it's shown.
    pub at: Duration,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What the decoding has for the one showing them.
#[derive(Debug)]
pub(crate) enum Next {
    Frame(Frame),
    /// None decoded yet.
    Waiting,
    /// No more: the pictures have ended.
    Ended,
}

#[derive(Debug)]
pub(crate) struct Frames {
    frames: Receiver<Frame>,
    /// Frames shown and done with, for their memory to be used again.
    spare: Sender<Vec<u8>>,
    _ffmpeg: ffmpeg::Child,
}

impl Frames {
    /// Decode `path`'s pictures from `from` on, `size` pixels, `rate` a
    /// second.
    pub fn start(path: &Path, from: Duration, size: (u32, u32), rate: f32) -> Result<Self, Error> {
        let (width, height) = (size.0.max(2), size.1.max(2));
        let mut ffmpeg = ffmpeg::spawn(
            "ffmpeg",
            Vec::<std::ffi::OsString>::from([
                "-ss".into(),
                ffmpeg::seconds(from).into(),
                "-i".into(),
                ffmpeg::input(path),
                // Pictures that move: not a cover riding along.
                "-map".into(),
                "0:V:0".into(),
                "-an".into(),
                "-sn".into(),
                "-dn".into(),
                "-vf".into(),
                format!("scale={width}:{height}:flags=bicubic").into(),
                "-pix_fmt".into(),
                "rgba".into(),
                "-fps_mode".into(),
                "cfr".into(),
                "-r".into(),
                format!("{rate:.3}").into(),
                "-f".into(),
                "rawvideo".into(),
                "-".into(),
            ]),
        )?;
        let mut out = ffmpeg
            .0
            .stdout
            .take()
            .ok_or_else(|| Error::new("ffmpeg gave nothing to read"))?;
        let (send, frames): (SyncSender<Frame>, _) = mpsc::sync_channel(AHEAD);
        let (spare, spares) = mpsc::channel::<Vec<u8>>();
        let bytes = width as usize * height as usize * 4;
        let step = 1.0 / f64::from(rate.max(1.0));
        thread::Builder::new()
            .name("enso-media-frames".to_owned())
            .spawn(move || {
                for count in 0u64.. {
                    let mut rgba = spares.try_recv().unwrap_or_default();
                    rgba.resize(bytes, 0);
                    if out.read_exact(&mut rgba).is_err() {
                        return;
                    }
                    let frame = Frame {
                        at: from + Duration::from_secs_f64(count as f64 * step),
                        width,
                        height,
                        rgba,
                    };
                    if send.send(frame).is_err() {
                        return;
                    }
                }
            })
            .map_err(|err| Error::new(format!("cannot decode: {err}")))?;
        Ok(Self {
            frames,
            spare,
            _ffmpeg: ffmpeg,
        })
    }

    pub fn next(&self) -> Next {
        match self.frames.try_recv() {
            Ok(frame) => Next::Frame(frame),
            Err(TryRecvError::Empty) => Next::Waiting,
            Err(TryRecvError::Disconnected) => Next::Ended,
        }
    }

    /// A frame done with: its memory goes to a later one.
    pub fn recycle(&self, frame: Frame) {
        let _ = self.spare.send(frame.rgba);
    }
}
