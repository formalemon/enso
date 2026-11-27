//! enso's icons, drawn: one hand for every tool.
//!
//! Each [`Figure`] is drawn on a 24-unit grid, the way icon sets are: the
//! drawing keeps to the middle 20 units or so, which leaves every icon the
//! same breathing room inside its box, and the same optical size beside
//! its neighbours. Lines are drawn with one pen, round at their ends and
//! where they turn; corners are softened; small solid parts — dots, the
//! bell — are filled.
//!
//! An icon is either placed on its grid in a square box ([`draw`]), which
//! is what rows, buttons, and tiles want, or fitted by its ink to a box's
//! height ([`draw_fitted`]), which is what a strip of status icons wants:
//! there, every icon stands exactly as tall as the next.
//!
//! Strength — Wi-Fi's signal, a speaker's waves — is told by lighting parts
//! and leaving the rest faint ([`Look::off`]), so the shape stays whole.

use std::{f32::consts::PI, time::Duration};

use enso_core::{Figure, Rgba};

use crate::{Canvas, Rect, Shape};

/// The grid an icon is drawn on.
const GRID: f32 = 24.0;
/// The pen: how thick lines are, in grid units.
pub const PEN: f32 = 2.0;
/// How much of the colour unlit parts keep, by default.
const FAINT: f32 = 0.4;

/// The colours and pen an icon is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The parts that are on.
    pub on: Rgba,
    /// The parts that are off: a whisper of `on`.
    pub off: Rgba,
    /// Line thickness, in grid units.
    pub pen: f32,
}

impl Look {
    /// `color` for what is on, a faint `color` for what is off, the usual
    /// pen.
    pub fn new(color: Rgba) -> Self {
        Self {
            on: color,
            off: color.alpha(FAINT),
            pen: PEN,
        }
    }
}

/// What a figure that moves, or shows how much of something, is drawn at:
/// the time, in seconds, for one that moves — the steam over a cup drifts —
/// and an amount (`0..=1`) for one that shows it — the sun's rays at a
/// brightness, a lock's shackle as it lifts. Figures that neither move nor
/// measure take no notice.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Live {
    pub time: Option<f32>,
    pub amount: Option<f32>,
}

/// Seconds for a wisp of steam to rise off a cup and go.
const STEAM: f32 = 2.8;
/// How often a figure that keeps moving moves on: the steam over the cup
/// drifts slowly enough that twenty steps a second look smooth.
pub const TICK: Duration = Duration::from_millis(50);

/// Draw `figure` on its grid, filling the square box of side `size` with its
/// top-left corner at `(x, y)`.
pub fn draw(canvas: &mut Canvas<'_>, figure: Figure, at: (f32, f32), size: f32, look: Look) {
    draw_live(canvas, figure, at, size, look, Live::default());
}

/// [`draw`], at a moment or an amount (see [`Live`]).
pub fn draw_live(
    canvas: &mut Canvas<'_>,
    figure: Figure,
    (x, y): (f32, f32),
    size: f32,
    look: Look,
    live: Live,
) {
    let frame = Frame {
        x,
        y,
        k: size / GRID,
        live,
        ..Frame::IDENTITY
    };
    sketch(&mut Pen::draw(canvas, frame, look), figure);
}

/// Draw `figure` fitted by its ink: exactly as tall as `rect`, centred
/// across it. `swing` (radians) tilts a bell about its knob. Use
/// [`aspect`] to know how wide to make `rect`.
///
/// Every state of a figure is fitted as its fullest state is — Wi-Fi as at
/// full signal, a speaker as at full volume — so a change of state never
/// changes its size or moves it.
pub fn draw_fitted(canvas: &mut Canvas<'_>, figure: Figure, rect: Rect, look: Look, swing: f32) {
    draw_fitted_live(canvas, figure, rect, look, swing, Live::default());
}

/// [`draw_fitted`], at a moment or an amount (see [`Live`]).
pub fn draw_fitted_live(
    canvas: &mut Canvas<'_>,
    figure: Figure,
    rect: Rect,
    look: Look,
    swing: f32,
    live: Live,
) {
    let Some(ink) = ink(fullest(figure), look.pen) else {
        return;
    };
    let k = rect.h / ink.h;
    let frame = Frame {
        x: rect.x + (rect.w - ink.w * k) / 2.0 - ink.x * k,
        y: rect.y - ink.y * k,
        k,
        swing,
        live,
        ..Frame::IDENTITY
    };
    sketch(&mut Pen::draw(canvas, frame, look), figure);
}

/// Width over height of `figure`'s ink (in its fullest state, as
/// [`draw_fitted`] fits it), drawn with this pen.
pub fn aspect(figure: Figure, pen: f32) -> f32 {
    ink(fullest(figure), pen).map_or(1.0, |ink| ink.w / ink.h)
}

/// The state of a figure that the others are fitted as.
fn fullest(figure: Figure) -> Figure {
    match figure {
        Figure::Wifi(_) | Figure::WifiOff => Figure::Wifi(4),
        Figure::Speaker(_) | Figure::SpeakerMuted => Figure::Speaker(3),
        Figure::Battery | Figure::BatteryCharging => Figure::Battery,
        Figure::Microphone | Figure::MicrophoneOff => Figure::Microphone,
        Figure::Bell | Figure::BellOff => Figure::Bell,
        other => other,
    }
}

/// Where `figure`'s ink falls on its grid, strokes included.
pub fn ink(figure: Figure, pen: f32) -> Option<Rect> {
    let look = Look {
        pen,
        ..Look::new(Rgba::rgb(0, 0, 0))
    };
    let mut pen = Pen::measure(look);
    sketch(&mut pen, figure);
    pen.bounds
}

