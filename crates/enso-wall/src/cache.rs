//! Wallpapers ready to show: a picture already fitted to a screen, kept in
//! the cache directory as raw pixels.
//!
//! A large photograph takes a second or more to decode and scale, and on
//! the first frame of a session that second is a black screen. Read back
//! from here it is one sequential read. The first session after a change of
//! picture, theme or screen pays the decode once; every later one doesn't.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use enso_core::{Rgba, wallpaper::Fit};
use enso_ui::Picture;

const MAGIC: &[u8; 8] = b"ENSOWAL1";
/// Fitted pictures kept: a couple of themes on a couple of screens.
const KEEP: usize = 4;

/// What a fitted picture depends on, spelled out: the file and when it was
/// written, the screen's size, the fit, and the colour around it.
pub fn key(
    path: &Path,
    modified: Option<SystemTime>,
    size: (u32, u32),
    fit: Fit,
    base: Rgba,
) -> String {
    let written = modified
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    format!(
        "{}|{written}|{}x{}|{fit:?}|{}",
        path.display(),
        size.0,
        size.1,
        base.hex()
    )
}

/// The file for `key`: named by its hash, and holding the key itself, so a
/// collision is only a miss.
fn file(dir: &Path, key: &str) -> PathBuf {
    // FNV-1a: a name, not a secret.
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    dir.join(format!("{hash:016x}.raw"))
}

/// The picture kept for `key`, if there is one and it is whole.
pub fn load(dir: &Path, key: &str, size: (u32, u32)) -> Option<Picture> {
    let bytes = fs::read(file(dir, key)).ok()?;
    let rest = bytes.strip_prefix(MAGIC)?;
    let number =
        |at: &[u8]| -> Option<u32> { Some(u32::from_le_bytes(at.get(..4)?.try_into().ok()?)) };
    let key_len = number(rest)? as usize;
    let rest = rest.get(4..)?;
    if rest.get(..key_len)? != key.as_bytes() {
        return None;
    }
    let rest = rest.get(key_len..)?;
    let (width, height) = (number(rest)?, number(rest.get(4..)?)?);
    if (width, height) != size {
        return None;
    }
    let pixels = rest.get(8..)?;
    if pixels.len() != width as usize * height as usize * 4 {
        return None;
    }
    Picture::from_rgba(width, height, pixels.to_vec())
}

/// Keep `picture` for `key`, and let the oldest go beyond the few kept.
pub fn store(dir: &Path, key: &str, picture: &Picture) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let (width, height) = picture.size();
    let mut bytes = Vec::with_capacity(MAGIC.len() + 12 + key.len() + picture.rgba().len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(key.len() as u32).to_le_bytes());
    bytes.extend_from_slice(key.as_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(picture.rgba());
    let path = file(dir, key);
    enso_core::disk::write_atomic(&path, &bytes, 0o600)?;

    let mut kept: Vec<(SystemTime, PathBuf)> = fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "raw"))
        .filter_map(|path| Some((fs::metadata(&path).ok()?.modified().ok()?, path)))
        .collect();
    kept.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    // The one just written always stays, whatever the clock says.
    for (_, old) in kept.into_iter().skip(KEEP) {
        if old != path {
            let _ = fs::remove_file(old);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("enso-wall-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn kept_and_read_back() {
        let dir = scratch("cache");
        let picture = Picture::solid(3, 2, Rgba::rgb(10, 20, 30));
        let key = key(
            Path::new("/pictures/a.png"),
            Some(SystemTime::UNIX_EPOCH),
            (3, 2),
            Fit::Cover,
            Rgba::rgb(1, 2, 3),
        );
        assert!(load(&dir, &key, (3, 2)).is_none());
        store(&dir, &key, &picture).unwrap();
        assert!(load(&dir, &key, (3, 2)).unwrap() == picture);
        // Another size, another key, or a damaged file: a miss, never junk.
        assert!(load(&dir, &key, (2, 3)).is_none());
        assert!(load(&dir, "something else", (3, 2)).is_none());
        let path = file(&dir, &key);
        let mut bytes = fs::read(&path).unwrap();
        bytes.pop();
        fs::write(&path, bytes).unwrap();
        assert!(load(&dir, &key, (3, 2)).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_of_picture_is_a_new_key() {
        let at = |nanos| Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(nanos));
        let base = Rgba::rgb(1, 2, 3);
        let path = Path::new("/p.png");
        let one = key(path, at(1), (4, 4), Fit::Cover, base);
        assert_ne!(one, key(path, at(2), (4, 4), Fit::Cover, base));
        assert_ne!(one, key(path, at(1), (4, 4), Fit::Contain, base));
        assert_ne!(
            one,
            key(path, at(1), (4, 4), Fit::Cover, Rgba::rgb(0, 0, 0))
        );
    }

    #[test]
    fn only_a_few_are_kept() {
        let dir = scratch("prune");
        let picture = Picture::solid(1, 1, Rgba::rgb(0, 0, 0));
        for i in 0..KEEP + 3 {
            store(&dir, &format!("key {i}"), &picture).unwrap();
        }
        let count = fs::read_dir(&dir).unwrap().count();
        assert_eq!(count, KEEP);
        // The newest is among them.
        assert!(load(&dir, &format!("key {}", KEEP + 2), (1, 1)).is_some());
        let _ = fs::remove_dir_all(&dir);
    }
}
