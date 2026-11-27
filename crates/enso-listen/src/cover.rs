//! An album's cover: the one a file carries, or a picture beside it in its
//! folder (`cover.jpg`, `folder.png`, …). Kept large, for the window, with
//! its average colour, which warms the window a touch — and written to the
//! cache, for the now-playing card to show it too.

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
};

use enso_core::Rgba;
use enso_ui::Image;

/// Longest side a cover is kept at.
const SIDE: u32 = 720;
/// The names a folder's cover goes by, best first.
const NAMES: [&str; 5] = ["cover", "folder", "front", "album", "albumart"];

#[derive(Clone, Debug)]
pub struct Cover {
    pub image: Image,
    pub tint: Rgba,
    /// Where it is as a file: the folder's own, or a copy in the cache.
    pub file: Option<PathBuf>,
}

/// The cover for the sound file at `path`, if there is one. Copies of
/// carried covers go in `cache`.
pub fn find(path: &Path, cache: &Path) -> Option<Cover> {
    if let Ok(bytes) = enso_media::still::cover(path)
        && let Ok(pixels) = enso_fs::picture::decode(&bytes)
    {
        let file = keep(&bytes, path, cache);
        return made(pixels, file);
    }
    let folder = path.parent()?;
    let mut beside: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|candidate| {
            let stem = candidate
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            NAMES.contains(&stem.as_str()) && is_picture(candidate)
        })
        .collect();
    beside.sort_by_key(|candidate| {
        let stem = candidate
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        NAMES.iter().position(|name| *name == stem)
    });
    let file = beside.into_iter().next()?;
    let pixels = enso_fs::picture::open(&file, SIDE).ok()?;
    made(pixels, Some(file))
}

fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["jpg", "jpeg", "png", "webp"]
                .iter()
                .any(|kind| kind.eq_ignore_ascii_case(ext))
        })
}

fn made(pixels: enso_fs::picture::Pixels, file: Option<PathBuf>) -> Option<Cover> {
    let pixels = enso_fs::picture::fit(&pixels, SIDE);
    let tint = average(&pixels.rgba);
    let image = Image::from_raw_up_to(pixels.width, pixels.height, true, &pixels.rgba, SIDE)?;
    Some(Cover { image, tint, file })
}

/// A carried cover, written out where a file path can name it.
fn keep(bytes: &[u8], path: &Path, cache: &Path) -> Option<PathBuf> {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let ext = if bytes.starts_with(b"\x89PNG") {
        "png"
    } else {
        "jpg"
    };
    let dir = cache.join("listen");
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join(format!("{:016x}.{ext}", hasher.finish()));
    if !file.exists() {
        std::fs::write(&file, bytes).ok()?;
    }
    Some(file)
}

/// The average colour of what can be seen.
pub fn average(rgba: &[u8]) -> Rgba {
    let (mut sum, mut weight) = ([0u64; 3], 0u64);
    for pixel in rgba.as_chunks::<4>().0.iter().step_by(7) {
        let alpha = u64::from(pixel[3]);
        for (total, channel) in sum.iter_mut().zip(pixel) {
            *total += u64::from(*channel) * alpha;
        }
        weight += alpha;
    }
    if weight == 0 {
        return Rgba::rgb(0, 0, 0);
    }
    Rgba::rgb(
        (sum[0] / weight) as u8,
        (sum[1] / weight) as u8,
        (sum[2] / weight) as u8,
    )
}
