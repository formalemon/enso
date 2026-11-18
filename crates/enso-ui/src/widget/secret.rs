//! A secret field: a password or a passphrase, typed as dots.
//!
//! What's typed is kept in memory that is overwritten when it's let go,
//! with room for the longest reserved up front, so growing never leaves a
//! copy behind. It shows as a row of dots, the newest swelling in — never
//! the characters, and never more dots than fit, so the row keeps the
//! length to itself. A refusal shakes the field; the line under it says
//! what went wrong, or that Caps Lock is on.
//!
//! The same field asks for a PIN or a pairing code, shown as typed.

use std::{f32::consts::PI, fmt, time::Instant};

use enso_core::{
    Icon, Motion, Rgba,
    motion::{Transition, Tween},
    style::{icon, space, stroke},
};
use zeroize::Zeroizing;

use super::{Look, field::Frame};
use crate::{Align, Canvas, Font, Key, KeyEvent, Mods, Rect, Text};

/// Dots drawn at most, however much room there is.
const MAX_DOTS: usize = 20;
/// How far the field sways as it shakes its head.
const SWAY: f32 = 10.0;
/// Sways to a refusal.
const SWAYS: f32 = 3.0;

/// How what's typed is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// A password: dots.
    Dots,
    /// As typed: a PIN, or an answer that isn't secret.
    Text,
    /// As typed, and only digits: a pairing code.
    Digits,
}

/// What a key did to the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    /// Not a key for the field.
    Ignored,
    /// What's typed changed.
    Changed,
    /// Enter.
    Submit,
    /// Escape.
    Cancel,
}

pub struct SecretField {
    typed: Zeroizing<String>,
    shown: Shown,
    /// Characters taken, at most.
    max: usize,
    /// How many dots there are, animated.
    dots: Tween<f32>,
    /// How far through its shake: `1` at rest.
    shake: Tween<f32>,
    caps_lock: bool,
}

impl fmt::Debug for SecretField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never what's typed.
        f.debug_struct("SecretField")
            .field("shown", &self.shown)
            .field("empty", &self.typed.is_empty())
            .finish_non_exhaustive()
    }
}

impl SecretField {
    /// An empty field taking at most `max` characters, shown as dots.
    pub fn new(max: usize) -> Self {
        Self {
            typed: room(max),
            shown: Shown::Dots,
            max,
            dots: Tween::new(0.0),
            shake: Tween::new(1.0),
            caps_lock: false,
        }
    }

    /// Ask for something else: shown as `shown`, at most `max`
    /// characters. Starts empty.
    pub fn ask(&mut self, shown: Shown, max: usize, motion: &Motion, now: Instant) {
        self.shown = shown;
        self.max = max;
        self.clear(motion, now);
    }

    pub fn shown(&self) -> Shown {
        self.shown
    }

    /// What's typed. Copy it nowhere that isn't wiped too.
    pub fn text(&self) -> &str {
        &self.typed
    }

    pub fn is_empty(&self) -> bool {
        self.typed.is_empty()
    }

    /// Hand over what's typed, leaving the field empty.
    pub fn take(&mut self, motion: &Motion, now: Instant) -> Zeroizing<String> {
        let typed = std::mem::replace(&mut self.typed, room(self.max));
        self.settle(motion, now);
        typed
    }

    /// Forget what's typed, overwriting it.
    pub fn clear(&mut self, motion: &Motion, now: Instant) {
        self.typed = room(self.max);
        self.settle(motion, now);
    }

    /// Shake the field's head: what was typed was refused.
    pub fn refuse(&mut self, motion: &Motion, now: Instant) {
        self.shake.jump(0.0);
        self.shake.set(1.0, motion.apply(Transition::SHAKE), now);
    }

    pub fn caps_lock(&self) -> bool {
        self.caps_lock
    }

    /// Note whether Caps Lock is on, from any key's modifiers. Returns
    /// whether that changed.
    pub fn note_mods(&mut self, mods: Mods) -> bool {
        std::mem::replace(&mut self.caps_lock, mods.caps_lock) != mods.caps_lock
    }

    /// Handle a key: typing, Backspace (with Ctrl, all of it), Ctrl+U.
    /// Enter and Escape are handed back as [`Response::Submit`] and
    /// [`Response::Cancel`]; other shortcuts aren't typed.
    pub fn key(&mut self, key: &KeyEvent, motion: &Motion, now: Instant) -> Response {
        match key.key {
            Key::Enter => return Response::Submit,
            Key::Escape => return Response::Cancel,
            Key::Char('u') | Key::Backspace if key.mods.ctrl => {
                if self.typed.is_empty() {
                    return Response::Ignored;
                }
                self.typed = room(self.max);
            }
            Key::Backspace => {
                if self.typed.pop().is_none() {
                    return Response::Ignored;
                }
            }
            _ => {
                let Some(c) = key.text() else {
                    return Response::Ignored;
                };
                let takes = match self.shown {
                    Shown::Dots | Shown::Text => !c.is_control(),
                    Shown::Digits => c.is_ascii_digit(),
                };
                if !takes || self.typed.chars().count() >= self.max {
                    return Response::Ignored;
                }
                self.typed.push(c);
            }
        }
        self.settle(motion, now);
        Response::Changed
    }