/// Grid units onto the canvas: scaled by `k`, moved to `(x, y)`, and turned
/// by `swing` about `pivot`.
#[derive(Clone, Copy, Debug)]
struct Frame {
    x: f32,
    y: f32,
    k: f32,
    swing: f32,
    pivot: (f32, f32),
    live: Live,
}

impl Frame {
    const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        k: 1.0,
        swing: 0.0,
        pivot: (12.0, 12.0),
        live: Live {
            time: None,
            amount: None,
        },
    };

    fn at(self, (u, v): (f32, f32)) -> (f32, f32) {
        let (u, v) = if self.swing == 0.0 {
            (u, v)
        } else {
            let (sin, cos) = self.swing.sin_cos();
            let (du, dv) = (u - self.pivot.0, v - self.pivot.1);
            (
                self.pivot.0 + du * cos - dv * sin,
                self.pivot.1 + du * sin + dv * cos,
            )
        };
        (self.x + u * self.k, self.y + v * self.k)
    }
}

/// A shape being drawn in grid units.
struct Sketch {
    shape: Shape,
    frame: Frame,
}

/// Degrees, for writing angles the way one thinks of them: 0 right, 90
/// down (clockwise on screen).
fn deg(degrees: f32) -> f32 {
    degrees * PI / 180.0
}

impl Sketch {
    fn move_to(&mut self, p: (f32, f32)) -> &mut Self {
        let (x, y) = self.frame.at(p);
        self.shape.move_to(x, y);
        self
    }

    fn line_to(&mut self, p: (f32, f32)) -> &mut Self {
        let (x, y) = self.frame.at(p);
        self.shape.line_to(x, y);
        self
    }

    fn line(&mut self, from: (f32, f32), to: (f32, f32)) -> &mut Self {
        self.move_to(from).line_to(to)
    }

    fn cubic_to(&mut self, c1: (f32, f32), c2: (f32, f32), p: (f32, f32)) -> &mut Self {
        let ((x1, y1), (x2, y2), (x, y)) = (self.frame.at(c1), self.frame.at(c2), self.frame.at(p));
        self.shape.cubic_to(x1, y1, x2, y2, x, y);
        self
    }

    fn close(&mut self) -> &mut Self {
        self.shape.close();
        self
    }

    /// An arc as a piece of its own; angles in degrees.
    fn arc(&mut self, c: (f32, f32), r: f32, start: f32, sweep: f32) -> &mut Self {
        self.arc_with(c, r, start, sweep, false)
    }

    /// An arc carrying on the current piece.
    fn arc_to(&mut self, c: (f32, f32), r: f32, start: f32, sweep: f32) -> &mut Self {
        self.arc_with(c, r, start, sweep, true)
    }

    fn arc_with(
        &mut self,
        c: (f32, f32),
        r: f32,
        start: f32,
        sweep: f32,
        joined: bool,
    ) -> &mut Self {
        // Turned with the frame: a swinging bell's curves swing with it.
        let (cx, cy) = self.frame.at(c);
        let start = deg(start) + self.frame.swing;
        let r = r * self.frame.k;
        if joined {
            self.shape.arc_to((cx, cy), r, start, deg(sweep));
        } else {
            self.shape.arc((cx, cy), r, start, deg(sweep));
        }
        self
    }

    fn circle(&mut self, c: (f32, f32), r: f32) -> &mut Self {
        let centre = self.frame.at(c);
        self.shape.circle(centre, r * self.frame.k);
        self
    }

    /// Lines through `points`, each corner rounded to `radius` (no more
    /// than half of either side it joins); `closed` comes back to the start.
    fn soft(&mut self, points: &[(f32, f32)], radius: f32, closed: bool) -> &mut Self {
        let n = points.len();
        if n < 2 {
            return self;
        }
        let toward = |from: (f32, f32), to: (f32, f32), by: f32| {
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let length = dx.hypot(dy).max(f32::EPSILON);
            let by = by.min(length / 2.0);
            (from.0 + dx / length * by, from.1 + dy / length * by)
        };
        let corner = |i: usize| {
            let at = points[i];
            let before = points[(i + n - 1) % n];
            let after = points[(i + 1) % n];
            (toward(at, before, radius), at, toward(at, after, radius))
        };
        if closed {
            let (_, _, first) = corner(0);
            self.move_to(first);
            for i in (1..n).chain([0]) {
                let (into, at, out) = corner(i);
                self.line_to(into);
                self.quad(at, out);
            }
            self.close();
        } else {
            self.move_to(points[0]);
            for i in 1..n - 1 {
                let (into, at, out) = corner(i);
                self.line_to(into);
                self.quad(at, out);
            }
            self.line_to(points[n - 1]);
        }
        self
    }

    fn quad(&mut self, control: (f32, f32), to: (f32, f32)) -> &mut Self {
        let ((x1, y1), (x, y)) = (self.frame.at(control), self.frame.at(to));
        self.shape.quad_to(x1, y1, x, y);
        self
    }

    /// A rectangle with rounded corners.
    fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) -> &mut Self {
        self.soft(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], r, true)
    }
}

/// Where a figure's parts go: onto a canvas, or into a measurement.
struct Pen<'p, 'c> {
    canvas: Option<&'p mut Canvas<'c>>,
    frame: Frame,
    look: Look,
    bounds: Option<Rect>,
}

impl<'p, 'c> Pen<'p, 'c> {
    fn draw(canvas: &'p mut Canvas<'c>, frame: Frame, look: Look) -> Self {
        Self {
            canvas: Some(canvas),
            frame,
            look,
            bounds: None,
        }
    }

