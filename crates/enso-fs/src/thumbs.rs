//! Thumbnails, as the freedesktop thumbnail specification keeps them: one
//! PNG per file in `~/.cache/thumbnails/<size>/`, named by the MD5 of the
//! file's URI, carrying the URI and the file's modification time — so a
//! thumbnail made here serves every program that reads the spec, and one
//! made by another program is used here, until the file changes.
//!
//! What can't be thumbnailed is remembered too (`fail/enso-files/`), so a
//! broken picture is tried once, not on every scroll past.
//!
//! PNG, JPEG, WebP, and GIF (its first frame) are decoded in pure Rust;
//! anything else is only shown if another program has already made its
//! thumbnail.

use enso_core::disk;
use std::{
    fs,
    io::{self},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use crate::uri;

/// The sizes the spec names, by their longest side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Size {
    Normal,
    Large,
    XLarge,
}

impl Size {
    pub fn pixels(self) -> u32 {
        match self {
            Self::Normal => 128,
            Self::Large => 256,
            Self::XLarge => 512,
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Large => "large",
            Self::XLarge => "x-large",
        }
    }

    /// The smallest size at least `pixels` across.
    pub fn for_pixels(pixels: f32) -> Self {
        if pixels <= 128.0 {
            Self::Normal
        } else if pixels <= 256.0 {
            Self::Large
        } else {
            Self::XLarge
        }
    }
}

/// Pixels: straight (not premultiplied) RGBA, row after row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Files larger than this aren't decoded for a thumbnail.
const MAX_BYTES: u64 = 64 << 20;
/// Nor pictures larger than this on a side.
const MAX_SIDE: u32 = 16_384;

/// Whether a file of this name is one this module can thumbnail itself.
pub fn can_make(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".jpe", ".webp", ".gif"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Where thumbnails live.
fn cache_dir() -> PathBuf {
    let home = crate::places::home();
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".cache"))
        .join("thumbnails")
}

/// `path`'s thumbnail at `size`: from the cache if it's still good, else
/// made now (and kept) if this module can, else `None`.
pub fn thumbnail(path: &Path, size: Size) -> Option<Thumb> {
    thumbnail_in(&cache_dir(), path, size)
}

fn thumbnail_in(cache: &Path, path: &Path, size: Size) -> Option<Thumb> {
    let path = std::path::absolute(path).ok()?;
    // Never thumbnail the thumbnails.
    if path.starts_with(cache) {
        return None;
    }
    let meta = fs::metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let uri = uri::from_path(&path);
    let name = format!("{}.png", enso_core::hex::encode(&md5(uri.as_bytes())));
    let mtime = meta.mtime();
    let cached = cache.join(size.dir()).join(&name);
    if let Some(thumb) = read_cached(&cached, &uri, mtime) {
        return Some(thumb);
    }
    let name_str = path.file_name()?.to_string_lossy();
    if !can_make(&name_str) || meta.len() > MAX_BYTES {
        return None;
    }
    let failed = cache.join("fail/enso-files").join(&name);
    if read_text(&failed)
        .is_some_and(|text| text.mtime == Some(mtime) && text.uri.as_deref() == Some(&uri))
    {
        return None;
    }
    match decode(&path, size.pixels()) {
        Some(thumb) => {
            let _ = write(&cached, &thumb, &uri, mtime, meta.len());
            Some(thumb)
        }
        None => {
            let _ = write(
                &failed,
                &Thumb {
                    width: 1,
                    height: 1,
                    rgba: vec![0; 4],
                },
                &uri,
                mtime,
                meta.len(),
            );
            None
        }
    }
}

/// Decode `path` and fit it within `max_side`, for a thumbnail or a
/// preview. PNG and JPEG only.
pub fn decode(path: &Path, max_side: u32) -> Option<Thumb> {
    let bytes = fs::read(path).ok()?;
    let full = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png(&bytes)?
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        decode_jpeg(&bytes)?
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        decode_webp(&bytes)?
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        decode_gif(&bytes)?
    } else {
        return None;
    };
    Some(fit(&full, max_side))
}

fn decode_png(bytes: &[u8]) -> Option<Thumb> {
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
    Some(Thumb {
        width,
        height,
        rgba,
    })
}

fn decode_webp(bytes: &[u8]) -> Option<Thumb> {
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
    Some(Thumb {
        width,
        height,
        rgba,
    })
}

/// A GIF's first frame, placed on its canvas.
fn decode_gif(bytes: &[u8]) -> Option<Thumb> {
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
    Some(Thumb {
        width,
        height,
        rgba,
    })
}

