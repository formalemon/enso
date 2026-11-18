//! How each folder is shown, for the folders where it was changed: a grid
//! for Pictures, newest first in Downloads, dotfiles shown in a dotfiles
//! repository. Everywhere else, the default from Settings.
//!
//! Kept in the state directory (`views.toml`), one table per folder.

use std::{collections::BTreeMap, fs, io, path::Path};

use enso_core::disk;
use serde::{Deserialize, Serialize};

use crate::config::{FilesConfig, SortBy, View};

pub const FILE: &str = "views.toml";
/// Folders remembered at most; past it the file isn't grown.
const MOST: usize = 2000;

/// What was changed for one folder; the rest is the default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct FolderView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<View>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<SortBy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descending: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folders_first: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// The grid's tile size, as an index into its sizes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: Option<usize>,
}

impl FolderView {
    /// `default` with this folder's changes over it.
    pub fn over(&self, default: &FilesConfig) -> FilesConfig {
        FilesConfig {
            view: self.view.unwrap_or(default.view),
            sort: self.sort.unwrap_or(default.sort),
            descending: self.descending.unwrap_or(default.descending),
            folders_first: self.folders_first.unwrap_or(default.folders_first),
            hidden: self.hidden.unwrap_or(default.hidden),
            ..default.clone()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct FolderViews {
    folders: BTreeMap<String, FolderView>,
}

impl FolderViews {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, folder: &Path) -> FolderView {
        folder
            .to_str()
            .and_then(|key| self.folders.get(key))
            .cloned()
            .unwrap_or_default()
    }

    /// Change what's remembered for `folder`. Returns whether it's new.
    pub fn change(&mut self, folder: &Path, change: impl FnOnce(&mut FolderView)) -> bool {
        let Some(key) = folder.to_str() else {
            return false;
        };
        let new = !self.folders.contains_key(key);
        if new && self.folders.len() >= MOST {
            return false;
        }
        change(self.folders.entry(key.to_owned()).or_default());
        new
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string(self).map_err(io::Error::other)?;
        let text = format!("# How enso-files shows the folders where it was changed.\n{text}");
        disk::write_atomic(path, text.as_bytes(), disk::PRIVATE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_keep_their_own_view() {
        let mut views = FolderViews::default();
        assert!(
            views.change(Path::new("/home/me/Pictures"), |view| view.view =
                Some(View::Grid))
        );
        assert!(!views.change(Path::new("/home/me/Pictures"), |view| view.zoom = Some(2)));
        views.change(Path::new("/home/me/Downloads"), |view| {
            view.sort = Some(SortBy::Modified);
            view.descending = Some(true);
        });
        let text = toml::to_string(&views).unwrap();
        let back: FolderViews = toml::from_str(&text).unwrap();
        assert_eq!(back, views);
        let default = FilesConfig::default();
        let pictures = back.get(Path::new("/home/me/Pictures"));
        assert_eq!(pictures.over(&default).view, View::Grid);
        assert_eq!(pictures.zoom, Some(2));
        let elsewhere = back.get(Path::new("/elsewhere")).over(&default);
        assert_eq!(elsewhere, default);
    }
}
