//! The scene's small motions: glints running over the water, stars
//! twinkling, petals falling from the cherry trees, and mist drifting along
//! the foot of the mountains.
//!
//! The picture is rendered once; what moves is painted over it, a frame at
//! a time, from a [`Life`]: what the renderer noted about the picture
//! (where its water, stars and blossom are, and the band of the far
//! valley). Painting only touches those few places, and [`Life::regions`]
//! says which, so a screen that can only copy rectangles (the boot menu,
//! in the firmware) redraws a fraction of itself each frame.
//!
//! This part needs no operating system: the boot menu runs it as well as
//! the login screen, and both move the same way.

use alloc::vec::Vec;

use libm::{floorf, sinf};

/// A run of water pixels in one row: `x0..x1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub y: u16,
    pub x0: u16,
    pub x1: u16,
}

/// A star: a single pixel of the sky.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Star {
    pub x: u16,
    pub y: u16,
    /// Where in its twinkle it starts, and how fast it goes.
    pub phase: u8,
}

/// What moves in a picture, and where.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Life {
    pub width: u16,
    pub height: u16,
    pub water: Vec<Run>,
    pub stars: Vec<Star>,
    /// Points on the cherry trees' crowns, where petals come loose.
    pub blossom: Vec<(u16, u16)>,
    /// Rows `from..to` where the mist lies: the far valley.
    pub mist: (u16, u16),
}

/// A rectangle of pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Which byte of a pixel is which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Rgba,
    Bgra,
}

/// The colours motion is painted in, as RGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ink {
    /// Light on the water, and stars at their brightest.
    pub glint: [u8; 3],
    pub mist: [u8; 3],
    /// Two blossom colours, pale and deep.
    pub petals: [[u8; 3]; 2],
}

/// Petals in the air at once.
const PETALS: usize = 32;
/// The side of a petal, in pixels: one voxel at the trees' distance.
const PETAL: u32 = 4;

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `0..1`, the same everywhere.
fn hash(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x8da6_b343) ^ b.wrapping_mul(0xd816_3841);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Smooth value noise, `0..1`.
fn noise(x: f32, y: f32) -> f32 {
    let (xf, yf) = (floorf(x), floorf(y));
    let (fx, fy) = (x - xf, y - yf);
    let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (xi, yi) = (xf as i32 as u32, yf as i32 as u32);
    let a = hash(xi, yi);
    let b = hash(xi.wrapping_add(1), yi);
    let c = hash(xi, yi.wrapping_add(1));
    let d = hash(xi.wrapping_add(1), yi.wrapping_add(1));
    let top = a + (b - a) * u;
    let bottom = c + (d - c) * u;
    top + (bottom - top) * v
}

/// Reading through kept bytes.
struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(n)?;
        self.0 = tail;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
}

/// A falling petal at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Petal {
    x: f32,
    y: f32,
    alpha: f32,
    colour: usize,
}

impl Rect {
    fn grown(self, by: u32) -> Self {
        Self {
            x: self.x.saturating_sub(by),
            y: self.y.saturating_sub(by),
            w: self.w + 2 * by,
            h: self.h + 2 * by,
        }
    }

    /// This rectangle within `width × height`, if any of it is.
    pub(crate) fn within(self, width: u32, height: u32) -> Option<Self> {
        let x1 = (self.x + self.w).min(width);
        let y1 = (self.y + self.h).min(height);
        (self.x < x1 && self.y < y1).then(|| Self {
            x: self.x,
            y: self.y,
            w: x1 - self.x,
            h: y1 - self.y,
        })
    }

    fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

impl Life {
    const MAGIC: &'static [u8; 8] = b"ENSOLIF1";

    /// The bytes this is kept as.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(Self::MAGIC);
        let mut put = |n: u32| out.extend_from_slice(&n.to_le_bytes());
        put(u32::from(self.width));
        put(u32::from(self.height));
        put(u32::from(self.mist.0));
        put(u32::from(self.mist.1));
        put(self.water.len() as u32);
        put(self.stars.len() as u32);
        put(self.blossom.len() as u32);
        for run in &self.water {
            for n in [run.y, run.x0, run.x1] {
                out.extend_from_slice(&n.to_le_bytes());
            }
        }
        for star in &self.stars {
            out.extend_from_slice(&star.x.to_le_bytes());
            out.extend_from_slice(&star.y.to_le_bytes());
            out.push(star.phase);
        }
        for (x, y) in &self.blossom {
            out.extend_from_slice(&x.to_le_bytes());
            out.extend_from_slice(&y.to_le_bytes());
        }
        out
    }

