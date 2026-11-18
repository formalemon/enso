//! Choosing by dragging a box over empty space: everything it touches is
//! chosen, in the list or the grid. With Ctrl held it adds to what was
//! chosen; without, it starts over in this folder. Dragging past the top or
//! bottom scrolls the list on.

use std::{path::PathBuf, time::Instant};

use enso_core::style::{opacity, radius, space, stroke};
use enso_ui::{Canvas, Rect};

use super::Files;
use crate::config::View;

/// A box being dragged, in content coordinates (the list's, scrolled).
#[derive(Debug)]
pub(super) struct Band {
    from: (f32, f32),
    to: (f32, f32),
    /// What was chosen before the drag, kept under it with Ctrl.
    base: Vec<PathBuf>,
}

/// How near the edge the pointer scrolls the list, and how fast.
const EDGE: f32 = 24.0;
const NUDGE: f32 = 12.0;

impl Files {
    /// Where entry `index` is, in content coordinates: x on screen, y down
    /// the whole list.
    pub(super) fn content_rect(&self, index: usize) -> Rect {
        match self.config.view {
            View::List | View::Columns => {
                let height = self.row_height();
                Rect::new(self.list.x, index as f32 * height, self.list.w, height)
            }
            View::Grid => {
                let (w, h) = self.tile();
                let columns = self.columns();
                let spare = (self.list.w - 2.0 * space::LG - columns as f32 * w) / columns as f32;
                let (col, row) = (index % columns, index / columns);
                Rect::new(
                    self.list.x + space::LG + spare / 2.0 + col as f32 * (w + spare),
                    space::SM + row as f32 * h,
                    w,
                    h - space::SM,
                )
            }
        }
    }

    /// A press on empty space starts a box there.
    pub(super) fn begin_band(&mut self, x: f32, y: f32, add: bool, now: Instant) {
        let content = (x, y - self.list.y + self.scroll.get(now));
        let base = if add {
            self.selection.paths().to_vec()
        } else {
            self.selection.clear_in(self.pane.location());
            self.selection.paths().to_vec()
        };
        self.band = Some(Band {
            from: content,
            to: content,
            base,
        });
    }

    /// The pointer moved with the box out: stretch it, and choose what it
    /// touches. Returns whether there's a box.
    pub(super) fn drag_band(&mut self, x: f32, y: f32, now: Instant) -> bool {
        if self.band.is_none() {
            return false;
        }
        // Past an edge: the list scrolls on under the box.
        if y < self.list.y + EDGE {
            let target = self.scroll.get(now) - NUDGE;
            self.scroll.to(target, None, now);
        } else if y > self.list.bottom() - EDGE {
            let target = self.scroll.get(now) + NUDGE;
            self.scroll.to(target, None, now);
        }
        let to = (x, y - self.list.y + self.scroll.get(now));
        let Some(band) = &mut self.band else {
            return false;
        };
        band.to = to;
        let (from, base) = (band.from, band.base.clone());
        let area = span(from, to);
        self.selection.clear();
        for path in base {
            self.selection.insert(path);
        }
        for index in 0..self.pane.len() {
            if self.can_choose(index) && overlaps(&self.content_rect(index), &area) {
                self.selection.insert(self.pane.path(index));
            }
        }
        true
    }

    /// The button came up: the box is done. Returns whether there was one.
    pub(super) fn end_band(&mut self) -> bool {
        self.band.take().is_some()
    }

    pub(super) fn draw_band(&self, canvas: &mut Canvas<'_>, now: Instant) {
        let Some(band) = &self.band else { return };
        let accent = self.ctx.accent();
        let scroll = self.scroll.get(now);
        let area = span(band.from, band.to);
        let on_screen = Rect::new(area.x, area.y + self.list.y - scroll, area.w, area.h);
        canvas.with_clip(self.list, |canvas| {
            canvas.fill(
                on_screen,
                radius::CONTROL / 2.0,
                accent.alpha(opacity::TINT),
            );
            canvas.stroke(
                on_screen,
                radius::CONTROL / 2.0,
                stroke::HAIRLINE,
                accent.alpha(0.7),
            );
        });
    }
}

/// The rectangle between two corners.
fn span(a: (f32, f32), b: (f32, f32)) -> Rect {
    Rect::new(
        a.0.min(b.0),
        a.1.min(b.1),
        (a.0 - b.0).abs(),
        (a.1 - b.1).abs(),
    )
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_cover_both_ways() {
        let area = span((50.0, 80.0), (10.0, 20.0));
        assert_eq!(area, Rect::new(10.0, 20.0, 40.0, 60.0));
        assert!(overlaps(&area, &Rect::new(45.0, 75.0, 100.0, 100.0)));
        assert!(!overlaps(&area, &Rect::new(50.0, 0.0, 10.0, 10.0)));
    }
}
