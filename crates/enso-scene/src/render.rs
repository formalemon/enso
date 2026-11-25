//! The renderer: the land, the rays, the light.

use std::{collections::HashMap, vec, vec::Vec};

use crate::{
    Palette,
    life::{Life, Run, Star},
    noise::{fbm, hash, lerp, noise, smooth, unit},
};

/// The land's size, in voxels: across, and away from the camera.
const WIDTH: i32 = 384;
const DEPTH: i32 = 560;
/// Nothing is taller than this.
const CEILING: f32 = 200.0;
/// The river's surface, and the land's lowest.
const WATER: i32 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stuff {
    /// The river.
    River,
    /// A flooded rice paddy: water, a little greener than the river.
    Paddy,
    /// Ripe rice.
    Rice,
    /// The low banks round a terrace.
    Bund,
    /// Sand at the river's edge.
    Sand,
    /// Bare earth: terrace walls, banks.
    Earth,
    /// Hillside grass.
    Grass,
    Rock,
    Snow,
    /// Cherry blossom, pale and deep.
    Blossom,
    Bloom,
    /// Evergreen.
    Needles,
    Trunk,
}

/// Sharp-crested noise for mountain ridges, `0..1`.
fn ridges(x: f32, y: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 0.5, 1.0, 0.0);
    for octave in 0..5 {
        let n = 1.0 - (noise(x * freq, y * freq, seed + octave) * 2.0 - 1.0).abs();
        sum += amp * n * n;
        norm += amp;
        amp *= 0.5;
        freq *= 2.1;
    }
    sum / norm
}

/// Where the river runs at depth `z`, and half its width there.
fn river(z: f32) -> (f32, f32) {
    let wander =
        (fbm(z / 140.0, 3.5, 11, 3) - 0.5) * 150.0 + (fbm(z / 45.0, 8.0, 12, 2) - 0.5) * 30.0;
    // It comes from between the mountains, and widens towards us.
    let centre = WIDTH as f32 * 0.46 + wander * smooth(-40.0, 120.0, z);
    // From its spring among the foothills, widening towards us.
    let half = (4.0 + 11.0 * (1.0 - smooth(0.0, 360.0, z))) * (1.0 - smooth(300.0, 380.0, z));
    (centre, half)
}

/// The high peaks: where each stands, how high, how wide at the foot.
const PEAKS: [(f32, f32, f32, f32); 6] = [
    (130.0, 505.0, 185.0, 95.0),
    (205.0, 540.0, 150.0, 90.0),
    (290.0, 480.0, 105.0, 70.0),
    (345.0, 530.0, 140.0, 85.0),
    (40.0, 470.0, 95.0, 70.0),
    (250.0, 420.0, 60.0, 55.0),
];

/// The peaks' height at (`x`, `z`): each a cone, roughened by ridges.
fn mountains(x: f32, z: f32) -> f32 {
    let rough = ridges(x / 55.0, z / 45.0, 6);
    PEAKS
        .iter()
        .map(|&(px, pz, height, foot)| {
            let d = ((x - px).powi(2) + (z - pz).powi(2)).sqrt() / foot;
            height * (1.0 - d).max(0.0).powf(1.25) * (0.72 + 0.4 * rough)
        })
        .fold(0.0, f32::max)
}

/// Voxels standing over a column: from, to (exclusive), and what.
type Span = (i32, i32, Stuff);

/// The land, worked out once.
struct Land {
    height: Vec<i32>,
    top: Vec<Stuff>,
    side: Vec<Stuff>,
    /// Trees: spans of voxels standing over a column, `(bottom, top)`
    /// exclusive of the top, and what they are.
    spans: HashMap<(i32, i32), Vec<Span>>,
}

impl Land {
    fn at(&self, x: i32, z: i32) -> Option<usize> {
        ((0..WIDTH).contains(&x) && (0..DEPTH).contains(&z)).then(|| (z * WIDTH + x) as usize)
    }

