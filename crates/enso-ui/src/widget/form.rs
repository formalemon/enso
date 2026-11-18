//! A dialog's card, laid out the one way every dialog in the suite is: a
//! title (beside a badge, if it has one), what it's about, its fields and
//! choices, a line for what went wrong, and the buttons at the bottom
//! right, the main one rightmost. The card is as tall as what it says.
//!
//! The form draws all of that except what's typed: fields belong to their
//! owners — a [`TextField`](super::TextField), a
//! [`SecretField`](super::SecretField) — which draw into the places
//! [`Form::draw`] hands back, and hit-test with them too.

use enso_core::{
    Icon, Rgba,
    style::{opacity, radius, space, type_scale},
};

use super::{
    Button, Interaction, Kind, Look, Toggle,
    controls::{self, TOGGLE},
    field,
    secret::Note,
};
use crate::{Align, Canvas, Font, Rect, Text};

/// A dialog's usual width.
pub const WIDTH: f32 = 440.0;
/// The badge beside the title.
const BADGE: f32 = 40.0;
/// Room for a field's label, beside it.
const LABEL: f32 = 64.0;
/// A row of choices, and each choice's pill in it.
const CHOICES: f32 = 30.0;
const CHOICE: f32 = 26.0;

/// One thing in the form, under what it's about.
#[derive(Clone, Copy, Debug)]
pub enum Part<'a> {
    /// A one-line field, with a label beside it or not. Its owner draws it
    /// into the place it's given.
    Field { label: Option<&'a str> },
    /// A row of pills to pick one of: who answers, which format.
    Choices {
        label: Option<&'a str>,
        options: &'a [&'a str],
        chosen: usize,
        hovered: Option<usize>,
        /// Whether they can be picked now (otherwise only the chosen one
        /// matters, and none is hit-tested).
        enabled: bool,
    },
    /// A number or a code to read off, large.
    Code(&'a str),
    /// A switch with what it does beside it. Its place is the whole row,
    /// to hit-test.
    Toggle { label: &'a str, on: bool },
    /// Room for the owner's own drawing.
    Space(f32),
}

/// Where everything went.
#[derive(Clone, Debug, Default)]
pub struct Placed {
    /// The card's inside.
    pub inner: Rect,
    /// Each part's place, in the order given: a field's own outline, a
    /// row of choices, a space.
    pub parts: Vec<Rect>,
    /// Each choice that can be picked: its part, its index, its pill.
    pub choices: Vec<(usize, usize, Rect)>,
    /// The line under the parts, if there is one.
    pub note: Option<Rect>,
    /// Each button, in the order given.
    pub buttons: Vec<Rect>,
}

/// A dialog's contents, to lay out and draw.
#[derive(Clone, Debug)]
pub struct Form<'a> {
    width: f32,
    badge: Option<(Icon, Rgba)>,
    title: &'a str,
    about: Vec<&'a str>,
    aside: Option<&'a str>,
    parts: Vec<Part<'a>>,
    note: Option<Note<'a>>,
    buttons: Vec<(&'a str, Kind, Interaction)>,
}

impl<'a> Form<'a> {
    pub fn new(title: &'a str) -> Self {
        Self {
            width: WIDTH,
            badge: None,
            title,
            about: Vec::new(),
            aside: None,
            parts: Vec::new(),
            note: None,
            buttons: Vec::new(),
        }
    }

    #[must_use]
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// A badge before the title: an icon in a tinted circle.
    #[must_use]
    pub fn badge(mut self, icon: Icon, color: Rgba) -> Self {
        self.badge = Some((icon, color));
        self
    }

    /// What it's about, in a paragraph under the title.
    #[must_use]
    pub fn about(mut self, text: &'a str) -> Self {
        if !text.is_empty() {
            self.about.push(text);
        }
        self
    }

    /// A smaller line after what it's about: who's asking, say.
    #[must_use]
    pub fn aside(mut self, text: Option<&'a str>) -> Self {
        self.aside = text.filter(|text| !text.is_empty());
        self
    }

    #[must_use]
    pub fn part(mut self, part: Part<'a>) -> Self {
        self.parts.push(part);
        self
    }

    /// A one-line field.
    #[must_use]
    pub fn field(self) -> Self {
        self.part(Part::Field { label: None })
    }

    /// A line under the parts for what went wrong or what to know. The
    /// room is kept even while it says nothing, so the card doesn't jump.
    #[must_use]
    pub fn note(mut self, note: Note<'a>) -> Self {
        self.note = Some(note);
        self
    }

    /// A button; each one added goes right of the one before.
    #[must_use]
    pub fn button(mut self, label: &'a str, kind: Kind, state: Interaction) -> Self {
        self.buttons.push((label, kind, state));
        self
    }

    fn sizes(look: &Look) -> (f32, f32, f32) {
        let title = title_size(look);
        let small = (look.size * type_scale::SMALL).round();
        let code = (look.size * type_scale::HEADING * 1.6).round();
        (title, small, code)
    }

    fn texts(&self, look: &Look, inner_w: f32) -> (Text<'a>, Vec<Text<'a>>, Option<Text<'a>>) {
        let (title_size, small, _) = Self::sizes(look);
        let title_w = match self.badge {
            Some(_) => inner_w - BADGE - space::MD,
            None => inner_w,
        };
        let title = Text::new(self.title, title_size, look.text)
            .medium()
            .max_width(title_w)
            .max_lines(2);
        let about = self
            .about
            .iter()
            .map(|&text| {
                Text::new(text, look.size, look.subtle)
                    .max_width(inner_w)
                    .max_lines(6)
            })
            .collect();
        let aside = self.aside.map(|text| {
            Text::new(text, small, look.muted)
                .max_width(inner_w)
                .max_lines(2)
        });
        (title, about, aside)
    }

    fn part_height(part: &Part<'_>, code: f32) -> f32 {
        match part {
            Part::Field { .. } => field::HEIGHT,
            Part::Choices { .. } => CHOICES,
            Part::Code(_) => code * 1.4,
            Part::Toggle { .. } => TOGGLE.1,
            Part::Space(height) => *height,
        }
    }

    /// How tall the card is, outside.
    pub fn height(&self, canvas: &mut Canvas<'_>, look: &Look) -> f32 {
        let inner_w = self.width - 2.0 * space::XL;
        let (_, small, code) = Self::sizes(look);
        let (title, about, aside) = self.texts(look, inner_w);
        let mut height = canvas.measure(&title).h;
        if self.badge.is_some() {
            height = height.max(BADGE);
        }
        for text in &about {
            height += space::MD + canvas.measure(text).h;
        }
        if let Some(aside) = &aside {
            height += space::XS + canvas.measure(aside).h;
        }
        for part in &self.parts {
            height += space::LG + Self::part_height(part, code);
        }
        if self.note.is_some() {
            height += space::SM + small * 1.5;
        }
        if !self.buttons.is_empty() {
            height += space::LG + controls::HEIGHT;
        }
        height + 2.0 * space::XL
    }

    /// Dim `bounds` and draw the card in its middle, sliding up a little
    /// as `presence` goes `0 → 1`. The canvas is left at `presence`'s
    /// opacity for the owner's fields; set it back to `1` after.
    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        bounds: Rect,
        look: &Look,
        presence: f32,
    ) -> Placed {
        let height = self.height(canvas, look);
        let inner = controls::dialog(canvas, bounds, self.width, height, look, presence);
        canvas.set_opacity(presence);
        let (_, small, code_size) = Self::sizes(look);
        let (title, about, aside) = self.texts(look, inner.w);
        let mut placed = Placed {
            inner,
            ..Placed::default()
        };

        // The title, beside its badge.
        let title_h = canvas.measure(&title).h;
        let row = if self.badge.is_some() {
            title_h.max(BADGE)
        } else {
            title_h
        };
        let mut x = inner.x;
        if let Some((icon, color)) = self.badge {
            let badge = Rect::new(inner.x, inner.y + (row - BADGE) / 2.0, BADGE, BADGE);
            let (cx, cy) = badge.center();
            canvas.circle(cx, cy, BADGE / 2.0, color.alpha(opacity::TINT));
            canvas.icon(icon, 20.0, color, badge, Align::Center);
            x = badge.right() + space::MD;
        }
        canvas.text(&title, x, inner.y + (row - title_h) / 2.0);
        let mut y = inner.y + row;

        for text in &about {
            y += space::MD;
            y += canvas.text(text, inner.x, y).h;
        }
        if let Some(aside) = &aside {
            y += space::XS;
            y += canvas.text(aside, inner.x, y).h;
        }

        for (index, part) in self.parts.iter().enumerate() {
            y += space::LG;
            let height = Self::part_height(part, code_size);
            let rect = Rect::new(inner.x, y, inner.w, height);
            let place = match *part {
                Part::Field { label } => match label {
                    Some(label) => {
                        canvas.text_in(
                            &Text::new(label, small, look.subtle),
                            Rect::new(inner.x, y, LABEL, height),
                            Align::Start,
                        );
                        Rect::new(inner.x + LABEL, y, inner.w - LABEL, height)
                    }
                    None => rect,
                },
                Part::Choices {
                    label,
                    options,
                    chosen,
                    hovered,
                    enabled,
                } => {
                    let mut x = inner.x;
                    if let Some(label) = label {
                        let label = Text::new(label, small, look.muted);
                        let width = canvas.measure(&label).w;
                        canvas.text_in(&label, Rect::new(x, y, width, height), Align::Start);
                        x += width + space::SM;
                    }
                    for (option, name) in options.iter().enumerate() {
                        if x + pill_width(canvas, name, small) > inner.right() {
                            break;
                        }
                        let hovered = enabled && hovered == Some(option);
                        let at = (x, y + (height - CHOICE) / 2.0);
                        let pill = pill(canvas, at, name, option == chosen, hovered, look);
                        if enabled {
                            placed.choices.push((index, option, pill));
                        }
                        x = pill.right() + space::XS;
                    }
                    rect
                }
                Part::Code(code) => {
                    let text = Text::new(code, code_size, look.text).font(Font::Mono);
                    canvas.text_in(&text, rect, Align::Center);
                    rect
                }
                Part::Toggle { label, on } => {
                    let switch = Rect::new(inner.x, y, TOGGLE.0, TOGGLE.1);
                    Toggle::new(on).draw(
                        canvas,
                        switch,
                        look,
                        Interaction::default(),
                        std::time::Instant::now(),
                    );
                    let words = Text::new(label, small, look.subtle);
                    let width = canvas.measure(&words).w;
                    let words_at = Rect::new(switch.right() + space::SM, y, width + 1.0, height);
                    canvas.text_in(&words, words_at, Align::Start);
                    Rect::new(inner.x, y, words_at.right() - inner.x, height)
                }
                Part::Space(_) => rect,
            };
            placed.parts.push(place);
            y += height;
        }

        if let Some(note) = self.note {
            y += space::SM;
            let line = Rect::new(inner.x, y, inner.w, small * 1.5);
            note.draw(canvas, line, look, small, Align::Start);
            placed.note = Some(line);
        }

        // The buttons, the last rightmost.
        let row_y = inner.bottom() - controls::HEIGHT;
        let mut right = inner.right();
        let mut rects = Vec::with_capacity(self.buttons.len());
        for (label, kind, state) in self.buttons.iter().rev() {
            let button = Button::new(label, *kind);
            let width = button.width(canvas, look);
            let rect = Rect::new(right - width, row_y, width, controls::HEIGHT);
            button.draw(canvas, rect, look, *state);
            rects.push(rect);
            right = rect.x - space::SM;
        }
        rects.reverse();
        placed.buttons = rects;
        placed
    }
}