    fn measure(look: Look) -> Self {
        Self {
            canvas: None,
            frame: Frame::IDENTITY,
            look,
            bounds: None,
        }
    }

    fn sketch(&self) -> Sketch {
        Sketch {
            shape: Shape::new(),
            frame: self.frame,
        }
    }

    fn color(&self, lit: bool) -> Rgba {
        if lit { self.look.on } else { self.look.off }
    }

    /// Draw lines with the pen.
    fn stroke(&mut self, lit: bool, draw: impl FnOnce(&mut Sketch)) {
        let mut sketch = self.sketch();
        draw(&mut sketch);
        let width = self.look.pen * self.frame.k;
        self.take(&sketch.shape, width / 2.0);
        let color = self.color(lit);
        if let Some(canvas) = &mut self.canvas {
            canvas.stroke_shape(&sketch.shape, width, color);
        }
    }

    /// Fill shapes.
    fn fill(&mut self, lit: bool, draw: impl FnOnce(&mut Sketch)) {
        let mut sketch = self.sketch();
        draw(&mut sketch);
        self.take(&sketch.shape, 0.0);
        let color = self.color(lit);
        if let Some(canvas) = &mut self.canvas {
            canvas.fill_shape(&sketch.shape, color);
        }
    }

    /// Note how far a shape reaches, `grow` beyond its outline.
    fn take(&mut self, shape: &Shape, grow: f32) {
        if self.canvas.is_some() {
            return;
        }
        let Some(b) = shape.bounds() else {
            return;
        };
        let b = Rect::new(b.x - grow, b.y - grow, b.w + 2.0 * grow, b.h + 2.0 * grow);
        self.bounds = Some(match self.bounds {
            None => b,
            Some(a) => {
                let (x, y) = (a.x.min(b.x), a.y.min(b.y));
                Rect::new(
                    x,
                    y,
                    a.right().max(b.right()) - x,
                    a.bottom().max(b.bottom()) - y,
                )
            }
        });
    }
}