    fn settle(&mut self, motion: &Motion, now: Instant) {
        let count = self.typed.chars().count().min(MAX_DOTS) as f32;
        self.dots.set(count, motion.apply(Transition::HOVER), now);
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.dots.is_animating(now) || self.shake.is_animating(now)
    }

    /// How far sideways the field is as it shakes.
    pub fn sway(&self, now: Instant) -> f32 {
        let t = self.shake.get(now);
        if t >= 1.0 {
            0.0
        } else {
            (t * PI * 2.0 * SWAYS).sin() * (1.0 - t) * SWAY
        }
    }

    /// Draw the field in `rect` (swaying from it while it shakes).
    pub fn draw(&self, canvas: &mut Canvas<'_>, rect: Rect, style: &Style<'_>, now: Instant) {
        let look = style.look;
        let rect = rect.translate(self.sway(now), 0.0);
        let dots = self.dots.get(now);
        let filled = !self.typed.is_empty() || dots > 0.0;
        let inner = Frame {
            focused: style.focused,
            filled,
            complaint: style.complaint,
            busy: style.busy,
        }
        .draw(canvas, rect, look, style.fill);
        let ink = if style.busy { look.subtle } else { look.text };

        if !filled {
            match style.icon {
                Some(icon) if canvas.glyphs() => {
                    let size = (rect.h * 0.42).round();
                    canvas.icon(icon, size, look.muted, rect, Align::Center);
                }
                _ => {
                    let hint = Text::new(style.hint, look.size, look.muted).max_width(inner.w);
                    canvas.text_in(&hint, inner, style.align);
                }
            }
            return;
        }

        if self.shown == Shown::Dots {
            // The row grows from its start (or its middle); the newest
            // dot swells in.
            let dot = (rect.h * 0.19).round().max(4.0);
            let fits = ((inner.w + dot) / (2.0 * dot)).floor();
            let dots = dots.min(fits);
            let row = dots * dot + (dots - 1.0).max(0.0) * dot;
            let start = match style.align {
                Align::Center => inner.x + (inner.w - row) / 2.0,
                Align::End => inner.right() - row,
                Align::Start => inner.x,
            };
            let (_, center_y) = rect.center();
            for index in 0..dots.ceil() as usize {
                let grown = (dots - index as f32).clamp(0.0, 1.0);
                let x = start + index as f32 * 2.0 * dot + dot / 2.0;
                canvas.circle(x, center_y, dot / 2.0 * grown, ink);
            }
            return;
        }

        let font = if self.shown == Shown::Digits {
            Font::Mono
        } else {
            Font::Ui
        };
        let text = Text::new(&self.typed, look.size, ink).font(font);
        let size = canvas.text_in(&text, inner, style.align);
        if style.focused && !style.busy {
            let start = match style.align {
                Align::Center => inner.x + (inner.w - size.w) / 2.0,
                Align::End => inner.right() - size.w,
                Align::Start => inner.x,
            };
            let caret = Rect::new(
                (start + size.w + 2.0).min(inner.right()),
                rect.y + rect.h * 0.27,
                stroke::FOCUS,
                rect.h * 0.46,
            );
            canvas.fill(caret, 1.0, look.accent);
        }
    }
}

/// An empty answer with room for `max` characters (four bytes each).
fn room(max: usize) -> Zeroizing<String> {
    Zeroizing::new(String::with_capacity(max * 4))
}

/// How a secret field is drawn where it is.
#[derive(Clone, Copy, Debug)]
pub struct Style<'a> {
    pub look: &'a Look,
    /// The field's own colour, under what's typed.
    pub fill: Rgba,
    /// Said while it's empty.
    pub hint: &'a str,
    /// Shown while it's empty in place of the hint, where icons are drawn.
    pub icon: Option<Icon>,
    /// Where the dots or the hint sit.
    pub align: Align,
    pub focused: bool,
    /// What was typed is being checked.
    pub busy: bool,
    /// What was typed was refused.
    pub complaint: bool,
}

impl<'a> Style<'a> {
    /// A focused field in a card: on `look`'s base, dots from the start.
    pub fn new(look: &'a Look, hint: &'a str) -> Self {
        Self {
            look,
            fill: look.base,
            hint,
            icon: None,
            align: Align::Start,
            focused: true,
            busy: false,
            complaint: false,
        }
    }
}