    fn height(&self, x: i32, z: i32) -> i32 {
        self.at(x, z).map_or(0, |i| self.height[i])
    }

    fn new() -> Self {
        let cells = (WIDTH * DEPTH) as usize;
        let mut raw = vec![0.0f32; cells];
        let mut terraced = vec![false; cells];
        for z in 0..DEPTH {
            let zf = z as f32;
            let (centre, half) = river(zf);
            let far = smooth(300.0, 470.0, zf);
            for x in 0..WIDTH {
                let xf = x as f32;
                let away = ((xf - centre).abs() - half).max(0.0);
                let roll = fbm(xf / 60.0, zf / 60.0, 1, 4);
                // Hills rising from the river on both sides.
                // Gentle by the water near us, steeper further off.
                let steep = 0.10 + 0.22 * smooth(10.0, 140.0, zf) + 0.12 * roll;
                let hills = WATER as f32 + 1.0 + away * steep + roll * 8.0 * smooth(0.0, 80.0, zf);
                let hills = hills.min(46.0 + roll * 18.0);
                // Mountains behind, rising out of the hills.
                // Rounded foothills, and the peaks behind them.
                let foothills = fbm(xf / 70.0, zf / 60.0, 5, 4) * 100.0 - 25.0;
                let peaks = foothills.max(mountains(xf, zf)) * far;
                let mut h = hills.max(peaks);
                // The river cuts its bed through whatever is there.
                if away <= 0.0 && half > 0.5 {
                    h = (WATER - 2) as f32;
                } else if away < 2.5 {
                    h = h.min(WATER as f32 + 1.0);
                }
                let i = (z * WIDTH + x) as usize;
                // Terraces on the gentler slopes this side of the mountains.
                let slopes = h > WATER as f32 + 3.0
                    && h < 44.0
                    && (36.0..340.0).contains(&zf)
                    && peaks < hills;
                if slopes {
                    h = (h / 3.0).floor() * 3.0;
                    terraced[i] = true;
                }
                raw[i] = h;
            }
        }
        let mut land = Self {
            height: raw.iter().map(|h| h.floor() as i32).collect(),
            top: vec![Stuff::Grass; cells],
            side: vec![Stuff::Earth; cells],
            spans: HashMap::new(),
        };
        for z in 0..DEPTH {
            let (centre, half) = river(z as f32);
            for x in 0..WIDTH {
                let i = (z * WIDTH + x) as usize;
                let h = land.height[i];
                let away = ((x as f32 - centre).abs() - half).max(0.0);
                let (top, side) = if away <= 0.0 && half > 0.5 {
                    (Stuff::River, Stuff::Earth)
                } else if h <= WATER + 1 {
                    (Stuff::Sand, Stuff::Earth)
                } else if terraced[i] {
                    // The outer edge of a terrace, where the land drops to
                    // the next, is its bank; the rest is paddy or rice.
                    let lower = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                        .iter()
                        .any(|(dx, dz)| land.height(x + dx, z + dz) < h);
                    let level = h / 3;
                    let field = hash(level, (x / 40) + (z / 50) * 7, 21) % 5;
                    let top = if lower {
                        Stuff::Bund
                    } else if field < 3 {
                        Stuff::Paddy
                    } else {
                        Stuff::Rice
                    };
                    (top, Stuff::Earth)
                } else if h > 118 + (noise(x as f32 / 9.0, z as f32 / 9.0, 31) * 18.0) as i32 {
                    (Stuff::Snow, Stuff::Rock)
                } else if h > 60 {
                    (Stuff::Rock, Stuff::Rock)
                } else {
                    (Stuff::Grass, Stuff::Earth)
                };
                // Water lies flat at its own level.
                if top == Stuff::River {
                    land.height[i] = WATER;
                }
                land.top[i] = top;
                land.side[i] = side;
            }
        }
        land.plant();
        land
    }