/// Every figure, in grid units.
fn sketch(p: &mut Pen<'_, '_>, figure: Figure) {
    match figure {
        Figure::Search => {
            p.stroke(true, |s| {
                s.circle((10.6, 10.6), 6.6);
                s.line((15.4, 15.4), (20.2, 20.2));
            });
        }
        Figure::Apps => p.fill(true, |s| {
            for (x, y) in [(3.5, 3.5), (13.5, 3.5), (3.5, 13.5), (13.5, 13.5)] {
                s.rounded(x, y, 7.0, 7.0, 2.4);
            }
        }),
        Figure::Folder => p.stroke(true, |s| {
            s.soft(
                &[
                    (2.8, 5.0),
                    (9.0, 5.0),
                    (11.2, 7.6),
                    (21.2, 7.6),
                    (21.2, 19.0),
                    (2.8, 19.0),
                ],
                2.2,
                true,
            );
        }),
        Figure::File => p.stroke(true, |s| {
            s.soft(
                &[
                    (5.5, 3.0),
                    (13.8, 3.0),
                    (18.5, 7.7),
                    (18.5, 21.0),
                    (5.5, 21.0),
                ],
                2.0,
                true,
            );
            s.soft(&[(13.8, 3.0), (13.8, 7.7), (18.5, 7.7)], 1.2, false);
        }),
        Figure::Bell => bell(p, true),
        Figure::BellOff => {
            // The bell, faint, struck through: there, but not ringing. As
            // far as it's hushed (`amount`), the strike draws across it and
            // it fades.
            let hushed = p.frame.live.amount.unwrap_or(1.0).clamp(0.0, 1.0);
            let on = p.look.on;
            p.look.on = on.mix(p.look.off, hushed);
            bell(p, true);
            p.look.on = on;
            if hushed > 0.0 {
                let (from, to) = ((4.0, 3.5), (20.0, 20.5));
                let reach = (
                    from.0 + (to.0 - from.0) * hushed,
                    from.1 + (to.1 - from.1) * hushed,
                );
                p.stroke(true, |s| {
                    s.line(from, reach);
                });
            }
        }
        Figure::Cup => {
            p.stroke(true, |s| {
                // The cup and its handle.
                s.soft(
                    &[(4.0, 9.6), (4.0, 20.0), (16.0, 20.0), (16.0, 9.6)],
                    2.6,
                    false,
                );
                s.line((4.0, 9.6), (16.0, 9.6));
                s.move_to((16.0, 12.0))
                    .line_to((17.4, 12.0))
                    .arc_to((17.4, 14.6), 2.6, -90.0, 180.0)
                    .line_to((16.0, 17.2));
            });
            match p.frame.live.time {
                // Two wisps of steam, still.
                None => p.stroke(true, |s| {
                    s.move_to((8.0, 6.6))
                        .cubic_to((6.8, 5.4), (9.2, 4.2), (8.0, 3.0));
                    s.move_to((12.0, 6.6))
                        .cubic_to((10.8, 5.4), (13.2, 4.2), (12.0, 3.0));
                }),
                Some(t) => steam(p, t),
            }
        }
        Figure::Display => {
            p.stroke(true, |s| {
                s.rounded(2.6, 4.4, 18.8, 12.6, 2.4);
                s.line((8.6, 20.6), (15.4, 20.6));
                s.line((12.0, 17.0), (12.0, 20.6));
            });
        }
        Figure::Sliders => {
            p.stroke(true, |s| {
                // Two tracks, their knobs at different places.
                s.line((3.5, 8.0), (9.0, 8.0));
                s.line((14.6, 8.0), (20.5, 8.0));
                s.circle((11.8, 8.0), 2.6);
                s.line((3.5, 16.0), (9.6, 16.0));
                s.line((15.2, 16.0), (20.5, 16.0));
                s.circle((12.4, 16.0), 2.6);
            });
        }
        Figure::Screencast => {
            p.stroke(true, |s| {
                s.rounded(2.6, 4.4, 18.8, 12.6, 2.4);
                s.line((8.6, 20.6), (15.4, 20.6));
            });
            p.fill(true, |s| {
                s.circle((12.0, 10.7), 2.6);
            });
        }
        Figure::Close => p.stroke(true, |s| {
            s.line((6.5, 6.5), (17.5, 17.5));
            s.line((17.5, 6.5), (6.5, 17.5));
        }),
        Figure::Check => p.stroke(true, |s| {
            s.soft(&[(4.5, 12.6), (9.6, 17.5), (19.5, 7.0)], 0.6, false);
        }),
        Figure::Palette => palette(p),
        Figure::Wifi(bars) => wifi(p, Some(bars)),
        Figure::WifiOff => {
            wifi(p, None);
            // Across the fan only, not beyond it.
            p.stroke(true, |s| {
                s.line((5.0, 5.0), (19.0, 19.5));
            });
        }
        Figure::Ethernet => {
            p.fill(true, |s| {
                s.rounded(8.8, 2.8, 6.4, 5.6, 1.6);
                s.rounded(2.8, 15.6, 6.4, 5.6, 1.6);
                s.rounded(14.8, 15.6, 6.4, 5.6, 1.6);
            });
            p.stroke(true, |s| {
                s.line((12.0, 8.0), (12.0, 12.0));
                s.soft(
                    &[(6.0, 16.0), (6.0, 12.0), (18.0, 12.0), (18.0, 16.0)],
                    1.2,
                    false,
                );
            });
        }
        Figure::Bluetooth => p.stroke(true, |s| {
            s.soft(
                &[
                    (6.5, 7.5),
                    (16.8, 16.4),
                    (12.0, 21.0),
                    (12.0, 3.0),
                    (16.8, 7.6),
                    (6.5, 16.5),
                ],
                0.5,
                false,
            );
        }),
        Figure::Speaker(waves) => speaker(p, Some(waves)),
        Figure::SpeakerMuted => speaker(p, None),
        Figure::Battery | Figure::BatteryCharging => {
            p.stroke(true, |s| {
                s.rounded(2.5, 6.8, 16.8, 10.4, 2.8);
            });
            p.fill(true, |s| {
                s.rounded(20.6, 9.8, 1.8, 4.4, 0.9);
            });
            if figure == Figure::Battery {
                p.fill(true, |s| {
                    s.rounded(5.4, 9.6, 7.6, 4.8, 1.2);
                });
            } else {
                p.fill(true, |s| {
                    s.soft(
                        &[
                            (12.4, 8.4),
                            (7.6, 12.7),
                            (10.6, 12.7),
                            (9.6, 15.6),
                            (14.4, 11.3),
                            (11.4, 11.3),
                        ],
                        0.3,
                        true,
                    );
                });
            }
        }
        Figure::Bolt => p.stroke(true, |s| {
            s.soft(
                &[
                    (13.2, 2.8),
                    (4.6, 13.4),
                    (11.4, 13.4),
                    (10.6, 21.2),
                    (19.4, 10.6),
                    (12.6, 10.6),
                ],
                1.0,
                true,
            );
        }),
        Figure::Thermometer => {
            // The bulb meets the tube where the circle is the tube's width.
            let meet = (2.5_f32 / 4.0).acos().to_degrees();
            let neck = 17.0 - 4.0 * meet.to_radians().sin();
            p.stroke(true, |s| {
                // Up the tube's left, over its top, down its right, and
                // round the bulb from its right shoulder to its left.
                s.move_to((9.5, neck))
                    .line_to((9.5, 5.2))
                    .arc_to((12.0, 5.2), 2.5, 180.0, 180.0)
                    .line_to((14.5, neck))
                    .arc_to((12.0, 17.0), 4.0, -meet, 180.0 + 2.0 * meet)
                    .close();
            });
            p.fill(true, |s| {
                s.circle((12.0, 17.0), 1.7);
            });
            p.stroke(true, |s| {
                s.line((12.0, 15.6), (12.0, 9.5));
            });
        }
        Figure::Brightness => {
            // The rays reach as far as the light is bright, given how bright.
            let reach = p
                .frame
                .live
                .amount
                .map_or(9.2, |amount| 7.9 + 1.9 * amount.clamp(0.0, 1.0));
            p.stroke(true, |s| {
                s.circle((12.0, 12.0), 4.2);
                for i in 0..8 {
                    let a = deg(i as f32 * 45.0);
                    let (cos, sin) = (a.cos(), a.sin());
                    s.line(
                        (12.0 + 7.4 * cos, 12.0 + 7.4 * sin),
                        (12.0 + reach * cos, 12.0 + reach * sin),
                    );
                }
            });
        }
        Figure::Keyboard => {
            p.stroke(true, |s| {
                s.rounded(2.5, 6.0, 19.0, 12.0, 2.8);
                s.line((8.5, 14.2), (15.5, 14.2));
            });
            p.fill(true, |s| {
                for x in [6.6, 10.2, 13.8, 17.4] {
                    s.circle((x, 10.2), 1.0);
                }
            });
        }
        Figure::Lock => {
            // The shackle lifts out of the body as far as it's opened.
            let lift = p.frame.live.amount.unwrap_or(0.0).clamp(0.0, 1.0) * 2.4;
            p.stroke(true, |s| {
                s.rounded(5.0, 10.0, 14.0, 10.6, 2.6);
                s.move_to((8.4, 10.0))
                    .line_to((8.4, 7.0 - lift))
                    .arc_to((12.0, 7.0 - lift), 3.6, 180.0, 180.0)
                    .line_to((15.6, 10.0 - lift));
            });
            p.fill(true, |s| {
                s.circle((12.0, 15.3), 1.5);
            });
        }
        Figure::ChevronLeft => chevron(p, &[(14.5, 6.0), (8.5, 12.0), (14.5, 18.0)]),
        Figure::ChevronRight => chevron(p, &[(9.5, 6.0), (15.5, 12.0), (9.5, 18.0)]),
        Figure::ChevronUp => chevron(p, &[(6.0, 14.5), (12.0, 8.5), (18.0, 14.5)]),
        Figure::ChevronDown => chevron(p, &[(6.0, 9.5), (12.0, 15.5), (18.0, 9.5)]),
        Figure::Microphone | Figure::MicrophoneOff => {
            p.stroke(true, |s| {
                s.rounded(9.0, 2.8, 6.0, 11.6, 3.0);
                s.move_to((5.6, 10.0))
                    .line_to((5.6, 11.0))
                    .arc_to((12.0, 11.0), 6.4, 180.0, -180.0)
                    .line_to((18.4, 10.0));
                s.line((12.0, 17.4), (12.0, 21.0));
            });
            if figure == Figure::MicrophoneOff {
                p.stroke(true, |s| {
                    s.line((4.0, 3.5), (20.0, 20.5));
                });
            }
        }
        Figure::Home => p.stroke(true, |s| {
            s.soft(&[(3.6, 11.4), (12.0, 4.0), (20.4, 11.4)], 1.0, false);
            s.soft(
                &[(6.0, 9.6), (6.0, 20.0), (18.0, 20.0), (18.0, 9.6)],
                1.8,
                false,
            );
            s.soft(
                &[(10.2, 20.0), (10.2, 15.0), (13.8, 15.0), (13.8, 20.0)],
                1.0,
                false,
            );
        }),
        Figure::Trash => p.stroke(true, |s| {
            s.line((4.0, 6.6), (20.0, 6.6));
            s.soft(
                &[(9.4, 6.6), (9.4, 3.8), (14.6, 3.8), (14.6, 6.6)],
                1.2,
                false,
            );
            s.soft(
                &[(6.2, 6.6), (7.4, 20.4), (16.6, 20.4), (17.8, 6.6)],
                1.8,
                false,
            );
            s.line((10.2, 10.4), (10.4, 16.6));
            s.line((13.8, 10.4), (13.6, 16.6));
        }),
        Figure::Drive => {
            p.stroke(true, |s| {
                s.rounded(3.2, 7.0, 17.6, 10.0, 2.6);
                s.line((6.8, 12.0), (11.0, 12.0));
            });
            p.fill(true, |s| {
                s.circle((16.6, 12.0), 1.4);
            });
        }
        Figure::Clock => p.stroke(true, |s| {
            s.circle((12.0, 12.0), 8.4);
            s.soft(&[(12.0, 7.2), (12.0, 12.0), (15.4, 14.2)], 0.8, false);
        }),
        Figure::Bookmark => p.stroke(true, |s| {
            s.soft(
                &[
                    (6.6, 20.4),
                    (6.6, 4.0),
                    (17.4, 4.0),
                    (17.4, 20.4),
                    (12.0, 16.2),
                ],
                1.8,
                true,
            );
        }),
        Figure::Eject => p.stroke(true, |s| {
            s.soft(&[(12.0, 4.6), (19.0, 13.0), (5.0, 13.0)], 1.4, true);
            s.line((5.0, 18.4), (19.0, 18.4));
        }),
        Figure::Power => p.stroke(true, |s| {
            s.arc((12.0, 13.0), 7.6, -52.0, 284.0);
            s.line((12.0, 3.2), (12.0, 11.4));
        }),
        Figure::Restart => {
            // A circle nearly closed, turning back on itself: the arrow's
            // head sits where it began, pointing the way it goes.
            let (c, r, start) = ((12.0, 12.5), 7.8, 210.0_f32);
            let tip = (c.0 + r * deg(start).cos(), c.1 + r * deg(start).sin());
            // The circle runs clockwise from the tip; the arrow runs back
            // the other way, so its arms open along the clockwise tangent.
            let along = deg(start) + PI / 2.0;
            let arm = |turn: f32| {
                let a = along + deg(turn);
                (tip.0 + 4.6 * a.cos(), tip.1 + 4.6 * a.sin())
            };
            p.stroke(true, |s| {
                s.arc(c, r, start, 300.0);
                s.soft(&[arm(-42.0), tip, arm(42.0)], 0.6, false);
            });
        }
        Figure::Logout => p.stroke(true, |s| {
            s.soft(
                &[(10.0, 3.5), (4.5, 3.5), (4.5, 20.5), (10.0, 20.5)],
                2.2,
                false,
            );
            s.line((9.6, 12.0), (20.2, 12.0));
            s.soft(&[(16.2, 8.0), (20.2, 12.0), (16.2, 16.0)], 0.6, false);
        }),
        Figure::Sleep => moon(p),
        // The media figures: a soft triangle, drawn, like the eject's, and
        // two bars. The triangle sits a little right of the middle, so its
        // weight — not its box — is centred.
        Figure::Play => p.stroke(true, |s| {
            s.soft(&[(7.8, 5.2), (18.4, 12.0), (7.8, 18.8)], 1.6, true);
        }),
        Figure::Info => {
            p.stroke(true, |s| {
                s.circle((12.0, 12.0), 8.4);
                s.line((12.0, 11.2), (12.0, 16.4));
            });
            p.fill(true, |s| {
                s.circle((12.0, 7.8), 1.3);
            });
        }
        Figure::Note => {
            // An eighth note: its head, its stem, the flag lifting off it.
            p.fill(true, |s| {
                s.circle((8.6, 16.8), 3.0);
            });
            p.stroke(true, |s| {
                s.line((11.4, 16.8), (11.4, 4.2));
                s.move_to((11.4, 4.2))
                    .cubic_to((12.4, 7.6), (18.4, 7.4), (16.8, 12.6));
            });
        }
        Figure::Pause => p.stroke(true, |s| {
            s.line((8.6, 5.6), (8.6, 18.4));
            s.line((15.4, 5.6), (15.4, 18.4));
        }),
        Figure::Next => p.stroke(true, |s| {
            s.soft(&[(5.4, 6.0), (14.6, 12.0), (5.4, 18.0)], 1.4, true);
            s.line((18.6, 6.0), (18.6, 18.0));
        }),
        Figure::Previous => p.stroke(true, |s| {
            s.soft(&[(18.6, 6.0), (9.4, 12.0), (18.6, 18.0)], 1.4, true);
            s.line((5.4, 6.0), (5.4, 18.0));
        }),
        Figure::CapsLock => p.stroke(true, |s| {
            s.soft(
                &[
                    (12.0, 3.4),
                    (20.2, 11.6),
                    (15.6, 11.6),
                    (15.6, 15.6),
                    (8.4, 15.6),
                    (8.4, 11.6),
                    (3.8, 11.6),
                ],
                1.0,
                true,
            );
            s.line((8.4, 20.2), (15.6, 20.2));
        }),
    }
}

