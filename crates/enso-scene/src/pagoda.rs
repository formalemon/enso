//! The boot menu's picture: a five-storeyed pagoda on an island floating in
//! the sky at dusk, in voxels, with the camera going slowly round it.
//!
//! [`Model::build`] works the island out on a grid: rock tapering to a
//! point below, grass on top, the pagoda in the middle on its platform, a
//! torii where the path meets the edge, stone lanterns, cherry trees and
//! pines, and a few clouds about it. Then it keeps only what can be seen:
//! each voxel face with air in front of it, already lit — the moon's light
//! and its shadows, shade in the corners, the haze the rock fades into
//! below — since the light stays where it is while the camera goes round.
//! [`crate::orbit`] draws those faces a frame at a time, and needs no
//! operating system: the boot menu runs it in the firmware.

use alloc::vec::Vec;

use libm::{atan2f, cosf, floorf, powf, sinf, sqrtf};

use crate::{
    Palette,
    noise::{fbm, hash, noise, smooth, unit},
};

/// The grid the island is worked out on: across (x), up (y), across (z).
pub const SIZE: [i32; 3] = [128, 136, 128];
/// The middle of the island, across.
pub const CENTRE: [i32; 2] = [64, 64];
/// The height of its grass.
pub const GROUND: i32 = 56;

/// What a voxel is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Stuff {
    Air = 0,
    Grass,
    Earth,
    Rock,
    DarkRock,
    /// Dressed stone: the platform, the lanterns, the steps.
    Stone,
    /// The path's flags.
    Flag,
    /// Dark wood: sills, and what is inside.
    Wood,
    /// Lacquered red: posts and beams.
    Lacquer,
    Plaster,
    /// The brackets under each roof.
    Bracket,
    Roof,
    /// A roof's edge.
    Eave,
    /// The spire's rings, and the bells at the eaves' corners.
    Gilt,
    /// Lit from inside: windows and doors.
    Window,
    /// A lantern's light.
    Light,
    Blossom,
    Bloom,
    Needles,
    Trunk,
    Moss,
    Cloud,
}

const ALL: [Stuff; 22] = [
    Stuff::Air,
    Stuff::Grass,
    Stuff::Earth,
    Stuff::Rock,
    Stuff::DarkRock,
    Stuff::Stone,
    Stuff::Flag,
    Stuff::Wood,
    Stuff::Lacquer,
    Stuff::Plaster,
    Stuff::Bracket,
    Stuff::Roof,
    Stuff::Eave,
    Stuff::Gilt,
    Stuff::Window,
    Stuff::Light,
    Stuff::Blossom,
    Stuff::Bloom,
    Stuff::Needles,
    Stuff::Trunk,
    Stuff::Moss,
    Stuff::Cloud,
];

impl Stuff {
    /// What gives off its own light, and is not shaded.
    fn glows(self) -> bool {
        matches!(self, Self::Window | Self::Light)
    }
}

/// The way each face looks: east (+x), west, up (+y), down, south (+z),
/// north.
pub const NORMALS: [[i32; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

/// One face of a voxel, with air in front of it: the voxel, the way the
/// face looks (an index into [`NORMALS`]), and its colour, lit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    pub at: [u8; 3],
    pub facing: u8,
    pub rgb: [u8; 3],
}

impl Face {
    /// Its four corners, in grid units, round its edge.
    pub fn corners(&self) -> [[f32; 3]; 4] {
        let [x, y, z] = self.at.map(f32::from);
        match self.facing {
            0 => [
                [x + 1.0, y, z],
                [x + 1.0, y + 1.0, z],
                [x + 1.0, y + 1.0, z + 1.0],
                [x + 1.0, y, z + 1.0],
            ],
            1 => [
                [x, y, z],
                [x, y, z + 1.0],
                [x, y + 1.0, z + 1.0],
                [x, y + 1.0, z],
            ],
            2 => [
                [x, y + 1.0, z],
                [x, y + 1.0, z + 1.0],
                [x + 1.0, y + 1.0, z + 1.0],
                [x + 1.0, y + 1.0, z],
            ],
            3 => [
                [x, y, z],
                [x + 1.0, y, z],
                [x + 1.0, y, z + 1.0],
                [x, y, z + 1.0],
            ],
            4 => [
                [x, y, z + 1.0],
                [x + 1.0, y, z + 1.0],
                [x + 1.0, y + 1.0, z + 1.0],
                [x, y + 1.0, z + 1.0],
            ],
            _ => [
                [x, y, z],
                [x, y + 1.0, z],
                [x + 1.0, y + 1.0, z],
                [x + 1.0, y, z],
            ],
        }
    }

    /// Its middle, in grid units.
    pub fn centre(&self) -> [f32; 3] {
        let n = NORMALS[usize::from(self.facing.min(5))];
        let [x, y, z] = self.at.map(f32::from);
        [
            x + 0.5 + n[0] as f32 * 0.5,
            y + 0.5 + n[1] as f32 * 0.5,
            z + 0.5 + n[2] as f32 * 0.5,
        ]
    }
}

/// The colours drawn with the island: the moon, the stars, and the haze
/// far things fade into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub moon: [u8; 3],
    pub star: [u8; 3],
    pub haze: [u8; 3],
}