    /// Cherry trees along the banks, evergreens up the hills.
    fn plant(&mut self) {
        for z in (40..340).step_by(3) {
            let (centre, half) = river(z as f32);
            for x in (2..WIDTH - 2).step_by(3) {
                let i = (z * WIDTH + x) as usize;
                let away = (x as f32 - centre).abs() - half;
                let chance = unit(x, z, 41);
                let (kind, size) = if (2.0..9.0).contains(&away) && chance < 0.07 {
                    (Stuff::Blossom, 2.6 + unit(x, z, 42) * 1.2)
                } else if self.top[i] == Stuff::Grass && self.height[i] > 30 && chance < 0.012 {
                    (Stuff::Needles, 1.8 + unit(x, z, 43))
                } else {
                    continue;
                };
                let ground = self.height[i];
                self.tree(x, z, ground, kind, size);
            }
        }
    }

    fn tree(&mut self, x: i32, z: i32, ground: i32, kind: Stuff, size: f32) {
        let trunk = 2 + (size as i32) / 2;
        self.spans
            .entry((x, z))
            .or_default()
            .push((ground, ground + trunk, Stuff::Trunk));
        let r = size.ceil() as i32;
        let middle = ground as f32 + trunk as f32 + size * 0.8;
        for dz in -r..=r {
            for dx in -r..=r {
                let across = ((dx * dx + dz * dz) as f32).sqrt();
                if across > size {
                    continue;
                }
                // A crown: round for blossom, a cone for evergreens.
                let reach = if kind == Stuff::Needles {
                    (size - across) * 2.2
                } else {
                    (size * size - across * across).sqrt() * 0.85
                };
                let bottom = (middle - reach * if kind == Stuff::Needles { 0.3 } else { 1.0 })
                    .round() as i32;
                let top = (middle + reach).round() as i32 + 1;
                if top <= bottom {
                    continue;
                }
                let stuff = match kind {
                    Stuff::Blossom if unit(x + dx, z + dz, 44) < 0.3 => Stuff::Bloom,
                    other => other,
                };
                self.spans
                    .entry((x + dx, z + dz))
                    .or_default()
                    .push((bottom, top, stuff));
            }
        }
    }
}

type Vec3 = [f32; 3];

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn normalise(a: Vec3) -> Vec3 {
    scale(a, 1.0 / dot(a, a).sqrt())
}

/// What a ray met.
#[derive(Clone, Copy, Debug)]
struct Hit {
    t: f32,
    normal: Vec3,
    stuff: Stuff,
    /// The voxel's column, for its own small variation.
    cell: (i32, i32, i32),
}