fn chevron(p: &mut Pen<'_, '_>, points: &[(f32, f32)]) {
    p.stroke(true, |s| {
        s.soft(points, 0.6, false);
    });
}

/// A dot and three arcs fanning up from it; `bars` lights the dot and then
/// the arcs from the inside out, `None` leaves every part faint.
fn wifi(p: &mut Pen<'_, '_>, bars: Option<u8>) {
    let lit = bars.map_or(0, |bars| bars.clamp(1, 4));
    let origin = (12.0, 18.2);
    p.fill(lit >= 1, |s| {
        s.circle((12.0, 17.9), 1.8);
    });
    for (n, r) in [(2, 4.2), (3, 8.4), (4, 12.6)] {
        p.stroke(lit >= n, |s| {
            s.arc(origin, r, -135.0, 90.0);
        });
    }
}

/// A soft cone and its waves: one; two with the outer faint; two. `None`
/// is muted: a small cross where the waves were.
fn speaker(p: &mut Pen<'_, '_>, waves: Option<u8>) {
    p.stroke(true, |s| {
        s.soft(
            &[
                (3.0, 9.0),
                (7.0, 9.0),
                (12.0, 4.6),
                (12.0, 19.4),
                (7.0, 15.0),
                (3.0, 15.0),
            ],
            1.4,
            true,
        );
    });
    match waves {
        Some(waves) => {
            p.stroke(true, |s| {
                s.arc((12.0, 12.0), 4.2, -45.0, 90.0);
            });
            if waves >= 2 {
                p.stroke(waves >= 3, |s| {
                    s.arc((12.0, 12.0), 8.4, -45.0, 90.0);
                });
            }
        }
        None => p.stroke(true, |s| {
            s.line((16.0, 9.5), (21.0, 14.5));
            s.line((21.0, 9.5), (16.0, 14.5));
        }),
    }
}

