//! Optional user configuration (`$XDG_CONFIG_HOME/enso/config.toml`).
//!
//! Every key has a default, so no file needs to exist. Shared settings live at
//! the top level; each tool owns one table (`[bar]`, `[launch]`, …) whose shape
//! it defines itself and reads with [`Config::section`].

use std::{fs, io};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Error, palette::Role, paths::Paths};

/// Settings shared by every tool.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Theme id used until one is chosen with `enso-ctl theme set`.
    pub theme: Option<String>,
    pub appearance: Appearance,
    pub motion: MotionConfig,
    /// Per-tool tables, deserialised on demand by their owners.
    #[serde(flatten)]
    sections: toml::Table,
}

/// Look and typography.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    /// Palette role used for active indicators and focus.
    pub accent: Role,
    /// Font family for interface text.
    pub font: String,
    /// Font family for monospaced text: the terminal, and code.
    pub mono_font: String,
    /// Base text size in logical pixels.
    pub font_size: f32,
    /// Whether icons are drawn, or stood in for by a few letters.
    pub icons: IconMode,
    /// Pointer cursor theme, by its directory name under an `icons`
    /// directory, until one is chosen with `enso-ctl cursor set`. Empty
    /// leaves every program's cursor alone.
    pub cursor: String,
    /// Cursor size in logical pixels.
    pub cursor_size: u32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            accent: Role::Rose,
            font: "sans-serif".to_owned(),
            mono_font: "monospace".to_owned(),
            font_size: 14.0,
            icons: IconMode::Auto,
            cursor: String::new(),
            cursor_size: 24,
        }
    }
}

/// How icons are rendered. enso draws its icons itself, so they need no
/// font; `text` is for those who would rather read a word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconMode {
    /// Drawn icons.
    #[default]
    Auto,
    /// Drawn icons: the same as `auto`, kept so older configs still read.
    Glyphs,
    /// A few letters for each icon.
    Text,
}

/// Animation preferences.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MotionConfig {
    /// `false` makes every transition instant.
    pub enabled: bool,
    /// Duration multiplier: `2.0` is twice as slow, `0.5` twice as fast.
    pub speed: f32,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            speed: 1.0,
        }
    }
}

impl Config {
    /// Read the config file. A missing file is not an error.
    pub fn load(paths: &Paths) -> Result<Self, Error> {
        let path = paths.config_file();
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text, &path.display().to_string()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(Error::io(&path, err)),
        }
    }

    /// Parse config TOML; `what` names the source in error messages.
    pub fn parse(text: &str, what: &str) -> Result<Self, Error> {
        toml::from_str(text).map_err(|err| Error::Parse {
            what: what.to_owned(),
            message: err.to_string(),
        })
    }

    /// A tool's own table, or its defaults when the table is absent.
    pub fn section<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, Error> {
        match self.sections.get(name) {
            None => Ok(T::default()),
            Some(value) => value
                .clone()
                .try_into()
                .map_err(|err: toml::de::Error| Error::Parse {
                    what: format!("[{name}] in config.toml"),
                    message: err.to_string(),
                }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir;

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(config.theme.as_deref(), Some(crate::theme::DEFAULT_THEME));
        assert_eq!(config.appearance, Appearance::default());
        assert_eq!(config.motion, MotionConfig::default());
    }

    #[derive(Debug, Default, PartialEq, Deserialize)]
    #[serde(default, deny_unknown_fields)]
    struct Bar {
        height: u32,
    }

    #[test]
    fn missing_file_yields_defaults() {
        let config = Config::load(&Paths::rooted(test_dir("no-config"))).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.appearance.accent, Role::Rose);
        assert!(config.motion.enabled);
        assert_eq!(config.section::<Bar>("bar").unwrap(), Bar::default());
    }

    #[test]
    fn parses_shared_keys_and_tool_sections() {
        let config = Config::parse(
            r#"
            theme = "rose-pine-dawn"

            [appearance]
            accent = "iris"
            icons = "text"

            [motion]
            speed = 0.5

            [bar]
            height = 40
            "#,
            "test",
        )
        .unwrap();
        assert_eq!(config.theme.as_deref(), Some("rose-pine-dawn"));
        assert_eq!(config.appearance.accent, Role::Iris);
        assert_eq!(config.appearance.icons, IconMode::Text);
        assert_eq!(config.appearance.font_size, 14.0);
        assert_eq!(config.motion.speed, 0.5);
        assert_eq!(config.section::<Bar>("bar").unwrap(), Bar { height: 40 });
    }

    #[test]
    fn reports_bad_values_with_their_section() {
        assert!(Config::parse("[appearance]\naccent = \"mauve\"", "test").is_err());
        let config = Config::parse("[bar]\nheight = \"tall\"", "test").unwrap();
        let err = config.section::<Bar>("bar").unwrap_err().to_string();
        assert!(err.contains("[bar]"), "{err}");
    }
}
