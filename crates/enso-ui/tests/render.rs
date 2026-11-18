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

/// Every figure, at the sizes the tools draw them on a plain screen, and
/// large: a contact sheet to judge the set by (`--features png` writes it
/// to `target/snapshots/figures-*.png`).
#[test]
fn every_figure_draws_at_every_size() {
    use enso_ui::figure::{self, ALL, Look};

    let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-test"));
    let colors = &ctx.theme.colors;
    let mut text = TextEngine::new(&ctx.config.appearance);
    for (name, size, scale) in [("small", 18.0, 1), ("large", 48.0, 2)] {
        let cell = size * 1.8;
        let columns = 12;
        let rows = ALL.len().div_ceil(columns);
        let frame = Offscreen::render(
            &mut text,
            ((cell * columns as f32) as u32, (cell * rows as f32) as u32),
            scale,
            |canvas| {
                canvas.clear(colors.base);
                for (i, figure) in ALL.iter().enumerate() {
                    let (col, row) = ((i % columns) as f32, (i / columns) as f32);
                    let at = (
                        col * cell + (cell - size) / 2.0,
                        row * cell + (cell - size) / 2.0,
                    );
                    figure::draw(canvas, *figure, at, size, Look::new(colors.text));
                }
            },
        );
        // Something was drawn in every cell.
        for (i, figure) in ALL.iter().enumerate() {
            let (col, row) = ((i % columns) as f32, (i / columns) as f32);
            let (cx, cy) = (
                ((col + 0.5) * cell) as u32 * scale,
                ((row + 0.5) * cell) as u32 * scale,
            );
            let reach = (size * 0.45) as u32 * scale;
            let inked = (cx - reach..cx + reach)
                .flat_map(|x| (cy - reach..cy + reach).map(move |y| (x, y)))
                .any(|(x, y)| frame.pixel(x, y) != frame.pixel(0, 0));
            assert!(inked, "{figure:?} drew nothing");
        }
        #[cfg(feature = "png")]
        frame
            .save_png(format!(
                "{}/../../target/snapshots/figures-{name}.png",
                env!("CARGO_MANIFEST_DIR")
            ))
            .expect("write snapshot");
        let _ = name;
    }
}

/// Every widget at rest and in use, in a dark and a light theme: a page to
/// eyeball when the widgets change.
#[test]
fn widgets_draw_in_both_themes() {
    use std::time::{Duration, Instant};

    use enso_core::motion::Transition;
    use enso_ui::widget::{
        Button, Interaction, Item, Kind, Look, Menu, Scroll, TextField, Toggle, controls, field,
    };

    for theme in ["rose-pine-moon", "rose-pine-dawn"] {
        let mut ctx = Context::load_from(Paths::rooted("/nonexistent/enso-test"));
        ctx.theme = ctx.themes.resolve(theme).0.clone();
        let colors = ctx.theme.colors.clone();
        let look = Look::new(&colors, ctx.accent(), 14.0);
        let mut text = TextEngine::new(&ctx.config.appearance);
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        let mut on = Toggle::new(false);
        on.set(true, Transition::TOGGLE, now);
        let mut field = TextField::new("holiday photos.zip");
        field.select_stem();
        let mut menu = Menu::new(
            vec![
                Item::new("Open").icon(icon::FOLDER).keys("Enter"),
                Item::new("Open with…").enabled(false),
                Item::Separator,
                Item::new("Copy").keys("Ctrl+C"),
                Item::new("Show hidden files").checked(true).keys("Ctrl+H"),
            ],
            420.0,
            30.0,
        )
        .with_selected(3);
        let mut scroll = Scroll::default();
        scroll.set_extent(1200.0, 180.0);
        scroll.to(300.0, None, now);

        let frame = Offscreen::render(&mut text, (680, 400), 2, |canvas| {
            canvas.clear(colors.base);
            let mut x = 24.0;
            for (button, state) in [
                (
                    Button::new("Upload 3 files", Kind::Primary),
                    Interaction::default(),
                ),
                (
                    Button::new("Cancel", Kind::Secondary),
                    Interaction {
                        hovered: true,
                        ..Interaction::default()
                    },
                ),
                (
                    Button::new("Delete", Kind::Danger),
                    Interaction {
                        focused: true,
                        ..Interaction::default()
                    },
                ),
                (
                    Button::new("", Kind::Quiet).icon(icon::SEARCH),
                    Interaction {
                        hovered: true,
                        ..Interaction::default()
                    },
                ),
            ] {
                let width = button.width(canvas, &look);
                button.draw(
                    canvas,
                    Rect::new(x, 24.0, width, controls::HEIGHT),
                    &look,
                    state,
                );
                x += width + space::MD;
            }
            Toggle::new(false).draw(
                canvas,
                Rect::new(24.0, 80.0, 40.0, 22.0),
                &look,
                Interaction::default(),
                later,
            );
            on.draw(
                canvas,
                Rect::new(80.0, 80.0, 40.0, 22.0),
                &look,
                Interaction::default(),
                later,
            );
            let field_rect = Rect::new(24.0, 124.0, 300.0, 34.0);
            canvas.fill(field_rect, radius::pill(34.0), colors.surface);
            canvas.stroke(field_rect, radius::pill(34.0), 1.0, look.accent);
            field.draw(
                canvas,
                field_rect.inset_xy(space::LG, 0.0),
                &field::Style {
                    size: 14.0,
                    color: colors.text,
                    placeholder: "Name",
                    placeholder_color: colors.muted,
                    caret: look.accent,
                    selection: look.accent.alpha(0.32),
                },
                true,
            );
            let list = Rect::new(24.0, 180.0, 300.0, 180.0);
            canvas.fill(list, radius::PANEL, colors.surface);
            scroll.draw_bar(canvas, list, colors.subtle, true, now);
            controls::tooltip(
                canvas,
                "a name too long for its column.txt",
                Rect::new(40.0, 200.0, 120.0, 20.0),
                canvas.bounds(),
                &look,
            );
            menu.draw(canvas, canvas.bounds(), &look.menu());
            let bounds = Rect::new(360.0, 220.0, 300.0, 160.0);
            let inner = controls::dialog(canvas, bounds, 260.0, 120.0, &look, 1.0);
            canvas.text(
                &Text::new("Turn on version control?", 15.0, colors.text).medium(),
                inner.x,
                inner.y,
            );
        });
        #[cfg(feature = "png")]
        frame
            .save_png(format!(
                "{}/../../target/snapshots/widgets-{theme}.png",
                env!("CARGO_MANIFEST_DIR")
            ))
            .expect("write snapshot");
        // The primary button is filled with the accent.
        assert_ne!(frame.pixel(60, 60), frame.pixel(4, 4));
    }
}
