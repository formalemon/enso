//! Keyboard layouts, handled in pure Rust with kbvm (no libxkbcommon).

use kbvm::{
    GroupIndex, Keycode, ModifierMask,
    lookup::LookupTable,
    syms,
    xkb::{Context, diagnostic::Diagnostic},
};

use crate::input::{Key, Mods};

/// The compositor's keymap, ready to translate key codes.
pub(crate) struct Keymap {
    table: LookupTable,
}

impl Keymap {
    /// Parse the XKB text keymap sent with `wl_keyboard.keymap`.
    pub fn from_xkb(text: &[u8]) -> Option<Self> {
        // Fatal problems come back as `Err`; warnings are not worth surfacing.
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let keymap = Context::default()
            .keymap_from_bytes(&mut diagnostics, None, text)
            .ok()?;
        Some(Self {
            table: keymap.to_builder().build_lookup_table(),
        })
    }

    /// What pressing the evdev key `keycode` means under the given modifier
    /// and layout state. `None` if the key produces nothing.
    pub fn translate(
        &self,
        keycode: u32,
        mods: ModifierMask,
        group: GroupIndex,
    ) -> Option<(Key, Mods)> {
        let props = self
            .table
            .lookup(group, mods, Keycode::from_evdev(keycode))
            // Report Ctrl+n as `n` with the ctrl flag, not as a control character.
            .with_ctrl_transform(false)
            .into_iter()
            .next()?;

        let sym = props.keysym();
        let named = [
            (syms::Escape, Key::Escape),
            (syms::Return, Key::Enter),
            (syms::KP_Enter, Key::Enter),
            (syms::Tab, Key::Tab),
            (syms::ISO_Left_Tab, Key::Tab),
            (syms::BackSpace, Key::Backspace),
            (syms::Delete, Key::Delete),
            (syms::Up, Key::Up),
            (syms::Down, Key::Down),
            (syms::Left, Key::Left),
            (syms::Right, Key::Right),
            (syms::Home, Key::Home),
            (syms::End, Key::End),
            (syms::Page_Up, Key::PageUp),
            (syms::Page_Down, Key::PageDown),
        ];
        let key = named
            .iter()
            .find(|(named, _)| *named == sym)
            .map(|(_, key)| *key)
            .unwrap_or_else(|| match props.char() {
                Some(c) if !c.is_control() => Key::Char(c),
                _ => Key::Other,
            });
        let mods = Mods {
            ctrl: mods.contains(ModifierMask::CONTROL),
            alt: mods.contains(ModifierMask::ALT),
            shift: mods.contains(ModifierMask::SHIFT),
            logo: mods.contains(ModifierMask::SUPER),
        };
        Some((key, mods))
    }

    /// Whether holding the key should auto-repeat.
    pub fn repeats(&self, keycode: u32) -> bool {
        self.table.repeats(Keycode::from_evdev(keycode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Linux evdev key codes.
    const KEY_ESC: u32 = 1;
    const KEY_ENTER: u32 = 28;
    const KEY_LEFTSHIFT: u32 = 42;
    const KEY_N: u32 = 49;
    const KEY_DOWN: u32 = 108;

    /// The default (US) layout, serialised the way a compositor sends it.
    /// `None` when the system has no XKB layout data to build it from.
    fn us_keymap() -> Option<Keymap> {
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let text = Context::default()
            .keymap_from_names(&mut diagnostics, None, None, None, None)
            .format()
            .to_string();
        let keymap = Keymap::from_xkb(text.as_bytes())?;
        keymap
            .translate(KEY_N, ModifierMask::default(), GroupIndex::default())
            .is_some()
            .then_some(keymap)
    }

    #[test]
    fn translates_keys_through_a_real_layout() {
        let Some(keymap) = us_keymap() else {
            eprintln!("skipped: no XKB layout data installed");
            return;
        };
        let press = |code, mods| keymap.translate(code, mods, GroupIndex::default());
        let none = ModifierMask::default();

        assert_eq!(press(KEY_N, none), Some((Key::Char('n'), Mods::default())));
        assert_eq!(press(KEY_ESC, none).map(|(key, _)| key), Some(Key::Escape));
        assert_eq!(press(KEY_ENTER, none).map(|(key, _)| key), Some(Key::Enter));
        assert_eq!(press(KEY_DOWN, none).map(|(key, _)| key), Some(Key::Down));

        let (key, mods) = press(KEY_N, ModifierMask::SHIFT).expect("shift+n");
        assert_eq!((key, mods.shift), (Key::Char('N'), true));

        // Ctrl+n stays a letter with the ctrl flag: that is what list
        // navigation matches on.
        let (key, mods) = press(KEY_N, ModifierMask::CONTROL).expect("ctrl+n");
        assert_eq!((key, mods.ctrl), (Key::Char('n'), true));

        assert!(keymap.repeats(KEY_N));
        assert!(!keymap.repeats(KEY_LEFTSHIFT));
    }

    #[test]
    fn rejects_garbage_keymaps() {
        assert!(Keymap::from_xkb(b"not a keymap").is_none());
    }
}
