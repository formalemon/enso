//! Motion tokens: easing curves, named transitions, and a retargetable tween.
//!
//! Everything animated in the suite goes through a [`Transition`] so the feel
//! stays consistent — smooth, natural easing in the 180–250 ms range, never an
//! abrupt snap. This module is pure maths over [`Instant`]; drawing lives in
//! `enso-ui`.

use std::time::{Duration, Instant};

use crate::config::MotionConfig;

/// An easing curve mapping linear progress `0..=1` to eased progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    /// Fast start, gentle settle. The default for things arriving or moving.
    OutCubic,
    /// Gentle start, fast end. For things leaving.
    InCubic,
    /// Gentle at both ends. For toggles and back-and-forth movement.
    InOutCubic,
    /// Like `OutCubic` with a longer, softer tail.
    OutQuint,
    /// Overshoots slightly, then settles. For small playful pops.
    OutBack,
    /// Half a cosine: the gentlest start and end. For slow things that
    /// should not be seen to begin or stop, like a brush stroke.
    InOutSine,
}

impl Easing {
    /// Eased value for progress `t` (clamped to `0..=1`).
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::OutCubic => 1.0 - (1.0 - t).powi(3),
            Self::InCubic => t.powi(3),
            Self::InOutCubic => {
                if t < 0.5 {
                    4.0 * t.powi(3)
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Self::OutQuint => 1.0 - (1.0 - t).powi(5),
            Self::OutBack => {
                const C1: f32 = 1.701_58;
                const C3: f32 = C1 + 1.0;
                1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
            }
            Self::InOutSine => 0.5 - 0.5 * (std::f32::consts::PI * t).cos(),
        }
    }
}

/// A duration paired with an easing curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub duration: Duration,
    pub easing: Easing,
}

impl Transition {
    pub const fn new(millis: u64, easing: Easing) -> Self {
        Self {
            duration: Duration::from_millis(millis),
            easing,
        }
    }

    /// No animation at all.
    pub const INSTANT: Self = Self::new(0, Easing::Linear);
    /// Hover and press feedback.
    pub const HOVER: Self = Self::new(120, Easing::OutCubic);
    /// Selection highlights and active indicators sliding between items.
    pub const SELECT: Self = Self::new(180, Easing::OutCubic);
    /// Toggle switches and other two-state controls.
    pub const TOGGLE: Self = Self::new(200, Easing::InOutCubic);
    /// Items sliding to a new slot when their neighbours change.
    pub const REFLOW: Self = Self::new(220, Easing::OutCubic);
    /// Panels, cards, and toasts appearing.
    pub const REVEAL: Self = Self::new(220, Easing::OutQuint);
    /// Panels, cards, and toasts leaving.
    pub const DISMISS: Self = Self::new(250, Easing::InOutCubic);
    /// The whole screen changing: one wallpaper giving way to another. Longer
    /// than interface motion, because it is scenery rather than an answer.
    pub const SCENE: Self = Self::new(420, Easing::InOutCubic);
    /// A refusal: a wrong password's field shaking its head, quickly, the
    /// sway dying away. Linear, because the sway itself is the curve.
    pub const SHAKE: Self = Self::new(380, Easing::Linear);
    /// The ensō being painted, along the stroke.
    pub const BRUSH: Self = Self::new(2800, Easing::InOutSine);
    /// The ensō's idle motion easing in once it is painted.
    pub const SETTLE: Self = Self::new(1600, Easing::InOutSine);
    /// One breath of the ensō: a slow swell and back. A cycle, not a tween.
    pub const BREATH: Self = Self::new(9000, Easing::InOutSine);
    /// Light travelling the ensō's stroke, once.
    pub const SHEEN: Self = Self::new(4000, Easing::InOutSine);
    /// How often the light comes round: its own run and the pause after.
    pub const SHEEN_EVERY: Self = Self::new(14000, Easing::Linear);
}

/// The user's motion preferences applied to transition tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    scale: f32,
}

impl Default for Motion {
    fn default() -> Self {
        Self { scale: 1.0 }
    }
}

impl Motion {
    pub fn new(config: MotionConfig) -> Self {
        let scale = if config.enabled && config.speed.is_finite() {
            config.speed.max(0.0)
        } else {
            0.0
        };
        Self { scale }
    }

    /// All transitions instant (reduced motion).
    pub const fn none() -> Self {
        Self { scale: 0.0 }
    }

    pub fn is_enabled(&self) -> bool {
        self.scale > 0.0
    }

    /// `transition` with its duration scaled by the user's preference.
    pub fn apply(&self, transition: Transition) -> Transition {
        Transition {
            duration: transition.duration.mul_f32(self.scale),
            ..transition
        }
    }
}

