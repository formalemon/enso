//! What's over the window: one thing at a time, which takes the keys and
//! the pointer while it's up — a question, a form, a panel, a menu.
//!
//! Being one value is what keeps it to one: opening something puts away
//! whatever was up before, and there is no order of precedence between
//! them to get wrong.

use std::path::PathBuf;

use enso_ui::widget::Menu;

use super::{MenuFor, archives, ops, palette, properties, renamer, vcs};

pub(super) enum Overlay {
    /// The keys worth knowing (`?`, `F1`).
    Help,
    /// The quick look (`Shift+Space`).
    Peek,
    /// The command palette (`Ctrl+P`).
    Palette(palette::Palette),
    /// Renaming many at once.
    Renamer(renamer::Renamer),
    /// Making an archive.
    Packing(archives::Packing),
    Properties(properties::Properties),
    /// Settings, and which line has the keyboard.
    Settings(usize),
    /// Version control's question or form.
    Vcs(vcs::Vcs),
    /// A name already taken, while copying or moving.
    Asked(ops::Asked),
    /// Deleting for good: what.
    ConfirmDelete(Vec<PathBuf>),
    /// Saving, in the picker, over this file.
    Replace(PathBuf),
    /// A menu, and what it's for.
    Menu((Menu, MenuFor)),
}

/// Getting at the overlay when it's a particular one. These borrow only
/// the overlay, so the rest of the window stays at hand meanwhile.
macro_rules! reach {
    ($($variant:ident($ty:ty) => $get:ident, $get_mut:ident, $take:ident;)*) => {
        // Every overlay gets the same three ways in, whether or not each
        // is needed for each yet.
        #[allow(dead_code)]
        pub(super) trait Reach {
            $(
                fn $get(&self) -> Option<&$ty>;
                fn $get_mut(&mut self) -> Option<&mut $ty>;
                /// Take it away, if it's this one; any other stays up.
                fn $take(&mut self) -> Option<$ty>;
            )*
        }

        impl Reach for Option<Overlay> {
            $(
                fn $get(&self) -> Option<&$ty> {
                    match self {
                        Some(Overlay::$variant(inner)) => Some(inner),
                        _ => None,
                    }
                }

                fn $get_mut(&mut self) -> Option<&mut $ty> {
                    match self {
                        Some(Overlay::$variant(inner)) => Some(inner),
                        _ => None,
                    }
                }

                fn $take(&mut self) -> Option<$ty> {
                    match self.take() {
                        Some(Overlay::$variant(inner)) => Some(inner),
                        other => {
                            *self = other;
                            None
                        }
                    }
                }
            )*
        }
    };
}

reach! {
    Palette(palette::Palette) => palette, palette_mut, take_palette;
    Renamer(renamer::Renamer) => renamer, renamer_mut, take_renamer;
    Packing(archives::Packing) => packing, packing_mut, take_packing;
    Properties(properties::Properties) => properties, properties_mut, take_properties;
    Settings(usize) => settings, settings_mut, take_settings;
    Vcs(vcs::Vcs) => vcs, vcs_mut, take_vcs;
    Asked(ops::Asked) => asked, asked_mut, take_asked;
    ConfirmDelete(Vec<PathBuf>) => confirm_delete, confirm_delete_mut, take_confirm_delete;
    Replace(PathBuf) => replace, replace_mut, take_replace;
    Menu((Menu, MenuFor)) => menu, menu_mut, take_menu;
}
