//! The daemon's interface: a column of toasts in the top-right corner.
//!
//! The surface exists only while something is showing; it is unmapped as
//! soon as the last toast has glided away.

use std::{
    io::{BufRead, BufReader},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use enso_core::{
    Context, Notices, Rgba,
    style::{Shadow, radius, space, stroke, type_scale},
};
use enso_ui::{
    Align, Anchor, App, Button, Canvas, Cx, Event, Key, Keyboard, Layer, Paint, PointerEvent,
    Proxy, Rect, SurfaceSpec, Text, watch::watch_dir,
};
use serde::Deserialize;

use crate::{
    center::{Center, Click, Look},
    dbus,
    stack::{CloseReason, DEFAULT_ACTION, Notification, Stack, StackOptions, Toast, Urgency},
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
    /// Notifications the history shows at once; the rest scroll.
    pub history_rows: usize,
    /// Notifications the history remembers.
    pub history_size: usize,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            width: 360,
            timeout_ms: 5000,
            max_visible: 5,
            gap: 10,
            history_rows: 5,
            history_size: 50,
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
/// Side of a notification's picture.
const IMAGE: f32 = 40.0;
/// Thickness of a level bar.
const PROGRESS: f32 = 4.0;
/// Height of an action button.
const BUTTON: f32 = 28.0;

/// An action button as last drawn, for the pointer.
#[derive(Clone, Debug, PartialEq)]
struct Hot {
    rect: Rect,
    id: u32,
    key: String,
}

#[derive(Debug)]
pub enum Message {
    /// The D-Bus service is up.
    Ready(zbus::Connection),
    /// The D-Bus service could not start; the daemon has no purpose without it.
    Failed(String),
    Notify(Notification),
    Close(u32),
    ThemeChanged,
    /// Asked for from outside: `enso-notify history`, which the bar runs.
    History(Ask),
}

/// What to do with the history card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Toggle,
    Open,
    Close,
}

impl Ask {
    /// The word for it on the command line and on the socket.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim() {
            "toggle" => Some(Self::Toggle),
            "open" => Some(Self::Open),
            "close" => Some(Self::Close),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Open => "open",
            Self::Close => "close",
        }
    }
}

/// The socket a running daemon listens on for [`Ask`]s.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("enso-notify.sock"))
}

/// Listen for asks on a background thread. Failing to only costs the
/// history its way of being opened from outside.
fn listen(proxy: Proxy<Message>) {
    let Some(path) = socket_path() else {
        return;
    };
    // A socket nobody answers on is left over from a daemon that died.
    if UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("enso-notify: cannot listen for `enso-notify history`: {err}");
            return;
        }
    };
    let served = std::thread::Builder::new()
        .name("enso-asks".to_owned())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                // Whoever connects says one word and hangs up; nobody gets
                // to hold the line.
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok()
                    && let Some(ask) = Ask::parse(&line)
                    && !proxy.send(Message::History(ask))
                {
                    return;
                }
            }
        });
    if let Err(err) = served {
        eprintln!("enso-notify: cannot listen for `enso-notify history`: {err}");
    }
}

#[derive(Debug)]
pub struct Daemon {
    ctx: Context,
    config: NotifyConfig,
    stack: Stack,
    connection: Option<zbus::Connection>,
    /// Raised when the daemon stops because of an error.
    failed: Arc<AtomicBool>,
    /// Where the pointer is, while it is over the toasts.
    pointer: Option<(f32, f32)>,
    /// Action buttons from the last frame.
    buttons: Vec<Hot>,
    /// The history card, which takes the toasts' place while it is open.
    center: Center,
    /// What the rest of the suite was last told.
    told: Option<Notices>,
}

