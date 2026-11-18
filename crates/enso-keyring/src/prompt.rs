//! The keyring's card: one password field over the dimmed screen, in the
//! same shape as enso-auth's. It sends what's typed to the keyring and
//! waits; the keyring closes it, or says why not and lets it try again.

use std::{
    f32::consts::PI,
    io::{self, BufRead, Write},
    thread,
    time::Instant,
};

use enso_core::{
    Context, Easing, Transition, Tween,
    style::{icon, opacity, radius, space, stroke, type_scale},
};
use enso_ui::{
    Align, App, Canvas, Cx, Event, Key, KeyEvent, Paint, PointerEvent, Presence, Proxy, Rect,
    SurfaceSpec, Text,
    widget::{Button, Interaction, Kind, Look, controls},
};
use zeroize::Zeroizing;

use crate::{
    asking::Mode,
    crypto::{hex, unhex},
};

const WIDTH: f32 = 440.0;
const BADGE: f32 = 40.0;
const FIELD: f32 = 44.0;
const DOT: f32 = 8.0;
const MAX_DOTS: usize = 20;
const MAX_PASSWORD: usize = 512;
const SHAKE: Transition = Transition::new(380, Easing::Linear);

#[derive(Debug)]
pub enum Message {
    /// The keyring took it: close.
    Done,
    /// Try again: in this mode, with these words.
    Again(Mode, String),
    /// The keyring went away.
    Gone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Unlock,
    Cancel,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Ignored,
    Redraw,
    Close,
}

pub struct Card {
    ctx: Context,
    mode: Mode,
    asker: String,
    out: Option<Box<dyn Write>>,
    presence: Presence,
    typed: Zeroizing<String>,
    dots: Tween<f32>,
    /// Sent, waiting for the keyring's answer.
    waiting: bool,
    leaving: bool,
    complaint: Option<String>,
    shake: Tween<f32>,
    caps_lock: bool,
    hits: Vec<(Rect, Hit)>,
    hover: Option<Hit>,
}

impl std::fmt::Debug for Card {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Card")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl Card {
    pub fn new(ctx: Context, mode: Mode, asker: String, out: Option<Box<dyn Write>>) -> Self {
        Self {
            presence: Presence::new(&ctx.motion),
            ctx,
            mode,
            asker,
            out,
            typed: Zeroizing::new(String::with_capacity(MAX_PASSWORD * 4)),
            dots: Tween::new(0.0),
            waiting: false,
            leaving: false,
            complaint: None,
            shake: Tween::new(1.0),
            caps_lock: false,
            hits: Vec::new(),
            hover: None,
        }
    }

    fn font(&self, scale: f32) -> f32 {
        (self.ctx.config.appearance.font_size * scale).round()
    }

