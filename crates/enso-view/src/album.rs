//! What a viewer steps through: the pictures and videos in the folder of
//! the one opened, in the order a file manager lists them.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// What the viewer reads, by name.
const PICTURES: [&str; 6] = ["png", "jpg", "jpeg", "jpe", "webp", "gif"];
const VIDEOS: [&str; 8] = ["mp4", "m4v", "mov", "mkv", "webm", "avi", "ogv", "3gp"];

fn named(path: &Path, kinds: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| kinds.iter().any(|kind| kind.eq_ignore_ascii_case(ext)))
}

/// Whether `path` is named like a picture the viewer reads.
pub fn is_picture(path: &Path) -> bool {
    named(path, &PICTURES)
}

/// Whether `path` is named like a video.
pub fn is_video(path: &Path) -> bool {
    named(path, &VIDEOS)
}

/// The pictures and videos of a folder, and which is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Album {
    pub items: Vec<PathBuf>,
    pub at: usize,
}

impl Album {
    /// For `path`: a picture or a video, with the others beside it; or a
    /// folder, from its first.
    pub fn open(path: &Path) -> Self {
        let (dir, chosen) = if path.is_dir() {
            (path.to_path_buf(), None)
        } else {
            (
                path.parent()
                    .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
                Some(path.to_path_buf()),
            )
        };
        let mut items: Vec<PathBuf> = fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| (is_picture(path) || is_video(path)) && path.is_file())
                    .collect()
            })
            .unwrap_or_default();
        items.sort_by(|a, b| {
            let name = |path: &PathBuf| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            enso_fs::sort::natural(name(a).as_bytes(), name(b).as_bytes())
        });
        let at = match chosen {
            Some(chosen) => match items.iter().position(|path| *path == chosen) {
                Some(at) => at,
                // Opened by a name that doesn't look like either: shown
                // anyway, first.
                None => {
                    items.insert(0, chosen);
                    0
                }
            },
            None => 0,
        };
        Self { items, at }
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.items.get(self.at)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// The one `step` along, if there is one.
    pub fn neighbour(&self, step: isize) -> Option<&PathBuf> {
        let to = self.at as isize + step;
        usize::try_from(to).ok().and_then(|to| self.items.get(to))
    }

    /// Whether there is one before, and one after.
    pub fn neighbours(&self) -> (bool, bool) {
        (self.at > 0, self.at + 1 < self.items.len())
    }

    /// Move `step` along, stopping at the ends. Whether it moved.
    pub fn step(&mut self, step: isize) -> bool {
        let last = self.items.len().saturating_sub(1) as isize;
        let to = (self.at as isize + step).clamp(0, last.max(0)) as usize;
        let moved = to != self.at;
        self.at = to;
        moved
    }

    /// Go to the one at `index`. Whether it moved.
    pub fn go(&mut self, index: usize) -> bool {
        if index >= self.items.len() || index == self.at {
            return false;
        }
        self.at = index;
        true
    }

    /// Take the current one out (it was thrown away): the next is shown,
    /// or the one before at the end. Returns it, and where it was.
    pub fn take(&mut self) -> Option<(PathBuf, usize)> {
        if self.at >= self.items.len() {
            return None;
        }
        let at = self.at;
        let path = self.items.remove(at);
        self.at = at.min(self.items.len().saturating_sub(1));
        Some((path, at))
    }

    /// Put one back where it was, and show it.
    pub fn put_back(&mut self, path: PathBuf, at: usize) {
        let at = at.min(self.items.len());
        self.items.insert(at, path);
        self.at = at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_through_the_pictures_and_videos_of_a_folder_in_order() {
        let dir = std::env::temp_dir().join(format!("enso-view-album-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in [
            "b10.png",
            "b2.JPG",
            "a.webp",
            "notes.txt",
            "c.gif",
            "clip.MP4",
            "song.mp3",
        ] {
            fs::write(dir.join(name), b"").unwrap();
        }
        let mut album = Album::open(&dir.join("b2.JPG"));
        let names: Vec<String> = album
            .items
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.webp", "b2.JPG", "b10.png", "c.gif", "clip.MP4"]);
        assert_eq!(album.at, 1);
        assert_eq!(album.neighbours(), (true, true));
        assert!(album.step(1));
        assert_eq!(album.current(), Some(&dir.join("b10.png")));
        assert!(album.go(4));
        assert!(is_video(album.current().unwrap()));
        assert!(!album.step(1), "the end stops");
        assert!(album.go(0) && !album.go(0) && !album.go(9));
        album.go(4);
        // Thrown away at the end: the one before is shown; put back, it is again.
        let (taken, at) = album.take().unwrap();
        assert_eq!(album.current(), Some(&dir.join("c.gif")));
        album.put_back(taken, at);
        assert_eq!(album.current(), Some(&dir.join("clip.MP4")));
        // A folder opens at its first.
        assert_eq!(Album::open(&dir).at, 0);
        fs::remove_dir_all(dir).unwrap();
    }
}
