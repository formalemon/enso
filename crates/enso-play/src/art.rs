//! Covers: a picture on disk (Firefox writes each page's artwork to a
//! file), or one on the web (Spotify's), fetched once and kept in the cache.
//! Each comes with its average colour, which warms the card a touch.

use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use enso_core::Rgba;
use enso_ui::Image;
use sha2::{Digest, Sha256};

/// Longest side a cover is decoded at: it's shown far smaller.
const SIDE: u32 = 192;
/// Nothing a player names is worth reading more than this for.
const MAX_BYTES: u64 = 8 << 20;
const TIMEOUT: Duration = Duration::from_secs(4);

/// A cover, ready to draw.
#[derive(Clone, Debug)]
pub struct Cover {
    pub image: Image,
    /// Its average colour.
    pub tint: Rgba,
}

/// The cover at `url`, or `None` if it can't be had. `cache` is where
/// covers from the web are kept.
pub fn load(url: &str, cache: &Path) -> Option<Cover> {
    let path = if let Some(path) = url.strip_prefix("file://") {
        PathBuf::from(unescape(path))
    } else if url.starts_with('/') {
        PathBuf::from(url)
    } else if url.starts_with("https://") {
        fetched(url, cache)?
    } else {
        return None;
    };
    if !path.metadata().is_ok_and(|meta| meta.len() <= MAX_BYTES) {
        return None;
    }
    let pixels = enso_fs::picture::open(&path, SIDE).ok()?;
    let tint = average(&pixels.rgba);
    let image = Image::from_raw(
        pixels.width,
        pixels.height,
        pixels.width as usize * 4,
        true,
        &pixels.rgba,
    )?;
    Some(Cover { image, tint })
}

/// A web cover's file in the cache, fetched first if it isn't there yet.
fn fetched(url: &str, cache: &Path) -> Option<PathBuf> {
    let name: String = Sha256::digest(url.as_bytes())
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let path = cache.join("covers").join(name);
    if path.exists() {
        return Some(path);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent("enso-play")
        .build()
        .into();
    let response = agent.get(url).call().ok()?;
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    std::fs::create_dir_all(path.parent()?).ok()?;
    enso_core::disk::write_atomic(&path, &bytes, 0o600).ok()?;
    Some(path)
}

/// `%20` and the like, as a file URL spells a path.
fn unescape(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = path.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The average colour of unpremultiplied RGBA pixels, by how opaque each is.
fn average(rgba: &[u8]) -> Rgba {
    let (mut r, mut g, mut b, mut weight) = (0u64, 0u64, 0u64, 0u64);
    for pixel in rgba.as_chunks::<4>().0 {
        let a = u64::from(pixel[3]);
        r += u64::from(pixel[0]) * a;
        g += u64::from(pixel[1]) * a;
        b += u64::from(pixel[2]) * a;
        weight += a;
    }
    let channel = |sum: u64| (sum / weight.max(1)).min(255) as u8;
    Rgba::rgb(channel(r), channel(g), channel(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(unescape("/tmp/My%20Cover.png"), "/tmp/My Cover.png");
        assert_eq!(unescape("/tmp/100%"), "/tmp/100%");
    }

    #[test]
    fn the_average_leaves_out_what_cannot_be_seen() {
        let pixels = [255, 0, 0, 255, 0, 0, 255, 0];
        assert_eq!(average(&pixels), Rgba::rgb(255, 0, 0));
        assert_eq!(average(&[]), Rgba::rgb(0, 0, 0));
    }

    #[test]
    fn what_cannot_be_read_is_no_cover() {
        let cache = std::env::temp_dir();
        assert!(load("file:///nonexistent/cover.png", &cache).is_none());
        assert!(load("data:image/png;base64,AAAA", &cache).is_none());
    }
}
