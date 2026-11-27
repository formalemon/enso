//! The keyring's card: one password field, in the same shape as
//! enso-auth's. Where a bar runs along the top of the screen it grows out
//! of the bar's clock, as the launcher does ([`enso_ui::emerge`]);
//! elsewhere it sits in the middle of the dimmed screen. It sends what's
//! typed to the keyring and waits; the keyring closes it, or says why not
//! and lets it try again.

use std::{
    io::{self, BufRead, Write},
    thread,
    time::Instant,
};

use enso_core::hex;
use enso_core::{
    Context,
    style::{icon, type_scale},
};
use enso_ui::{
    App, Canvas, Cx, Event, Key, KeyEvent, Paint, PointerEvent, Presence, Proxy, Rect, SurfaceSpec,
    emerge::Perch,
    widget::{Form, Interaction, Kind, Look, Note, SecretField, secret},
};
use zeroize::Zeroizing;

use crate::asking::Mode;

const MAX_PASSWORD: usize = 512;

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
    /// The screen it opens on, and the bar's clock there to grow out of.
    screen: Option<String>,
    perch: Perch,
    field: SecretField,
    /// Sent, waiting for the keyring's answer.
    waiting: bool,
    leaving: bool,
    complaint: Option<String>,
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
            screen: None,
            perch: Perch::default(),
            ctx,
            mode,
            asker,
            out,
            field: SecretField::new(MAX_PASSWORD),
            waiting: false,
            leaving: false,
            complaint: None,
            hits: Vec::new(),
            hover: None,
        }
    }

    /// Open on `screen`, out of the bar's clock there if `perch` has it.
    #[must_use]
    pub fn on(mut self, screen: Option<String>, perch: Perch) -> Self {
        self.screen = screen;
        self.presence = perch.presence(&self.ctx.motion);
        self.perch = perch;
        self
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

    fn submit(&mut self, now: Instant) -> Outcome {
        if self.waiting || self.field.is_empty() {
            return Outcome::Ignored;
        }
        let typed = self.field.take(&self.ctx.motion, now);
        let encoded = Zeroizing::new(hex::encode(typed.as_bytes()));
        let line = Zeroizing::new(format!("password {}", encoded.as_str()));
        self.say(&line);
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
                    self.field.refuse(&self.ctx.motion, now);
                }
                Outcome::Redraw
            }
        }
    }

    fn key(&mut self, key: &KeyEvent, now: Instant) -> Outcome {
        let quiet = if self.field.note_mods(key.mods) {
            Outcome::Redraw
        } else {
            Outcome::Ignored
        };
        match key.key {
            Key::Escape => return self.cancel(),
            Key::Enter => return self.submit(now),
            _ if self.waiting => return quiet,
            _ => {}
        }
        match self.field.key(key, &self.ctx.motion, now) {
            secret::Response::Changed => {
                self.complaint = None;
                Outcome::Redraw
            }
            _ => quiet,
        }
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
        let look = Look::new(
            &self.ctx.theme.colors,
            self.ctx.accent(),
            self.font(type_scale::BODY),
        );
        self.hits.clear();
        let (title, about) = self.words();
        let label = match self.mode {
            Mode::Create => "Set up",
            Mode::Unlock => "Unlock",
            Mode::Lost => "Start anew",
        };
        let said = self.complaint.as_deref().map(|words| (words, true));
        let form = Form::new(title)
            .badge(icon::LOCK, look.accent)
            .about(&about)
            .field()
            .note(Note::under(&self.field, said, self.waiting))
            .button(
                "Not now",
                Kind::Secondary,
                Interaction {
                    hovered: self.hover == Some(Hit::Cancel),
                    ..Interaction::default()
                },
            )
            .button(
                label,
                Kind::Primary,
                Interaction {
                    hovered: self.hover == Some(Hit::Unlock),
                    disabled: self.waiting || self.field.is_empty(),
                    ..Interaction::default()
                },
            );
        let placed = if self.perch.island.is_some() {
            // Nothing dimmed: it hangs under the bar, as what's summoned does.
            let (width, height) = form.size(canvas, &look);
            let rest = self.perch.rest(canvas.bounds(), width, height);
            let card = self
                .perch
                .card(canvas, rest, presence, &self.ctx.theme.colors);
            form.draw_in(canvas, card, &look)
        } else {
            form.draw(canvas, canvas.bounds(), &look, presence)
        };
        let hint = if self.waiting {
            "Checking…"
        } else {
            "Your login password"
        };
        let style = secret::Style {
            busy: self.waiting,
            complaint: self.complaint.is_some(),
            ..secret::Style::new(&look, hint)
        };
        self.field.draw(canvas, placed.parts[0], &style, now);
        self.hits.push((placed.buttons[0], Hit::Cancel));
        self.hits.push((placed.buttons[1], Hit::Unlock));
        canvas.set_opacity(1.0);
        Paint::when(self.presence.is_animating(now) || self.field.is_animating(now))
    }
}

