//! The launcher overlay: a search field over a ranked list.

use std::time::Instant;

use enso_core::{
    Context, Rgba, Role, Transition, Tween,
    style::{Shadow, icon, opacity, radius, space, stroke, type_scale},
};
use enso_ui::{
    Align, App as UiApp, Button, Canvas, Cx, Event, Key, KeyEvent, Nav, Paint, PointerEvent,
    Presence, Rect, SurfaceSpec, Text,
};
use serde::Deserialize;

use crate::{apps::App, fuzzy};

/// The `[launch]` table of `config.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchConfig {
    /// Card width in logical pixels.
    pub width: u32,
    /// Rows visible at once; the list scrolls beyond this.
    pub max_results: usize,
    /// Terminal emulator for applications that need one.
    pub terminal: String,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            width: 560,
            max_results: 8,
            terminal: "alacritty".to_owned(),
        }
    }
}

const SEARCH_HEIGHT: f32 = 56.0;
const ROW_HEIGHT: f32 = 44.0;
const CHIP: f32 = 28.0;

#[derive(Debug)]
pub struct Launcher {
    ctx: Context,
    config: LaunchConfig,
    presence: Presence,
    apps: Vec<App>,
    query: String,
    /// Indices into `apps`, best match first.
    results: Vec<usize>,
    selected: usize,
    /// First visible result.
    offset: usize,
    /// Animated position of the selection highlight, in visible rows.
    highlight: Tween<f32>,
    /// Animated height of the result list.
    list_height: Tween<f32>,
    /// Geometry from the last frame, for hit-testing.
    card: Rect,
    rows: Vec<Rect>,
}

impl Launcher {
    pub fn new(ctx: Context, config: LaunchConfig, apps: Vec<App>) -> Self {
        let mut launcher = Self {
            presence: Presence::new(&ctx.motion),
            ctx,
            config,
            apps,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            offset: 0,
            highlight: Tween::new(0.0),
            list_height: Tween::new(0.0),
            card: Rect::default(),
            rows: Vec::new(),
        };
        launcher.search();
        // The list starts at its full height instead of growing in.
        launcher.list_height.jump(launcher.list_target());
        launcher
    }

    fn visible_rows(&self) -> usize {
        self.results.len().min(self.config.max_results.max(1))
    }

    fn list_target(&self) -> f32 {
        match self.visible_rows() {
            // Room for the "no matches" line.
            0 => ROW_HEIGHT + space::SM,
            rows => rows as f32 * ROW_HEIGHT + space::SM,
        }
    }

    /// Re-rank applications for the current query and reset the selection.
    fn search(&mut self) {
        let mut ranked: Vec<(i32, usize)> = self
            .apps
            .iter()
            .enumerate()
            .filter_map(|(index, app)| rank(&self.query, app).map(|score| (score, index)))
            .collect();
        // Stable sort: ties keep the alphabetical order of `apps`.
        ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        self.results = ranked.into_iter().map(|(_, index)| index).collect();
        self.selected = 0;
        self.offset = 0;
        let now = Instant::now();
        self.highlight
            .set(0.0, self.ctx.motion.apply(Transition::SELECT), now);
        self.list_height.set(
            self.list_target(),
            self.ctx.motion.apply(Transition::REFLOW),
            now,
        );
    }

    fn select(&mut self, index: usize) {
        if self.results.is_empty() {
            return;
        }
        self.selected = index.min(self.results.len() - 1);
        // Scroll just enough to keep the selection visible.
        let rows = self.visible_rows();
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + rows {
            self.offset = self.selected + 1 - rows;
        }
        self.highlight.set(
            (self.selected - self.offset) as f32,
            self.ctx.motion.apply(Transition::SELECT),
            Instant::now(),
        );
    }

    fn move_selection(&mut self, nav: Nav) {
        let count = self.results.len();
        if count == 0 {
            return;
        }
        self.select(match nav {
            Nav::Down => (self.selected + 1) % count,
            Nav::Up => (self.selected + count - 1) % count,
        });
    }

    /// Edit the query. Returns whether the key was consumed.
    fn edit(&mut self, key: &KeyEvent) -> bool {
        match key.key {
            Key::Backspace => {
                self.query.pop();
            }
            Key::Char('u') if key.mods.ctrl => self.query.clear(),
            Key::Char('w') if key.mods.ctrl => {
                let trimmed = self.query.trim_end();
                let cut = trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
                self.query.truncate(cut);
            }
            _ => match key.text() {
                Some(c) => self.query.push(c),
                None => return false,
            },
        }
        self.search();
        true
    }

