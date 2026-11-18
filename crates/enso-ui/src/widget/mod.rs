//! Controls shared by the suite's interfaces, styled only from the shared
//! tokens.
//!
//! A widget is a plain struct: it takes events and says what they did,
//! and draws itself into a rectangle it is given. There is no widget tree;
//! the interface that owns a widget lays it out and routes input to it,
//! as it does for everything else it draws.
//!
//! - [`field`] — a one-line text field.

pub mod field;

pub use field::TextField;
