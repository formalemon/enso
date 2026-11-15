//! The bar itself: three floating islands — workspaces, clock, status.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use enso_core::{
    Context, Rgba, Transition, Tween,
    style::{icon, space, type_scale},
};
use enso_sys::power::{Battery, Sysfs};
use enso_ui::{
    Align, Anchor, App, Button, Canvas, Cx, Event, Layer, Paint, PointerEvent, Proxy, Rect,
    SurfaceSpec, Text, watch::watch_dir,
};

use crate::{
    config::{BarConfig, DEFAULT_CLOCK, Position},
    niri::{self, Workspace},
};

/// Gap between the islands and the screen edges.
const EDGE: f32 = space::SM;
/// Vertical inset of the islands inside the bar.
const INSET: f32 = 4.0;
/// Diameter of an inactive workspace dot, and height of the active pill.
const DOT: f32 = 8.0;
/// Width of the active workspace pill.
const ACTIVE_WIDTH: f32 = 24.0;

/// Messages from background threads.
#[derive(Debug)]
pub enum Message {
    Workspaces(Vec<Workspace>),
    ThemeChanged,
}

#[derive(Debug)]
pub struct Bar {
    ctx: Context,
    config: BarConfig,
    sysfs: Sysfs,
    workspaces: Vec<Workspace>,
    /// How "active" each workspace looks (`0..=1`), animated, by workspace id.
    activity: HashMap<u64, Tween<f32>>,
    /// Click targets from the last frame.
    hits: Vec<(Rect, u64)>,
    /// Output this bar is on; workspaces of other outputs are hidden.
    output: Option<String>,
    clock: String,
    battery: Option<Battery>,
}

impl Bar {
    pub fn new(ctx: Context, config: BarConfig) -> Self {
        let mut bar = Self {
            ctx,
            config,
            sysfs: Sysfs::system(),
            workspaces: Vec::new(),
            activity: HashMap::new(),
            hits: Vec::new(),
            output: None,
            clock: String::new(),
            battery: None,
        };
        bar.refresh_status();
        bar
    }

    /// Re-read the clock and battery. Returns the delay until the next minute.
    fn refresh_status(&mut self) -> Duration {
        let now = jiff::Zoned::now();
        self.clock = jiff::fmt::strtime::format(&self.config.clock, &now)
            .or_else(|_| jiff::fmt::strtime::format(DEFAULT_CLOCK, &now))
            .unwrap_or_default();
        self.battery = self.sysfs.battery();
        let into_minute = Duration::new(
            now.second().unsigned_abs().into(),
            now.subsec_nanosecond().unsigned_abs(),
        );
        Duration::from_secs(60).saturating_sub(into_minute) + Duration::from_millis(20)
    }

    fn set_workspaces(&mut self, workspaces: Vec<Workspace>) {
        let now = Instant::now();
        let select = self.ctx.motion.apply(Transition::SELECT);
        for ws in &workspaces {
            let target = if ws.active { 1.0 } else { 0.0 };
            self.activity
                .entry(ws.id)
                // A workspace seen for the first time appears in its final state.
                .or_insert_with(|| Tween::new(target))
                .set(target, select, now);
        }
        self.activity
            .retain(|id, _| workspaces.iter().any(|ws| ws.id == *id));
        self.workspaces = workspaces;
    }

    /// Workspaces on this bar's output (all of them until the output is known).
    fn visible_workspaces(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces
            .iter()
            .filter(|ws| self.output.is_none() || ws.output.is_none() || ws.output == self.output)
    }

