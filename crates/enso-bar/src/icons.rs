//! The status island's icons: Wi-Fi (or a cable), the bell — enso's own
//! drawings ([`enso_ui::figure`], the same everywhere in the suite) — and
//! the battery, which is the bar's alone: a pill holding its percentage.
//!
//! Here the icons are fitted by their ink rather than placed on their grid:
//! each stands exactly as tall as the next and as the battery, and takes
//! exactly as much width as it has ink, so they line up along the island
//! and the gaps between them are even. Every measure comes from the bar's
//! height (see [`Scale`]), so the island grows and shrinks with the bar.

use std::f32::consts::PI;

use enso_core::{Figure, Palette, Rgba};
use enso_ui::{
    Align, Canvas, Rect, Text,
    figure::{self, Look},
};

/// The bar's island height the icons were drawn for; every size scales by
/// the island's own height over this.
const DESIGN_HEIGHT: f32 = 28.0;

/// How much of an unlit part's colour is the ground's: "off" is the icon's
/// colour, faded into the island behind it.
pub const UNLIT: f32 = 0.62;

/// The pen for status icons: a little heavier than elsewhere, because they
/// are small.
const PEN: f32 = figure::PEN * 1.2;

/// Sizes for one bar height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    unit: f32,
}

impl Scale {
    /// For an island `height` logical pixels tall.
    pub fn new(height: f32) -> Self {
        Self {
            unit: (height / DESIGN_HEIGHT).max(0.5),
        }
    }

    /// `px` at the design height, at this one.
    pub fn px(self, px: f32) -> f32 {
        px * self.unit
    }

    /// The height of an icon: half the island's, so the icons sit in it
    /// with room around them rather than filling it.
    pub fn icon(self) -> f32 {
        self.px(14.0).round()
    }

    /// The width a figure takes.
    pub fn width(self, figure: Figure) -> f32 {
        (self.icon() * figure::aspect(figure, PEN)).round()
    }

    /// Space between neighbouring icons.
    pub fn gap(self) -> f32 {
        self.px(10.0).round()
    }

    /// Space between the island's ends and its first and last icon.
    pub fn pad(self) -> f32 {
        self.px(12.0).round()
    }
}

/// The colours an icon is drawn in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink {
    /// The parts that are on.
    pub on: Rgba,
    /// What it sits on: the island's colour, as lit as it is, before any
    /// see-through. Unlit parts are the icon's colour faded into it.
    pub ground: Rgba,
}

impl Ink {
    fn look(self) -> Look {
        Look {
            on: self.on,
            off: self.on.mix(self.ground, UNLIT),
            pen: PEN,
        }
    }
}

/// How far a figure is drawn above centre, in whole pixels for an icon
/// `height` tall: an optical correction. Wi-Fi meets its top line at a
/// single point, where the others meet it all along, and centred by its box
/// it reads as hanging low.
fn lift(figure: Figure, height: f32) -> f32 {
    match figure {
        Figure::Wifi(_) | Figure::WifiOff => (height * 0.07).round(),
        _ => 0.0,
    }
}

/// Draw a status figure in `rect` (as wide as [`Scale::width`] said).
pub fn draw(canvas: &mut Canvas<'_>, figure: Figure, rect: Rect, ink: Ink) {
    let rect = Rect {
        y: rect.y - lift(figure, rect.h),
        ..rect
    };
    figure::draw_fitted(canvas, figure, rect, ink.look(), 0.0);
}

/// [`draw`], at `t` seconds: for a figure that moves, the cup's steam.
pub fn draw_at(canvas: &mut Canvas<'_>, figure: Figure, rect: Rect, ink: Ink, t: f32) {
    let live = figure::Live {
        time: Some(t),
        amount: None,
    };
    draw_live(canvas, figure, rect, ink, live);
}