    fn say(&mut self, line: &str) {
        if let Some(out) = &mut self.out {
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    }

    fn set_dots(&mut self, now: Instant) {
        let count = self.typed.chars().count().min(MAX_DOTS) as f32;
        self.dots
            .set(count, self.ctx.motion.apply(Transition::HOVER), now);
    }

    fn submit(&mut self, now: Instant) -> Outcome {
        if self.waiting || self.typed.is_empty() {
            return Outcome::Ignored;
        }
        let line = Zeroizing::new(format!("password {}", hex(self.typed.as_bytes())));
        self.say(&line);
        self.typed = Zeroizing::new(String::with_capacity(MAX_PASSWORD * 4));
        self.set_dots(now);
        self.waiting = true;
        self.complaint = None;
        Outcome::Redraw
    }

    fn cancel(&mut self) -> Outcome {
        if self.leaving {
            return Outcome::Ignored;
        }
        self.say("cancel");
        self.leaving = true;
        Outcome::Close
    }

    fn message(&mut self, message: Message, now: Instant) -> Outcome {
        match message {
            Message::Done => {
                self.leaving = true;
                Outcome::Close
            }
            Message::Gone => self.cancel(),
            Message::Again(mode, words) => {
                self.waiting = false;
                let changed = mode != self.mode;
                self.mode = mode;
                // A new question starts clean; the same one says what was wrong.
                self.complaint = (!words.is_empty()).then_some(words);
                // A new question isn't a wrong answer.
                if !changed {
                    self.shake.jump(0.0);
                    self.shake.set(1.0, self.ctx.motion.apply(SHAKE), now);
                }
                Outcome::Redraw
            }
        }
    }

    fn key(&mut self, key: &KeyEvent, now: Instant) -> Outcome {
        let caps = std::mem::replace(&mut self.caps_lock, key.mods.caps_lock) != key.mods.caps_lock;
        let quiet = if caps {
            Outcome::Redraw
        } else {
            Outcome::Ignored
        };
        match key.key {
            Key::Escape => return self.cancel(),
            Key::Enter => return self.submit(now),
            _ if self.waiting => return quiet,
            Key::Char('u') if key.mods.ctrl => self.typed.clear(),
            Key::Backspace => {
                if self.typed.pop().is_none() {
                    return quiet;
                }
            }
            _ => match key.text() {
                Some(c) if self.typed.chars().count() < MAX_PASSWORD => self.typed.push(c),
                _ => return quiet,
            },
        }
        self.complaint = None;
        self.set_dots(now);
        Outcome::Redraw
    }

    fn pointer(&mut self, event: PointerEvent, now: Instant) -> Outcome {
        match event {
            PointerEvent::Enter { x, y } | PointerEvent::Move { x, y } => {
                let hover = self
                    .hits
                    .iter()
                    .rev()
                    .find(|(rect, _)| rect.contains(x, y))
                    .map(|(_, hit)| *hit);
                if std::mem::replace(&mut self.hover, hover) == hover {
                    Outcome::Ignored
                } else {
                    Outcome::Redraw
                }
            }
            PointerEvent::Press { x, y, .. } => {
                match self
                    .hits
                    .iter()
                    .rev()
                    .find(|(rect, _)| rect.contains(x, y))
                    .map(|(_, hit)| *hit)
                {
                    Some(Hit::Unlock) => self.submit(now),
                    Some(Hit::Cancel) => self.cancel(),
                    None => Outcome::Ignored,
                }
            }
            _ => Outcome::Ignored,
        }
    }

    fn words(&self) -> (&'static str, String) {
        let who = if self.asker.is_empty() {
            "A program".to_owned()
        } else {
            self.asker.clone()
        };
        match self.mode {
            Mode::Create => (
                "Set up your keyring",
                format!(
                    "{who} wants to keep a password or token. They're kept sealed with your login password: type it to begin."
                ),
            ),
            Mode::Unlock => (
                "Unlock your keyring",
                format!("{who} wants a password or token kept there. Your login password opens it."),
            ),
            Mode::Lost => (
                "This machine can't open your keyring",
                "What sealed it here no longer opens it — the TPM was cleared, or this is another machine. Your login password starts a new, empty keyring; the old one is kept beside it."
                    .to_owned(),
            ),
        }
    }

    pub fn render(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let presence = self.presence.get(now);
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let look = Look::new(&colors, accent, self.font(type_scale::BODY));
        let (body, small) = (self.font(type_scale::BODY), self.font(type_scale::SMALL));
        let inner_w = WIDTH - 2.0 * space::XL;
        self.hits.clear();
        let (title, about) = self.words();
        let title_text = Text::new(title, self.font(type_scale::TITLE * 1.1), colors.text)
            .medium()
            .max_width(inner_w - BADGE - space::MD);
        let about_text = Text::new(&about, body, colors.subtle)
            .max_width(inner_w)
            .max_lines(5);
        let about_h = canvas.measure(&about_text).h;
        let height = 2.0 * space::XL
            + BADGE
            + space::MD
            + about_h
            + space::LG
            + FIELD
            + space::SM
            + small * 1.5
            + space::MD
            + controls::HEIGHT;
        let inner = controls::dialog(canvas, bounds_of(canvas), WIDTH, height, &look, presence);
        canvas.set_opacity(presence);

        let badge = Rect::new(inner.x, inner.y, BADGE, BADGE);
        let (bx, by) = badge.center();
        canvas.circle(bx, by, BADGE / 2.0, accent.alpha(opacity::TINT));
        canvas.icon(icon::LOCK, 20.0, accent, badge, Align::Center);
        canvas.text_in(
            &title_text,
            Rect::new(
                badge.right() + space::MD,
                inner.y,
                inner_w - BADGE - space::MD,
                BADGE,
            ),
            Align::Start,
        );
        let mut y = inner.y + BADGE + space::MD;
        canvas.text(&about_text, inner.x, y);
        y += about_h + space::LG;

        let t = self.shake.get(now);
        let shake = if t < 1.0 {
            (t * PI * 6.0).sin() * (1.0 - t) * 10.0
        } else {
            0.0
        };
        let field = Rect::new(inner.x + shake, y, inner_w, FIELD);
        let complaint = self.complaint.is_some();
        canvas.fill(field, radius::pill(FIELD), colors.base);
        let ring = if complaint {
            colors.love.alpha(0.9)
        } else {
            accent.alpha(0.8)
        };
        canvas.stroke(field, radius::pill(FIELD), stroke::FOCUS, ring);
        let text_area = field.inset_xy(space::LG + 2.0, 0.0);
        let dots = self.dots.get(now);
        if dots <= 0.0 {
            let hint = match (self.waiting, self.mode) {
                (true, _) => "Checking…",
                (false, _) => "Your login password",
            };
            canvas.text_in(
                &Text::new(hint, body, colors.muted),
                text_area,
                Align::Start,
            );
        } else {
            let ink = if self.waiting {
                colors.subtle
            } else {
                colors.text
            };
            let (_, center_y) = field.center();
            for index in 0..dots.ceil() as usize {
                let grown = (dots - index as f32).clamp(0.0, 1.0);
                let x = text_area.x + index as f32 * (DOT * 2.0) + DOT / 2.0;
                canvas.circle(x, center_y, DOT / 2.0 * grown, ink);
            }
        }
        y += FIELD + space::SM;
        let line = Rect::new(inner.x, y, inner_w, small * 1.5);
        if let Some(words) = &self.complaint {
            canvas.text_in(
                &Text::new(words, small, colors.love).max_width(inner_w),
                line,
                Align::Start,
            );
        } else if self.caps_lock {
            canvas.icon(
                icon::CAPS_LOCK,
                small + 2.0,
                colors.gold,
                line,
                Align::Start,
            );
            let words = Rect::new(line.x + small + 2.0 + space::SM, line.y, line.w, line.h);
            canvas.text_in(
                &Text::new("Caps Lock is on", small, colors.subtle),
                words,
                Align::Start,
            );
        }

        let row_y = inner.bottom() - controls::HEIGHT;
        let label = match self.mode {
            Mode::Create => "Set up",
            Mode::Unlock => "Unlock",
            Mode::Lost => "Start anew",
        };
        let primary = Button::new(label, Kind::Primary);
        let w = primary.width(canvas, &look);
        let rect = Rect::new(inner.right() - w, row_y, w, controls::HEIGHT);
        primary.draw(
            canvas,
            rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::Unlock),
                disabled: self.waiting || self.typed.is_empty(),
                ..Interaction::default()
            },
        );
        self.hits.push((rect, Hit::Unlock));
        let cancel = Button::new("Not now", Kind::Secondary);
        let w2 = cancel.width(canvas, &look);
        let cancel_rect = Rect::new(rect.x - space::SM - w2, row_y, w2, controls::HEIGHT);
        cancel.draw(
            canvas,
            cancel_rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::Cancel),
                ..Interaction::default()
            },
        );
        self.hits.push((cancel_rect, Hit::Cancel));
        canvas.set_opacity(1.0);
        Paint::when(
            self.presence.is_animating(now)
                || self.dots.is_animating(now)
                || self.shake.is_animating(now),
        )
    }
}

