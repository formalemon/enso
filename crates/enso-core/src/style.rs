//! Style tokens: the shared geometry, spacing, and type scale behind the
//! "soft pastel zen" look. All values are logical pixels.
//!
//! Components should reach for these instead of literal numbers so the suite
//! reads as one family.

/// Corner radii.
pub mod radius {
    /// Floating cards, launcher, notification toasts.
    pub const CARD: f32 = 14.0;
    /// Docked panels and larger inner containers.
    pub const PANEL: f32 = 12.0;
    /// Rows, buttons, inputs.
    pub const CONTROL: f32 = 8.0;
    /// Fully rounded ends for a shape of the given height (active indicators, chips).
    pub const fn pill(height: f32) -> f32 {
        height / 2.0
    }
}

/// Spacing scale. Favour the larger steps: breathing room is the point.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// Type scale, as multipliers of the configured base font size.
pub mod type_scale {
    /// Captions and secondary lines.
    pub const SMALL: f32 = 0.86;
    /// Body text.
    pub const BODY: f32 = 1.0;
    /// Row titles, input text.
    pub const TITLE: f32 = 1.14;
    /// Panel headings.
    pub const HEADING: f32 = 1.43;
    /// Line height as a multiple of font size.
    pub const LINE_HEIGHT: f32 = 1.35;
}

/// Stroke widths.
pub mod stroke {
    pub const HAIRLINE: f32 = 1.0;
    pub const FOCUS: f32 = 2.0;
}

/// Opacity levels for tints and scrims.
pub mod opacity {
    /// Backdrop behind modal overlays.
    pub const SCRIM: f32 = 0.45;
    /// Accent-tinted fills (selected rows).
    pub const TINT: f32 = 0.16;
    /// Disabled content.
    pub const DISABLED: f32 = 0.5;
}

/// Soft drop shadow under floating surfaces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// How far the shadow spreads beyond the shape.
    pub blur: f32,
    /// Downward offset.
    pub offset_y: f32,
    /// Peak opacity of the shadow colour.
    pub opacity: f32,
}

impl Shadow {
    /// Cards, toasts, launcher.
    pub const FLOATING: Self = Self {
        blur: 18.0,
        offset_y: 6.0,
        opacity: 0.28,
    };
    /// Small raised controls.
    pub const RAISED: Self = Self {
        blur: 8.0,
        offset_y: 2.0,
        opacity: 0.18,
    };
}

/// An icon: a Nerd Font glyph plus a plain-text stand-in for systems without one,
/// so binaries never depend on SVGs or icon packs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icon {
    /// Nerd Font codepoint (Material Design Icons range).
    pub glyph: &'static str,
    /// Short text used when no Nerd Font is available.
    pub text: &'static str,
}

impl Icon {
    pub const fn new(glyph: &'static str, text: &'static str) -> Self {
        Self { glyph, text }
    }

    /// The glyph or its stand-in.
    pub const fn get(self, glyphs: bool) -> &'static str {
        if glyphs { self.glyph } else { self.text }
    }
}

/// The suite's icon vocabulary. Codepoints are `nf-md-*` names from Nerd Fonts v3.
pub mod icon {
    use super::Icon;

    pub const SEARCH: Icon = Icon::new("\u{f0349}", "/"); // magnify
    pub const APPS: Icon = Icon::new("\u{f003b}", "#"); // apps
    pub const FOLDER: Icon = Icon::new("\u{f024b}", "~"); // folder
    pub const BELL: Icon = Icon::new("\u{f009a}", "!"); // bell
    pub const CLOSE: Icon = Icon::new("\u{f0156}", "x"); // close
    pub const CHECK: Icon = Icon::new("\u{f012c}", "*"); // check
    pub const PALETTE: Icon = Icon::new("\u{f03d8}", "theme"); // palette
    pub const WIFI: Icon = Icon::new("\u{f05a9}", "wifi"); // wifi
    pub const BLUETOOTH: Icon = Icon::new("\u{f00af}", "bt"); // bluetooth
    pub const VOLUME: Icon = Icon::new("\u{f057e}", "vol"); // volume-high
    pub const VOLUME_MUTED: Icon = Icon::new("\u{f0581}", "mute"); // volume-off
    pub const BATTERY: Icon = Icon::new("\u{f0079}", "bat"); // battery
    pub const BATTERY_CHARGING: Icon = Icon::new("\u{f0084}", "chg"); // battery-charging
    pub const POWER_PROFILE: Icon = Icon::new("\u{f0241}", "mode"); // flash
    pub const THERMOMETER: Icon = Icon::new("\u{f050f}", "temp"); // thermometer
    pub const BRIGHTNESS: Icon = Icon::new("\u{f00df}", "lum"); // brightness-6
    pub const KEYBOARD: Icon = Icon::new("\u{f030c}", "keys"); // keyboard
    pub const LOCK: Icon = Icon::new("\u{f033e}", ""); // lock
    pub const CHEVRON_LEFT: Icon = Icon::new("\u{f0141}", "<"); // chevron-left
    pub const CHEVRON_RIGHT: Icon = Icon::new("\u{f0142}", ">"); // chevron-right
    pub const CHEVRON_DOWN: Icon = Icon::new("\u{f0140}", "v"); // chevron-down
    pub const CHEVRON_UP: Icon = Icon::new("\u{f0143}", "^"); // chevron-up
    pub const VOLUME_LOW: Icon = Icon::new("\u{f057f}", "vol"); // volume-low
    pub const VOLUME_MEDIUM: Icon = Icon::new("\u{f0580}", "vol"); // volume-medium
    pub const MICROPHONE: Icon = Icon::new("\u{f036c}", "mic"); // microphone
    pub const MICROPHONE_OFF: Icon = Icon::new("\u{f036d}", "mic"); // microphone-off
    pub const ETHERNET: Icon = Icon::new("\u{f0200}", "net"); // ethernet
    pub const WIFI_OFF: Icon = Icon::new("\u{f05aa}", "off"); // wifi-off
    pub const WIFI_1: Icon = Icon::new("\u{f091f}", "wifi"); // wifi-strength-1
    pub const WIFI_2: Icon = Icon::new("\u{f0922}", "wifi"); // wifi-strength-2
    pub const WIFI_3: Icon = Icon::new("\u{f0925}", "wifi"); // wifi-strength-3
    pub const WIFI_4: Icon = Icon::new("\u{f0928}", "wifi"); // wifi-strength-4
}
