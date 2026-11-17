//! The status island's icons, drawn rather than taken from a font: Wi-Fi
//! (or a cable), the speaker, the battery, and the bell.
//!
//! They share one hand. Lines are thick and round at their ends and
//! corners; shapes are soft-cornered and a little plump. Strength — signal,
//! volume — is told the same way in both: the parts that are on are drawn
//! in the icon's colour, the parts that are off in a whisper of it, so the
//! shape stays whole and only its fill changes.
//!
//! Every measure is a fraction of the icon's height, and that height comes
//! from the bar's (see [`Scale`]), so the island grows and shrinks with the
//! bar and stays sharp at any size and on any screen.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use enso_core::{Palette, Rgba};
use enso_ui::{Align, Canvas, Rect, Shape, Text};

/// The bar's island height the icons were drawn for; every size scales by
/// the island's own height over this.
const DESIGN_HEIGHT: f32 = 28.0;

/// How much of an unlit part's colour is the ground's: "off" is the icon's
/// colour, faded into the island behind it. Opaque, so parts that overlap do
/// not darken where they meet.
const UNLIT: f32 = 0.72;

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

    /// The height of an icon.
    pub fn icon(self) -> f32 {
        self.px(16.0).round()
    }

    /// The width each icon takes.
    pub fn width(self, icon: Icon) -> f32 {
        (self.icon() * icon.aspect()).round()
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

/// Which icon, for its proportions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Wifi,
    Wired,
    Speaker,
    Bell,
}

/// Where an icon's ink falls, in the units it is drawn in: `u` across and
/// `v` down, strokes and round ends included.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Extent {
    u: (f32, f32),
    v: (f32, f32),
}

impl Icon {
    /// The ink of each icon, worked out from its drawing below: the outermost
    /// curve or corner, plus half its pen. Every icon is fitted by this, so
    /// all of them stand exactly as tall as each other, and each takes
    /// exactly as much width as it has ink — the gaps between them are then
    /// all the same, to the eye as well as to the ruler.
    fn extent(self) -> Extent {
        match self {
            // The outer arc's ends (0.72 at 45°) and top, plus half its
            // 0.13 pen; the dot's foot below.
            Self::Wifi => Extent {
                u: (-0.574, 0.574),
                v: (0.095, 0.94),
            },
            // The outer squares; the top one's head and the feet.
            Self::Wired => Extent {
                u: (-0.47, 0.47),
                v: (0.06, 0.94),
            },
            // The cone's back, plus half its 0.1 pen; the outer wave (and
            // the cross, which reaches as far) at the front; the outer
            // wave's ends above and below.
            Self::Speaker => Extent {
                u: (-0.01, 1.035),
                v: (0.054, 0.946),
            },
            // The rim's ends; the knob's top and the clapper's foot.
            Self::Bell => Extent {
                u: (-0.4, 0.4),
                v: (0.025, 1.005),
            },
        }
    }

    /// Width over height, of the ink.
    fn aspect(self) -> f32 {
        let Extent { u, v } = self.extent();
        (u.1 - u.0) / (v.1 - v.0)
    }
}

/// The colours an icon is drawn in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink {
    /// The parts that are on.
    pub on: Rgba,
    /// What it sits on: the island, as lit as it is.
    pub ground: Rgba,
}

impl Ink {
    fn off(self) -> Rgba {
        self.on.mix(self.ground, UNLIT)
    }
}

/// An icon's drawing units laid onto a box: its ink's top at the box's
/// top, its foot at the box's foot, and centred across.
#[derive(Clone, Copy)]
struct Frame {
    /// Where `(u, v) = (0, 0)` lands.
    x: f32,
    y: f32,
    /// Logical pixels per unit.
    k: f32,
}

impl Frame {
    fn fit(rect: Rect, icon: Icon) -> Self {
        let Extent { u, v } = icon.extent();
        let k = rect.h / (v.1 - v.0);
        let ink = (u.1 - u.0) * k;
        Self {
            x: rect.x + (rect.w - ink) / 2.0 - u.0 * k,
            y: rect.y - v.0 * k,
            k,
        }
    }

