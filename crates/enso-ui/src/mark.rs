//! The ensō: the suite's mark, one brush stroke round an open circle, as
//! the Neovim start screen draws it in text.
//!
//! The stroke is a centre line round the circle (`centre`), a brush width
//! along it (`half`), and twenty-six bristles across that width, each
//! running dry in its own places (`noise`) so the end of the stroke breaks
//! up the way a dry brush does. Everything is computed from those few
//! functions; nothing is read from a file, so the lock screen draws the
//! same pixels at every size.
//!
//! It is painted once per size into a raster and kept. In motion it is
//! painted in along the stroke, then breathes: a slow swell and a faint
//! glow behind it, with a light travelling the stroke every so often.
//! Without motion it is simply there.

use std::{
    f64::consts::PI,
    sync::OnceLock,
    time::{Duration, Instant},
};

use enso_core::{Motion, Palette, Rgba, Transition, Tween};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};

use crate::{Canvas, Picture, Rect};

/// Points along the stroke.
const STEPS: usize = 720;
const BRISTLES: usize = 26;
/// Points each little line of a bristle covers. The stroke is drawn a
/// group at a time, so a half-painted stroke is the whole one's first
/// groups exactly.
const GROUP: usize = 3;
const GROUPS: usize = STEPS / GROUP;
/// Where the stroke starts and ends: upper left, round the bottom, and on
/// up past the top. Clockwise on screen is decreasing angle.
const FROM: f64 = -128.0 * PI / 180.0;
const TO: f64 = -436.0 * PI / 180.0;

/// The glow behind the mark: its alpha at rest, and how much a breath adds.
const GLOW: f32 = 0.035;
const GLOW_BREATH: f32 = 0.06;
/// How much a breath swells the mark.
const SWELL: f32 = 0.012;
/// How far the light reaches along the stroke either side of its head,
/// and how bright it is at most.
const SHEEN_REACH: f64 = 0.15;
const SHEEN_WIDTH: f64 = 0.05;
const SHEEN_ALPHA: f64 = 0.38;
/// The light comes round only once the breathing has begun.
const SHEEN_AFTER: f64 = 2000.0 / 14000.0;
/// How often a breathing mark is redrawn: its change between two frames
/// this far apart is far below a pixel and a shade.
const BREATH_FRAME: Duration = Duration::from_millis(100);
const SHEEN_FRAME: Duration = Duration::from_millis(33);

fn smooth(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The ring's radius at `t` along the stroke: round, a little uneven, and
/// opening outwards as the brush leaves.
fn radius(t: f64) -> f64 {
    1.0 + 0.012 * (t * 9.0).sin() + 0.16 * smooth(0.9, 1.0, t).powf(1.6)
}

/// Half the brush's width at `t`: thin on the left, full on the right,
/// landing at the start and lifting at the end.
fn half(t: f64) -> f64 {
    let mut w = 0.04 + 0.105 * smooth(0.12, 0.72, t);
    w *= 0.35 + 0.65 * smooth(0.0, 0.06, t);
    w *= 1.0 - 0.6 * smooth(0.86, 1.0, t);
    w
}

fn centre(t: f64) -> (f64, f64) {
    let a = FROM + (TO - FROM) * t;
    let r = radius(t);
    (r * a.cos(), r * a.sin())
}

fn hash(i: f64, j: f64) -> f64 {
    let s = (i * 127.1 + j * 311.7).sin() * 43758.5453;
    s - s.floor()
}

/// Smooth value noise for bristle `i` at `x`.
fn noise(i: f64, x: f64) -> f64 {
    let k = x.floor();
    let f = x - k;
    let u = f * f * (3.0 - 2.0 * f);
    hash(i, k) * (1.0 - u) + hash(i, k + 1.0) * u
}

/// A point of one bristle, in units of the ring's radius.
#[derive(Clone, Copy, Debug)]
struct Bristle {
    x: f32,
    y: f32,
    /// The brush's half-width here.
    half: f32,
    /// Whether this bristle leaves ink here.
    ink: bool,
    /// How much ink it leaves.
    alpha: f32,
}

/// Every bristle's points, worked out once.
fn bristles() -> &'static [Vec<Bristle>] {
    static BRISTLES_: OnceLock<Vec<Vec<Bristle>>> = OnceLock::new();
    BRISTLES_.get_or_init(|| {
        (0..BRISTLES)
            .map(|b| {
                let bf = b as f64;
                let across = -1.0 + 2.0 * (bf + 0.5) / BRISTLES as f64;
                let jitter = (hash(bf, 7.0) - 0.5) * 0.25;
                let alpha = (0.82 + 0.18 * hash(bf, 3.0)) as f32;
                (0..=STEPS)
                    .map(|s| {
                        let t = s as f64 / STEPS as f64;
                        let (x, y) = centre(t);
                        let (x2, y2) = centre((t + 1e-3).min(1.0));
                        let (mut nx, mut ny) = (-(y2 - y), x2 - x);
                        let n = nx.hypot(ny);
                        let n = if n > 0.0 { n } else { 1.0 };
                        nx /= n;
                        ny /= n;
                        let w = half(t);
                        let off = (across + jitter / BRISTLES as f64) * w;
                        let dry = 0.62 * smooth(0.74, 1.0, t) * (0.35 + 0.65 * across.abs())
                            + 0.12 * across.abs().powi(3);
                        Bristle {
                            x: (x + nx * off) as f32,
                            y: (y + ny * off) as f32,
                            half: w as f32,
                            ink: noise(bf, t * 46.0) > dry,
                            alpha,
                        }
                    })
                    .collect()
            })
            .collect()
    })
}

