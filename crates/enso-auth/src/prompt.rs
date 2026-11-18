//! The prompt: one card over a dimmed screen, asking one thing.
//!
//! For polkit, a password: typed as dots, checked by polkit's helper when
//! Enter is pressed, the field shaking if it was wrong. For Bluetooth, a
//! code to compare or type, a PIN or passkey to enter, or a yes or no.
//! Enter answers yes, Esc no; the mouse works too. It takes the keyboard
//! while it's up and isn't dismissed by a stray click outside.
//!
//! The answer is one line on standard output (see [`crate::request`]).

use std::{
    f32::consts::PI,
    io::{self, BufRead, Write},
    sync::mpsc,
    thread,
    time::Instant,
};

use enso_core::{
    Context, Easing, Transition, Tween,
    style::{icon, opacity, radius, space, stroke, type_scale},
};
use enso_ui::{
    Align, App, Canvas, Cx, Event, Font, Key, KeyEvent, Paint, PointerEvent, Presence, Proxy, Rect,
    SurfaceSpec, Text,
    widget::{Button, Interaction, Kind, Look, controls},
};

use crate::{
    helper::{self, Said, Verdict},
    request::{Answer, Request},
};

const WIDTH: f32 = 440.0;
const BADGE: f32 = 40.0;
const FIELD: f32 = 44.0;
const DOT: f32 = 8.0;
const DOT_GAP: f32 = 8.0;
/// Dots drawn at most: a longer password stops growing, which also keeps
/// its length to itself.
const MAX_DOTS: usize = 20;
/// Characters taken: PAM takes no more than this anyway.
const MAX_SECRET: usize = 256;
const MAX_PIN: usize = 16;
const PASSKEY_DIGITS: usize = 6;
/// The field's refusal: a quick sideways shake that dies away.
const SHAKE: Transition = Transition::new(380, Easing::Linear);

#[derive(Debug)]
pub enum Message {
    /// How the password check went.
    Verdict(Result<Verdict, String>),
    /// PAM says something along the way, or asks something more.
    Said(Said),
    /// The agent took the question back.
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Primary,
    Secondary,
    User(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Asking,
    /// The helper has the password.
    Checking,
    /// Answered; fading out.
    Leaving,
}

/// What the field takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    /// Shown as dots.
    Secret,
    /// Shown as typed.
    Plain(usize),
    Digits(usize),
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Ignored,
    Redraw,
    Close,
}

