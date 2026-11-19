//! The timeline: what has played is a wave, what's left a quiet straight
//! line, and between them a small upright thumb. While something plays the
//! wave rolls gently onwards; paused, it lies down flat — so the line itself
//! says whether it's playing.

use std::f32::consts::TAU;

use enso_core::Rgba;
use enso_ui::{Canvas, Rect, Shape};

/// How thick the line is.
pub const PEN: f32 = 4.0;
/// The thumb: a rounded upright bar.
const THUMB: (f32, f32) = (4.0, 18.0);
/// Room between the thumb and the lines either side of it.
const GAP: f32 = 5.0;
/// How far the drawn wave steps along, in pixels.
const STEP: f32 = 1.0;

/// The shape of the wave.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wave {
    /// Height of a crest above the middle; 0 is a straight line.
    pub amplitude: f32,
    /// Length of one crest and trough.
    pub wavelength: f32,
    /// How far it has rolled, in radians.
    pub phase: f32,
}

impl Wave {
    /// The wave at rest — `amplitude` scaled by how much it's playing.
    pub fn calm(playing: f32, phase: f32) -> Self {
        Self {
            amplitude: 3.0 * playing.clamp(0.0, 1.0),
            wavelength: 22.0,
            phase,
        }
    }
}

/// The colours of the timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub played: Rgba,
    pub rest: Rgba,
}

/// Points along the wave from `x0` to `x1` about `y`. It eases in from the
/// start and out into the thumb over half a wavelength each, so it begins
/// and ends on the middle line rather than mid-crest.
pub fn points(x0: f32, x1: f32, y: f32, wave: Wave) -> Vec<(f32, f32)> {
    if x1 <= x0 {
        return Vec::new();
    }
    let ease = wave.wavelength / 2.0;
    let steps = ((x1 - x0) / STEP).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|i| {
            let x = (x0 + i as f32 * STEP).min(x1);
            let edge = ((x - x0).min(x1 - x)) / ease;
            let taper = smoothstep(edge.clamp(0.0, 1.0));
            let angle = TAU * (x - x0) / wave.wavelength - wave.phase;
            (x, y + wave.amplitude * taper * angle.sin())
        })
        .collect()
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Where along `track` a pointer at `x` falls, `0..=1`.
pub fn fraction_at(track: Rect, x: f32) -> f32 {
    let (x0, x1) = ends(track);
    ((x - x0) / (x1 - x0).max(1.0)).clamp(0.0, 1.0)
}

/// The first and last points the line reaches: its round ends stay inside
/// the track.
fn ends(track: Rect) -> (f32, f32) {
    (track.x + PEN / 2.0, track.right() - PEN / 2.0)
}

/// Draw the timeline across `track`, `fraction` of it played. `grow` (0..1)
/// lifts the thumb a little, while it's held.
pub fn draw(
    canvas: &mut Canvas<'_>,
    track: Rect,
    fraction: f32,
    wave: Wave,
    look: Look,
    grow: f32,
) {
    let (x0, x1) = ends(track);
    let y = track.y + track.h / 2.0;
    let at = x0 + (x1 - x0) * fraction.clamp(0.0, 1.0);

    if at + GAP < x1 {
        let mut rest = Shape::new();
        rest.move_to((at + GAP).max(x0), y).line_to(x1, y);
        canvas.stroke_shape(&rest, PEN, look.rest);
    }
    let played = points(x0, at - GAP, y, wave);
    if let Some((&first, others)) = played.split_first() {
        let mut line = Shape::new();
        line.move_to(first.0, first.1);
        for &(x, y) in others {
            line.line_to(x, y);
        }
        if others.is_empty() {
            line.line_to(first.0, first.1);
        }
        canvas.stroke_shape(&line, PEN, look.played);
    }
    let (w, h) = (THUMB.0, THUMB.1 + 4.0 * grow.clamp(0.0, 1.0));
    canvas.fill(
        Rect::new(at - w / 2.0, y - h / 2.0, w, h),
        w / 2.0,
        look.played,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wave_begins_and_ends_on_the_line_and_stays_within_its_crests() {
        let wave = Wave::calm(1.0, 1.3);
        let points = points(10.0, 200.0, 50.0, wave);
        let (first, last) = (points[0], points[points.len() - 1]);
        assert!((first.1 - 50.0).abs() < 0.01 && (last.1 - 50.0).abs() < 0.01);
        assert_eq!((first.0, last.0), (10.0, 200.0));
        assert!(
            points
                .iter()
                .all(|&(_, y)| (y - 50.0).abs() <= wave.amplitude + 0.01)
        );
        // It does wave in the middle.
        assert!(
            points
                .iter()
                .any(|&(_, y)| (y - 50.0).abs() > wave.amplitude * 0.9)
        );
    }

    #[test]
    fn paused_it_lies_flat() {
        let points = points(0.0, 100.0, 20.0, Wave::calm(0.0, 2.0));
        assert!(points.iter().all(|&(_, y)| y == 20.0));
        assert!(super::points(5.0, 5.0, 0.0, Wave::calm(1.0, 0.0)).is_empty());
    }

    #[test]
    fn the_pointer_finds_its_place_on_the_track() {
        let track = Rect::new(20.0, 0.0, 204.0, 24.0);
        assert_eq!(fraction_at(track, 0.0), 0.0);
        assert_eq!(fraction_at(track, 500.0), 1.0);
        assert!((fraction_at(track, 122.0) - 0.5).abs() < 0.01);
    }
}
