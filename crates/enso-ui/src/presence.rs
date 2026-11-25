//! Reveal and dismiss animation for floating overlays.

use std::time::Instant;

use enso_core::{Motion, Transition, Tween};

use crate::shell::Cx;

/// Tracks an overlay easing in when it opens and out before it exits.
///
/// Call [`Presence::get`] from `draw` and scale opacity and offset by it;
/// call [`Presence::close`] instead of `cx.exit()`, then exit on the
/// [`Event::Wake`](crate::Event::Wake) that follows.
#[derive(Clone, Debug)]
pub struct Presence {
    value: Tween<f32>,
    reveal: Transition,
    dismiss: Transition,
    opened: bool,
    closing: bool,
}

impl Presence {
    pub fn new(motion: &Motion) -> Self {
        Self::with(motion, Transition::REVEAL, Transition::DISMISS)
    }

    /// Easing in by `reveal` and out by `dismiss`: for an overlay with a
    /// motion of its own, a linear pair gives it the time to shape.
    pub fn with(motion: &Motion, reveal: Transition, dismiss: Transition) -> Self {
        Self {
            value: Tween::new(0.0),
            reveal: motion.apply(reveal),
            dismiss: motion.apply(dismiss),
            opened: false,
            closing: false,
        }
    }

    /// How present the overlay is: `0` hidden, `1` fully shown. The first
    /// call starts the reveal, so it begins on the first frame actually drawn.
    pub fn get(&mut self, now: Instant) -> f32 {
        if !self.opened {
            self.opened = true;
            self.value.set(1.0, self.reveal, now);
        }
        self.value.get(now)
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.value.is_animating(now)
    }

    /// Start easing out and schedule the wake-up on which the app should exit.
    pub fn close(&mut self, cx: &mut Cx<'_>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.opened = true;
        self.value.set(0.0, self.dismiss, Instant::now());
        cx.wake_in(self.dismiss.duration);
        cx.redraw();
    }

    /// Whether [`Presence::close`] was called.
    pub fn is_closing(&self) -> bool {
        self.closing
    }
}