    fn at(self, u: f32, v: f32) -> (f32, f32) {
        (self.x + u * self.k, self.y + v * self.k)
    }

    fn len(self, units: f32) -> f32 {
        units * self.k
    }
}

/// Wi-Fi: a dot and three arcs fanning up from it. `bars` (`0..=4`) lights
/// the dot and then the arcs from the inside out; `None` is offline — every
/// part unlit, and a slash across.
pub fn wifi(canvas: &mut Canvas<'_>, rect: Rect, bars: Option<u8>, ink: Ink) {
    let f = Frame::fit(rect, Icon::Wifi);
    let origin = f.at(0.0, 0.88);
    let lit = bars.map_or(0, |bars| bars.clamp(1, 4));
    let paint = |n: u8| if n < lit { ink.on } else { ink.off() };

    // The dot.
    let mut dot = Shape::new();
    dot.circle((origin.0, origin.1 - f.len(0.04)), f.len(0.1));
    canvas.fill_shape(&dot, paint(0));
    // The arcs: a quarter turn each, centred on straight up.
    for (n, radius) in [(1, 0.28), (2, 0.5), (3, 0.72)] {
        let mut arc = Shape::new();
        arc.arc(origin, f.len(radius), -FRAC_PI_2 - FRAC_PI_4, FRAC_PI_2);
        canvas.stroke_shape(&arc, f.len(0.13), paint(n));
    }
    if bars.is_none() {
        slash(canvas, f, ink);
    }
}

/// A cable: three soft squares, joined like a little family tree.
pub fn wired(canvas: &mut Canvas<'_>, rect: Rect, ink: Ink) {
    let f = Frame::fit(rect, Icon::Wired);
    let square = |canvas: &mut Canvas<'_>, u: f32, v: f32| {
        let (x, y) = f.at(u - 0.15, v);
        let mut shape = Shape::new();
        shape.rounded_rect(Rect::new(x, y, f.len(0.3), f.len(0.28)), f.len(0.08));
        canvas.fill_shape(&shape, ink.on);
    };
    square(canvas, 0.0, 0.06);
    square(canvas, -0.32, 0.66);
    square(canvas, 0.32, 0.66);
    let mut lines = Shape::new();
    let (x, y) = f.at(0.0, 0.34);
    lines.move_to(x, y);
    let (x, y) = f.at(0.0, 0.5);
    lines.line_to(x, y);
    let (x0, y0) = f.at(-0.32, 0.68);
    let (x1, y1) = f.at(-0.32, 0.5);
    let (x2, y2) = f.at(0.32, 0.5);
    let (x3, y3) = f.at(0.32, 0.68);
    lines
        .move_to(x0, y0)
        .line_to(x1, y1)
        .line_to(x2, y2)
        .line_to(x3, y3);
    canvas.stroke_shape(&lines, f.len(0.1), ink.on);
}

/// The speaker: a soft cone, with `waves` (`0..=3`) of its three sound
/// waves lit; `None` is muted — the waves give way to a small cross.
pub fn speaker(canvas: &mut Canvas<'_>, rect: Rect, waves: Option<u8>, ink: Ink) {
    let f = Frame::fit(rect, Icon::Speaker);
    let point = |u: f32, v: f32| f.at(u, v);
    let mut cone = Shape::new();
    let corners = [
        (0.04, 0.36),
        (0.24, 0.36),
        (0.48, 0.14),
        (0.48, 0.86),
        (0.24, 0.64),
        (0.04, 0.64),
    ];
    let (x, y) = point(corners[0].0, corners[0].1);
    cone.move_to(x, y);
    for (u, v) in &corners[1..] {
        let (x, y) = point(*u, *v);
        cone.line_to(x, y);
    }
    cone.close();
    // Filled, then traced with a round pen: the corners come out soft.
    canvas.fill_shape(&cone, ink.on);
    canvas.stroke_shape(&cone, f.len(0.1), ink.on);

    let centre = point(0.42, 0.5);
    match waves {
        Some(lit) => {
            for (n, radius) in [(0_u8, 0.22), (1, 0.39), (2, 0.56)] {
                let mut wave = Shape::new();
                wave.arc(centre, f.len(radius), -FRAC_PI_4, FRAC_PI_2);
                let color = if n < lit { ink.on } else { ink.off() };
                canvas.stroke_shape(&wave, f.len(0.1), color);
            }
        }
        None => {
            let (cx, cy) = point(0.84, 0.5);
            let reach = f.len(0.14);
            let mut cross = Shape::new();
            cross
                .move_to(cx - reach, cy - reach)
                .line_to(cx + reach, cy + reach)
                .move_to(cx + reach, cy - reach)
                .line_to(cx - reach, cy + reach);
            canvas.stroke_shape(&cross, f.len(0.11), ink.on);
        }
    }
}

