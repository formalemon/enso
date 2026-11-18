//! The small controls: buttons, toggle switches, the dialog card, tooltips,
//! and a splitter to drag. Each draws itself from a [`Look`] — the palette
//! and accent at hand — and leaves layout and hit-testing to its owner.

use std::time::{Duration, Instant};

use enso_core::{
    Icon, Palette, Rgba,
    motion::{Transition, Tween},
    style::{Shadow, opacity, radius, space, stroke},
};

use crate::{Align, Canvas, Rect, Text};

/// Colours and size every control draws with, from the active theme.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// Body text size.
    pub size: f32,
    pub base: Rgba,
    pub surface: Rgba,
    pub overlay: Rgba,
    pub text: Rgba,
    pub subtle: Rgba,
    pub muted: Rgba,
    pub accent: Rgba,
    pub highlight_low: Rgba,
    pub highlight_med: Rgba,
    pub danger: Rgba,
}

impl Look {
    pub fn new(palette: &Palette, accent: Rgba, size: f32) -> Self {
        Self {
            size,
            base: palette.base,
            surface: palette.surface,
            overlay: palette.overlay,
            text: palette.text,
            subtle: palette.subtle,
            muted: palette.muted,
            accent,
            highlight_low: palette.highlight_low,
            highlight_med: palette.highlight_med,
            danger: palette.love,
        }
    }

    /// The same look, for a menu.
    pub fn menu(&self) -> super::menu::Style {
        super::menu::Style {
            size: self.size,
            background: self.overlay,
            border: self.highlight_med,
            text: self.text,
            muted: self.subtle,
            highlight: self.accent.alpha(opacity::TINT * 1.5),
            accent: self.accent,
        }
    }
}

/// How a control is being touched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Interaction {
    pub hovered: bool,
    pub pressed: bool,
    /// Has the keyboard: drawn with a focus ring.
    pub focused: bool,
    pub disabled: bool,
}

/// What a button is for, which decides how loudly it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The one thing to do here: filled with the accent.
    Primary,
    /// An alternative: a quiet fill.
    Secondary,
    /// Only text (or an icon) until hovered.
    Quiet,
    /// Something that can't be taken back: deleting.
    Danger,
}

/// Height of a button, and of other controls in a row with it.
pub const HEIGHT: f32 = 34.0;

/// A button's label and icon.
#[derive(Clone, Copy, Debug)]
pub struct Button<'a> {
    pub label: &'a str,
    pub icon: Option<Icon>,
    pub kind: Kind,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str, kind: Kind) -> Self {
        Self {
            label,
            icon: None,
            kind,
        }
    }

    #[must_use]
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// How wide it wants to be.
    pub fn width(&self, canvas: &mut Canvas<'_>, look: &Look) -> f32 {
        let label = if self.label.is_empty() {
            0.0
        } else {
            canvas
                .measure(&Text::new(self.label, look.size, look.text).medium())
                .w
        };
        let icon = if self.icon.is_some() { 18.0 } else { 0.0 };
        let gap = if icon > 0.0 && label > 0.0 {
            space::SM
        } else {
            0.0
        };
        if label == 0.0 {
            HEIGHT
        } else {
            (label + icon + gap + 2.0 * space::LG).max(80.0)
        }
    }

    pub fn draw(&self, canvas: &mut Canvas<'_>, rect: Rect, look: &Look, state: Interaction) {
        let pill = radius::pill(rect.h);
        let (fill, ink) = match self.kind {
            Kind::Primary => (Some(look.accent), look.base),
            Kind::Danger => (Some(look.danger), look.base),
            Kind::Secondary => (Some(look.highlight_low), look.text),
            Kind::Quiet => (None, look.text),
        };
        let lift = if state.pressed {
            0.85
        } else if state.hovered {
            1.1
        } else {
            1.0
        };
        canvas.with_opacity(
            if state.disabled {
                opacity::DISABLED
            } else {
                1.0
            },
            |canvas| {
                match fill {
                    Some(color) => {
                        let color = if lift > 1.0 {
                            color.mix(Rgba::rgb(255, 255, 255), 0.08)
                        } else if lift < 1.0 {
                            color.mix(Rgba::rgb(0, 0, 0), 0.12)
                        } else {
                            color
                        };
                        canvas.fill(rect, pill, color);
                    }
                    None if state.hovered || state.pressed => {
                        canvas.fill(rect, pill, look.highlight_low)
                    }
                    None => {}
                }
                if state.focused {
                    canvas.stroke(rect.inset(-3.0), pill + 3.0, stroke::FOCUS, look.accent);
                }
                let label = Text::new(self.label, look.size, ink).medium();
                let label_width = if self.label.is_empty() {
                    0.0
                } else {
                    canvas.measure(&label).w
                };
                let icon_width = if self.icon.is_some() { 18.0 } else { 0.0 };
                let gap = if icon_width > 0.0 && label_width > 0.0 {
                    space::SM
                } else {
                    0.0
                };
                let mut x = rect.x + (rect.w - label_width - icon_width - gap) / 2.0;
                if let Some(icon) = self.icon {
                    canvas.icon(
                        icon,
                        18.0,
                        ink,
                        Rect::new(x, rect.y, 18.0, rect.h),
                        Align::Start,
                    );
                    x += icon_width + gap;
                }
                if label_width > 0.0 {
                    canvas.text_in(
                        &label,
                        Rect::new(x, rect.y, label_width + 1.0, rect.h),
                        Align::Start,
                    );
                }
            },
        );
    }
}

