//! Full-screen pictures: wallpapers, and the soft backdrop made from one.
//!
//! An [`Image`](crate::Image) is a thumbnail, kept small and drawn at any
//! size. A [`Picture`] is the opposite: it is fitted to a screen once, pixel
//! for pixel, and then copied onto the canvas as it is — which is what keeps
//! a wallpaper sharp and a redraw over it cheap.

use std::collections::VecDeque;

use enso_core::{Rgba, wallpaper::Fit};

/// Longest side accepted, in pixels. Larger files are refused, not decoded.
const MAX_SIDE: u32 = 16_384;

/// Opaque pixels, row-major, 4 bytes each in R, G, B, A order.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picture")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Picture {
    /// From premultiplied RGBA pixels. `None` if the numbers do not describe
    /// `rgba`.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        let sane = width > 0 && height > 0 && width <= MAX_SIDE && height <= MAX_SIDE;
        (sane && rgba.len() == width as usize * height as usize * 4).then_some(Self {
            width,
            height,
            rgba,
        })
    }

    /// One colour, `width × height`.
    pub fn solid(width: u32, height: u32, color: Rgba) -> Self {
        let (width, height) = (width.clamp(1, MAX_SIDE), height.clamp(1, MAX_SIDE));
        let pixel = [color.r, color.g, color.b, 255];
        Self {
            width,
            height,
            rgba: pixel.repeat(width as usize * height as usize),
        }
    }

    /// Decode a PNG or JPEG file, told apart by content rather than by name.
    #[cfg(feature = "picture")]
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
        decode(&bytes).map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// This picture made exactly `width × height`. [`Fit::Cover`] fills the
    /// frame and crops the overflow evenly; [`Fit::Contain`] shows all of it,
    /// centred on `background`.
    ///
    /// Shrinking averages over every source pixel a result pixel covers, so
    /// a large photograph comes down without shimmer; enlarging interpolates.
    #[must_use]
    pub fn fitted(&self, width: u32, height: u32, fit: Fit, background: Rgba) -> Self {
        let (width, height) = (width.clamp(1, MAX_SIDE), height.clamp(1, MAX_SIDE));
        let (sw, sh) = (self.width as f32, self.height as f32);
        let (dw, dh) = (width as f32, height as f32);
        match fit {
            Fit::Cover => {
                // The part of the source with the frame's shape.
                let scale = (dw / sw).max(dh / sh);
                let (cw, ch) = ((dw / scale).min(sw), (dh / scale).min(sh));
                let crop = ((sw - cw) / 2.0, (sh - ch) / 2.0, cw, ch);
                Self {
                    width,
                    height,
                    rgba: resample(self, crop, width, height),
                }
            }
            Fit::Contain => {
                let scale = (dw / sw).min(dh / sh);
                let iw = ((sw * scale).round() as u32).clamp(1, width);
                let ih = ((sh * scale).round() as u32).clamp(1, height);
                let inner = resample(self, (0.0, 0.0, sw, sh), iw, ih);
                let mut out = Self::solid(width, height, background);
                let (left, top) = (((width - iw) / 2) as usize, ((height - ih) / 2) as usize);
                let row = iw as usize * 4;
                for (y, source) in inner.chunks_exact(row).enumerate() {
                    let at = ((top + y) * width as usize + left) * 4;
                    out.rgba[at..at + row].copy_from_slice(source);
                }
                // Transparent parts of the picture show the background too.
                out.flatten(background);
                out
            }
        }
    }

    /// Blur by about `radius` pixels: three box passes, which is as close to
    /// a Gaussian as anyone can see.
    #[must_use]
    pub fn blurred(mut self, radius: u32) -> Self {
        if radius == 0 {
            return self;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let mut scratch = vec![0; self.rgba.len()];
        for _ in 0..3 {
            box_blur(&self.rgba, &mut scratch, w, h, radius as usize, false);
            box_blur(&scratch, &mut self.rgba, w, h, radius as usize, true);
        }
        self
    }

    /// Mix every pixel `amount` of the way towards `color`: how a wallpaper
    /// is quietened into a backdrop that text can sit on.
    #[must_use]
    pub fn tinted(mut self, color: Rgba, amount: f32) -> Self {
        let amount = (amount.clamp(0.0, 1.0) * 256.0) as u32;
        let target = [color.r, color.g, color.b];
        for pixel in self.rgba.as_chunks_mut::<4>().0 {
            for (channel, to) in pixel.iter_mut().zip(target) {
                let from = u32::from(*channel);
                *channel = ((from * (256 - amount) + u32::from(to) * amount) >> 8) as u8;
            }
            pixel[3] = 255;
        }
        self
    }

    /// Put `background` behind whatever is not opaque.
    fn flatten(&mut self, background: Rgba) {
        let behind = [background.r, background.g, background.b];
        for pixel in self.rgba.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            if alpha == 255 {
                continue;
            }
            for (channel, under) in pixel.iter_mut().zip(behind) {
                // The pixels are premultiplied: only the background is scaled.
                let under = (u32::from(under) * (255 - alpha) + 127) / 255;
                *channel = (u32::from(*channel) + under).min(255) as u8;
            }
            pixel[3] = 255;
        }
    }
}

