//! Minimal layout geometry in logical pixels.

/// A size in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

/// Horizontal placement inside a rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

/// An axis-aligned rectangle in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Shrink by `amount` on every side (grow if negative).
    #[must_use]
    pub fn inset(&self, amount: f32) -> Self {
        self.inset_xy(amount, amount)
    }

    /// Shrink by `x` on the left and right and `y` on the top and bottom.
    #[must_use]
    pub fn inset_xy(&self, x: f32, y: f32) -> Self {
        Self::new(
            self.x + x,
            self.y + y,
            (self.w - 2.0 * x).max(0.0),
            (self.h - 2.0 * y).max(0.0),
        )
    }

    /// Move by an offset.
    #[must_use]
    pub fn translate(&self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// A `w × h` rectangle centred inside this one.
    #[must_use]
    pub fn centered(&self, w: f32, h: f32) -> Self {
        Self::new(
            self.x + (self.w - w) / 2.0,
            self.y + (self.h - h) / 2.0,
            w,
            h,
        )
    }

    /// The `index`-th of a column of rows, each `height` tall with `gap` between.
    #[must_use]
    pub fn row(&self, index: usize, height: f32, gap: f32) -> Self {
        Self::new(
            self.x,
            self.y + index as f32 * (height + gap),
            self.w,
            height,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_helpers() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(r.inset(5.0), Rect::new(15.0, 25.0, 90.0, 40.0));
        assert_eq!(r.centered(20.0, 10.0), Rect::new(50.0, 40.0, 20.0, 10.0));
        assert_eq!(r.row(2, 10.0, 4.0), Rect::new(10.0, 48.0, 100.0, 10.0));
        assert!(r.contains(10.0, 20.0) && !r.contains(110.0, 20.0));
    }
}