/// A switch: on or off, its knob sliding between.
#[derive(Clone, Debug)]
pub struct Toggle {
    on: bool,
    knob: Tween<f32>,
}

/// A toggle's size.
pub const TOGGLE: (f32, f32) = (40.0, 22.0);

impl Toggle {
    pub fn new(on: bool) -> Self {
        Self {
            on,
            knob: Tween::new(if on { 1.0 } else { 0.0 }),
        }
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    pub fn set(&mut self, on: bool, transition: Transition, now: Instant) {
        self.on = on;
        self.knob.set(if on { 1.0 } else { 0.0 }, transition, now);
    }

    /// Draw in `rect` (best [`TOGGLE`] sized). Returns whether it is still
    /// moving.
    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        rect: Rect,
        look: &Look,
        state: Interaction,
        now: Instant,
    ) -> bool {
        let t = self.knob.get(now);
        let track = look.highlight_med.mix(look.accent, t);
        canvas.with_opacity(
            if state.disabled {
                opacity::DISABLED
            } else {
                1.0
            },
            |canvas| {
                canvas.pill(rect, track);
                if state.focused {
                    canvas.stroke(
                        rect.inset(-3.0),
                        radius::pill(rect.h) + 3.0,
                        stroke::FOCUS,
                        look.accent,
                    );
                }
                let knob = rect.h - 6.0;
                let x = rect.x + 3.0 + t * (rect.w - knob - 6.0);
                // The knob is the lightest thing in the theme, so it reads on
                // the accent in light themes and dark alike.
                let light = if look.base.luminance() > look.text.luminance() {
                    look.base
                } else {
                    look.text
                };
                let knob_color = if state.hovered {
                    light
                } else {
                    light.mix(look.subtle, 0.12)
                };
                canvas.circle(
                    x + knob / 2.0,
                    rect.y + rect.h / 2.0,
                    knob / 2.0,
                    knob_color,
                );
            },
        );
        self.knob.is_animating(now)
    }
}

/// Draw a dialog: the window dimmed, and a card of `width × height` in the
/// middle of `bounds`, sliding up a little as `presence` goes `0 → 1`.
/// Returns the card's inside, padded.
pub fn dialog(
    canvas: &mut Canvas<'_>,
    bounds: Rect,
    width: f32,
    height: f32,
    look: &Look,
    presence: f32,
) -> Rect {
    canvas.fill(bounds, 0.0, look.base.alpha(opacity::SCRIM * presence));
    let width = width.min(bounds.w - 2.0 * space::LG);
    let height = height.min(bounds.h - 2.0 * space::LG);
    let card = bounds
        .centered(width, height)
        .translate(0.0, (1.0 - presence) * space::MD);
    canvas.with_opacity(presence, |canvas| {
        canvas.shadow(card, radius::CARD, Shadow::FLOATING, Rgba::rgb(0, 0, 0));
        canvas.fill(card, radius::CARD, look.surface);
        canvas.stroke(card, radius::CARD, stroke::HAIRLINE, look.highlight_med);
    });
    card.inset(space::XL)
}

/// When a tooltip shows: after the pointer has rested on one thing.
#[derive(Clone, Debug, Default)]
pub struct Tooltip<T> {
    /// What the pointer is resting on, and since when.
    over: Option<(T, Instant)>,
}

/// How long the pointer rests before a tooltip appears.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

impl<T: PartialEq> Tooltip<T> {
    /// The pointer is over `target` (or nothing). Returns how long until
    /// the tooltip is due, for a wake-up, if one is newly waiting.
    pub fn hover(&mut self, target: Option<T>, now: Instant) -> Option<Duration> {
        match (&self.over, target) {
            (Some((current, _)), Some(target)) if *current == target => None,
            (_, Some(target)) => {
                self.over = Some((target, now));
                Some(TOOLTIP_DELAY)
            }
            (_, None) => {
                self.over = None;
                None
            }
        }
    }

