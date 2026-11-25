//! The sea in the clock's pill while something plays: along the foot of
//! the pill, under the words, three waves roll across at their own speeds —
//! the far one darker and slower, the nearest lightest. It rises like a
//! tide as playing starts and ebbs away when it stops; the words ride above
//! it on the island's own ground, the far wave now and then lapping at them.
//!
//! Every so often a set rolls in: a swell sweeps across from the left,
//! lifting the far waves first and the nearest highest a moment after, with
//! a line of foam along its crest that fades as it passes.

use std::{
    f32::consts::{PI, TAU},
    time::Duration,
};

use enso_core::Rgba;
use enso_ui::{Canvas, Rect, Shape};

/// How often the sea moves on: smooth enough for water, far short of every
/// frame.
pub const TICK: Duration = Duration::from_millis(33);
/// How far the drawn waves step along, in pixels.
const STEP: f32 = 1.0;
/// How often a set rolls in, and how long it takes to cross.
const SET_EVERY: f32 = 22.0;
const SET_FOR: f32 = 3.4;
/// How much higher a set lifts each wave, as part of the sea's depth —
/// the nearest most — and how much later it reaches each one.
const SET_LIFT: [f32; 3] = [0.10, 0.28, 0.38];
const SET_LAG: f32 = 0.22;

/// How much of the pill the sea takes, from its foot: the words sit above.
pub const SEA: f32 = 0.34;

/// One wave: where it rests (as part of the sea's depth, from its top),
/// how high it swells, how long it is, and how fast it rolls (crests a
/// second; negative rolls the other way).
struct Wave {
    rest: f32,
    swell: f32,
    length: f32,
    speed: f32,
}

const WAVES: [Wave; 3] = [
    Wave {
        rest: 0.14,
        swell: 0.22,
        length: 70.0,
        speed: 0.10,
    },
    Wave {
        rest: 0.42,
        swell: 0.20,
        length: 48.0,
        speed: -0.16,
    },
    Wave {
        rest: 0.72,
        swell: 0.16,
        length: 34.0,
        speed: 0.24,
    },
];

/// The sea's colours: the far wave, and the nearest; the one between is
/// mixed from the two.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub deep: Rgba,
    pub shallow: Rgba,
    /// The foam along a set's crest.
    pub foam: Rgba,
}

/// How far the pill reaches up and down at `x` from its left end: its round
/// ends are half circles. Returns `(top, bottom)` relative to its top.
fn extent(x: f32, w: f32, h: f32) -> (f32, f32) {
    let r = h / 2.0;
    let from_end = if x < r {
        r - x
    } else if x > w - r {
        x - (w - r)
    } else {
        0.0
    };
    let half = (r * r - from_end * from_end).max(0.0).sqrt();
    (r - half, r + half)
}

/// How much of a passing set is at `x` (of a pill `w` wide) at time `t`,
/// `0..=1`: a swell, steeper in front than behind, crossing from left to
/// right and rising and fading as it goes.
pub fn set(x: f32, t: f32, w: f32) -> f32 {
    let into = t.rem_euclid(SET_EVERY);
    if into >= SET_FOR || w <= 0.0 {
        return 0.0;
    }
    let p = into / SET_FOR;
    let centre = w * (-0.25 + 1.5 * p);
    // Steep in front, long behind, as a wave about to break.
    let width = if x > centre { w * 0.12 } else { w * 0.3 };
    let d = (x - centre) / width;
    (-d * d).exp() * (PI * p).sin()
}

/// Where wave `index`'s surface is at `x`, from the pill's top, at time `t`,
/// in a pill `w` by `h`, the tide `fill` of the way in: out with the tide,
/// every wave is below the pill.
fn surface(index: usize, x: f32, t: f32, w: f32, h: f32, fill: f32) -> f32 {
    let wave = &WAVES[index];
    let angle = TAU * (x / wave.length - t * wave.speed);
    // A shorter ripple on the long wave, so no two crests match.
    let ripple = (TAU * (x / (wave.length * 0.43) + t * wave.speed * 1.7)).sin() * 0.35;
    let lift = SET_LIFT[index] * set(x, t - index as f32 * SET_LAG, w);
    let (top, depth) = (h * (1.0 - SEA), h * SEA);
    let level = top + depth * (wave.rest + wave.swell * (angle.sin() + ripple) / 1.35 - lift);
    // Out with the tide, below the pill altogether.
    let out = depth * (1.0 - wave.rest + wave.swell + SET_LIFT[index]) + h * 0.05;
    level + (1.0 - fill) * out
}