    /// Read back what [`Life::encode`] wrote. `None` for anything else.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut at = Cursor(bytes.strip_prefix(Self::MAGIC)?);
        let width = u16::try_from(at.u32()?).ok()?;
        let height = u16::try_from(at.u32()?).ok()?;
        let mist = (
            u16::try_from(at.u32()?).ok()?,
            u16::try_from(at.u32()?).ok()?,
        );
        let (runs, stars, blossom) = (at.u32()? as usize, at.u32()? as usize, at.u32()? as usize);
        // No more than the bytes could hold: a damaged file is refused, not
        // allocated for.
        let needed = runs
            .checked_mul(6)?
            .checked_add(stars.checked_mul(5)?)?
            .checked_add(blossom.checked_mul(4)?)?;
        if needed != at.0.len() {
            return None;
        }
        let mut water = Vec::with_capacity(runs);
        for _ in 0..runs {
            water.push(Run {
                y: at.u16()?,
                x0: at.u16()?,
                x1: at.u16()?,
            });
        }
        let mut list = Vec::with_capacity(stars);
        for _ in 0..stars {
            let (x, y) = (at.u16()?, at.u16()?);
            let phase = *at.take(1)?.first()?;
            list.push(Star { x, y, phase });
        }
        let mut points = Vec::with_capacity(blossom);
        for _ in 0..blossom {
            points.push((at.u16()?, at.u16()?));
        }
        Some(Self {
            width,
            height,
            water,
            stars: list,
            blossom: points,
            mist,
        })
    }

    /// Petal `i` at `t` seconds, if it is in the air.
    fn petal(&self, i: usize, t: f32) -> Option<Petal> {
        if self.blossom.is_empty() {
            return None;
        }
        let i32_ = i as u32;
        let span = 7.0 + 4.0 * hash(i32_, 1);
        let shifted = t + span * hash(i32_, 2);
        let round = floorf(shifted / span);
        let age = shifted - round * span;
        let pick = (hash(i32_, round as u32 ^ 0x5bd1) * self.blossom.len() as f32) as usize;
        let (sx, sy) = self.blossom[pick.min(self.blossom.len() - 1)];
        let fall = 14.0 + 12.0 * hash(i32_, 3);
        // A light wind from the left, and each petal's own tumble.
        let drift = 5.0 + 7.0 * (hash(i32_, 4) - 0.3);
        let sway = 5.0 * sinf(age * (1.3 + hash(i32_, 5)) + i as f32);
        let alpha = smooth(0.0, 0.6, age) * (1.0 - smooth(span - 1.4, span, age));
        Some(Petal {
            x: f32::from(sx) + drift * age + sway,
            y: f32::from(sy) + fall * age,
            alpha,
            colour: (hash(i32_, round as u32) > 0.6) as usize,
        })
    }

    fn petal_rect(p: &Petal) -> Rect {
        Rect {
            x: p.x.max(0.0) as u32,
            y: p.y.max(0.0) as u32,
            w: PETAL + 1,
            h: PETAL + 1,
        }
    }

    /// The water's bounds, once.
    fn water_bounds(&self) -> Option<Rect> {
        let first = self.water.first()?;
        let (mut x0, mut x1, mut y0, mut y1) = (first.x0, first.x1, first.y, first.y);
        for run in &self.water {
            x0 = x0.min(run.x0);
            x1 = x1.max(run.x1);
            y0 = y0.min(run.y);
            y1 = y1.max(run.y);
        }
        Some(Rect {
            x: u32::from(x0),
            y: u32::from(y0),
            w: u32::from(x1 - x0),
            h: u32::from(y1 - y0) + 1,
        })
    }

    /// The places that look different at `t` from how they looked at
    /// `before`: what to redraw, and nothing else.
    pub fn regions(&self, before: f32, t: f32) -> Vec<Rect> {
        let (width, height) = (u32::from(self.width), u32::from(self.height));
        let mut out = Vec::new();
        if let Some(water) = self.water_bounds() {
            out.push(water);
        }
        if self.mist.1 > self.mist.0 {
            out.push(Rect {
                x: 0,
                y: u32::from(self.mist.0),
                w: width,
                h: u32::from(self.mist.1 - self.mist.0),
            });
        }
        for star in &self.stars {
            out.push(Rect {
                x: u32::from(star.x),
                y: u32::from(star.y),
                w: 1,
                h: 1,
            });
        }
        for i in 0..PETALS {
            for at in [before, t] {
                if let Some(p) = self.petal(i, at) {
                    out.push(Self::petal_rect(&p).grown(1));
                }
            }
        }
        out.into_iter()
            .filter_map(|rect| rect.within(width, height))
            .collect()
    }

    /// Paint `rect` of `out` as the picture `base` looks at `t` seconds:
    /// both `width × height` pixels in `order`. `strength` (`0..1`) is how
    /// much of the motion shows; `0` is the still picture.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &self,
        base: &[u8],
        out: &mut [u8],
        order: Order,
        rect: Rect,
        t: f32,
        strength: f32,
        ink: &Ink,
    ) {
        let (width, height) = (u32::from(self.width), u32::from(self.height));
        let Some(rect) = rect.within(width, height) else {
            return;
        };
        let row_bytes = width as usize * 4;
        if base.len() < row_bytes * height as usize || out.len() < base.len() {
            return;
        }
        // The still picture first.
        for y in rect.y..rect.y + rect.h {
            let from = y as usize * row_bytes + rect.x as usize * 4;
            let to = from + rect.w as usize * 4;
            out[from..to].copy_from_slice(&base[from..to]);
        }
        if strength <= 0.0 {
            return;
        }
        let colour = |rgb: [u8; 3]| match order {
            Order::Rgba => rgb,
            Order::Bgra => [rgb[2], rgb[1], rgb[0]],
        };
        let mix = |pixel: &mut [u8], to: [u8; 3], amount: f32| {
            let amount = amount.clamp(0.0, 1.0);
            for (c, target) in pixel.iter_mut().zip(to) {
                *c = (f32::from(*c) + (f32::from(target) - f32::from(*c)) * amount + 0.5) as u8;
            }
        };
        let at = |x: u32, y: u32| (y as usize * row_bytes) + x as usize * 4;

        // Glints on the water: two slow ripples crossing, lit only where
        // both crest, in two-pixel blocks like the rest of the voxels.
        let glint = colour(ink.glint);
        for run in &self.water {
            let y = u32::from(run.y);
            if y < rect.y || y >= rect.y + rect.h {
                continue;
            }
            let x0 = u32::from(run.x0).max(rect.x);
            let x1 = u32::from(run.x1).min(rect.x + rect.w);
            let by = (y / 2) as f32;
            for x in x0..x1 {
                let bx = (x / 2) as f32;
                let ripple =
                    sinf(bx * 0.09 + by * 0.21 - t * 1.1) * sinf(bx * 0.033 - by * 0.12 + t * 0.55);
                let lit = smooth(0.86, 1.0, ripple) * 0.4 * strength;
                if lit > 0.004 {
                    let i = at(x, y);
                    mix(&mut out[i..i + 3], glint, lit);
                }
            }
        }

        // Mist along the foot of the mountains, drifting right.
        let (m0, m1) = (u32::from(self.mist.0), u32::from(self.mist.1));
        if m1 > m0 {
            let mist = colour(ink.mist);
            let band = (m1 - m0) as f32;
            for y in rect.y.max(m0)..(rect.y + rect.h).min(m1) {
                let across = (y - m0) as f32 / band;
                let shape = sinf(core::f32::consts::PI * across);
                for x in rect.x..rect.x + rect.w {
                    let n = noise(x as f32 / 170.0 - t * 0.035, y as f32 / 26.0 + t * 0.01);
                    let amount = shape * shape * (0.25 + 0.75 * n) * 0.2 * strength;
                    let i = at(x, y);
                    mix(&mut out[i..i + 3], mist, amount);
                }
            }
        }

        // Stars: each brightens and fades on its own slow beat, against
        // the sky a step to its left.
        for star in &self.stars {
            let (x, y) = (u32::from(star.x), u32::from(star.y));
            if !rect.contains(x, y) || x < 2 {
                continue;
            }
            let phase = f32::from(star.phase) / 255.0;
            let beat = 0.5 + 0.5 * sinf(t * (0.6 + 0.9 * phase) + phase * core::f32::consts::TAU);
            let (i, sky) = (at(x, y), at(x - 2, y));
            let mut sky_px = [0u8; 3];
            sky_px.copy_from_slice(&base[sky..sky + 3]);
            let mut lit = [0u8; 3];
            lit.copy_from_slice(&base[i..i + 3]);
            mix(&mut lit, glint, beat * 0.35);
            let mut px = sky_px;
            mix(&mut px, lit, 1.0 - strength + strength * (0.2 + 0.8 * beat));
            out[i..i + 3].copy_from_slice(&px);
        }

        // Petals, drifting down from the trees.
        for i in 0..PETALS {
            let Some(p) = self.petal(i, t) else {
                continue;
            };
            let ink_ = colour(ink.petals[p.colour]);
            let (px, py) = (p.x as i64, p.y as i64);
            for dy in 0..PETAL as i64 {
                for dx in 0..PETAL as i64 {
                    let (x, y) = (px + dx, py + dy);
                    if x < 0 || y < 0 || !rect.contains(x as u32, y as u32) {
                        continue;
                    }
                    let i = at(x as u32, y as u32);
                    mix(&mut out[i..i + 3], ink_, p.alpha * strength * 0.9);
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use alloc::vec;

    use super::*;

    fn life() -> Life {
        Life {
            width: 40,
            height: 30,
            water: vec![
                Run {
                    y: 20,
                    x0: 2,
                    x1: 30,
                },
                Run {
                    y: 21,
                    x0: 4,
                    x1: 28,
                },
            ],
            stars: vec![Star {
                x: 10,
                y: 3,
                phase: 40,
            }],
            blossom: vec![(12, 10), (20, 11)],
            mist: (12, 18),
        }
    }

    const INK: Ink = Ink {
        glint: [255, 255, 255],
        mist: [200, 200, 220],
        petals: [[234, 154, 151], [235, 111, 146]],
    };

    #[test]
    fn kept_and_read_back() {
        let life = life();
        assert_eq!(Life::decode(&life.encode()), Some(life.clone()));
        let bytes = life.encode();
        assert_eq!(Life::decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(Life::decode(b"nonsense"), None);
    }

    #[test]
    fn still_at_no_strength_and_moving_otherwise() {
        let life = life();
        let base = vec![40u8; 40 * 30 * 4];
        let all = Rect {
            x: 0,
            y: 0,
            w: 40,
            h: 30,
        };
        let mut out = vec![0u8; base.len()];
        life.paint(&base, &mut out, Order::Rgba, all, 3.0, 0.0, &INK);
        assert!(out == base);
        // Over a few seconds something changes, and only in the regions
        // said to change.
        let mut moved = false;
        for step in 0..40 {
            let t = step as f32 * 0.25;
            life.paint(&base, &mut out, Order::Bgra, all, t, 1.0, &INK);
            moved |= out != base;
            let regions = life.regions(t, t);
            for (i, (a, b)) in out.chunks(4).zip(base.chunks(4)).enumerate() {
                if a != b {
                    let (x, y) = ((i % 40) as u32, (i / 40) as u32);
                    assert!(regions.iter().any(|r| r.contains(x, y)), "{x},{y} at {t}");
                }
            }
        }
        assert!(moved);
        // Painting part of the screen touches nothing outside it.
        let mut part = base.clone();
        let rect = Rect {
            x: 5,
            y: 19,
            w: 6,
            h: 3,
        };
        life.paint(&base, &mut part, Order::Rgba, rect, 1.0, 1.0, &INK);
        for (i, (a, b)) in part.chunks(4).zip(base.chunks(4)).enumerate() {
            let (x, y) = ((i % 40) as u32, (i / 40) as u32);
            if !rect.contains(x, y) {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn petals_fall_and_come_back() {
        let life = life();
        let a = life.petal(0, 1.0).unwrap();
        let b = life.petal(0, 2.0).unwrap();
        assert!(b.y > a.y);
        // A petal's round ends and it starts again from a tree.
        let ys: Vec<f32> = (0..400)
            .map(|k| life.petal(0, k as f32 * 0.1).unwrap().y)
            .collect();
        assert!(ys.windows(2).any(|w| w[1] < w[0]));
        assert_eq!(Life::default().petal(0, 1.0), None);
    }
}
