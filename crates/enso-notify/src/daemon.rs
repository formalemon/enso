//! The daemon's interface: a column of toasts in the top-right corner.
//!
//! The surface exists only while something is showing; it is unmapped as
//! soon as the last toast has glided away.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use enso_core::{
    Context, Rgba,
    style::{Shadow, radius, space, stroke, type_scale},
};
use enso_ui::{
    Align, Anchor, App, Button, Canvas, Cx, Event, Layer, Paint, PointerEvent, Proxy, Rect,
    SurfaceSpec, Text, watch::watch_dir,
};
use serde::Deserialize;

use crate::{
    dbus,
    stack::{CloseReason, Notification, Stack, StackOptions, Toast, Urgency},
};

/// The `[notify]` table of `config.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotifyConfig {
    /// Toast width in logical pixels.
    pub width: u32,
    /// How long a notification stays when it does not say, in milliseconds.
    pub timeout_ms: u64,
    /// Toasts shown at once; more wait in a queue.
    pub max_visible: usize,
    /// Space between toasts.
    pub gap: u32,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            width: 360,
            timeout_ms: 5000,
            max_visible: 5,
            gap: 10,
        }
    }
}

/// Room around the column for shadows, and the gap to the screen edge.
const MARGIN: f32 = space::LG;
/// Padding inside a toast.
const PAD: f32 = 14.0;
const BODY_LINES: usize = 3;
/// How far a toast travels while entering (from the right) or leaving (upward).
const TRAVEL: f32 = 28.0;

#[derive(Debug)]
pub enum Message {
    /// The D-Bus service is up.
    Ready(zbus::Connection),
    /// The D-Bus service could not start; the daemon has no purpose without it.
    Failed(String),
    Notify(Notification),
    Close(u32),
    ThemeChanged,
}

#[derive(Debug)]
pub struct Daemon {
    ctx: Context,
    config: NotifyConfig,
    stack: Stack,
    connection: Option<zbus::Connection>,
    /// Raised when the daemon stops because of an error.
    failed: Arc<AtomicBool>,
}

impl Daemon {
    pub fn new(ctx: Context, config: NotifyConfig, failed: Arc<AtomicBool>) -> Self {
        let options = StackOptions {
            gap: config.gap as f32,
            max_visible: config.max_visible,
            default_timeout: Duration::from_millis(config.timeout_ms),
        };
        Self {
            stack: Stack::new(options, ctx.motion),
            ctx,
            config,
            connection: None,
            failed,
        }
    }

    fn size(&self) -> f32 {
        self.ctx.config.appearance.font_size
    }

    fn text_width(&self) -> f32 {
        self.config.width as f32 - 2.0 * PAD
    }

    fn caption<'a>(&self, note: &'a Notification, color: Rgba) -> Text<'a> {
        Text::new(&note.app_name, self.size() * type_scale::SMALL, color)
            .max_width(self.text_width())
    }

    fn summary<'a>(&self, note: &'a Notification, color: Rgba) -> Text<'a> {
        Text::new(&note.summary, self.size(), color)
            .bold()
            .max_width(self.text_width())
    }

    fn body<'a>(&self, note: &'a Notification, color: Rgba) -> Text<'a> {
        Text::new(&note.body, self.size() * type_scale::SMALL, color)
            .max_width(self.text_width())
            .max_lines(BODY_LINES)
    }

    /// Height of the card for `note`, from its measured text.
    fn toast_height(&self, note: &Notification, cx: &mut Cx<'_>) -> f32 {
        let any = Rgba::TRANSPARENT;
        let mut height = 2.0 * PAD;
        if !note.app_name.is_empty() {
            height += self.caption(note, any).line_height() + space::XS / 2.0;
        }
        height += self.summary(note, any).line_height();
        if !note.body.is_empty() {
            height += space::XS / 2.0 + cx.measure(&self.body(note, any)).h;
        }
        height.ceil()
    }

    fn closed(&self, id: u32, reason: CloseReason) {
        if let Some(connection) = &self.connection {
            dbus::emit_closed(connection, id, reason);
        }
    }

    /// Bring the surface in line with the stack after any change.
    fn sync(&mut self, cx: &mut Cx<'_>) {
        let now = Instant::now();
        if self.stack.is_empty() {
            cx.set_visible(false);
            return;
        }
        let width = self.config.width + 2 * MARGIN as u32;
        let height = (self.stack.height(now) + 2.0 * MARGIN).ceil() as u32;
        cx.resize(width, height);
        cx.set_visible(true);
        cx.redraw();
        if let Some(delay) = self.stack.next_deadline(now) {
            cx.wake_in(delay);
        }
    }

    fn draw_toast(&self, canvas: &mut Canvas<'_>, toast: &Toast, now: Instant) {
        let colors = &self.ctx.theme.colors;
        let presence = toast.presence(now);
        // Arrive from the side; leave upward, toward the bar.
        let (dx, dy) = if toast.is_leaving() {
            (0.0, (presence - 1.0) * TRAVEL)
        } else {
            ((1.0 - presence) * TRAVEL, 0.0)
        };
        let card = Rect::new(
            MARGIN + dx,
            MARGIN / 2.0 + toast.y(now) + dy,
            self.config.width as f32,
            toast.height,
        );

        canvas.set_opacity(presence);
        canvas.shadow(card, radius::CARD, Shadow::FLOATING, Rgba::rgb(0, 0, 0));
        canvas.fill(card, radius::CARD, colors.surface);
        let outline = match toast.note.urgency {
            Urgency::Critical => colors.love,
            Urgency::Low | Urgency::Normal => colors.highlight_med,
        };
        canvas.stroke(card, radius::CARD, stroke::HAIRLINE, outline);

        let note = &toast.note;
        let (x, mut y) = (card.x + PAD, card.y + PAD);
        if !note.app_name.is_empty() {
            let caption = self.caption(note, colors.subtle);
            // A small accent dot marks where the notification came from.
            let dot = match note.urgency {
                Urgency::Critical => colors.love,
                Urgency::Normal => self.ctx.accent(),
                Urgency::Low => colors.muted,
            };
            let line = caption.line_height();
            canvas.circle(x + 3.0, y + line / 2.0, 3.0, dot);
            canvas.text_in(
                &caption,
                Rect::new(x + 12.0, y, self.text_width() - 12.0, line),
                Align::Start,
            );
            y += line + space::XS / 2.0;
        }
        let summary = self.summary(note, colors.text);
        canvas.text(&summary, x, y);
        y += summary.line_height() + space::XS / 2.0;
        if !note.body.is_empty() {
            canvas.text(&self.body(note, colors.subtle), x, y);
        }
        canvas.set_opacity(1.0);
    }
}