fn bounds_of(canvas: &Canvas<'_>) -> Rect {
    canvas.bounds()
}

impl App for Card {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::overlay("enso-keyring")
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        if self.out.is_none() {
            return;
        }
        // The keyring's answers, after each try.
        thread::spawn(move || {
            let stdin = io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                match stdin.lock().read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        proxy.send(Message::Gone);
                        return;
                    }
                    Ok(_) => {}
                }
                let line = line.trim_end();
                if line == "ok" {
                    proxy.send(Message::Done);
                    return;
                }
                if let Some(rest) = line.strip_prefix("again ") {
                    let (mode, words) = rest.split_once(' ').unwrap_or((rest, ""));
                    let words = unhex(words)
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                        .unwrap_or_default();
                    if let Some(mode) = Mode::from_word(mode) {
                        proxy.send(Message::Again(mode, words));
                    }
                }
            }
        });
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let now = Instant::now();
        let outcome = match event {
            Event::Wake if self.leaving && self.presence.is_closing() => {
                cx.exit();
                return;
            }
            Event::Message(message) => self.message(message, now),
            Event::Key(key) if !self.leaving => self.key(&key, now),
            Event::Pointer(pointer) if !self.leaving => self.pointer(pointer, now),
            Event::Close => self.cancel(),
            _ => Outcome::Ignored,
        };
        match outcome {
            Outcome::Ignored => {}
            Outcome::Redraw => cx.redraw(),
            Outcome::Close => self.presence.close(cx),
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        self.render(canvas, now)
    }
}

