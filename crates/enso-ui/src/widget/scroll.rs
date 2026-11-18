//! Scrolling a view taller than its window: the offset, gliding when a
//! wheel or a key moves it and following exactly when fingers do, and a
//! thin scrollbar that can be dragged or clicked.

use std::time::Instant;

use enso_core::{
    Rgba,
    motion::{Transition, Tween},
};

use crate::{Canvas, Rect};

/// What a compositor reports for one wheel notch, in logical pixels.
pub const NOTCH: f32 = 15.0;
/// The scrollbar, thin at rest and wider under the pointer.
const BAR: f32 = 4.0;
const BAR_HOVER: f32 = 8.0;
const BAR_MIN: f32 = 28.0;

#[derive(Clone, Debug)]
pub struct Scroll {
    offset: Tween<f32>,
    content: f32,
    view: f32,
    /// While the thumb is dragged: where the pointer grabbed it, along it.
    drag: Option<f32>,
}

impl Default for Scroll {
    fn default() -> Self {
        Self {
            offset: Tween::new(0.0),
            content: 0.0,
            view: 0.0,
            drag: None,
        }
    }
}

impl Scroll {
    /// How tall the content is and how much of it shows. Keeps the offset
    /// in range when either shrinks.
    pub fn set_extent(&mut self, content: f32, view: f32) {
        self.content = content.max(0.0);
        self.view = view.max(0.0);
        if self.offset.target() > self.max() {
            self.offset.jump(self.max());
        }
    }

    pub fn max(&self) -> f32 {
        (self.content - self.view).max(0.0)
    }

    /// The offset to draw at.
    pub fn get(&self, now: Instant) -> f32 {
        self.offset.get(now)
    }

