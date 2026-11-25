//! The pagoda island drawn a frame at a time, as the camera goes round it.
//!
//! The camera circles the island at a steady height, looking a little down
//! at it, once every [`PERIOD`] seconds. Turning about the upright, it sees
//! the same sky from every side but for the stars and the moon: [`sky`]
//! draws that sky once, and a frame redraws only the island and the few
//! small places the stars and the moon pass through ([`Orbit::paint`] says
//! which, for a screen that copies rectangles).
//!
//! The island's faces ([`crate::pagoda`]) are projected and filled four
//! samples to a pixel, the nearest face kept at each, then averaged, so
//! edges stay smooth as they turn. All of it needs no operating system: the
//! boot menu runs it in the firmware, on one processor.

use alloc::{vec, vec::Vec};

use libm::{cosf, floorf, powf, sinf, sqrtf, tanf};

use crate::{
    Palette,
    life::{Order, Rect},
    noise::{noise, smooth, unit},
    pagoda::{self, CENTRE, Face, GROUND, Look, Model, NORMALS},
};

/// Seconds for the camera to go once round.
pub const PERIOD: f32 = 80.0;
/// Where it starts: looking up the path, through the torii, a little from
/// one side.
const START: f32 = 0.55;
/// How far it looks down, in radians.
const PITCH: f32 = 0.02;
/// How much it sees from top to bottom, in radians.
const FOV: f32 = 0.55;
/// Where the island's middle sits on the screen: half way across, and
/// this share of the way down.
const AIM: f32 = 0.45;
/// What the camera looks at, and from how far.
const TARGET: [f32; 3] = [
    CENTRE[0] as f32 + 0.5,
    GROUND as f32 + 6.0,
    CENTRE[1] as f32 + 0.5,
];
const DISTANCE: f32 = 235.0;

/// The moon: its bearing, how high it stands (radians), and its size.
const MOON_BEARING: f32 = 3.95;
const MOON_HIGH: f32 = 0.2;
const MOON_SIZE: f32 = 0.0115;
/// Its halo reaches this many times its own size.
const HALO: f32 = 5.0;
/// Pixels across a block of the moon, at 1080 rows: it is made of voxels too.
const MOON_BLOCK: f32 = 6.0;
/// Stars in the sky, how high the highest stands (radians), and how much
/// the haze above the horizon hides of the lowest.
const STARS: u32 = 320;
const STARS_UP_TO: f32 = 0.55;
const STARS_FROM: f32 = 0.03;

type Vec3 = [f32; 3];

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Where the camera is when it has turned `angle` radians round, and how
/// it sees: the point of the screen anything falls on.
#[derive(Clone, Copy, Debug)]
struct Camera {
    eye: Vec3,
    right: Vec3,
    up: Vec3,
    ahead: Vec3,
    /// Pixels per unit at unit distance, and the screen point straight ahead.
    focal: f32,
    centre: (f32, f32),
}

impl Camera {
    fn new(width: u32, height: u32, angle: f32) -> Self {
        let (s, c) = (sinf(angle), cosf(angle));
        let (sp, cp) = (sinf(PITCH), cosf(PITCH));
        let back = [s * cp, sp, c * cp];
        Self {
            eye: [
                TARGET[0] + back[0] * DISTANCE,
                TARGET[1] + back[1] * DISTANCE,
                TARGET[2] + back[2] * DISTANCE,
            ],
            ahead: back.map(|v| -v),
            right: [c, 0.0, -s],
            up: [-s * sp, cp, -c * sp],
            focal: height as f32 / (2.0 * tanf(FOV / 2.0)),
            centre: (width as f32 / 2.0, height as f32 * AIM),
        }
    }

    /// The screen point of a direction (a star, the moon), if it is ahead.
    fn towards(&self, dir: Vec3) -> Option<(f32, f32)> {
        let z = dot(dir, self.ahead);
        (z > 0.05).then(|| {
            (
                self.centre.0 + self.focal * dot(dir, self.right) / z,
                self.centre.1 - self.focal * dot(dir, self.up) / z,
            )
        })
    }

