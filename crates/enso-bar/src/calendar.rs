//! The month, hanging under the clock while the pointer rests there and
//! nothing is playing (`enso-bar --dropdown`).
//!
//! A card in the control center's family: the month and year with arrows
//! to step through months, the days of the week, and the days, today in
//! the accent. Scrolling turns the months too; a click on the month's name
//! comes back to this one. It never takes the keyboard, and goes when the
//! pointer has left both it and the clock — as what's playing does, this
//! is the same bar run again, told by the bar on standard input whether the
//! pointer is on the clock.

use std::{
    io::BufRead,
    thread,
    time::{Duration, Instant},
};

use enso_core::{
    Context,
    style::{edge, icon, space, type_scale},
};
use enso_ui::{
    Align, Anchor, App, Button, Canvas, Cx, Event, Keyboard, Layer, Paint, PointerEvent, Presence,
    Proxy, Rect, SurfaceSpec, Text, watch::watch_dir,
};
use jiff::civil::Date;

/// The name the card keeps to one copy by.
pub const NAME: &str = "enso-calendar";

const WIDTH: f32 = 300.0;
const PAD: f32 = space::LG;
/// The month's name and its arrows.
const HEADER: f32 = 36.0;
/// The days of the week.
const WEEKDAYS: f32 = 24.0;
/// One week's row; each day is a circle this wide.
const DAY: f32 = 36.0;
/// Room the surface keeps around the card for its shadow.
const SHADOW_ROOM: f32 = 28.0;
/// Between the bar and the card.
const DROP_GAP: f32 = edge::BELOW_BAR;
/// How long the card outlives the pointer leaving, so that crossing the
/// gap from the clock does not close it.
const GRACE: Duration = Duration::from_millis(300);
/// Scrolling that turns one month: a wheel's notch.
const NOTCH: f32 = 15.0;

#[derive(Debug)]
pub enum Message {
    /// From the bar: whether the pointer is on the clock.
    Owner(bool),
    Dismiss,
    ThemeChanged,
    /// Midnight passed: today moved.
    Tick,
}

/// Which screen edge the card hangs from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Back,
    On,
    Month,
}

#[derive(Debug)]
pub struct Calendar {
    ctx: Context,
    edge: Edge,
    today: Date,
    /// The first day of the month shown.
    month: Date,
    presence: Presence,
    pointer_inside: bool,
    owner_hovering: bool,
    leave_at: Option<Instant>,
    bounds: Rect,
    hits: Vec<(Rect, Hit)>,
    hovered: Option<Hit>,
    scrolled: f32,
}

impl Calendar {
    pub fn new(ctx: Context, edge: Edge, today: Date) -> Self {
        Self {
            presence: Presence::new(&ctx.motion),
            ctx,
            edge,
            today,
            month: today.first_of_month(),
            pointer_inside: false,
            owner_hovering: true,
            leave_at: None,
            bounds: Rect::new(0.0, 0.0, surface_width(), surface_height()),
            hits: Vec::new(),
            hovered: None,
            scrolled: 0.0,
        }
    }

    /// Step `months` forward (or back).
    fn turn(&mut self, months: i32) {
        if let Ok(month) = self.month.checked_add(jiff::Span::new().months(months)) {
            self.month = month.first_of_month();
        }
    }

    /// The days the grid shows, Monday first: from the Monday on or before
    /// the first of the month, as many whole weeks as it takes.
    fn days(&self) -> Vec<Date> {
        let first = self.month;
        let back = i64::from(first.weekday().to_monday_zero_offset());
        let start = first
            .checked_sub(jiff::Span::new().days(back))
            .unwrap_or(first);
        let shown = back + i64::from(first.days_in_month());
        let weeks = (shown + 6) / 7;
        (0..weeks * 7)
            .filter_map(|n| start.checked_add(jiff::Span::new().days(n)).ok())
            .collect()
    }

    fn weeks(&self) -> usize {
        self.days().len() / 7
    }

