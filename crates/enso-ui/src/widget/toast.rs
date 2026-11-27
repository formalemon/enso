//! A toast: a word about what was just done — moved to the trash, copied —
//! floating over the foot of a window for a few seconds, then gone. The
//! file manager's and the viewer's are this one.

use std::time::Duration;

use enso_core::style::{Shadow, radius, space, type_scale};

use crate::{Canvas, Rect, Text, widget::Look};

/// How long a toast stays.
pub const STAYS: Duration = Duration::from_secs(4);

/// Draw `text` as a toast, centred across the canvas, its foot a little
/// above `above` (what it mustn't cover: the status line, a bar).
pub fn draw(canvas: &mut Canvas<'_>, text: &str, above: f32, look: &Look) {
    let size = look.size * type_scale::SMALL;
    let bounds = canvas.bounds();
    let label = Text::new(text, size, look.text).max_width((bounds.w - 4.0 * space::XL).max(100.0));
    let measured = canvas.measure(&label);
    let (w, h) = (measured.w + 2.0 * space::LG, measured.h + 2.0 * space::MD);
    let rect = Rect::new((bounds.w - w) / 2.0, above - h - space::LG, w, h);
    canvas.shadow(rect, radius::pill(h.min(40.0)), Shadow::FLOATING);
    canvas.fill(rect, radius::CONTROL.max(h.min(40.0) / 2.0), look.overlay);
    canvas.text(&label, rect.x + space::LG, rect.y + space::MD);
}