impl Daemon {
    pub fn new(ctx: Context, config: NotifyConfig, failed: Arc<AtomicBool>) -> Self {
        let options = StackOptions {
            gap: config.gap as f32,
            max_visible: config.max_visible,
            default_timeout: Duration::from_millis(config.timeout_ms),
        };
        Self {
            center: Center::new(config.history_rows, config.history_size, ctx.motion),
            told: None,
            stack: Stack::new(options, ctx.motion),
            ctx,
            config,
            connection: None,
            failed,
            pointer: None,
            buttons: Vec::new(),
        }
    }

    fn size(&self) -> f32 {
        self.ctx.config.appearance.font_size
    }

    /// Width of the text column: the card's, less a picture if there is one.
    fn text_width(&self, note: &Notification) -> f32 {
        let picture = if note.image.is_some() {
            IMAGE + space::MD
        } else {
            0.0
        };
        self.config.width as f32 - 2.0 * PAD - picture
    }

    fn caption<'a>(&self, note: &'a Notification, color: Rgba) -> Text<'a> {
        Text::new(&note.app_name, self.size() * type_scale::SMALL, color)
            .max_width(self.text_width(note))
    }

    fn summary<'a>(&self, note: &'a Notification, color: Rgba) -> Text<'a> {
        Text::new(&note.summary, self.size(), color)
            .bold()
            .max_width(self.text_width(note))
    }

    fn body<'a>(&self, note: &'a Notification, color: Rgba) -> Text<'a> {
        Text::new(&note.body, self.size() * type_scale::SMALL, color)
            .max_width(self.text_width(note))
            .max_lines(BODY_LINES)
    }

    /// Height of the card for `note`, given the measured height of its body.
    fn card_height(&self, note: &Notification, body: f32) -> f32 {
        let any = Rgba::TRANSPARENT;
        let mut text = self.summary(note, any).line_height();
        if !note.app_name.is_empty() {
            text += self.caption(note, any).line_height() + space::XS / 2.0;
        }
        if !note.body.is_empty() {
            text += space::XS / 2.0 + body;
        }
        let mut height = 2.0 * PAD + self.content_height(note, text);
        if note.progress.is_some() {
            height += space::SM + PROGRESS;
        }
        if note.buttons().next().is_some() {
            height += space::MD + BUTTON;
        }
        height.ceil()
    }

    /// Height of the picture-and-text block: whichever is taller.
    fn content_height(&self, note: &Notification, text: f32) -> f32 {
        if note.image.is_some() {
            text.max(IMAGE)
        } else {
            text
        }
    }

    /// Height of the card for `note`, from its measured text.
    fn toast_height(&self, note: &Notification, cx: &mut Cx<'_>) -> f32 {
        let body = if note.body.is_empty() {
            0.0
        } else {
            cx.measure(&self.body(note, Rgba::TRANSPARENT)).h
        };
        self.card_height(note, body)
    }

    fn closed(&self, id: u32, reason: CloseReason) {
        if let Some(connection) = &self.connection {
            dbus::emit_closed(connection, id, reason);
        }
    }

    /// Bring the surface in line with what there is to show after any
    /// change: the history card if it is up, otherwise the toasts.
    fn sync(&mut self, cx: &mut Cx<'_>) {
        let now = Instant::now();
        let width = self.config.width + 2 * MARGIN as u32;
        if self.center.is_showing() {
            let height = (self.center.full_height() + 2.0 * MARGIN).ceil() as u32;
            cx.resize(width, height);
            // The surface is as tall as the card can get; only the card
            // itself takes the pointer.
            let card = self.center.card_at_rest(&self.look());
            cx.set_input_region(Some(card));
            cx.set_visible(true);
            cx.redraw();
        } else if self.stack.is_empty() {
            // An unmapped surface gets no leave event; do not stay paused.
            self.pointer = None;
            self.stack.resume(now);
            cx.set_visible(false);
        } else {
            let height = (self.stack.height(now) + 2.0 * MARGIN).ceil() as u32;
            cx.resize(width, height);
            cx.set_visible(true);
            cx.redraw();
        }
        let due = [
            self.stack.next_deadline(now),
            self.center.next_deadline(now),
        ]
        .into_iter()
        .flatten()
        .min();
        if let Some(delay) = due {
            cx.wake_in(delay);
        }
        self.tell();
    }

    /// How the history card is to look.
    fn look(&self) -> Look<'_> {
        Look {
            colors: &self.ctx.theme.colors,
            accent: self.ctx.accent(),
            size: self.size(),
            width: self.config.width as f32,
            origin: (MARGIN, MARGIN / 2.0),
        }
    }

    /// Keep the state file — the bar's bell — in step.
    fn tell(&mut self) {
        let notices = Notices {
            unseen: self.center.unseen(),
            open: self.center.is_open(),
        };
        if self.told != Some(notices) {
            self.told = Some(notices);
            if let Err(err) = notices.write(&self.ctx.paths) {
                eprintln!("enso-notify: cannot record the notification count: {err}");
            }
        }
    }

    /// Open the history in the toasts' place, or close it again.
    fn set_history(&mut self, open: bool, cx: &mut Cx<'_>) {
        let now = Instant::now();
        if open == self.center.is_open() {
            return;
        }
        if open {
            // Whatever was showing is in the history, where it is about to
            // be read.
            for id in self.stack.clear() {
                self.closed(id, CloseReason::Dismissed);
            }
            self.buttons.clear();
            self.pointer = None;
            self.center.open(now);
            // Focus on a click, never unasked: opening it from the bar
            // must not take the keyboard from what is being typed.
            cx.set_keyboard(Keyboard::OnDemand);
        } else {
            self.center.close(now);
        }
        self.sync(cx);
    }

    /// The history card has finished closing: back to being a toast column.
    fn history_gone(&mut self, cx: &mut Cx<'_>) {
        cx.set_input_region(None);
        cx.set_keyboard(Keyboard::None);
    }

    /// Tell the sender an action was chosen, then let the toast go.
    fn invoke(&mut self, id: u32, key: &str, cx: &mut Cx<'_>) {
        if let Some(connection) = &self.connection {
            dbus::emit_action(connection, id, key);
        }
        self.dismiss(id, cx);
    }

    fn dismiss(&mut self, id: u32, cx: &mut Cx<'_>) {
        if self.stack.close(id, Instant::now()) {
            self.closed(id, CloseReason::Dismissed);
            self.sync(cx);
        }
    }

    /// What a click at `(x, y)` lands on: an action button, or a toast.
    fn target(&self, x: f32, y: f32, now: Instant) -> Option<(u32, Option<&str>)> {
        if let Some(hot) = self.buttons.iter().find(|hot| hot.rect.contains(x, y)) {
            return Some((hot.id, Some(&hot.key)));
        }
        let inside = x >= MARGIN && x < MARGIN + self.config.width as f32;
        self.stack
            .hit(y - MARGIN / 2.0, now)
            .filter(|_| inside)
            .map(|id| (id, None))
    }

    /// Draw one toast; its action buttons are added to `buttons`.
    fn draw_toast(
        &self,
        canvas: &mut Canvas<'_>,
        toast: &Toast,
        now: Instant,
        buttons: &mut Vec<Hot>,
    ) {
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
        let top = card.y + PAD;
        let mut x = card.x + PAD;
        if let Some(image) = &note.image {
            canvas.image(image, Rect::new(x, top, IMAGE, IMAGE), radius::CONTROL);
            x += IMAGE + space::MD;
        }
        let width = self.text_width(note);
        let mut y = top;
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
                Rect::new(x + 12.0, y, width - 12.0, line),
                Align::Start,
            );
            y += line + space::XS / 2.0;
        }
        let summary = self.summary(note, colors.text);
        canvas.text(&summary, x, y);
        y += summary.line_height();
        if !note.body.is_empty() {
            y += space::XS / 2.0;
            y += canvas.text(&self.body(note, colors.subtle), x, y).h;
        }

        // Below the picture and the text, across the whole card.
        let inner = card.inset_xy(PAD, 0.0);
        let mut y = top + self.content_height(note, y - top);
        if let Some(percent) = note.progress {
            y += space::SM;
            let track = Rect::new(inner.x, y, inner.w, PROGRESS);
            canvas.pill(track, colors.highlight_med);
            let filled = inner.w * f32::from(percent.min(100)) / 100.0;
            if filled > 0.0 {
                let level = Rect::new(inner.x, y, filled.max(PROGRESS), PROGRESS);
                canvas.pill(level, self.ctx.accent());
            }
            y += PROGRESS;
        }
        let actions: Vec<_> = note.buttons().collect();
        if !actions.is_empty() {
            y += space::MD;
            let gap = space::SM;
            let each = (inner.w - gap * (actions.len() - 1) as f32) / actions.len() as f32;
            for (index, action) in actions.into_iter().enumerate() {
                let rect = Rect::new(inner.x + index as f32 * (each + gap), y, each, BUTTON);
                let hovered = self.pointer.is_some_and(|(px, py)| rect.contains(px, py));
                let fill = if hovered {
                    colors.highlight_med
                } else {
                    colors.overlay
                };
                canvas.fill(rect, radius::CONTROL, fill);
                let label = Text::new(&action.label, self.size() * type_scale::SMALL, colors.text)
                    .medium()
                    .max_width(each - 2.0 * space::SM);
                canvas.text_in(&label, rect, Align::Center);
                // A toast on its way out takes no more clicks.
                if !toast.is_leaving() {
                    buttons.push(Hot {
                        rect,
                        id: note.id,
                        key: action.key.clone(),
                    });
                }
            }
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
        listen(proxy.clone());
        self.tell();
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
                self.center.record(note.clone(), now);
                if self.center.is_showing() {
                    // It appears in the history, in front of the reader;
                    // a toast as well would say it twice.
                    self.closed(note.id, CloseReason::Dismissed);
                } else {
                    let height = self.toast_height(&note, cx);
                    self.stack.push(note, height, now);
                }
                self.sync(cx);
            }
            Event::Message(Message::Close(id)) => {
                // Withdrawn by its sender: no longer news.
                let forgotten = self.center.forget(id, now);
                let closed = self.stack.close(id, now);
                if closed {
                    self.closed(id, CloseReason::Closed);
                }
                if forgotten || closed {
                    self.sync(cx);
                }
            }
            Event::Message(Message::ThemeChanged) => {
                if self.ctx.reload_theme() {
                    cx.redraw();
                }
            }
            Event::Message(Message::History(ask)) => {
                let open = match ask {
                    Ask::Toggle => !self.center.is_open(),
                    Ask::Open => true,
                    Ask::Close => false,
                };
                self.set_history(open, cx);
            }
            Event::Wake => {
                for id in self.stack.tick(now) {
                    self.closed(id, CloseReason::Expired);
                    self.center.missed();
                }
                if self.center.tick(now) {
                    self.history_gone(cx);
                }
                self.sync(cx);
            }
            // While the history is up, input is the history's.
            Event::Key(key) if self.center.is_open() => {
                if let Some(nav) = key.nav(true) {
                    self.center.on_nav(nav, now);
                    cx.redraw();
                } else if matches!(key.key, Key::Escape | Key::Char('q')) {
                    self.set_history(false, cx);
                }
            }
            Event::Pointer(event) if self.center.is_showing() => match event {
                PointerEvent::Enter { x, y } | PointerEvent::Move { x, y } => {
                    self.center.on_pointer(x, y);
                    cx.redraw();
                }
                PointerEvent::Leave => {
                    self.center.on_leave(now);
                    self.sync(cx);
                }
                PointerEvent::Scroll { dy, .. } => {
                    if self.center.on_scroll(dy, now) {
                        cx.redraw();
                    }
                }
                PointerEvent::Press { x, y, .. } if self.center.is_open() => {
                    match self.center.on_press(x, y) {
                        Click::Remove(id) => {
                            self.center.forget(id, now);
                        }
                        Click::Clear => self.center.clear(now),
                        Click::Nothing => return,
                    }
                    self.sync(cx);
                }
                _ => {}
            },
            // The pointer on the toasts means they are being read: hold
            // them until it leaves.
            Event::Pointer(PointerEvent::Enter { x, y } | PointerEvent::Move { x, y }) => {
                self.pointer = Some((x, y));
                self.stack.pause(now);
                if !self.buttons.is_empty() {
                    cx.redraw();
                }
            }
            Event::Pointer(PointerEvent::Leave) => {
                self.pointer = None;
                self.stack.resume(now);
                self.sync(cx);
            }
            Event::Pointer(PointerEvent::Press { x, y, button }) => {
                let Some((id, key)) = self.target(x, y, now) else {
                    return;
                };
                let key = key.map(str::to_owned);
                let has_default = self
                    .stack
                    .get(id)
                    .is_some_and(Notification::has_default_action);
                match (button, key) {
                    (Button::Left, Some(key)) => self.invoke(id, &key, cx),
                    // Clicking the toast itself is "open it", if the sender
                    // offers that; otherwise it just goes away.
                    (Button::Left, None) if has_default => self.invoke(id, DEFAULT_ACTION, cx),
                    // Any other button dismisses without doing anything.
                    _ => self.dismiss(id, cx),
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        if self.center.is_showing() {
            let look = Look {
                colors: &self.ctx.theme.colors,
                accent: self.ctx.accent(),
                size: self.ctx.config.appearance.font_size,
                width: self.config.width as f32,
                origin: (MARGIN, MARGIN / 2.0),
            };
            return Paint::when(self.center.draw(canvas, &look, now));
        }
        let mut buttons = Vec::new();
        for toast in self.stack.toasts() {
            self.draw_toast(canvas, toast, now, &mut buttons);
        }
        self.buttons = buttons;
        Paint::when(self.stack.is_animating(now))
    }
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_ui::{Offscreen, TextEngine};

    use super::*;
    use crate::stack::{Action, Timeout};
    use enso_ui::Image;

    fn daemon() -> Daemon {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-notify-test"));
        Daemon::new(ctx, NotifyConfig::default(), Arc::default())
    }

    /// Push `notes` with the heights the daemon would give them (bodies
    /// taken as `lines` lines each) and draw the stack at rest.
    fn render(daemon: &mut Daemon, notes: Vec<(Notification, usize)>) -> Offscreen {
        let mut text = TextEngine::new(&daemon.ctx.config.appearance);
        let t0 = Instant::now();
        for (note, lines) in notes {
            let body = daemon.body(&note, Rgba::TRANSPARENT).line_height() * lines as f32;
            let height = daemon.card_height(&note, body);
            daemon.stack.push(note, height, t0);
        }
        let height = daemon.stack.height(t0) as u32 + 2 * MARGIN as u32;
        let settled = t0 + Duration::from_secs(1);
        Offscreen::render(&mut text, (392, height), 2, |canvas| {
            daemon.draw(canvas, settled);
        })
    }

    #[test]
    fn asks_have_words() {
        for ask in [Ask::Toggle, Ask::Open, Ask::Close] {
            assert_eq!(Ask::parse(ask.word()), Some(ask));
        }
        assert_eq!(Ask::parse(" open\n"), Some(Ask::Open));
        assert_eq!(Ask::parse("shout"), None);
    }

    #[test]
    fn renders_the_history() {
        let mut daemon = daemon();
        let t0 = Instant::now();
        let mut pixels = Vec::new();
        for _ in 0..8 {
            for x in 0..8 {
                pixels.extend(if x < 4 {
                    [235, 111, 146]
                } else {
                    [62, 143, 176]
                });
            }
        }
        let notes = [
            ("Calendar", "Stand-up in 10 minutes", "", 7200),
            (
                "Mail",
                "Invoice for November",
                "Hi, please find attached…",
                2400,
            ),
            (
                "Messages",
                "Mira",
                "Tea at four? I found the place with the matcha cake.",
                540,
            ),
            ("Battery", "Battery low — 9% remaining", "", 200),
            (
                "Screenshots",
                "Screenshot saved",
                "~/Pictures/2026-11-16.png",
                61,
            ),
            ("Music", "Nocturne in E-flat", "Chopin", 12),
            ("Updates", "14 packages can be upgraded", "", 0),
        ];
        for (id, (app, summary, body, age)) in notes.into_iter().enumerate() {
            let note = Notification {
                id: id as u32 + 1,
                app_name: app.into(),
                summary: summary.into(),
                body: body.into(),
                urgency: if app == "Battery" {
                    Urgency::Critical
                } else {
                    Urgency::Normal
                },
                image: (app == "Messages")
                    .then(|| Image::from_raw(8, 8, 24, false, &pixels))
                    .flatten(),
                ..Notification::default()
            };
            daemon.center.record(note, t0 - Duration::from_secs(age));
        }
        daemon.center.open(t0);

        let mut text = TextEngine::new(&daemon.ctx.config.appearance);
        let height = daemon.center.full_height() as u32 + 2 * MARGIN as u32;
        let snapshot = |name: &str, frame: &Offscreen| {
            frame
                .save_png(format!(
                    "{}/../../target/snapshots/{name}.png",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .unwrap();
        };
        let mut frame_at = |daemon: &mut Daemon, at: Instant| {
            let mut paint = Paint::Idle;
            let frame = Offscreen::render(&mut text, (392, height), 2, |canvas| {
                paint = daemon.draw(canvas, at);
            });
            (frame, paint)
        };

        // Mid-reveal the rows are still on their way in; then it rests.
        let (opening, paint) = frame_at(&mut daemon, t0 + Duration::from_millis(120));
        assert_eq!(paint, Paint::Animating);
        snapshot("notify-history-opening", &opening);
        let settled = t0 + Duration::from_secs(2);
        let (frame, paint) = frame_at(&mut daemon, settled);
        assert_eq!(paint, Paint::Idle);
        snapshot("notify-history", &frame);

        // Five rows of seven, newest first, and the way to clear them.
        let card = daemon.center.card_at_rest(&daemon.look());
        let rows = |daemon: &Daemon| -> Vec<Click> {
            (0..5)
                .map(|row| {
                    let y = card.y + 46.0 + 34.0 + row as f32 * 74.0;
                    daemon.center.on_press(card.x + 40.0, y)
                })
                .collect()
        };
        assert_eq!(rows(&daemon), [7, 6, 5, 4, 3].map(Click::Remove));
        assert_eq!(
            daemon.center.on_press(card.right() - 30.0, card.y + 23.0),
            Click::Clear
        );
        assert_eq!(
            daemon.center.on_press(card.x + 40.0, card.y + 23.0),
            Click::Nothing
        );

        // Scrolled to the end: the oldest five are in view, and nothing
        // spills past the card's edge — the list is clipped to it.
        daemon.center.on_scroll(1000.0, settled);
        let halfway = settled + Duration::from_millis(50);
        let (gliding, paint) = frame_at(&mut daemon, halfway);
        assert_eq!(paint, Paint::Animating);
        let surface = daemon.ctx.theme.colors.surface;
        let below = ((card.bottom() - 3.0) * 2.0) as u32;
        assert_eq!(
            gliding.pixel(200, below),
            [surface.r, surface.g, surface.b, 255]
        );
        let later = settled + Duration::from_secs(1);
        let (scrolled, _) = frame_at(&mut daemon, later);
        assert_eq!(rows(&daemon), [5, 4, 3, 2, 1].map(Click::Remove));
        snapshot("notify-history-scrolled", &scrolled);

        // Empty: a short card that says so.
        daemon.center.clear(later);
        let (empty, _) = frame_at(&mut daemon, later + Duration::from_secs(1));
        assert!(daemon.center.card_at_rest(&daemon.look()).h < card.h / 2.0);
        snapshot("notify-history-empty", &empty);
    }

    #[test]
    fn renders_pictures_levels_and_actions() {
        let mut daemon = daemon();
        // A small two-tone picture: left half rose, right half pine.
        let mut pixels = Vec::new();
        for _ in 0..8 {
            for x in 0..8 {
                pixels.extend(if x < 4 {
                    [235, 111, 146]
                } else {
                    [62, 143, 176]
                });
            }
        }
        let action = |key: &str, label: &str| Action {
            key: key.into(),
            label: label.into(),
        };
        let notes = vec![
            (
                Notification {
                    id: 1,
                    app_name: "Messages".into(),
                    summary: "Mira".into(),
                    body: "Tea at four? I found the place with the matcha cake.".into(),
                    image: Image::from_raw(8, 8, 24, false, &pixels),
                    actions: vec![
                        action("default", "Open"),
                        action("reply", "Reply"),
                        action("read", "Mark as read"),
                    ],
                    timeout: Timeout::Never,
                    ..Notification::default()
                },
                2,
            ),
            (
                Notification {
                    id: 2,
                    summary: "Volume".into(),
                    progress: Some(60),
                    timeout: Timeout::Never,
                    ..Notification::default()
                },
                0,
            ),
        ];
        let frame = render(&mut daemon, notes);

        // The picture is drawn where the text column would otherwise start,
        // each half in its own colour.
        let at = |x: f32, y: f32| frame.pixel((x * 2.0) as u32, (y * 2.0) as u32);
        let (left, top) = (MARGIN + PAD, MARGIN / 2.0 + PAD);
        assert_eq!(at(left + 10.0, top + 20.0), [235, 111, 146, 255]);
        assert_eq!(at(left + 30.0, top + 20.0), [62, 143, 176, 255]);

        // Two buttons, side by side inside the first card; "default" is the
        // toast itself and gets none.
        let keys: Vec<&str> = daemon.buttons.iter().map(|hot| hot.key.as_str()).collect();
        assert_eq!(keys, ["reply", "read"]);
        let (reply, read) = (daemon.buttons[0].rect, daemon.buttons[1].rect);
        assert_eq!(reply.y, read.y);
        assert!(reply.right() < read.x);
        let first = &daemon.stack.toasts()[0];
        assert!(reply.bottom() <= MARGIN / 2.0 + first.height - PAD + 0.5);

        // Clicks: a button, the toast around it, the gap, the margin.
        let now = Instant::now() + Duration::from_secs(1);
        let (bx, by) = reply.center();
        assert_eq!(daemon.target(bx, by, now), Some((1, Some("reply"))));
        assert_eq!(daemon.target(left, top, now), Some((1, None)));
        assert_eq!(daemon.target(2.0, top, now), None);
        let second_top = MARGIN / 2.0 + first.height + 10.0;
        assert_eq!(daemon.target(left, second_top - 5.0, now), None);
        assert_eq!(daemon.target(left, second_top + 5.0, now), Some((2, None)));

        // The level bar: accent to 60 %, track beyond.
        let second = &daemon.stack.toasts()[1];
        let bar_y = second_top + second.height - PAD - PROGRESS / 2.0;
        let inner = daemon.config.width as f32 - 2.0 * PAD;
        let accent = daemon.ctx.accent();
        let track = daemon.ctx.theme.colors.highlight_med;
        assert_eq!(
            at(left + inner * 0.3, bar_y),
            [accent.r, accent.g, accent.b, 255]
        );
        assert_eq!(
            at(left + inner * 0.8, bar_y),
            [track.r, track.g, track.b, 255]
        );
        frame
            .save_png(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/snapshots/notify-rich.png"
            ))
            .unwrap();
    }

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