#[cfg(feature = "picture")]
fn decode(bytes: &[u8]) -> Result<Picture, String> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const JPEG: &[u8] = &[0xff, 0xd8, 0xff];
    let too_large = || format!("larger than {MAX_SIDE} pixels on a side");

    if bytes.starts_with(PNG) {
        let pixmap = tiny_skia::Pixmap::decode_png(bytes).map_err(|err| err.to_string())?;
        let (width, height) = (pixmap.width(), pixmap.height());
        return Picture::from_rgba(width, height, pixmap.take()).ok_or_else(too_large);
    }
    if bytes.starts_with(JPEG) {
        use zune_jpeg::{
            JpegDecoder,
            zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions},
        };
        let options = DecoderOptions::default()
            .set_max_width(MAX_SIDE as usize)
            .set_max_height(MAX_SIDE as usize)
            .jpeg_set_out_colorspace(ColorSpace::RGBA);
        let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
        let rgba = decoder.decode().map_err(|err| err.to_string())?;
        let info = decoder.info().ok_or("no image header")?;
        return Picture::from_rgba(u32::from(info.width), u32::from(info.height), rgba)
            .ok_or_else(too_large);
    }
    Err("not a PNG or JPEG image".to_owned())
}

/// For each output position along one axis: the first source index it reads
/// and the weight of each source pixel from there on.
struct Taps {
    starts: Vec<usize>,
    /// `width` weights per output position, summing to one.
    weights: Vec<f32>,
    width: usize,
}

impl Taps {
    /// Map `out` positions onto the source span `offset .. offset + span`
    /// of an axis `len` long, with a triangle filter that widens when
    /// shrinking so that every source pixel is counted.
    fn new(len: usize, offset: f32, span: f32, out: usize) -> Self {
        let ratio = span / out as f32;
        let support = ratio.max(1.0);
        let width = (support * 2.0).ceil() as usize + 1;
        let mut starts = Vec::with_capacity(out);
        let mut weights = vec![0.0; out * width];
        let last = len.saturating_sub(1) as isize;
        for (position, taps) in weights.chunks_exact_mut(width).enumerate() {
            let center = offset + (position as f32 + 0.5) * ratio;
            let first = (center - support).floor() as isize;
            let start = first.clamp(0, last);
            starts.push(start as usize);
            let mut total = 0.0;
            for tap in 0..width as isize {
                let source = first + tap;
                let distance = (source as f32 + 0.5 - center).abs() / support;
                let weight = (1.0 - distance).max(0.0);
                // Past either end, the edge pixel stands in.
                let slot = (source.clamp(0, last) - start).clamp(0, width as isize - 1);
                taps[slot as usize] += weight;
                total += weight;
            }
            if total > 0.0 {
                for weight in taps.iter_mut() {
                    *weight /= total;
                }
            }
        }
        Self {
            starts,
            weights,
            width,
        }
    }

    fn at(&self, position: usize) -> (usize, &[f32]) {
        let from = position * self.width;
        (
            self.starts[position],
            &self.weights[from..from + self.width],
        )
    }
}