/// Step `origin + t·dir` through the land's columns until it meets
/// something, or leaves, or `limit` is passed.
fn cast(land: &Land, origin: Vec3, dir: Vec3, limit: f32) -> Option<Hit> {
    let (mut x, mut z) = (origin[0].floor() as i32, origin[2].floor() as i32);
    let step_x = if dir[0] > 0.0 { 1 } else { -1 };
    let step_z = if dir[2] > 0.0 { 1 } else { -1 };
    let delta_x = if dir[0] == 0.0 {
        f32::INFINITY
    } else {
        (1.0 / dir[0]).abs()
    };
    let delta_z = if dir[2] == 0.0 {
        f32::INFINITY
    } else {
        (1.0 / dir[2]).abs()
    };
    let next = |p: f32, cell: i32, step: i32, delta: f32| {
        if delta.is_infinite() {
            f32::INFINITY
        } else if step > 0 {
            ((cell + 1) as f32 - p) * delta
        } else {
            (p - cell as f32) * delta
        }
    };
    let mut t_x = next(origin[0], x, step_x, delta_x);
    let mut t_z = next(origin[2], z, step_z, delta_z);
    let mut t_in = 0.0f32;
    // Which way the ray came into the current column: 0 for x, 2 for z.
    let mut across = 1usize;
    loop {
        let t_out = t_x.min(t_z);
        if t_in > limit {
            return None;
        }
        let y_in = origin[1] + dir[1] * t_in;
        let y_out = origin[1] + dir[1] * t_out;
        if y_in > CEILING && dir[1] >= 0.0 {
            return None;
        }
        if let Some(i) = land.at(x, z) {
            let side_normal = |axis: usize| {
                let mut n = [0.0; 3];
                n[axis] = if axis == 0 {
                    -step_x as f32
                } else {
                    -step_z as f32
                };
                n
            };
            let mut best: Option<Hit> = None;
            // The ground.
            let h = land.height[i] as f32;
            if y_in <= h && across != 1 {
                best = Some(Hit {
                    t: t_in,
                    normal: side_normal(across),
                    stuff: land.side[i],
                    cell: (x, y_in.floor() as i32, z),
                });
            } else if y_out <= h && dir[1] < 0.0 {
                let t = t_in + (h - y_in) / dir[1];
                best = Some(Hit {
                    t,
                    normal: [0.0, 1.0, 0.0],
                    stuff: land.top[i],
                    cell: (x, h as i32, z),
                });
            }
            // Trees over it.
            if let Some(spans) = land.spans.get(&(x, z)) {
                for &(bottom, top, stuff) in spans {
                    let (b, tp) = (bottom as f32, top as f32);
                    let hit = if (b..tp).contains(&y_in) && across != 1 {
                        Some((t_in, side_normal(across), y_in))
                    } else if y_in >= tp && y_out < tp && dir[1] < 0.0 {
                        Some((t_in + (tp - y_in) / dir[1], [0.0, 1.0, 0.0], tp - 0.5))
                    } else if y_in < b && y_out >= b && dir[1] > 0.0 {
                        Some((t_in + (b - y_in) / dir[1], [0.0, -1.0, 0.0], b))
                    } else {
                        None
                    };
                    if let Some((t, normal, y)) = hit
                        && best.is_none_or(|best| t < best.t)
                    {
                        best = Some(Hit {
                            t,
                            normal,
                            stuff,
                            cell: (x, y.floor() as i32, z),
                        });
                    }
                }
            }
            if best.is_some() {
                return best;
            }
        } else {
            // Outside the land, and heading further out: nothing to meet.
            let leaving_x = (x < 0 && step_x < 0) || (x >= WIDTH && step_x > 0);
            let leaving_z = (z < 0 && step_z < 0) || (z >= DEPTH && step_z > 0);
            if leaving_x || leaving_z {
                return None;
            }
        }
        if t_x < t_z {
            x += step_x;
            t_in = t_x;
            t_x += delta_x;
            across = 0;
        } else {
            z += step_z;
            t_in = t_z;
            t_z += delta_z;
            across = 2;
        }
    }
}

/// Colours as `0..1` floats.
type Rgb = [f32; 3];

fn rgb(c: [u8; 3]) -> Rgb {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
    ]
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

/// How near the moon's direction a ray must be to meet it: about 1.4°.
const MOON_EDGE: f32 = 0.99970;

/// The scene's lighting and colours, from a palette.
struct Look {
    base: Rgb,
    horizon: Rgb,
    glow: Rgb,
    moon: Rgb,
    moon_dir: Vec3,
    light: Vec3,
    palette: Palette,
}

impl Look {
    fn new(palette: &Palette) -> Self {
        let base = rgb(palette.base);
        Self {
            base,
            // Dusk: the last of the light low over the mountains.
            horizon: mix(base, mix(rgb(palette.iris), rgb(palette.rose), 0.45), 0.42),
            glow: mix(base, rgb(palette.rose), 0.55),
            moon: rgb(palette.text),
            moon_dir: normalise([0.36, 0.115, 1.0]),
            light: normalise([0.55, 0.75, -0.45]),
            palette: *palette,
        }
    }