/// A dialog's title size, for `look`: every dialog's, and the cards that
/// lay themselves out (Properties, Settings) take it too.
pub fn title_size(look: &Look) -> f32 {
    (look.size * type_scale::TITLE).round()
}

/// How wide a choice's pill is for `label` in small type of `size`.
pub fn pill_width(canvas: &mut Canvas<'_>, label: &str, size: f32) -> f32 {
    canvas
        .measure(&Text::new(label, size, Rgba::rgb(0, 0, 0)))
        .w
        + 2.0 * space::MD
}

/// A choice's pill at `(x, y)`: lit with the accent when `on`. Returns
/// where it went, to hit-test.
pub fn pill(
    canvas: &mut Canvas<'_>,
    (x, y): (f32, f32),
    label: &str,
    on: bool,
    hovered: bool,
    look: &Look,
) -> Rect {
    let size = (look.size * type_scale::SMALL).round();
    let ink = if on { look.base } else { look.subtle };
    let text = Text::new(label, size, ink);
    let rect = Rect::new(x, y, canvas.measure(&text).w + 2.0 * space::MD, CHOICE);
    let fill = if on {
        look.accent
    } else if hovered {
        look.highlight_med
    } else {
        look.overlay
    };
    canvas.fill(rect, radius::pill(CHOICE), fill);
    canvas.text_in(&text, rect, Align::Center);
    rect
}
