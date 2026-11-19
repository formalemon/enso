//! A picture ready to show at any size: its pixels, and halves of them down
//! to a small one, so a large photo shrinks to the window without shimmer
//! and zooms in from the full pixels.

use std::{path::Path, sync::Arc};

/// Longest side read.
const MAX_SIDE: u32 = 16_384;
/// The smallest half kept.
const SMALLEST: u32 = 256;

/// One size of the picture: premultiplied R, G, B, A.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Photo {
    /// Full size first, then each half of the one before.
    levels: Vec<Level>,
}

impl Photo {
    /// Read the file: PNG, JPEG, WebP, or GIF, turned as the camera was held.
    pub fn open(path: &Path) -> Result<Self, String> {
        let pixels = enso_fs::picture::open(path, MAX_SIDE)
            .map_err(|err| err.to_string())?
            .premultiplied();
        Ok(Self::new(pixels.width, pixels.height, pixels.rgba))
    }

    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        let mut levels = vec![Level {
            width,
            height,
            rgba: rgba.into(),
        }];
        while let Some(last) = levels.last()
            && last.width.max(last.height) / 2 >= SMALLEST
        {
            let half = halve(last);
            levels.push(half);
        }
        Self { levels }
    }

    /// Size in its own pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.levels[0].width, self.levels[0].height)
    }

    /// The level to draw `width` output pixels wide from: the smallest at
    /// least that wide, so it is only ever shrunk by less than half.
    pub fn level_for(&self, width: f32) -> &Level {
        self.levels
            .iter()
            .rev()
            .find(|level| level.width as f32 >= width)
            .unwrap_or(&self.levels[0])
    }

    /// Turned a quarter: clockwise, or not.
    #[must_use]
    pub fn turned(&self, clockwise: bool) -> Self {
        let full = &self.levels[0];
        let (w, h) = (full.width as usize, full.height as usize);
        let mut rgba = vec![0; full.rgba.len()];
        for y in 0..h {
            for x in 0..w {
                // Clockwise: (x, y) goes to (h - 1 - y, x) of an h-wide picture.
                let (nx, ny) = if clockwise { (h - 1 - y, x) } else { (y, w - 1 - x) };
                let from = (y * w + x) * 4;
                let to = (ny * h + nx) * 4;
                rgba[to..to + 4].copy_from_slice(&full.rgba[from..from + 4]);
            }
        }
        Self::new(full.height, full.width, rgba)
    }
}

/// Half the size, each pixel the average of the four it covers.
fn halve(level: &Level) -> Level {
    let (w, h) = (level.width as usize, level.height as usize);
    let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = Vec::with_capacity(hw * hh * 4);
    for y in 0..hh {
        for x in 0..hw {
            let mut sum = [0u32; 4];
            for (sx, sy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let px = (x * 2 + sx).min(w - 1);
                let py = (y * 2 + sy).min(h - 1);
                let at = (py * w + px) * 4;
                for (total, byte) in sum.iter_mut().zip(&level.rgba[at..at + 4]) {
                    *total += u32::from(*byte);
                }
            }
            out.extend(sum.map(|total| ((total + 2) / 4) as u8));
        }
    }
    Level {
        width: hw as u32,
        height: hh as u32,
        rgba: out.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_halves_and_picks_the_one_to_shrink_from() {
        let photo = Photo::new(1200, 600, vec![255; 1200 * 600 * 4]);
        let widths: Vec<u32> = photo.levels.iter().map(|level| level.width).collect();
        assert_eq!(widths, [1200, 600, 300]);
        assert_eq!(photo.level_for(500.0).width, 600);
        assert_eq!(photo.level_for(600.0).width, 600);
        assert_eq!(photo.level_for(100.0).width, 300);
        assert_eq!(photo.level_for(2400.0).width, 1200);
    }

    #[test]
    fn turns_a_quarter_either_way() {
        // 2 × 1: a b.
        let photo = Photo::new(2, 1, vec![1, 0, 0, 255, 2, 0, 0, 255]);
        let right = photo.turned(true);
        assert_eq!(right.size(), (1, 2));
        assert_eq!((right.levels[0].rgba[0], right.levels[0].rgba[4]), (1, 2));
        let left = photo.turned(false);
        assert_eq!((left.levels[0].rgba[0], left.levels[0].rgba[4]), (2, 1));
        assert_eq!(right.turned(false), photo);
    }
}