    fn colour(&self, stuff: Stuff) -> Rgb {
        let p = &self.palette;
        match stuff {
            Stuff::River => rgb(p.pine),
            Stuff::Paddy => mix(rgb(p.iris), rgb(p.foam), 0.4),
            Stuff::Rice => rgb(p.gold),
            Stuff::Bund => mix(rgb(p.foam), rgb(p.pine), 0.35),
            Stuff::Sand => mix(rgb(p.subtle), rgb(p.gold), 0.25),
            Stuff::Earth => mix(rgb(p.overlay), rgb(p.muted), 0.45),
            Stuff::Grass => mix(rgb(p.pine), rgb(p.foam), 0.55),
            Stuff::Rock => mix(rgb(p.overlay), rgb(p.muted), 0.55),
            Stuff::Snow => rgb(p.text),
            Stuff::Blossom => rgb(p.rose),
            Stuff::Bloom => mix(rgb(p.rose), rgb(p.love), 0.55),
            Stuff::Needles => mix(rgb(p.pine), rgb(p.base), 0.35),
            Stuff::Trunk => mix(rgb(p.muted), rgb(p.base), 0.3),
        }
    }

    /// The sky in direction `dir`. With `disc`, the moon itself is in it
    /// where that direction meets it (water shows only its light).
    fn sky(&self, dir: Vec3, disc: Option<Vec3>) -> Rgb {
        let up = dir[1].max(0.0);
        let mut sky = mix(self.horizon, self.base, smooth(0.0, 0.2, up).powf(0.8));
        // Warmer where the light went down, low and to the right.
        let towards = dot(
            normalise([dir[0], 0.0, dir[2]]),
            normalise([0.35, 0.0, 1.0]),
        )
        .max(0.0);
        let warm = towards.powi(6) * (1.0 - smooth(0.0, 0.18, up));
        sky = mix(sky, self.glow, warm * 0.5);
        // The moon, and the light round it.
        let at = dot(dir, self.moon_dir);
        let halo = smooth(0.985, MOON_EDGE, at);
        sky = mix(sky, mix(self.base, self.moon, 0.25), halo * 0.35);
        if let Some(disc) = disc
            && dot(disc, self.moon_dir) > MOON_EDGE
        {
            // Faintly marked, as the moon is.
            let mark = noise(disc[0] * 900.0, disc[1] * 900.0, 91);
            sky = mix(sky, self.moon, 0.86 - mark * 0.14);
        }
        sky
    }
}

/// Where the camera is and how it looks: the ray through any point of
/// the picture.
struct Camera {
    eye: Vec3,
    ahead: Vec3,
    right: Vec3,
    up: Vec3,
    span: f32,
    size: (f32, f32),
}

impl Camera {
    fn new(width: u32, height: u32) -> Self {
        let ahead = normalise([0.0, -0.085, 1.0]);
        let right = normalise([ahead[2], 0.0, -ahead[0]]);
        // ahead × right: up on the screen.
        let up = [
            ahead[1] * right[2] - ahead[2] * right[1],
            ahead[2] * right[0] - ahead[0] * right[2],
            ahead[0] * right[1] - ahead[1] * right[0],
        ];
        Self {
            eye: [WIDTH as f32 * 0.47, 74.0, -70.0],
            ahead,
            right,
            up,
            // A horizontal field of view of 55°.
            span: (55.0f32.to_radians() / 2.0).tan(),
            size: (width as f32, height as f32),
        }
    }

    /// The ray through the point (`x`, `y`) of the picture, in pixels.
    fn ray(&self, x: f32, y: f32) -> Vec3 {
        let (w, h) = self.size;
        let sx = (2.0 * x / w - 1.0) * self.span;
        let sy = (1.0 - 2.0 * y / h) * self.span * h / w;
        normalise(add(
            self.ahead,
            add(scale(self.right, sx), scale(self.up, sy)),
        ))
    }
}

