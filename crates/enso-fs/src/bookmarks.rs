//! Bookmarked folders, in GTK's file (`~/.config/gtk-3.0/bookmarks`), so
//! the same list shows in GTK's dialogs, Nautilus, and here.
//!
//! One bookmark a line: a URI, then optionally a space and a label.
//! Bookmarks that aren't local folders (`sftp://…`) are kept as they are
//! but not offered here.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use enso_core::disk;

use crate::uri;

/// One line of the file.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    uri: String,
    label: Option<String>,
}

/// A bookmarked folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub path: PathBuf,
    /// The name given to it, if not its own.
    pub label: Option<String>,
}

impl Bookmark {
    /// What to call it: its label, else the folder's name.
    pub fn name(&self) -> String {
        self.label.clone().unwrap_or_else(|| {
            self.path.file_name().map_or_else(
                || "/".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        })
    }
}

/// The bookmarks file, as read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bookmarks {
    lines: Vec<Line>,
}

/// Where GTK keeps them.
pub fn file() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| crate::places::home().join(".config"));
    config.join("gtk-3.0").join("bookmarks")
}

impl Bookmarks {
    /// Read them; no file is no bookmarks.
    pub fn load(path: &Path) -> Self {
        Self::parse(&fs::read_to_string(path).unwrap_or_default())
    }

    pub fn parse(text: &str) -> Self {
        let lines = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| match line.split_once(' ') {
                Some((uri, label)) if !label.trim().is_empty() => Line {
                    uri: uri.to_owned(),
                    label: Some(label.trim().to_owned()),
                },
                _ => Line {
                    uri: line.split(' ').next().unwrap_or(line).to_owned(),
                    label: None,
                },
            })
            .collect();
        Self { lines }
    }

    /// The local folders, in order.
    pub fn folders(&self) -> Vec<Bookmark> {
        self.lines
            .iter()
            .filter_map(|line| {
                Some(Bookmark {
                    path: uri::to_path(&line.uri)?,
                    label: line.label.clone(),
                })
            })
            .collect()
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.position(path).is_some()
    }

    fn position(&self, path: &Path) -> Option<usize> {
        self.lines
            .iter()
            .position(|line| uri::to_path(&line.uri).as_deref() == Some(path))
    }

    /// Bookmark `path`, at the end. Returns false if it already was.
    pub fn add(&mut self, path: &Path) -> bool {
        if self.contains(path) {
            return false;
        }
        self.lines.push(Line {
            uri: uri::from_path(path),
            label: None,
        });
        true
    }

    /// Returns false if it wasn't bookmarked.
    pub fn remove(&mut self, path: &Path) -> bool {
        match self.position(path) {
            Some(at) => {
                self.lines.remove(at);
                true
            }
            None => false,
        }
    }

    /// Move `path` one place up (`-1`) or down (`1`) among all bookmarks.
    pub fn shift(&mut self, path: &Path, step: isize) -> bool {
        let Some(at) = self.position(path) else {
            return false;
        };
        let Some(to) = at
            .checked_add_signed(step)
            .filter(|&to| to < self.lines.len())
        else {
            return false;
        };
        self.lines.swap(at, to);
        true
    }

    pub fn to_text(&self) -> String {
        self.lines
            .iter()
            .map(|line| match &line.label {
                Some(label) => format!("{} {label}\n", line.uri),
                None => format!("{}\n", line.uri),
            })
            .collect()
    }

    /// Write them back, whole or not at all.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        disk::write_atomic(path, self.to_text().as_bytes(), disk::SHARED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_change_and_write_back() {
        let text =
            "file:///home/kai/dev\nfile:///home/kai/My%20Notes Notes\nsftp://kiri/srv Server\n";
        let mut marks = Bookmarks::parse(text);
        let folders = marks.folders();
        assert_eq!(folders.len(), 2, "the server stays, unoffered");
        assert_eq!(folders[1].path, Path::new("/home/kai/My Notes"));
        assert_eq!(folders[1].name(), "Notes");
        assert_eq!(folders[0].name(), "dev");
        assert_eq!(marks.to_text(), text, "untouched, it reads back the same");

        assert!(!marks.add(Path::new("/home/kai/dev")));
        assert!(marks.add(Path::new("/tmp/a b")));
        assert!(marks.shift(Path::new("/tmp/a b"), -1));
        assert!(
            !marks.shift(Path::new("/home/kai/dev"), -1),
            "already first"
        );
        assert!(marks.remove(Path::new("/home/kai/dev")));
        assert_eq!(
            marks.to_text(),
            "file:///home/kai/My%20Notes Notes\nfile:///tmp/a%20b\nsftp://kiri/srv Server\n"
        );
    }
}
