//! Pictures, decoded in pure Rust: PNG, JPEG, WebP, and GIF (its first
//! frame) — for thumbnails and previews, notification pictures, the
//! wallpaper, and the lock screen's backdrop. A camera's photo is turned
//! the way it was held: the EXIF orientation it carries is applied.

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
    let decoded = decoded.ok_or(Error::Unreadable)?;
    Ok(match exif(bytes).and_then(orientation) {
        Some(turn @ 2..=8) => oriented(&decoded, turn),
        _ => decoded,
    })
}

/// The EXIF block (a TIFF structure) a JPEG, PNG, or WebP carries, if any.
fn exif(bytes: &[u8]) -> Option<&[u8]> {
    const TIFF_AFTER: &[u8] = b"Exif\0\0";
    if bytes.starts_with(&[0xff, 0xd8]) {
        // JPEG: segments up to the image data; EXIF is an APP1.
        let mut at = 2;
        while at + 4 <= bytes.len() && bytes[at] == 0xff {
            let marker = bytes[at + 1];
            if marker == 0xda || marker == 0xd9 {
                break;
            }
            let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
            let body = bytes.get(at + 4..at + 2 + length)?;
            if marker == 0xe1 && body.starts_with(TIFF_AFTER) {
                return Some(&body[TIFF_AFTER.len()..]);
            }
            at += 2 + length;
        }
        None
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        // PNG: an eXIf chunk, before the image data.
        let mut at = 8;
        while at + 8 <= bytes.len() {
            let length = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
            let kind = &bytes[at + 4..at + 8];
            if kind == b"IDAT" {
                break;
            }
            let body = bytes.get(at + 8..at + 8 + length)?;
            if kind == b"eXIf" {
                return Some(body);
            }
            at += 12 + length;
        }
        None
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        // WebP: an EXIF chunk, padded to even lengths.
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let kind = &bytes[at..at + 4];
            let length = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
            let body = bytes.get(at + 8..at + 8 + length)?;
            if kind == b"EXIF" {
                return Some(body.strip_prefix(TIFF_AFTER).unwrap_or(body));
            }
            at += 8 + length + (length & 1);
        }
        None
    } else {
        None
    }
}

/// What a photo says of itself: when it was taken, and with what.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Details {
    /// `2026:11:02 10:00:00`, as the camera wrote it.
    pub taken: Option<String>,
    /// The make and model, as the camera names itself: `Google Pixel 8`.
    pub camera: Option<String>,
}

/// The details `bytes` (a JPEG, PNG, or WebP file) carry in their EXIF.
pub fn details(bytes: &[u8]) -> Details {
    let Some(tiff) = exif(bytes) else {
        return Details::default();
    };
    let make = ascii(tiff, 0, 0x010f);
    let model = ascii(tiff, 0, 0x0110);
    let camera = match (make, model) {
        // Models often lead with the make already: "Canon EOS R6".
        (Some(make), Some(model)) if model.to_lowercase().starts_with(&make.to_lowercase()) => {
            Some(model)
        }
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => make.or(model),
    };
    // When the shutter went, from the EXIF block's own; else when it was
    // last written.
    let taken = entry_u32(tiff, 0, 0x8769)
        .and_then(|exif| ascii(tiff, exif as usize, 0x9003))
        .or_else(|| ascii(tiff, 0, 0x0132));
    Details { taken, camera }
}

/// A TIFF block's byte order, and its first directory.
fn tiff_start(tiff: &[u8]) -> Option<(bool, usize)> {
    let big = match tiff.get(..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let read = |at: usize, len: usize| tiff.get(at..at + len);
    let magic = read(2, 2)?;
    let magic = if big {
        u16::from_be_bytes([magic[0], magic[1]])
    } else {
        u16::from_le_bytes([magic[0], magic[1]])
    };
    if magic != 42 {
        return None;
    }
    let first = read(4, 4)?;
    let first = if big {
        u32::from_be_bytes([first[0], first[1], first[2], first[3]])
    } else {
        u32::from_le_bytes([first[0], first[1], first[2], first[3]])
    };
    Some((big, first as usize))
}

/// The 12-byte entry for `tag` in the directory at `dir` (`0`: the first).
fn entry(tiff: &[u8], dir: usize, tag: u16) -> Option<(bool, usize)> {
    let (big, first) = tiff_start(tiff)?;
    let dir = if dir == 0 { first } else { dir };
    let u16_at = |at: usize| -> Option<u16> {
        let pair: [u8; 2] = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        })
    };
    let count = usize::from(u16_at(dir)?).min(512);
    (0..count)
        .map(|index| dir + 2 + index * 12)
        .find(|&at| u16_at(at) == Some(tag))
        .map(|at| (big, at))
}

fn entry_u32(tiff: &[u8], dir: usize, tag: u16) -> Option<u32> {
    let (big, at) = entry(tiff, dir, tag)?;
    let four: [u8; 4] = tiff.get(at + 8..at + 12)?.try_into().ok()?;
    Some(if big {
        u32::from_be_bytes(four)
    } else {
        u32::from_le_bytes(four)
    })
}

