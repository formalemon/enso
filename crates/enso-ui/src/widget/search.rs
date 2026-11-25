//! The search card — the launcher, the clipboard history: a field on top
//! and a list under it, the chosen row highlighted.
//!
//! A tool keeps its own results and says what each row shows; [`List`]
//! keeps which row is chosen and which rows are in view, and glides the
//! highlight and the list's height as they change, and the functions here
//! draw the parts every search card shares. So every search card looks and
//! moves the same: the same field, the same rows, the same hints.
//!
//! The card's body itself — where it rests, how it comes and goes — is
//! [`crate::emerge`]'s.

use std::time::Instant;

use enso_core::{
    Icon, Motion, Palette, Rgba, Role, Transition, Tween,
    style::{icon, opacity, radius, search, space, stroke, type_scale},
};

use crate::{
    Align, Canvas, Nav, Rect, Text,
    widget::{TextField, field},
};

/// What a search card is drawn in: the theme's colours, its accent, and
/// the size of body text.
#[derive(Clone, Debug)]
pub struct Look {
    pub colors: Palette,
    pub accent: Rgba,
    pub size: f32,
}

/// The list's height for `lines` rows: nothing at all for none, so the
/// card is the field alone.
pub fn height(lines: usize) -> f32 {
    if lines == 0 {
        0.0
    } else {
        lines as f32 * search::ROW + space::SM
    }
}

/// Which result is chosen and which are in view; the highlight and the
/// list's height gliding as they change; and where the rows were drawn,
/// for the pointer.
#[derive(Clone, Debug)]
pub struct List {
    /// The most rows in view at once; the list scrolls beyond.
    most: usize,
    count: usize,
    selected: usize,
    /// The first result in view.
    offset: usize,
    /// The highlight's place, in rows from the top of the view.
    highlight: Tween<f32>,
    height: Tween<f32>,
    /// The rows drawn last frame.
    drawn: Vec<Rect>,
    motion: Motion,
}

/// One row as drawn: the result it shows, the room for its insides, and
/// whether it is the chosen one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub index: usize,
    pub content: Rect,
    pub chosen: bool,
}

impl List {
    /// An empty list, showing at most `most` rows at once.
    pub fn new(most: usize, motion: &Motion) -> Self {
        Self {
            most: most.max(1),
            count: 0,
            selected: 0,
            offset: 0,
            highlight: Tween::new(0.0),
            height: Tween::new(0.0),
            drawn: Vec::new(),
            motion: *motion,
        }
    }

    /// The results are new: `count` of them, the first one chosen.
    pub fn reset(&mut self, count: usize) {
        self.count = count;
        self.selected = 0;
        self.offset = 0;
        self.highlight
            .set(0.0, self.motion.apply(Transition::SELECT), Instant::now());
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// How many rows are in view: as many results as there are, up to the
    /// most.
    pub fn visible(&self) -> usize {
        self.count.min(self.most)
    }

    /// The chosen result, if there are any.
    pub fn selected(&self) -> Option<usize> {
        (self.count > 0).then_some(self.selected)
    }

    /// The first result in view.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Choose result `index`, scrolling just enough to keep it in view.
    pub fn select(&mut self, index: usize) {
        if self.count == 0 {
            return;
        }
        self.selected = index.min(self.count - 1);
        let rows = self.visible();
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + rows {
            self.offset = self.selected + 1 - rows;
        }
        self.highlight.set(
            (self.selected - self.offset) as f32,
            self.motion.apply(Transition::SELECT),
            Instant::now(),
        );
    }

    /// A row up or down, from the last round to the first and back.
    pub fn step(&mut self, nav: Nav) {
        if self.count == 0 {
            return;
        }
        self.select(match nav {
            Nav::Down => (self.selected + 1) % self.count,
            Nav::Up => (self.selected + self.count - 1) % self.count,
        });
    }

    /// A turn of the wheel: a row down for a turn down, up for up.
    pub fn scroll(&mut self, dy: f32) {
        if dy != 0.0 {
            self.step(if dy > 0.0 { Nav::Down } else { Nav::Up });
        }
    }

    /// The result whose row is under (`x`, `y`), as last drawn.
    pub fn at(&self, x: f32, y: f32) -> Option<usize> {
        self.drawn
            .iter()
            .position(|row| row.contains(x, y))
            .map(|row| self.offset + row)
    }

    /// The rows drawn last frame.
    pub fn drawn(&self) -> &[Rect] {
        &self.drawn
    }

    /// Let the list's height glide to `target` (see [`height`]).
    pub fn grow(&mut self, target: f32) {
        self.height.set(
            target,
            self.motion.apply(Transition::REFLOW),
            Instant::now(),
        );
    }

    /// The height the list is heading for.
    pub fn target(&self) -> f32 {
        self.height.target()
    }

    pub fn height(&self, now: Instant) -> f32 {
        self.height.get(now)
    }

    /// Whether the highlight or the height is still moving.
    pub fn is_animating(&self, now: Instant) -> bool {
        self.highlight.is_animating(now) || self.height.is_animating(now)
    }

    /// Draw the highlight under the field of `card`, and lay out the rows
    /// that fit so far — while the card grows, rows below its edge wait
    /// their turn. The caller draws each row's insides.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas<'_>,
        card: Rect,
        now: Instant,
        look: &Look,
    ) -> Vec<Row> {
        self.drawn.clear();
        if self.count == 0 {
            return Vec::new();
        }
        let list = list_area(card);
        let highlight = list
            .row(0, search::ROW, 0.0)
            .translate(0.0, self.highlight.get(now) * search::ROW);
        if highlight.bottom() <= card.bottom() + 0.5 {
            canvas.fill(highlight, radius::CONTROL, look.colors.overlay);
        }
        let mut rows = Vec::with_capacity(self.visible());
        for slot in 0..self.visible() {
            let row = list.row(slot, search::ROW, 0.0);
            if row.bottom() > card.bottom() {
                break;
            }
            self.drawn.push(row);
            let index = self.offset + slot;
            rows.push(Row {
                index,
                content: row.inset_xy(space::SM, 0.0),
                chosen: index == self.selected,
            });
        }
        rows
    }
}

