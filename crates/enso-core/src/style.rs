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

/// Where floating things sit. The bar's islands and every card that drops
/// from the bar — the control center, the notification history, toasts —
/// take their place from these, so their edges line up.
pub mod edge {
    use super::space;

    /// Between the side of the screen and an island or a card.
    pub const SCREEN: f32 = space::SM;
    /// Between the bar's surface and a card hanging from it. The islands are
    /// inset [`ISLAND_INSET`] within the bar, so the card is
    /// `ISLAND_INSET + BELOW_BAR` — [`SCREEN`] — from its island: the same
    /// breath in both directions.
    pub const BELOW_BAR: f32 = space::XS;
    /// Between the bar's edges and its islands, above and below.
    pub const ISLAND_INSET: f32 = space::XS;
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

/// The search cards — the launcher, the clipboard history: a field on top,
/// rows under it, each row starting with a square chip (a letter, an icon,
/// a picture).
pub mod search {
    pub const WIDTH: f32 = 560.0;
    pub const FIELD: f32 = 56.0;
    pub const ROW: f32 = 44.0;
    pub const CHIP: f32 = 28.0;
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
    /// Black in every theme: light comes from above, whatever the colours.
    pub color: crate::Rgba,
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
        color: crate::Rgba::rgb(0, 0, 0),
        blur: 18.0,
        offset_y: 6.0,
        opacity: 0.28,
    };
    /// Where a floating card nearly touches what is behind it: tight and
    /// dark. Drawn with [`Shadow::AMBIENT`], for depth as light gives it.
    pub const CONTACT: Self = Self {
        color: crate::Rgba::rgb(0, 0, 0),
        blur: 3.0,
        offset_y: 1.0,
        opacity: 0.34,
    };
    /// The wide, faint shadow a floating card casts further off.
    pub const AMBIENT: Self = Self {
        color: crate::Rgba::rgb(0, 0, 0),
        blur: 30.0,
        offset_y: 12.0,
        opacity: 0.16,
    };
    /// Small raised controls.
    pub const RAISED: Self = Self {
        color: crate::Rgba::rgb(0, 0, 0),
        blur: 8.0,
        offset_y: 2.0,
        opacity: 0.18,
    };
}

/// What an icon shows. enso-ui draws every one itself — round-ended
/// strokes, soft corners, one pen for all — so no tool depends on an icon
/// font, an image file, or an icon pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Figure {
    Search,
    Apps,
    Folder,
    File,
    Bell,
    Close,
    Check,
    Palette,
    /// Wi-Fi with this much signal, `0..=4`: the dot, then each arc.
    Wifi(u8),
    WifiOff,
    Ethernet,
    Bluetooth,
    /// A speaker sounding `1..=3`: one wave, two with the outer faint, two.
    Speaker(u8),
    SpeakerMuted,
    Battery,
    BatteryCharging,
    /// A lightning bolt: the power profile.
    Bolt,
    Thermometer,
    Brightness,
    Keyboard,
    Lock,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    Microphone,
    MicrophoneOff,
    Power,
    Restart,
    Logout,
    Sleep,
    CapsLock,
    Home,
    Trash,
    /// A disk: a drive, a partition, a USB stick.
    Drive,
    Eject,
    /// Time: what was used lately.
    Clock,
    /// A ribbon: a folder kept at hand.
    Bookmark,
    /// Media: a triangle pointing on.
    Play,
    /// Media: two bars.
    Pause,
    /// Media: the triangle up against a bar — skip on.
    Next,
    /// Media: the same, turned back.
    Previous,
    /// The bell, quietened: Do not disturb.
    BellOff,
    /// A cup with steam over it: kept awake.
    Cup,
    /// A screen with a dot on it: the screen is being shared.
    Screencast,
    /// A screen on its stand: displays.
    Display,
    /// Two sliders, each with its knob: settings.
    Sliders,
}

/// An icon: the figure drawn, and a few letters standing in for it where
/// pictures are not wanted (`icons = "text"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icon {
    pub figure: Figure,
    /// Short text used instead, when icons are words.
    pub text: &'static str,
}

impl Icon {
    pub const fn new(figure: Figure, text: &'static str) -> Self {
        Self { figure, text }
    }
}

/// The suite's icon vocabulary.
pub mod icon {
    use super::{Figure, Icon};

