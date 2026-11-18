//! Pictures, decoded in pure Rust: PNG, JPEG, WebP, and GIF (its first
//! frame) — for thumbnails and previews, notification pictures, the
//! wallpaper, and the lock screen's backdrop.

use std::{fmt, fs, io, path::Path};

/// Longest side decoded, in pixels: larger files are refused.
pub const MAX_SIDE: u32 = 16_384;

/// Why a file couldn't be shown.
#[derive(Debug)]
pub enum Error {
    Read(io::Error),
    /// Not a kind of picture this reads.
    NotAPicture,
    /// The kind is known, but the picture is broken, or too large.
    Unreadable,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(err) => write!(f, "{err}"),
            Self::NotAPicture => f.write_str("not a PNG, JPEG, WebP, or GIF picture"),
            Self::Unreadable => write!(
                f,
                "a picture that can't be read: broken, or larger than {MAX_SIDE} pixels on a side"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// A picture's pixels: row-major RGBA, four bytes each, alpha straight
/// (not premultiplied).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Pixels {
    /// The same pixels with their colour multiplied by their alpha, as a
    /// canvas draws them.
    #[must_use]
    pub fn premultiplied(mut self) -> Self {
        for pixel in self.rgba.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            if alpha < 255 {
                for channel in &mut pixel[..3] {
                    *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
                }
            }
        }
        self
    }
}

/// The picture in `bytes`, whole: a PNG, JPEG, WebP, or GIF (its first
/// frame), told apart by content rather than by name.
pub fn decode(bytes: &[u8]) -> Result<Pixels, Error> {
    let decoded = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png(bytes)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        decode_jpeg(bytes)
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        decode_webp(bytes)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        decode_gif(bytes)
    } else {
        return Err(Error::NotAPicture);
    };
    decoded.ok_or(Error::Unreadable)
}

/// The picture in the file at `path`, fitted within `max_side` (and not
/// copied when it already fits).
pub fn open(path: &Path, max_side: u32) -> Result<Pixels, Error> {
    let full = decode(&fs::read(path).map_err(Error::Read)?)?;
    if full.width <= max_side && full.height <= max_side {
        return Ok(full);
    }
    Ok(fit(&full, max_side))
}

fn decode_png(bytes: &[u8]) -> Option<Pixels> {
    let mut decoder = png::Decoder::new(io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let (width, height) = (frame.width, frame.height);
    if width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let data = &buffer[..frame.buffer_size()];
    let rgba: Vec<u8> = match frame.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => data
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(Pixels {
        width,
        height,
        rgba,
    })
}

fn decode_webp(bytes: &[u8]) -> Option<Pixels> {
    let mut decoder = image_webp::WebPDecoder::new(io::Cursor::new(bytes)).ok()?;
    let (width, height) = decoder.dimensions();
    if width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let mut buffer = vec![0; decoder.output_buffer_size()?];
    decoder.read_image(&mut buffer).ok()?;
    let rgba = if decoder.has_alpha() {
        buffer
    } else {
        buffer
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect()
    };
    Some(Pixels {
        width,
        height,
        rgba,
    })
}

/// A GIF's first frame, placed on its canvas.
fn decode_gif(bytes: &[u8]) -> Option<Pixels> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(io::Cursor::new(bytes)).ok()?;
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let frame = decoder.read_next_frame().ok()??;
    let mut rgba = vec![0; (width * height * 4) as usize];
    let (left, top) = (u32::from(frame.left), u32::from(frame.top));
    for y in 0..u32::from(frame.height) {
        for x in 0..u32::from(frame.width) {
            let (cx, cy) = (left + x, top + y);
            if cx >= width || cy >= height {
                continue;
            }
            let from = ((y * u32::from(frame.width) + x) * 4) as usize;
            let to = ((cy * width + cx) * 4) as usize;
            rgba[to..to + 4].copy_from_slice(frame.buffer.get(from..from + 4)?);
        }
    }
    Some(Pixels {
        width,
        height,
        rgba,
    })
}

fn decode_jpeg(bytes: &[u8]) -> Option<Pixels> {
    use zune_jpeg::{
        JpegDecoder,
        zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions},
    };
    let options = DecoderOptions::default()
        .set_max_width(MAX_SIDE as usize)
        .set_max_height(MAX_SIDE as usize)
        .jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let rgba = decoder.decode().ok()?;
    let info = decoder.info()?;
    Some(Pixels {
        width: u32::from(info.width),
        height: u32::from(info.height),
        rgba,
    })
}

/// `image` scaled to fit within `max_side`, each new pixel the average of
/// the ones it covers (weighted by alpha, so edges don't darken). Smaller
/// images are left as they are.
pub fn fit(image: &Pixels, max_side: u32) -> Pixels {
    let (w, h) = (image.width, image.height);
    if w <= max_side && h <= max_side {
        return image.clone();
    }
    let scale = max_side as f64 / w.max(h) as f64;
    let (nw, nh) = (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        let (y0, y1) = (y * h / nh, ((y + 1) * h / nh).max(y * h / nh + 1).min(h));
        for x in 0..nw {
            let (x0, x1) = (x * w / nw, ((x + 1) * w / nw).max(x * w / nw + 1).min(w));
            let mut sum = [0u64; 4];
            for sy in y0..y1 {
                let row = (sy * w) as usize * 4;
                for sx in x0..x1 {
                    let p = &image.rgba[row + sx as usize * 4..][..4];
                    let a = u64::from(p[3]);
                    sum[0] += u64::from(p[0]) * a;
                    sum[1] += u64::from(p[1]) * a;
                    sum[2] += u64::from(p[2]) * a;
                    sum[3] += a;
                }
            }
            let count = u64::from((y1 - y0) * (x1 - x0));
            // Fully transparent: nothing to average.
            let channel = |c: u64| c.checked_div(sum[3]).unwrap_or(0) as u8;
            out.extend([
                channel(sum[0]),
                channel(sum[1]),
                channel(sum[2]),
                (sum[3] / count) as u8,
            ]);
        }
    }
    Pixels {
        width: nw,
        height: nh,
        rgba: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_what_is_not_a_picture_from_what_is_broken() {
        let dir = std::env::temp_dir().join(format!("enso-fs-picture-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let text = dir.join("notes.png");
        fs::write(&text, "not a picture").unwrap();
        assert!(matches!(open(&text, 64), Err(Error::NotAPicture)));
        let broken = dir.join("cut.png");
        fs::write(&broken, b"\x89PNG\r\n\x1a\nhalf").unwrap();
        assert!(matches!(open(&broken, 64), Err(Error::Unreadable)));
        assert!(matches!(
            open(&dir.join("missing"), 64),
            Err(Error::Read(_))
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn premultiplies_what_is_see_through() {
        let pixels = Pixels {
            width: 2,
            height: 1,
            rgba: vec![200, 100, 50, 255, 200, 100, 50, 128],
        }
        .premultiplied();
        assert_eq!(pixels.rgba, [200, 100, 50, 255, 100, 50, 25, 128]);
    }
}