fn decode_jpeg(bytes: &[u8]) -> Option<Thumb> {
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
    Some(Thumb {
        width: u32::from(info.width),
        height: u32::from(info.height),
        rgba,
    })
}

/// `image` scaled to fit within `max_side`, each new pixel the average of
/// the ones it covers (weighted by alpha, so edges don't darken). Smaller
/// images are left as they are.
pub fn fit(image: &Thumb, max_side: u32) -> Thumb {
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
    Thumb {
        width: nw,
        height: nh,
        rgba: out,
    }
}

/// What a thumbnail says about its file.
#[derive(Default)]
struct Text {
    uri: Option<String>,
    mtime: Option<i64>,
}

fn read_text(path: &Path) -> Option<Text> {
    let file = fs::File::open(path).ok()?;
    let reader = png::Decoder::new(io::BufReader::new(file))
        .read_info()
        .ok()?;
    let mut text = Text::default();
    for chunk in &reader.info().uncompressed_latin1_text {
        match chunk.keyword.as_str() {
            "Thumb::URI" => text.uri = Some(chunk.text.clone()),
            "Thumb::MTime" => text.mtime = chunk.text.parse().ok(),
            _ => {}
        }
    }
    Some(text)
}

/// A cached thumbnail, if it's for this file as it is now.
fn read_cached(path: &Path, uri: &str, mtime: i64) -> Option<Thumb> {
    let bytes = fs::read(path).ok()?;
    let decoder = png::Decoder::new(io::Cursor::new(&bytes));
    let reader = decoder.read_info().ok()?;
    let info = reader.info();
    let mut good_uri = false;
    let mut good_mtime = false;
    for chunk in &info.uncompressed_latin1_text {
        match chunk.keyword.as_str() {
            "Thumb::URI" => good_uri = chunk.text == uri,
            "Thumb::MTime" => good_mtime = chunk.text.parse::<i64>().ok() == Some(mtime),
            _ => {}
        }
    }
    // Other programs may write the text compressed or international.
    for chunk in &info.utf8_text {
        if chunk.keyword == "Thumb::URI" {
            good_uri |= chunk.get_text().is_ok_and(|text| text == uri);
        }
    }
    for chunk in &info.compressed_latin1_text {
        if chunk.keyword == "Thumb::URI" {
            good_uri |= chunk.get_text().is_ok_and(|text| text == uri);
        }
    }
    if !(good_uri && good_mtime) {
        return None;
    }
    decode_png(&bytes)
}

fn write(path: &Path, thumb: &Thumb, uri: &str, mtime: i64, size: u64) -> io::Result<()> {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, thumb.width, thumb.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let text = |e: png::EncodingError| io::Error::other(e.to_string());
    encoder
        .add_text_chunk("Thumb::URI".to_owned(), uri.to_owned())
        .map_err(text)?;
    encoder
        .add_text_chunk("Thumb::MTime".to_owned(), mtime.to_string())
        .map_err(text)?;
    encoder
        .add_text_chunk("Thumb::Size".to_owned(), size.to_string())
        .map_err(text)?;
    encoder
        .add_text_chunk("Software".to_owned(), "enso-files".to_owned())
        .map_err(text)?;
    let mut writer = encoder.write_header().map_err(text)?;
    writer.write_image_data(&thumb.rgba).map_err(text)?;
    writer.finish().map_err(text)?;
    // Thumbnails can show what's private: only the user reads them.
    disk::write_atomic(path, &png, disk::PRIVATE)
}