    /// The screen point of a place, and one over its distance ahead.
    fn place(&self, p: Vec3) -> Option<(f32, f32, f32)> {
        let d = sub(p, self.eye);
        let z = dot(d, self.ahead);
        (z > 1.0).then(|| {
            let inverse = 1.0 / z;
            (
                self.centre.0 + self.focal * dot(d, self.right) * inverse,
                self.centre.1 - self.focal * dot(d, self.up) * inverse,
                inverse,
            )
        })
    }
}

/// How high above the horizon the screen point (`x`, `y`) looks, as the
/// sine of the angle: the same whichever way the camera has turned.
fn rise(width: u32, height: u32, x: f32, y: f32) -> f32 {
    let camera = Camera::new(width, height, 0.0);
    let (u, v) = (
        (x - camera.centre.0) / camera.focal,
        (camera.centre.1 - y) / camera.focal,
    );
    let up = cosf(PITCH) * v - sinf(PITCH);
    up / sqrtf(u * u + v * v + 1.0)
}

fn rgbf(c: [u8; 3]) -> [f32; 3] {
    c.map(|c| f32::from(c) / 255.0)
}

fn mixf(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// The sky behind the island, `width × height`, as opaque RGBA: dusk
/// above, the last light along the horizon, and below it a sea of cloud
/// banked away into the haze. It looks the same from every side, so it is
/// drawn once.
pub fn sky(width: u32, height: u32, palette: &Palette) -> Vec<u8> {
    let p = palette;
    let base = rgbf(p.base);
    let glow = mixf(base, mixf(rgbf(p.iris), rgbf(p.rose), 0.45), 0.42);
    let ember = mixf(base, rgbf(p.rose), 0.55);
    let haze = rgbf(pagoda::haze(p));
    let tops = mixf(haze, rgbf(p.text), 0.16);
    let deep = mixf(haze, base, 0.45);
    // A 4 × 4 ordered dither, so the long gradients don't band.
    const BAYER: [f32; 16] = [
        0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0,
    ];
    let mut out = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let s = rise(width, height, x as f32 + 0.5, y as f32 + 0.5);
            let colour = if s >= 0.0 {
                let mut sky = mixf(glow, base, powf(smooth(0.0, 0.3, s), 0.8));
                // The last of the warmth, low down.
                sky = mixf(sky, ember, (1.0 - smooth(0.0, 0.05, s)) * 0.35);
                sky
            } else {
                // The sea of cloud: bright at the horizon, banked in ridges
                // that come nearer and broader further down.
                let down = -s;
                let mut sea = mixf(glow, haze, smooth(0.0, 0.05, down));
                sea = mixf(sea, deep, smooth(0.08, 0.5, down));
                // Broad banks, nearer and wider further down, soft.
                let banks = 0.28 / (down + 0.02);
                let wave = 0.5 + 0.5 * sinf(banks * core::f32::consts::TAU);
                let crest = wave * wave * (1.0 - smooth(0.0, 0.35, down) * 0.5);
                sea = mixf(sea, tops, crest * 0.22 * smooth(0.002, 0.03, down));
                sea
            };
            let dither = (BAYER[((y % 4) * 4 + x % 4) as usize] + 0.5) / 16.0 - 0.5;
            let i = (y as usize * width as usize + x as usize) * 4;
            for c in 0..3 {
                out[i + c] = (colour[c] * 255.0 + dither).clamp(0.0, 255.0) as u8;
            }
            out[i + 3] = 255;
        }
    }
    out
}

/// A star: where it is in the sky, how bright, and where in its twinkle.
#[derive(Clone, Copy, Debug)]
struct Star {
    dir: Vec3,
    bright: f32,
    phase: f32,
}

/// A face ready to draw: its corners and middle, the way it looks, and its
/// colour.
#[derive(Clone, Copy, Debug)]
struct Quad {
    corners: [Vec3; 4],
    centre: Vec3,
    normal: Vec3,
    rgb: [u8; 3],
}

/// A face as it falls on the screen this frame.
#[derive(Clone, Copy, Debug)]
struct Shown {
    points: [(f32, f32); 4],
    depth: [f32; 4],
}