/// Where the rows go under the field of `card`.
fn list_area(card: Rect) -> Rect {
    Rect::new(
        card.x + space::SM,
        card.y + search::FIELD + space::XS,
        card.w - 2.0 * space::SM,
        0.0,
    )
}

/// Draw the field across the top of `card`: the magnifier, as tall as the
/// words typed after it, then `field` saying `placeholder` while empty.
/// The rule under it belongs to the list and fades in as the list opens
/// (`list` its height now).
pub fn head(
    canvas: &mut Canvas<'_>,
    card: Rect,
    field: &mut TextField,
    placeholder: &str,
    list: f32,
    look: &Look,
) {
    let colors = &look.colors;
    let title = look.size * type_scale::TITLE;
    let search = Rect::new(card.x, card.y, card.w, search::FIELD).inset_xy(space::LG, 0.0);
    let magnifier = (title * 1.25).round();
    let room = if canvas.glyphs() {
        magnifier + space::SM
    } else {
        space::LG
    };
    canvas.icon(icon::SEARCH, magnifier, colors.subtle, search, Align::Start);
    let text = Rect::new(
        search.x + room + space::XS,
        search.y,
        search.w - room - space::XS,
        search.h,
    );
    // A caret to move, a selection, what an input method is composing — as
    // in every enso text box.
    let style = field::Style {
        size: title,
        color: colors.text,
        placeholder,
        placeholder_color: colors.muted,
        caret: look.accent,
        selection: look.accent.alpha(opacity::TINT * 2.0),
    };
    field.draw(canvas, text, &style, true);
    let open = (list / search::ROW).min(1.0);
    if open > 0.0 {
        canvas.fill(
            Rect::new(
                card.x + space::LG,
                card.y + search::FIELD - 0.5,
                card.w - 2.0 * space::LG,
                stroke::HAIRLINE,
            ),
            0.0,
            colors.highlight_med.alpha(open),
        );
    }
}

/// A quiet line where the rows would be — "No matches" — once the card has
/// grown past it.
pub fn note(canvas: &mut Canvas<'_>, card: Rect, said: &str, look: &Look) {
    let row = list_area(card)
        .row(0, search::ROW, 0.0)
        .inset_xy(space::SM, 0.0);
    if row.bottom() <= card.bottom() {
        let words = Text::new(said, look.size, look.colors.muted).max_width(row.w);
        canvas.text_in(&words, row, Align::Start);
    }
}

/// The square a row's insides start with.
pub fn chip(content: Rect) -> Rect {
    Rect::new(
        content.x,
        content.y + (content.h - search::CHIP) / 2.0,
        search::CHIP,
        search::CHIP,
    )
}

