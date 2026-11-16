//! Shared interface layer of the enso desktop suite.
//!
//! - [`shell`] — a small layer-shell runtime: implement [`App`], call [`run`].
//! - [`canvas`] — a software canvas for rounded shapes, soft shadows, text,
//!   and Nerd Font icons.
//! - [`input`] — keyboard and pointer events, including the suite-wide
//!   navigation keys.
//! - [`watch`] — inotify-based change notification for live theme switching.
//!
//! Everything is pure Rust: the Wayland protocol (no libwayland), keyboard
//! layouts (kbvm, no libxkbcommon), rasterisation (tiny-skia), and text
//! shaping (cosmic-text). Colours, spacing, and motion come from `enso-core`.

pub mod canvas;
pub mod geom;
pub mod image;
pub mod input;
mod keymap;
pub mod presence;
pub mod shell;
pub mod text;
pub mod watch;

pub use canvas::{Canvas, Offscreen};
pub use geom::{Align, Rect, Size};
pub use image::Image;
pub use input::{Button, Key, KeyEvent, Mods, Nav, PointerEvent};
pub use presence::Presence;
pub use shell::{
    Anchor, App, Cx, Error, Event, Keyboard, Layer, Margin, Paint, Proxy, SurfaceSpec, run,
};
pub use text::{Font, Text, TextEngine, Weight};
