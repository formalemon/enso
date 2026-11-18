//! Data-driven themes: built-ins embedded at compile time plus any `*.toml`
//! dropped into a theme directory.
//!
//! A theme file is just a name, a variant, and a `[colors]` table:
//!
//! ```toml
//! name = "Rosé Pine Moon"
//! variant = "dark"
//!
//! [colors]
//! base = "#232136"
//! # … the other fourteen roles, see `Palette`
//! ```
//!
//! The file stem is the theme id (`rose-pine-moon.toml` → `rose-pine-moon`).

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{Error, palette::Palette, paths::Paths};

include!(concat!(env!("OUT_DIR"), "/builtin_themes.rs"));

/// Theme used when nothing else is configured.
pub const DEFAULT_THEME: &str = "rose-pine-moon";

/// Whether a theme is dark or light; drives the system colour-scheme preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    Dark,
    Light,
}

impl Variant {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// Where a theme was loaded from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Compiled into the binary.
    Builtin,
    /// Read from a theme directory.
    File(PathBuf),
}

/// On-disk shape of a theme file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    variant: Variant,
    colors: Palette,
}

/// A loaded theme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Stable identifier (the file stem), used in config and on the CLI.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    pub variant: Variant,
    pub colors: Palette,
    pub origin: Origin,
}

impl Theme {
    /// Parse theme TOML. `id` is normally the file stem.
    pub fn parse(id: &str, text: &str, origin: Origin) -> Result<Self, Error> {
        let file: ThemeFile = toml::from_str(text).map_err(|err| Error::Parse {
            what: match &origin {
                Origin::Builtin => format!("built-in theme `{id}`"),
                Origin::File(path) => path.display().to_string(),
            },
            message: err.to_string(),
        })?;
        Ok(Self {
            id: id.to_owned(),
            name: file.name,
            variant: file.variant,
            colors: file.colors,
            origin,
        })
    }

    /// Load a theme from a `.toml` file.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| Error::Parse {
                what: path.display().to_string(),
                message: "theme file name is not valid UTF-8".to_owned(),
            })?;
        let text = fs::read_to_string(path).map_err(|source| Error::io(path, source))?;
        Self::parse(id, &text, Origin::File(path.to_owned()))
    }

    pub fn is_dark(&self) -> bool {
        self.variant == Variant::Dark
    }
}

/// Every theme available to this process, keyed and ordered by id.
#[derive(Clone, Debug)]
pub struct Registry {
    themes: BTreeMap<String, Theme>,
    warnings: Vec<String>,
}

impl Registry {
    /// Only the themes compiled into the binary. Always contains [`DEFAULT_THEME`].
    pub fn builtin() -> Self {
        let themes = BUILTIN
            .iter()
            .map(|(id, text)| {
                let theme = Theme::parse(id, text, Origin::Builtin)
                    .unwrap_or_else(|err| panic!("built-in themes are validated by tests: {err}"));
                ((*id).to_owned(), theme)
            })
            .collect();
        Self {
            themes,
            warnings: Vec::new(),
        }
    }

    /// Built-ins plus every `*.toml` in the theme directories. A file with the
    /// same id as a built-in replaces it; unreadable files become warnings.
    pub fn load(paths: &Paths) -> Self {
        let mut registry = Self::builtin();
        for dir in paths.theme_dirs() {
            registry.load_dir(&dir);
        }
        registry
    }