impl App for Card {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        Perch::surface("enso-keyring", self.screen.clone())
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
                    let words = hex::decode(words)
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
        // The one held to a reference in `snapshots/`; the rest are only
        // written, to look at.
        if name == "enso-keyring-unlock" {
            frame.check(env!("CARGO_MANIFEST_DIR"), name);
        } else {
            frame.snapshot(env!("CARGO_MANIFEST_DIR"), name);
        }
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
        assert!(card.waiting && card.field.is_empty());
        card.message(
            Message::Again(Mode::Unlock, "That's not your login password".into()),
            now,
        );
        assert!(!card.waiting && card.field.is_animating(now));
        render(&mut card, "enso-keyring-unlock");
        // A new question: the keyring can't be opened here anymore.
        card.message(Message::Again(Mode::Lost, String::new()), now);
        assert_eq!(card.mode, Mode::Lost);
        assert!(card.complaint.is_none());
        render(&mut card, "enso-keyring-lost");
        assert_eq!(card.message(Message::Done, now), Outcome::Close);
    }

    /// Opened over a bar, it grows out of the clock and hangs under it, in
    /// the middle: `target/snapshots/enso-keyring-emerge.png`, a frame at a
    /// time.
    #[test]
    fn opens_out_of_the_bars_clock() {
        use enso_ui::{Align, Text};
        let (width, height) = (800u32, 320u32);
        let island = enso_core::island::Island {
            pid: 1,
            top: true,
            bar: 36.0,
            x: 300.0,
            y: 4.0,
            w: 200.0,
            h: 28.0,
            opacity: 0.9,
            ..enso_core::island::Island::default()
        };
        let mut card = card(Mode::Unlock).on(
            None,
            Perch {
                island: Some(island),
                ..Perch::default()
            },
        );
        let colors = card.ctx.theme.colors.clone();
        let mut text = TextEngine::new(&card.ctx.config.appearance);
        let desk = Offscreen::render(&mut text, (width, height), 1, |canvas| {
            canvas.clear(colors.pine.mix(colors.overlay, 0.6));
            let clock = Rect::new(island.x, island.y, island.w, island.h);
            canvas.pill(clock, colors.base.alpha(island.opacity));
            let words = Text::new("Thu 26 Nov  17:40", 13.0, colors.text).medium();
            canvas.text_in(&words, clock, Align::Center);
        });
        let start = Instant::now();
        let moments = [0u64, 60, 130, 250, 420, 700];
        let mut sheet = Vec::new();
        for (n, ms) in moments.into_iter().enumerate() {
            let at = start + std::time::Duration::from_millis(ms);
            let frame = Offscreen::render(&mut text, (width, height), 1, |canvas| {
                card.render(canvas, at);
            });
            if n == 0 {
                // Nothing at all before it has begun: no dimming either.
                assert!(frame.rgba.iter().all(|&byte| byte == 0));
            }
            let mut over = desk.rgba.clone();
            for (dst, src) in over.chunks_mut(4).zip(frame.rgba.chunks(4)) {
                let keep = 255 - u32::from(src[3]);
                for c in 0..4 {
                    dst[c] = (u32::from(src[c]) + (u32::from(dst[c]) * keep + 127) / 255) as u8;
                }
            }
            sheet.extend(over);
        }
        let sheet = Offscreen {
            width,
            height: height * moments.len() as u32,
            rgba: sheet,
        };
        sheet.snapshot(env!("CARGO_MANIFEST_DIR"), "enso-keyring-emerge");
        // At rest, under the bar and in the middle: its buttons are there.
        let buttons: Vec<Rect> = card.hits.iter().map(|(rect, _)| *rect).collect();
        assert!(buttons.iter().all(|rect| rect.y > 40.0 && rect.right() < 620.0));
        // Nothing dimmed beside it.
        let last = (moments.len() as u32 - 1) * height;
        assert_eq!(sheet.pixel(40, last + 200), desk.pixel(40, 200));
    }

    #[test]
    fn renders_the_first_time() {
        let mut card = card(Mode::Create);
        render(&mut card, "enso-keyring-create");
    }
}
