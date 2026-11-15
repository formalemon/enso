//! Headless rendering checks for the canvas. With `--features png` the frame
//! is also written to `target/snapshots/` for eyeballing.

use enso_core::{
    Context, Paths, Rgba,
    style::{Shadow, icon, radius, space},
};
use enso_ui::{Align, Offscreen, Rect, Text, TextEngine};

#[test]
fn canvas_draws_shapes_text_and_icons() {
    let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-test"));
    let colors = &ctx.theme.colors;
    let mut text = TextEngine::new(&ctx.config.appearance);

    let frame = Offscreen::render(&mut text, (360, 200), 2, |canvas| {
        canvas.clear(colors.base);
        let card = Rect::new(20.0, 20.0, 320.0, 160.0);
        canvas.shadow(card, radius::CARD, Shadow::FLOATING, Rgba::rgb(0, 0, 0));
        canvas.fill(card, radius::CARD, colors.surface);
        canvas.stroke(card, radius::CARD, 1.0, colors.highlight_med);

        let inner = card.inset(space::LG);
        canvas.text(
            &Text::new("Soft pastel zen", 20.0, colors.text).bold(),
            inner.x,
            inner.y,
        );
        canvas.text(
            &Text::new(
                "Rounded shapes, gentle shadows, and breathing room.",
                14.0,
                colors.subtle,
            )
            .max_width(inner.w)
            .max_lines(2),
            inner.x,
            inner.y + 32.0,
        );

        // Accent swatches as pills, then a truncated line and an icon.
        for (i, color) in [
            colors.love,
            colors.gold,
            colors.rose,
            colors.pine,
            colors.foam,
            colors.iris,
        ]
        .into_iter()
        .enumerate()
        {
            canvas.pill(
                Rect::new(inner.x + i as f32 * 36.0, inner.y + 84.0, 28.0, 12.0),
                color,
            );
        }
        let row = Rect::new(inner.x, inner.bottom() - 22.0, inner.w, 22.0);
        canvas.icon(icon::BELL, 16.0, colors.rose, row, Align::Start);
        canvas.text_in(
            &Text::new(
                "A very long line that has to be truncated with an ellipsis",
                13.0,
                colors.muted,
            ),
            Rect::new(row.x + 40.0, row.y, 180.0, row.h),
            Align::Start,
        );
        canvas.with_opacity(0.5, |canvas| {
            canvas.circle(row.right() - 8.0, row.y + 11.0, 6.0, colors.foam)
        });
    });

    assert_eq!((frame.width, frame.height), (720, 400));
    let px = |c: Rgba| [c.r, c.g, c.b, c.a];
    // Outside the card: opaque base. Inside: the surface colour.
    assert_eq!(frame.pixel(4, 4), px(colors.base));
    assert_eq!(frame.pixel(360, 300), px(colors.surface));
    // The rounded corner leaves the card's bounding-box corner unfilled.
    assert_ne!(frame.pixel(41, 41), px(colors.surface));
    // The first swatch is the `love` accent.
    assert_eq!(frame.pixel(100, 250), px(colors.love));
    // Text was rasterised: some pixels in the heading are not plain surface.
    let heading_has_ink = (80..400).any(|x| frame.pixel(x, 95) != px(colors.surface));
    assert!(
        heading_has_ink,
        "no glyph pixels found — are any fonts installed?"
    );

    #[cfg(feature = "png")]
    frame
        .save_png(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/snapshots/ui-canvas.png"
        ))
        .expect("write snapshot");
}
