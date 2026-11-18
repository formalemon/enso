//! What's changed in the window's settings, over `config.toml`.
//!
//! `config.toml`'s `[files]` table is the hand-written default; the
//! settings panel writes what it changes to `files.toml` in the state
//! directory, and that wins — the way `enso-ctl theme set` overrides the
//! configured theme. A setting not changed in the window has no line
//! there, so `config.toml` still decides it; "Reset" takes the line away.
//!
//! The answer to "use version control?" lives here too: `None` until the
//! question has been asked.

use std::{fs, io, path::Path};

use enso_core::disk;
use serde::{Deserialize, Serialize};

use crate::config::{Density, FilesConfig, SortBy, View};

/// File name inside the state directory.
pub const FILE: &str = "files.toml";

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Overrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<SortBy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descending: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folders_first: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub density: Option<Density>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<View>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bulk_rename: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_client_id: Option<String>,
    /// The GitHub account signed in to (its token is in the keyring).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_account: Option<String>,
    /// Version control: on, off, or not asked yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<bool>,
}

impl Overrides {
    /// What's in `path`; nothing if it's missing or doesn't read.
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string(self).map_err(io::Error::other)?;
        let text = format!(
            "# Written by enso-files' settings; config.toml's [files] is the default under it.\n{text}"
        );
        disk::write_atomic(path, text.as_bytes(), disk::SHARED)
    }

    /// `config` with these changes over it.
    pub fn over(&self, config: &FilesConfig) -> FilesConfig {
        FilesConfig {
            hidden: self.hidden.unwrap_or(config.hidden),
            sort: self.sort.unwrap_or(config.sort),
            descending: self.descending.unwrap_or(config.descending),
            folders_first: self.folders_first.unwrap_or(config.folders_first),
            density: self.density.unwrap_or(config.density),
            view: self.view.unwrap_or(config.view),
            bulk_rename: self.bulk_rename.unwrap_or(config.bulk_rename),
            github_client_id: self
                .github_client_id
                .clone()
                .unwrap_or_else(|| config.github_client_id.clone()),
        }
    }

    /// Whether version control is on (off until asked and answered).
    pub fn git(&self) -> bool {
        self.git == Some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_win_and_the_rest_comes_from_the_config() {
        let config = FilesConfig {
            hidden: true,
            ..FilesConfig::default()
        };
        let overrides = Overrides {
            sort: Some(SortBy::Modified),
            ..Overrides::default()
        };
        let effective = overrides.over(&config);
        assert!(effective.hidden, "unchanged: from config.toml");
        assert_eq!(effective.sort, SortBy::Modified);
        assert!(!overrides.git());
    }

    #[test]
    fn only_what_changed_is_written() {
        let root = std::env::temp_dir().join(format!("enso-files-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let path = root.join(FILE);
        assert_eq!(Overrides::load(&path), Overrides::default());
        let overrides = Overrides {
            density: Some(Density::Compact),
            git: Some(false),
            ..Overrides::default()
        };
        overrides.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("density = \"compact\""), "{text}");
        assert!(text.contains("git = false"), "{text}");
        assert!(!text.contains("hidden"), "{text}");
        assert_eq!(Overrides::load(&path), overrides);
        fs::remove_dir_all(root).unwrap();
    }
}