/// Linear interpolation, the building block for animating a value.
pub trait Lerp: Copy {
    #[must_use]
    fn lerp(self, to: Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(self, to: Self, t: f32) -> Self {
        self + (to - self) * t
    }
}

impl<A: Lerp, B: Lerp> Lerp for (A, B) {
    fn lerp(self, to: Self, t: f32) -> Self {
        (self.0.lerp(to.0, t), self.1.lerp(to.1, t))
    }
}

/// A value that glides to each new target instead of jumping.
///
/// Retargeting mid-flight starts from wherever the value currently is, so
/// rapid changes (holding `j` in a list) stay smooth.
#[derive(Clone, Copy, Debug)]
pub struct Tween<T> {
    from: T,
    to: T,
    start: Option<Instant>,
    transition: Transition,
}

impl<T: Lerp + PartialEq> Tween<T> {
    /// A value at rest.
    pub fn new(value: T) -> Self {
        Self {
            from: value,
            to: value,
            start: None,
            transition: Transition::INSTANT,
        }
    }

    /// Start moving towards `target`. Does nothing if already heading there.
    pub fn set(&mut self, target: T, transition: Transition, now: Instant) {
        if target == self.to {
            return;
        }
        self.from = self.get(now);
        self.to = target;
        self.transition = transition;
        self.start = (!transition.duration.is_zero()).then_some(now);
    }

    /// Move to `value` immediately.
    pub fn jump(&mut self, value: T) {
        *self = Self::new(value);
    }

    /// Current value.
    pub fn get(&self, now: Instant) -> T {
        match self.progress(now) {
            Some(t) => self.from.lerp(self.to, self.transition.easing.apply(t)),
            None => self.to,
        }
    }

    /// Where the value is heading.
    pub fn target(&self) -> T {
        self.to
    }

    /// Whether the value is still moving, i.e. another frame is needed.
    pub fn is_animating(&self, now: Instant) -> bool {
        self.progress(now).is_some()
    }

    fn progress(&self, now: Instant) -> Option<f32> {
        let elapsed = now.saturating_duration_since(self.start?);
        (elapsed < self.transition.duration)
            .then(|| elapsed.as_secs_f32() / self.transition.duration.as_secs_f32())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Easing; 6] = [
        Easing::Linear,
        Easing::OutCubic,
        Easing::InCubic,
        Easing::InOutCubic,
        Easing::OutQuint,
        Easing::OutBack,
    ];

    #[test]
    fn easings_start_at_zero_and_end_at_one() {
        for easing in ALL {
            assert!(easing.apply(0.0).abs() < 1e-5, "{easing:?}");
            assert!((easing.apply(1.0) - 1.0).abs() < 1e-5, "{easing:?}");
            assert_eq!(easing.apply(-1.0), easing.apply(0.0));
            assert_eq!(easing.apply(2.0), easing.apply(1.0));
        }
        assert!(Easing::OutCubic.apply(0.5) > 0.5);
        assert!(Easing::InCubic.apply(0.5) < 0.5);
    }

    #[test]
    fn interface_transitions_stay_in_the_brief_range() {
        for t in [
            Transition::SELECT,
            Transition::TOGGLE,
            Transition::REFLOW,
            Transition::REVEAL,
            Transition::DISMISS,
        ] {
            assert!((180..=250).contains(&t.duration.as_millis()), "{t:?}");
        }
    }

    #[test]
    fn tween_glides_and_retargets_from_current_value() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let linear = Transition::new(100, Easing::Linear);

        let mut tween = Tween::new(0.0_f32);
        assert!(!tween.is_animating(t0));
        tween.set(10.0, linear, t0);
        assert!(tween.is_animating(t0 + ms(50)));
        assert!((tween.get(t0 + ms(50)) - 5.0).abs() < 1e-3);

        tween.set(0.0, linear, t0 + ms(50));
        assert!((tween.get(t0 + ms(50)) - 5.0).abs() < 1e-3);
        assert!((tween.get(t0 + ms(100)) - 2.5).abs() < 1e-3);
        assert_eq!(tween.get(t0 + ms(150)), 0.0);
        assert!(!tween.is_animating(t0 + ms(150)));
    }

    #[test]
    fn disabled_motion_makes_transitions_instant() {
        let motion = Motion::new(MotionConfig {
            enabled: false,
            speed: 1.0,
        });
        assert!(!motion.is_enabled());
        let now = Instant::now();
        let mut tween = Tween::new(0.0_f32);
        tween.set(1.0, motion.apply(Transition::REVEAL), now);
        assert_eq!(tween.get(now), 1.0);
        assert!(!tween.is_animating(now));

        let slow = Motion::new(MotionConfig {
            enabled: true,
            speed: 2.0,
        });
        assert_eq!(
            slow.apply(Transition::TOGGLE).duration,
            Duration::from_millis(400)
        );
    }
}
