//! Shared interface layer of the enso desktop suite.
//!
//! - [`shell`] — a small runtime for one layer surface or one window:
//!   implement [`App`], call [`run`].
//! - [`screens`] — its sibling for interfaces that cover every output, one
//!   surface each: the wallpaper and the lock screen.
//! - [`canvas`] — a software canvas for rounded shapes, soft shadows, text,
//!   icons, and shapes of one's own.
//! - [`figure`] — the suite's icons, drawn: one hand, one pen, no icon font.
//! - [`picture`] — full-screen pictures, fitted to an output once and copied.
//! - input methods (`ime`) — composing text into the focused field, the
//!   preedit shown in it.
//! - [`input`] — keyboard and pointer events, including the suite-wide
//!   navigation keys.
//! - [`watch`] — inotify-based change notification for live theme switching.
//! - [`widget`] — controls: text fields, menus, buttons, toggles, dialogs,
//!   tooltips, scrolling, splitters.
//!
//! Everything is pure Rust: the Wayland protocol (no libwayland), keyboard
//! layouts (kbvm, no libxkbcommon), rasterisation (tiny-skia), and text
//! shaping (cosmic-text). Colours, spacing, and motion come from `enso-core`.

pub mod canvas;
pub mod figure;
mod fractional;
pub mod geom;
pub mod idle;
pub mod image;
mod ime;
pub mod input;
mod keymap;
pub mod picture;
pub mod presence;
pub mod screens;
pub mod shell;
pub mod text;
pub mod watch;
pub mod widget;

pub use canvas::{Canvas, Offscreen, Shape};
pub use geom::{Align, Rect, Size};
pub use image::Image;
pub use ime::Preedit;
pub use input::{
    BUTTON_BACK, BUTTON_FORWARD, Button, Clicks, Key, KeyEvent, Mods, Nav, PointerEvent,
};
pub use picture::Picture;
pub use presence::Presence;
pub use screens::{Role, Screen, Screens, run_screens};
pub use shell::{
    Anchor, App, Clip, Cursor, Cx, Error, Event, Keyboard, Layer, Margin, Paint, Proxy,
    SurfaceSpec, TEXT_TYPES, WindowSpec, run,
};
pub use text::{Font, Text, TextEngine, Weight};