/// What of the pill lies below `top(x)`, as a shape: the curve along the
/// top, the pill's own round edge along the bottom.
fn under(pill: Rect, top: impl Fn(f32) -> f32) -> Shape {
    let steps = (pill.w / STEP).ceil().max(1.0) as usize;
    let xs: Vec<f32> = (0..=steps).map(|i| (i as f32 * STEP).min(pill.w)).collect();
    let mut shape = Shape::new();
    for (i, &x) in xs.iter().enumerate() {
        let (upper, lower) = extent(x, pill.w, pill.h);
        let y = pill.y + top(x).clamp(upper, lower);
        if i == 0 {
            shape.move_to(pill.x + x, y);
        } else {
            shape.line_to(pill.x + x, y);
        }
    }
    for &x in xs.iter().rev() {
        let (_, lower) = extent(x, pill.w, pill.h);
        shape.line_to(pill.x + x, pill.y + lower);
    }
    shape.close();
    shape
}

/// Draw the sea into `pill`, the tide `fill` of the way in, at time `t`.
pub fn draw(canvas: &mut Canvas<'_>, pill: Rect, fill: f32, t: f32, look: Look) {
    if fill <= 0.0 || pill.w <= 0.0 || pill.h <= 0.0 {
        return;
    }
    let front = WAVES.len() - 1;
    for i in 0..WAVES.len() {
        let near = i as f32 / front as f32;
        let color = look.deep.mix(look.shallow, near);
        let top = |x: f32| surface(i, x, t, pill.w, pill.h, fill);
        canvas.fill_shape(&under(pill, top), color);
    }
    // Foam along the nearest wave where a set lifts it, and only well inside
    // the pill's round ends.
    let lag = t - front as f32 * SET_LAG;
    let steps = (pill.w / STEP).ceil() as usize;
    let mut foam = Shape::new();
    let mut drawing = false;
    let mut any = false;
    for i in 0..=steps {
        let x = (i as f32 * STEP).min(pill.w);
        let (upper, lower) = extent(x, pill.w, pill.h);
        let y = surface(front, x, t, pill.w, pill.h, fill);
        let crest = set(x, lag, pill.w);
        if crest > 0.25 && y > upper + 1.5 && y < lower - 1.5 {
            if drawing {
                foam.line_to(pill.x + x, pill.y + y);
            } else {
                foam.move_to(pill.x + x, pill.y + y);
                drawing = true;
                any = true;
            }
        } else {
            drawing = false;
        }
    }
    if any {
        let peak = (PI * (lag.rem_euclid(SET_EVERY) / SET_FOR).min(1.0)).sin();
        canvas.stroke_shape(&foam, 1.2, look.foam.alpha(0.6 * peak * fill));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_waves_stay_in_the_pill_and_ebb_away_with_the_tide() {
        let h = 28.0;
        for i in 0..300 {
            let (x, t) = (i as f32 * 0.9, i as f32 * 0.07);
            for wave in 0..WAVES.len() {
                let full = surface(wave, x, t, 180.0, h, 1.0);
                // Under the words: the far wave laps a little above the
                // sea's top at most, the nearer ones never.
                let reach = if wave == 0 { 0.35 } else { 0.1 };
                assert!(full > h * (1.0 - SEA * (1.0 + reach)) && full < h, "{wave}: {full}");
                assert!(surface(wave, x, t, 180.0, h, 0.0) >= h);
            }
        }
    }

    #[test]
    fn the_far_waves_ride_higher() {
        // Between sets.
        let mean = |wave: usize| {
            (0..200)
                .map(|i| surface(wave, i as f32, 10.0, 180.0, 28.0, 1.0))
                .sum::<f32>()
                / 200.0
        };
        assert!(mean(0) < mean(1) && mean(1) < mean(2));
    }

    #[test]
    fn a_set_rolls_in_now_and_then_from_the_left() {
        let w = 180.0;
        // Quiet between sets.
        assert!((0..180).all(|x| set(x as f32, SET_FOR + 1.0, w) == 0.0));
        // Early on it's at the left; later, further right.
        let crest_at = |t: f32| {
            (0..180)
                .max_by(|a, b| set(*a as f32, t, w).total_cmp(&set(*b as f32, t, w)))
                .unwrap_or(0)
        };
        assert!(crest_at(SET_FOR * 0.3) < crest_at(SET_FOR * 0.7));
        // And it lifts the nearest wave above where it would be.
        let t = SET_FOR * 0.5 + 2.0 * SET_LAG;
        let x = crest_at(SET_FOR * 0.5) as f32;
        let calm = surface(2, x, t + SET_EVERY / 2.0, w, 28.0, 1.0);
        let lifted = surface(2, x, t, w, 28.0, 1.0);
        assert!(lifted < calm || set(x, t - 2.0 * SET_LAG, w) > 0.5);
    }

    #[test]
    fn the_pill_rounds_off_at_its_ends() {
        let (w, h) = (120.0, 28.0);
        assert_eq!(extent(60.0, w, h), (0.0, 28.0));
        let (top, bottom) = extent(2.0, w, h);
        assert!(top > 5.0 && bottom < 23.0);
        assert_eq!(extent(0.0, w, h), (14.0, 14.0));
    }
}