    /// What to show a tooltip for now, if anything.
    pub fn due(&self, now: Instant) -> Option<&T> {
        self.over
            .as_ref()
            .filter(|(_, since)| now.saturating_duration_since(*since) >= TOOLTIP_DELAY)
            .map(|(target, _)| target)
    }
}

/// Draw a tooltip saying `text` just below `anchor`, kept inside `bounds`.
pub fn tooltip(canvas: &mut Canvas<'_>, text: &str, anchor: Rect, bounds: Rect, look: &Look) {
    let label = Text::new(text, look.size * 0.9, look.text)
        .max_width(bounds.w * 0.6)
        .max_lines(4);
    let size = canvas.measure(&label);
    let (w, h) = (size.w + 2.0 * space::MD, size.h + 2.0 * space::SM);
    let x = (anchor.x).clamp(
        bounds.x + space::SM,
        (bounds.right() - w - space::SM).max(bounds.x),
    );
    let below = anchor.bottom() + space::XS;
    let y = if below + h > bounds.bottom() - space::SM {
        anchor.y - h - space::XS
    } else {
        below
    };
    let rect = Rect::new(x, y, w, h);
    canvas.shadow(rect, radius::CONTROL, Shadow::RAISED, Rgba::rgb(0, 0, 0));
    canvas.fill(rect, radius::CONTROL, look.overlay);
    canvas.text(&label, rect.x + space::MD, rect.y + space::SM);
}

/// A divider that can be dragged: between a sidebar and a list, say.
#[derive(Clone, Copy, Debug)]
pub struct Splitter {
    /// Where the divider is, in pixels from the start of its axis.
    pub at: f32,
    pub min: f32,
    pub max: f32,
    /// While dragging: how far from the divider the pointer grabbed it.
    grab: Option<f32>,
}

/// How wide the grabbable strip is, either side of the divider.
pub const SPLITTER_GRAB: f32 = 4.0;

impl Splitter {
    pub fn new(at: f32, min: f32, max: f32) -> Self {
        Self {
            at: at.clamp(min, max),
            min,
            max,
            grab: None,
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.grab.is_some()
    }

    /// Whether the pointer at `pos` (along the axis) is on the divider.
    pub fn is_over(&self, pos: f32) -> bool {
        (pos - self.at).abs() <= SPLITTER_GRAB
    }

    /// A press at `pos`: starts a drag if it is on the divider.
    pub fn press(&mut self, pos: f32) -> bool {
        if self.is_over(pos) {
            self.grab = Some(pos - self.at);
        }
        self.grab.is_some()
    }

    /// The pointer moved to `pos`. Returns whether the divider moved.
    pub fn drag(&mut self, pos: f32) -> bool {
        let Some(grab) = self.grab else { return false };
        let at = (pos - grab).clamp(self.min, self.max);
        let moved = at != self.at;
        self.at = at;
        moved
    }

    pub fn release(&mut self) {
        self.grab = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltips_wait_for_the_pointer_to_rest() {
        let mut tip = Tooltip::default();
        let start = Instant::now();
        assert_eq!(tip.hover(Some(1), start), Some(TOOLTIP_DELAY));
        // Still on the same thing: no new wait.
        assert_eq!(tip.hover(Some(1), start + Duration::from_millis(100)), None);
        assert_eq!(tip.due(start + Duration::from_millis(300)), None);
        assert_eq!(tip.due(start + Duration::from_millis(600)), Some(&1));
        // Moving on starts over.
        assert!(
            tip.hover(Some(2), start + Duration::from_millis(700))
                .is_some()
        );
        assert_eq!(tip.due(start + Duration::from_millis(800)), None);
        tip.hover(None, start);
        assert_eq!(tip.due(start + Duration::from_secs(5)), None);
    }

    #[test]
    fn splitters_drag_within_their_limits() {
        let mut split = Splitter::new(220.0, 160.0, 400.0);
        assert!(!split.press(300.0));
        assert!(split.press(222.0));
        assert!(split.drag(262.0));
        assert_eq!(split.at, 260.0);
        split.drag(1000.0);
        assert_eq!(split.at, 400.0);
        split.drag(0.0);
        assert_eq!(split.at, 160.0);
        split.release();
        assert!(!split.drag(300.0));
    }

    #[test]
    fn toggles_slide() {
        let now = Instant::now();
        let mut toggle = Toggle::new(false);
        toggle.set(true, Transition::TOGGLE, now);
        assert!(toggle.is_on());
        assert!(toggle.knob.is_animating(now + Duration::from_millis(50)));
        assert!(!toggle.knob.is_animating(now + Duration::from_secs(1)));
    }
}
