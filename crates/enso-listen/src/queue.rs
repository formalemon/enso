//! What plays: the sound files in the folder of the one opened, in the
//! order a file manager lists them, and what's known of each.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use enso_media::Media;

const KINDS: [&str; 12] = [
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "wma", "aiff", "aif", "alac",
];

/// Whether `path` is named like a sound file.
pub fn is_sound(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| KINDS.iter().any(|kind| kind.eq_ignore_ascii_case(ext)))
}

/// One track, as far as it's known.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub path: PathBuf,
    /// What ffprobe read, once it has.
    pub media: Option<Media>,
}

impl Track {
    /// Its title: as tagged, or its name without the number a ripper put
    /// in front and the extension.
    pub fn title(&self) -> String {
        if let Some(title) = self
            .media
            .as_ref()
            .and_then(|media| media.tags.title.clone())
        {
            return title;
        }
        let stem = self
            .path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let trimmed = stem
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches([' ', '.', '-', '_'])
            .trim();
        if trimmed.is_empty() {
            stem
        } else {
            trimmed.to_owned()
        }
    }

    pub fn artist(&self) -> Option<String> {
        self.media.as_ref()?.tags.artist.clone()
    }

    pub fn album(&self) -> Option<String> {
        self.media.as_ref()?.tags.album.clone()
    }

    pub fn length(&self) -> Option<Duration> {
        self.media.as_ref()?.duration
    }
}

/// The tracks, and which plays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Queue {
    pub tracks: Vec<Track>,
    pub at: usize,
}

impl Queue {
    /// For `path`: a sound file, with the others beside it; or a folder,
    /// from its first.
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
        let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| is_sound(path) && path.is_file())
                    .collect()
            })
            .unwrap_or_default();
        paths.sort_by(|a, b| {
            let name = |path: &PathBuf| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            enso_fs::sort::natural(name(a).as_bytes(), name(b).as_bytes())
        });
        let at = match chosen {
            Some(chosen) => match paths.iter().position(|path| *path == chosen) {
                Some(at) => at,
                None => {
                    paths.insert(0, chosen);
                    0
                }
            },
            None => 0,
        };
        Self {
            tracks: paths
                .into_iter()
                .map(|path| Track { path, media: None })
                .collect(),
            at,
        }
    }

    pub fn current(&self) -> Option<&Track> {
        self.tracks.get(self.at)
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    /// Whether there's one before, and one after.
    pub fn neighbours(&self) -> (bool, bool) {
        (self.at > 0, self.at + 1 < self.tracks.len())
    }

    /// Go to `index`. Whether it moved.
    pub fn go(&mut self, index: usize) -> bool {
        if index >= self.tracks.len() || index == self.at {
            return false;
        }
        self.at = index;
        true
    }

    /// What ffprobe read of `path`.
    pub fn learn(&mut self, path: &Path, media: Media) {
        if let Some(track) = self.tracks.iter_mut().find(|track| track.path == path) {
            track.media = Some(media);
        }
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.tracks.iter().position(|track| track.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_in_order_with_titles_from_names() {
        let dir = std::env::temp_dir().join(format!("enso-listen-queue-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in [
            "10 - Ten.mp3",
            "02 Two.flac",
            "01. One.ogg",
            "cover.jpg",
            "notes.txt",
        ] {
            fs::write(dir.join(name), b"").unwrap();
        }
        let mut queue = Queue::open(&dir.join("02 Two.flac"));
        let titles: Vec<String> = queue.tracks.iter().map(Track::title).collect();
        assert_eq!(titles, ["One", "Two", "Ten"]);
        assert_eq!(queue.at, 1);
        assert_eq!(queue.neighbours(), (true, true));
        assert!(queue.go(2) && !queue.go(2) && !queue.go(3));
        assert_eq!(queue.neighbours(), (true, false));
        fs::remove_dir_all(dir).unwrap();
    }
}
