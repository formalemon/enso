//! Controls shared by the suite's interfaces, styled only from the shared
//! tokens.
//!
//! A widget is a plain struct: it takes events and says what they did,
//! and draws itself into a rectangle it is given. There is no widget tree;
//! the interface that owns a widget lays it out and routes input to it,
//! as it does for everything else it draws.
//!
//! - [`field`] — a one-line text field.
//! - [`menu`] — context menus and dropdowns.
//! - [`scroll`] — a scrolling offset and its scrollbar.
//! - [`controls`] — buttons, toggles, the dialog card, tooltips, splitters,
//!   and the [`Look`] they share.

pub mod controls;
pub mod field;
pub mod menu;
pub mod scroll;

pub use controls::{Button, Interaction, Kind, Look, Splitter, Toggle, Tooltip};
pub use field::TextField;
pub use menu::{Item, Menu};
pub use scroll::Scroll;