/// A plump bell on a flared rim, a knob above and a clapper below: solid,
/// so it reads at the smallest size. It swings about its knob.
fn bell(p: &mut Pen<'_, '_>, lit: bool) {
    p.frame.pivot = (12.0, 3.2);
    // The outline in fractions of the bell's height across from its
    // middle, and down from its top: each curve is three points after the
    // first.
    const BODY: [(f32, f32); 28] = [
        (0.0, 0.13),
        (0.2, 0.13),
        (0.3, 0.27),
        (0.3, 0.44),
        (0.3, 0.52),
        (0.3, 0.58),
        (0.3, 0.6),
        (0.3, 0.68),
        (0.4, 0.7),
        (0.4, 0.77),
        (0.4, 0.81),
        (0.37, 0.82),
        (0.33, 0.82),
        (0.1, 0.82),
        (-0.1, 0.82),
        (-0.33, 0.82),
        (-0.37, 0.82),
        (-0.4, 0.81),
        (-0.4, 0.77),
        (-0.4, 0.7),
        (-0.3, 0.68),
        (-0.3, 0.6),
        (-0.3, 0.58),
        (-0.3, 0.52),
        (-0.3, 0.44),
        (-0.3, 0.27),
        (-0.2, 0.13),
        (0.0, 0.13),
    ];
    let at = |(u, v): (f32, f32)| (12.0 + u * 20.0, 1.6 + v * 20.0);
    p.fill(lit, |s| {
        s.move_to(at(BODY[0]));
        for curve in BODY[1..].chunks(3) {
            s.cubic_to(at(curve[0]), at(curve[1]), at(curve[2]));
        }
        s.close();
        s.circle(at((0.0, 0.1)), 1.5);
    });
    // The clapper swings a little further than the bell.
    let swing = p.frame.swing;
    p.frame.swing = swing * 1.6;
    p.fill(lit, |s| {
        s.circle(at((0.0, 0.92)), 1.7);
    });
    p.frame.swing = swing;
}