/// The pagoda island, drawing itself a frame at a time.
#[derive(Debug)]
pub struct Orbit {
    width: u32,
    height: u32,
    quads: Vec<Quad>,
    look: Look,
    stars: Vec<Star>,
    moon: MoonSprite,
    /// How far the island reaches from what the camera looks at.
    reach: f32,
    /// The screen's samples, two to a pixel each way: the nearest face at
    /// each and how near, `0` for none. Left empty after each frame.
    samples: Vec<u32>,
    shown: Vec<Shown>,
    colours: Vec<[u8; 3]>,
    /// What the last frame drew over the sky: the island's box, and the
    /// small places of the moon and the stars.
    island: Option<Rect>,
    drawn: Vec<Rect>,
    /// Places to redraw next frame whatever happens there.
    owed: Vec<Rect>,
    /// Nothing has been drawn yet: the whole screen is owed.
    fresh: bool,
}

impl Orbit {
    /// The island of `model` on a screen of `width × height`.
    pub fn new(model: &Model, width: u32, height: u32) -> Self {
        let quads: Vec<Quad> = model.faces.iter().map(quad).collect();
        let reach = quads
            .iter()
            .flat_map(|quad| quad.corners)
            .map(|c| {
                let d = sub(c, TARGET);
                sqrtf(dot(d, d))
            })
            .fold(0.0, f32::max);
        let stars = (0..STARS)
            .map(|i| {
                let i = i as i32;
                let bearing = unit(i, 0, 81) * core::f32::consts::TAU;
                // Evenly over the sky, thinning towards the horizon.
                let high = STARS_FROM + (STARS_UP_TO - STARS_FROM) * unit(i, 1, 81);
                let (sh, ch) = (sinf(high), cosf(high));
                Star {
                    dir: [ch * sinf(bearing), sh, ch * cosf(bearing)],
                    bright: 0.4 + 0.6 * powf(unit(i, 2, 81), 2.0),
                    phase: unit(i, 3, 81) * core::f32::consts::TAU,
                }
            })
            .collect();
        let focal = Camera::new(width, height, 0.0).focal;
        Self {
            width,
            height,
            quads,
            look: model.look,
            stars,
            moon: MoonSprite::new(focal, height),
            reach,
            samples: Vec::new(),
            shown: Vec::new(),
            colours: Vec::new(),
            island: None,
            drawn: Vec::new(),
            owed: Vec::new(),
            fresh: true,
        }
    }

    /// Have `rect` redrawn on the next frame, whether anything moves there
    /// or not: something was drawn over it.
    pub fn owe(&mut self, rect: Rect) {
        self.owed.push(rect);
    }

    /// Draw the scene as it is `t` seconds in onto `screen` (`width ×
    /// height` pixels, bytes in `order`), over `sky` (the same, from
    /// [`sky`]), where anything has changed since the last frame. Returns
    /// those places: what has to go to the display.
    pub fn paint(&mut self, sky: &[u8], screen: &mut [u8], order: Order, t: f32) -> Vec<Rect> {
        let (width, height) = (self.width, self.height);
        let size = width as usize * height as usize * 4;
        if sky.len() < size || screen.len() < size {
            return Vec::new();
        }
        let angle = START + t / PERIOD * core::f32::consts::TAU;
        let camera = Camera::new(width, height, angle);
        let whole = Rect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        };

        // What is where this frame.
        let island = self.project(&camera);
        let moon = self.moon(&camera);
        let stars: Vec<(f32, f32, f32)> = self
            .stars
            .iter()
            .filter_map(|star| {
                let (x, y) = camera.towards(star.dir)?;
                let twinkle = 0.62 + 0.38 * sinf(t * (1.1 + star.phase * 0.2) + star.phase);
                (x >= 0.0 && y >= 0.0 && x < width as f32 && y < height as f32).then_some((
                    x,
                    y,
                    star.bright * twinkle,
                ))
            })
            .collect();
        let mut now: Vec<Rect> = Vec::with_capacity(stars.len() + 1);
        now.extend(moon);
        now.extend(stars.iter().map(|&(x, y, _)| Rect {
            x: (x as u32).saturating_sub(1),
            y: (y as u32).saturating_sub(1),
            w: 3,
            h: 3,
        }));
        let mut changed: Vec<Rect> = if self.fresh {
            vec![whole]
        } else {
            let mut changed = core::mem::take(&mut self.drawn);
            changed.append(&mut self.owed);
            changed.extend_from_slice(&now);
            // The island's box then and now as one: they mostly overlap.
            if let Some(both) = union(self.island, island) {
                changed.push(both);
            }
            changed
        };
        changed.retain_mut(|rect| match rect.within(width, height) {
            Some(inside) => {
                *rect = inside;
                true
            }
            None => false,
        });
        self.fresh = false;

