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
    /// island while the pointer rests on it. A click opens it either way.
    pub control_center: bool,
    /// With `enso-play` installed: the clock turns to sea while something plays,
    /// what's playing under it while the pointer rests there, and a click on
    /// it to play or pause.
    pub now_playing: bool,
    /// The month under the clock while the pointer rests there and nothing
    /// is playing.
    pub calendar: bool,
    /// How solid the islands are, `0.0..=1.0`: a little less than solid lets
    /// the wallpaper show through, just.
    pub opacity: f32,
    /// The screens to have a bar, by name (`eDP-1`); empty: every screen.
    pub outputs: Vec<String>,
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            height: 36,
            position: Position::Top,
            clock: DEFAULT_CLOCK.to_owned(),
            control_center: true,
            now_playing: true,
            calendar: true,
            opacity: 0.9,
            outputs: Vec::new(),
        }
    }
}

/// Used when the configured clock format is invalid.
pub const DEFAULT_CLOCK: &str = "%a %-d %b  %H:%M";