/// A text entry: inside the entry when four bytes or fewer, else where it
/// points.
fn ascii(tiff: &[u8], dir: usize, tag: u16) -> Option<String> {
    let (big, at) = entry(tiff, dir, tag)?;
    let four: [u8; 4] = tiff.get(at + 4..at + 8)?.try_into().ok()?;
    let count = if big {
        u32::from_be_bytes(four)
    } else {
        u32::from_le_bytes(four)
    } as usize;
    let count = count.min(256);
    let bytes = if count <= 4 {
        tiff.get(at + 8..at + 8 + count)?
    } else {
        let offset = entry_u32(tiff, dir, tag)? as usize;
        tiff.get(offset..offset + count)?
    };
    let text: String = String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .trim()
        .to_owned();
    (!text.is_empty()).then_some(text)
}

/// The orientation an EXIF block gives (1 to 8; 1 is as stored).
fn orientation(tiff: &[u8]) -> Option<u16> {
    let big = match tiff.get(..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let pair: [u8; 2] = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let four: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(four)
        } else {
            u32::from_le_bytes(four)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let first = u32_at(4)? as usize;
    let count = usize::from(u16_at(first)?);
    (0..count)
        .map(|entry| first + 2 + entry * 12)
        .find(|&entry| u16_at(entry) == Some(0x0112))
        .and_then(|entry| u16_at(entry + 8))
        .filter(|turn| (1..=8).contains(turn))
}

/// `image` turned and flipped as EXIF orientation `turn` says it was held.
fn oriented(image: &Pixels, turn: u16) -> Pixels {
    let (w, h) = (image.width, image.height);
    let swapped = turn >= 5;
    let (out_w, out_h) = if swapped { (h, w) } else { (w, h) };
    let mut rgba = Vec::with_capacity(image.rgba.len());
    for y in 0..out_h {
        for x in 0..out_w {
            let (sx, sy) = match turn {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                8 => (w - 1 - y, x),
                _ => (x, y),
            };
            let at = ((sy * w + sx) * 4) as usize;
            rgba.extend_from_slice(&image.rgba[at..at + 4]);
        }
    }
    Pixels {
        width: out_w,
        height: out_h,
        rgba,
    }
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

    /// A TIFF block saying `turn`, little-endian or big.
    fn tiff(turn: u16, big: bool) -> Vec<u8> {
        let u16b = |v: u16| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let u32b = |v: u32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let mut out = Vec::new();
        out.extend(if big { b"MM" } else { b"II" });
        out.extend(u16b(42));
        out.extend(u32b(8));
        out.extend(u16b(2));
        // Another tag first, then the orientation: SHORT, one of it.
        for (tag, value) in [(0x010f, 7), (0x0112, turn)] {
            out.extend(u16b(tag));
            out.extend(u16b(3));
            out.extend(u32b(1));
            out.extend(u16b(value));
            out.extend([0, 0]);
        }
        out.extend(u32b(0));
        out
    }

    #[test]
    fn reads_the_orientation_from_exif() {
        assert_eq!(orientation(&tiff(6, false)), Some(6));
        assert_eq!(orientation(&tiff(8, true)), Some(8));
        assert_eq!(orientation(&tiff(9, true)), None);
        assert_eq!(orientation(b"II*\0"), None);
        // Found in a JPEG's APP1, after another segment.
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, 1, 2];
        let body = [b"Exif\0\0".as_slice(), &tiff(3, false)].concat();
        jpeg.extend([0xff, 0xe1]);
        jpeg.extend(u16::try_from(body.len() + 2).unwrap().to_be_bytes());
        jpeg.extend(&body);
        jpeg.extend([0xff, 0xda, 0, 2]);
        assert_eq!(exif(&jpeg).and_then(orientation), Some(3));
    }

    #[test]
    fn turns_a_photo_the_way_it_was_held() {
        // 3 × 2: a b c / d e f, one byte standing for each pixel.
        let image = Pixels {
            width: 3,
            height: 2,
            rgba: b"abcdef".iter().flat_map(|&c| [c, 0, 0, 255]).collect(),
        };
        let read = |p: &Pixels| -> (u32, u32, String) {
            let text = p.rgba.chunks(4).map(|px| px[0] as char).collect();
            (p.width, p.height, text)
        };
        assert_eq!(read(&oriented(&image, 2)), (3, 2, "cbafed".into()));
        assert_eq!(read(&oriented(&image, 3)), (3, 2, "fedcba".into()));
        assert_eq!(read(&oriented(&image, 4)), (3, 2, "defabc".into()));
        assert_eq!(read(&oriented(&image, 5)), (2, 3, "adbecf".into()));
        // Held turned right: the left column becomes the top row.
        assert_eq!(read(&oriented(&image, 6)), (2, 3, "daebfc".into()));
        assert_eq!(read(&oriented(&image, 7)), (2, 3, "fcebda".into()));
        assert_eq!(read(&oriented(&image, 8)), (2, 3, "cfbead".into()));
    }

    #[test]
    fn a_png_carrying_exif_is_turned_too() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_chunk(png::chunk::ChunkType(*b"eXIf"), &tiff(6, true))
                .unwrap();
            writer.write_image_data(&[1, 0, 0, 255, 2, 0, 0, 255]).unwrap();
        }
        let picture = decode(&bytes).unwrap();
        assert_eq!((picture.width, picture.height), (1, 2));
        assert_eq!((picture.rgba[0], picture.rgba[4]), (1, 2));
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