/// The points of group `g` of `bristle`, if it leaves ink there.
fn group(bristle: &[Bristle], g: usize) -> Option<&[Bristle]> {
    let s = 1 + g * GROUP;
    let points = bristle.get(s - 1..=s + GROUP - 1)?;
    (points[0].ink && points[GROUP].ink).then_some(points)
}

/// How thick a bristle's line is, for a ring of `radius`.
fn thickness(half: f32, radius: f32, least: f32) -> f32 {
    (2.0 * half * radius / BRISTLES as f32 * 1.9).max(least)
}

/// Where the mark sits on a screen of `bounds`: its centre, and the ring's
/// radius. Logical pixels, like `bounds`.
pub fn place(bounds: Rect) -> ((f32, f32), f32) {
    let radius = bounds.w.min(bounds.h) * 0.19;
    (
        (bounds.x + bounds.w / 2.0, bounds.y + bounds.h * 0.41),
        radius,
    )
}

/// The furthest the drawing reaches from the centre, for a ring of
/// `radius`: the glow's edge.
pub fn reach(radius: f32) -> f32 {
    radius * 2.2
}

/// The outline of one inked stretch of a bristle — `points`, at least two
/// — for a ring of `radius`: each point pushed out either side by half the
/// bristle's thickness there. Filled whole, a stretch has no seams.
fn outline(points: &[Bristle], radius: f32) -> Option<tiny_skia::Path> {
    let at = |i: usize| (points[i].x * radius, points[i].y * radius);
    let side = |i: usize, sign: f32| {
        let (before, after) = (at(i.saturating_sub(1)), at((i + 1).min(points.len() - 1)));
        let (dx, dy) = (after.0 - before.0, after.1 - before.1);
        let length = dx.hypot(dy).max(f32::EPSILON);
        let h = thickness(points[i].half, radius, 0.6) / 2.0 * sign;
        let (x, y) = at(i);
        (x - dy / length * h, y + dx / length * h)
    };
    let mut path = PathBuilder::new();
    let (x, y) = side(0, 1.0);
    path.move_to(x, y);
    for i in 1..points.len() {
        let (x, y) = side(i, 1.0);
        path.line_to(x, y);
    }
    for i in (0..points.len()).rev() {
        let (x, y) = side(i, -1.0);
        path.line_to(x, y);
    }
    path.close();
    path.finish()
}

/// The stroke painted at one size, in both byte orders.
struct Raster {
    /// The ring's radius in the output's pixels.
    radius: f32,
    rgba: Pixmap,
    bgra: Pixmap,
    /// How many of the stroke's points are painted.
    painted: usize,
}

impl Raster {
    fn new(radius: f32) -> Option<Self> {
        let side = (radius * 2.9).ceil().max(1.0) as u32;
        Some(Self {
            radius,
            rgba: Pixmap::new(side, side)?,
            bgra: Pixmap::new(side, side)?,
            painted: 0,
        })
    }