/// A painter's palette: a round board with a thumb hole's dip, and four
/// dots of paint.
fn palette(p: &mut Pen<'_, '_>) {
    let c = (12.0, 12.0);
    let r = 9.0;
    let at = |a: f32| (c.0 + r * deg(a).cos(), c.1 + r * deg(a).sin());
    let (from, to) = (at(68.0), at(4.0));
    p.stroke(true, |s| {
        s.arc(c, r, 68.0, 296.0);
        s.cubic_to((to.0 - 0.4, to.1 + 3.4), (16.0, 11.4), (15.4, 14.6));
        s.cubic_to((14.9, 17.0), (from.0 - 1.6, from.1 - 2.6), from);
        s.close();
    });
    p.fill(true, |s| {
        for (x, y) in [(7.6, 12.6), (8.6, 8.0), (12.6, 6.6), (16.4, 8.6)] {
            s.circle((x, y), 1.5);
        }
    });
}

/// A crescent moon: the part of one circle a second, nearer the top right,
/// does not cover.
fn moon(p: &mut Pen<'_, '_>) {
    let (a, ra) = ((11.4_f32, 12.6_f32), 8.4_f32);
    let (b, rb) = ((16.6_f32, 7.6_f32), 6.6_f32);
    // Where the circles cross.
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let d = dx.hypot(dy);
    let along = (d * d + ra * ra - rb * rb) / (2.0 * d);
    let across = (ra * ra - along * along).max(0.0).sqrt();
    let (ux, uy) = (dx / d, dy / d);
    let mid = (a.0 + ux * along, a.1 + uy * along);
    let p1 = (mid.0 - uy * across, mid.1 + ux * across);
    let p2 = (mid.0 + uy * across, mid.1 - ux * across);
    let angle = |c: (f32, f32), q: (f32, f32)| (q.1 - c.1).atan2(q.0 - c.0).to_degrees();
    // Round the outer circle the long way, away from the second; back
    // along the second the short way, inside the first.
    let (a1, a2) = (angle(a, p1), angle(a, p2));
    let outer = (a2 - a1).rem_euclid(360.0);
    let (b2, b1) = (angle(b, p2), angle(b, p1));
    let inner = -(b2 - b1).rem_euclid(360.0);
    p.stroke(true, |s| {
        s.arc(a, ra, a1, outer);
        s.arc_to(b, rb, b2, inner);
        s.close();
    });
}

/// Every figure, and each state of those with states: for tests and
/// contact sheets.
pub const ALL: [Figure; 51] = [
    Figure::Search,
    Figure::Apps,
    Figure::Folder,
    Figure::File,
    Figure::Bell,
    Figure::Close,
    Figure::Check,
    Figure::Palette,
    Figure::Wifi(0),
    Figure::Wifi(2),
    Figure::Wifi(4),
    Figure::WifiOff,
    Figure::Ethernet,
    Figure::Bluetooth,
    Figure::Speaker(1),
    Figure::Speaker(2),
    Figure::Speaker(3),
    Figure::SpeakerMuted,
    Figure::Battery,
    Figure::BatteryCharging,
    Figure::Bolt,
    Figure::Thermometer,
    Figure::Brightness,
    Figure::Keyboard,
    Figure::Lock,
    Figure::ChevronLeft,
    Figure::ChevronRight,
    Figure::ChevronUp,
    Figure::ChevronDown,
    Figure::Microphone,
    Figure::MicrophoneOff,
    Figure::Power,
    Figure::Restart,
    Figure::Logout,
    Figure::Sleep,
    Figure::CapsLock,
    Figure::Home,
    Figure::Trash,
    Figure::Drive,
    Figure::Eject,
    Figure::Clock,
    Figure::Bookmark,
    Figure::Play,
    Figure::Pause,
    Figure::Next,
    Figure::Previous,
    Figure::BellOff,
    Figure::Cup,
    Figure::Screencast,
    Figure::Display,
    Figure::Sliders,
];