/// MD5, which the spec names thumbnails by. (Not for anything secret.)
fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64)
        .map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32)
        .collect();
    let mut message = input.to_vec();
    let bits = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_le_bytes());
    let (mut a0, mut b0, mut c0, mut d0) = (
        0x6745_2301u32,
        0xefcd_ab89u32,
        0x98ba_dcfeu32,
        0x1032_5476u32,
    );
    for block in message.as_chunks::<64>().0 {
        let m: Vec<u32> = block
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut digest = [0; 16];
    for (i, word) in [a0, b0, c0, d0].into_iter().enumerate() {
        digest[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_the_reference() {
        assert_eq!(
            enso_core::hex::encode(&md5(b"")),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        assert_eq!(
            enso_core::hex::encode(&md5(b"abc")),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(
            enso_core::hex::encode(&md5(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
        // The spec's own example.
        assert_eq!(
            enso_core::hex::encode(&md5(b"file:///home/jens/photos/me.png")),
            "c6ee772d9e49320e97ec29a7eb5b1697"
        );
    }

    #[test]
    fn webp_and_gif_decode() {
        let root = std::env::temp_dir().join(format!("enso-fs-formats-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        // A 4×2 lossless WebP: red left, blue right.
        let pixels: Vec<u8> = (0..8)
            .flat_map(|i| if i % 4 < 2 { [255, 0, 0] } else { [0, 0, 255] })
            .collect();
        let mut webp = Vec::new();
        image_webp::WebPEncoder::new(&mut webp)
            .encode(&pixels, 4, 2, image_webp::ColorType::Rgb8)
            .unwrap();
        fs::write(root.join("a.webp"), webp).unwrap();
        let decoded = decode(&root.join("a.webp"), 100).unwrap();
        assert_eq!((decoded.width, decoded.height), (4, 2));
        assert_eq!(&decoded.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&decoded.rgba[12..16], &[0, 0, 255, 255]);

        // A 3×3 GIF whose first frame is green; the second isn't looked at.
        let mut gif_bytes = Vec::new();
        {
            let palette = [0, 255, 0, 255, 0, 255];
            let mut encoder = gif::Encoder::new(&mut gif_bytes, 3, 3, &palette).unwrap();
            for index in [0u8, 1] {
                let frame = gif::Frame {
                    width: 3,
                    height: 3,
                    buffer: std::borrow::Cow::Owned(vec![index; 9]),
                    ..gif::Frame::default()
                };
                encoder.write_frame(&frame).unwrap();
            }
        }
        fs::write(root.join("b.gif"), gif_bytes).unwrap();
        let decoded = decode(&root.join("b.gif"), 100).unwrap();
        assert_eq!((decoded.width, decoded.height), (3, 3));
        assert_eq!(&decoded.rgba[..4], &[0, 255, 0, 255]);
        assert!(can_make("Clip.GIF") && can_make("x.webp"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fitting_keeps_the_shape_and_averages() {
        let image = Thumb {
            width: 4,
            height: 2,
            rgba: [[255, 0, 0, 255], [0, 0, 255, 255]].repeat(4).concat(),
        };
        let small = fit(&image, 2);
        assert_eq!((small.width, small.height), (2, 1));
        // Red and blue, averaged.
        assert_eq!(&small.rgba[..4], &[127, 0, 127, 255]);
        assert_eq!(fit(&small, 10), small, "already small enough");
    }

    fn png_file(path: &Path, width: u32, height: u32) {
        let thumb = Thumb {
            width,
            height,
            rgba: vec![200; (width * height * 4) as usize],
        };
        write(path, &thumb, "file:///x", 0, 0).unwrap();
    }

    #[test]
    fn thumbnails_are_made_once_and_kept() {
        let root = std::env::temp_dir().join(format!("enso-fs-thumbs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("pics")).unwrap();
        let cache = root.join("cache");
        let picture = root.join("pics/big.png");
        png_file(&picture, 600, 300);
        let thumb = thumbnail_in(&cache, &picture, Size::Normal).unwrap();
        assert_eq!((thumb.width, thumb.height), (128, 64));
        let uri = uri::from_path(&picture);
        let kept = cache.join("normal").join(format!(
            "{}.png",
            enso_core::hex::encode(&md5(uri.as_bytes()))
        ));
        assert!(kept.exists());
        let text = read_text(&kept).unwrap();
        assert_eq!(text.uri.as_deref(), Some(uri.as_str()));
        // Read back from the cache: the same.
        assert_eq!(thumbnail_in(&cache, &picture, Size::Normal), Some(thumb));
        // A file that isn't a picture fails once, and is remembered.
        let fake = root.join("pics/fake.png");
        fs::write(&fake, "not a png").unwrap();
        assert_eq!(thumbnail_in(&cache, &fake, Size::Normal), None);
        assert!(
            cache
                .join("fail/enso-files")
                .read_dir()
                .unwrap()
                .next()
                .is_some()
        );
        // Nothing for kinds this can't make.
        fs::write(root.join("pics/notes.txt"), "hi").unwrap();
        assert_eq!(
            thumbnail_in(&cache, &root.join("pics/notes.txt"), Size::Normal),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod timing {
    /// `cargo test --release -p enso-fs -- --ignored thumb_timing --nocapture`
    #[test]
    #[ignore = "reads ~/Downloads, writes ~/.cache/thumbnails"]
    fn thumb_timing() {
        for entry in std::fs::read_dir(crate::places::home().join("Downloads"))
            .unwrap()
            .flatten()
        {
            let start = std::time::Instant::now();
            let made = super::thumbnail(&entry.path(), super::Size::Large).is_some();
            let first = start.elapsed();
            let start = std::time::Instant::now();
            super::thumbnail(&entry.path(), super::Size::Large);
            eprintln!(
                "{:?}: made {made} in {first:?}, cached {:?}",
                entry.file_name(),
                start.elapsed()
            );
        }
    }
}