    /// Paint the stroke as far as point `upto`, afresh.
    fn paint(&mut self, upto: usize, ink: Rgba) {
        let upto = upto.min(STEPS);
        if upto == self.painted {
            return;
        }
        self.rgba.fill(Color::TRANSPARENT);
        self.bgra.fill(Color::TRANSPARENT);
        self.painted = upto;
        let side = self.rgba.width() as f32;
        let to_middle = Transform::from_translate(side / 2.0, side / 2.0);
        let channel = |c: u8| f32::from(c) / 255.0;
        let (r, g, b) = (channel(ink.r), channel(ink.g), channel(ink.b));
        for bristle in bristles() {
            let alpha = bristle[0].alpha * f32::from(ink.a) / 255.0;
            let painted = &bristle[..=upto];
            // Each stretch where the bristle leaves ink, whole.
            for stretch in painted.split(|point| !point.ink) {
                if stretch.len() < 2 {
                    continue;
                }
                let Some(path) = outline(stretch, self.radius) else {
                    continue;
                };
                for (pixmap, (r, b)) in [(&mut self.rgba, (r, b)), (&mut self.bgra, (b, r))] {
                    if let Some(color) = Color::from_rgba(r, g, b, alpha) {
                        let mut paint = Paint {
                            anti_alias: true,
                            ..Paint::default()
                        };
                        paint.set_color(color);
                        pixmap.fill_path(&path, &paint, FillRule::Winding, to_middle, None);
                    }
                }
            }
        }
    }
}

/// How soon a [`Mark`] wants drawing again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Every frame: it is being painted.
    Frames,
    /// After this long: it is breathing, or the light is passing.
    After(Duration),
    /// Not until something else changes: it is still.
    Rest,
}

/// The ensō, as a screen shows it: painted in or simply there, then
/// breathing for as long as it is shown.
pub struct Mark {
    ink: Rgba,
    light: Rgba,
    raster: Option<Raster>,
    /// How far along the stroke the painting is.
    brush: Tween<f32>,
    /// How much of the idle motion is let in.
    settle: Tween<f32>,
    /// When the breathing began: a breath and the light count from here.
    idle_since: Option<Instant>,
    breath: Duration,
    sheen: Duration,
    sheen_every: Duration,
    settle_in: Transition,
    /// Set moving by [`Mark::paint`] or [`Mark::show`], until [`Mark::rest`].
    moving: bool,
}

impl std::fmt::Debug for Mark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mark")
            .field("ink", &self.ink)
            .field("radius", &self.raster.as_ref().map(|raster| raster.radius))
            .finish_non_exhaustive()
    }
}

impl Mark {
    /// The mark in `ink`, with the light that passes over it in `light`:
    /// whole and still until told to move.
    pub fn new(ink: Rgba, light: Rgba) -> Self {
        Self {
            ink,
            light,
            raster: None,
            brush: Tween::new(1.0),
            settle: Tween::new(0.0),
            idle_since: None,
            breath: Duration::ZERO,
            sheen: Duration::ZERO,
            sheen_every: Duration::ZERO,
            settle_in: Transition::INSTANT,
            moving: false,
        }
    }

    fn timings(&mut self, motion: &Motion) {
        self.breath = motion.apply(Transition::BREATH).duration;
        self.sheen = motion.apply(Transition::SHEEN).duration;
        self.sheen_every = motion.apply(Transition::SHEEN_EVERY).duration;
        self.settle_in = motion.apply(Transition::SETTLE);
        self.moving = !self.breath.is_zero();
    }

    /// Paint it in from nothing, then breathe. Without motion it is just
    /// there, still.
    pub fn paint(&mut self, motion: &Motion, now: Instant) {
        self.timings(motion);
        self.brush.jump(0.0);
        self.brush.set(1.0, motion.apply(Transition::BRUSH), now);
        self.settle.jump(0.0);
        self.idle_since = None;
        if let Some(raster) = &mut self.raster {
            *raster = match Raster::new(raster.radius) {
                Some(fresh) => fresh,
                None => return,
            };
        }
    }

    /// Show it whole from the start, and begin breathing.
    pub fn show(&mut self, motion: &Motion, now: Instant) {
        self.timings(motion);
        self.brush.jump(1.0);
        self.settle.jump(0.0);
        self.idle_since = None;
        self.begin_idle(now);
    }

    /// Stop moving; whole and still. For a screen nobody is looking at.
    pub fn rest(&mut self) {
        self.moving = false;
        self.brush.jump(1.0);
        self.settle.jump(0.0);
        self.idle_since = None;
    }

    fn begin_idle(&mut self, now: Instant) {
        if self.breath.is_zero() || self.idle_since.is_some() {
            return;
        }
        self.idle_since = Some(now);
        self.settle.set(1.0, self.settle_in, now);
    }

