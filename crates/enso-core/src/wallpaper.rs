//! Which picture goes behind everything: one per theme, with a default to
//! fall back on.
//!
//! The choice comes from two places, the same way the theme does. The
//! `[wall]` table of `config.toml` declares it:
//!
//! ```toml
//! [wall]
//! default = "~/Pictures/walls/quiet.jpg"
//!
//! [wall.themes]
//! rose-pine-dawn = "~/Pictures/walls/morning.png"
//! ```
//!
//! and `enso-wall set` records it in a state file, which wins — so a
//! wallpaper can be changed from a key binding without editing anything.
//! For a theme, the first of these that names a file is used: the theme's
//! own picture (state, then config), then the default (state, then config).
//! When none does there is no picture, and the theme's base colour stands in.
//!
//! This lives in `enso-core` because more than one tool asks the question:
//! `enso-wall` to draw the wallpaper, `enso-lock` to blur it behind the
//! password field.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{Error, paths::Paths};

/// How a picture is fitted to a screen of another shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    /// Fill the screen, cropping what does not fit.
    #[default]
    Cover,
    /// Show the whole picture, with the theme's base colour around it.
    Contain,
}

/// The `[wall]` table of `config.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WallConfig {
    /// Picture for themes that have none of their own.
    pub default: Option<String>,
    /// Picture per theme id.
    pub themes: BTreeMap<String, String>,
    pub fit: Fit,
}

/// Choices made with `enso-wall set`, kept in the state directory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chosen {
    pub default: Option<String>,
    pub themes: BTreeMap<String, String>,
}

impl Chosen {
    /// Read the state file. Missing or unreadable means nothing was chosen.
    pub fn load(paths: &Paths) -> Self {
        fs::read_to_string(paths.wallpapers_file())
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Write the state file. Running tools pick the change up by watching
    /// [`Paths::state_dir`]; the write is atomic so they never see half of it.
    pub fn save(&self, paths: &Paths) -> Result<(), Error> {
        let file = paths.wallpapers_file();
        let text = toml::to_string(self).map_err(|err| Error::Parse {
            what: file.display().to_string(),
            message: err.to_string(),
        })?;
        crate::disk::write_atomic(&file, text.as_bytes(), crate::disk::SHARED)
            .map_err(|source| Error::io(&file, source))
    }

    /// Choose `picture` for `theme`, or as the default with `None`.
    pub fn set(&mut self, theme: Option<&str>, picture: &Path) {
        let picture = picture.to_string_lossy().into_owned();
        match theme {
            Some(theme) => {
                self.themes.insert(theme.to_owned(), picture);
            }
            None => self.default = Some(picture),
        }
    }

    /// Forget the choice for `theme`, or the default with `None`. Returns
    /// whether there was one.
    pub fn unset(&mut self, theme: Option<&str>) -> bool {
        match theme {
            Some(theme) => self.themes.remove(theme).is_some(),
            None => self.default.take().is_some(),
        }
    }
}

/// Where a theme's picture was decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The theme's own picture.
    Theme,
    /// The fallback shared by themes without one.
    Default,
}

/// The picture for the theme `theme`, and whether it is the theme's own.
/// `None` when nothing is set, or when what is set is not a file — a
/// wallpaper on an unplugged drive falls through to the next choice rather
/// than leaving the screen blank.
pub fn resolve(config: &WallConfig, chosen: &Chosen, theme: &str) -> Option<(PathBuf, Source)> {
    let own = [chosen.themes.get(theme), config.themes.get(theme)];
    let default = [chosen.default.as_ref(), config.default.as_ref()];
    let pick = |candidates: [Option<&String>; 2], source| {
        candidates
            .into_iter()
            .flatten()
            .map(|path| expand(path))
            .find(|path| path.is_file())
            .map(|path| (path, source))
    };
    pick(own, Source::Theme).or_else(|| pick(default, Source::Default))
}

/// A path as written in config: `~` and `~/…` mean the home directory.
pub fn expand(path: &str) -> PathBuf {
    let home = || env::var_os("HOME").map(PathBuf::from);
    match path.strip_prefix('~') {
        Some("") => home().unwrap_or_else(|| PathBuf::from(path)),
        Some(rest) if rest.starts_with('/') => home().map_or_else(
            || PathBuf::from(path),
            |home| home.join(rest.trim_start_matches('/')),
        ),
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir;

    #[test]
    fn a_theme_picture_wins_over_the_default_and_state_over_config() {
        let root = test_dir("wallpaper");
        fs::create_dir_all(&root).unwrap();
        let file = |name: &str| {
            let path = root.join(name);
            fs::write(&path, "").unwrap();
            path.to_string_lossy().into_owned()
        };
        let config = WallConfig {
            default: Some(file("default.png")),
            themes: BTreeMap::from([("dawn".to_owned(), file("dawn.png"))]),
            fit: Fit::Cover,
        };
        let mut chosen = Chosen::default();
        let name = |theme: &str, chosen: &Chosen| {
            resolve(&config, chosen, theme).map(|(path, source)| {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                (name, source)
            })
        };

        assert_eq!(
            name("dawn", &chosen),
            Some(("dawn.png".to_owned(), Source::Theme))
        );
        assert_eq!(
            name("moon", &chosen),
            Some(("default.png".to_owned(), Source::Default))
        );

        // A choice made with `enso-wall set` wins over the config file…
        chosen.set(Some("dawn"), Path::new(&file("chosen.png")));
        assert_eq!(name("dawn", &chosen).unwrap().0, "chosen.png");
        // …but a theme's configured picture still beats a chosen default.
        chosen.unset(Some("dawn"));
        chosen.set(None, Path::new(&file("other.png")));
        assert_eq!(name("dawn", &chosen).unwrap().0, "dawn.png");
        assert_eq!(name("moon", &chosen).unwrap().0, "other.png");

        // A file that has gone falls through to the next choice.
        chosen.set(Some("moon"), &root.join("missing.png"));
        assert_eq!(name("moon", &chosen).unwrap().0, "other.png");

        // Nothing set anywhere: no picture.
        assert_eq!(
            resolve(&WallConfig::default(), &Chosen::default(), "moon"),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn choices_survive_a_round_trip_through_the_state_file() {
        let root = test_dir("wallpaper-state");
        let paths = Paths::rooted(&root);
        assert_eq!(Chosen::load(&paths), Chosen::default());

        let mut chosen = Chosen::default();
        chosen.set(None, Path::new("/walls/default.jpg"));
        chosen.set(Some("rose-pine-dawn"), Path::new("/walls/dawn.png"));
        chosen.save(&paths).unwrap();
        assert_eq!(Chosen::load(&paths), chosen);

        assert!(chosen.unset(None));
        assert!(!chosen.unset(Some("rose-pine-moon")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tilde_means_home() {
        let home = env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return;
        }
        assert_eq!(expand("~"), PathBuf::from(&home));
        assert_eq!(expand("~/a/b.png"), Path::new(&home).join("a/b.png"));
        assert_eq!(expand("/abs/~/b.png"), PathBuf::from("/abs/~/b.png"));
        assert_eq!(expand("~user/b.png"), PathBuf::from("~user/b.png"));
    }
}