/// The island as it is drawn: its faces, lit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub faces: Vec<Face>,
    pub look: Look,
}

/// The start of a kept model.
const MAGIC: &[u8; 4] = b"PGD1";

impl Model {
    /// Work the island out in `palette`'s colours.
    pub fn build(palette: &Palette) -> Self {
        let mut grid = Grid::new();
        grid.island();
        grid.platform_and_path();
        grid.pagoda();
        grid.torii();
        for side in [-1, 1] {
            grid.lantern(CENTRE[0] + 3 * side, CENTRE[1] + 18);
        }
        for (x, z, size) in [
            (-19, 11, 5.0),
            (18, 15, 4.2),
            (15, -21, 4.8),
            (-25, -7, 3.6),
            (27, -4, 3.4),
        ] {
            grid.cherry(CENTRE[0] + x, CENTRE[1] + z, size);
        }
        for (x, z, size) in [(-15, -20, 5), (23, 6, 4), (-6, -27, 4), (-28, 5, 3)] {
            grid.pine(CENTRE[0] + x, CENTRE[1] + z, size);
        }
        grid.clouds();
        let look = Shade::new(palette);
        Self {
            faces: grid.faces(&look),
            look: Look {
                moon: palette.text,
                star: palette.text,
                haze: look.haze,
            },
        }
    }

    /// The model as bytes, to keep in a program.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.faces.len() * 7);
        out.extend_from_slice(MAGIC);
        for rgb in [self.look.moon, self.look.star, self.look.haze] {
            out.extend_from_slice(&rgb);
        }
        out.extend_from_slice(&(self.faces.len() as u32).to_le_bytes());
        for face in &self.faces {
            out.extend_from_slice(&face.at);
            out.push(face.facing);
            out.extend_from_slice(&face.rgb);
        }
        out
    }

    /// The other half of [`Model::encode`].
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (head, rest) = bytes.split_at_checked(4 + 9 + 4)?;
        (&head[..4] == MAGIC).then_some(())?;
        let rgb = |at: usize| -> [u8; 3] { [head[at], head[at + 1], head[at + 2]] };
        let count = u32::from_le_bytes(head[13..17].try_into().ok()?) as usize;
        (rest.len() == count.checked_mul(7)?).then_some(())?;
        let faces = rest
            .as_chunks::<7>()
            .0
            .iter()
            .map(|b| Face {
                at: [b[0], b[1], b[2]],
                facing: b[3].min(5),
                rgb: [b[4], b[5], b[6]],
            })
            .collect();
        Some(Self {
            faces,
            look: Look {
                moon: rgb(4),
                star: rgb(7),
                haze: rgb(10),
            },
        })
    }
}

/// The voxels.
struct Grid {
    cells: Vec<u8>,
}

impl Grid {
    fn new() -> Self {
        Self {
            cells: alloc::vec![0; (SIZE[0] * SIZE[1] * SIZE[2]) as usize],
        }
    }

