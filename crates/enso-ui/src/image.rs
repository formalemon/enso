//! Raster images, held ready to draw: premultiplied pixels, already scaled
//! down to the size interfaces show them at.

use std::sync::Arc;

/// Longest side kept, in pixels. Notification pictures and the like are
/// drawn far smaller; anything larger is averaged down on the way in.
const MAX_SIDE: u32 = 192;

/// A picture to draw with [`Canvas::image`](crate::Canvas::image). Cloning
/// shares the pixels.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Premultiplied, row-major, 4 bytes per pixel in R, G, B, A order.
    pub(crate) rgba: Arc<[u8]>,
    /// The same pixels in B, G, R, A order, for Wayland buffers.
    pub(crate) bgra: Arc<[u8]>,
    /// Whether every pixel is opaque: a photo, not a cut-out.
    opaque: bool,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Image {
    /// From raw, unpremultiplied pixels as desktop notifications carry them:
    /// rows of `rowstride` bytes, 3 bytes per pixel or 4 with `has_alpha`.
    /// `None` if the numbers do not describe `data`.
    pub fn from_raw(
        width: u32,
        height: u32,
        rowstride: usize,
        has_alpha: bool,
        data: &[u8],
    ) -> Option<Self> {
        let channels = if has_alpha { 4 } else { 3 };
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || w > 8192 || h > 8192 || rowstride < w * channels {
            return None;
        }
        // The last row may stop at its last pixel, short of a full stride.
        if data.len() < rowstride * (h - 1) + w * channels {
            return None;
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for row in data.chunks(rowstride).take(h) {
            for pixel in row.chunks_exact(channels).take(w) {
                let alpha = if has_alpha { pixel[3] } else { 255 };
                let mul = |c: u8| ((u32::from(c) * u32::from(alpha) + 127) / 255) as u8;
                rgba.extend([mul(pixel[0]), mul(pixel[1]), mul(pixel[2]), alpha]);
            }
        }
        Some(Self::from_premultiplied(width, height, rgba, MAX_SIDE))
    }

    /// [`Image::from_raw`] for a picture shown large — an album's cover
    /// filling a window — kept up to `max_side` pixels on its longer side
    /// rather than the usual small size.
    pub fn from_raw_up_to(
        width: u32,
        height: u32,
        has_alpha: bool,
        data: &[u8],
        max_side: u32,
    ) -> Option<Self> {
        let channels = if has_alpha { 4 } else { 3 };
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || w > 16_384 || h > 16_384 || data.len() < w * h * channels {
            return None;
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for pixel in data.chunks_exact(channels).take(w * h) {
            let alpha = if has_alpha { pixel[3] } else { 255 };
            let mul = |c: u8| ((u32::from(c) * u32::from(alpha) + 127) / 255) as u8;
            rgba.extend([mul(pixel[0]), mul(pixel[1]), mul(pixel[2]), alpha]);
        }
        Some(Self::from_premultiplied(width, height, rgba, max_side.max(1)))
    }

    /// Image size in its own pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Whether it has no see-through pixels, so drawn over a rectangle it
    /// covers all of it.
    pub fn is_opaque(&self) -> bool {
        self.opaque
    }

    fn from_premultiplied(width: u32, height: u32, rgba: Vec<u8>, max_side: u32) -> Self {
        let (width, height, rgba) = shrink(width, height, rgba, max_side);
        let opaque = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[3] == u8::MAX);
        let mut bgra = rgba.clone();
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        Self {
            width,
            height,
            rgba: rgba.into(),
            bgra: bgra.into(),
            opaque,
        }
    }
}

/// Average whole blocks of pixels until the longest side fits [`MAX_SIDE`].
fn shrink(width: u32, height: u32, rgba: Vec<u8>, max_side: u32) -> (u32, u32, Vec<u8>) {
    let factor = width.max(height).div_ceil(max_side) as usize;
    if factor <= 1 {
        return (width, height, rgba);
    }
    let (w, h) = (width as usize, height as usize);
    let (out_w, out_h) = ((w / factor).max(1), (h / factor).max(1));
    let mut out = Vec::with_capacity(out_w * out_h * 4);
    for oy in 0..out_h {
        for ox in 0..out_w {
            let mut sum = [0u32; 4];
            let mut count = 0;
            for y in (oy * factor..((oy + 1) * factor).min(h)).take(factor) {
                for x in (ox * factor..((ox + 1) * factor).min(w)).take(factor) {
                    let at = (y * w + x) * 4;
                    for (total, byte) in sum.iter_mut().zip(&rgba[at..at + 4]) {
                        *total += u32::from(*byte);
                    }
                    count += 1;
                }
            }
            out.extend(sum.map(|total| (total / count.max(1)) as u8));
        }
    }
    (out_w as u32, out_h as u32, out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn reads_raw_pixels_with_and_without_alpha() {
        // 2×1 RGB with a padded row.
        let image = Image::from_raw(2, 1, 8, false, &[255, 0, 0, 0, 255, 0, 9, 9]).unwrap();
        assert_eq!(image.size(), (2, 1));
        assert_eq!(&image.rgba[..], [255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!(&image.bgra[..], [0, 0, 255, 255, 0, 255, 0, 255]);

        // Alpha is premultiplied in.
        let image = Image::from_raw(1, 1, 4, true, &[200, 100, 0, 128]).unwrap();
        assert_eq!(&image.rgba[..], [100, 50, 0, 128]);

        // The last row may omit its padding…
        assert!(Image::from_raw(1, 2, 8, false, &[0; 11]).is_some());
        // …but not its pixels; and nonsense is refused.
        assert!(Image::from_raw(1, 2, 8, false, &[0; 10]).is_none());
        assert!(Image::from_raw(0, 1, 4, true, &[]).is_none());
        assert!(Image::from_raw(2, 1, 4, true, &[0; 8]).is_none());
    }

    #[test]
    fn large_images_are_averaged_down() {
        // 400×200, left half white and right half black.
        let mut data = Vec::new();
        for _ in 0..200 {
            for x in 0..400 {
                let value = if x < 200 { 255 } else { 0 };
                data.extend([value, value, value]);
            }
        }
        let image = Image::from_raw(400, 200, 1200, false, &data).unwrap();
        assert_eq!(image.size(), (133, 66));
        assert_eq!(&image.rgba[..4], [255, 255, 255, 255]);
        let last = image.rgba.len() - 4;
        assert_eq!(&image.rgba[last..], [0, 0, 0, 255]);
    }
}