        // The sky back where anything was, then the moon and the stars,
        // then the island in front of them.
        let stride = width as usize * 4;
        for rect in &changed {
            for y in rect.y..rect.y + rect.h {
                let from = y as usize * stride + rect.x as usize * 4;
                let to = from + rect.w as usize * 4;
                screen[from..to].copy_from_slice(&sky[from..to]);
            }
        }
        let put = Put { width, order };
        if let Some(moon) = moon {
            self.draw_moon(screen, &put, moon, &camera);
        }
        for &(x, y, bright) in &stars {
            put.splat(screen, x, y, self.look.star, bright * 0.95);
        }
        if let Some(rect) = island {
            self.draw_island(screen, &put, rect);
        }
        self.island = island;
        self.drawn = now;
        changed
    }

    /// Project the faces turned to the camera; where they fall, if
    /// anywhere.
    fn project(&mut self, camera: &Camera) -> Option<Rect> {
        self.shown.clear();
        self.colours.clear();
        let haze = rgbf(self.look.haze);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for quad in &self.quads {
            let to_eye = sub(camera.eye, quad.centre);
            if dot(to_eye, quad.normal) <= 0.0 {
                continue;
            }
            let mut points = [(0.0, 0.0); 4];
            let mut depth = [0.0; 4];
            let mut ahead = true;
            for (k, corner) in quad.corners.iter().enumerate() {
                match camera.place(*corner) {
                    Some((x, y, inverse)) => {
                        points[k] = (x, y);
                        depth[k] = inverse;
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x);
                        y1 = y1.max(y);
                    }
                    None => ahead = false,
                }
            }
            if !ahead || self.shown.len() >= 65_535 {
                continue;
            }
            // The far side a shade further into the haze.
            let far = sqrtf(dot(to_eye, to_eye));
            let mist = smooth(DISTANCE - 40.0, DISTANCE + 80.0, far) * 0.3;
            let lit = mixf(rgbf(quad.rgb), haze, mist);
            self.colours
                .push(lit.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
            self.shown.push(Shown { points, depth });
        }
        if self.shown.is_empty() {
            return None;
        }
        let x = (floorf(x0) as i64).clamp(0, i64::from(self.width)) as u32;
        let y = (floorf(y0) as i64).clamp(0, i64::from(self.height)) as u32;
        let right = ((x1 as i64) + 2).clamp(0, i64::from(self.width)) as u32;
        let bottom = ((y1 as i64) + 2).clamp(0, i64::from(self.height)) as u32;
        (right > x && bottom > y).then_some(Rect {
            x,
            y,
            w: right - x,
            h: bottom - y,
        })
    }

    /// Fill the faces into `rect`, four samples a pixel, and put the
    /// result over what is on the screen there.
    fn draw_island(&mut self, screen: &mut [u8], put: &Put, rect: Rect) {
        let (sw, sh) = (self.width as usize * 2, self.height as usize * 2);
        if self.samples.len() != sw * sh {
            self.samples = vec![0; sw * sh];
        }
        // Nearness, as one over the distance, spread over 16 bits between
        // the island's nearest and furthest.
        let near = 1.0 / (DISTANCE - self.reach - 1.0).max(1.0);
        let far = 1.0 / (DISTANCE + self.reach + 1.0);
        let scale = 65_535.0 / (near - far);
        for (id, shown) in self.shown.iter().enumerate() {
            fill(
                &mut self.samples,
                sw,
                sh,
                shown,
                (far, scale),
                id as u32 + 1,
            );
        }
        // Each pixel: the average of its four samples, the screen behind
        // where a sample met nothing.
        let stride = self.width as usize * 4;
        for py in rect.y as usize..(rect.y + rect.h) as usize {
            let (top, low) = self.samples[py * 2 * sw..(py * 2 + 2) * sw].split_at_mut(sw);
            let row = py * stride;
            for px in rect.x as usize..(rect.x + rect.w) as usize {
                let keys = [top[px * 2], top[px * 2 + 1], low[px * 2], low[px * 2 + 1]];
                if keys == [0; 4] {
                    continue;
                }
                // Empty again for the next frame.
                top[px * 2..px * 2 + 2].fill(0);
                low[px * 2..px * 2 + 2].fill(0);
                let at = row + px * 4;
                let behind = put.read(&screen[at..at + 4]);
                let mut sum = [0u32; 3];
                for key in keys {
                    // A key left from an earlier frame can't happen; if it did, the
                    // sky shows there rather than the menu stopping.
                    let colour = (key & 0xffff)
                        .checked_sub(1)
                        .and_then(|id| self.colours.get(id as usize))
                        .copied()
                        .unwrap_or(behind);
                    for c in 0..3 {
                        sum[c] += u32::from(colour[c]);
                    }
                }
                put.write(&mut screen[at..at + 4], sum.map(|s| ((s + 2) / 4) as u8));
            }
        }
    }

    /// Where the moon is this frame: the square it and its halo cover.
    fn moon(&self, camera: &Camera) -> Option<Rect> {
        let (sh, ch) = (sinf(MOON_HIGH), cosf(MOON_HIGH));
        let dir = [ch * sinf(MOON_BEARING), sh, ch * cosf(MOON_BEARING)];
        let (x, y) = camera.towards(dir)?;
        let half = self.moon.half as f32;
        let (left, top) = (floorf(x) - half, floorf(y) - half);
        let side = self.moon.side as f32;
        let on_screen = left + side > 0.0
            && top + side > 0.0
            && left < self.width as f32
            && top < self.height as f32;
        on_screen.then(|| Rect {
            x: left.max(0.0) as u32,
            y: top.max(0.0) as u32,
            w: (left + side).min(self.width as f32) as u32 - left.max(0.0) as u32,
            h: (top + side).min(self.height as f32) as u32 - top.max(0.0) as u32,
        })
    }

    /// Put the moon over the screen within `rect`, where [`Orbit::moon`]
    /// placed it.
    fn draw_moon(&self, screen: &mut [u8], put: &Put, rect: Rect, camera: &Camera) {
        let (sh, ch) = (sinf(MOON_HIGH), cosf(MOON_HIGH));
        let dir = [ch * sinf(MOON_BEARING), sh, ch * cosf(MOON_BEARING)];
        let Some((x, y)) = camera.towards(dir) else {
            return;
        };
        let (left, top) = (
            floorf(x) as i64 - self.moon.half,
            floorf(y) as i64 - self.moon.half,
        );
        let colour = self.look.moon;
        let stride = self.width as usize * 4;
        let side = self.moon.side as usize;
        for py in rect.y..rect.y + rect.h {
            let sy = (i64::from(py) - top) as usize;
            for px in rect.x..rect.x + rect.w {
                let sx = (i64::from(px) - left) as usize;
                let a = u32::from(self.moon.alpha[sy * side + sx]);
                if a == 0 {
                    continue;
                }
                let at = py as usize * stride + px as usize * 4;
                let behind = put.read(&screen[at..at + 4]);
                let seen = core::array::from_fn(|c| {
                    let (b, m) = (u32::from(behind[c]), u32::from(colour[c]));
                    ((b * (255 - a) + m * a + 127) / 255) as u8
                });
                put.write(&mut screen[at..at + 4], seen);
            }
        }
    }
}