    fn selected_app(&self) -> Option<&App> {
        self.results
            .get(self.selected)
            .map(|index| &self.apps[*index])
    }

    fn launch_selected(&mut self, cx: &mut Cx<'_>) {
        if let Some(app) = self.selected_app() {
            if let Err(err) = app.launch(&self.config.terminal) {
                eprintln!("enso-launch: cannot start {}: {err}", app.name);
            }
            self.presence.close(cx);
        }
    }
}

/// Best score for an application across its searchable fields. The name
/// counts most; description, keywords, and id are progressively discounted.
fn rank(query: &str, app: &App) -> Option<i32> {
    let name = fuzzy::score(query, &app.name);
    let description = app
        .description
        .as_deref()
        .and_then(|d| fuzzy::score(query, d))
        .map(|s| s - 25);
    let keywords = app
        .keywords
        .iter()
        .filter_map(|k| fuzzy::score(query, k))
        .max()
        .map(|s| s - 30);
    let id = fuzzy::score(query, &app.id).map(|s| s - 35);
    [name, description, keywords, id]
        .into_iter()
        .flatten()
        .max()
}

/// A stable accent for an application's letter chip, picked by name.
fn chip_role(name: &str) -> Role {
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    });
    Role::ACCENTS[hash as usize % Role::ACCENTS.len()]
}