    fn index(x: i32, y: i32, z: i32) -> Option<usize> {
        ((0..SIZE[0]).contains(&x) && (0..SIZE[1]).contains(&y) && (0..SIZE[2]).contains(&z))
            .then(|| ((y * SIZE[2] + z) * SIZE[0] + x) as usize)
    }

    fn get(&self, x: i32, y: i32, z: i32) -> Stuff {
        Self::index(x, y, z).map_or(Stuff::Air, |i| ALL[usize::from(self.cells[i])])
    }

    fn solid(&self, x: i32, y: i32, z: i32) -> bool {
        self.get(x, y, z) != Stuff::Air
    }

    fn set(&mut self, x: i32, y: i32, z: i32, stuff: Stuff) {
        if let Some(i) = Self::index(x, y, z) {
            self.cells[i] = stuff as u8;
        }
    }

    /// The highest solid voxel in a column, below `under`.
    fn top(&self, x: i32, z: i32, under: i32) -> Option<i32> {
        (0..under.min(SIZE[1])).rev().find(|&y| self.solid(x, y, z))
    }

    /// The rock and grass: a rough disc, its underside tapering to a point.
    fn island(&mut self) {
        let [cx, cz] = CENTRE.map(|c| c as f32);
        for z in 0..SIZE[2] {
            for x in 0..SIZE[0] {
                let (dx, dz) = (x as f32 + 0.5 - cx, z as f32 + 0.5 - cz);
                let r = sqrtf(dx * dx + dz * dz);
                let angle = atan2f(dz, dx);
                // The edge wanders in and out.
                let wander = fbm(cosf(angle) * 1.7 + 5.0, sinf(angle) * 1.7 + 5.0, 3, 3);
                let rim = 36.0 + 14.0 * (wander - 0.5);
                if r > rim {
                    continue;
                }
                let d = r / rim;
                let roll = fbm(x as f32 / 14.0, z as f32 / 14.0, 4, 2);
                let top = GROUND + floorf((roll - 0.5) * 3.0 - 2.5 * smooth(0.82, 1.0, d)) as i32;
                let rough = fbm(x as f32 / 7.0, z as f32 / 7.0, 5, 3);
                let depth = 3.0 + 27.0 * powf(1.0 - d, 1.1) * (0.6 + 0.8 * rough);
                let bottom = top - depth as i32;
                for y in bottom.max(1)..=top {
                    let stuff = if y == top {
                        Stuff::Grass
                    } else if y >= top - 2 {
                        Stuff::Earth
                    } else {
                        // The rock in strata, wavering.
                        let band = y as f32 + 3.0 * noise(x as f32 / 11.0, z as f32 / 11.0, 6);
                        if (band as i32).rem_euclid(7) < 2 {
                            Stuff::DarkRock
                        } else {
                            Stuff::Rock
                        }
                    };
                    self.set(x, y, z, stuff);
                }
            }
        }
        // A few spires of rock hanging lower than the rest.
        for i in 0..7 {
            let angle = i as f32 * 0.9 + unit(i, 0, 51) * 0.7;
            let reach = 4.0 + unit(i, 1, 51) * 20.0;
            let x = CENTRE[0] + (cosf(angle) * reach) as i32;
            let z = CENTRE[1] + (sinf(angle) * reach) as i32;
            let length = 5 + (unit(i, 2, 51) * 9.0) as i32;
            let Some(bottom) = (1..GROUND).find(|&y| self.solid(x, y, z)) else {
                continue;
            };
            for k in 0..length {
                let radius = 2.4 * (1.0 - k as f32 / length as f32);
                for dz in -3..=3i32 {
                    for dx in -3..=3i32 {
                        if ((dx * dx + dz * dz) as f32) <= radius * radius + 0.3 {
                            let stuff = if (bottom - k) % 5 < 2 {
                                Stuff::DarkRock
                            } else {
                                Stuff::Rock
                            };
                            self.set(x + dx, bottom - 1 - k, z + dz, stuff);
                        }
                    }
                }
            }
        }
    }

