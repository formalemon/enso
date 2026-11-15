//! The control center panel: a floating card in the top-right corner.
//!
//! Today it shows a read-only power summary and a working theme picker.
//! Wi-Fi, Bluetooth, and audio sections will slot in above the themes as
//! their backends land in `enso-sys`.

use std::time::Instant;

use enso_core::{
    Context, Rgba, Theme, Transition, Tween,
    style::{Shadow, icon, opacity, radius, space, stroke, type_scale},
};
use enso_sys::power::Sysfs;
use enso_ui::{
    Align, App, Button, Canvas, Cx, Event, Key, Nav, Paint, PointerEvent, Presence, Proxy, Rect,
    SurfaceSpec, Text, watch::watch_dir,
};
use serde::Deserialize;

use crate::theme;

/// The `[ctl]` table of `config.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CtlConfig {
    /// Panel width in logical pixels.
    pub width: u32,
}

impl Default for CtlConfig {
    fn default() -> Self {
        Self { width: 340 }
    }
}

const MARGIN: f32 = space::SM;
const PAD: f32 = space::LG;
const TILE_HEIGHT: f32 = 60.0;
const ROW_HEIGHT: f32 = 40.0;
const LABEL_HEIGHT: f32 = 28.0;
const FOOTER_HEIGHT: f32 = 30.0;

#[derive(Debug)]
pub enum Message {
    ThemeChanged,
}

/// A small labelled reading in the summary row.
#[derive(Debug)]
struct Tile {
    icon: enso_core::Icon,
    value: String,
    label: &'static str,
}

#[derive(Debug)]
pub struct Panel {
    ctx: Context,
    config: CtlConfig,
    presence: Presence,
    tiles: Vec<Tile>,
    /// Index of the keyboard-selected theme row.
    selected: usize,
    /// Animated position of the selection highlight, in rows.
    highlight: Tween<f32>,
    /// Geometry from the last frame, for hit-testing.
    card: Rect,
    rows: Vec<Rect>,
}

impl Panel {
    pub fn new(ctx: Context, config: CtlConfig) -> Self {
        let selected = ctx
            .themes
            .iter()
            .position(|theme| theme.id == ctx.theme.id)
            .unwrap_or(0);
        Self {
            presence: Presence::new(&ctx.motion),
            tiles: tiles(&Sysfs::system()),
            selected,
            highlight: Tween::new(selected as f32),
            card: Rect::default(),
            rows: Vec::new(),
            ctx,
            config,
        }
    }

    fn themes(&self) -> Vec<&Theme> {
        self.ctx.themes.iter().collect()
    }

    fn select(&mut self, index: usize) {
        self.selected = index;
        self.highlight.set(
            index as f32,
            self.ctx.motion.apply(Transition::SELECT),
            Instant::now(),
        );
    }

    fn move_selection(&mut self, nav: Nav) {
        let count = self.ctx.themes.len();
        let next = match nav {
            Nav::Down => (self.selected + 1) % count,
            Nav::Up => (self.selected + count - 1) % count,
        };
        self.select(next);
    }

    /// Apply the selected theme everywhere and recolour the panel itself.
    fn apply_selected(&mut self) {
        let Some(theme) = self
            .themes()
            .get(self.selected)
            .map(|theme| (*theme).clone())
        else {
            return;
        };
        if let Err(err) = theme::apply(&self.ctx, &theme) {
            eprintln!("enso-ctl: {err}");
        }
        self.ctx.reload_theme();
    }

    fn card_height(&self) -> f32 {
        let tiles = if self.tiles.is_empty() {
            0.0
        } else {
            TILE_HEIGHT + space::LG
        };
        PAD + tiles
            + LABEL_HEIGHT
            + self.ctx.themes.len() as f32 * ROW_HEIGHT
            + space::SM
            + FOOTER_HEIGHT
            + PAD / 2.0
    }
}

/// Read the power summary once; the panel is short-lived.
fn tiles(sysfs: &Sysfs) -> Vec<Tile> {
    let mut tiles = Vec::new();
    if let Some(battery) = sysfs.battery() {
        tiles.push(Tile {
            icon: if battery.is_plugged_in() {
                icon::BATTERY_CHARGING
            } else {
                icon::BATTERY
            },
            value: format!("{}%", battery.percent),
            label: "Battery",
        });
    }
    if let Some(profile) = sysfs.platform_profile() {
        tiles.push(Tile {
            icon: icon::POWER_PROFILE,
            value: profile.current,
            label: "Profile",
        });
    }
    if let Some(temperature) = sysfs.cpu_temperature() {
        tiles.push(Tile {
            icon: icon::THERMOMETER,
            value: format!("{temperature:.0}°"),
            label: "CPU",
        });
    }
    tiles
}