    fn draw_workspaces(&mut self, canvas: &mut Canvas<'_>, island: Rect, now: Instant) -> bool {
        let colors = &self.ctx.theme.colors;
        let accent = self.ctx.accent();
        let slots: Vec<(u64, f32, Rgba)> = self
            .visible_workspaces()
            .map(|ws| {
                let activity = self
                    .activity
                    .get(&ws.id)
                    .map_or(0.0, |tween| tween.get(now));
                let idle = match (ws.urgent, ws.occupied) {
                    (true, _) => colors.love,
                    (false, true) => colors.subtle,
                    (false, false) => colors.highlight_high,
                };
                (
                    ws.id,
                    DOT + (ACTIVE_WIDTH - DOT) * activity,
                    idle.mix(accent, activity),
                )
            })
            .collect();
        self.hits.clear();
        if slots.is_empty() {
            return false;
        }

        let content: f32 = slots.iter().map(|(_, width, _)| width).sum::<f32>()
            + space::SM * (slots.len() - 1) as f32;
        let island = Rect::new(island.x, island.y, content + 2.0 * space::MD, island.h);
        canvas.pill(island, colors.base);

        let mut x = island.x + space::MD;
        for (id, width, color) in slots {
            let dot = Rect::new(x, island.y + (island.h - DOT) / 2.0, width, DOT);
            canvas.pill(dot, color);
            // Generous click target: the full island height and half the gap either side.
            self.hits.push((
                Rect::new(x - space::XS, island.y, width + space::SM, island.h),
                id,
            ));
            x += width + space::SM;
        }
        self.activity.values().any(|tween| tween.is_animating(now))
    }

    fn draw_clock(&self, canvas: &mut Canvas<'_>, bar: Rect, size: f32) {
        let colors = &self.ctx.theme.colors;
        let text = Text::new(&self.clock, size, colors.text).medium();
        let width = canvas.measure(&text).w + 2.0 * space::LG;
        let island = bar.centered(width, bar.h);
        canvas.pill(island, colors.base);
        canvas.text_in(&text, island, Align::Center);
    }

    fn draw_status(&self, canvas: &mut Canvas<'_>, bar: Rect, size: f32) {
        let Some(battery) = &self.battery else { return };
        let colors = &self.ctx.theme.colors;
        let (icon, tint) = match (battery.is_plugged_in(), battery.percent) {
            (true, _) => (icon::BATTERY_CHARGING, colors.foam),
            (false, ..=15) => (icon::BATTERY, colors.love),
            (false, ..=30) => (icon::BATTERY, colors.gold),
            (false, _) => (icon::BATTERY, colors.subtle),
        };
        let label = format!("{}%", battery.percent);
        let text = Text::new(&label, size, colors.text).medium();
        let icon_width = if canvas.glyphs() {
            size + space::XS
        } else {
            size * 1.9
        };
        let width = icon_width + space::SM + canvas.measure(&text).w + 2.0 * space::MD;
        let island = Rect::new(bar.right() - width, bar.y, width, bar.h);
        canvas.pill(island, colors.base);

        let inner = island.inset_xy(space::MD, 0.0);
        canvas.icon(
            icon,
            size + 2.0,
            tint,
            Rect::new(inner.x, inner.y, icon_width, inner.h),
            Align::Start,
        );
        canvas.text_in(&text, inner, Align::End);
    }
}