    /// The stone platform the pagoda stands on, its steps, and the path
    /// from them to the edge.
    fn platform_and_path(&mut self) {
        let [cx, cz] = CENTRE;
        let base = GROUND + 1;
        for z in cz - 11..=cz + 11 {
            for x in cx - 11..=cx + 11 {
                // Filled down to the rock, wherever the grass dips.
                let ground = self.top(x, z, base).unwrap_or(GROUND);
                for y in ground + 1..base + 2 {
                    self.set(x, y, z, Stuff::Stone);
                }
            }
        }
        // Steps down on the south side.
        for (z, y) in [(cz + 12, base + 1), (cz + 13, base)] {
            for x in cx - 2..=cx + 2 {
                for fill in GROUND - 1..=y {
                    self.set(x, fill, z, Stuff::Stone);
                }
            }
        }
        // The path: flags in the grass, out to the edge.
        for z in cz + 14..SIZE[2] {
            for x in cx - 1..=cx + 1 {
                if let Some(y) = self.top(x, z, GROUND + 3)
                    && self.get(x, y, z) == Stuff::Grass
                {
                    let flag = if hash(x, z, 61).is_multiple_of(5) {
                        Stuff::Moss
                    } else {
                        Stuff::Flag
                    };
                    self.set(x, y, z, flag);
                }
            }
        }
        // Boulders, mossy on top.
        for (x, z) in [(-9, 22), (24, 12), (-27, 9), (8, -26), (-20, -26)] {
            let (x, z) = (cx + x, cz + z);
            let Some(ground) = self.top(x, z, GROUND + 3) else {
                continue;
            };
            for dz in 0..2 {
                for dx in 0..2 {
                    let height = 1 + (hash(x + dx, z + dz, 62) % 2) as i32;
                    for dy in 1..=height {
                        let stuff = if dy == height {
                            Stuff::Moss
                        } else {
                            Stuff::Rock
                        };
                        self.set(x + dx, ground + dy, z + dz, stuff);
                    }
                }
            }
        }
    }

    /// Five storeys, each a little narrower, with roofs reaching out and
    /// turning up at the corners, and the gilt spire on top.
    fn pagoda(&mut self) {
        let [cx, cz] = CENTRE;
        let mut y = GROUND + 3;
        let storeys: [(i32, i32); 5] = [(8, 5), (7, 4), (6, 4), (5, 4), (4, 3)];
        let mut last_reach = 0;
        for (storey, &(half, tall)) in storeys.iter().enumerate() {
            // The walls: posts at the corners and between, plaster, a sill,
            // and light behind the middle of each side.
            for dy in 0..tall {
                for dz in -half..=half {
                    for dx in -half..=half {
                        let edge = dx.abs() == half || dz.abs() == half;
                        let stuff = if !edge {
                            Stuff::Wood
                        } else {
                            let along = if dx.abs() == half { dz } else { dx };
                            let corner = dx.abs() == half && dz.abs() == half;
                            let lit = (1..tall - 1).contains(&dy) && along.abs() <= 1;
                            if corner || along.abs() == half - 2 {
                                Stuff::Lacquer
                            } else if dy == 0 {
                                Stuff::Wood
                            } else if lit {
                                Stuff::Window
                            } else if dy == tall - 1 {
                                Stuff::Lacquer
                            } else {
                                Stuff::Plaster
                            }
                        };
                        self.set(cx + dx, y + dy, cz + dz, stuff);
                    }
                }
            }
            y += tall;
            // The brackets, a band a little wider.
            self.square(cx, cz, y, half + 1, Stuff::Bracket, None);
            y += 1;
            // The roof: an edge reaching out, two courses stepping in.
            let reach = half + 5;
            self.square(cx, cz, y, reach, Stuff::Roof, Some(Stuff::Eave));
            self.square(cx, cz, y + 1, reach - 2, Stuff::Roof, None);
            // The corners turn up, and a bell hangs under each.
            for (sx, sz) in [(-1, -1), (-1, 1), (1, -1), (1, 1)] {
                let (x, z) = (cx + sx * reach, cz + sz * reach);
                self.set(x, y + 1, z, Stuff::Eave);
                self.set(x - sx, y + 1, z, Stuff::Eave);
                self.set(x, y + 1, z - sz, Stuff::Eave);
                self.set(x + sx, y + 2, z + sz, Stuff::Eave);
                self.set(x, y + 2, z, Stuff::Eave);
                self.set(x, y - 1, z, Stuff::Gilt);
            }
            if storey + 1 < storeys.len() {
                self.square(cx, cz, y + 2, reach - 4, Stuff::Roof, None);
                y += 2;
            } else {
                // The last roof rises to a point.
                let mut step = reach - 4;
                y += 2;
                while step >= 1 {
                    self.square(cx, cz, y, step, Stuff::Roof, None);
                    y += 1;
                    step -= 2;
                }
            }
            last_reach = reach;
        }
        let _ = last_reach;
        // The spire: a gilt mast with its rings, and the jewel on top.
        for k in 0..13 {
            self.set(cx, y + k, cz, Stuff::Gilt);
        }
        for ring in 0..5 {
            self.square(cx, cz, y + 2 + ring * 2, 1, Stuff::Gilt, None);
        }
        self.set(cx, y + 13, cz, Stuff::Light);
    }