impl UiApp for Launcher {
    type Message = ();

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::overlay("enso-launch")
    }

    fn event(&mut self, event: Event<()>, cx: &mut Cx<'_>) {
        match event {
            Event::Wake if self.presence.is_closing() => cx.exit(),
            Event::Key(key) => {
                if let Some(nav) = key.nav(false) {
                    self.move_selection(nav);
                } else {
                    match key.key {
                        Key::Escape => return self.presence.close(cx),
                        Key::Enter => return self.launch_selected(cx),
                        _ if !self.edit(&key) => return,
                        _ => {}
                    }
                }
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Move { x, y }) => {
                if let Some(row) = self.rows.iter().position(|row| row.contains(x, y))
                    && self.offset + row != self.selected
                {
                    self.select(self.offset + row);
                    cx.redraw();
                }
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                if let Some(row) = self.rows.iter().position(|row| row.contains(x, y)) {
                    self.select(self.offset + row);
                    self.launch_selected(cx);
                } else if !self.card.contains(x, y) {
                    self.presence.close(cx);
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let presence = self.presence.get(now);
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let base_size = self.ctx.config.appearance.font_size;
        let title = base_size * type_scale::TITLE;
        let bounds = canvas.bounds();

        // Dim what is behind so the card is the only thing asking for attention.
        canvas.fill(bounds, 0.0, colors.base.alpha(opacity::SCRIM * presence));

        let list_height = self.list_height.get(now);
        let width = (self.config.width as f32).min(bounds.w - 2.0 * space::LG);
        let card = Rect::new(
            (bounds.w - width) / 2.0,
            (bounds.h * 0.2).round(),
            width,
            SEARCH_HEIGHT + list_height,
        )
        .translate(0.0, (presence - 1.0) * space::MD);
        self.card = card;

        canvas.set_opacity(presence);
        canvas.shadow(card, radius::CARD, Shadow::FLOATING, Rgba::rgb(0, 0, 0));
        canvas.fill(card, radius::CARD, colors.surface);
        canvas.stroke(card, radius::CARD, stroke::HAIRLINE, colors.highlight_med);

        // Search field.
        let search = Rect::new(card.x, card.y, card.w, SEARCH_HEIGHT).inset_xy(space::LG, 0.0);
        let icon_width = if canvas.glyphs() {
            title + space::SM
        } else {
            space::LG
        };
        canvas.icon(
            icon::SEARCH,
            title + 2.0,
            colors.subtle,
            search,
            Align::Start,
        );
        let field = Rect::new(
            search.x + icon_width + space::XS,
            search.y,
            search.w - icon_width - space::XS,
            search.h,
        );
        let typed = if self.query.is_empty() {
            canvas.text_in(
                &Text::new("Search apps", title, colors.muted),
                field,
                Align::Start,
            );
            0.0
        } else {
            canvas
                .text_in(
                    &Text::new(&self.query, title, colors.text),
                    field,
                    Align::Start,
                )
                .w
        };
        let caret = Rect::new(
            field.x + typed + 1.0,
            field.y + (field.h - title * 1.3) / 2.0,
            2.0,
            title * 1.3,
        );
        canvas.fill(caret, 1.0, accent);

        canvas.fill(
            Rect::new(
                card.x + space::LG,
                card.y + SEARCH_HEIGHT - 0.5,
                card.w - 2.0 * space::LG,
                stroke::HAIRLINE,
            ),
            0.0,
            colors.highlight_med,
        );

        // Results.
        let list = Rect::new(
            card.x + space::SM,
            card.y + SEARCH_HEIGHT + space::XS,
            card.w - 2.0 * space::SM,
            0.0,
        );
        self.rows.clear();
        if self.results.is_empty() {
            let row = list.row(0, ROW_HEIGHT, 0.0).inset_xy(space::SM, 0.0);
            canvas.text_in(
                &Text::new("No matches", base_size, colors.muted),
                row,
                Align::Start,
            );
        } else {
            let highlight = list
                .row(0, ROW_HEIGHT, 0.0)
                .translate(0.0, self.highlight.get(now) * ROW_HEIGHT);
            canvas.fill(highlight, radius::CONTROL, colors.overlay);

            let visible = self.visible_rows();
            for (row_index, app_index) in self
                .results
                .iter()
                .skip(self.offset)
                .take(visible)
                .enumerate()
            {
                let app = &self.apps[*app_index];
                let row = list.row(row_index, ROW_HEIGHT, 0.0);
                // While the card is still growing, rows below its edge wait their turn.
                if row.bottom() > card.bottom() {
                    break;
                }
                self.rows.push(row);
                let content = row.inset_xy(space::SM, 0.0);

                // A letter chip stands in for an icon: no icon themes needed.
                let tint = colors.get(chip_role(&app.name));
                let chip = Rect::new(content.x, content.y + (content.h - CHIP) / 2.0, CHIP, CHIP);
                canvas.fill(chip, radius::CONTROL, tint.alpha(opacity::TINT + 0.06));
                let initial: String = app
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().collect())
                    .unwrap_or_default();
                canvas.text_in(
                    &Text::new(&initial, base_size, tint).bold(),
                    chip,
                    Align::Center,
                );

                let text_x = chip.right() + space::MD;
                let available = content.right() - text_x;
                let name = Text::new(&app.name, base_size, colors.text)
                    .medium()
                    .max_width(available);
                let name_width = canvas
                    .text_in(
                        &name,
                        Rect::new(text_x, content.y, available, content.h),
                        Align::Start,
                    )
                    .w;
                if let Some(description) = &app.description {
                    let x = text_x + name_width + space::MD;
                    let room = content.right() - x;
                    if room > 60.0 {
                        let text =
                            Text::new(description, base_size * type_scale::SMALL, colors.muted);
                        canvas.text_in(
                            &text,
                            Rect::new(x, content.y, room, content.h),
                            Align::Start,
                        );
                    }
                }
            }
        }

        canvas.set_opacity(1.0);
        Paint::when(
            self.presence.is_animating(now)
                || self.highlight.is_animating(now)
                || self.list_height.is_animating(now),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use enso_core::Paths;
    use enso_ui::{Mods, Offscreen, TextEngine};

    use super::*;

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<LaunchConfig>("launch").unwrap(),
            LaunchConfig::default()
        );
    }

    fn app(name: &str, description: &str) -> App {
        App {
            id: name.to_lowercase().replace(' ', "-"),
            name: name.to_owned(),
            description: (!description.is_empty()).then(|| description.to_owned()),
            keywords: Vec::new(),
            command: vec!["true".to_owned()],
            terminal: false,
        }
    }

    fn launcher() -> Launcher {
        let apps = vec![
            app("Alacritty", "Terminal"),
            app("Calculator", ""),
            app("Files", "File Manager"),
            app("Firefox", "Web Browser"),
            app("Image Viewer", ""),
            app("Neovim", "Text Editor"),
            app("Settings", ""),
            app("Text Editor", ""),
            app("Videos", "Media Player"),
            app("Zathura", "Document Viewer"),
        ];
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-launch-test"));
        Launcher::new(
            ctx,
            LaunchConfig {
                max_results: 6,
                ..LaunchConfig::default()
            },
            apps,
        )
    }

    fn key(key: Key, ctrl: bool) -> KeyEvent {
        KeyEvent {
            key,
            mods: Mods {
                ctrl,
                ..Mods::default()
            },
            repeat: false,
        }
    }

    fn type_text(launcher: &mut Launcher, text: &str) {
        for c in text.chars() {
            assert!(launcher.edit(&key(Key::Char(c), false)));
        }
    }

    fn names(launcher: &Launcher) -> Vec<&str> {
        launcher
            .results
            .iter()
            .map(|i| launcher.apps[*i].name.as_str())
            .collect()
    }

    #[test]
    fn empty_query_lists_everything_alphabetically() {
        let launcher = launcher();
        assert_eq!(launcher.results.len(), 10);
        assert_eq!(names(&launcher)[..3], ["Alacritty", "Calculator", "Files"]);
        assert_eq!(launcher.selected_app().unwrap().name, "Alacritty");
    }

    #[test]
    fn typing_ranks_names_above_descriptions() {
        let mut launcher = launcher();
        type_text(&mut launcher, "fi");
        assert_eq!(names(&launcher)[..2], ["Files", "Firefox"]);

        // "Text Editor" by name beats Neovim, which only matches by description.
        launcher.edit(&key(Key::Char('u'), true));
        type_text(&mut launcher, "text ed");
        assert_eq!(names(&launcher), ["Text Editor", "Neovim"]);

        type_text(&mut launcher, "zzz");
        assert!(launcher.results.is_empty());
        assert!(launcher.selected_app().is_none());
    }

    #[test]
    fn editing_keys() {
        let mut launcher = launcher();
        type_text(&mut launcher, "web brow");
        launcher.edit(&key(Key::Char('w'), true));
        assert_eq!(launcher.query, "web ");
        launcher.edit(&key(Key::Backspace, false));
        assert_eq!(launcher.query, "web");
        launcher.edit(&key(Key::Char('u'), true));
        assert_eq!(launcher.query, "");
        // Shortcuts are not typed into the field.
        assert!(!launcher.edit(&key(Key::Char('x'), true)));
        assert!(!launcher.edit(&key(Key::Tab, false)));
    }

    #[test]
    fn selection_wraps_and_scrolls() {
        let mut launcher = launcher();
        assert_eq!(key(Key::Char('n'), true).nav(false), Some(Nav::Down));
        for _ in 0..6 {
            launcher.move_selection(Nav::Down);
        }
        assert_eq!((launcher.selected, launcher.offset), (6, 1));
        launcher.move_selection(Nav::Up);
        assert_eq!((launcher.selected, launcher.offset), (5, 1));
        launcher.select(0);
        launcher.move_selection(Nav::Up);
        assert_eq!((launcher.selected, launcher.offset), (9, 4));
        launcher.move_selection(Nav::Down);
        assert_eq!((launcher.selected, launcher.offset), (0, 0));
    }

    #[test]
    fn renders_card_and_rows() {
        let mut launcher = launcher();
        type_text(&mut launcher, "e");
        launcher.move_selection(Nav::Down);
        let settled = Instant::now() + Duration::from_secs(2);
        let mut text = TextEngine::new(&launcher.ctx.config.appearance);
        // Draw once to start the reveal, then again at rest.
        let frame = Offscreen::render(&mut text, (800, 520), 2, |canvas| {
            assert_eq!(launcher.draw(canvas, Instant::now()), Paint::Animating);
            canvas.clear(Rgba::TRANSPARENT);
            assert_eq!(launcher.draw(canvas, settled), Paint::Idle);
        });

        assert_eq!(launcher.rows.len(), 6);
        assert!(
            launcher
                .rows
                .iter()
                .all(|row| launcher.card.contains(row.x, row.bottom() - 1.0))
        );
        // The scrim tints everything outside the card.
        assert!(frame.pixel(10, 10)[3] > 0 && frame.pixel(10, 10)[3] < 255);
        let surface = launcher.ctx.theme.colors.surface;
        let (x, y) = (
            (launcher.card.x + 6.0) * 2.0,
            (launcher.card.y + 28.0) * 2.0,
        );
        assert_eq!(
            frame.pixel(x as u32, y as u32),
            [surface.r, surface.g, surface.b, 255]
        );
        frame
            .save_png(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/snapshots/launch.png"
            ))
            .unwrap();
    }
}