/// A chip holding the first letter of `name`, in a colour of its own: a
/// stand-in for an application's icon, needing no icon theme.
pub fn letter_chip(canvas: &mut Canvas<'_>, chip: Rect, name: &str, look: &Look) {
    let tint = look.colors.get(role_of(name));
    canvas.fill(chip, radius::CONTROL, tint.alpha(opacity::TINT + 0.06));
    let initial: String = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    canvas.text_in(
        &Text::new(&initial, look.size, tint).medium(),
        chip,
        Align::Center,
    );
}

/// A chip holding `icon` in `tint`, over a wash of it.
pub fn icon_chip(canvas: &mut Canvas<'_>, chip: Rect, icon: Icon, tint: Rgba, look: &Look) {
    canvas.fill(chip, radius::CONTROL, tint.alpha(opacity::TINT));
    let size = if canvas.glyphs() {
        look.size + 4.0
    } else {
        look.size
    };
    canvas.icon(icon, size, tint, chip, Align::Center);
}

/// The same name always gets the same of the theme's accents.
fn role_of(name: &str) -> Role {
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    });
    Role::ACCENTS[hash as usize % Role::ACCENTS.len()]
}

/// A row's words, from `from` across `content`: the `name`, then quieter
/// after it the `detail` if there is room, and at the end of the chosen
/// row the `hint` — what `Enter` does — if it is short enough. Returns
/// where the name went, its left edge and its width, for marking what
/// matched in it.
#[allow(clippy::too_many_arguments)]
pub fn words(
    canvas: &mut Canvas<'_>,
    content: Rect,
    from: f32,
    name: &str,
    detail: Option<&str>,
    hint: Option<&str>,
    chosen: bool,
    look: &Look,
) -> (f32, f32) {
    let colors = &look.colors;
    let mut right = content.right();
    if let Some(hint) = hint.filter(|_| chosen) {
        let text = Text::new(hint, look.size * type_scale::SMALL, colors.subtle);
        let width = canvas.measure(&text).w;
        if width < content.w * 0.45 {
            canvas.text_in(&text, content, Align::End);
            right -= width + space::MD;
        }
    }
    let room = (right - from).max(1.0);
    let title = Text::new(name, look.size, colors.text)
        .medium()
        .max_width(room);
    let width = canvas
        .text_in(
            &title,
            Rect::new(from, content.y, room, content.h),
            Align::Start,
        )
        .w;
    if let Some(detail) = detail.filter(|detail| !detail.is_empty()) {
        let x = from + width + space::MD;
        let room = right - x;
        if room > 60.0 {
            let text =
                Text::new(detail, look.size * type_scale::SMALL, colors.muted).max_width(room);
            canvas.text_in(
                &text,
                Rect::new(x, content.y, room, content.h),
                Align::Start,
            );
        }
    }
    (from, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chosen_row_stays_in_view_and_wraps_round() {
        let mut list = List::new(3, &Motion::none());
        assert_eq!(list.selected(), None);
        list.step(Nav::Down);
        assert_eq!(list.selected(), None);
        list.reset(5);
        assert_eq!(
            (list.selected(), list.offset(), list.visible()),
            (Some(0), 0, 3)
        );
        for _ in 0..3 {
            list.step(Nav::Down);
        }
        assert_eq!((list.selected(), list.offset()), (Some(3), 1));
        list.step(Nav::Up);
        list.step(Nav::Up);
        list.step(Nav::Up);
        assert_eq!((list.selected(), list.offset()), (Some(0), 0));
        // Up from the first goes round to the last, in view.
        list.step(Nav::Up);
        assert_eq!((list.selected(), list.offset()), (Some(4), 2));
        list.scroll(1.0);
        assert_eq!((list.selected(), list.offset()), (Some(0), 0));
        list.scroll(-1.0);
        assert_eq!(list.selected(), Some(4));
        // Past the end is the last.
        list.select(99);
        assert_eq!(list.selected(), Some(4));
        // New results start again at the top.
        list.reset(2);
        assert_eq!(
            (list.selected(), list.offset(), list.visible()),
            (Some(0), 0, 2)
        );
    }

    #[test]
    fn the_list_is_as_tall_as_its_rows() {
        assert_eq!(height(0), 0.0);
        assert_eq!(height(2), 2.0 * search::ROW + space::SM);
        let mut list = List::new(8, &Motion::none());
        list.grow(height(3));
        assert_eq!(list.height(Instant::now()), height(3));
        assert_eq!(list.target(), height(3));
    }

    #[test]
    fn a_name_keeps_its_colour() {
        assert_eq!(role_of("Firefox"), role_of("Firefox"));
    }
}
