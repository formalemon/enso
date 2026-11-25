//! The level the status island shows as it changes: the volume, the
//! screen's brightness, the microphone, the keyboard's light.
//!
//! Pressing a volume key, scrolling on the island, a headset's buttons, the
//! brightness keys — whatever changed it, the island answers: it widens to
//! the left into a speaker (or a sun, or a microphone), a track filled as
//! far as the level, and the figure, then folds back into its icons a
//! moment after the last change. One surface, the island's own; nothing
//! opens.

use std::time::{Duration, Instant};

use enso_core::{Figure, Motion, Palette, Rgba, Transition, Tween};
use enso_ui::{Align, Canvas, Rect, Text};

use crate::icons::{self, Ink, Scale};

/// How long the level stays after the last change.
const HOLD: Duration = Duration::from_millis(1500);

/// What the island is showing the level of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gauge {
    Volume,
    Brightness,
    Microphone,
    /// The keyboard's light.
    Keyboard,
}

/// How the level is drawn: at the island's scale, in the theme's colours.
#[derive(Clone, Copy, Debug)]
pub struct Look<'a> {
    pub scale: Scale,
    /// The figures' size.
    pub size: f32,
    pub colors: &'a Palette,
}

#[derive(Debug)]
pub struct Osd {
    gauge: Gauge,
    level: u8,
    muted: bool,
    /// `0` folded into the island's icons, `1` showing the level.
    shown: Tween<f32>,
    /// How far the track is filled, `0..=1`, gliding between levels.
    fill: Tween<f32>,
    /// When to fold away, while showing.
    hide_at: Option<Instant>,
}

impl Osd {
    pub fn new() -> Self {
        Self {
            gauge: Gauge::Volume,
            level: 0,
            muted: false,
            shown: Tween::new(0.0),
            fill: Tween::new(0.0),
            hide_at: None,
        }
    }

    /// Show `gauge` at `level` percent (`muted`: heard as nothing).
    pub fn show(&mut self, gauge: Gauge, level: u8, muted: bool, now: Instant, motion: &Motion) {
        let fill = f32::from(level.min(100)) / 100.0;
        // A new gauge, or one coming up from folded away, starts where it
        // is; a level changing while shown glides there.
        if self.gauge != gauge || self.shown.target() == 0.0 {
            self.fill = Tween::new(fill);
        } else {
            self.fill.set(fill, motion.apply(Transition::SELECT), now);
        }
        self.gauge = gauge;
        self.level = level;
        self.muted = muted;
        if self.shown.target() != 1.0 {
            self.shown.set(1.0, motion.apply(Transition::REVEAL), now);
        }
        self.hide_at = Some(now + HOLD);
    }

    /// Fold away if its time is up. Returns whether that began now.
    pub fn tick(&mut self, now: Instant, motion: &Motion) -> bool {
        if self.hide_at.is_some_and(|at| now >= at) {
            self.hide_at = None;
            self.shown.set(0.0, motion.apply(Transition::DISMISS), now);
            return true;
        }
        false
    }

    /// When [`Osd::tick`] next has something to do.
    pub fn due(&self) -> Option<Instant> {
        self.hide_at
    }

    /// How much it shows, `0..=1`.
    pub fn presence(&self, now: Instant) -> f32 {
        self.shown.get(now)
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.shown.is_animating(now) || self.fill.is_animating(now)
    }

    fn figure(&self) -> Figure {
        match self.gauge {
            Gauge::Volume if self.muted || self.level == 0 => Figure::SpeakerMuted,
            Gauge::Volume => Figure::Speaker(match self.level {
                0..=33 => 1,
                34..=66 => 2,
                _ => 3,
            }),
            Gauge::Brightness => Figure::Brightness,
            Gauge::Microphone if self.muted => Figure::MicrophoneOff,
            Gauge::Microphone => Figure::Microphone,
            Gauge::Keyboard => Figure::Keyboard,
        }
    }

    fn label(&self) -> String {
        if self.muted {
            "Off".to_owned()
        } else {
            format!("{}%", self.level)
        }
    }