impl App for Panel {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::overlay("enso-ctl")
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, move || {
            proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-ctl: live theme switching unavailable: {err}");
        }
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        match event {
            Event::Message(Message::ThemeChanged) => {
                self.ctx.reload_theme();
                cx.redraw();
            }
            Event::Wake if self.presence.is_closing() => cx.exit(),
            Event::Key(key) => {
                if let Some(nav) = key.nav(true) {
                    self.move_selection(nav);
                    cx.redraw();
                    return;
                }
                match key.key {
                    Key::Escape | Key::Char('q') => self.presence.close(cx),
                    Key::Enter | Key::Char(' ') => {
                        self.apply_selected();
                        cx.redraw();
                    }
                    _ => {}
                }
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                if let Some(index) = self.rows.iter().position(|row| row.contains(x, y)) {
                    self.select(index);
                    self.apply_selected();
                    cx.redraw();
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
        let small = base_size * type_scale::SMALL;

        let bounds = canvas.bounds();
        let width = (self.config.width as f32).min(bounds.w - 2.0 * MARGIN);
        // The card drops in from just above its resting place.
        let card = Rect::new(
            bounds.right() - width - MARGIN,
            MARGIN,
            width,
            self.card_height(),
        )
        .translate(0.0, (presence - 1.0) * space::MD);
        self.card = card;

        canvas.set_opacity(presence);
        canvas.shadow(card, radius::CARD, Shadow::FLOATING, Rgba::rgb(0, 0, 0));
        canvas.fill(card, radius::CARD, colors.surface);
        canvas.stroke(card, radius::CARD, stroke::HAIRLINE, colors.highlight_med);

        let inner = card.inset_xy(PAD, 0.0);
        let mut y = card.y + PAD;

        // Power summary.
        if !self.tiles.is_empty() {
            let gap = space::SM;
            let tile_width =
                (inner.w - gap * (self.tiles.len() - 1) as f32) / self.tiles.len() as f32;
            for (i, tile) in self.tiles.iter().enumerate() {
                let rect = Rect::new(
                    inner.x + i as f32 * (tile_width + gap),
                    y,
                    tile_width,
                    TILE_HEIGHT,
                );
                canvas.fill(rect, radius::CONTROL, colors.overlay);
                let content = rect.inset_xy(space::MD, space::SM);
                let top = Rect::new(content.x, content.y, content.w, content.h / 2.0);
                let bottom = top.translate(0.0, content.h / 2.0);
                if canvas.glyphs() {
                    canvas.icon(tile.icon, base_size, accent, top, Align::End);
                }
                canvas.text_in(
                    &Text::new(&tile.value, base_size, colors.text).bold(),
                    top,
                    Align::Start,
                );
                canvas.text_in(
                    &Text::new(tile.label, small, colors.subtle),
                    bottom,
                    Align::Start,
                );
            }
            y += TILE_HEIGHT + space::LG;
        }

        // Theme picker.
        let label = Rect::new(inner.x, y, inner.w, LABEL_HEIGHT);
        canvas.text_in(
            &Text::new("Theme", small, colors.subtle).medium(),
            label,
            Align::Start,
        );
        y += LABEL_HEIGHT;

        let list = Rect::new(card.x + space::SM, y, card.w - 2.0 * space::SM, 0.0);
        let highlight = list
            .row(0, ROW_HEIGHT, 0.0)
            .translate(0.0, self.highlight.get(now) * ROW_HEIGHT);
        canvas.fill(highlight, radius::CONTROL, colors.overlay);

        self.rows.clear();
        let themes: Vec<&Theme> = self.ctx.themes.iter().collect();
        for (i, theme) in themes.iter().enumerate() {
            let row = list.row(i, ROW_HEIGHT, 0.0);
            self.rows.push(row);
            let content = row.inset_xy(space::SM, 0.0);

            // Each theme previews itself with its own colours.
            let (_, cy) = content.center();
            let swatches = [
                theme.colors.base,
                theme.colors.rose,
                theme.colors.foam,
                theme.colors.iris,
            ];
            for (j, swatch) in swatches.into_iter().enumerate() {
                let cx = content.x + 7.0 + j as f32 * 11.0;
                canvas.circle(cx, cy, 7.0, colors.highlight_high);
                canvas.circle(cx, cy, 6.0, swatch);
            }

            let active = theme.id == self.ctx.theme.id;
            let name = Text::new(&theme.name, base_size, colors.text);
            let name = if active { name.medium() } else { name };
            let name_rect = Rect::new(
                content.x + 60.0,
                content.y,
                content.w - 60.0 - 28.0,
                content.h,
            );
            canvas.text_in(&name, name_rect, Align::Start);
            if active {
                canvas.icon(icon::CHECK, base_size + 2.0, accent, content, Align::End);
            }
        }
        y += themes.len() as f32 * ROW_HEIGHT + space::SM;

        // What is coming next, kept quiet.
        let footer = Rect::new(inner.x, y, inner.w, FOOTER_HEIGHT);
        canvas.text_in(
            &Text::new(
                "Wi-Fi · Bluetooth · Audio — coming soon",
                small,
                colors.muted.alpha(1.0 - opacity::DISABLED / 2.0),
            ),
            footer,
            Align::Start,
        );

        canvas.set_opacity(1.0);
        Paint::when(self.presence.is_animating(now) || self.highlight.is_animating(now))
    }
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_ui::{KeyEvent, Mods, Offscreen, TextEngine};

    use super::*;

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<CtlConfig>("ctl").unwrap(),
            CtlConfig::default()
        );
    }