    fn card_height(weeks: usize) -> f32 {
        PAD + HEADER + WEEKDAYS + weeks as f32 * DAY + PAD
    }

    fn card_rect(&self) -> Rect {
        let height = Self::card_height(self.weeks());
        let x = ((self.bounds.w - WIDTH) / 2.0).round();
        let y = match self.edge {
            Edge::Top => DROP_GAP,
            Edge::Bottom => self.bounds.bottom() - DROP_GAP - height,
        };
        Rect::new(x, y, WIDTH, height)
    }

    /// The card stays while the pointer is on it or on the clock.
    fn wanted(&self) -> bool {
        self.pointer_inside || self.owner_hovering
    }

    fn watch_pointer(&mut self, cx: &mut Cx<'_>) {
        if self.presence.is_closing() {
            return;
        }
        if self.wanted() {
            self.leave_at = None;
        } else if self.leave_at.is_none() {
            self.leave_at = Some(Instant::now() + GRACE);
            cx.wake_in(GRACE);
        }
    }

    /// Input only on the card and the gap to the bar, so the rest of the
    /// surface neither blocks clicks nor counts as hovering.
    fn update_input(&self, cx: &mut Cx<'_>) {
        let card = self.card_rect();
        let region = match self.edge {
            Edge::Top => Rect::new(card.x, 0.0, card.w, card.bottom()),
            Edge::Bottom => Rect::new(card.x, card.y, card.w, self.bounds.h - card.y),
        };
        cx.set_input_region(Some(region));
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, hit)| *hit)
    }

    fn paint(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let presence = self.presence.get(now);
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let size = self.ctx.config.appearance.font_size;
        let small = size * type_scale::SMALL;
        self.bounds = canvas.bounds();
        self.hits.clear();

        let from = match self.edge {
            Edge::Top => -1.0,
            Edge::Bottom => 1.0,
        };
        let card = self
            .card_rect()
            .translate(0.0, from * (1.0 - presence) * space::MD);
        canvas.set_opacity(presence);
        canvas.card(card, colors.surface, colors.highlight_med);

        // The month and year, with an arrow either side of it to step.
        let inner = card.inset_xy(PAD, 0.0);
        let header = Rect::new(inner.x, card.y + PAD, inner.w, HEADER);
        let arrow = HEADER;
        let back = Rect::new(header.x, header.y, arrow, arrow);
        let on = Rect::new(header.right() - arrow, header.y, arrow, arrow);
        let title = Rect::new(back.right(), header.y, on.x - back.right(), HEADER);
        let name = self.month.strftime("%B %Y").to_string();
        let away = self.month != self.today.first_of_month();
        let title_color = if self.hovered == Some(Hit::Month) && away {
            accent
        } else {
            colors.text
        };
        canvas.text_in(
            &Text::new(&name, size, title_color).medium(),
            title,
            Align::Center,
        );
        for (rect, hit, figure) in [
            (back, Hit::Back, icon::CHEVRON_LEFT),
            (on, Hit::On, icon::CHEVRON_RIGHT),
        ] {
            if self.hovered == Some(hit) {
                canvas.pill(rect, colors.overlay);
            }
            canvas.icon(figure, small * 1.2, colors.subtle, rect, Align::Center);
            self.hits.push((rect, hit));
        }
        if away {
            self.hits.push((title, Hit::Month));
        }

        // Mon … Sun.
        let column = inner.w / 7.0;
        let names = Rect::new(inner.x, header.bottom(), inner.w, WEEKDAYS);
        for (i, day) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]
            .iter()
            .enumerate()
        {
            let cell = Rect::new(names.x + column * i as f32, names.y, column, WEEKDAYS);
            canvas.text_in(&Text::new(day, small, colors.muted), cell, Align::Center);
        }

        // The days, today in the accent; the neighbouring months' faint.
        let top = names.bottom();
        for (n, date) in self.days().into_iter().enumerate() {
            let (row, col) = (n / 7, n % 7);
            let cell = Rect::new(
                inner.x + column * col as f32,
                top + DAY * row as f32,
                column,
                DAY,
            );
            let (cx, cy) = cell.center();
            let label = date.day().to_string();
            let color = if date == self.today {
                let r = (DAY / 2.0 - 3.0).min(column / 2.0 - 1.0);
                canvas.circle(cx, cy, r, accent);
                colors.base
            } else if date.month() != self.month.month() {
                colors.muted.alpha(0.6)
            } else if col >= 5 {
                colors.subtle
            } else {
                colors.text
            };
            // Figures all one width, so the days stand in true columns.
            let text = if date == self.today {
                Text::new(&label, small, color).bold().tabular()
            } else {
                Text::new(&label, small, color).tabular()
            };
            canvas.text_in(&text, cell, Align::Center);
        }
        canvas.set_opacity(1.0);
        Paint::when(self.presence.is_animating(now))
    }
}

