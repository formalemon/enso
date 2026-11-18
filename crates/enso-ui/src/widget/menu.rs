//! A menu: what a right-click opens, and what a dropdown drops.
//!
//! A floating card of items, each with an optional icon and the keys that
//! do the same thing — so reaching for the mouse teaches the keyboard.
//! Arrows (and `j`/`k`, `Ctrl+n`/`Ctrl+p`) move, `Enter` or a click
//! chooses, `Esc` or a click outside closes, and a letter jumps to the next
//! item starting with it. Disabled items are shown, quieter, and skipped.

use enso_core::{
    Icon, Rgba,
    style::{Shadow, opacity, radius, space, stroke},
};

use crate::{Align, Canvas, Key, KeyEvent, Nav, PointerEvent, Rect, Text};

/// One line of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Action {
        label: String,
        icon: Option<Icon>,
        /// The keys that do this, shown at the right: `Ctrl+C`.
        keys: Option<String>,
        enabled: bool,
        /// A tick beside it: a setting that is on, the chosen option.
        checked: bool,
    },
    Separator,
}

impl Item {
    pub fn new(label: &str) -> Self {
        Self::Action {
            label: label.to_owned(),
            icon: None,
            keys: None,
            enabled: true,
            checked: false,
        }
    }

    #[must_use]
    pub fn icon(mut self, with: Icon) -> Self {
        if let Self::Action { icon, .. } = &mut self {
            *icon = Some(with);
        }
        self
    }

    #[must_use]
    pub fn keys(mut self, shown: &str) -> Self {
        if let Self::Action { keys, .. } = &mut self {
            *keys = Some(shown.to_owned());
        }
        self
    }

    #[must_use]
    pub fn enabled(mut self, on: bool) -> Self {
        if let Self::Action { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }

    #[must_use]
    pub fn checked(mut self, on: bool) -> Self {
        if let Self::Action { checked, .. } = &mut self {
            *checked = on;
        }
        self
    }

    fn is_choosable(&self) -> bool {
        matches!(self, Self::Action { enabled: true, .. })
    }
}

/// What an event did to the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    /// Not for the menu (it's still open).
    Ignored,
    /// The highlight moved, or the pointer is over the menu: redraw.
    Changed,
    /// The item at this index was chosen; the menu should close.
    Chosen(usize),
    /// Closed without a choice.
    Closed,
}

const ROW: f32 = 32.0;
const SEPARATOR: f32 = 9.0;
const WIDTH_MIN: f32 = 180.0;

#[derive(Clone, Debug)]
pub struct Menu {
    items: Vec<Item>,
    /// The highlighted item.
    selected: Option<usize>,
    /// Where it was asked to appear: its top-left corner, before fitting.
    at: (f32, f32),
    /// Where it was drawn last, and each item's rectangle.
    rect: Rect,
    rows: Vec<Rect>,
}

impl Menu {
    /// A menu of `items` opening at `(x, y)` — usually the pointer, or below
    /// the control that opened it.
    pub fn new(items: Vec<Item>, x: f32, y: f32) -> Self {
        Self {
            items,
            selected: None,
            at: (x, y),
            rect: Rect::default(),
            rows: Vec::new(),
        }
    }