/// The bell: a plump dome on a flared rim, a knob on top and a clapper
/// below. `swing` (radians) tilts it about the knob — the bell ringing.
/// With `dot`, a small dot in that colour sits on its shoulder: something
/// is waiting.
pub fn bell(canvas: &mut Canvas<'_>, rect: Rect, swing: f32, dot: Option<Rgba>, ink: Ink) {
    let f = Frame::fit(rect, Icon::Bell);
    let pivot = f.at(0.0, 0.06);
    let (sin, cos) = swing.sin_cos();
    let at = |u: f32, v: f32| {
        let (x, y) = f.at(u, v);
        let (dx, dy) = (x - pivot.0, y - pivot.1);
        (pivot.0 + dx * cos - dy * sin, pivot.1 + dx * sin + dy * cos)
    };
    let mut body = Shape::new();
    let mut path = |points: &[(f32, f32)]| {
        // A curve per three points, after the first.
        let (x, y) = at(points[0].0, points[0].1);
        body.move_to(x, y);
        for curve in points[1..].chunks(3) {
            let [(u1, v1), (u2, v2), (u3, v3)] = [curve[0], curve[1], curve[2]];
            let ((x1, y1), (x2, y2), (x3, y3)) = (at(u1, v1), at(u2, v2), at(u3, v3));
            body.cubic_to(x1, y1, x2, y2, x3, y3);
        }
        body.close();
    };
    path(&[
        (0.0, 0.13),
        // Right shoulder, and down the side.
        (0.2, 0.13),
        (0.3, 0.27),
        (0.3, 0.44),
        (0.3, 0.52),
        (0.3, 0.58),
        (0.3, 0.6),
        // The flare.
        (0.3, 0.68),
        (0.4, 0.7),
        (0.4, 0.77),
        // The rim, softly cornered.
        (0.4, 0.81),
        (0.37, 0.82),
        (0.33, 0.82),
        (0.1, 0.82),
        (-0.1, 0.82),
        (-0.33, 0.82),
        (-0.37, 0.82),
        (-0.4, 0.81),
        (-0.4, 0.77),
        // Up the left side.
        (-0.4, 0.7),
        (-0.3, 0.68),
        (-0.3, 0.6),
        (-0.3, 0.58),
        (-0.3, 0.52),
        (-0.3, 0.44),
        (-0.3, 0.27),
        (-0.2, 0.13),
        (0.0, 0.13),
    ]);
    canvas.fill_shape(&body, ink.on);
    let mut knob = Shape::new();
    knob.circle(at(0.0, 0.1), f.len(0.075));
    // The clapper swings a little further than the bell.
    let clapper = {
        let (sin, cos) = (swing * 1.6).sin_cos();
        let (x, y) = f.at(0.0, 0.92);
        let (dx, dy) = (x - pivot.0, y - pivot.1);
        (pivot.0 + dx * cos - dy * sin, pivot.1 + dx * sin + dy * cos)
    };
    knob.circle(clapper, f.len(0.085));
    canvas.fill_shape(&knob, ink.on);

    if let Some(color) = dot {
        let (x, y) = f.at(0.34, 0.2);
        canvas.circle(x, y, f.len(0.24), ink.ground);
        canvas.circle(x, y, f.len(0.16), color);
    }
}