/// The moon as it is drawn over the sky: how much of its light covers each
/// pixel of a square round it, its halo and then its disc a block at a
/// time, faintly marked. Worked out once.
#[derive(Clone, Debug)]
struct MoonSprite {
    side: u32,
    /// From its middle to the square's edge.
    half: i64,
    alpha: Vec<u8>,
}

impl MoonSprite {
    fn new(focal: f32, height: u32) -> Self {
        let radius = MOON_SIZE * focal;
        let reach = radius * HALO;
        let half = reach as i64 + 1;
        let side = (half * 2 + 1) as u32;
        let block = (MOON_BLOCK * height as f32 / 1080.0).max(2.0);
        let mut alpha = vec![0u8; side as usize * side as usize];
        for sy in 0..side as i64 {
            for sx in 0..side as i64 {
                let (dx, dy) = ((sx - half) as f32 + 0.5, (sy - half) as f32 + 0.5);
                let d = sqrtf(dx * dx + dy * dy);
                if d >= reach {
                    continue;
                }
                let halo = 1.0 - d / reach;
                let mut cover = halo * halo * 0.22;
                let (bx, by) = (floorf(dx / block), floorf(dy / block));
                let (cx, cy) = ((bx + 0.5) * block, (by + 0.5) * block);
                if sqrtf(cx * cx + cy * cy) <= radius {
                    let mark = noise(bx * 0.9 + 3.0, by * 0.9 + 7.0, 91);
                    let disc = 0.88 - mark * 0.16;
                    cover = 1.0 - (1.0 - cover) * (1.0 - disc);
                }
                alpha[(sy * side as i64 + sx) as usize] = (cover * 255.0 + 0.5) as u8;
            }
        }
        Self { side, half, alpha }
    }
}