/// Pixels across a block of the moon: it is made of voxels too.
const MOON_BLOCK: u32 = 6;
/// Neighbours this different get more rays: edges, where one ray a pixel
/// would leave stairs and shimmer.
const EDGE: f32 = 0.05;
/// Where the extra rays go within a pixel: a rotated grid.
const SUBPIXELS: [(f32, f32); 4] = [
    (0.375, 0.125),
    (0.875, 0.375),
    (0.625, 0.875),
    (0.125, 0.625),
];

/// Run `each(row, y)` over the rows of `out` (`width` items each), the
/// rows shared out among the processors.
fn rows<T: Send>(out: &mut [T], width: usize, each: impl Fn(&mut [T], u32) + Sync) {
    let height = out.len() / width.max(1);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let rows_each = height.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (chunk, part) in out.chunks_mut(rows_each * width).enumerate() {
            let each = &each;
            scope.spawn(move || {
                for (r, row) in part.chunks_mut(width).enumerate() {
                    each(row, (chunk * rows_each + r) as u32);
                }
            });
        }
    });
}

/// What a pixel shows, as far as motion cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Sky,
    Star,
    Water,
    Blossom,
    /// The valley at the mountains' feet, where mist lies.
    Far,
    Land,
}

/// Render the scene, `width × height`, as RGBA bytes (opaque).
pub fn render(width: u32, height: u32, palette: &Palette) -> Vec<u8> {
    render_with_life(width, height, palette).0
}

/// Render the scene, and note what in it can move.
pub fn render_with_life(width: u32, height: u32, palette: &Palette) -> (Vec<u8>, Life) {
    let land = Land::new();
    let look = Look::new(palette);
    let camera = Camera::new(width, height);
    let sample = |x: f32, y: f32, px: u32, py: u32| {
        // The moon is drawn a block at a time, like everything else.
        let block = |p: u32| ((p / MOON_BLOCK) * MOON_BLOCK) as f32 + MOON_BLOCK as f32 / 2.0;
        let moon = camera.ray(block(px), block(py));
        shade(&land, &look, camera.eye, camera.ray(x, y), moon, (px, py))
    };

    // One ray through the middle of each pixel.
    let w = width as usize;
    let mut first = vec![([0.0f32; 3], Kind::Sky); w * height as usize];
    rows(&mut first, w, |row, py| {
        for (px, seen) in row.iter_mut().enumerate() {
            *seen = sample(px as f32 + 0.5, py as f32 + 0.5, px as u32, py);
        }
    });
    let life = life_of(&first, width, height);

    // More where the picture changes sharply.
    let at = |x: i64, y: i64| {
        let (x, y) = (x.clamp(0, w as i64 - 1), y.clamp(0, height as i64 - 1));
        first[y as usize * w + x as usize].0
    };
    let mut out = vec![0u8; w * height as usize * 4];
    rows(&mut out, w * 4, |row, py| {
        for (px, pixel) in row.chunks_mut(4).enumerate() {
            let here = at(px as i64, py as i64);
            let differs = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                let there = at(px as i64 + dx, py as i64 + dy);
                (0..3).any(|c| (here[c] - there[c]).abs() > EDGE)
            });
            let colour = if differs {
                let mut sum = here;
                for (sx, sy) in SUBPIXELS {
                    let (more, _) = sample(px as f32 + sx, py as f32 + sy, px as u32, py);
                    for c in 0..3 {
                        sum[c] += more[c];
                    }
                }
                sum.map(|c| c / (SUBPIXELS.len() + 1) as f32)
            } else {
                here
            };
            for (byte, c) in pixel.iter_mut().zip(colour) {
                *byte = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            pixel[3] = 255;
        }
    });
    (out, life)
}