    /// A square slab, `reach` from the middle each way, its rim of `edge`.
    fn square(&mut self, cx: i32, cz: i32, y: i32, reach: i32, stuff: Stuff, edge: Option<Stuff>) {
        for dz in -reach..=reach {
            for dx in -reach..=reach {
                let rim = dx.abs() == reach || dz.abs() == reach;
                let what = match edge {
                    Some(edge) if rim => edge,
                    _ => stuff,
                };
                self.set(cx + dx, y, cz + dz, what);
            }
        }
    }

    /// The gate where the path comes to the edge.
    fn torii(&mut self) {
        let (cx, z) = (CENTRE[0], CENTRE[1] + 27);
        let ground = self.top(cx, z, GROUND + 3).unwrap_or(GROUND);
        let foot = |grid: &Self, x: i32| grid.top(x, z, GROUND + 3).unwrap_or(ground);
        for x in [cx - 4, cx + 4] {
            for y in foot(self, x) + 1..=ground + 9 {
                self.set(x, y, z, Stuff::Lacquer);
            }
        }
        for x in cx - 5..=cx + 5 {
            self.set(x, ground + 6, z, Stuff::Lacquer);
            self.set(x, ground + 9, z, Stuff::Lacquer);
        }
        self.set(cx, ground + 7, z, Stuff::Lacquer);
        self.set(cx, ground + 8, z, Stuff::Lacquer);
        // The top beam, dark, its ends lifting.
        for x in cx - 6..=cx + 6 {
            self.set(x, ground + 10, z, Stuff::Bracket);
        }
        for x in [cx - 7, cx + 7] {
            self.set(x, ground + 11, z, Stuff::Bracket);
        }
    }

    /// A stone lantern, lit.
    fn lantern(&mut self, x: i32, z: i32) {
        let ground = self.top(x, z, GROUND + 3).unwrap_or(GROUND);
        for y in ground + 1..=ground + 3 {
            self.set(x, y, z, Stuff::Stone);
        }
        let y = ground + 4;
        for dz in -1..=1i32 {
            for dx in -1..=1i32 {
                let stuff = if dx != 0 && dz != 0 {
                    Stuff::Stone
                } else {
                    Stuff::Light
                };
                self.set(x + dx, y, z + dz, stuff);
            }
        }
        self.square(x, z, y + 1, 1, Stuff::Stone, None);
        self.set(x, y + 2, z, Stuff::Stone);
    }

    /// A cherry tree in flower.
    fn cherry(&mut self, x: i32, z: i32, size: f32) {
        let Some(ground) = self.top(x, z, GROUND + 3) else {
            return;
        };
        let trunk = 5;
        for k in 1..=trunk {
            // Leaning a little as it rises.
            let lean = i32::from(k > 3);
            self.set(x + lean, ground + k, z, Stuff::Trunk);
        }
        let (mx, my, mz) = (
            x as f32 + 1.5,
            (ground + trunk) as f32 + size * 0.6,
            z as f32 + 0.5,
        );
        let r = size as i32 + 1;
        for dy in -r..=r {
            for dz in -r..=r {
                for dx in -r..=r {
                    let (px, py, pz) = (
                        x + 1 + dx,
                        ground + trunk + (size * 0.6) as i32 + dy,
                        z + dz,
                    );
                    let (fx, fy, fz) = (
                        px as f32 + 0.5 - mx,
                        (py as f32 + 0.5 - my) * 1.35,
                        pz as f32 + 0.5 - mz,
                    );
                    let d = sqrtf(fx * fx + fy * fy + fz * fz);
                    let ragged = size * (0.78 + 0.32 * unit(px * 7 + py, pz, 44));
                    if d <= ragged {
                        let stuff = if unit(px, py * 13 + pz, 45) < 0.32 {
                            Stuff::Bloom
                        } else {
                            Stuff::Blossom
                        };
                        self.set(px, py, pz, stuff);
                    }
                }
            }
        }
    }

