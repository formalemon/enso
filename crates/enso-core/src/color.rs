//! 8-bit sRGB colour with alpha, parsed from and printed as `#rrggbb[aa]`.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Error, motion::Lerp};

/// A straight (non-premultiplied) sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Self = Self::new(0, 0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 0xff)
    }

    /// Same colour with its alpha multiplied by `opacity` (clamped to `0..=1`).
    #[must_use]
    pub fn alpha(self, opacity: f32) -> Self {
        let a = f32::from(self.a) * opacity.clamp(0.0, 1.0);
        Self {
            a: a.round() as u8,
            ..self
        }
    }

    /// Blend towards `other`; `t = 0` is `self`, `t = 1` is `other`.
    #[must_use]
    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let ch = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Self::new(
            ch(self.r, other.r),
            ch(self.g, other.g),
            ch(self.b, other.b),
            ch(self.a, other.a),
        )
    }

    /// Relative luminance (`0` black, `1` white), for picking readable foregrounds.
    pub fn luminance(self) -> f32 {
        let lin = |c: u8| {
            let c = f32::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// `#rrggbb`, or `#rrggbbaa` when not fully opaque.
    pub fn hex(self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 0xff {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

impl FromStr for Rgba {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || Error::InvalidColor(s.to_owned());
        let hex = s.strip_prefix('#').ok_or_else(invalid)?;
        if !hex.is_ascii() {
            return Err(invalid());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| invalid());
        let nibble = |i: usize| {
            u8::from_str_radix(&hex[i..=i], 16)
                .map(|n| n * 17)
                .map_err(|_| invalid())
        };
        match hex.len() {
            3 => Ok(Self::rgb(nibble(0)?, nibble(1)?, nibble(2)?)),
            6 => Ok(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Ok(Self::new(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => Err(invalid()),
        }
    }
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl Lerp for Rgba {
    fn lerp(self, to: Self, t: f32) -> Self {
        self.mix(to, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_hex() {
        let c: Rgba = "#eb6f92".parse().unwrap();
        assert_eq!(c, Rgba::rgb(0xeb, 0x6f, 0x92));
        assert_eq!(c.to_string(), "#eb6f92");
        assert_eq!("#fff".parse::<Rgba>().unwrap(), Rgba::rgb(255, 255, 255));
        assert_eq!(
            "#00000080".parse::<Rgba>().unwrap().to_string(),
            "#00000080"
        );
    }

    #[test]
    fn rejects_malformed_colours() {
        for bad in ["eb6f92", "#eb6f9", "#gggggg", "", "#ébébéb"] {
            assert!(bad.parse::<Rgba>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn mixes_and_fades() {
        let black = Rgba::rgb(0, 0, 0);
        let white = Rgba::rgb(255, 255, 255);
        assert_eq!(black.mix(white, 0.5), Rgba::rgb(128, 128, 128));
        assert_eq!(white.alpha(0.5).a, 128);
        assert!(white.luminance() > 0.99 && black.luminance() < 0.01);
    }
}