/// A slash across an icon, cut out of what it crosses.
fn slash(canvas: &mut Canvas<'_>, f: Frame, ink: Ink) {
    let mut line = Shape::new();
    let (x0, y0) = f.at(-0.42, 0.1);
    let (x1, y1) = f.at(0.42, 0.92);
    line.move_to(x0, y0).line_to(x1, y1);
    canvas.stroke_shape(&line, f.len(0.26), ink.ground);
    canvas.stroke_shape(&line, f.len(0.11), ink.on);
}

/// The bell's ring: a swing that dies away over `t` (`0..=1`), a few times
/// to each side.
pub fn ring(t: f32) -> f32 {
    if !(0.0..1.0).contains(&t) {
        return 0.0;
    }
    0.32 * (t * 3.0 * 2.0 * PI).sin() * (1.0 - t).powi(2)
}

/// How the battery is drawn: its charge, whether it is on power, and the
/// colour of its fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Charge {
    pub percent: u8,
    pub plugged: bool,
    pub fill: Rgba,
}

/// The battery pill's sizes, at a scale.
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

    /// As tall as every other icon: the pill reads as one of them.
    fn height(self) -> f32 {
        self.scale.icon()
    }

    fn pad(self) -> f32 {
        self.scale.px(6.0)
    }

    /// The terminal cap after the pill: width, height, and the gap before it.
    /// Whole pixels, so that everything after the battery stays on them.
    fn nub(self) -> (f32, f32, f32) {
        let whole = |px: f32| self.scale.px(px).round().max(1.0);
        (whole(2.0), whole(6.0), whole(2.0))
    }

    /// The charging bolt's size, and the space between it and the figures.
    fn bolt(self) -> (f32, f32, f32) {
        (self.scale.px(6.0), self.scale.px(10.0), self.scale.px(2.0))
    }

    /// The figures' size: they fill the pill as the bar's text fills it.
    pub fn figures(self) -> f32 {
        self.scale.px(10.4)
    }

    /// The whole width, cap included, for figures this wide.
    pub fn width(self, figures: f32, plugged: bool) -> f32 {
        let (bolt, _, bolt_gap) = self.bolt();
        let bolt = if plugged { bolt + bolt_gap } else { 0.0 };
        let (nub, _, nub_gap) = self.nub();
        (figures + bolt + 2.0 * self.pad())
            .max(self.scale.px(30.0))
            .round()
            + nub
            + nub_gap
    }

    /// Draw it in `slot` (as wide as [`width`](Self::width) said): a pill on
    /// a quiet track, filled from the left to its charge and cut off
    /// straight, with the bolt (on power) and the figures centred in it.
    /// Figures and bolt are drawn twice, each time confined to one side of
    /// the fill's edge — in the ground's colour over the fill, in the text
    /// colour over the track — so a figure the edge runs through is split
    /// cleanly.
    pub fn draw(
        self,
        canvas: &mut Canvas<'_>,
        slot: Rect,
        charge: Charge,
        label: &Text<'_>,
        glow: f32,
        colors: &Palette,
    ) {
        let (nub_w, nub_h, nub_gap) = self.nub();
        let height = self.height();
        let pill = Rect::new(
            slot.x,
            slot.y + ((slot.h - height) / 2.0).round(),
            slot.w - nub_w - nub_gap,
            height,
        );
        let track = colors.highlight_med;
        canvas.pill(pill, track);
        canvas.fill(
            Rect::new(
                pill.right() + nub_gap,
                pill.y + (pill.h - nub_h) / 2.0,
                nub_w,
                nub_h,
            ),
            nub_w / 2.0,
            track,
        );

        let level = pill.w * f32::from(charge.percent.min(100)) / 100.0;
        let filled = Rect::new(pill.x, pill.y, level, pill.h);
        let empty = Rect::new(pill.x + level, pill.y, pill.w - level, pill.h);
        let fill = charge.fill.mix(colors.text, glow * 0.25);
        canvas.with_clip(filled, |canvas| canvas.pill(pill, fill));

        let figures = canvas.measure(label).w;
        let (bolt_w, bolt_h, bolt_gap) = self.bolt();
        let bolt = if charge.plugged {
            bolt_w + bolt_gap
        } else {
            0.0
        };
        let start = pill.x + (pill.w - figures - bolt) / 2.0;
        let (_, middle) = pill.center();
        for (side, ink) in [(filled, colors.base), (empty, colors.text)] {
            canvas.with_clip(side, |canvas| {
                if charge.plugged {
                    let (x, y) = (start, middle - bolt_h / 2.0);
                    let shape = BOLT_SHAPE.map(|(u, v)| (x + u * bolt_w, y + v * bolt_h));
                    canvas.polygon(&shape, ink);
                }
                let text = Text {
                    color: ink,
                    ..*label
                };
                canvas.text_in(
                    &text,
                    Rect::new(start + bolt, pill.y, figures + 1.0, pill.h),
                    Align::Start,
                );
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_follow_the_bar() {
        let normal = Scale::new(28.0);
        assert_eq!(normal.icon(), 16.0);
        assert_eq!(normal.gap(), 10.0);
        let tall = Scale::new(56.0);
        assert_eq!(tall.icon(), 32.0);
        // Twice the bar, twice the icon (to the pixel each is rounded to).
        assert!((tall.width(Icon::Wifi) - 2.0 * normal.width(Icon::Wifi)).abs() <= 1.0);
        assert!(Battery::new(tall).width(20.0, true) > Battery::new(normal).width(20.0, true));
        // Never smaller than half the design, however small the bar.
        assert_eq!(Scale::new(4.0).icon(), 8.0);
    }

    /// Every icon's ink fills its box exactly — as tall as the others, and
    /// as wide as it is — so they line up along the island and the gaps
    /// between them are even. Measured from what is actually drawn.
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
        // Muted, the waves are gone and the cone alone stands a little
        // shorter — centred where the whole speaker is, not stretched.
        let cone = (0.09 - 0.054) / (0.946 - 0.054) * h;
        /// A name, the icon, how far its ink stands in from top and foot,
        /// and how to draw it.
        type Drawing<'a> = (&'a str, Icon, f32, &'a dyn Fn(&mut Canvas<'_>, Rect));
        let drawings: [Drawing<'_>; 7] = [
            ("wifi", Icon::Wifi, 0.0, &|c, r| wifi(c, r, Some(4), ink)),
            ("weak wifi", Icon::Wifi, 0.0, &|c, r| {
                wifi(c, r, Some(1), ink)
            }),
            ("wired", Icon::Wired, 0.0, &|c, r| wired(c, r, ink)),
            ("speaker", Icon::Speaker, 0.0, &|c, r| {
                speaker(c, r, Some(3), ink)
            }),
            ("quiet speaker", Icon::Speaker, 0.0, &|c, r| {
                speaker(c, r, Some(1), ink)
            }),
            ("muted", Icon::Speaker, cone, &|c, r| {
                speaker(c, r, None, ink)
            }),
            ("bell", Icon::Bell, 0.0, &|c, r| bell(c, r, 0.0, None, ink)),
        ];
        for (name, icon, inset, draw) in drawings {
            let rect = Rect::new(40.0, 40.0, (h * icon.aspect()).round(), h);
            let image = Offscreen::render(&mut text, (260, 180), 1, |canvas| draw(canvas, rect));
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
            for y in 0..image.height {
                for x in 0..image.width {
                    if image.pixel(x, y)[3] > 64 {
                        (x0, x1, y0, y1) = (x0.min(x), x1.max(x + 1), y0.min(y), y1.max(y + 1));
                    }
                }
            }
            let near = |drawn: u32, wanted: f32| (drawn as f32 - wanted).abs() <= 2.0;
            assert!(
                near(x0, rect.x)
                    && near(x1, rect.right())
                    && near(y0, rect.y + inset)
                    && near(y1, rect.bottom() - inset),
                "{name}: ink {x0}..{x1} × {y0}..{y1}, box {rect:?}"
            );
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