    /// Width of the island while it shows the level.
    pub fn width(&self, canvas: &mut Canvas<'_>, scale: Scale, size: f32) -> f32 {
        // As wide for 5 % as for 100 %, so the island doesn't twitch.
        let widest = canvas
            .measure(&Text::new("100%", size, Rgba::TRANSPARENT).medium().tabular())
            .w
            .ceil();
        let icon = scale
            .width(Figure::Speaker(3))
            .max(scale.width(Figure::Brightness));
        scale.pad() + icon + scale.gap() + track(scale) + scale.gap() + widest + scale.pad()
    }

    /// Draw the level across `island` (as wide as [`Osd::width`] said).
    pub fn draw(&self, canvas: &mut Canvas<'_>, island: Rect, look: Look<'_>, now: Instant) {
        let Look {
            scale,
            size,
            colors,
        } = look;
        let quiet = self.muted;
        let on = if quiet { colors.muted } else { colors.text };
        let icon_h = scale.icon();
        let figure = self.figure();
        let icon_w = scale.width(figure);
        let x = island.x + scale.pad();
        let y = island.y + ((island.h - icon_h) / 2.0).round();
        let fill = self.fill.get(now).clamp(0.0, 1.0);
        // The sun's rays reach as far as the screen is bright, as it changes.
        let live = enso_ui::figure::Live {
            time: None,
            amount: (figure == Figure::Brightness).then_some(fill),
        };
        let ink = Ink {
            on,
            ground: colors.base,
        };
        icons::draw_live(canvas, figure, Rect::new(x, y, icon_w, icon_h), ink, live);

        let slot = scale
            .width(Figure::Speaker(3))
            .max(scale.width(Figure::Brightness));
        let track_x = x + slot + scale.gap();
        let thickness = scale.px(5.0).round().max(3.0);
        let bar = Rect::new(
            track_x,
            island.y + ((island.h - thickness) / 2.0).round(),
            track(scale),
            thickness,
        );
        canvas.pill(bar, colors.highlight_high);
        // A level is a quantity, not a state: quiet, as the control
        // center's sliders are.
        if fill > 0.0 {
            let lit = Rect {
                w: (bar.w * fill).max(bar.h),
                ..bar
            };
            canvas.pill(lit, if quiet { colors.muted } else { colors.subtle });
        }

        let label = self.label();
        let text = Text::new(&label, size, on).medium().tabular();
        let figures = Rect::new(
            bar.right() + scale.gap(),
            island.y,
            island.right() - scale.pad() - bar.right() - scale.gap(),
            island.h,
        );
        canvas.text_in(&text, figures, Align::End);
    }
}

/// How long the track is.
fn track(scale: Scale) -> f32 {
    scale.px(92.0).round()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_then_folds_away_after_the_last_change() {
        let motion = Motion::default();
        let mut osd = Osd::new();
        let t0 = Instant::now();
        assert_eq!(osd.presence(t0), 0.0);
        osd.show(Gauge::Volume, 40, false, t0, &motion);
        assert!(osd.due().is_some());
        assert!(!osd.tick(t0 + HOLD / 2, &motion));
        // Another change keeps it up for another while.
        osd.show(Gauge::Volume, 45, false, t0 + HOLD / 2, &motion);
        assert!(!osd.tick(t0 + HOLD, &motion));
        assert!(osd.tick(t0 + HOLD * 2, &motion));
        assert_eq!(osd.due(), None);
        assert_eq!(osd.presence(t0 + HOLD * 4), 0.0);
    }

    #[test]
    fn the_figure_follows_the_level() {
        let motion = Motion::default();
        let now = Instant::now();
        let mut osd = Osd::new();
        osd.show(Gauge::Volume, 20, false, now, &motion);
        assert_eq!(osd.figure(), Figure::Speaker(1));
        osd.show(Gauge::Volume, 90, false, now, &motion);
        assert_eq!(osd.figure(), Figure::Speaker(3));
        osd.show(Gauge::Volume, 90, true, now, &motion);
        assert_eq!(
            (osd.figure(), osd.label().as_str()),
            (Figure::SpeakerMuted, "Off")
        );
        osd.show(Gauge::Microphone, 70, true, now, &motion);
        assert_eq!(osd.figure(), Figure::MicrophoneOff);
        osd.show(Gauge::Brightness, 5, false, now, &motion);
        assert_eq!(
            (osd.figure(), osd.label().as_str()),
            (Figure::Brightness, "5%")
        );
    }
}