    /// Where it's going.
    pub fn target(&self) -> f32 {
        self.offset.target()
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.offset.is_animating(now)
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Go to `offset`, gliding with `transition` or jumping with `None`.
    pub fn to(&mut self, offset: f32, transition: Option<Transition>, now: Instant) {
        let offset = offset.clamp(0.0, self.max());
        match transition {
            Some(transition) => self.offset.set(offset, transition, now),
            None => self.offset.jump(offset),
        }
    }

    /// A scroll event: from a wheel, `step` per notch, gliding; from a
    /// touchpad, exactly as far as the fingers went.
    pub fn scroll(
        &mut self,
        dy: f32,
        wheel: bool,
        step: f32,
        transition: Transition,
        now: Instant,
    ) {
        if wheel {
            let notches = (dy / NOTCH).round();
            let notches = if notches == 0.0 { dy.signum() } else { notches };
            let target = self.offset.target() + notches * step;
            self.to(target, Some(transition), now);
        } else {
            let target = self.offset.get(now) + dy;
            self.to(target, None, now);
        }
    }

    /// Scroll as little as keeps `top..top + height` in view.
    pub fn reveal(&mut self, top: f32, height: f32, transition: Transition, now: Instant) {
        let scroll = self.offset.target();
        let target = if top < scroll {
            top
        } else if top + height > scroll + self.view {
            top + height - self.view
        } else {
            return;
        };
        self.to(target, Some(transition), now);
    }

    /// The scrollbar's track and thumb along the right of `view`, if the
    /// content overflows.
    fn bar(&self, view: Rect, now: Instant, wide: bool) -> Option<(Rect, Rect)> {
        if self.content <= self.view || self.view <= 0.0 {
            return None;
        }
        let width = if wide { BAR_HOVER } else { BAR };
        let track = Rect::new(
            view.right() - width - 2.0,
            view.y + 2.0,
            width,
            view.h - 4.0,
        );
        let length = (track.h * self.view / self.content)
            .max(BAR_MIN)
            .min(track.h);
        let travel = track.h - length;
        let at = if self.max() > 0.0 {
            self.get(now) / self.max()
        } else {
            0.0
        };
        Some((
            track,
            Rect::new(track.x, track.y + travel * at, width, length),
        ))
    }

    /// Draw the scrollbar at the right of `view`; wider while `hovered`
    /// or dragged.
    pub fn draw_bar(
        &self,
        canvas: &mut Canvas<'_>,
        view: Rect,
        color: Rgba,
        hovered: bool,
        now: Instant,
    ) {
        let wide = hovered || self.is_dragging();
        if let Some((_, thumb)) = self.bar(view, now, wide) {
            let alpha = if wide { 0.7 } else { 0.4 };
            canvas.pill(thumb, color.alpha(alpha));
        }
    }

    /// Whether `(x, y)` is on the scrollbar's strip.
    pub fn is_over_bar(&self, view: Rect, x: f32, y: f32) -> bool {
        self.content > self.view && view.contains(x, y) && x >= view.right() - BAR_HOVER - 6.0
    }

    /// A press on the scrollbar: grab the thumb, or page towards the
    /// press. Returns whether it was on the bar.
    pub fn press(
        &mut self,
        view: Rect,
        x: f32,
        y: f32,
        transition: Transition,
        now: Instant,
    ) -> bool {
        if !self.is_over_bar(view, x, y) {
            return false;
        }
        let Some((_, thumb)) = self.bar(view, now, true) else {
            return false;
        };
        if y >= thumb.y && y <= thumb.bottom() {
            self.drag = Some(y - thumb.y);
        } else {
            let page = if y < thumb.y { -self.view } else { self.view };
            let target = self.offset.target() + page * 0.9;
            self.to(target, Some(transition), now);
        }
        true
    }

    /// The pointer moved while the thumb is held. Returns whether it moved.
    pub fn drag(&mut self, view: Rect, y: f32, now: Instant) -> bool {
        let (Some(grab), Some((track, thumb))) = (self.drag, self.bar(view, now, true)) else {
            return false;
        };
        let travel = track.h - thumb.h;
        if travel <= 0.0 {
            return false;
        }
        let at = ((y - grab - track.y) / travel).clamp(0.0, 1.0);
        self.to(at * self.max(), None, now);
        true
    }

    pub fn release(&mut self) {
        self.drag = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheels_step_and_fingers_follow() {
        let now = Instant::now();
        let mut scroll = Scroll::default();
        scroll.set_extent(1000.0, 200.0);
        scroll.scroll(15.0, true, 96.0, Transition::SELECT, now);
        assert_eq!(scroll.target(), 96.0);
        scroll.scroll(30.0, true, 96.0, Transition::SELECT, now);
        assert_eq!(scroll.target(), 288.0);
        // Never past the end.
        scroll.scroll(1500.0, true, 96.0, Transition::SELECT, now);
        assert_eq!(scroll.target(), 800.0);
        scroll.to(100.0, None, now);
        scroll.scroll(7.5, false, 96.0, Transition::SELECT, now);
        assert_eq!(scroll.get(now), 107.5);
    }

    #[test]
    fn revealing_scrolls_as_little_as_it_can() {
        let now = Instant::now();
        let mut scroll = Scroll::default();
        scroll.set_extent(1000.0, 200.0);
        scroll.reveal(250.0, 32.0, Transition::INSTANT, now);
        assert_eq!(scroll.target(), 82.0);
        scroll.reveal(100.0, 32.0, Transition::INSTANT, now);
        assert_eq!(scroll.target(), 82.0, "already in view");
        scroll.reveal(10.0, 32.0, Transition::INSTANT, now);
        assert_eq!(scroll.target(), 10.0);
    }

    #[test]
    fn shrinking_content_pulls_the_offset_back() {
        let now = Instant::now();
        let mut scroll = Scroll::default();
        scroll.set_extent(1000.0, 200.0);
        scroll.to(800.0, None, now);
        scroll.set_extent(300.0, 200.0);
        assert_eq!(scroll.target(), 100.0);
    }

    #[test]
    fn the_thumb_drags_and_the_track_pages() {
        let now = Instant::now();
        let view = Rect::new(0.0, 0.0, 300.0, 204.0);
        let mut scroll = Scroll::default();
        scroll.set_extent(2000.0, 204.0);
        // A press low on the track pages down.
        assert!(scroll.press(view, 295.0, 190.0, Transition::INSTANT, now));
        assert!(scroll.target() > 0.0);
        scroll.to(0.0, None, now);
        // Grab the thumb at its top and pull it to the bottom.
        assert!(scroll.press(view, 295.0, 3.0, Transition::INSTANT, now));
        assert!(scroll.is_dragging());
        assert!(scroll.drag(view, 1000.0, now));
        assert_eq!(scroll.get(now), scroll.max());
        scroll.release();
        assert!(!scroll.drag(view, 0.0, now));
        // Away from the bar, presses are the list's.
        assert!(!scroll.press(view, 100.0, 100.0, Transition::INSTANT, now));
    }
}