pub struct Prompt {
    ctx: Context,
    request: Request,
    /// Shown by `enso-auth try`: nothing is checked or sent anywhere.
    preview: bool,
    out: Option<Box<dyn Write>>,
    answered: Option<Answer>,
    presence: Presence,
    phase: Phase,
    typed: String,
    dots: Tween<f32>,
    /// Which of the users is answering.
    user: usize,
    /// A line under the field: what went wrong (`true`), or a note.
    notice: Option<(String, bool)>,
    shake: Tween<f32>,
    caps_lock: bool,
    /// A further question from PAM: its words, and whether typing shows.
    further: Option<(String, bool)>,
    reply: Option<mpsc::Sender<Option<String>>>,
    proxy: Option<Proxy<Message>>,
    hits: Vec<(Rect, Hit)>,
    hover: Option<Hit>,
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never what's typed.
        f.debug_struct("Prompt")
            .field("request", &self.request.is_bluetooth())
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl Prompt {
    pub fn new(ctx: Context, request: Request, out: Option<Box<dyn Write>>, preview: bool) -> Self {
        Self {
            presence: enso_ui::Presence::new(&ctx.motion),
            ctx,
            request,
            preview,
            out,
            answered: None,
            phase: Phase::Asking,
            // Room enough that typing never moves it, leaving copies behind.
            typed: String::with_capacity(MAX_SECRET * 4),
            dots: Tween::new(0.0),
            user: 0,
            notice: None,
            shake: Tween::new(1.0),
            caps_lock: false,
            further: None,
            reply: None,
            proxy: None,
            hits: Vec::new(),
            hover: None,
        }
    }

    fn font(&self, scale: f32) -> f32 {
        (self.ctx.config.appearance.font_size * scale).round()
    }

    fn look(&self) -> Look {
        Look::new(
            &self.ctx.theme.colors,
            self.ctx.accent(),
            self.font(type_scale::BODY),
        )
    }

    fn field(&self) -> Option<Field> {
        match (&self.request, &self.further) {
            (Request::Password { .. }, Some((_, true))) => Some(Field::Plain(MAX_SECRET)),
            (Request::Password { .. }, _) => Some(Field::Secret),
            (Request::Pin { .. }, _) => Some(Field::Plain(MAX_PIN)),
            (Request::Passkey { .. }, _) => Some(Field::Digits(PASSKEY_DIGITS)),
            _ => None,
        }
    }

    fn users(&self) -> &[String] {
        match &self.request {
            Request::Password { users, .. } => users,
            _ => &[],
        }
    }

    /// The two buttons' words: the "no", and the "yes" (if there is one).
    fn buttons(&self) -> (&'static str, Option<&'static str>) {
        match &self.request {
            Request::Password { .. } if self.phase == Phase::Checking => {
                ("Cancel", Some("Checking…"))
            }
            Request::Password { .. } => ("Cancel", Some("Allow")),
            Request::Confirm { .. } => ("They differ", Some("Pair")),
            Request::Authorize { .. } => ("Don't pair", Some("Pair")),
            Request::Service { .. } => ("Don't allow", Some("Allow")),
            Request::Pin { .. } | Request::Passkey { .. } => ("Cancel", Some("Pair")),
            Request::Show { .. } => ("Cancel", None),
        }
    }

    fn set_dots(&mut self, now: Instant) {
        let count = self.typed.chars().count().min(MAX_DOTS) as f32;
        self.dots
            .set(count, self.ctx.motion.apply(Transition::HOVER), now);
    }

    fn clear(&mut self, now: Instant) {
        helper::wipe(std::mem::replace(
            &mut self.typed,
            String::with_capacity(MAX_SECRET * 4),
        ));
        self.set_dots(now);
    }

    /// Say `answer`, once, and go.
    fn answer(&mut self, answer: Answer) -> Outcome {
        if self.answered.is_some() {
            return Outcome::Ignored;
        }
        if let Some(out) = &mut self.out {
            let _ = writeln!(out, "{}", answer.line());
            let _ = out.flush();
        }
        // A conversation still waiting on a further question is given up.
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(None);
        }
        self.answered = Some(answer);
        self.phase = Phase::Leaving;
        Outcome::Close
    }

    fn primary(&mut self, now: Instant) -> Outcome {
        if self.phase != Phase::Asking {
            return Outcome::Ignored;
        }
        match &self.request {
            Request::Password { users, cookie, .. } => {
                let (user, cookie) = (users.get(self.user).cloned(), cookie.clone());
                if self.typed.is_empty() {
                    return Outcome::Ignored;
                }
                let typed =
                    std::mem::replace(&mut self.typed, String::with_capacity(MAX_SECRET * 4));
                self.notice = None;
                self.phase = Phase::Checking;
                self.set_dots(now);
                if self.further.take().is_some() {
                    // The answer to PAM's further question.
                    match &self.reply {
                        Some(reply) => drop(reply.send(Some(typed))),
                        None => helper::wipe(typed),
                    }
                    return Outcome::Redraw;
                }
                if self.preview {
                    helper::wipe(typed);
                    return self.answer(Answer::Allow);
                }
                let (Some(user), Some(proxy)) = (user, self.proxy.clone()) else {
                    helper::wipe(typed);
                    return self.answer(Answer::Deny);
                };
                let (reply, replies) = mpsc::channel();
                self.reply = Some(reply);
                thread::spawn(move || {
                    let verdict = helper::check(&user, &cookie, typed, |said| {
                        let asks = matches!(said, Said::Ask { .. });
                        proxy.send(Message::Said(said));
                        if asks {
                            replies.recv().ok().flatten()
                        } else {
                            None
                        }
                    });
                    proxy.send(Message::Verdict(verdict.map_err(|err| err.to_string())));
                });
                Outcome::Redraw
            }
            Request::Pin { .. } if !self.typed.is_empty() => {
                let pin = std::mem::take(&mut self.typed);
                self.answer(Answer::Pin(pin))
            }
            Request::Passkey { .. } => match self.typed.parse() {
                Ok(passkey) => self.answer(Answer::Passkey(passkey)),
                Err(_) => Outcome::Ignored,
            },
            Request::Confirm { .. } | Request::Authorize { .. } | Request::Service { .. } => {
                self.answer(Answer::Allow)
            }
            _ => Outcome::Ignored,
        }
    }