    /// An evergreen: tiers narrowing to a point.
    fn pine(&mut self, x: i32, z: i32, size: i32) {
        let Some(ground) = self.top(x, z, GROUND + 3) else {
            return;
        };
        for k in 1..=3 {
            self.set(x, ground + k, z, Stuff::Trunk);
        }
        let mut y = ground + 3;
        let mut reach = size;
        while reach >= 0 {
            for tier in 0..2 {
                let r = reach - tier;
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dz * dz <= r * r + 1 {
                            self.set(x + dx, y, z + dz, Stuff::Needles);
                        }
                    }
                }
                y += 1;
            }
            reach -= 2;
        }
        self.set(x, y, z, Stuff::Needles);
    }

    /// Clouds about the island: heaps of rounded lumps, flat underneath.
    fn clouds(&mut self) {
        let [cx, cz] = CENTRE;
        let heaps: [(i32, i32, i32, f32); 6] = [
            (cx - 46, GROUND - 16, cz + 30, 1.0),
            (cx + 47, GROUND - 6, cz - 26, 0.9),
            (cx + 34, GROUND - 18, cz + 44, 1.1),
            (cx - 40, GROUND + 24, cz - 42, 0.7),
            (cx + 46, GROUND + 30, cz + 34, 0.6),
            (cx - 22, GROUND - 22, cz - 48, 0.85),
        ];
        for (i, &(px, py, pz, size)) in heaps.iter().enumerate() {
            let seed = 70 + i as u32 * 5;
            // Lumps along a line, biggest in the middle.
            let lumps = 5;
            for k in 0..lumps {
                let along = k as f32 / (lumps - 1) as f32 - 0.5;
                let r = size * (6.5 - 9.0 * along * along + 2.0 * unit(i as i32, k, seed));
                let lx = px as f32 + along * size * 26.0;
                let lz = pz as f32 + (unit(i as i32, k, seed + 1) - 0.5) * 6.0 * size;
                let ly = py as f32 + r * 0.15;
                let reach = r as i32 + 1;
                for dy in -reach..=reach {
                    for dz in -reach..=reach {
                        for dx in -reach..=reach {
                            let (x, y, z) = (lx as i32 + dx, ly as i32 + dy, lz as i32 + dz);
                            if (y as f32) < py as f32 - size * 1.5 {
                                continue;
                            }
                            let (fx, fy, fz) = (
                                x as f32 + 0.5 - lx,
                                (y as f32 + 0.5 - ly) * 1.25,
                                (z as f32 + 0.5 - lz) * 1.1,
                            );
                            let ragged = r
                                * (0.9
                                    + 0.2 * noise(x as f32 / 2.5, (y * 7 + z) as f32 / 2.5, seed));
                            if fx * fx + fy * fy + fz * fz <= ragged * ragged {
                                self.set(x, y, z, Stuff::Cloud);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Every face with air in front of it, lit by `shade`.
    fn faces(&self, shade: &Shade) -> Vec<Face> {
        let mut faces = Vec::new();
        for y in 0..SIZE[1] {
            for z in 0..SIZE[2] {
                for x in 0..SIZE[0] {
                    let stuff = self.get(x, y, z);
                    if stuff == Stuff::Air {
                        continue;
                    }
                    for (facing, n) in NORMALS.iter().enumerate() {
                        if self.solid(x + n[0], y + n[1], z + n[2]) {
                            continue;
                        }
                        faces.push(Face {
                            at: [x as u8, y as u8, z as u8],
                            facing: facing as u8,
                            rgb: shade.face(self, stuff, [x, y, z], facing),
                        });
                    }
                }
            }
        }
        faces
    }

    /// Whether anything stands between `from` and the sky along `dir`.
    fn shadowed(&self, from: [f32; 3], dir: [f32; 3]) -> bool {
        // Stepping a voxel boundary at a time (a 3-D DDA).
        let mut cell = from.map(|c| floorf(c) as i32);
        let step = dir.map(|d| if d > 0.0 { 1 } else { -1 });
        let delta = dir.map(|d| {
            if d == 0.0 {
                f32::INFINITY
            } else {
                (1.0 / d).abs()
            }
        });
        let mut next = [0.0f32; 3];
        for axis in 0..3 {
            next[axis] = if dir[axis] == 0.0 {
                f32::INFINITY
            } else if step[axis] > 0 {
                (cell[axis] as f32 + 1.0 - from[axis]) * delta[axis]
            } else {
                (from[axis] - cell[axis] as f32) * delta[axis]
            };
        }
        for _ in 0..400 {
            let axis = if next[0] < next[1] && next[0] < next[2] {
                0
            } else if next[1] < next[2] {
                1
            } else {
                2
            };
            cell[axis] += step[axis];
            next[axis] += delta[axis];
            if Self::index(cell[0], cell[1], cell[2]).is_none() {
                return false;
            }
            if self.solid(cell[0], cell[1], cell[2]) {
                return true;
            }
        }
        false
    }
}

/// How the faces are coloured and lit.
struct Shade {
    palette: Palette,
    /// Where the moonlight comes from.
    light: [f32; 3],
    /// What lies in shadow: the dusk itself.
    dark: [f32; 3],
    /// The haze below.
    haze: [u8; 3],
    /// The dusk's last warmth, on everything that faces sideways.
    warm: [f32; 3],
}

fn rgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|c| f32::from(c) / 255.0)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn mix8(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    mix(rgb(a), rgb(b), t).map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

impl Shade {
    fn new(palette: &Palette) -> Self {
        let p = palette;
        let light = [0.55f32, 0.75, -0.45];
        let len = sqrtf(light.iter().map(|c| c * c).sum());
        Self {
            palette: *p,
            light: light.map(|c| c / len),
            dark: rgb(p.base),
            haze: haze(p),
            warm: rgb(mix8(p.rose, p.gold, 0.3)),
        }
    }

    fn colour(&self, stuff: Stuff) -> [f32; 3] {
        let p = &self.palette;
        let c = |a: [u8; 3], b: [u8; 3], t: f32| mix(rgb(a), rgb(b), t);
        match stuff {
            Stuff::Air | Stuff::Grass => mix(c(p.pine, p.foam, 0.4), rgb(p.base), 0.18),
            Stuff::Moss => c(p.pine, p.base, 0.25),
            Stuff::Earth => c(p.overlay, p.muted, 0.45),
            Stuff::Rock => c(p.overlay, p.muted, 0.6),
            Stuff::DarkRock => c(p.overlay, p.base, 0.25),
            Stuff::Stone => c(p.subtle, p.muted, 0.45),
            Stuff::Flag => c(p.subtle, p.gold, 0.15),
            Stuff::Wood => c(p.love, p.base, 0.55),
            Stuff::Lacquer => c(p.love, p.base, 0.12),
            Stuff::Plaster => c(p.text, p.gold, 0.2),
            Stuff::Bracket => c(p.love, p.base, 0.62),
            Stuff::Roof => c(p.overlay, p.muted, 0.25),
            Stuff::Eave => c(p.subtle, p.text, 0.2),
            Stuff::Gilt => rgb(p.gold),
            Stuff::Window => c(p.gold, p.text, 0.25),
            Stuff::Light => c(p.gold, p.text, 0.45),
            Stuff::Blossom => rgb(p.rose),
            Stuff::Bloom => c(p.rose, p.love, 0.55),
            Stuff::Needles => c(p.pine, p.base, 0.35),
            Stuff::Trunk => c(p.muted, p.base, 0.3),
            Stuff::Cloud => mix(c(p.text, p.iris, 0.15), rgb(p.rose), 0.1),
        }
    }

    /// The colour of the face of the voxel `at` looking `facing`.
    fn face(&self, grid: &Grid, stuff: Stuff, at: [i32; 3], facing: usize) -> [u8; 3] {
        let n = NORMALS[facing];
        let [x, y, z] = at;
        let mut colour = self.colour(stuff);
        // A little difference between neighbouring voxels.
        let jitter = (unit(x * 31 + y, z, 3) - 0.5) * 0.08;
        colour = colour.map(|c| c * (1.0 + jitter));
        if stuff.glows() {
            return colour.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
        let nf = n.map(|c| c as f32);
        let facing_light = nf[0] * self.light[0] + nf[1] * self.light[1] + nf[2] * self.light[2];
        let lit = if facing_light > 0.0 {
            let from = [
                x as f32 + 0.5 + nf[0] * 0.51,
                y as f32 + 0.5 + nf[1] * 0.51,
                z as f32 + 0.5 + nf[2] * 0.51,
            ];
            if grid.shadowed(from, self.light) {
                0.0
            } else {
                facing_light
            }
        } else {
            0.0
        };
        // The corners round the air in front of the face: the more of them
        // are solid, the less light reaches in.
        let (ax, ay, az) = (x + n[0], y + n[1], z + n[2]);
        let mut around = 0;
        for a in -1..=1i32 {
            for b in -1..=1i32 {
                if a == 0 && b == 0 {
                    continue;
                }
                let (dx, dy, dz) = match facing {
                    0 | 1 => (0, a, b),
                    2 | 3 => (a, 0, b),
                    _ => (a, b, 0),
                };
                if grid.solid(ax + dx, ay + dy, az + dz) {
                    around += 1;
                }
            }
        }
        let open = 1.0 - 0.06 * around as f32;
        let mut light = (0.4 + 0.6 * lit) * open;
        // Faces turned up see more of the sky.
        if n[1] > 0 {
            light += 0.08;
        } else if n[1] < 0 {
            light *= 0.8;
        }
        colour = mix(self.dark, colour, light.min(1.1));
        // The dusk's warmth along the sides.
        if n[1] == 0 {
            colour = mix(colour, self.warm, 0.07);
        }
        // Below the grass the rock fades into the haze; clouds stay bright.
        if stuff != Stuff::Cloud {
            let below = smooth(GROUND as f32 - 3.0, GROUND as f32 - 36.0, y as f32);
            colour = mix(colour, rgb(self.haze), below * 0.5);
        }
        colour.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
    }
}

/// The haze the island fades into below, and far things into: the sea of
/// cloud at dusk.
pub fn haze(p: &Palette) -> [u8; 3] {
    mix8(p.base, mix8(p.iris, p.rose, 0.35), 0.38)
}

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

    #[test]
    fn the_island_is_built_the_same_every_time() {
        let a = Model::build(&MOON);
        let b = Model::build(&MOON);
        assert!(a == b);
        // Few enough faces for the drawing to tell them apart.
        assert!(a.faces.len() < 65_000, "{} faces", a.faces.len());
        assert!(a.faces.len() > 10_000, "{} faces", a.faces.len());
        assert_eq!(Model::decode(&a.encode()), Some(a.clone()));
        assert_eq!(Model::decode(&a.encode()[..20]), None);
    }

    #[test]
    fn the_pagoda_stands_on_the_island() {
        let mut grid = Grid::new();
        grid.island();
        let [cx, cz] = CENTRE;
        assert!(
            grid.solid(cx, GROUND - 20, cz),
            "the rock is deep in the middle"
        );
        assert!(!grid.solid(cx + 50, GROUND, cz), "and ends");
        grid.platform_and_path();
        grid.pagoda();
        // Up the middle: platform, storeys, spire, and the jewel on top.
        let top = grid.top(cx, cz, SIZE[1]).unwrap();
        assert_eq!(grid.get(cx, top, cz), Stuff::Light);
        assert!(top < SIZE[1] - 4, "room above the jewel: {top}");
        // Nothing reached the floor of the grid.
        assert!((0..SIZE[2]).all(|z| (0..SIZE[0]).all(|x| !grid.solid(x, 0, z))));
    }
}