/// Read what the keyring sends before the card shows: its mode, and who's
/// asking.
pub fn read_request() -> (Mode, String) {
    let stdin = io::stdin();
    let mut mode = Mode::Unlock;
    let mut asker = String::new();
    for _ in 0..2 {
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if let Some(word) = line.strip_prefix("mode ") {
            mode = Mode::from_word(word).unwrap_or(Mode::Unlock);
        } else if let Some(name) = line.strip_prefix("asker ") {
            asker = name.to_owned();
        }
    }
    (mode, asker)
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_ui::{Mods, Offscreen, TextEngine};

    use super::*;

    fn card(mode: Mode) -> Card {
        Card::new(
            Context::load_from(Paths::rooted("/nonexistent/enso-keyring-test")),
            mode,
            "gh".into(),
            None,
        )
    }

    fn render(card: &mut Card, name: &str) {
        let mut text = TextEngine::new(&card.ctx.config.appearance);
        let now = Instant::now();
        Offscreen::render(&mut text, (760, 520), 1, |canvas| {
            card.render(canvas, now);
        });
        let later = now + std::time::Duration::from_secs(2);
        let frame = Offscreen::render(&mut text, (760, 520), 2, |canvas| {
            card.render(canvas, later);
        });
        frame
            .save_png(format!(
                "{}/../../target/snapshots/{name}.png",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap();
    }

    #[test]
    fn typed_sent_refused_and_asked_again() {
        let mut card = card(Mode::Unlock);
        let now = Instant::now();
        for c in "hunter2".chars() {
            card.key(
                &KeyEvent {
                    key: Key::Char(c),
                    mods: Mods::default(),
                    repeat: false,
                },
                now,
            );
        }
        assert_eq!(card.submit(now), Outcome::Redraw);
        assert!(card.waiting && card.typed.is_empty());
        card.message(
            Message::Again(Mode::Unlock, "That's not your login password".into()),
            now,
        );
        assert!(!card.waiting && card.shake.is_animating(now));
        render(&mut card, "enso-keyring-unlock");
        // A new question: the keyring can't be opened here anymore.
        card.message(Message::Again(Mode::Lost, String::new()), now);
        assert_eq!(card.mode, Mode::Lost);
        assert!(card.complaint.is_none());
        render(&mut card, "enso-keyring-lost");
        assert_eq!(card.message(Message::Done, now), Outcome::Close);
    }

    #[test]
    fn renders_the_first_time() {
        let mut card = card(Mode::Create);
        render(&mut card, "enso-keyring-create");
    }
}
