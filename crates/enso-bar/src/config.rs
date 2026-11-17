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
    /// Open the control center (`enso-ctl`, when installed) under the status
    /// island while the pointer rests on it.
    pub control_center: bool,
    /// How solid the islands are, `0.0..=1.0`: a little less than solid lets
    /// the wallpaper show through, just.
    pub opacity: f32,
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            height: 36,
            position: Position::Top,
            clock: DEFAULT_CLOCK.to_owned(),
            control_center: true,
            opacity: 0.9,
        }
    }
}

/// Used when the configured clock format is invalid.
pub const DEFAULT_CLOCK: &str = "%a %-d %b  %H:%M";
