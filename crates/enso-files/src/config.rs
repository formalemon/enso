//! The `[files]` table of `config.toml`.

use enso_fs::sort::{By, Sort};
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilesConfig {
    /// Show dotfiles.
    pub hidden: bool,
    /// What folders are ordered by.
    pub sort: SortBy,
    pub descending: bool,
    /// Folders before files, whatever the order says.
    pub folders_first: bool,
    /// Row height.
    pub density: Density,
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            hidden: false,
            sort: SortBy::Name,
            descending: false,
            folders_first: true,
            density: Density::Comfortable,
        }
    }
}

impl FilesConfig {
    pub fn sort(&self) -> Sort {
        Sort {
            by: self.sort.into(),
            descending: self.descending,
            folders_first: self.folders_first,
        }
    }
}

/// [`By`], as configuration spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    Name,
    Size,
    Modified,
    Created,
    Extension,
}

impl From<SortBy> for By {
    fn from(by: SortBy) -> Self {
        match by {
            SortBy::Name => Self::Name,
            SortBy::Size => Self::Size,
            SortBy::Modified => Self::Modified,
            SortBy::Created => Self::Created,
            SortBy::Extension => Self::Extension,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    Comfortable,
    Compact,
}

impl Density {
    /// Height of one row of the listing, in logical pixels.
    pub fn row_height(self) -> f32 {
        match self {
            Self::Comfortable => 32.0,
            Self::Compact => 26.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<FilesConfig>("files").unwrap(),
            FilesConfig::default()
        );
    }

    #[test]
    fn words_become_orders() {
        let config: FilesConfig = toml::from_str("sort = \"modified\"\ndescending = true").unwrap();
        assert_eq!(config.sort().by, By::Modified);
        assert!(config.sort().descending);
        assert!(config.sort().folders_first);
    }
}