/// [`draw`], at a moment or an amount (see [`figure::Live`]).
pub fn draw_live(
    canvas: &mut Canvas<'_>,
    figure: Figure,
    rect: Rect,
    ink: Ink,
    live: figure::Live,
) {
    let rect = Rect {
        y: rect.y - lift(figure, rect.h),
        ..rect
    };
    figure::draw_fitted_live(canvas, figure, rect, ink.look(), 0.0, live);
}

/// The bell, swung `swing` radians about its knob. With `dot`, a small dot
/// in that colour sits just clear of its shoulder — no ring is cut around
/// it, because the island behind may be see-through and a ring would show.
pub fn bell(canvas: &mut Canvas<'_>, rect: Rect, swing: f32, dot: Option<Rgba>, ink: Ink) {
    figure::draw_fitted(canvas, Figure::Bell, rect, ink.look(), swing);
    if let Some(color) = dot {
        let (cx, _) = rect.center();
        canvas.circle(
            cx + rect.h * 0.47,
            rect.y + rect.h * 0.12,
            rect.h * 0.15,
            color,
        );
    }
}

/// The bell's ring: a swing that dies away over `t` (`0..=1`), a few times
/// to each side.
pub fn ring(t: f32) -> f32 {
    if !(0.0..1.0).contains(&t) {
        return 0.0;
    }
    0.32 * (t * 3.0 * 2.0 * PI).sin() * (1.0 - t).powi(2)
}

/// How the battery reads: its charge, whether it is on power, and the
/// colour of its figures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Charge {
    pub percent: u8,
    pub plugged: bool,
    /// The figures' colour: the text's, gold as it runs low, love when it
    /// is nearly out.
    pub ink: Rgba,
}

/// The battery's sizes, at a scale: its percentage in words, and a small
/// bolt before it while it is on power.
#[derive(Clone, Copy, Debug)]
pub struct Battery {
    scale: Scale,
}

/// The bolt as a shape in a unit square: a zig-zag, broad at the top.
const BOLT_SHAPE: [(f32, f32); 6] = [
    (0.62, 0.0),
    (0.0, 0.58),
    (0.44, 0.58),
    (0.34, 1.0),
    (1.0, 0.40),
    (0.56, 0.40),
];

impl Battery {
    pub fn new(scale: Scale) -> Self {
        Self { scale }
    }

    /// The bolt's width and height — as tall as a figure — and the space
    /// between it and the figures.
    fn bolt(self) -> (f32, f32, f32) {
        let h = (self.scale.icon() * 0.78).round();
        (h * 0.62, h, (self.scale.icon() * 0.22).round())
    }

    /// The figures' size: the clock's, on a bar of the usual height, and
    /// growing with the bar as the icons do.
    pub fn figures(self) -> f32 {
        self.scale.icon() * 0.86
    }

    /// The whole width, for words this wide.
    pub fn width(self, words: f32, plugged: bool) -> f32 {
        let (bolt, _, gap) = self.bolt();
        let bolt = if plugged { bolt + gap } else { 0.0 };
        (bolt + words).ceil()
    }