    /// Open with `index` highlighted, as a dropdown opens on its value.
    #[must_use]
    pub fn with_selected(mut self, index: usize) -> Self {
        self.selected = self
            .items
            .get(index)
            .filter(|item| item.is_choosable())
            .map(|_| index);
        self
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Where the menu is, as last drawn.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn key(&mut self, key: &KeyEvent) -> Response {
        if let Some(nav) = key.nav(true) {
            self.step(nav);
            return Response::Changed;
        }
        match key.key {
            Key::Escape => Response::Closed,
            Key::Enter => match self.selected {
                Some(index) => Response::Chosen(index),
                None => Response::Ignored,
            },
            Key::Home => {
                self.selected = None;
                self.step(Nav::Down);
                Response::Changed
            }
            Key::End => {
                self.selected = None;
                self.step(Nav::Up);
                Response::Changed
            }
            Key::Tab => {
                self.step(if key.mods.shift { Nav::Up } else { Nav::Down });
                Response::Changed
            }
            Key::Char(c) if key.mods.is_plain() => self.jump(c),
            _ => Response::Ignored,
        }
    }

    /// Move the highlight one choosable item, wrapping round.
    fn step(&mut self, nav: Nav) {
        let count = self.items.len();
        if count == 0 {
            return;
        }
        let mut at = self.selected;
        for _ in 0..count {
            let next = match (at, nav) {
                (None, Nav::Down) => 0,
                (None, Nav::Up) => count - 1,
                (Some(i), Nav::Down) => (i + 1) % count,
                (Some(i), Nav::Up) => (i + count - 1) % count,
            };
            at = Some(next);
            if self.items[next].is_choosable() {
                self.selected = Some(next);
                return;
            }
        }
    }

    /// Highlight the next item starting with `letter`; if it's the only
    /// one, choose it.
    fn jump(&mut self, letter: char) -> Response {
        let letter = letter.to_lowercase().next().unwrap_or(letter);
        let starts = |item: &Item| match item {
            Item::Action {
                label,
                enabled: true,
                ..
            } => label.chars().next().and_then(|c| c.to_lowercase().next()) == Some(letter),
            _ => false,
        };
        let matching: Vec<usize> = (0..self.items.len())
            .filter(|&i| starts(&self.items[i]))
            .collect();
        match matching.as_slice() {
            [] => Response::Ignored,
            [only] => Response::Chosen(*only),
            _ => {
                let after = self.selected.map_or(0, |i| i + 1);
                let next = matching
                    .iter()
                    .find(|&&i| i >= after)
                    .unwrap_or(&matching[0]);
                self.selected = Some(*next);
                Response::Changed
            }
        }
    }

    pub fn pointer(&mut self, event: &PointerEvent) -> Response {
        match *event {
            PointerEvent::Enter { x, y } | PointerEvent::Move { x, y } => {
                let over = self.item_at(x, y);
                if over != self.selected && (over.is_some() || self.rect.contains(x, y)) {
                    self.selected = over;
                    return Response::Changed;
                }
                if self.rect.contains(x, y) {
                    Response::Changed
                } else {
                    Response::Ignored
                }
            }
            // Choose on release, so press-drag-release works as well as a click.
            PointerEvent::Release { x, y, .. } => match self.item_at(x, y) {
                Some(index) => Response::Chosen(index),
                None => Response::Ignored,
            },
            PointerEvent::Press { x, y, .. } if !self.rect.contains(x, y) => Response::Closed,
            _ => Response::Ignored,
        }
    }

    fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.contains(x, y))
            .filter(|&index| self.items[index].is_choosable())
    }

    /// Draw the menu, fitted inside `bounds` (flipped up or left where it
    /// would run off).
    pub fn draw(&mut self, canvas: &mut Canvas<'_>, bounds: Rect, style: &Style) {
        let label_width = self
            .items
            .iter()
            .map(|item| match item {
                Item::Action { label, keys, .. } => {
                    let label = canvas.measure(&Text::new(label, style.size, style.text)).w;
                    let keys = keys.as_deref().map_or(0.0, |keys| {
                        canvas
                            .measure(&Text::new(keys, style.size * 0.9, style.muted))
                            .w
                            + space::XL
                    });
                    label + keys
                }
                Item::Separator => 0.0,
            })
            .fold(0.0_f32, f32::max);
        let icon_room = 18.0 + space::MD;
        let width = (label_width + icon_room + 2.0 * space::MD + space::SM * 2.0)
            .max(WIDTH_MIN)
            .min(bounds.w - 2.0 * space::SM);
        let height: f32 = self
            .items
            .iter()
            .map(|item| {
                if matches!(item, Item::Separator) {
                    SEPARATOR
                } else {
                    ROW
                }
            })
            .sum::<f32>()
            + 2.0 * space::XS;
        let height = height.min(bounds.h - 2.0 * space::SM);
        let (mut x, mut y) = self.at;
        if x + width > bounds.right() - space::SM {
            x = (x - width).max(bounds.x + space::SM);
        }
        if y + height > bounds.bottom() - space::SM {
            y = (y - height).max(bounds.y + space::SM);
        }
        let rect = Rect::new(x, y, width, height);
        self.rect = rect;

        canvas.shadow(rect, radius::CARD, Shadow::FLOATING, Rgba::rgb(0, 0, 0));
        canvas.fill(rect, radius::CARD, style.background);
        canvas.stroke(rect, radius::CARD, stroke::HAIRLINE, style.border);

        self.rows.clear();
        let mut top = rect.y + space::XS;
        for (index, item) in self.items.iter().enumerate() {
            match item {
                Item::Separator => {
                    let line = Rect::new(
                        rect.x + space::MD,
                        top + SEPARATOR / 2.0,
                        rect.w - 2.0 * space::MD,
                        stroke::HAIRLINE,
                    );
                    canvas.fill(line, 0.0, style.border);
                    self.rows.push(Rect::new(rect.x, top, rect.w, SEPARATOR));
                    top += SEPARATOR;
                }
                Item::Action {
                    label,
                    icon,
                    keys,
                    enabled,
                    checked,
                } => {
                    let row = Rect::new(rect.x + space::XS, top, rect.w - 2.0 * space::XS, ROW);
                    self.rows.push(row);
                    if self.selected == Some(index) {
                        canvas.fill(row, radius::CONTROL, style.highlight);
                    }
                    let fade = |color: Rgba| {
                        if *enabled {
                            color
                        } else {
                            color.alpha(opacity::DISABLED)
                        }
                    };
                    let inner = row.inset_xy(space::SM, 0.0);
                    let icon_box = Rect::new(inner.x, inner.y, 18.0, inner.h);
                    if *checked {
                        canvas.icon(
                            enso_core::style::icon::CHECK,
                            16.0,
                            fade(style.accent),
                            icon_box,
                            Align::Center,
                        );
                    } else if let Some(icon) = icon {
                        canvas.icon(*icon, 16.0, fade(style.muted), icon_box, Align::Center);
                    }
                    let text = Rect::new(
                        icon_box.right() + space::MD,
                        inner.y,
                        inner.right() - icon_box.right() - space::MD,
                        inner.h,
                    );
                    if let Some(keys) = keys {
                        canvas.text_in(
                            &Text::new(keys, style.size * 0.9, fade(style.muted)),
                            text,
                            Align::End,
                        );
                    }
                    canvas.text_in(
                        &Text::new(label, style.size, fade(style.text)),
                        text,
                        Align::Start,
                    );
                    top += ROW;
                }
            }
        }
    }
}