impl App for Daemon {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec {
            width: self.config.width + 2 * MARGIN as u32,
            height: 1,
            // Nothing to show until the first notification arrives.
            visible: false,
            ..SurfaceSpec::new("enso-notify", Layer::Overlay, Anchor::TOP | Anchor::RIGHT)
        }
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        dbus::serve(proxy.clone());
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, move || {
            proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-notify: live theme switching unavailable: {err}");
        }
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let now = Instant::now();
        match event {
            Event::Message(Message::Ready(connection)) => self.connection = Some(connection),
            Event::Message(Message::Failed(reason)) => {
                eprintln!("enso-notify: {reason}");
                self.failed.store(true, Ordering::Relaxed);
                cx.exit();
            }
            Event::Message(Message::Notify(note)) => {
                let height = self.toast_height(&note, cx);
                self.stack.push(note, height, now);
                self.sync(cx);
            }
            Event::Message(Message::Close(id)) => {
                if self.stack.close(id, now) {
                    self.closed(id, CloseReason::Closed);
                    self.sync(cx);
                }
            }
            Event::Message(Message::ThemeChanged) => {
                if self.ctx.reload_theme() {
                    cx.redraw();
                }
            }
            Event::Wake => {
                for id in self.stack.tick(now) {
                    self.closed(id, CloseReason::Expired);
                }
                self.sync(cx);
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                let inside = x >= MARGIN && x < MARGIN + self.config.width as f32;
                if let Some(id) = self.stack.hit(y - MARGIN / 2.0, now).filter(|_| inside)
                    && self.stack.close(id, now)
                {
                    self.closed(id, CloseReason::Dismissed);
                    self.sync(cx);
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        for toast in self.stack.toasts() {
            self.draw_toast(canvas, toast, now);
        }
        Paint::when(self.stack.is_animating(now))
    }
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_ui::{Offscreen, TextEngine};

    use super::*;
    use crate::stack::Timeout;

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<NotifyConfig>("notify").unwrap(),
            NotifyConfig::default()
        );
    }

    #[test]
    fn renders_a_stack_of_toasts() {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-notify-test"));
        let mut daemon = Daemon::new(ctx, NotifyConfig::default(), Arc::default());
        let mut text = TextEngine::new(&daemon.ctx.config.appearance);
        let t0 = Instant::now();

        let notes = [
            Notification {
                id: 1,
                app_name: "Messages".into(),
                summary: "Mira".into(),
                body: "Tea at four? I found the place with the matcha cake you liked.".into(),
                ..Notification::default()
            },
            Notification {
                id: 2,
                app_name: "Battery".into(),
                summary: "Battery low — 9% remaining".into(),
                urgency: Urgency::Critical,
                ..Notification::default()
            },
            Notification {
                id: 3,
                summary: "Screenshot saved".into(),
                timeout: Timeout::Never,
                ..Notification::default()
            },
        ];
        // Heights as the daemon would measure them, without a live surface.
        for (note, height) in notes.into_iter().zip([104.0, 68.0, 48.0]) {
            daemon.stack.push(note, height, t0);
        }

        let height = daemon.stack.height(t0) as u32 + 2 * MARGIN as u32;
        let settled = t0 + Duration::from_secs(1);
        let frame = Offscreen::render(&mut text, (392, height), 2, |canvas| {
            assert_eq!(daemon.draw(canvas, t0), Paint::Animating);
            canvas.clear(Rgba::TRANSPARENT);
            assert_eq!(daemon.draw(canvas, settled), Paint::Idle);
        });

        let surface = daemon.ctx.theme.colors.surface;
        let inside = frame.pixel((MARGIN as u32 + 6) * 2, (MARGIN as u32 / 2 + 50) * 2);
        assert_eq!(inside, [surface.r, surface.g, surface.b, 255]);
        frame
            .save_png(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/snapshots/notify.png"
            ))
            .unwrap();
    }
}
