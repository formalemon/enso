//! The `[bar]` table of `config.toml`.

use serde::Deserialize;

/// Which screen edge the bar sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    #[default]
    Top,
    Bottom,
}

/// Bar settings. Every key is optional.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BarConfig {
    /// Bar height in logical pixels.
    pub height: u32,
    pub position: Position,
    /// `strftime`-style clock format.
    pub clock: String,
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            height: 36,
            position: Position::Top,
            clock: DEFAULT_CLOCK.to_owned(),
        }
    }
}

/// Used when the configured clock format is invalid.
pub const DEFAULT_CLOCK: &str = "%a %-d %b  %H:%M";
