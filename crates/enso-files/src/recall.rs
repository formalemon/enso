//! Where each application last picked from, so its next "choose a file"
//! opens there.
//!
//! One small text file in the state directory, a line per application:
//! its id, a tab, the folder. Losing it loses nothing but the convenience.

use enso_core::disk;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

/// File name inside the state directory.
pub const FILE: &str = "files-picker-folders";
/// Applications remembered, at most: the most recent survive.
const LIMIT: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recall {
    /// Application id → folder, and when, as a sequence number.
    folders: BTreeMap<String, (u64, PathBuf)>,
}

impl Recall {
    pub fn load(path: &Path) -> Self {
        let text = fs::read_to_string(path).unwrap_or_default();
        let folders = text
            .lines()
            .enumerate()
            .filter_map(|(at, line)| {
                let (app, folder) = line.split_once('\t')?;
                (!app.is_empty() && folder.starts_with('/'))
                    .then(|| (app.to_owned(), (at as u64, PathBuf::from(folder))))
            })
            .collect();
        Self { folders }
    }

    /// The folder `app` last picked from, if it still exists.
    pub fn folder(&self, app: &str) -> Option<&Path> {
        self.folders
            .get(app)
            .map(|(_, folder)| folder.as_path())
            .filter(|folder| folder.is_dir())
    }

    pub fn remember(&mut self, app: &str, folder: &Path) {
        if app.is_empty() {
            return;
        }
        let next = self
            .folders
            .values()
            .map(|(at, _)| at + 1)
            .max()
            .unwrap_or(0);
        self.folders
            .insert(app.to_owned(), (next, folder.to_owned()));
        if self.folders.len() > LIMIT
            && let Some(oldest) = self
                .folders
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(app, _)| app.clone())
        {
            self.folders.remove(&oldest);
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut entries: Vec<_> = self.folders.iter().collect();
        entries.sort_by_key(|(_, (at, _))| *at);
        let text: String = entries
            .into_iter()
            // A tab or newline in either would break the line apart.
            .filter(|(app, (_, folder))| {
                let folder = folder.to_string_lossy();
                !app.contains(['\t', '\n']) && !folder.contains(['\t', '\n'])
            })
            .map(|(app, (_, folder))| format!("{app}\t{}\n", folder.to_string_lossy()))
            .collect();
        disk::write_atomic(path, text.as_bytes(), disk::PRIVATE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_remembered_per_application() {
        let root = std::env::temp_dir().join(format!("enso-files-recall-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let file = root.join(FILE);
        let mut recall = Recall::load(&file);
        assert_eq!(recall.folder("org.mozilla.firefox"), None);
        recall.remember("org.mozilla.firefox", Path::new("/tmp"));
        recall.remember("", Path::new("/"));
        recall.save(&file).unwrap();
        let recall = Recall::load(&file);
        assert_eq!(
            recall.folder("org.mozilla.firefox"),
            Some(Path::new("/tmp"))
        );
        assert_eq!(recall.folders.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gone_folders_are_forgotten_and_old_apps_make_room() {
        let mut recall = Recall::default();
        recall.remember("a", Path::new("/nonexistent/enso"));
        assert_eq!(recall.folder("a"), None);
        for i in 0..=LIMIT {
            recall.remember(&format!("app{i}"), Path::new("/"));
        }
        assert_eq!(recall.folders.len(), LIMIT);
        assert!(!recall.folders.contains_key("a"), "the oldest went");
    }
}