    fn load_dir(&mut self, dir: &Path) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return,
            Err(err) => return self.warnings.push(Error::io(dir, err).to_string()),
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        files.sort();
        for path in files {
            match Theme::load(&path) {
                Ok(theme) => {
                    self.themes.insert(theme.id.clone(), theme);
                }
                Err(err) => self.warnings.push(err.to_string()),
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Theme> {
        self.themes.get(id)
    }

    /// The theme with this id, or the default with a warning if it is unknown.
    pub fn resolve(&self, id: &str) -> (&Theme, Option<String>) {
        match self.get(id) {
            Some(theme) => (theme, None),
            None => (
                self.default_theme(),
                Some(format!(
                    "unknown theme `{id}`, falling back to `{DEFAULT_THEME}`"
                )),
            ),
        }
    }

    pub fn default_theme(&self) -> &Theme {
        self.get(DEFAULT_THEME).expect("default theme is built in")
    }

    /// All themes, ordered by id.
    pub fn iter(&self) -> impl Iterator<Item = &Theme> {
        self.themes.values()
    }

    pub fn len(&self) -> usize {
        self.themes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.themes.is_empty()
    }

    /// Problems hit while scanning theme directories.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// Id recorded by [`set_active`], if any.
pub fn active(paths: &Paths) -> Option<String> {
    let text = fs::read_to_string(paths.active_theme_file()).ok()?;
    let id = text.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// Record `id` as the active theme. Running components pick the change up by
/// watching [`Paths::state_dir`]; the write is atomic so they never see a
/// partial file.
pub fn set_active(paths: &Paths, id: &str) -> Result<(), Error> {
    let file = paths.active_theme_file();
    crate::disk::write_atomic(&file, format!("{id}\n").as_bytes(), crate::disk::SHARED)
        .map_err(|source| Error::io(&file, source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir;

    #[test]
    fn builtins_parse_and_include_default() {
        let registry = Registry::builtin();
        assert!(registry.len() >= 3);
        for id in ["rose-pine-main", "rose-pine-moon", "rose-pine-dawn"] {
            assert!(registry.get(id).is_some(), "missing built-in {id}");
        }
        let default = registry.default_theme();
        assert_eq!(default.id, DEFAULT_THEME);
        assert!(default.is_dark());
        assert_eq!(default.colors.base.to_string(), "#232136");
        assert!(!registry.get("rose-pine-dawn").unwrap().is_dark());
    }

    #[test]
    fn builtin_names_are_distinct() {
        let registry = Registry::builtin();
        let mut names: Vec<&str> = registry.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), registry.len());
    }

    #[test]
    fn user_themes_are_discovered_and_shadow_builtins() {
        let root = test_dir("themes");
        let paths = Paths::rooted(&root);
        let dir = paths.user_themes_dir();
        fs::create_dir_all(&dir).unwrap();

        let moon = BUILTIN
            .iter()
            .find(|(id, _)| *id == DEFAULT_THEME)
            .unwrap()
            .1;
        fs::write(
            dir.join("my-theme.toml"),
            moon.replace("Rosé Pine Moon", "Mine"),
        )
        .unwrap();
        fs::write(
            dir.join("rose-pine-moon.toml"),
            moon.replace("#232136", "#000000"),
        )
        .unwrap();
        fs::write(dir.join("broken.toml"), "name = 3").unwrap();
        fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let registry = Registry::load(&paths);
        assert_eq!(registry.get("my-theme").unwrap().name, "Mine");
        assert_eq!(registry.default_theme().colors.base.to_string(), "#000000");
        assert!(matches!(registry.default_theme().origin, Origin::File(_)));
        assert_eq!(registry.warnings().len(), 1, "{:?}", registry.warnings());
        assert!(registry.warnings()[0].contains("broken.toml"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_incomplete_palettes_and_unknown_keys() {
        let moon = BUILTIN
            .iter()
            .find(|(id, _)| *id == DEFAULT_THEME)
            .unwrap()
            .1;
        let missing = moon.replace("iris = \"#c4a7e7\"\n", "");
        assert!(Theme::parse("x", &missing, Origin::Builtin).is_err());
        let extra = format!("{moon}mauve = \"#ffffff\"\n");
        assert!(Theme::parse("x", &extra, Origin::Builtin).is_err());
    }

    #[test]
    fn active_theme_round_trips() {
        let root = test_dir("active");
        let paths = Paths::rooted(&root);
        assert_eq!(active(&paths), None);
        set_active(&paths, "rose-pine-dawn").unwrap();
        assert_eq!(active(&paths).as_deref(), Some("rose-pine-dawn"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_ids_resolve_to_default() {
        let registry = Registry::builtin();
        let (theme, warning) = registry.resolve("nope");
        assert_eq!(theme.id, DEFAULT_THEME);
        assert!(warning.is_some());
    }
}
