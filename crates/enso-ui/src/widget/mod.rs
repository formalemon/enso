//! Controls shared by the suite's interfaces, styled only from the shared
//! tokens.
//!
//! A widget is a plain struct: it takes events and says what they did,
//! and draws itself into a rectangle it is given. There is no widget tree;
//! the interface that owns a widget lays it out and routes input to it,
//! as it does for everything else it draws.
//!
//! - [`field`] — a one-line text field, and the shape every field has.
//! - [`secret`] — a password field: dots, a shake, the Caps Lock note.
//! - [`form`] — a dialog's card: title, what it's about, fields, choices,
//!   buttons.
//! - [`menu`] — context menus and dropdowns.
//! - [`scroll`] — a scrolling offset and its scrollbar.
//! - [`search`] — the search card's field and list: the launcher's, the
//!   clipboard history's.
//! - [`controls`] — buttons, toggles, the dialog card, tooltips, splitters,
//!   and the [`Look`] they share.
//! - [`wave`] — a player's timeline: a wave for what has played.
//! - [`toast`] — a word about what was just done, a moment over a window.

pub mod controls;
pub mod field;
pub mod form;
pub mod menu;
pub mod scroll;
pub mod search;
pub mod secret;
pub mod toast;
pub mod wave;

pub use controls::{Button, Interaction, Kind, Look, Splitter, Toggle, Tooltip};
pub use field::TextField;
pub use form::{Form, Part, Placed};
pub use menu::{Item, Menu};
pub use scroll::Scroll;
pub use secret::{Note, SecretField, Shown};
