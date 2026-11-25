//! Noise for the pictures: the same numbers everywhere, with or without an
//! operating system, so a picture comes out the same wherever it is drawn.

use libm::floorf;

/// 32-bit integer hash: the same everywhere, unlike `sin`.
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32)
        .wrapping_mul(0x8da6_b343)
        .wrapping_add((y as u32).wrapping_mul(0xd816_3841))
        .wrapping_add(seed.wrapping_mul(0xcb1a_b31f));
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    h
}

/// A number in `0..1` for a point and a seed.
pub fn unit(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) >> 8) as f32 / (1u32 << 24) as f32
}

/// `0` below `a`, `1` above `b`, and smoothly between.
pub fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Smooth value noise, `0..1`.
pub fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (floorf(x), floorf(y));
    let (fx, fy) = (x - xi, y - yi);
    let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (xi, yi) = (xi as i32, yi as i32);
    let a = unit(xi, yi, seed);
    let b = unit(xi + 1, yi, seed);
    let c = unit(xi, yi + 1, seed);
    let d = unit(xi + 1, yi + 1, seed);
    lerp(lerp(a, b, u), lerp(c, d, u), v)
}

/// Noise at several scales, `0..1`.
pub fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 0.5, 1.0, 0.0);
    for octave in 0..octaves {
        sum += amp * noise(x * freq, y * freq, seed + octave);
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm
}