fn surface_width() -> f32 {
    WIDTH + 2.0 * (edge::SCREEN + SHADOW_ROOM)
}

fn surface_height() -> f32 {
    Calendar::card_height(6) + DROP_GAP + SHADOW_ROOM
}

impl App for Calendar {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec {
            width: surface_width().ceil() as u32,
            height: surface_height().ceil() as u32,
            keyboard: Keyboard::None,
            ..SurfaceSpec::new(
                NAME,
                Layer::Overlay,
                match self.edge {
                    Edge::Top => Anchor::TOP,
                    Edge::Bottom => Anchor::BOTTOM,
                },
            )
        }
    }

    fn start(&mut self, proxy: Proxy<Message>, cx: &mut Cx<'_>) {
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, {
            let proxy = proxy.clone();
            move || proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-bar: live theme switching unavailable: {err}");
        }
        if let Err(err) = enso_core::ipc::listen(NAME, {
            let proxy = proxy.clone();
            move |word| word != "close" || proxy.send(Message::Dismiss)
        }) {
            eprintln!("enso-bar: the calendar cannot be closed from outside: {err}");
        }
        // Today changes at midnight, even with the card open.
        let ticker = proxy.clone();
        let _ = thread::Builder::new()
            .name("enso-calendar-day".to_owned())
            .spawn(move || {
                loop {
                    thread::sleep(Duration::from_secs(60));
                    if !ticker.send(Message::Tick) {
                        return;
                    }
                }
            });
        self.owner_hovering = follow_owner(proxy);
        self.update_input(cx);
        self.watch_pointer(cx);
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let mut close = false;
        match event {
            Event::Message(Message::Owner(hovering)) => self.owner_hovering = hovering,
            Event::Message(Message::Dismiss) => close = true,
            Event::Message(Message::ThemeChanged) => {
                if self.ctx.reload_theme() {
                    cx.redraw();
                }
            }
            Event::Message(Message::Tick) => {
                let today = jiff::Zoned::now().date();
                if today != self.today {
                    self.today = today;
                    cx.redraw();
                }
            }
            Event::Wake if self.presence.is_closing() => return cx.exit(),
            Event::Wake => {
                close = self
                    .leave_at
                    .is_some_and(|leave_at| Instant::now() >= leave_at)
                    && !self.wanted();
            }
            Event::Resized { .. } => {
                let (width, height) = cx.size();
                self.bounds = Rect::new(0.0, 0.0, width as f32, height as f32);
                self.update_input(cx);
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Enter { x, y } | PointerEvent::Move { x, y }) => {
                self.pointer_inside = true;
                let hovered = self.hit(x, y);
                if hovered != self.hovered {
                    self.hovered = hovered;
                    cx.redraw();
                }
            }
            Event::Pointer(PointerEvent::Leave) => {
                self.pointer_inside = false;
                self.hovered = None;
                self.scrolled = 0.0;
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Scroll { dy, .. }) => {
                self.scrolled += dy;
                let months = (self.scrolled / NOTCH).trunc();
                if months != 0.0 {
                    self.scrolled -= months * NOTCH;
                    self.turn(months as i32);
                    self.update_input(cx);
                    cx.redraw();
                }
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                match self.hit(x, y) {
                    Some(Hit::Back) => self.turn(-1),
                    Some(Hit::On) => self.turn(1),
                    Some(Hit::Month) => self.month = self.today.first_of_month(),
                    None => {}
                }
                self.update_input(cx);
                cx.redraw();
            }
            _ => {}
        }
        if close {
            self.presence.close(cx);
            return;
        }
        self.watch_pointer(cx);
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        self.paint(canvas, now)
    }
}