/// What can move, from what each pixel shows.
fn life_of(seen: &[(Rgb, Kind)], width: u32, height: u32) -> Life {
    let (w, h) = (width as usize, height as usize);
    let kind = |x: usize, y: usize| seen[y * w + x].1;
    let mut life = Life {
        width: width.min(u32::from(u16::MAX)) as u16,
        height: height.min(u32::from(u16::MAX)) as u16,
        ..Life::default()
    };
    // How much far valley floor each row shows.
    let mut far_rows = Vec::with_capacity(h);
    for y in 0..h {
        let mut run: Option<usize> = None;
        let mut far = 0;
        for x in 0..=w {
            let here = (x < w).then(|| kind(x, y));
            match (here, run) {
                (Some(Kind::Water), None) => run = Some(x),
                (Some(Kind::Water), Some(_)) => {}
                (_, Some(start)) => {
                    life.water.push(Run {
                        y: y as u16,
                        x0: start as u16,
                        x1: x as u16,
                    });
                    run = None;
                }
                _ => {}
            }
            match here {
                Some(Kind::Star) => life.stars.push(Star {
                    x: x as u16,
                    y: y as u16,
                    phase: (unit(x as i32, y as i32, 78) * 255.0) as u8,
                }),
                // The top of a crown, a few of them: where petals let go.
                Some(Kind::Blossom)
                    if y > 0 && kind(x, y - 1) != Kind::Blossom && (x * 7 + y * 13) % 11 == 0 =>
                {
                    life.blossom.push((x as u16, y as u16));
                }
                Some(Kind::Far) => far += 1,
                _ => {}
            }
        }
        far_rows.push(far);
    }
    // The mist lies along the row with most of it, a band either side,
    // if there is enough of it to lie on.
    if let Some((row, &most)) = far_rows.iter().enumerate().max_by_key(|(_, far)| **far)
        && most * 10 > w
    {
        let half = (h * MIST_BAND / 1080 / 2).max(1);
        life.mist = (row.saturating_sub(half) as u16, (row + half).min(h) as u16);
    }
    life
}

fn shade(
    land: &Land,
    look: &Look,
    eye: Vec3,
    dir: Vec3,
    moon: Vec3,
    pixel: (u32, u32),
) -> (Rgb, Kind) {
    let sky = look.sky(dir, Some(moon));
    let Some(hit) = cast(land, eye, dir, 1400.0) else {
        // A few stars, high up.
        let star = unit(pixel.0 as i32, pixel.1 as i32, 77);
        if dir[1] > 0.12 && star > 0.9993 {
            let faint = smooth(0.12, 0.4, dir[1]) * 0.45;
            return (mix(sky, look.moon, faint), Kind::Star);
        }
        return (sky, Kind::Sky);
    };
    let kind = match hit.stuff {
        Stuff::River | Stuff::Paddy => Kind::Water,
        Stuff::Blossom | Stuff::Bloom => Kind::Blossom,
        // The valley floor out there, not the mountains rising from it.
        _ if (MIST_FROM..MIST_TO).contains(&hit.t) && hit.cell.1 < MIST_BELOW => Kind::Far,
        _ => Kind::Land,
    };
    let point = add(eye, scale(dir, hit.t));
    let mut colour = look.colour(hit.stuff);
    // A little difference between neighbouring voxels.
    let (cx, cy, cz) = hit.cell;
    let jitter = (unit(cx * 31 + cy, cz, 3) - 0.5) * 0.07;
    colour = colour.map(|c| c * (1.0 + jitter));

    let water = matches!(hit.stuff, Stuff::River | Stuff::Paddy);
    if water {
        // Water shows the sky, darkened, over its own colour.
        let reflected = look.sky([dir[0], -dir[1], dir[2]], None);
        colour = mix(colour, reflected, 0.55);
    }
    // Moonlight, and the moon's shadow.
    let facing = dot(hit.normal, look.light).max(0.0);
    let lit = if facing > 0.0 {
        let from = add(point, scale(hit.normal, 0.02));
        if cast(land, from, look.light, 260.0).is_some() {
            0.0
        } else {
            facing
        }
    } else {
        0.0
    };
    let mut light = 0.42 + 0.58 * lit;
    // Faces turned up are lit by the sky as well.
    if hit.normal[1] > 0.0 {
        light += 0.08;
    }
    // Voxel edges, faintly: the grid the land is made of.
    if !water {
        let (u, v) = if hit.normal[1] != 0.0 {
            (point[0], point[2])
        } else if hit.normal[0] != 0.0 {
            (point[2], point[1])
        } else {
            (point[0], point[1])
        };
        let edge = |a: f32| {
            let f = a - a.floor();
            f.min(1.0 - f)
        };
        // Near only: far off, the lines would be finer than a pixel.
        if hit.t < 220.0 && edge(u).min(edge(v)) < 0.045 {
            light *= 1.0 - 0.1 * (1.0 - smooth(120.0, 220.0, hit.t));
        }
    }
    colour = mix(look.base, colour, light.min(1.1));
    // Distance: into the dusk.
    let fog = 1.0 - (-hit.t / 620.0).exp();
    let haze = look.sky([dir[0], 0.0, dir[2]], None);
    (mix(colour, haze, fog * 0.85), kind)
}