impl App for Bar {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        let edge = match self.config.position {
            Position::Top => Anchor::TOP,
            Position::Bottom => Anchor::BOTTOM,
        };
        SurfaceSpec {
            height: self.config.height,
            exclusive_zone: self.config.height as i32,
            ..SurfaceSpec::new("enso-bar", Layer::Top, edge | Anchor::LEFT | Anchor::RIGHT)
        }
    }

    fn start(&mut self, proxy: Proxy<Message>, cx: &mut Cx<'_>) {
        let workspaces = proxy.clone();
        niri::watch(move |snapshot| workspaces.send(Message::Workspaces(snapshot)));

        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, move || {
            proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-bar: live theme switching unavailable: {err}");
        }
        cx.wake_in(self.refresh_status());
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        match event {
            Event::Message(Message::Workspaces(workspaces)) => {
                self.set_workspaces(workspaces);
                cx.redraw();
            }
            Event::Message(Message::ThemeChanged) => {
                if self.ctx.reload_theme() {
                    cx.redraw();
                }
            }
            Event::Wake => {
                cx.wake_in(self.refresh_status());
                cx.redraw();
            }
            Event::Output(name) => {
                self.output = Some(name);
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                if let Some((_, id)) = self.hits.iter().find(|(rect, _)| rect.contains(x, y))
                    && let Err(err) = niri::focus_workspace(*id)
                {
                    eprintln!("enso-bar: cannot switch workspace: {err}");
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let bar = canvas.bounds().inset_xy(EDGE, INSET);
        let size = self.ctx.config.appearance.font_size * type_scale::SMALL;

        let animating = self.draw_workspaces(canvas, bar, now);
        self.draw_clock(canvas, bar, size);
        self.draw_status(canvas, bar, size);
        Paint::when(animating)
    }
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_sys::power::BatteryStatus;
    use enso_ui::{Offscreen, TextEngine};

    use super::*;

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<BarConfig>("bar").unwrap(),
            BarConfig::default()
        );
    }

    fn workspace(id: u64, active: bool, occupied: bool) -> Workspace {
        Workspace {
            id,
            index: id as u8,
            output: Some("eDP-1".into()),
            active,
            occupied,
            urgent: false,
        }
    }

    fn sample(theme: &str) -> Bar {
        // Each call gets its own directory so parallel tests never collide.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let paths = Paths::rooted(
            std::env::temp_dir().join(format!("enso-bar-test-{}-{unique}", std::process::id())),
        );
        enso_core::theme::set_active(&paths, theme).expect("write theme state");
        let mut bar = Bar::new(Context::load_from(paths), BarConfig::default());
        bar.clock = "Sun 15 Nov  22:46".into();
        bar.battery = Some(Battery {
            name: "BAT1".into(),
            percent: 82,
            status: BatteryStatus::Discharging,
        });
        bar.set_workspaces(vec![
            workspace(1, false, true),
            workspace(2, true, true),
            workspace(3, false, true),
            workspace(4, false, false),
        ]);
        bar
    }

    #[test]
    fn renders_islands_for_each_builtin_theme() {
        for theme in ["rose-pine-moon", "rose-pine-main", "rose-pine-dawn"] {
            let mut bar = sample(theme);
            let mut text = TextEngine::new(&bar.ctx.config.appearance);
            let frame = Offscreen::render(&mut text, (900, 36), 2, |canvas| {
                assert_eq!(bar.draw(canvas, Instant::now()), Paint::Idle);
            });

            let base = bar.ctx.theme.colors.base;
            // The bar is transparent between islands and opaque inside them.
            assert_eq!(
                frame.pixel(400, 36)[3],
                0,
                "gap between islands should be transparent"
            );
            assert_eq!(
                frame.pixel(900, 20),
                [base.r, base.g, base.b, 255],
                "clock island"
            );
            // One click target per workspace, left to right.
            assert_eq!(bar.hits.len(), 4);
            assert!(bar.hits.windows(2).all(|pair| pair[0].0.x < pair[1].0.x));
            // The active workspace's pill is wider than the idle dots.
            assert!(bar.hits[1].0.w > bar.hits[0].0.w);

            frame
                .save_png(format!(
                    "{}/../../target/snapshots/bar-{theme}.png",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .expect("write snapshot");
        }
    }

    #[test]
    fn workspace_switch_animates_then_settles() {
        let mut bar = sample("rose-pine-moon");
        bar.set_workspaces(vec![workspace(1, true, true), workspace(2, false, true)]);
        let mut text = TextEngine::new(&bar.ctx.config.appearance);
        let now = Instant::now();
        Offscreen::render(&mut text, (900, 36), 1, |canvas| {
            assert_eq!(bar.draw(canvas, now), Paint::Animating);
            assert_eq!(bar.draw(canvas, now + Duration::from_secs(1)), Paint::Idle);
        });
        assert_eq!(bar.hits.len(), 2);
    }

    #[test]
    fn only_shows_workspaces_of_its_own_output() {
        let mut bar = sample("rose-pine-moon");
        bar.output = Some("HDMI-A-1".into());
        assert_eq!(bar.visible_workspaces().count(), 0);
        bar.output = Some("eDP-1".into());
        assert_eq!(bar.visible_workspaces().count(), 4);
    }
}