/// Follow the bar on standard input: `enter` and `leave` as the pointer
/// comes and goes from the clock, `close` when it wants the card gone. The
/// bar going away counts as leaving. Returns whether anyone is listening.
fn follow_owner(proxy: Proxy<Message>) -> bool {
    thread::Builder::new()
        .name("enso-owner".to_owned())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let hovering = match line.as_deref().map(str::trim) {
                    Ok("enter") => true,
                    Ok("leave") => false,
                    Ok("close") => {
                        proxy.send(Message::Dismiss);
                        return;
                    }
                    Ok(_) => continue,
                    Err(_) => break,
                };
                if !proxy.send(Message::Owner(hovering)) {
                    return;
                }
            }
            proxy.send(Message::Owner(false));
        })
        .is_ok()
}

/// Run the card, unless one is up already.
pub fn run(edge: Edge) -> Result<(), String> {
    let _only = match enso_core::ipc::only(NAME) {
        enso_core::ipc::Only::Mine(guard) => guard,
        enso_core::ipc::Only::Taken => return Ok(()),
    };
    let ctx = Context::load();
    ctx.report_warnings("enso-bar");
    let appearance = ctx.config.appearance.clone();
    let today = jiff::Zoned::now().date();
    enso_ui::run(Calendar::new(ctx, edge, today), &appearance).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_ui::{Offscreen, TextEngine};

    use super::*;

    fn calendar(today: Date) -> Calendar {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-calendar-test"));
        Calendar::new(ctx, Edge::Top, today)
    }

    #[test]
    fn weeks_start_on_monday_and_hold_the_whole_month() {
        // November 2026 starts on a Sunday: six weeks, from 26 October.
        let cal = calendar(jiff::civil::date(2026, 11, 19));
        let days = cal.days();
        assert_eq!(days.len(), 42);
        assert_eq!(days[0], jiff::civil::date(2026, 10, 26));
        assert_eq!(days[6], jiff::civil::date(2026, 11, 1));
        assert_eq!(*days.last().unwrap(), jiff::civil::date(2026, 12, 6));
        // February 2027 starts on a Monday and fills four weeks exactly.
        let mut cal = calendar(jiff::civil::date(2027, 2, 10));
        assert_eq!(cal.weeks(), 4);
        cal.turn(-3);
        assert_eq!(cal.month, jiff::civil::date(2026, 11, 1));
        cal.turn(14);
        assert_eq!(cal.month, jiff::civil::date(2028, 1, 1));
    }

    #[test]
    fn renders_the_month_with_today_marked() {
        let mut cal = calendar(jiff::civil::date(2026, 11, 19));
        let mut text = TextEngine::new(&cal.ctx.config.appearance);
        let size = (surface_width() as u32, surface_height() as u32);
        let later = Instant::now() + Duration::from_secs(1);
        let frame = Offscreen::render(&mut text, size, 2, |canvas| {
            // The first frame starts it appearing; a second, later, has it whole.
            cal.paint(canvas, Instant::now());
            canvas.clear(enso_core::Rgba::TRANSPARENT);
            cal.paint(canvas, later);
        });
        let accent = cal.ctx.accent();
        let has_accent = (0..frame.width).any(|x| {
            (0..frame.height).any(|y| frame.pixel(x, y) == [accent.r, accent.g, accent.b, 255])
        });
        assert!(has_accent, "today is marked");
        frame.snapshot(env!("CARGO_MANIFEST_DIR"), "calendar");
    }
}