/// The smallest rectangle round both, if either is.
fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let (x, y) = (a.x.min(b.x), a.y.min(b.y));
            let right = (a.x + a.w).max(b.x + b.w);
            let bottom = (a.y + a.h).max(b.y + b.h);
            Some(Rect {
                x,
                y,
                w: right - x,
                h: bottom - y,
            })
        }
        (a, b) => a.or(b),
    }
}

fn quad(face: &Face) -> Quad {
    let n = NORMALS[usize::from(face.facing.min(5))];
    Quad {
        corners: face.corners(),
        centre: face.centre(),
        normal: n.map(|c| c as f32),
        rgb: face.rgb,
    }
}

/// Fill one face's samples where it is nearer than what is there: four
/// edges, each a line it stays on the inside of, stepped across the box
/// it falls in. Samples sit half a sample in, two to a pixel each way.
fn fill(
    samples: &mut [u32],
    sw: usize,
    sh: usize,
    shown: &Shown,
    (far, scale): (f32, f32),
    id: u32,
) {
    // Corners in sample space, in sixteenths.
    const SUB: f32 = 16.0;
    let v: [(i64, i64); 4] = shown.points.map(|(x, y)| {
        (
            libm::roundf(x * 2.0 * SUB) as i64,
            libm::roundf(y * 2.0 * SUB) as i64,
        )
    });
    let mut area = 0i64;
    for k in 0..4 {
        let (a, b) = (v[k], v[(k + 1) % 4]);
        area += a.0 * b.1 - b.0 * a.1;
    }
    if area == 0 {
        return;
    }
    let sign = if area > 0 { 1 } else { -1 };
    // E(x, y) = A·x + B·y + C, non-negative inside.
    let mut edges = [(0i64, 0i64, 0i64); 4];
    for k in 0..4 {
        let (a, b) = (v[k], v[(k + 1) % 4]);
        edges[k] = (
            (a.1 - b.1) * sign,
            (b.0 - a.0) * sign,
            (a.0 * b.1 - b.0 * a.1) * sign,
        );
    }
    let sub = SUB as i64;
    let lo = |c: i64| (c.div_euclid(sub)).max(0) as usize;
    let hi =
        |c: i64, limit: usize| ((c + sub - 1).div_euclid(sub) + 1).clamp(0, limit as i64) as usize;
    let (xs, ys) = (v.map(|p| p.0), v.map(|p| p.1));
    let (kx0, kx1) = (
        lo(*xs.iter().min().unwrap_or(&0)),
        hi(*xs.iter().max().unwrap_or(&0), sw),
    );
    let (ky0, ky1) = (
        lo(*ys.iter().min().unwrap_or(&0)),
        hi(*ys.iter().max().unwrap_or(&0), sh),
    );
    if kx0 >= kx1 || ky0 >= ky1 {
        return;
    }
    // Nearness over the face, a plane in sample space through three of
    // its corners: 16.16 fixed point.
    let corner = |k: usize| {
        (
            shown.points[k].0 * 2.0,
            shown.points[k].1 * 2.0,
            (shown.depth[k] - far) * scale,
        )
    };
    let plane = |a: (f32, f32, f32), b: (f32, f32, f32), c: (f32, f32, f32)| {
        let det = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
        (libm::fabsf(det) > 1e-4).then(|| {
            let qa = ((b.2 - a.2) * (c.1 - a.1) - (c.2 - a.2) * (b.1 - a.1)) / det;
            let qb = ((b.0 - a.0) * (c.2 - a.2) - (c.0 - a.0) * (b.2 - a.2)) / det;
            (qa, qb, a.2 - qa * a.0 - qb * a.1)
        })
    };
    let Some((qa, qb, qc)) =
        plane(corner(0), corner(1), corner(2)).or_else(|| plane(corner(0), corner(2), corner(3)))
    else {
        return;
    };
    let fixed = |q: f32| (q * 65_536.0) as i64;
    let (qa, qb) = (fixed(qa), fixed(qb));
    let qc = fixed(qc) + qa / 2 + qb / 2;
    let step = edges.map(|(a, _, _)| a * sub);
    for ky in ky0..ky1 {
        let y = ky as i64 * sub + sub / 2;
        let x = kx0 as i64 * sub + sub / 2;
        let mut e = edges.map(|(a, b, c)| a * x + b * y + c);
        let mut q = qa * kx0 as i64 + qb * ky as i64 + qc;
        let row = &mut samples[ky * sw..(ky + 1) * sw];
        for slot in &mut row[kx0..kx1] {
            if (e[0] | e[1] | e[2] | e[3]) >= 0 {
                let near = (q >> 16).clamp(1, 65_535) as u32;
                let key = (near << 16) | id;
                if key > *slot {
                    *slot = key;
                }
            }
            for k in 0..4 {
                e[k] += step[k];
            }
            q += qa;
        }
    }
}