    fn panel(name: &str) -> Panel {
        let root =
            std::env::temp_dir().join(format!("enso-ctl-panel-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut panel = Panel::new(
            Context::load_from(Paths::rooted(root)),
            CtlConfig::default(),
        );
        panel.tiles = vec![
            Tile {
                icon: icon::BATTERY,
                value: "82%".into(),
                label: "Battery",
            },
            Tile {
                icon: icon::POWER_PROFILE,
                value: "balanced".into(),
                label: "Profile",
            },
            Tile {
                icon: icon::THERMOMETER,
                value: "54°".into(),
                label: "CPU",
            },
        ];
        panel
    }

    fn settled() -> Instant {
        Instant::now() + std::time::Duration::from_secs(2)
    }

    #[test]
    fn renders_and_tracks_row_geometry() {
        let mut panel = panel("render");
        let mut text = TextEngine::new(&panel.ctx.config.appearance);
        let frame = Offscreen::render(&mut text, (420, 360), 2, |canvas| {
            // The first frame starts the reveal; a later one is at rest.
            assert_eq!(panel.draw(canvas, Instant::now()), Paint::Animating);
        });
        assert_eq!(
            frame.pixel(4, 700)[3],
            0,
            "outside the card stays transparent"
        );

        let frame = Offscreen::render(&mut text, (420, 360), 2, |canvas| {
            assert_eq!(panel.draw(canvas, settled()), Paint::Idle);
        });
        assert_eq!(panel.rows.len(), 3);
        assert!(
            panel
                .card
                .contains(panel.rows[2].x, panel.rows[2].bottom() - 1.0)
        );
        let surface = panel.ctx.theme.colors.surface;
        let (cx, cy) = ((panel.card.x + 4.0) * 2.0, (panel.card.y + 40.0) * 2.0);
        assert_eq!(
            frame.pixel(cx as u32, cy as u32),
            [surface.r, surface.g, surface.b, 255]
        );
        frame
            .save_png(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/snapshots/ctl-panel.png"
            ))
            .unwrap();
    }

    #[test]
    fn keyboard_moves_wraps_and_applies() {
        let mut panel = panel("keys");
        // Built-ins sort as dawn, main, moon; moon is active, so selected.
        assert_eq!(panel.selected, 2);
        panel.move_selection(Nav::Down);
        assert_eq!(panel.selected, 0);
        panel.move_selection(Nav::Up);
        assert_eq!(panel.selected, 2);

        let j = KeyEvent {
            key: Key::Char('j'),
            mods: Mods::default(),
            repeat: false,
        };
        assert_eq!(j.nav(true), Some(Nav::Down));
        panel.move_selection(Nav::Down);
        panel.apply_selected();
        assert_eq!(panel.ctx.theme.id, "rose-pine-dawn");
        assert_eq!(
            enso_core::theme::active(&panel.ctx.paths).as_deref(),
            Some("rose-pine-dawn")
        );
    }
}