    /// Draw it in `slot`: the bolt (on power) in foam, the figures in their
    /// colour, and the percent sign a step quieter, as a unit is.
    pub fn draw(
        self,
        canvas: &mut Canvas<'_>,
        slot: Rect,
        charge: Charge,
        figures: &Text<'_>,
        glow: f32,
        colors: &Palette,
    ) {
        let (bolt_w, bolt_h, gap) = self.bolt();
        let (_, middle) = slot.center();
        let mut x = slot.x;
        if charge.plugged {
            let y = (middle - bolt_h / 2.0).round();
            let shape = BOLT_SHAPE.map(|(u, v)| (x + u * bolt_w, y + v * bolt_h));
            canvas.polygon(&shape, colors.foam.mix(colors.text, glow * 0.25));
            x += bolt_w + gap;
        }
        let number = Text {
            color: charge.ink.mix(colors.text, glow * 0.5),
            ..*figures
        };
        let room = Rect::new(x, slot.y, slot.right() - x, slot.h);
        let said = canvas.text_in(&number, room, Align::Start);
        let sign = Text::new("%", figures.size, colors.subtle.mix(colors.text, glow * 0.5));
        let at = x + said.w;
        canvas.text_in(&sign, Rect::new(at, slot.y, slot.right() - at, slot.h), Align::Start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_follow_the_bar() {
        let normal = Scale::new(28.0);
        assert_eq!(normal.icon(), 14.0);
        assert_eq!(normal.gap(), 10.0);
        let tall = Scale::new(56.0);
        assert_eq!(tall.icon(), 28.0);
        // Twice the bar, twice the icon (to the pixel each is rounded to).
        assert!((tall.width(Figure::Wifi(4)) - 2.0 * normal.width(Figure::Wifi(4))).abs() <= 1.0);
        assert!(Battery::new(tall).width(20.0, true) > Battery::new(normal).width(20.0, true));
        // Never smaller than half the design, however small the bar.
        assert_eq!(Scale::new(4.0).icon(), 7.0);
    }

    /// Every status icon's ink fills its box exactly — as tall as the
    /// others, and as wide as it is — so they line up along the island and
    /// the gaps between them are even. Measured from what is drawn.
    #[test]
    fn every_icon_fills_its_box() {
        use enso_core::{Context, Paths};
        use enso_ui::{Offscreen, TextEngine};

        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-icons"));
        let mut text = TextEngine::new(&ctx.config.appearance);
        // The ground is clear, so only the icon's own ink is seen.
        let ink = Ink {
            on: Rgba::rgb(255, 255, 255),
            ground: Rgba::new(0, 0, 0, 0),
        };
        let h = 100.0;
        for figure in [
            Figure::Wifi(4),
            Figure::Wifi(1),
            Figure::Ethernet,
            Figure::Bell,
        ] {
            let w = (h * figure::aspect(figure, PEN)).round();
            let rect = Rect::new(40.0, 40.0, w, h);
            let image = Offscreen::render(&mut text, (260, 180), 1, |canvas| {
                if figure == Figure::Bell {
                    bell(canvas, rect, 0.0, None, ink);
                } else {
                    draw(canvas, figure, rect, ink);
                }
            });
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
            for y in 0..image.height {
                for x in 0..image.width {
                    if image.pixel(x, y)[3] > 64 {
                        (x0, x1, y0, y1) = (x0.min(x), x1.max(x + 1), y0.min(y), y1.max(y + 1));
                    }
                }
            }
            let near = |drawn: u32, wanted: f32| (drawn as f32 - wanted).abs() <= 2.0;
            let lift = lift(figure, h);
            assert!(
                near(x0, rect.x)
                    && near(x1, rect.right())
                    && near(y0, rect.y - lift)
                    && near(y1, rect.bottom() - lift),
                "{figure:?}: ink {x0}..{x1} × {y0}..{y1}, box {rect:?}"
            );
        }
    }

    /// Only Wi-Fi is lifted, by whole pixels, and more on a taller bar.
    #[test]
    fn wifi_is_lifted_a_whole_pixel() {
        assert_eq!(lift(Figure::Wifi(3), 14.0), 1.0);
        assert_eq!(lift(Figure::WifiOff, 28.0), 2.0);
        for figure in [Figure::Ethernet, Figure::Bell] {
            assert_eq!(lift(figure, 14.0), 0.0);
        }
    }

    #[test]
    fn the_ring_dies_away() {
        assert_eq!(ring(0.0), 0.0);
        assert_eq!(ring(1.0), 0.0);
        assert_eq!(ring(-0.5), 0.0);
        let early = (0..20)
            .map(|i| ring(i as f32 / 100.0).abs())
            .fold(0.0, f32::max);
        let late = (80..100)
            .map(|i| ring(i as f32 / 100.0).abs())
            .fold(0.0, f32::max);
        assert!(early > 0.1 && late < 0.02, "{early} {late}");
    }
}