    /// Note the painting ending.
    fn update(&mut self, now: Instant) {
        if self.moving && self.idle_since.is_none() && !self.brush.is_animating(now) {
            self.begin_idle(now);
        }
    }

    /// Where in a breath and in the light's round it is: the breath
    /// `0..=1`, how much of the idle motion is let in, and the light's
    /// progress if it is passing.
    fn idle(&self, now: Instant) -> (f32, f32, Option<f64>) {
        let Some(since) = self.idle_since else {
            return (0.0, 0.0, None);
        };
        let after = now.saturating_duration_since(since).as_secs_f64();
        let breath = if self.breath.is_zero() {
            0.0
        } else {
            0.5 - 0.5 * (2.0 * PI * after / self.breath.as_secs_f64()).cos()
        };
        let every = self.sheen_every.as_secs_f64();
        let sheen = (every > 0.0 && after > every * SHEEN_AFTER)
            .then(|| (after % every) / self.sheen.as_secs_f64().max(1e-3))
            .filter(|p| *p < 1.0);
        (breath as f32, self.settle.get(now), sheen)
    }

    /// When it next wants drawing.
    pub fn pace(&self, now: Instant) -> Pace {
        if self.brush.is_animating(now) {
            return Pace::Frames;
        }
        if self.idle_since.is_none() {
            // A painting just finished begins to breathe at the next draw.
            return if self.moving {
                Pace::After(BREATH_FRAME)
            } else {
                Pace::Rest
            };
        }
        match self.idle(now).2 {
            Some(_) => Pace::After(SHEEN_FRAME),
            None => Pace::After(BREATH_FRAME),
        }
    }

    /// Draw it centred on `centre` with a ring of `radius`, and its glow.
    /// Logical pixels.
    pub fn draw(&mut self, canvas: &mut Canvas<'_>, centre: (f32, f32), radius: f32, now: Instant) {
        self.update(now);
        let (breath, settle, sheen) = self.idle(now);
        let breathing = breath * settle;
        let brush = self.brush.get(now);

        // The glow comes up as the stroke goes down: nothing before it.
        canvas.glow(
            centre,
            radius * 0.2,
            reach(radius),
            self.ink,
            (GLOW + GLOW_BREATH * breathing) * brush,
        );

        let scale = canvas.scale();
        let pixels = (radius * scale).round().max(1.0);
        if self
            .raster
            .as_ref()
            .is_none_or(|raster| raster.radius != pixels)
        {
            self.raster = Raster::new(pixels);
        }
        let Some(raster) = &mut self.raster else {
            return;
        };
        let upto = (f64::from(brush) * STEPS as f64).floor() as usize;
        raster.paint(upto, self.ink);
        let zoom = 1.0 + SWELL * breathing;
        let side = raster.rgba.width();
        canvas.pixels_at(
            raster.rgba.data(),
            raster.bgra.data(),
            (side, side),
            centre,
            zoom,
        );

        if let Some(p) = sheen {
            light(canvas, self.light, centre, radius * zoom, p, settle);
        }
    }
}

/// The light travelling the stroke, `p` of the way through its run (`0..1`)
/// at `strength` (`0..1`), over a ring of `ring` centred on `centre`.
fn light(
    canvas: &mut Canvas<'_>,
    colour: Rgba,
    centre: (f32, f32),
    ring: f32,
    p: f64,
    strength: f32,
) {
    let head = -SHEEN_REACH + 1.3 * (0.5 - 0.5 * (PI * p).cos());
    let strength = SHEEN_ALPHA * (PI * p).sin() * f64::from(strength);
    let least = 0.6 / canvas.scale();
    let mut points = Vec::with_capacity(GROUP + 1);
    for g in 0..GROUPS {
        let t = (1 + g * GROUP) as f64 / STEPS as f64;
        if (t - head).abs() > SHEEN_REACH {
            continue;
        }
        let k = strength * (-(t - head).powi(2) / (2.0 * SHEEN_WIDTH.powi(2))).exp();
        if k <= 0.01 {
            continue;
        }
        for bristle in bristles() {
            let Some(group) = group(bristle, g) else {
                continue;
            };
            points.clear();
            points.extend(
                group
                    .iter()
                    .map(|p| (centre.0 + p.x * ring, centre.1 + p.y * ring)),
            );
            let width = thickness(group[0].half, ring, least);
            canvas.line(&points, width, colour, group[0].alpha * k as f32);
        }
    }
}