    fn message(&mut self, message: Message, now: Instant) -> Outcome {
        match message {
            Message::Close => self.answer(Answer::Deny),
            Message::Said(Said::Ask { text, echo }) => {
                self.further = Some((text.trim().trim_end_matches(':').to_owned(), echo));
                self.phase = Phase::Asking;
                self.notice = None;
                self.clear(now);
                Outcome::Redraw
            }
            Message::Said(Said::Info(text)) => {
                self.notice = Some((text, false));
                Outcome::Redraw
            }
            Message::Said(Said::Error(text)) => {
                self.notice = Some((text, true));
                Outcome::Redraw
            }
            Message::Verdict(verdict) => {
                self.reply = None;
                self.further = None;
                if self.phase == Phase::Leaving {
                    return Outcome::Ignored;
                }
                match verdict {
                    Ok(Verdict::Accepted) => return self.answer(Answer::Allow),
                    Ok(Verdict::Abandoned) => return self.answer(Answer::Deny),
                    Ok(Verdict::Refused) => {
                        // PAM's own words win: "the account is locked…".
                        if !self
                            .notice
                            .as_ref()
                            .is_some_and(|(_, complaint)| *complaint)
                        {
                            self.notice =
                                Some(("That's not the password. Try again.".to_owned(), true));
                        }
                        self.shake.jump(0.0);
                        self.shake.set(1.0, self.ctx.motion.apply(SHAKE), now);
                    }
                    Err(err) => self.notice = Some((format!("Can't check it: {err}"), true)),
                }
                self.phase = Phase::Asking;
                self.clear(now);
                Outcome::Redraw
            }
        }
    }