/// How far off the valley floor lies where the mist gathers.
const MIST_FROM: f32 = 380.0;
const MIST_TO: f32 = 640.0;
/// The mist's depth at 1080 rows; other sizes in proportion.
const MIST_BAND: usize = 84;
/// And how high the land may be there: below the foothills.
const MIST_BELOW: i32 = 34;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Rosé Pine Moon.
    pub(crate) const MOON: Palette = Palette {
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
    fn the_same_picture_every_time() {
        let a = render(160, 90, &MOON);
        let b = render(160, 90, &MOON);
        assert_eq!(a.len(), 160 * 90 * 4);
        assert!(a == b);
        assert!(a.chunks(4).all(|px| px[3] == 255));
        save("scene-small", 160, 90, &a);
    }

    #[test]
    fn the_river_runs_between_the_hills() {
        let land = Land::new();
        for z in [0, 100, 200, 300] {
            let (centre, _) = river(z as f32);
            let x = centre.round() as i32;
            let i = land.at(x, z).unwrap();
            assert_eq!(land.top[i], Stuff::River, "at {z}");
            assert!(land.height(x + 80, z) > WATER || land.height(x - 80, z) > WATER);
        }
        assert!(land.top.contains(&Stuff::Paddy));
        assert!(land.top.contains(&Stuff::Rice));
        assert!(land.top.contains(&Stuff::Snow));
        assert!(!land.spans.is_empty());
    }

    /// The full picture, to look at: `cargo test -p enso-scene -- --ignored`.
    #[test]
    #[ignore = "a second or two of rays; run to look at the picture"]
    fn full_size() {
        let (rgba, life) = render_with_life(1920, 1080, &MOON);
        save("scene", 1920, 1080, &rgba);
        println!(
            "water runs {}, stars {}, blossom points {}, mist rows {:?}",
            life.water.len(),
            life.stars.len(),
            life.blossom.len(),
            life.mist
        );
        // A few moments of the motion, to look at.
        let ink = crate::life::Ink {
            glint: MOON.text,
            mist: MOON.subtle,
            petals: [MOON.rose, MOON.love],
        };
        let mut out = rgba.clone();
        let all = crate::life::Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        for (n, t) in [2.0f32, 5.0, 8.0].into_iter().enumerate() {
            life.paint(&rgba, &mut out, crate::life::Order::Rgba, all, t, 1.0, &ink);
            save(&format!("scene-life-{n}"), 1920, 1080, &out);
        }
        // Close up, by the trees on the river.
        let crop: Vec<u8> = (560..920)
            .flat_map(|y| out[(y * 1920 + 700) * 4..(y * 1920 + 1340) * 4].to_vec())
            .collect();
        save("scene-life-crop", 640, 360, &crop);
        // A part at its real size, to judge the detail.
        let crop: Vec<u8> = (300..660)
            .flat_map(|y| rgba[(y * 1920 + 200) * 4..(y * 1920 + 840) * 4].to_vec())
            .collect();
        save("scene-crop", 640, 360, &crop);
    }
}