    pub const SEARCH: Icon = Icon::new(Figure::Search, "/");
    pub const APPS: Icon = Icon::new(Figure::Apps, "#");
    pub const FOLDER: Icon = Icon::new(Figure::Folder, "/");
    pub const FILE: Icon = Icon::new(Figure::File, "-");
    pub const BELL: Icon = Icon::new(Figure::Bell, "!");
    pub const CLOSE: Icon = Icon::new(Figure::Close, "x");
    pub const CHECK: Icon = Icon::new(Figure::Check, "*");
    pub const PALETTE: Icon = Icon::new(Figure::Palette, "theme");
    pub const WIFI: Icon = Icon::new(Figure::Wifi(4), "wifi");
    pub const BLUETOOTH: Icon = Icon::new(Figure::Bluetooth, "bt");
    pub const VOLUME: Icon = Icon::new(Figure::Speaker(3), "vol");
    pub const VOLUME_MUTED: Icon = Icon::new(Figure::SpeakerMuted, "mute");
    pub const BATTERY: Icon = Icon::new(Figure::Battery, "bat");
    pub const BATTERY_CHARGING: Icon = Icon::new(Figure::BatteryCharging, "chg");
    pub const POWER_PROFILE: Icon = Icon::new(Figure::Bolt, "mode");
    pub const THERMOMETER: Icon = Icon::new(Figure::Thermometer, "temp");
    pub const BRIGHTNESS: Icon = Icon::new(Figure::Brightness, "lum");
    pub const KEYBOARD: Icon = Icon::new(Figure::Keyboard, "keys");
    pub const LOCK: Icon = Icon::new(Figure::Lock, "");
    pub const CHEVRON_LEFT: Icon = Icon::new(Figure::ChevronLeft, "<");
    pub const CHEVRON_RIGHT: Icon = Icon::new(Figure::ChevronRight, ">");
    pub const CHEVRON_DOWN: Icon = Icon::new(Figure::ChevronDown, "v");
    pub const CHEVRON_UP: Icon = Icon::new(Figure::ChevronUp, "^");
    pub const VOLUME_LOW: Icon = Icon::new(Figure::Speaker(1), "vol");
    pub const VOLUME_MEDIUM: Icon = Icon::new(Figure::Speaker(2), "vol");
    pub const MICROPHONE: Icon = Icon::new(Figure::Microphone, "mic");
    pub const MICROPHONE_OFF: Icon = Icon::new(Figure::MicrophoneOff, "mic");
    pub const ETHERNET: Icon = Icon::new(Figure::Ethernet, "net");
    pub const WIFI_OFF: Icon = Icon::new(Figure::WifiOff, "off");
    pub const WIFI_1: Icon = Icon::new(Figure::Wifi(1), "wifi");
    pub const WIFI_2: Icon = Icon::new(Figure::Wifi(2), "wifi");
    pub const WIFI_3: Icon = Icon::new(Figure::Wifi(3), "wifi");
    pub const WIFI_4: Icon = Icon::new(Figure::Wifi(4), "wifi");
    pub const POWER: Icon = Icon::new(Figure::Power, "off");
    pub const RESTART: Icon = Icon::new(Figure::Restart, "re");
    pub const LOGOUT: Icon = Icon::new(Figure::Logout, "exit");
    pub const SLEEP: Icon = Icon::new(Figure::Sleep, "zz");
    pub const CAPS_LOCK: Icon = Icon::new(Figure::CapsLock, "caps");
    pub const HOME: Icon = Icon::new(Figure::Home, "~");
    pub const TRASH: Icon = Icon::new(Figure::Trash, "del");
    pub const DRIVE: Icon = Icon::new(Figure::Drive, "disk");
    pub const EJECT: Icon = Icon::new(Figure::Eject, "^");
    pub const CLOCK: Icon = Icon::new(Figure::Clock, "@");
    pub const BOOKMARK: Icon = Icon::new(Figure::Bookmark, "*");
    pub const PLAY: Icon = Icon::new(Figure::Play, "play");
    pub const PAUSE: Icon = Icon::new(Figure::Pause, "pause");
    pub const NEXT: Icon = Icon::new(Figure::Next, ">>");
    pub const PREVIOUS: Icon = Icon::new(Figure::Previous, "<<");
    pub const BELL_OFF: Icon = Icon::new(Figure::BellOff, "dnd");
    pub const CUP: Icon = Icon::new(Figure::Cup, "awake");
    pub const SCREENCAST: Icon = Icon::new(Figure::Screencast, "rec");
    pub const DISPLAY: Icon = Icon::new(Figure::Display, "scr");
    pub const SLIDERS: Icon = Icon::new(Figure::Sliders, "set");
}