    fn key(&mut self, key: &KeyEvent, now: Instant) -> Outcome {
        let caps_changed =
            std::mem::replace(&mut self.caps_lock, key.mods.caps_lock) != key.mods.caps_lock;
        let quiet = if caps_changed {
            Outcome::Redraw
        } else {
            Outcome::Ignored
        };
        match key.key {
            Key::Escape => return self.answer(Answer::Deny),
            Key::Enter => return self.primary(now),
            _ if self.phase != Phase::Asking => return quiet,
            Key::Tab | Key::Left | Key::Right
                if self.users().len() > 1 && self.further.is_none() =>
            {
                let count = self.users().len();
                self.user = if key.key == Key::Left || (key.key == Key::Tab && key.mods.shift) {
                    (self.user + count - 1) % count
                } else {
                    (self.user + 1) % count
                };
                self.notice = None;
                self.clear(now);
                return Outcome::Redraw;
            }
            _ => {}
        }
        let Some(field) = self.field() else {
            return quiet;
        };
        match key.key {
            Key::Char('u') if key.mods.ctrl => self.clear(now),
            Key::Backspace if key.mods.ctrl => self.clear(now),
            Key::Backspace => {
                if self.typed.pop().is_none() {
                    return quiet;
                }
                self.set_dots(now);
            }
            _ => {
                let Some(c) = key.text() else { return quiet };
                let count = self.typed.chars().count();
                let fits = match field {
                    Field::Secret => count < MAX_SECRET,
                    Field::Plain(max) => count < max && !c.is_control(),
                    Field::Digits(max) => count < max && c.is_ascii_digit(),
                };
                if !fits {
                    return quiet;
                }
                self.typed.push(c);
                self.set_dots(now);
            }
        }
        // Typing again takes back the complaint.
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, complaint)| *complaint)
        {
            self.notice = None;
        }
        Outcome::Redraw
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, hit)| *hit)
    }

    fn pointer(&mut self, event: PointerEvent, now: Instant) -> Outcome {
        match event {
            PointerEvent::Enter { x, y } | PointerEvent::Move { x, y } => {
                let hover = self.hit(x, y);
                if std::mem::replace(&mut self.hover, hover) == hover {
                    Outcome::Ignored
                } else {
                    Outcome::Redraw
                }
            }
            PointerEvent::Leave => {
                self.hover = None;
                Outcome::Redraw
            }
            PointerEvent::Press { x, y, .. } => match self.hit(x, y) {
                Some(Hit::Primary) => self.primary(now),
                Some(Hit::Secondary) => self.answer(Answer::Deny),
                Some(Hit::User(index)) if self.phase == Phase::Asking && self.further.is_none() => {
                    self.user = index;
                    self.clear(now);
                    Outcome::Redraw
                }
                _ => Outcome::Ignored,
            },
            _ => Outcome::Ignored,
        }
    }

    /// The words: title, what it's about, and who's asking.
    fn words(&self) -> (String, String, Option<String>) {
        match &self.request {
            Request::Password {
                message,
                action,
                program,
                ..
            } => (
                "Confirm it's you".to_owned(),
                message.clone(),
                Some(match program {
                    Some(program) => format!("Asked by {program}"),
                    None => action.clone(),
                }),
            ),
            Request::Confirm { device, .. } => (
                format!("Pair with {device}?"),
                format!("Pair if {device} shows the same number."),
                None,
            ),
            Request::Authorize { device } => (
                format!("Pair with {device}?"),
                "It wants to pair without a code. Pair only if you started this.".to_owned(),
                None,
            ),
            Request::Service { device, service } => (
                format!("Let {device} use {service}?"),
                "It isn't trusted yet. Allow it only if you know this device.".to_owned(),
                None,
            ),
            Request::Pin { device } => (
                format!("PIN for {device}"),
                format!("Type the PIN {device} wants. Older headsets and car kits often use 0000."),
                None,
            ),
            Request::Passkey { device } => (
                format!("Code from {device}"),
                format!("Type the six-digit number {device} shows."),
                None,
            ),
            Request::Show { device, .. } => (
                format!("Type this on {device}"),
                "Then press Enter on it. This goes away once it's paired.".to_owned(),
                None,
            ),
        }
    }

    /// The big number to compare or type, spaced in threes.
    fn code(&self) -> Option<String> {
        let digits = match &self.request {
            Request::Confirm { passkey, .. } => format!("{passkey:06}"),
            Request::Show { code, .. } => code.clone(),
            _ => return None,
        };
        let chars: Vec<char> = digits.chars().collect();
        Some(
            chars
                .chunks(3)
                .map(|chunk| chunk.iter().collect::<String>())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    pub fn render(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let presence = self.presence.get(now);
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let look = self.look();
        let bounds = canvas.bounds();
        let (body, small) = (self.font(type_scale::BODY), self.font(type_scale::SMALL));
        let inner_w = WIDTH - 2.0 * space::XL;
        self.hits.clear();

        // Measure first: the card is as tall as what it says.
        let (title, about, meta) = self.words();
        let title_text = Text::new(&title, self.font(type_scale::TITLE * 1.1), colors.text)
            .medium()
            .max_width(inner_w - BADGE - space::MD)
            .max_lines(2);
        let about_text = Text::new(&about, body, colors.subtle)
            .max_width(inner_w)
            .max_lines(5);
        let meta_text = meta.as_deref().map(|meta| {
            Text::new(meta, small, colors.muted)
                .max_width(inner_w)
                .max_lines(2)
        });
        let code = self.code();
        let code_size = self.font(type_scale::HEADING * 1.6);
        let title_h = canvas.measure(&title_text).h.max(BADGE);
        let about_h = canvas.measure(&about_text).h;
        let meta_h = meta_text
            .as_ref()
            .map_or(0.0, |text| canvas.measure(text).h + space::XS);
        let users_h = if self.users().is_empty() {
            0.0
        } else {
            space::LG + 30.0
        };
        let code_h = if code.is_some() {
            space::LG + code_size * 1.4
        } else {
            0.0
        };
        let field_h = if self.field().is_some() {
            space::LG + FIELD
        } else {
            0.0
        };
        let notice_h = space::SM + small * 1.5;
        let height = 2.0 * space::XL
            + title_h
            + space::MD
            + about_h
            + meta_h
            + users_h
            + code_h
            + field_h
            + notice_h
            + space::MD
            + controls::HEIGHT;
        let inner = controls::dialog(canvas, bounds, WIDTH, height, &look, presence);
        canvas.set_opacity(presence);

        // The badge: a lock, or Bluetooth's mark.
        let (badge_icon, badge_color) = if self.request.is_bluetooth() {
            (icon::BLUETOOTH, colors.foam)
        } else {
            (icon::LOCK, accent)
        };
        let badge = Rect::new(inner.x, inner.y + (title_h - BADGE) / 2.0, BADGE, BADGE);
        let (bx, by) = badge.center();
        canvas.circle(bx, by, BADGE / 2.0, badge_color.alpha(opacity::TINT));
        canvas.icon(badge_icon, 20.0, badge_color, badge, Align::Center);
        let title_size = canvas.measure(&title_text);
        canvas.text(
            &title_text,
            badge.right() + space::MD,
            inner.y + (title_h - title_size.h) / 2.0,
        );
        let mut y = inner.y + title_h + space::MD;
        canvas.text(&about_text, inner.x, y);
        y += about_h;
        if let Some(meta) = &meta_text {
            y += space::XS;
            canvas.text(meta, inner.x, y);
            y += canvas.measure(meta).h;
        }

        // Who answers: their name, or a choice of names.
        if !self.users().is_empty() {
            y += space::LG;
            let label = Text::new("As", small, colors.muted);
            let label_w = canvas.measure(&label).w;
            canvas.text_in(&label, Rect::new(inner.x, y, label_w, 30.0), Align::Start);
            let mut x = inner.x + label_w + space::SM;
            let users = self.users().to_vec();
            let choosable = users.len() > 1 && self.further.is_none();
            for (index, name) in users.iter().enumerate() {
                let chosen = index == self.user;
                let ink = if chosen { look.base } else { colors.subtle };
                let text = Text::new(name, small, ink);
                let w = canvas.measure(&text).w + 2.0 * space::MD;
                if x + w > inner.right() {
                    break;
                }
                let pill = Rect::new(x, y + 2.0, w, 26.0);
                let fill = if chosen {
                    accent
                } else if self.hover == Some(Hit::User(index)) {
                    colors.highlight_med
                } else {
                    colors.overlay
                };
                canvas.fill(pill, radius::pill(pill.h), fill);
                canvas.text_in(&text, pill, Align::Center);
                if choosable {
                    self.hits.push((pill, Hit::User(index)));
                }
                x = pill.right() + space::XS;
            }
            y += 30.0;
        }

        if let Some(code) = &code {
            y += space::LG;
            let text = Text::new(code, code_size, colors.text).font(Font::Mono);
            let size = canvas.measure(&text);
            canvas.text(
                &text,
                inner.x + (inner.w - size.w) / 2.0,
                y + (code_size * 1.4 - size.h) / 2.0,
            );
            y += code_size * 1.4;
        }

        let mut caps_note = false;
        if let Some(field) = self.field() {
            y += space::LG;
            let t = self.shake.get(now);
            let shake = if t < 1.0 {
                (t * PI * 6.0).sin() * (1.0 - t) * 10.0
            } else {
                0.0
            };
            let rect = Rect::new(inner.x + shake, y, inner.w, FIELD);
            let complaint = self
                .notice
                .as_ref()
                .is_some_and(|(_, complaint)| *complaint);
            let dots = self.dots.get(now);
            let typed = !self.typed.is_empty() || dots > 0.0;
            canvas.fill(rect, radius::pill(FIELD), colors.base);
            let (width, color) = if complaint {
                (stroke::FOCUS, colors.love.alpha(0.9))
            } else if self.phase == Phase::Asking {
                (stroke::FOCUS, accent.alpha(if typed { 0.9 } else { 0.55 }))
            } else {
                (stroke::HAIRLINE, colors.highlight_high)
            };
            canvas.stroke(rect, radius::pill(FIELD), width, color);
            let text_area = rect.inset_xy(space::LG + 2.0, 0.0);
            let ink = if self.phase == Phase::Checking {
                colors.subtle
            } else {
                colors.text
            };
            if !typed {
                let hint = match (&self.further, field) {
                    (Some((question, _)), _) => question.clone(),
                    (None, Field::Secret) => match self.users().get(self.user) {
                        Some(user) => format!("{user}'s password"),
                        None => "Password".to_owned(),
                    },
                    (None, Field::Digits(_)) => "000000".to_owned(),
                    (None, Field::Plain(_)) => "PIN".to_owned(),
                };
                let hint = if self.phase == Phase::Checking {
                    "Checking…".to_owned()
                } else {
                    hint
                };
                canvas.text_in(
                    &Text::new(&hint, body, colors.muted),
                    text_area,
                    Align::Start,
                );
            } else if field == Field::Secret {
                // The row grows from the left; the newest dot swells in.
                let whole = dots.ceil() as usize;
                let (_, center_y) = rect.center();
                for index in 0..whole {
                    let grown = (dots - index as f32).clamp(0.0, 1.0);
                    let x = text_area.x + index as f32 * (DOT + DOT_GAP) + DOT / 2.0;
                    canvas.circle(x, center_y, DOT / 2.0 * grown, ink);
                }
            } else {
                let font = if matches!(field, Field::Digits(_)) {
                    Font::Mono
                } else {
                    Font::Ui
                };
                let text = Text::new(&self.typed, body, ink).font(font);
                let size = canvas.text_in(&text, text_area, Align::Start);
                if self.phase == Phase::Asking {
                    let caret = Rect::new(
                        text_area.x + size.w + 2.0,
                        rect.y + 12.0,
                        stroke::FOCUS,
                        FIELD - 24.0,
                    );
                    canvas.fill(caret, 1.0, accent);
                }
            }
            caps_note = field == Field::Secret && self.caps_lock && self.notice.is_none();
            y += FIELD;
        }

        // A line for what went wrong, a note, or Caps Lock.
        y += space::SM;
        let line = Rect::new(inner.x, y, inner.w, small * 1.5);
        if let Some((text, complaint)) = &self.notice {
            let color = if *complaint {
                colors.love
            } else {
                colors.subtle
            };
            canvas.text_in(
                &Text::new(text, small, color).max_width(inner.w),
                line,
                Align::Start,
            );
        } else if caps_note {
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

        // The buttons, the "yes" rightmost.
        let (no, yes) = self.buttons();
        let row_y = inner.bottom() - controls::HEIGHT;
        let mut x = inner.right();
        if let Some(yes) = yes {
            let button = Button::new(yes, Kind::Primary);
            let w = button.width(canvas, &look);
            let rect = Rect::new(x - w, row_y, w, controls::HEIGHT);
            let disabled = self.phase != Phase::Asking
                || (self.field().is_some() && self.typed.is_empty())
                || (matches!(self.field(), Some(Field::Digits(_))) && self.typed.is_empty());
            button.draw(
                canvas,
                rect,
                &look,
                Interaction {
                    hovered: self.hover == Some(Hit::Primary),
                    disabled,
                    ..Interaction::default()
                },
            );
            self.hits.push((rect, Hit::Primary));
            x = rect.x - space::SM;
        }
        let button = Button::new(no, Kind::Secondary);
        let w = button.width(canvas, &look);
        let rect = Rect::new(x - w, row_y, w, controls::HEIGHT);
        button.draw(
            canvas,
            rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::Secondary),
                ..Interaction::default()
            },
        );
        self.hits.push((rect, Hit::Secondary));
        canvas.set_opacity(1.0);

        Paint::when(
            self.presence.is_animating(now)
                || self.dots.is_animating(now)
                || self.shake.is_animating(now),
        )
    }
}