/// How a menu looks: colours from the palette, so it follows the theme.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub size: f32,
    pub background: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub highlight: Rgba,
    pub accent: Rgba,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Button, Mods};

    fn key(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            mods: Mods::default(),
            repeat: false,
        }
    }

    fn menu() -> Menu {
        Menu::new(
            vec![
                Item::new("Open"),
                Item::new("Open with…").enabled(false),
                Item::Separator,
                Item::new("Copy").keys("Ctrl+C"),
                Item::new("Cut").keys("Ctrl+X"),
                Item::new("Rename").keys("F2"),
            ],
            10.0,
            10.0,
        )
    }

    #[test]
    fn arrows_skip_what_cannot_be_chosen_and_wrap() {
        let mut menu = menu();
        menu.key(&key(Key::Down));
        assert_eq!(menu.selected(), Some(0));
        menu.key(&key(Key::Down));
        assert_eq!(
            menu.selected(),
            Some(3),
            "past the disabled item and the line"
        );
        menu.key(&key(Key::Up));
        assert_eq!(menu.selected(), Some(0));
        menu.key(&key(Key::Up));
        assert_eq!(menu.selected(), Some(5), "round to the end");
        assert_eq!(menu.key(&key(Key::Enter)), Response::Chosen(5));
        assert_eq!(menu.key(&key(Key::Escape)), Response::Closed);
    }

    #[test]
    fn letters_jump_and_a_lone_match_is_chosen() {
        let mut menu = menu();
        assert_eq!(menu.key(&key(Key::Char('c'))), Response::Changed);
        assert_eq!(menu.selected(), Some(3));
        menu.key(&key(Key::Char('c')));
        assert_eq!(menu.selected(), Some(4));
        assert_eq!(menu.key(&key(Key::Char('r'))), Response::Chosen(5));
        // Only enabled items count: "o" has one choosable match.
        assert_eq!(menu.key(&key(Key::Char('o'))), Response::Chosen(0));
        assert_eq!(menu.key(&key(Key::Char('z'))), Response::Ignored);
    }

    #[test]
    fn dropdowns_open_on_their_value() {
        assert_eq!(menu().with_selected(4).selected(), Some(4));
        assert_eq!(menu().with_selected(1).selected(), None);
    }

    #[test]
    fn the_pointer_chooses_on_release_and_closes_outside() {
        let mut menu = menu();
        menu.rect = Rect::new(0.0, 0.0, 200.0, 200.0);
        menu.rows = (0..6)
            .map(|i| Rect::new(0.0, i as f32 * 30.0, 200.0, 30.0))
            .collect();
        assert_eq!(
            menu.pointer(&PointerEvent::Move { x: 50.0, y: 100.0 }),
            Response::Changed
        );
        assert_eq!(menu.selected(), Some(3));
        // Over the disabled item nothing is highlighted, or chosen.
        menu.pointer(&PointerEvent::Move { x: 50.0, y: 40.0 });
        assert_eq!(menu.selected(), None);
        assert_eq!(
            menu.pointer(&PointerEvent::Release {
                x: 50.0,
                y: 40.0,
                button: Button::Left
            }),
            Response::Ignored
        );
        assert_eq!(
            menu.pointer(&PointerEvent::Release {
                x: 50.0,
                y: 130.0,
                button: Button::Left
            }),
            Response::Chosen(4)
        );
        assert_eq!(
            menu.pointer(&PointerEvent::Press {
                x: 300.0,
                y: 10.0,
                button: Button::Left
            }),
            Response::Closed
        );
    }
}