/// What the line under a field says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Note<'a> {
    #[default]
    Nothing,
    /// A note, quietly.
    Says(&'a str),
    /// What went wrong.
    Complains(&'a str),
    /// Caps Lock is on: worth knowing while typing a password.
    CapsLock,
}

impl<'a> Note<'a> {
    /// The line under `field`: what's `said` (a complaint if `true`), or
    /// else, while a password is typed, whether Caps Lock is on.
    pub fn under(field: &SecretField, said: Option<(&'a str, bool)>, busy: bool) -> Self {
        match said {
            Some((words, true)) => Self::Complains(words),
            Some((words, false)) => Self::Says(words),
            None if field.caps_lock && field.shown == Shown::Dots && !busy => Self::CapsLock,
            None => Self::Nothing,
        }
    }

    pub fn is_complaint(&self) -> bool {
        matches!(self, Self::Complains(_))
    }

    /// Draw the line in `rect`, in small type of `size`.
    pub fn draw(self, canvas: &mut Canvas<'_>, rect: Rect, look: &Look, size: f32, align: Align) {
        let (words, tint) = match self {
            Self::Nothing => return,
            Self::Says(words) => (words, look.subtle),
            Self::Complains(words) => (words, look.danger),
            Self::CapsLock => ("Caps Lock is on", look.subtle),
        };
        let text = Text::new(words, size, tint).max_width(rect.w);
        if self != Self::CapsLock || !canvas.glyphs() {
            canvas.text_in(&text, rect, align);
            return;
        }
        // Caps Lock gets its glyph, just before the words.
        let glyph = size + 2.0;
        let width = canvas.measure(&text).w;
        let whole = glyph + space::SM + width;
        let start = match align {
            Align::Center => rect.x + (rect.w - whole) / 2.0,
            Align::End => rect.right() - whole,
            Align::Start => rect.x,
        };
        canvas.icon(
            icon::CAPS_LOCK,
            glyph,
            look.warning,
            Rect::new(start, rect.y, glyph, rect.h),
            Align::Start,
        );
        canvas.text_in(
            &text,
            Rect::new(start + glyph + space::SM, rect.y, width + 1.0, rect.h),
            Align::Start,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(field: &mut SecretField, key: Key, mods: Mods) -> Response {
        let event = KeyEvent {
            key,
            mods,
            repeat: false,
        };
        field.key(&event, &Motion::none(), Instant::now())
    }

    fn type_text(field: &mut SecretField, text: &str) {
        for c in text.chars() {
            press(field, Key::Char(c), Mods::default());
        }
    }

    #[test]
    fn typing_backspace_and_clearing() {
        let mut field = SecretField::new(8);
        type_text(&mut field, "hunter2");
        assert_eq!(field.text(), "hunter2");
        assert_eq!(
            press(&mut field, Key::Backspace, Mods::default()),
            Response::Changed
        );
        assert_eq!(field.text(), "hunter");
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        // Other shortcuts aren't typed into it.
        assert_eq!(press(&mut field, Key::Char('a'), ctrl), Response::Ignored);
        assert_eq!(press(&mut field, Key::Char('u'), ctrl), Response::Changed);
        assert!(field.is_empty());
        assert_eq!(press(&mut field, Key::Backspace, ctrl), Response::Ignored);
        assert_eq!(
            press(&mut field, Key::Enter, Mods::default()),
            Response::Submit
        );
        assert_eq!(
            press(&mut field, Key::Escape, Mods::default()),
            Response::Cancel
        );
    }

    #[test]
    fn takes_no_more_than_it_may_and_digits_only_when_asked() {
        let mut field = SecretField::new(3);
        type_text(&mut field, "abcdef");
        assert_eq!(field.text(), "abc");
        field.ask(Shown::Digits, 6, &Motion::none(), Instant::now());
        assert!(field.is_empty());
        type_text(&mut field, "12a3456789");
        assert_eq!(field.text(), "123456");
    }

    #[test]
    fn taking_empties_it_and_keeps_room() {
        let mut field = SecretField::new(16);
        type_text(&mut field, "secret");
        let taken = field.take(&Motion::none(), Instant::now());
        assert_eq!(taken.as_str(), "secret");
        assert!(field.is_empty());
        assert!(field.typed.capacity() >= 64);
        assert!(!format!("{field:?}").contains("secret"));
    }

    #[test]
    fn caps_lock_is_noted_and_said_only_for_passwords() {
        let mut field = SecretField::new(8);
        let caps = Mods {
            caps_lock: true,
            ..Mods::default()
        };
        assert!(field.note_mods(caps));
        assert!(!field.note_mods(caps));
        assert_eq!(Note::under(&field, None, false), Note::CapsLock);
        assert_eq!(Note::under(&field, None, true), Note::Nothing);
        assert_eq!(
            Note::under(&field, Some(("No", true)), false),
            Note::Complains("No")
        );
        field.ask(Shown::Text, 8, &Motion::none(), Instant::now());
        assert_eq!(Note::under(&field, None, false), Note::Nothing);
    }

    #[test]
    fn a_refusal_shakes_and_settles() {
        let mut field = SecretField::new(8);
        let now = Instant::now();
        assert_eq!(field.sway(now), 0.0);
        field.refuse(&Motion::default(), now);
        let during = now + std::time::Duration::from_millis(40);
        assert!(field.sway(during).abs() > 0.5);
        assert!(field.is_animating(during));
        let after = now + std::time::Duration::from_secs(1);
        assert_eq!(field.sway(after), 0.0);
        // Without motion, no shake at all.
        field.refuse(&Motion::none(), now);
        assert_eq!(field.sway(during), 0.0);
    }
}