/// How far the terraces are quietened towards the base colour behind the
/// login, and how much they are blurred: there, but not competing.
const BACKDROP_TINT: f32 = 0.5;
const BACKDROP_BLUR: u32 = 2;

/// The voxel scene (terraces, river, mountains) in `colors`, `size`
/// pixels: the boot menu's picture.
pub fn scene(colors: &Palette, size: (u32, u32)) -> Picture {
    scene_with_life(colors, size).0
}

/// The scene, and what in it moves (see [`enso_scene::life`]).
pub fn scene_with_life(colors: &Palette, size: (u32, u32)) -> (Picture, enso_scene::Life) {
    let rgb = |c: Rgba| [c.r, c.g, c.b];
    let palette = enso_scene::Palette {
        base: rgb(colors.base),
        surface: rgb(colors.surface),
        overlay: rgb(colors.overlay),
        muted: rgb(colors.muted),
        subtle: rgb(colors.subtle),
        text: rgb(colors.text),
        love: rgb(colors.love),
        gold: rgb(colors.gold),
        rose: rgb(colors.rose),
        pine: rgb(colors.pine),
        foam: rgb(colors.foam),
        iris: rgb(colors.iris),
    };
    let (rgba, life) = enso_scene::render_with_life(size.0, size.1, &palette);
    let picture = Picture::from_rgba(size.0, size.1, rgba)
        .unwrap_or_else(|| Picture::solid(1, 1, colors.base));
    (picture, life)
}

/// The colours the scene's motion is painted in: as they are, or
/// `quiet`, taken half way to the base as the backdrop is.
pub fn life_ink(colors: &Palette, quiet: bool) -> enso_scene::life::Ink {
    let tone = |c: Rgba| {
        let c = if quiet {
            c.mix(colors.base, BACKDROP_TINT)
        } else {
            c
        };
        [c.r, c.g, c.b]
    };
    enso_scene::life::Ink {
        glint: tone(colors.text),
        mist: tone(colors.subtle),
        petals: [tone(colors.rose), tone(colors.rose.mix(colors.love, 0.55))],
    }
}

/// The scene quietened into the login screen's backdrop: softened, and
/// half way to `base`.
pub fn backdrop(scene: Picture, base: Rgba) -> Picture {
    scene.blurred(BACKDROP_BLUR).tinted(base, BACKDROP_TINT)
}