/// Reading and writing a screen's pixels in its byte order.
struct Put {
    width: u32,
    order: Order,
}

impl Put {
    fn read(&self, px: &[u8]) -> [u8; 3] {
        match self.order {
            Order::Rgba => [px[0], px[1], px[2]],
            Order::Bgra => [px[2], px[1], px[0]],
        }
    }

    fn write(&self, px: &mut [u8], rgb: [u8; 3]) {
        let [r, g, b] = rgb;
        let bytes = match self.order {
            Order::Rgba => [r, g, b, 255],
            Order::Bgra => [b, g, r, 255],
        };
        px.copy_from_slice(&bytes);
    }

    /// A point of light at (`x`, `y`), shared among the four pixels
    /// round it by how near it is to each, so it glides as it moves.
    fn splat(&self, screen: &mut [u8], x: f32, y: f32, rgb: [u8; 3], strength: f32) {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (ix, iy) = (floorf(fx), floorf(fy));
        let (ax, ay) = (fx - ix, fy - iy);
        let colour = rgbf(rgb);
        let height = screen.len() / (self.width as usize * 4);
        for (dx, dy, w) in [
            (0, 0, (1.0 - ax) * (1.0 - ay)),
            (1, 0, ax * (1.0 - ay)),
            (0, 1, (1.0 - ax) * ay),
            (1, 1, ax * ay),
        ] {
            let (px, py) = (ix as i64 + dx, iy as i64 + dy);
            if px < 0 || py < 0 || px >= i64::from(self.width) || py >= height as i64 {
                continue;
            }
            let at = (py as usize * self.width as usize + px as usize) * 4;
            let behind = rgbf(self.read(&screen[at..at + 4]));
            let seen = mixf(behind, colour, (w * strength).clamp(0.0, 1.0));
            self.write(
                &mut screen[at..at + 4],
                seen.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8),
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const MOON: Palette = Palette {
        base: [0x23, 0x21, 0x36],
        surface: [0x2a, 0x27, 0x3f],
        overlay: [0x39, 0x35, 0x52],
        muted: [0x6e, 0x6a, 0x86],
        subtle: [0x90, 0x8c, 0xaa],
        text: [0xe0, 0xde, 0xf4],
        love: [0xeb, 0x6f, 0x92],
        gold: [0xf6, 0xc1, 0x77],
        rose: [0xea, 0x9a, 0x97],
        pine: [0x3e, 0x8f, 0xb0],
        foam: [0x9c, 0xcf, 0xd8],
        iris: [0xc4, 0xa7, 0xe7],
    };

    #[cfg(feature = "render")]
    fn save(name: &str, width: u32, height: u32, rgba: &[u8]) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(rgba)
            .unwrap();
    }

    #[test]
    fn the_sky_is_the_same_whichever_way_the_camera_turns() {
        for (x, y) in [(10.0, 10.0), (300.0, 200.0), (55.0, 170.0)] {
            let s = rise(320, 180, x, y);
            for angle in [0.3f32, 1.9, 4.0] {
                let camera = Camera::new(320, 180, angle);
                let (u, v) = (
                    (x - camera.centre.0) / camera.focal,
                    (camera.centre.1 - y) / camera.focal,
                );
                let ray = [
                    camera.right[0] * u + camera.up[0] * v + camera.ahead[0],
                    camera.right[1] * u + camera.up[1] * v + camera.ahead[1],
                    camera.right[2] * u + camera.up[2] * v + camera.ahead[2],
                ];
                let world = ray[1] / sqrtf(dot(ray, ray));
                assert!((world - s).abs() < 1e-5, "{world} {s}");
            }
        }
    }

    /// Redrawing only what changed, frame after frame, ends where drawing
    /// the whole screen at that moment does.
    #[test]
    fn a_frame_leaves_nothing_behind() {
        let model = Model::build(&MOON);
        let (width, height) = (240, 135);
        let sky = sky(width, height, &MOON);
        let mut orbit = Orbit::new(&model, width, height);
        let mut screen = sky.clone();
        let mut t = 0.0;
        for _ in 0..40 {
            orbit.paint(&sky, &mut screen, Order::Rgba, t);
            t += 1.7;
        }
        let mut fresh = Orbit::new(&model, width, height);
        let mut whole = vec![0; screen.len()];
        let all = fresh.paint(&sky, &mut whole, Order::Rgba, t - 1.7);
        assert_eq!(all.len(), 1);
        assert!(screen == whole, "a frame left something behind");
        assert!(screen != sky, "nothing was drawn");
    }

    /// Frames to look at, and how long one takes on one processor:
    /// `cargo test -p enso-scene --release -- --ignored orbit --nocapture`.
    #[cfg(feature = "render")]
    #[test]
    #[ignore = "seconds of drawing; run to look at the frames"]
    fn orbit_frames() {
        let model = Model::build(&MOON);
        println!("{} faces", model.faces.len());
        let (width, height) = (1920, 1080);
        let sky = sky(width, height, &MOON);
        let mut orbit = Orbit::new(&model, width, height);
        let mut screen = sky.clone();
        let started = std::time::Instant::now();
        let frames = 120;
        let mut most = std::time::Duration::ZERO;
        for frame in 0..frames {
            let t = frame as f32 / 30.0;
            let one = std::time::Instant::now();
            let changed = orbit.paint(&sky, &mut screen, Order::Rgba, t);
            most = most.max(one.elapsed());
            if frame == 1 {
                let pixels: u32 = changed.iter().map(|r| r.w * r.h).sum();
                println!("{} rects, {pixels} pixels changed", changed.len());
            }
        }
        println!(
            "{:.1} ms a frame on average, {:.1} at most",
            started.elapsed().as_secs_f64() * 1000.0 / frames as f64,
            most.as_secs_f64() * 1000.0
        );
        for (n, t) in [0.0f32, 13.0, 27.0, 40.0, 53.0, 67.0]
            .into_iter()
            .enumerate()
        {
            let mut fresh = Orbit::new(&model, width, height);
            let mut shot = sky.clone();
            fresh.paint(&sky, &mut shot, Order::Rgba, t);
            save(&format!("pagoda-{n}"), width, height, &shot);
        }
    }
}
