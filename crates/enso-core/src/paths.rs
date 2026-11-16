//! XDG locations used by the suite. Nothing here has to exist on disk.

use std::{
    env,
    path::{Path, PathBuf},
};

const APP: &str = "enso";

/// Resolved directories for config, state, and shared data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// `$XDG_CONFIG_HOME/enso` — user config and user themes.
    pub config_dir: PathBuf,
    /// `$XDG_STATE_HOME/enso` — state written by the tools (active theme).
    pub state_dir: PathBuf,
    /// `$XDG_CACHE_HOME/enso` — things that can be rebuilt (the launcher's
    /// application index).
    pub cache_dir: PathBuf,
    /// `<dir>/enso` for each of `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`, most specific first.
    pub data_dirs: Vec<PathBuf>,
}

impl Paths {
    /// Resolve from the process environment, following the XDG base directory spec.
    pub fn from_env() -> Self {
        let home = env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let dir = |var: &str, fallback: &str| {
            env::var_os(var)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };

        let mut data_dirs = vec![dir("XDG_DATA_HOME", ".local/share").join(APP)];
        let system = env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
        data_dirs.extend(
            system
                .split(':')
                .map(Path::new)
                .filter(|path| path.is_absolute())
                .map(|p| p.join(APP)),
        );

        Self {
            config_dir: dir("XDG_CONFIG_HOME", ".config").join(APP),
            state_dir: dir("XDG_STATE_HOME", ".local/state").join(APP),
            cache_dir: dir("XDG_CACHE_HOME", ".cache").join(APP),
            data_dirs,
        }
    }

    /// Everything under one directory. Handy for tests and sandboxes.
    pub fn rooted(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            cache_dir: root.join("cache"),
            data_dirs: vec![root.join("data")],
        }
    }

    /// `config.toml` — optional; every key has a default.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// Where users drop their own `*.toml` themes.
    pub fn user_themes_dir(&self) -> PathBuf {
        self.config_dir.join("themes")
    }

    /// Theme directories from lowest to highest priority (later ones win).
    pub fn theme_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self
            .data_dirs
            .iter()
            .rev()
            .map(|d| d.join("themes"))
            .collect();
        dirs.push(self.user_themes_dir());
        dirs
    }

    /// File holding the id of the theme chosen with `enso-ctl theme set`.
    pub fn active_theme_file(&self) -> PathBuf {
        self.state_dir.join("theme")
    }

    /// File holding the wallpapers chosen with `enso-wall set`.
    pub fn wallpapers_file(&self) -> PathBuf {
        self.state_dir.join("wallpapers.toml")
    }
}