/// `backdrop` over `base`: the boot menu's last frame, the splash, and the
/// login screen's first, the same picture from one to the next.
pub fn stage(canvas: &mut Canvas<'_>, base: Rgba, backdrop: Option<&Picture>) {
    canvas.clear(base);
    if let Some(backdrop) = backdrop {
        canvas.picture(backdrop);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use enso_core::{Context, Paths};

    use super::*;
    use crate::{Offscreen, TextEngine};

    fn context() -> Context {
        Context::load_from(Paths::rooted("/nonexistent/enso-mark-test"))
    }

    #[test]
    fn the_stroke_is_an_open_ring() {
        let start = centre(0.0);
        let end = centre(1.0);
        // It starts upper left and ends past the top, opened outwards.
        assert!(start.0 < 0.0 && start.1 < 0.0);
        assert!(end.1 < 0.0);
        assert!(radius(1.0) > 1.1);
        // Thin where it lands and lifts, full on the right.
        assert!(half(0.0) < half(0.5) && half(1.0) < half(0.6));
        let points = bristles();
        assert_eq!(points.len(), BRISTLES);
        assert!(points.iter().all(|bristle| bristle.len() == STEPS + 1));
        // The dry end breaks up; the start is solid.
        let inked = |range: std::ops::Range<usize>| {
            points
                .iter()
                .flat_map(|b| &b[range.clone()])
                .filter(|p| p.ink)
                .count() as f32
                / (range.len() * BRISTLES) as f32
        };
        assert!(inked(0..200) > 0.85);
        assert!(inked(650..720) < inked(0..200));
    }

    #[test]
    fn painted_in_then_breathing_then_still() {
        let ctx = context();
        let motion = ctx.motion;
        let mut mark = Mark::new(ctx.theme.colors.rose, ctx.theme.colors.text);
        assert_eq!(mark.pace(Instant::now()), Pace::Rest);
        let start = Instant::now();
        mark.paint(&motion, start);
        assert_eq!(mark.pace(start), Pace::Frames);
        let done = start + Duration::from_secs(3);
        assert_eq!(mark.pace(done), Pace::After(BREATH_FRAME));
        mark.update(done);
        assert_eq!(mark.idle_since, Some(done));
        // The light passes a few seconds in.
        let (_, _, sheen) = mark.idle(done + Duration::from_secs(15));
        assert!(sheen.is_some());
        assert_eq!(
            mark.pace(done + Duration::from_secs(15)),
            Pace::After(SHEEN_FRAME)
        );
        mark.rest();
        assert_eq!(mark.pace(done), Pace::Rest);

        // Without motion: whole and still at once.
        let mut still = Mark::new(ctx.theme.colors.rose, ctx.theme.colors.text);
        still.paint(&Motion::none(), start);
        assert_eq!(still.pace(start), Pace::Rest);
        still.show(&Motion::none(), start);
        assert_eq!(still.pace(start), Pace::Rest);
    }

    fn render(size: (u32, u32), scale: u32, draw: impl FnOnce(&mut Canvas<'_>)) -> Offscreen {
        let ctx = context();
        let mut text = TextEngine::new(&ctx.config.appearance);
        Offscreen::render(&mut text, size, scale, draw)
    }

    #[test]
    fn both_byte_orders_are_the_same_picture() {
        let ctx = context();
        let mut raster = Raster::new(80.0).unwrap();
        raster.paint(STEPS, ctx.theme.colors.rose);
        let swapped: Vec<u8> = raster
            .bgra
            .data()
            .chunks(4)
            .flat_map(|px| [px[2], px[1], px[0], px[3]])
            .collect();
        assert!(swapped == raster.rgba.data());
        assert!(raster.rgba.data().iter().any(|&byte| byte != 0));
        // Painted again to the same point: nothing changes; to none: empty.
        raster.paint(STEPS, ctx.theme.colors.rose);
        raster.paint(0, ctx.theme.colors.rose);
        assert!(raster.rgba.data().iter().all(|&byte| byte == 0));
    }

    /// The ensō whole on its stage, at rest.
    fn frame(canvas: &mut Canvas<'_>, base: Rgba, ink: Rgba, backdrop: Option<&Picture>) {
        stage(canvas, base, backdrop);
        let (centre, radius) = place(canvas.bounds());
        Mark::new(ink, ink).draw(canvas, centre, radius, Instant::now());
    }

    #[test]
    fn renders_the_mark() {
        let ctx = context();
        let colors = ctx.theme.colors.clone();
        let still = render((480, 270), 2, |canvas| {
            frame(canvas, colors.base, colors.rose, None)
        });
        // The corners are the base, untouched by the glow.
        assert_eq!(
            still.pixel(0, 0),
            [colors.base.r, colors.base.g, colors.base.b, 255]
        );
        still.check(env!("CARGO_MANIFEST_DIR"), "mark");

        // Over the scene, quietened.
        let backdrop = backdrop(scene(&colors, (480, 270)), colors.base);
        let over = render((480, 270), 1, |canvas| {
            frame(canvas, colors.base, colors.rose, Some(&backdrop))
        });
        assert_ne!(
            over.pixel(0, 269),
            [colors.base.r, colors.base.g, colors.base.b, 255]
        );
        over.snapshot(env!("CARGO_MANIFEST_DIR"), "mark-backdrop");

        // Half painted; and breathing with the light on it.
        let start = Instant::now();
        let mut mark = Mark::new(colors.rose, colors.text);
        mark.paint(&ctx.motion, start);
        let half = render((480, 270), 2, |canvas| {
            canvas.clear(colors.base);
            let (centre, radius) = place(canvas.bounds());
            mark.draw(canvas, centre, radius, start + Duration::from_millis(1400));
        });
        half.snapshot(env!("CARGO_MANIFEST_DIR"), "mark-painting");
        let done = start + Duration::from_secs(3);
        let lit = render((480, 270), 2, |canvas| {
            canvas.clear(colors.base);
            let (centre, radius) = place(canvas.bounds());
            mark.draw(canvas, centre, radius, done);
            mark.draw(canvas, centre, radius, done + Duration::from_millis(16_500));
        });
        lit.snapshot(env!("CARGO_MANIFEST_DIR"), "mark-light");

        // At the size a 1080p screen shows it, to look at closely.
        let actual = render((640, 640), 1, |canvas| {
            canvas.clear(colors.base);
            Mark::new(colors.rose, colors.text).draw(canvas, (320.0, 320.0), 205.0, start);
        });
        actual.snapshot(env!("CARGO_MANIFEST_DIR"), "mark-1080p");
    }
}
