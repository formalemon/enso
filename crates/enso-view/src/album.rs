//! The pictures a viewer steps through: those in the folder of the one
//! opened, in the order a file manager lists them.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// What the viewer reads, by name.
const KINDS: [&str; 6] = ["png", "jpg", "jpeg", "jpe", "webp", "gif"];

/// Whether `path` is named like a picture the viewer reads.
pub fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| KINDS.iter().any(|kind| kind.eq_ignore_ascii_case(ext)))
}

/// The pictures of a folder, and which is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Album {
    pub pictures: Vec<PathBuf>,
    pub at: usize,
}

impl Album {
    /// For `path`: a picture, with the others beside it; or a folder, from
    /// its first picture.
    pub fn open(path: &Path) -> Self {
        let (dir, chosen) = if path.is_dir() {
            (path.to_path_buf(), None)
        } else {
            (
                path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf),
                Some(path.to_path_buf()),
            )
        };
        let mut pictures: Vec<PathBuf> = fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| is_picture(path) && path.is_file())
                    .collect()
            })
            .unwrap_or_default();
        pictures.sort_by(|a, b| {
            let name = |path: &PathBuf| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            enso_fs::sort::natural(name(a).as_bytes(), name(b).as_bytes())
        });
        let at = match chosen {
            Some(chosen) => match pictures.iter().position(|path| *path == chosen) {
                Some(at) => at,
                // Opened by a name that doesn't look like a picture: shown
                // anyway, first.
                None => {
                    pictures.insert(0, chosen);
                    0
                }
            },
            None => 0,
        };
        Self { pictures, at }
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.pictures.get(self.at)
    }

    /// The one `step` along, if there is one.
    pub fn neighbour(&self, step: isize) -> Option<&PathBuf> {
        let to = self.at as isize + step;
        usize::try_from(to).ok().and_then(|to| self.pictures.get(to))
    }

    /// Move `step` along, stopping at the ends. Whether it moved.
    pub fn step(&mut self, step: isize) -> bool {
        let last = self.pictures.len().saturating_sub(1) as isize;
        let to = (self.at as isize + step).clamp(0, last.max(0)) as usize;
        let moved = to != self.at;
        self.at = to;
        moved
    }

    /// Go to the first (`false`) or the last (`true`).
    pub fn end(&mut self, last: bool) -> bool {
        let to = if last {
            self.pictures.len().saturating_sub(1)
        } else {
            0
        };
        let moved = to != self.at;
        self.at = to;
        moved
    }

    /// Take the current one out (it was thrown away): the next is shown,
    /// or the one before at the end. Returns it, and where it was.
    pub fn take(&mut self) -> Option<(PathBuf, usize)> {
        if self.at >= self.pictures.len() {
            return None;
        }
        let at = self.at;
        let path = self.pictures.remove(at);
        self.at = at.min(self.pictures.len().saturating_sub(1));
        Some((path, at))
    }

    /// Put one back where it was, and show it.
    pub fn put_back(&mut self, path: PathBuf, at: usize) {
        let at = at.min(self.pictures.len());
        self.pictures.insert(at, path);
        self.at = at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_through_the_pictures_of_a_folder_in_order() {
        let dir = std::env::temp_dir().join(format!("enso-view-album-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in ["b10.png", "b2.JPG", "a.webp", "notes.txt", "c.gif"] {
            fs::write(dir.join(name), b"").unwrap();
        }
        let mut album = Album::open(&dir.join("b2.JPG"));
        let names: Vec<String> = album
            .pictures
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.webp", "b2.JPG", "b10.png", "c.gif"]);
        assert_eq!(album.at, 1);
        assert!(album.step(1));
        assert_eq!(album.current(), Some(&dir.join("b10.png")));
        assert!(album.end(true));
        assert!(!album.step(1), "the end stops");
        // Thrown away at the end: the one before is shown; put back, it is again.
        let (taken, at) = album.take().unwrap();
        assert_eq!(album.current(), Some(&dir.join("b10.png")));
        album.put_back(taken, at);
        assert_eq!(album.current(), Some(&dir.join("c.gif")));
        // A folder opens at its first.
        assert_eq!(Album::open(&dir).at, 0);
        fs::remove_dir_all(dir).unwrap();
    }
}
