//! The boot menu's pictures, drawn ahead of time on the build machine:
//! the scene behind the menu, the menu's card once for each choice, and
//! the frame Arch starts on. The firmware has no fonts and no time to
//! spare, so at boot these are only unpacked and put on the screen.
//!
//! Used by the build script, and by the tests to look at.

use enso_core::{
    Context, Paths,
    style::{radius, space, type_scale},
};
use enso_ui::{Align, Offscreen, Picture, Rect, Text, TextEngine, mark};

/// The screen everything is drawn for: this laptop's, and most.
pub const SCREEN: (u32, u32) = (1920, 1080);
/// The card's size.
pub const CARD: (u32, u32) = (440, 196);
/// What the choices are called, in [`crate::menu::CHOICES`]' order.
pub const LABELS: [&str; 2] = ["Arch Linux", "Windows"];

const ROW: f32 = 52.0;

/// The look to draw with: enso's default theme, which is also the login
/// screen's, so the frame Arch starts on is the login screen's first; and
/// the font from `font`, the interface font of whoever builds.
pub fn context(font: Option<String>) -> Context {
    let mut ctx = Context::load_from(Paths::rooted("/nonexistent/enso-boot"));
    if let Some(font) = font {
        ctx.config.appearance.font = font;
    }
    ctx
}

/// The scene, as opaque RGBA, and what in it moves.
pub fn scene(ctx: &Context, size: (u32, u32)) -> (Picture, enso_scene::Life) {
    mark::scene_with_life(&ctx.theme.colors, size)
}

/// The colours of the scene's motion, in full.
pub fn ink(ctx: &Context) -> enso_scene::life::Ink {
    mark::life_ink(&ctx.theme.colors, false)
}

/// The frame Arch starts on: the scene, quietened, exactly as the login
/// screen first shows it before it paints the ensō.
pub fn frame(ctx: &Context, text: &mut TextEngine, scene: &Picture) -> Offscreen {
    let colors = ctx.theme.colors.clone();
    let backdrop = mark::backdrop(scene.clone(), colors.base);
    Offscreen::render(text, backdrop.size(), 1, |canvas| {
        mark::stage(canvas, colors.base, Some(&backdrop));
    })
}

/// The menu's card with choice `chosen` highlighted: premultiplied RGBA.
pub fn card(ctx: &Context, text: &mut TextEngine, chosen: usize) -> Offscreen {
    let colors = ctx.theme.colors.clone();
    let size = ctx.config.appearance.font_size;
    Offscreen::render(text, CARD, 1, |canvas| {
        let bounds = canvas.bounds();
        // Dusk through frosted glass: the scene shows, the words read.
        canvas.fill(bounds, radius::CARD, colors.base.alpha(0.78));
        canvas.stroke(
            bounds.inset(0.5),
            radius::CARD,
            1.0,
            colors.overlay.alpha(0.9),
        );
        let inner = bounds.inset(space::MD);
        for (i, label) in LABELS.iter().enumerate() {
            let row = Rect::new(inner.x, inner.y + i as f32 * ROW, inner.w, ROW - space::XS);
            let chosen = i == chosen;
            if chosen {
                canvas.fill(row, radius::CONTROL, colors.rose.alpha(0.18));
                // The highlight's mark: a small round of rose at its start.
                canvas.circle(
                    row.x + space::LG + 4.0,
                    row.y + row.h / 2.0,
                    4.0,
                    colors.rose,
                );
            }
            let words = Text::new(
                label,
                size * type_scale::HEADING,
                if chosen { colors.text } else { colors.subtle },
            )
            .medium();
            let room = Rect::new(row.x + space::XXL + space::SM, row.y, row.w, row.h);
            canvas.text_in(&words, room, Align::Start);
        }
        let hint = Text::new(
            "↑ ↓  choose      Enter  start",
            size * type_scale::SMALL,
            colors.muted,
        );
        let line = Rect::new(
            inner.x,
            inner.y + LABELS.len() as f32 * ROW + space::SM,
            inner.w,
            inner.bottom() - (inner.y + LABELS.len() as f32 * ROW + space::SM),
        );
        canvas.text_in(&hint, line, Align::Center);
    })
}

/// RGBA to the firmware's B, G, R, A, in place.
pub fn to_bgra(rgba: &mut [u8]) {
    for px in rgba.chunks_mut(4) {
        px.swap(0, 2);
    }
}