/// Resample the `crop` (`x, y, w, h` in source pixels) of `source` to
/// `width × height`. Rows are filtered across first and kept only while a
/// result row still needs them, so the working memory is a handful of rows
/// whatever the size of the picture.
fn resample(source: &Picture, crop: (f32, f32, f32, f32), width: u32, height: u32) -> Vec<u8> {
    let (sw, sh) = (source.width as usize, source.height as usize);
    let (dw, dh) = (width as usize, height as usize);
    let across = Taps::new(sw, crop.0, crop.2, dw);
    let down = Taps::new(sh, crop.1, crop.3, dh);

    let filter_row = |y: usize| -> Vec<f32> {
        let row = &source.rgba[y * sw * 4..(y + 1) * sw * 4];
        let mut out = vec![0.0; dw * 4];
        for (x, pixel) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (start, weights) = across.at(x);
            for (tap, weight) in weights.iter().enumerate() {
                if *weight == 0.0 {
                    continue;
                }
                let at = (start + tap).min(sw - 1) * 4;
                for (sum, value) in pixel.iter_mut().zip(&row[at..at + 4]) {
                    *sum += f32::from(*value) * weight;
                }
            }
        }
        out
    };

    let mut out = vec![0; dw * dh * 4];
    // Filtered source rows, oldest first, with the index of the first.
    let mut rows: VecDeque<Vec<f32>> = VecDeque::new();
    let mut first = 0;
    let mut sums = vec![0.0f32; dw * 4];
    for (y, line) in out.chunks_exact_mut(dw * 4).enumerate() {
        let (start, weights) = down.at(y);
        while first < start && !rows.is_empty() {
            rows.pop_front();
            first += 1;
        }
        if rows.is_empty() {
            first = start;
        }
        sums.fill(0.0);
        for (tap, weight) in weights.iter().enumerate() {
            if *weight == 0.0 {
                continue;
            }
            let needed = (start + tap).min(sh - 1);
            while first + rows.len() <= needed {
                rows.push_back(filter_row(first + rows.len()));
            }
            for (sum, value) in sums.iter_mut().zip(&rows[needed - first]) {
                *sum += value * weight;
            }
        }
        for (byte, sum) in line.iter_mut().zip(&sums) {
            *byte = (sum + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// One box-blur pass along rows (`vertical` false) or columns, with a
/// running sum: the cost does not depend on the radius.
fn box_blur(from: &[u8], to: &mut [u8], w: usize, h: usize, radius: usize, vertical: bool) {
    let (lines, len, step, line_step) = if vertical {
        (w, h, w * 4, 4)
    } else {
        (h, w, 4, w * 4)
    };
    let window = (radius * 2 + 1) as u32;
    for line in 0..lines {
        let base = line * line_step;
        let at = |i: isize| base + (i.clamp(0, len as isize - 1) as usize) * step;
        let mut sum = [0u32; 4];
        for i in -(radius as isize)..=radius as isize {
            let p = at(i);
            for (total, value) in sum.iter_mut().zip(&from[p..p + 4]) {
                *total += u32::from(*value);
            }
        }
        for i in 0..len as isize {
            let p = at(i);
            for (byte, total) in to[p..p + 4].iter_mut().zip(sum) {
                *byte = ((total + window / 2) / window) as u8;
            }
            let (leave, enter) = (at(i - radius as isize), at(i + radius as isize + 1));
            for channel in 0..4 {
                sum[channel] += u32::from(from[enter + channel]);
                sum[channel] -= u32::from(from[leave + channel]);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Left half red, right half blue.
    fn halves(width: u32, height: u32) -> Picture {
        let mut rgba = Vec::new();
        for _ in 0..height {
            for x in 0..width {
                rgba.extend(if x < width / 2 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                });
            }
        }
        Picture::from_rgba(width, height, rgba).unwrap()
    }

    fn pixel(picture: &Picture, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * picture.width as usize + x as usize) * 4;
        [
            picture.rgba[at],
            picture.rgba[at + 1],
            picture.rgba[at + 2],
            picture.rgba[at + 3],
        ]
    }

    #[test]
    fn refuses_sizes_that_do_not_describe_the_pixels() {
        assert!(Picture::from_rgba(2, 2, vec![0; 16]).is_some());
        assert!(Picture::from_rgba(2, 2, vec![0; 15]).is_none());
        assert!(Picture::from_rgba(0, 2, Vec::new()).is_none());
    }

    #[test]
    fn cover_fills_the_frame_and_crops_evenly() {
        let black = Rgba::rgb(0, 0, 0);
        // A wide picture into a square frame: the middle is kept, so the
        // seam between the halves stays in the centre.
        let fitted = halves(400, 100).fitted(50, 50, Fit::Cover, black);
        assert_eq!(fitted.size(), (50, 50));
        assert_eq!(pixel(&fitted, 2, 25), [255, 0, 0, 255]);
        assert_eq!(pixel(&fitted, 47, 25), [0, 0, 255, 255]);

        // Enlarging keeps flat areas flat and the edges in place.
        let grown = halves(4, 2).fitted(400, 200, Fit::Cover, black);
        assert_eq!(pixel(&grown, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&grown, 399, 199), [0, 0, 255, 255]);

        // Same size: untouched.
        let same = halves(40, 20);
        assert_eq!(same.fitted(40, 20, Fit::Cover, black), same);
    }

    #[test]
    fn shrinking_averages_instead_of_skipping() {
        // One-pixel stripes of black and white, shrunk eightfold, come out
        // an even grey rather than whichever stripe was sampled.
        let mut rgba = Vec::new();
        for _ in 0..16 {
            for x in 0..160 {
                let value = if x % 2 == 0 { 0 } else { 255 };
                rgba.extend([value, value, value, 255]);
            }
        }
        let stripes = Picture::from_rgba(160, 16, rgba).unwrap();
        let small = stripes.fitted(20, 2, Fit::Cover, Rgba::rgb(0, 0, 0));
        for x in 1..19 {
            let [value, _, _, alpha] = pixel(&small, x, 1);
            assert!((120..=135).contains(&value), "{value} at {x}");
            assert_eq!(alpha, 255);
        }
    }

    #[test]
    fn contain_shows_everything_on_the_background() {
        let green = Rgba::rgb(0, 255, 0);
        let fitted = halves(100, 50).fitted(100, 100, Fit::Contain, green);
        assert_eq!(pixel(&fitted, 50, 5), [0, 255, 0, 255]);
        assert_eq!(pixel(&fitted, 50, 95), [0, 255, 0, 255]);
        assert_eq!(pixel(&fitted, 5, 50), [255, 0, 0, 255]);
        assert_eq!(pixel(&fitted, 95, 50), [0, 0, 255, 255]);
    }

    #[test]
    fn blur_softens_edges_and_keeps_flat_areas() {
        let blurred = halves(64, 8).blurred(4);
        assert_eq!(pixel(&blurred, 2, 4), [255, 0, 0, 255]);
        assert_eq!(pixel(&blurred, 61, 4), [0, 0, 255, 255]);
        let [r, _, b, a] = pixel(&blurred, 32, 4);
        assert!(r > 60 && b > 60, "seam should be a mix, got {r} {b}");
        assert_eq!(a, 255);
    }

    #[test]
    fn tint_moves_towards_the_colour() {
        let white = Rgba::rgb(255, 255, 255);
        let tinted = Picture::solid(2, 2, Rgba::rgb(0, 0, 0)).tinted(white, 0.5);
        let [value, ..] = pixel(&tinted, 0, 0);
        assert!((126..=129).contains(&value), "{value}");
        let all = Picture::solid(2, 2, Rgba::rgb(0, 0, 0)).tinted(white, 1.0);
        assert_eq!(pixel(&all, 1, 1), [255, 255, 255, 255]);
    }

    #[cfg(feature = "picture")]
    #[test]
    fn reads_png_and_refuses_other_files() {
        let dir = std::env::temp_dir().join(format!("enso-ui-picture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("wall.jpg"); // the name is not what decides
        let mut pixmap = tiny_skia::Pixmap::new(3, 2).unwrap();
        pixmap.fill(tiny_skia::Color::from_rgba8(10, 20, 30, 255));
        pixmap.save_png(&png).unwrap();
        let picture = Picture::open(&png).unwrap();
        assert_eq!(picture.size(), (3, 2));
        assert_eq!(pixel(&picture, 2, 1), [10, 20, 30, 255]);

        let text = dir.join("notes.png");
        std::fs::write(&text, "not a picture").unwrap();
        assert!(Picture::open(&text).is_err());
        assert!(Picture::open(dir.join("missing.png")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