impl App for Prompt {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::overlay("enso-auth")
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        if !self.preview {
            // The agent may take the question back; if it goes away, so does
            // the question.
            let closer = proxy.clone();
            thread::spawn(move || {
                let stdin = io::stdin();
                let mut line = String::new();
                loop {
                    line.clear();
                    match stdin.lock().read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if line.trim() == "close" => break,
                        Ok(_) => {}
                    }
                }
                closer.send(Message::Close);
            });
        }
        self.proxy = Some(proxy);
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let now = Instant::now();
        let leaving = self.phase == Phase::Leaving;
        let outcome = match event {
            Event::Wake if leaving && self.presence.is_closing() => {
                cx.exit();
                return;
            }
            Event::Message(message) => self.message(message, now),
            Event::Key(key) if !leaving => self.key(&key, now),
            Event::Pointer(event) if !leaving => self.pointer(event, now),
            Event::Close => self.answer(Answer::Deny),
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

impl Drop for Prompt {
    fn drop(&mut self) {
        helper::wipe(std::mem::take(&mut self.typed));
    }
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_ui::{Mods, Offscreen, TextEngine};

    use super::*;

    fn context() -> Context {
        Context::load_from(Paths::rooted("/nonexistent/enso-auth-test"))
    }

    fn prompt(request: Request) -> Prompt {
        Prompt::new(context(), request, None, true)
    }

    fn password() -> Request {
        Request::Password {
            message: "Authentication is required to mount Windows (1.0 TB NTFS).".into(),
            action: "org.freedesktop.udisks2.filesystem-mount-system".into(),
            program: Some("/usr/bin/enso-files".into()),
            users: vec!["ren".into(), "root".into()],
            cookie: "cookie".into(),
        }
    }

    fn press(prompt: &mut Prompt, key: Key) -> Outcome {
        let event = KeyEvent {
            key,
            mods: Mods::default(),
            repeat: false,
        };
        prompt.key(&event, Instant::now())
    }

    fn type_text(prompt: &mut Prompt, text: &str) {
        for c in text.chars() {
            press(prompt, Key::Char(c));
        }
    }

    fn render(prompt: &mut Prompt, name: &str) {
        let mut text = TextEngine::new(&prompt.ctx.config.appearance);
        // The first frame starts the reveal; the snapshot is after it.
        let now = Instant::now();
        Offscreen::render(&mut text, (760, 520), 1, |canvas| {
            prompt.render(canvas, now);
        });
        let later = now + std::time::Duration::from_secs(2);
        let frame = Offscreen::render(&mut text, (760, 520), 2, |canvas| {
            prompt.render(canvas, later);
        });
        frame
            .save_png(format!(
                "{}/../../target/snapshots/{name}.png",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap();
    }

    #[test]
    fn a_password_is_typed_and_sent() {
        let mut prompt = prompt(password());
        assert_eq!(
            press(&mut prompt, Key::Enter),
            Outcome::Ignored,
            "nothing typed"
        );
        type_text(&mut prompt, "hunter2");
        press(&mut prompt, Key::Backspace);
        assert_eq!(prompt.typed, "hunter");
        assert_eq!(press(&mut prompt, Key::Enter), Outcome::Close);
        assert_eq!(prompt.answered, Some(Answer::Allow));
        assert!(prompt.typed.is_empty());
    }

    #[test]
    fn a_wrong_password_shakes_and_is_cleared() {
        let mut prompt = prompt(password());
        prompt.preview = false;
        prompt.phase = Phase::Checking;
        let now = Instant::now();
        prompt.message(Message::Verdict(Ok(Verdict::Refused)), now);
        assert_eq!(prompt.phase, Phase::Asking);
        assert!(prompt.shake.is_animating(now));
        assert!(prompt.notice.as_ref().unwrap().1);
        // PAM's own complaint is kept over the plain one.
        prompt.phase = Phase::Checking;
        prompt.message(
            Message::Said(Said::Error("The account is locked.".into())),
            now,
        );
        prompt.message(Message::Verdict(Ok(Verdict::Refused)), now);
        assert_eq!(prompt.notice.as_ref().unwrap().0, "The account is locked.");
        render(&mut prompt, "enso-auth-password");
        // Typing takes it back.
        type_text(&mut prompt, "x");
        assert!(prompt.notice.is_none());
    }

    #[test]
    fn users_can_be_chosen_and_esc_says_no() {
        let mut prompt = prompt(password());
        type_text(&mut prompt, "abc");
        press(&mut prompt, Key::Tab);
        assert_eq!(prompt.user, 1);
        assert!(prompt.typed.is_empty(), "a new user starts afresh");
        press(&mut prompt, Key::Left);
        assert_eq!(prompt.user, 0);
        assert_eq!(press(&mut prompt, Key::Escape), Outcome::Close);
        assert_eq!(prompt.answered, Some(Answer::Deny));
        assert_eq!(
            press(&mut prompt, Key::Enter),
            Outcome::Ignored,
            "once only"
        );
    }

    #[test]
    fn a_further_question_is_asked_in_the_field() {
        let mut prompt = prompt(password());
        prompt.phase = Phase::Checking;
        let now = Instant::now();
        prompt.message(
            Message::Said(Said::Ask {
                text: "Verification code: ".into(),
                echo: true,
            }),
            now,
        );
        assert_eq!(prompt.field(), Some(Field::Plain(MAX_SECRET)));
        let (reply, replies) = mpsc::channel();
        prompt.reply = Some(reply);
        type_text(&mut prompt, "123456");
        prompt.primary(now);
        assert_eq!(replies.recv().unwrap().as_deref(), Some("123456"));
        assert_eq!(prompt.phase, Phase::Checking);
    }

    #[test]
    fn bluetooth_codes() {
        let mut passkey = prompt(Request::Passkey {
            device: "Keyboard".into(),
        });
        type_text(&mut passkey, "12a34567");
        assert_eq!(passkey.typed, "123456", "digits only, six of them");
        assert_eq!(press(&mut passkey, Key::Enter), Outcome::Close);
        assert_eq!(passkey.answered, Some(Answer::Passkey(123_456)));

        let mut pin = prompt(Request::Pin {
            device: "Car".into(),
        });
        type_text(&mut pin, "0000");
        press(&mut pin, Key::Enter);
        assert_eq!(pin.answered, Some(Answer::Pin("0000".into())));

        let mut confirm = prompt(Request::Confirm {
            device: "Pixel 9".into(),
            passkey: 42,
        });
        assert_eq!(confirm.code().as_deref(), Some("000 042"));
        press(&mut confirm, Key::Enter);
        assert_eq!(confirm.answered, Some(Answer::Allow));

        let mut show = prompt(Request::Show {
            device: "Keyboard".into(),
            code: "482913".into(),
        });
        assert_eq!(
            press(&mut show, Key::Enter),
            Outcome::Ignored,
            "nothing to say yes to"
        );
        assert_eq!(show.message(Message::Close, Instant::now()), Outcome::Close);
    }

    #[test]
    fn renders_each_kind() {
        let mut fresh = prompt(password());
        fresh.caps_lock = true;
        render(&mut fresh, "enso-auth-password-empty");
        let mut confirm = prompt(Request::Confirm {
            device: "Sony WH-1000XM5".into(),
            passkey: 482_913,
        });
        render(&mut confirm, "enso-auth-confirm");
        let mut pin = prompt(Request::Pin {
            device: "Car kit".into(),
        });
        type_text(&mut pin, "00");
        render(&mut pin, "enso-auth-pin");
        let mut show = prompt(Request::Show {
            device: "MX Keys".into(),
            code: "039264".into(),
        });
        render(&mut show, "enso-auth-show");
        let mut service = prompt(Request::Service {
            device: "Pixel 9".into(),
            service: "the network".into(),
        });
        render(&mut service, "enso-auth-service");
    }
}
