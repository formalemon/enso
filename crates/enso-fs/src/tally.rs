//! How much is in something, all the way down: bytes, files, folders.
//!
//! Links are counted as themselves, not followed, and the count stays on
//! the filesystem it starts on — a folder holding a mount point isn't
//! credited with the whole disk mounted there. It reports as it goes, so a
//! large folder's size can count up on screen.

use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// What's been counted so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub bytes: u64,
    pub files: u64,
    pub folders: u64,
    /// Folders that couldn't be looked into.
    pub unreadable: u64,
}

/// How often progress is reported.
const EVERY: Duration = Duration::from_millis(80);

/// Count everything in `paths` (each counted itself, and a folder all that
/// is inside it), calling `progress` now and then with the count so far.
/// Stops early, with what it has, if `cancel` is set.
pub fn tally(paths: &[PathBuf], cancel: &AtomicBool, mut progress: impl FnMut(&Tally)) -> Tally {
    let mut tally = Tally::default();
    let mut last = Instant::now();
    for path in paths {
        let Ok(meta) = fs::symlink_metadata(path) else {
            continue;
        };
        let device = meta.dev();
        let mut stack = vec![(path.clone(), meta)];
        while let Some((path, meta)) = stack.pop() {
            if cancel.load(Ordering::Relaxed) {
                return tally;
            }
            if meta.is_dir() {
                tally.folders += 1;
                if meta.dev() != device {
                    continue;
                }
                match fs::read_dir(&path) {
                    Ok(entries) => {
                        for entry in entries.flatten() {
                            if let Ok(meta) = entry.metadata() {
                                stack.push((entry.path(), meta));
                            }
                        }
                    }
                    Err(_) => tally.unreadable += 1,
                }
            } else {
                tally.files += 1;
                tally.bytes += meta.len();
            }
            if last.elapsed() >= EVERY {
                progress(&tally);
                last = Instant::now();
            }
        }
    }
    tally
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_all_the_way_down_without_following_links() {
        let root = std::env::temp_dir().join(format!("enso-fs-tally-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/one"), [0; 100]).unwrap();
        fs::write(root.join("a/b/two"), [0; 20]).unwrap();
        fs::write(root.join("three"), [0; 3]).unwrap();
        std::os::unix::fs::symlink("/usr", root.join("a/link")).unwrap();
        let none = AtomicBool::new(false);
        let tally = tally(&[root.join("a"), root.join("three")], &none, |_| {});
        // a, a/b; one, two, the link (its own few bytes), three.
        assert_eq!(tally.folders, 2);
        assert_eq!(tally.files, 4);
        assert_eq!(tally.bytes, 100 + 20 + 3 + "/usr".len() as u64);
        fs::remove_dir_all(root).unwrap();
    }
}
