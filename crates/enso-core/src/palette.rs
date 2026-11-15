//! The fifteen Rosé Pine colour roles every theme must define.
//!
//! Themes are not limited to Rosé Pine colours — any palette works as long as
//! it fills these roles, which is what lets every component stay theme-agnostic.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{Error, color::Rgba};

/// A complete set of theme colours, keyed by role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Palette {
    /// Primary background (bars, backdrops).
    pub base: Rgba,
    /// Raised background (cards, panels, toasts).
    pub surface: Rgba,
    /// Topmost background (popovers, hovered rows).
    pub overlay: Rgba,
    /// Lowest-contrast foreground (disabled, placeholders).
    pub muted: Rgba,
    /// Secondary foreground (captions, inactive icons).
    pub subtle: Rgba,
    /// Primary foreground.
    pub text: Rgba,
    /// Errors, critical urgency, destructive actions.
    pub love: Rgba,
    /// Warnings, low battery.
    pub gold: Rgba,
    /// Signature warm accent.
    pub rose: Rgba,
    /// Deep cool accent, success.
    pub pine: Rgba,
    /// Light cool accent, information.
    pub foam: Rgba,
    /// Violet accent, links and focus.
    pub iris: Rgba,
    /// Faint highlight (track backgrounds, cursor line).
    pub highlight_low: Rgba,
    /// Medium highlight (selection, inactive indicators).
    pub highlight_med: Rgba,
    /// Strong highlight (borders, dividers).
    pub highlight_high: Rgba,
}

impl Palette {
    /// Colour for a role chosen at runtime (for example the configured accent).
    pub fn get(&self, role: Role) -> Rgba {
        match role {
            Role::Base => self.base,
            Role::Surface => self.surface,
            Role::Overlay => self.overlay,
            Role::Muted => self.muted,
            Role::Subtle => self.subtle,
            Role::Text => self.text,
            Role::Love => self.love,
            Role::Gold => self.gold,
            Role::Rose => self.rose,
            Role::Pine => self.pine,
            Role::Foam => self.foam,
            Role::Iris => self.iris,
            Role::HighlightLow => self.highlight_low,
            Role::HighlightMed => self.highlight_med,
            Role::HighlightHigh => self.highlight_high,
        }
    }

    /// Every role with its colour, in canonical order.
    pub fn iter(&self) -> impl Iterator<Item = (Role, Rgba)> + '_ {
        Role::ALL.into_iter().map(|role| (role, self.get(role)))
    }
}

/// Name of a palette slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Base,
    Surface,
    Overlay,
    Muted,
    Subtle,
    Text,
    Love,
    Gold,
    Rose,
    Pine,
    Foam,
    Iris,
    HighlightLow,
    HighlightMed,
    HighlightHigh,
}

impl Role {
    pub const ALL: [Self; 15] = [
        Self::Base,
        Self::Surface,
        Self::Overlay,
        Self::Muted,
        Self::Subtle,
        Self::Text,
        Self::Love,
        Self::Gold,
        Self::Rose,
        Self::Pine,
        Self::Foam,
        Self::Iris,
        Self::HighlightLow,
        Self::HighlightMed,
        Self::HighlightHigh,
    ];

    /// The six expressive colours, suitable as an accent.
    pub const ACCENTS: [Self; 6] = [
        Self::Love,
        Self::Gold,
        Self::Rose,
        Self::Pine,
        Self::Foam,
        Self::Iris,
    ];

    /// Kebab-case name as written in theme and config files.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Surface => "surface",
            Self::Overlay => "overlay",
            Self::Muted => "muted",
            Self::Subtle => "subtle",
            Self::Text => "text",
            Self::Love => "love",
            Self::Gold => "gold",
            Self::Rose => "rose",
            Self::Pine => "pine",
            Self::Foam => "foam",
            Self::Iris => "iris",
            Self::HighlightLow => "highlight-low",
            Self::HighlightMed => "highlight-med",
            Self::HighlightHigh => "highlight-high",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Role {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|role| role.name() == s)
            .ok_or_else(|| Error::UnknownRole(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_round_trip() {
        for role in Role::ALL {
            assert_eq!(role.name().parse::<Role>().unwrap(), role);
        }
        assert!("mauve".parse::<Role>().is_err());
    }
}
