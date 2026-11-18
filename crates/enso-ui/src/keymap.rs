//! Keyboard layouts, handled in pure Rust with kbvm (no libxkbcommon).

use kbvm::{
    GroupIndex, Keycode, ModifierMask,
    lookup::LookupTable,
    syms,
    xkb::{Context, diagnostic::Diagnostic},
};

use std::{
    os::{fd::OwnedFd, unix::fs::FileExt},
    time::Duration,
};

use crate::input::{Key, KeyEvent, Mods};

/// Everything `wl_keyboard` tells a client, kept between events: the layout,
/// which modifiers are down, and how held keys repeat. Each runtime feeds it
/// from its own keyboard and asks it what a key code means.
#[derive(Default)]
pub(crate) struct Keys {
    keymap: Option<Keymap>,
    mods: ModifierMask,
    group: GroupIndex,
    /// Repeats per second and initial delay in milliseconds.
    repeat: Option<(u32, u32)>,
}

impl Keys {
    /// Take the keymap sent with `wl_keyboard.keymap`.
    pub fn load(&mut self, fd: OwnedFd, size: u32) {
        let mut bytes = vec![0; size as usize];
        // The compositor may share one file description between clients, so
        // read at an explicit offset rather than from the current position.
        if std::fs::File::from(fd)
            .read_exact_at(&mut bytes, 0)
            .is_err()
        {
            return;
        }
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        self.keymap = Keymap::from_xkb(&bytes);
    }

    /// Take `wl_keyboard.modifiers`. Returns whether Caps Lock changed.
    pub fn set_modifiers(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) -> bool {
        let before = self.mods.contains(ModifierMask::LOCK);
        self.mods = ModifierMask(depressed | latched | locked);
        self.group = GroupIndex(group);
        before != self.mods.contains(ModifierMask::LOCK)
    }

    /// Take `wl_keyboard.repeat_info`.
    pub fn set_repeat(&mut self, rate: i32, delay: i32) {
        self.repeat = (rate > 0).then_some((rate.unsigned_abs(), delay.unsigned_abs()));
    }

    /// The modifiers held right now.
    pub fn mods(&self) -> Mods {
        mods_of(self.mods)
    }

    /// What pressing the evdev key `keycode` means right now. `None` if the
    /// key produces nothing, or no keymap has arrived.
    pub fn translate(&self, keycode: u32, repeat: bool) -> Option<KeyEvent> {
        let (key, mods) = self
            .keymap
            .as_ref()?
            .translate(keycode, self.mods, self.group)?;
        Some(KeyEvent { key, mods, repeat })
    }

    /// If holding `keycode` repeats: the delay before the first repeat and
    /// the interval between the following ones.
    pub fn repeat_timing(&self, keycode: u32) -> Option<(Duration, Duration)> {
        let (rate, delay) = self.repeat?;
        let repeats = self
            .keymap
            .as_ref()
            .is_some_and(|keymap| keymap.repeats(keycode));
        (rate > 0 && repeats).then(|| {
            (
                Duration::from_millis(u64::from(delay)),
                Duration::from_millis(u64::from(1000 / rate).max(1)),
            )
        })
    }
}

fn mods_of(mods: ModifierMask) -> Mods {
    Mods {
        ctrl: mods.contains(ModifierMask::CONTROL),
        alt: mods.contains(ModifierMask::ALT),
        shift: mods.contains(ModifierMask::SHIFT),
        logo: mods.contains(ModifierMask::SUPER),
        caps_lock: mods.contains(ModifierMask::LOCK),
    }
}

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
            (syms::Insert, Key::Insert),
            (syms::Menu, Key::Menu),
            (syms::F1, Key::F(1)),
            (syms::F2, Key::F(2)),
            (syms::F3, Key::F(3)),
            (syms::F4, Key::F(4)),
            (syms::F5, Key::F(5)),
            (syms::F6, Key::F(6)),
            (syms::F7, Key::F(7)),
            (syms::F8, Key::F(8)),
            (syms::F9, Key::F(9)),
            (syms::F10, Key::F(10)),
            (syms::F11, Key::F(11)),
            (syms::F12, Key::F(12)),
        ];
        let key = named
            .iter()
            .find(|(named, _)| *named == sym)
            .map(|(_, key)| *key)
            .unwrap_or_else(|| match props.char() {
                Some(c) if !c.is_control() => Key::Char(c),
                _ => Key::Other,
            });
        Some((key, mods_of(mods)))
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
