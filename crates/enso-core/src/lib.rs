//! Shared foundation of the enso desktop suite: palette, themes, config, and
//! the motion and style tokens that keep every tool looking like one family.
//!
//! This crate is deliberately tiny (serde + toml) and knows nothing about
//! Wayland, so CLI paths and future non-GUI tools can depend on it freely.
//! Nothing here requires any file to exist on disk: themes are embedded and
//! every config key has a default.
//!
//! ```
//! let ctx = enso_core::Context::load();
//! let accent = ctx.accent();
//! println!("{} — accent {accent}", ctx.theme.name);
//! ```

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod color;
pub mod config;
pub mod motion;
pub mod notices;
pub mod palette;
pub mod paths;
pub mod style;
pub mod theme;

use std::{fmt, io, path::Path, path::PathBuf};

pub use color::Rgba;
pub use config::Config;
pub use motion::{Easing, Lerp, Motion, Transition, Tween};
pub use notices::Notices;
pub use palette::{Palette, Role};
pub use paths::Paths;
pub use style::Icon;
pub use theme::{Registry, Theme, Variant};

/// Everything a tool needs at startup, resolved in one call.
#[derive(Clone, Debug)]
pub struct Context {
    pub paths: Paths,
    pub config: Config,
    pub themes: Registry,
    /// The active theme.
    pub theme: Theme,
    pub motion: Motion,
    /// Non-fatal problems found while loading (bad config, broken theme files).
    pub warnings: Vec<String>,
}

impl Context {
    /// Load config and themes from the standard locations.
    ///
    /// Never fails: anything unreadable falls back to defaults and is reported
    /// in [`Context::warnings`], so a typo in a config file cannot take the
    /// desktop down.
    pub fn load() -> Self {
        Self::load_from(Paths::from_env())
    }

    /// Like [`Context::load`] with explicit locations.
    pub fn load_from(paths: Paths) -> Self {
        let mut warnings = Vec::new();
        let config = Config::load(&paths).unwrap_or_else(|err| {
            warnings.push(format!("{err}; using default settings"));
            Config::default()
        });
        let themes = Registry::load(&paths);
        warnings.extend_from_slice(themes.warnings());

        let (theme, warning) = themes.resolve(&active_theme_id(&paths, &config));
        let theme = theme.clone();
        warnings.extend(warning);

        let motion = Motion::new(config.motion);
        Self {
            paths,
            config,
            themes,
            theme,
            motion,
            warnings,
        }
    }

    /// Re-read the active theme (and theme files). Returns whether it changed.
    pub fn reload_theme(&mut self) -> bool {
        self.themes = Registry::load(&self.paths);
        let theme = self
            .themes
            .resolve(&active_theme_id(&self.paths, &self.config))
            .0
            .clone();
        let changed = theme != self.theme;
        self.theme = theme;
        changed
    }

    /// The configured accent colour in the active theme.
    pub fn accent(&self) -> Rgba {
        self.theme.colors.get(self.config.appearance.accent)
    }

    /// A tool's config table, falling back to defaults (with a warning) if it is invalid.
    pub fn section<T: serde::de::DeserializeOwned + Default>(&mut self, name: &str) -> T {
        self.config.section(name).unwrap_or_else(|err| {
            self.warnings.push(format!("{err}; using defaults"));
            T::default()
        })
    }

    /// Print collected warnings to stderr, prefixed with the tool's name.
    pub fn report_warnings(&self, tool: &str) {
        for warning in &self.warnings {
            eprintln!("{tool}: warning: {warning}");
        }
    }
}

/// Id of the theme that should be active: the one chosen with
/// `enso-ctl theme set`, else `theme` from the config file, else the default.
pub fn active_theme_id(paths: &Paths, config: &Config) -> String {
    theme::active(paths)
        .or_else(|| config.theme.clone())
        .unwrap_or_else(|| theme::DEFAULT_THEME.to_owned())
}

/// Errors from loading themes and configuration.
#[derive(Debug)]
pub enum Error {
    Io { path: PathBuf, source: io::Error },
    Parse { what: String, message: String },
    InvalidColor(String),
    UnknownRole(String),
    UnknownTheme(String),
}

impl Error {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse { what, message } => write!(f, "{what}: {}", message.trim_end()),
            Self::InvalidColor(value) => {
                write!(
                    f,
                    "invalid colour `{value}` (expected #rgb, #rrggbb, or #rrggbbaa)"
                )
            }
            Self::UnknownRole(name) => write!(f, "unknown palette role `{name}`"),
            Self::UnknownTheme(id) => write!(f, "unknown theme `{id}`"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A fresh, empty scratch directory for a test.
#[cfg(test)]
pub(crate) fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("enso-core-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn loads_with_nothing_on_disk() {
        let ctx = Context::load_from(Paths::rooted(test_dir("empty")));
        assert_eq!(ctx.theme.id, theme::DEFAULT_THEME);
        assert!(ctx.warnings.is_empty(), "{:?}", ctx.warnings);
        assert_eq!(ctx.accent(), ctx.theme.colors.rose);
        assert!(ctx.motion.is_enabled());
    }

    #[test]
    fn chosen_theme_overrides_config_and_reloads_live() {
        let root = test_dir("precedence");
        let paths = Paths::rooted(&root);
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(paths.config_file(), "theme = \"rose-pine-main\"\n").unwrap();

        let mut ctx = Context::load_from(paths.clone());
        assert_eq!(ctx.theme.id, "rose-pine-main");

        theme::set_active(&paths, "rose-pine-dawn").unwrap();
        assert!(ctx.reload_theme());
        assert_eq!(ctx.theme.id, "rose-pine-dawn");
        assert!(!ctx.reload_theme());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn broken_config_degrades_to_defaults_with_a_warning() {
        let root = test_dir("broken");
        let paths = Paths::rooted(&root);
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(paths.config_file(), "theme = [").unwrap();
        theme::set_active(&paths, "gone").unwrap();

        let ctx = Context::load_from(paths);
        assert_eq!(ctx.theme.id, theme::DEFAULT_THEME);
        assert_eq!(ctx.warnings.len(), 2, "{:?}", ctx.warnings);
        fs::remove_dir_all(root).unwrap();
    }
}