/// Two wisps of steam rising off a cup at `t` seconds: each leaves the
/// coffee straight, sways more as it climbs, thins to a whisper near the
/// top, and lets go, the second half a breath behind the first.
fn steam(p: &mut Pen<'_, '_>, t: f32) {
    for (x0, behind) in [(8.0f32, 0.0f32), (12.0, 0.5)] {
        let age = (t / STEAM + behind).rem_euclid(1.0);
        // The stretch of the wisp that shows, as heights 0 (the cup) to 1.
        let top = (age * 1.4).min(1.0);
        let bottom = (age * 1.4 - 0.85).max(0.0);
        if top - bottom < 0.04 {
            continue;
        }
        let at = |v: f32| {
            let sway = (std::f32::consts::TAU * (v * 0.6 - age)).sin();
            (x0 + 1.0 * v * sway, 6.8 - 4.6 * v)
        };
        // Low, it is the cup's own colour; higher, a whisper of it.
        let fade = 0.65f32.clamp(bottom, top);
        for (from, to, lit) in [(bottom, fade, true), (fade, top, false)] {
            if to - from < 0.02 {
                continue;
            }
            p.stroke(lit, |s| {
                s.move_to(at(from));
                for i in 1..=8 {
                    s.line_to(at(from + (to - from) * i as f32 / 8.0));
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A figure drawn alone, 48 pixels square, at a moment or an amount.
    fn alone(figure: Figure, live: Live) -> crate::Offscreen {
        let mut text = crate::TextEngine::new(&enso_core::config::Appearance::default());
        crate::Offscreen::render(&mut text, (48, 48), SHOWN, |canvas| {
            draw_live(
                canvas,
                figure,
                (0.0, 0.0),
                48.0,
                Look::new(Rgba::rgb(255, 255, 255)),
                live,
            );
        })
    }

    /// Pixels a logical pixel, in the pictures made to look at.
    const SHOWN: u32 = 2;

    /// How much of the box is inked, and how high the ink reaches.
    fn ink_of(frame: &crate::Offscreen) -> (usize, u32) {
        let lit = frame.rgba.chunks(4).filter(|px| px[3] > 40).count();
        let top = (0..frame.height)
            .find(|&y| (0..frame.width).any(|x| frame.pixel(x, y)[3] > 40))
            .unwrap_or(frame.height);
        (lit, top)
    }

    #[test]
    fn steam_drifts_a_sun_shines_a_lock_opens_and_a_bell_hushes() {
        let at = |t: f32| Live {
            time: Some(t),
            amount: None,
        };
        let by = |amount: f32| Live {
            time: None,
            amount: Some(amount),
        };
        // The steam moves; the cup under it does not.
        let (one, two) = (alone(Figure::Cup, at(0.4)), alone(Figure::Cup, at(1.3)));
        assert!(one.rgba != two.rgba, "the steam stood still");
        let low = |frame: &crate::Offscreen| frame.rgba[frame.rgba.len() / 2..].to_vec();
        assert!(low(&one) == low(&two), "the cup moved");
        // Brighter, the rays reach further.
        let dim = alone(Figure::Brightness, by(0.0));
        let bright = alone(Figure::Brightness, by(1.0));
        assert!(ink_of(&bright).0 > ink_of(&dim).0 + 40 * SHOWN as usize);
        // Opened, the shackle stands higher.
        let (shut, open) = (alone(Figure::Lock, by(0.0)), alone(Figure::Lock, by(1.0)));
        assert!(ink_of(&open).1 + 3 * SHOWN < ink_of(&shut).1);
        // Still, as they always were.
        assert!(alone(Figure::Lock, Live::default()).rgba == shut.rgba);
        // Not yet hushed, the bell is the bell; all the way, the struck-
        // through bell it always was: nothing jumps at either end.
        assert!(alone(Figure::BellOff, by(0.0)).rgba == alone(Figure::Bell, Live::default()).rgba);
        let hushed = alone(Figure::BellOff, by(1.0));
        assert!(alone(Figure::BellOff, Live::default()).rgba == hushed.rgba);
        #[cfg(feature = "png")]
        {
            let frames: Vec<crate::Offscreen> = (0..8)
                .map(|i| alone(Figure::Cup, at(i as f32 * STEAM / 8.0)))
                .chain([0.0, 0.5, 1.0].map(|a| alone(Figure::Brightness, by(a))))
                .chain([0.0, 0.5, 1.0].map(|a| alone(Figure::Lock, by(a))))
                .chain([0.0, 0.5, 1.0].map(|a| alone(Figure::BellOff, by(a))))
                .collect();
            let side = (48 * SHOWN) as usize;
            let width = (side * frames.len()) as u32;
            // Over the dusk's base, to look at.
            let mut strip = [0x23u8, 0x21, 0x36, 255].repeat(width as usize * side);
            for (n, frame) in frames.iter().enumerate() {
                for y in 0..side {
                    for x in 0..side {
                        let src = &frame.rgba[(y * side + x) * 4..(y * side + x) * 4 + 4];
                        let to = (y * width as usize + n * side + x) * 4;
                        let keep = 255 - u32::from(src[3]);
                        for c in 0..3 {
                            let dst = u32::from(strip[to + c]);
                            strip[to + c] = (u32::from(src[c]) + dst * keep / 255) as u8;
                        }
                    }
                }
            }
            crate::Offscreen {
                width,
                height: side as u32,
                rgba: strip,
            }
            .snapshot(env!("CARGO_MANIFEST_DIR"), "figures-live");
        }
    }

    /// Every figure keeps to its grid with room to spare, and fills enough
    /// of it to be seen: the same optical size, the same breathing room.
    #[test]
    fn every_figure_keeps_to_the_middle_of_its_grid() {
        for figure in ALL {
            let ink = ink(figure, PEN).unwrap_or_else(|| panic!("{figure:?} draws nothing"));
            assert!(
                ink.x >= 1.5 && ink.y >= 1.5 && ink.right() <= 22.5 && ink.bottom() <= 22.5,
                "{figure:?} reaches the edge: {ink:?}"
            );
            // Marks — a cross, a chevron — are drawn smaller, as in any set.
            assert!(ink.w.max(ink.h) >= 12.0, "{figure:?} is too small: {ink:?}");
            // Centred, to within a unit, both ways — but for the speaker
            // sounding quietly: its cone stays where the full speaker's is,
            // so turning the volume never moves it.
            if figure == Figure::Speaker(1) {
                continue;
            }
            let (cx, cy) = ink.center();
            assert!(
                (cx - 12.0).abs() <= 1.0 && (cy - 12.0).abs() <= 1.0,
                "{figure:?} is off centre: {ink:?}"
            );
        }
    }

    #[test]
    fn a_heavier_pen_reaches_further() {
        let light = ink(Figure::Wifi(4), PEN).expect("Wi-Fi draws");
        let heavy = ink(Figure::Wifi(4), PEN * 1.5).expect("Wi-Fi draws");
        assert!(heavy.w > light.w && heavy.h > light.h);
        assert!(aspect(Figure::Wifi(4), PEN) > 1.0);
        assert!(aspect(Figure::Bell, PEN) < 1.0);
    }
}
