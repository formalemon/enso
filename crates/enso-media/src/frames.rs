//! A video's pictures, decoded by ffmpeg from a point on, at the size the
//! window shows them, a few ahead of when they're wanted. ffmpeg keeps an
//! even pace (frames repeated or dropped to it), so each frame's time is
//! where it started plus its count over the rate.

use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
    },
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
        let mut args = Vec::<OsString>::from([
            // A few threads decode, enough for 4K, holding far fewer frames
            // than one each for every core; and one scales: spread over every
            // core, keeping them in step cost more than the scaling.
            "-threads".into(),
            "4".into(),
            "-filter_threads".into(),
            "1".into(),
        ]);
        let scale = match video_engine() {
            Some(engine) => {
                let mut device = OsString::from("vaapi=enso:");
                device.push(engine);
                args.extend([
                    "-init_hw_device".into(),
                    device,
                    "-filter_hw_device".into(),
                    "enso".into(),
                    "-hwaccel".into(),
                    "vaapi".into(),
                    "-hwaccel_device".into(),
                    "enso".into(),
                    "-hwaccel_output_format".into(),
                    "vaapi".into(),
                ]);
                // What the engine can't decode (an older codec, a deeper
                // colour) the processor does, and the engine still scales it.
                format!(
                    "format=nv12|p010|vaapi,hwupload,\
                     scale_vaapi=w={width}:h={height}:format=rgba:mode=hq,\
                     hwdownload,format=rgba"
                )
            }
            None => format!("scale={width}:{height}:flags=bicubic"),
        };
        args.extend([
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
            scale.into(),
            "-pix_fmt".into(),
            "rgba".into(),
            "-fps_mode".into(),
            "cfr".into(),
            "-r".into(),
            format!("{rate:.3}").into(),
            "-f".into(),
            "rawvideo".into(),
            "-".into(),
        ]);
        let mut ffmpeg = ffmpeg::spawn("ffmpeg", args)?;
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

/// The graphics chip's video engine, when ffmpeg can reach one (VA-API, on
/// Intel's and AMD's): it decodes, scales, and turns the pictures to RGBA,
/// so a 1080p video at 60 frames takes a tenth of a core rather than most
/// of one. Looked for once; without one the processor does it all.
fn video_engine() -> Option<&'static Path> {
    static ENGINE: OnceLock<Option<PathBuf>> = OnceLock::new();
    ENGINE.get_or_init(find_engine).as_deref()
}

fn find_engine() -> Option<PathBuf> {
    let mut nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    nodes.into_iter().find(|node| {
        let Some(name) = node.file_name() else {
            return false;
        };
        let driver = Path::new("/sys/class/drm").join(name).join("device/driver");
        let driver = std::fs::read_link(driver).ok();
        // Not NVIDIA's: VA-API doesn't reach its engine, and asking would
        // wake a card that may be asleep.
        let driver = driver.as_deref().and_then(Path::file_name);
        driver.is_some_and(|driver| {
            matches!(driver.to_str(), Some("i915" | "xe" | "amdgpu" | "radeon"))
        }) && opens(node)
    })
}

/// Whether ffmpeg can open the video engine at `node` (its driver is there).
fn opens(node: &Path) -> bool {
    let mut device = OsString::from("vaapi=enso:");
    device.push(node);
    let args: [OsString; 11] = [
        "-init_hw_device".into(),
        device,
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        "nullsrc=s=16x16:d=0.04".into(),
        "-frames:v".into(),
        "1".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ];
    ffmpeg::output("ffmpeg", args).is_ok()
}
